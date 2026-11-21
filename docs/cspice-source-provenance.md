# CSPICE source provenance

The WebAssembly artifacts in `wasm/` are compiled from the CSPICE Toolkit, version N0066, produced by NAIF. This note records exactly which source is compiled and how it differs from what NAIF distributes, so that the project can state it plainly and so that NAIF's rule about labeling modified code can be met. The facts below come from the `naif-source-check` workflow, which downloads the N0066 packages from the NAIF server and compares them file by file (line endings and trailing whitespace ignored) with the source this project builds.

## What is compiled

`scripts/build-cspice.sh` fetches the arturania/cspice GitHub fork at commit `53bce326267dd2d6d567de92b15869c9ed7d0629` and compiles every C file under its `src/` directory. That fork is NAIF's N0066 package for Mac, Intel, 64 bit, with a small number of changes by the fork's author. This project does not copy the Toolkit source, its documentation, or its utility programs into this repository; it ships only the compiled library and its own wrapper.

## Comparison with NAIF's N0066 packages

| Compared with | Identical files | Differing files | Files only in the fork |
| --- | --- | --- | --- |
| NAIF N0066, MacIntel_OSX_AppleC_64bit | 2,179 | 6 | 1 |
| NAIF N0066, PC_Linux_GCC_64bit | 2,176 | 9 | 1 |

The three extra differences against the Linux package (`SpiceZpl.h`, `zzplatfm.c`, `fndlun.c`) are NAIF's own platform variants, not edits: the Mac package identifies itself as a Mac build and allows 63 logical units where the Linux package allows 99. One consequence is that this WebAssembly build reports NAIF's Mac platform identifiers.

## Changes made by the fork (not by NAIF, and not by this project)

| File | Change | Why (from the fork's commit history) |
| --- | --- | --- |
| `include/SpiceZpr.h` | `extern` added to every prototype; a guarded include of `SpiceCK.h` | Building a Windows DLL |
| `include/fio.h` | Includes `unistd.h` when compiling with Emscripten | WebAssembly build |
| `src/rsfe.c` | `zzsetnnread_` returns `int` in place of `void` | WebAssembly build, which needs matching function signatures |
| `src/s_cat.c` | `s_cat` returns `int` in place of `void` | Same |
| `src/s_copy.c` | `s_copy` returns `int` in place of `void` | Same |
| `src/writln.c` | A comment block added; no code change | Not stated |
| `src/erf.c` | Added file: `erf` and `erfc` from Sun Microsystems' freely distributable math library, with its notice | Not stated |

None of these files carries a header note saying it was modified. NAIF's rules ask that modified Toolkit code say so in its header, and NAIF does not support modified code; anyone needing NAIF support should use the Toolkit from NAIF.

## Changes made by this project

None to the CSPICE source. The build adds two lint directives to the top of the generated JavaScript glue, and the choice of which routines to export.

## Getting the Toolkit

To obtain the SPICE Toolkit, its documentation, and SPICE kernels, go to NAIF: https://naif.jpl.nasa.gov/naif/toolkit.html. Use of SPICE is governed by NAIF's rules: https://naif.jpl.nasa.gov/naif/rules.html.

## A cleaner arrangement, not yet done

The build could fetch N0066 directly from the NAIF server and apply this project's own small patch set for the few files WebAssembly needs (the three return type changes and the `fio.h` include), with each patched file labeled as modified. That would remove the dependency on a third-party fork and make the labeling explicit. It changes the build inputs, so it needs a rebuild and the full test suite before it replaces the current arrangement.
