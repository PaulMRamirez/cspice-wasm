#!/usr/bin/env bash
#
# Builds the CSPICE WebAssembly artifacts from the SPICE Toolkit as NAIF
# distributes it. The script downloads the N0066 CSPICE package from the NAIF
# server, checks it against a pinned SHA-256, applies the small labeled patch set
# in patches/ (the changes WebAssembly needs, each marked as modified in its file
# header), compiles the library with Emscripten, and links the exported SPICE
# surface into an ES module plus a .wasm payload. Kernels are never embedded:
# callers pass kernel bytes at runtime and the engine writes them into the
# Emscripten FS.
#
# Usage: bash scripts/build-cspice.sh   (or: pnpm cspice:build)
# Requires: emscripten (emcc, emar), curl, gzip, tar, patch on PATH.
# CSPICE_FETCH_ONLY=1 stops after the source is fetched, verified, and patched.
#
# The package is NAIF's Mac, Intel, 64 bit build of N0066. Its C source is the
# same across NAIF's platform packages apart from a few platform identifiers; the
# Mac package is kept because the artifacts were first built from it, so the
# build reports the same platform strings as before. See
# docs/cspice-source-provenance.md.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$REPO_ROOT/vendor/naif-n0066"
OUT="$REPO_ROOT/wasm"
NAIF_URL="https://naif.jpl.nasa.gov/pub/naif/misc/toolkit_N0066/C/MacIntel_OSX_AppleC_64bit/packages/cspice.tar.Z"
NAIF_SHA256="f5d48c4b0d558c5d71e8bf6fcdf135b0943210c1ff91f8191dfc447419a6b12e"
PATCHED_FILES="fio.h rsfe.c s_cat.c s_copy.c"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

mkdir -p "$OUT"

if [ ! -f "$VENDOR/.patched" ]; then
  echo "Fetching the CSPICE N0066 package from NAIF ..."
  rm -rf "$VENDOR" && mkdir -p "$VENDOR"
  curl -fsSL -o "$VENDOR/cspice.tar.Z" "$NAIF_URL"
  got="$(sha256_of "$VENDOR/cspice.tar.Z")"
  if [ "$got" != "$NAIF_SHA256" ]; then
    echo "SHA-256 mismatch for the NAIF package: expected $NAIF_SHA256, got $got" >&2
    exit 1
  fi
  gzip -dc "$VENDOR/cspice.tar.Z" | tar xf - -C "$VENDOR"
  # Each patch names one file; NAIF ships some headers in both include/ and
  # src/cspice/, so apply it to every copy.
  for name in $PATCHED_FILES; do
    applied=0
    for target in "$VENDOR/cspice/src/cspice/$name" "$VENDOR/cspice/include/$name"; do
      if [ -f "$target" ]; then
        patch -s "$target" < "$REPO_ROOT/patches/$name.patch"
        applied=$((applied + 1))
      fi
    done
    if [ "$applied" -eq 0 ]; then echo "patch target $name not found in the NAIF package" >&2; exit 1; fi
    echo "patched $name ($applied copy or copies)"
  done
  touch "$VENDOR/.patched"
fi

if [ "${CSPICE_FETCH_ONLY:-0}" = "1" ]; then
  echo "Source fetched, verified, and patched in vendor/naif-n0066."
  exit 0
fi

LIB="$VENDOR/libcspice_wasm.a"
if [ ! -f "$LIB" ]; then
  echo "Building libcspice_wasm.a (this compiles about 2000 C files) ..."
  (
    cd "$VENDOR/cspice/src/cspice"
    emcc *.c -c -O2 -I../../include \
      -Wno-parentheses -Wno-shift-op-parentheses -Wno-logical-op-parentheses \
      -Wno-bitwise-op-parentheses -Wno-dangling-else -Wno-format -Wno-implicit-int
    emar rcs "$LIB" *.o
    rm -f *.o
  )
fi

# The exported SPICE surface lives in scripts/cspice-exports.txt, shared with the WASI
# target (scripts/build-cspice-wasi.sh) so the two builds stay in step. Extend it
# deliberately; every symbol there is reachable from the typed API in src/bindings.ts.
EXPORTS_FILE="$REPO_ROOT/scripts/cspice-exports.txt"
EXPORTS="[$(grep -v '^#' "$EXPORTS_FILE" | tr -s ' \t\n' '\n' | sed '/^$/d; s/.*/"_&"/' | paste -sd, -)]"

RUNTIME_METHODS='["FS","ccall","cwrap","getValue","setValue","UTF8ToString","stringToUTF8","lengthBytesUTF8","writeArrayToMemory"]'

echo "Linking cspice.mjs + cspice.wasm ..."
emcc "$LIB" -o "$OUT/cspice.mjs" \
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
