#!/usr/bin/env bash
#
# Second build target for cspice-wasm: CSPICE as a glue-free WASI 0.1 module restricted
# to the WebAssembly MVP feature set, for bare Wasm runtimes (wasmtime, Node's WASI,
# SpaceWasm) that cannot load the Emscripten artifact.
#
# It compiles the same pinned CSPICE source as scripts/build-cspice.sh with plain clang
# for wasm32-wasi at -mcpu=mvp (no bulk memory, sign extension, nontrapping float to
# int, multivalue, or SIMD), against a wasi-libc sysroot that this script also builds
# at -mcpu=mvp, because distro and wasi-sdk sysroots ship libc objects that use bulk
# memory. It links two modules from the shared export list in
# scripts/cspice-exports.txt:
#
#   wasm/cspice.wasi.wasm         WASI reactor: exports _initialize, malloc, free, and
#                                 the SPICE functions, so a host calls them directly.
#   wasm/cspice-driver.wasi.wasm  WASI command (scripts/wasi/driver.c) for hosts that
#                                 only run _start.
#
# Kernels are never embedded: the guest reads them through WASI from a directory the
# host preopens, so furnsh_c takes an ordinary path.
#
# Usage: bash scripts/build-cspice-wasi.sh   (or: pnpm cspice:build:wasi)
# Requires: clang and wasm-ld with the WebAssembly backend, llvm-ar, llvm-nm,
# llvm-objcopy, make, git, node, and the compiler-rt builtins for wasm32 (Ubuntu:
# apt-get install clang lld llvm libclang-rt-18-dev-wasm32). Tested with clang 18.1.3.
# Environment overrides: CSPICE_COMMIT, WASI_LIBC_COMMIT, CC, AR, NM, OBJCOPY,
# WASI_SYSROOT (use an existing MVP sysroot instead of building one), STACK_SIZE, JOBS.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$REPO_ROOT/vendor/cspice"
BUILD="$REPO_ROOT/vendor/wasi-build"
OUT="$REPO_ROOT/wasm"
CSPICE_REMOTE="https://github.com/arturania/cspice.git"
# Keep in step with scripts/build-cspice.sh: both targets compile the same toolkit.
CSPICE_COMMIT="${CSPICE_COMMIT:-53bce326267dd2d6d567de92b15869c9ed7d0629}"
WASI_LIBC_REMOTE="https://github.com/WebAssembly/wasi-libc.git"
# wasi-libc at the wasi-sdk-25 tag, the last line with the plain Makefile build.
WASI_LIBC_COMMIT="${WASI_LIBC_COMMIT:-574b88da481569b65a237cb80daf9a2d5aeaf82d}"

CC="${CC:-clang}"
AR="${AR:-llvm-ar}"
NM="${NM:-llvm-nm}"
OBJCOPY="${OBJCOPY:-llvm-objcopy}"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"
# Linear-memory stack for the guest, in bytes. CSPICE keeps nearly all of its state in
# static storage, so this is far below the 5 MB the Emscripten target reserves; see
# docs/wasi-mvp-measurements.md for the measured high-water mark.
STACK_SIZE="${STACK_SIZE:-1048576}"

TARGET_FLAGS=(--target=wasm32-wasi -mcpu=mvp)

mkdir -p "$OUT" "$BUILD"

fetch_pinned() { # dir remote commit
  if [ ! -d "$1/.git" ]; then
    mkdir -p "$1"
    git -C "$1" init -q
    git -C "$1" fetch -q --depth 1 "$2" "$3"
    git -C "$1" checkout -q FETCH_HEAD
  fi
}

if [ ! -d "$VENDOR/src" ]; then
  echo "Fetching CSPICE source at $CSPICE_COMMIT into vendor/cspice ..."
  fetch_pinned "$VENDOR" "$CSPICE_REMOTE" "$CSPICE_COMMIT"
fi

# 1. A wasi-libc sysroot compiled at -mcpu=mvp. BULK_MEMORY_SOURCES is emptied so that
#    memcpy, memmove, and memset build as ordinary loops instead of memory.copy and
#    memory.fill.
if [ -n "${WASI_SYSROOT:-}" ]; then
  SYSROOT="$WASI_SYSROOT"
else
  SYSROOT="$BUILD/sysroot"
  if [ ! -f "$SYSROOT/lib/wasm32-wasi/libc.a" ]; then
    echo "Building wasi-libc at $WASI_LIBC_COMMIT for -mcpu=mvp ..."
    fetch_pinned "$REPO_ROOT/vendor/wasi-libc" "$WASI_LIBC_REMOTE" "$WASI_LIBC_COMMIT"
    make -C "$REPO_ROOT/vendor/wasi-libc" -j"$JOBS" \
      CC="$CC" AR="$AR" NM="$NM" \
      EXTRA_CFLAGS="-O2 -DNDEBUG -mcpu=mvp" \
      BULK_MEMORY_SOURCES= \
      OBJDIR="$BUILD/libc-obj" SYSROOT="$SYSROOT" \
      libc "$BUILD/libc-obj/copy-startup-files.stamp" >"$BUILD/wasi-libc.log" 2>&1 ||
      { tail -n 40 "$BUILD/wasi-libc.log"; exit 1; }
  fi
fi

# 2. The CSPICE static library. Same warning set as the fork's mk_wasm.csh; the two
#    emulation defines map the libf2c signal and clock calls onto wasi-libc's shims.
CFLAGS=("${TARGET_FLAGS[@]}" --sysroot="$SYSROOT" -O2 -I"$VENDOR/include"
  -D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS
  -ffile-prefix-map="$REPO_ROOT"=. -w -Wno-implicit-int)
LIB="$BUILD/libcspice_wasi.a"
if [ ! -f "$LIB" ]; then
  echo "Building libcspice_wasi.a (this compiles about 2000 C files) ..."
  OBJ="$BUILD/cspice-obj"
  mkdir -p "$OBJ"
  export OBJ CC
  CFLAGS_STR="${CFLAGS[*]}"
  export CFLAGS_STR
  ( cd "$VENDOR/src" && ls *.c | xargs -P "$JOBS" -n 16 sh -c '
      for f in "$@"; do
        $CC $CFLAGS_STR -c "$f" -o "$OBJ/${f%.c}.o" || exit 255
      done' sh )
  rm -f "$LIB"
  ( cd "$OBJ" && ls *.o | LC_ALL=C sort | xargs "$AR" rcsD "$LIB" )
fi

# 3. Link. --stack-first puts the stack below the data so an overflow traps instead of
#    silently overwriting SPICE's static state. No -Wl,--import-memory: the module owns
#    its memory, and the declared minimum is whatever CSPICE's static arrays need.
EXPORT_FLAGS=()
for sym in $(grep -v '^#' "$REPO_ROOT/scripts/cspice-exports.txt"); do
  EXPORT_FLAGS+=("-Wl,--export=$sym")
done
LINK_COMMON=("${TARGET_FLAGS[@]}" --sysroot="$SYSROOT" -O2
  -Wl,--stack-first -Wl,-z,stack-size="$STACK_SIZE" -Wl,--strip-all
  -lwasi-emulated-signal -lwasi-emulated-process-clocks -lwasi-emulated-getpid)

echo "Linking wasm/cspice.wasi.wasm (reactor) ..."
"$CC" "${CFLAGS[@]}" -mexec-model=reactor "$REPO_ROOT/scripts/wasi/cspice_wasi_shim.c" "$LIB" \
  "${LINK_COMMON[@]}" "${EXPORT_FLAGS[@]}" -o "$OUT/cspice.wasi.wasm"

echo "Linking wasm/cspice-driver.wasi.wasm (command) ..."
"$CC" "${CFLAGS[@]}" "$REPO_ROOT/scripts/wasi/driver.c" \
  "$REPO_ROOT/scripts/wasi/cspice_wasi_shim.c" "$LIB" \
  "${LINK_COMMON[@]}" -o "$OUT/cspice-driver.wasi.wasm"

# The apt compiler-rt builtins archive is tagged as built for sign extension, and the
# linker copies that tag into the target_features custom section even when no such
# instruction is linked. Drop the section so the module does not advertise a feature it
# does not use; the opcode scan below is what actually proves the claim.
for m in cspice.wasi.wasm cspice-driver.wasi.wasm; do
  "$OBJCOPY" --remove-section=target_features "$OUT/$m"
done

# 4. Strict validation: every section and opcode must be Wasm 1.0 plus mutable globals.
node "$REPO_ROOT/scripts/check-wasm-mvp.mjs" "$OUT/cspice.wasi.wasm" "$OUT/cspice-driver.wasi.wasm"

echo "Done. Artifacts in wasm/:"
ls -la "$OUT"/*.wasi.wasm
