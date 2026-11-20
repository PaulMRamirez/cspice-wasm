#!/usr/bin/env bash
#
# Measurements for the WASI (Wasm MVP) CSPICE build, as recorded in
# docs/wasi-mvp-measurements.md. Prints everything to stdout; CI tees it into an
# artifact. Run scripts/build-cspice-wasi.sh first.
#
# Usage: bash scripts/wasi-measure.sh   (or: pnpm bench:spacewasm)
# Requires: node, cargo (Rust 1.87 or newer). If the `spacewasm` ground tool
# (cargo install spacewasm_tools --version =0.8.0-rc.3 --locked) is on PATH, its
# `validate` and `run` results are recorded too; otherwise those sections are skipped.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"
REACTOR=wasm/cspice.wasi.wasm
DRIVER=wasm/cspice-driver.wasi.wasm
HOST_OUT="$(mktemp)"
trap 'rm -f "$HOST_OUT"' EXIT
status=0

section() { printf '\n==== %s ====\n' "$1"; }

section "host"
uname -srm
grep -m1 'model name' /proc/cpuinfo 2>/dev/null | sed 's/^[^:]*: *//'
echo "cpus: $(nproc 2>/dev/null)"
node --version
cargo --version
"${CC:-clang}" --version | head -n 1

section "static module facts (scripts/check-wasm-mvp.mjs)"
node scripts/check-wasm-mvp.mjs "$REACTOR" "$DRIVER" || status=1
node scripts/check-wasm-mvp.mjs --json "$REACTOR" || status=1

if command -v spacewasm >/dev/null 2>&1; then
  section "spacewasm features"
  spacewasm features

  section "spacewasm validate, flight defaults (expected to fail: the module is too large)"
  spacewasm validate "$REACTOR"
  echo "exit status: $?"

  section "spacewasm validate, limits raised until the load fits"
  spacewasm validate --verbose --max-code-pages 65536 --heap-page-size 65536 --heap-pages 4096 \
    "$REACTOR" "$DRIVER" || status=1

  section "spacewasm run on the command driver (fails in 0.8.0-rc.3: no fd_renumber binding)"
  spacewasm run -d test/fixtures::/kernels "$DRIVER" /kernels/naif0012.tls \
    /kernels/de440s-inner-cassini.bsp
  echo "exit status: $?"
else
  section "spacewasm ground tool not on PATH: validate and run sections skipped"
fi

section "SpaceWasm host harness (bench/spacewasm-host)"
cargo build --release --locked --manifest-path bench/spacewasm-host/Cargo.toml || exit 1
HARNESS=bench/spacewasm-host/target/release/cspice-spacewasm-host
"$HARNESS" --module "$REACTOR" --fixtures test/fixtures --json wasi-measurements.json |
  tee "$HOST_OUT" || status=1

section "interpreter operand stack probe (32-bit words; the F Prime default is 1024)"
for words in 128 256 512 1024; do
  if "$HARNESS" --module "$REACTOR" --fixtures test/fixtures --skip-exact --stack-words "$words" \
    >/dev/null 2>&1; then
    echo "$words words: scenario completes"
  else
    echo "$words words: scenario fails (stack overflow trap)"
  fi
done

section "values against the Emscripten oracle (scripts/wasi-smoke.mjs)"
node --no-warnings scripts/wasi-smoke.mjs --host-output "$HOST_OUT" || status=1

exit "$status"
