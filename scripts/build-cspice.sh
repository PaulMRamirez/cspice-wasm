#!/usr/bin/env bash
#
# Reproducible CSPICE-WASM build for cspice-wasm.
#
# Vendors NASA/JPL CSPICE via the arturania/cspice fork, pinned to one commit so
# the build is reproducible, compiles it to a static library with Emscripten,
# then links the exported SPICE surface into an ES module plus a .wasm payload.
# Kernels are never embedded: callers pass kernel bytes at runtime and the engine
# writes them into the Emscripten FS.
#
# Usage: bash scripts/build-cspice.sh   (or: pnpm cspice:build)
# Requires: emscripten (emcc), csh, and git on PATH.
# Override the pin with CSPICE_COMMIT=<sha> when deliberately moving the toolkit.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$REPO_ROOT/vendor/cspice"
OUT="$REPO_ROOT/wasm"
CSPICE_REMOTE="https://github.com/arturania/cspice.git"
# arturania/cspice master as of 2026-11-20. Bump deliberately, rebuild, and rerun the tests.
CSPICE_COMMIT="${CSPICE_COMMIT:-53bce326267dd2d6d567de92b15869c9ed7d0629}"

mkdir -p "$OUT"

if [ ! -d "$VENDOR/src" ]; then
  echo "Fetching CSPICE source at $CSPICE_COMMIT into vendor/cspice ..."
  mkdir -p "$VENDOR"
  git -C "$VENDOR" init -q
  git -C "$VENDOR" fetch -q --depth 1 "$CSPICE_REMOTE" "$CSPICE_COMMIT"
  git -C "$VENDOR" checkout -q FETCH_HEAD
fi

if [ ! -f "$VENDOR/lib/libcspice_wasm.a" ]; then
  echo "Building libcspice_wasm.a (this compiles ~2000 C files) ..."
  ( cd "$VENDOR/src" && csh ./mk_wasm.csh )
fi

# The exported SPICE surface lives in scripts/cspice-exports.txt, shared with the WASI
# target (scripts/build-cspice-wasi.sh) so the two builds stay in step. Extend it
# deliberately; every symbol there is reachable from the typed API in src/bindings.ts.
EXPORTS_FILE="$REPO_ROOT/scripts/cspice-exports.txt"
EXPORTS="[$(grep -v '^#' "$EXPORTS_FILE" | tr -s ' \t\n' '\n' | sed '/^$/d; s/.*/"_&"/' | paste -sd, -)]"

RUNTIME_METHODS='["FS","ccall","cwrap","getValue","setValue","UTF8ToString","stringToUTF8","lengthBytesUTF8","writeArrayToMemory"]'

echo "Linking cspice.mjs + cspice.wasm ..."
emcc "$VENDOR/lib/libcspice_wasm.a" -o "$OUT/cspice.mjs" \
  -O2 \
  -s MODULARIZE=1 \
  -s EXPORT_ES6=1 \
  -s EXPORT_NAME=CSpice \
  -s WASM=1 \
  -s ALLOW_MEMORY_GROWTH=1 \
  -s INITIAL_MEMORY=167772160 \
  -s STACK_SIZE=5242880 \
  -s FORCE_FILESYSTEM=1 \
  -s EXPORTED_RUNTIME_METHODS="$RUNTIME_METHODS" \
  -s EXPORTED_FUNCTIONS="$EXPORTS"

# Mark the generated glue as not lintable; it is a build artifact.
sed -i.bak '1s;^;/* eslint-disable */\n// @ts-nocheck\n;' "$OUT/cspice.mjs"
rm -f "$OUT/cspice.mjs.bak"

echo "Done. Artifacts in wasm/:"
ls -la "$OUT"
