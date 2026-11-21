# CSPICE source provenance

The WebAssembly artifacts in `wasm/` are compiled from the CSPICE Toolkit, version N0066, produced by NAIF. This note records exactly which source is compiled and how it differs from what NAIF distributes. The `naif-source-check` workflow enforces it: the job fails if the compiled source differs from NAIF's package in any file other than the labeled patches below.

## What is compiled

`scripts/build-cspice.sh` downloads NAIF's N0066 CSPICE package (Mac, Intel, 64 bit) from the NAIF server, checks it against a pinned SHA-256, applies the patch set in `patches/`, and compiles every C file under `src/cspice`. This project does not copy the Toolkit source, its documentation, or its utility programs into this repository; it ships only the compiled library, its own wrapper, and the four small patch files.

| Item | Value |
| --- | --- |
| Package | `https://naif.jpl.nasa.gov/pub/naif/misc/toolkit_N0066/C/MacIntel_OSX_AppleC_64bit/packages/cspice.tar.Z` |
| SHA-256 | `f5d48c4b0d558c5d71e8bf6fcdf135b0943210c1ff91f8191dfc447419a6b12e` |
| Toolkit version | N0066 |
| Compiler | Emscripten 7.0.0 (`EMSDK_VERSION` in `scripts/build-cspice.sh`) |

The committed `wasm/cspice.wasm` and `wasm/cspice.mjs` are the output of that script with that compiler. The `rebuild-wasm` workflow repeats the build on a GitHub Actions runner and fails unless it reproduces the committed files byte for byte; the WASI module `wasm/cspice.wasi.wasm` is held to the same standard by the `wasi` job in `ci`. The artifacts committed before the compiler was pinned came from the fork described below and an Emscripten version that was not recorded, so they could not be reproduced; they were replaced by a build made in CI with the pinned version (the `commit-wasm-artifacts` workflow, which commits as `github-actions[bot]`).

NAIF's platform packages share their C source apart from a few platform identifiers (`SpiceZpl.h`, `zzplatfm.c`, and the logical unit count in `fndlun.c`). The Mac package is used because the artifacts were first built from it, so this build reports NAIF's Mac platform identifiers. Moving to another platform package is a deliberate change that needs a rebuild and the test suite.

## Reproduced outside CI

The byte for byte claim has also been checked away from GitHub's runners. On 21 November 2026, on an Ubuntu 24.04.5 x86-64 machine that was not a GitHub Actions runner, a fresh checkout of commit `3a9486a` was built with Emscripten 7.0.0 installed through emsdk (`emsdk install 7.0.0`, which reports release `777e6e5db7b4990bcaeb1b9998ed642cc84e6629`), from the NAIF package downloaded at that time and checked against the pinned SHA-256. `bash scripts/build-cspice.sh` produced the committed files exactly:

| File | Bytes | SHA-256 of the local build and of the committed file |
| --- | --- | --- |
| `wasm/cspice.wasm` | 1,411,926 | `c69dbe9cc73f4df982fc0e3eed247f918d45a13877cc0f1284730a5281cdb028` |
| `wasm/cspice.mjs` | 71,176 | `ce26f272e2964eb52ee97c5af10905c24c0ebcda5d237877b0178cfd4f507cef` |

On the same machine `bash scripts/build-cspice-wasi.sh`, with Ubuntu's clang 18.1.3 and binaryen 108 and a wasi-libc built by the script, reproduced `wasm/cspice.wasi.wasm` exactly as well (1,468,086 bytes, SHA-256 `11fc02ada9d5e264b41eda6f22f4a5ec713cb5d927b25e6c4cfd0edfe2bb801f`).

This is one machine on one day, of the same operating system family and architecture as the CI runner. It shows that the build does not depend on anything particular to the runner image; it says nothing about other hosts (macOS, or Linux on arm64), where emsdk installs different compiler binaries and the result has not been compared.

## Changes this project makes

Four files are patched, all for one reason: WebAssembly requires a function's definition to match the way its callers declare it, and requires the POSIX declarations the f2c runtime uses to be visible. Each patched file begins with a header comment stating that it is modified, what changed, that the change is not NAIF's, and that NAIF does not support modified code.

| File | Change |
| --- | --- |
| `rsfe.c` | `zzsetnnread_` returns `int` in place of `void` |
| `s_cat.c` | `s_cat` returns `int` in place of `void` |
| `s_copy.c` | `s_copy` returns `int` in place of `void` |
| `fio.h` | Includes `unistd.h` when compiling with Emscripten |

These changes originated in the arturania/cspice GitHub fork, which this project compiled before it moved to NAIF's package. The build also adds two lint directives to the top of the generated JavaScript glue and chooses which routines to export.

## History: the fork

Until it moved to NAIF's package the build fetched the arturania/cspice fork at commit `53bce326267dd2d6d567de92b15869c9ed7d0629`. A file by file comparison with NAIF's N0066 Mac package found 2,179 files identical, six changed, and one added: the four changes above, `extern` added to the prototypes in `SpiceZpr.h` (for a Windows DLL build), a comment block added to `writln.c`, and an added `erf.c`. None was labeled as modified. The three that WebAssembly does not need are no longer part of the build. The `rebuild-wasm` workflow still builds both ways with the same compiler and reports whether the results are byte-identical; with Emscripten 7.0.0 they are.

## Getting the Toolkit

To obtain the SPICE Toolkit, its documentation, and SPICE kernels, go to NAIF: https://naif.jpl.nasa.gov/naif/toolkit.html. Use of SPICE is governed by NAIF's rules: https://naif.jpl.nasa.gov/naif/rules.html. Modified code is not supported by NAIF; anyone who needs NAIF support should use the Toolkit from NAIF.
