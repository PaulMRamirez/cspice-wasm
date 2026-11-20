# Full CSPICE as a WASI, Wasm MVP guest: measurements

This note answers one question with numbers: how heavy is the full CSPICE toolkit as a
guest in a flight-style WebAssembly interpreter? The subject is the second build target
of this repository, `wasm/cspice.wasi.wasm`, built by `scripts/build-cspice-wasi.sh`:
the same pinned CSPICE source and the same export list as the Emscripten artifact,
compiled for `wasm32-wasi` at `-mcpu=mvp` with no JS glue. The interpreter is
[SpaceWasm](https://github.com/nasa/SpaceWasm), the `no_std` Wasm 1.0 interpreter
behind F Prime's `Svc::WasmSequencer`, run here on a ground machine.

This is an independent, personal project. Nothing here is a NASA, JPL, NAIF, or F Prime
result or position, and nothing was run on flight hardware. Sections 1 to 4 are
measured facts; section 5 is interpretation and is labeled as such.

## 1. What was measured, and how

Host: Linux 6.18 x86_64, Intel Xeon at 2.10 GHz, 2 virtual CPUs, in a cloud sandbox;
wall times are from that one shared machine and moved by 10 to 30 percent between runs,
so treat them as indicative. Counts (bytes, functions, instructions, slices, system
calls) are deterministic and were identical on every run.

Toolchain: Ubuntu clang 18.1.3 and wasm-ld 18, binaryen 108 (`apt` packages `clang`,
`lld`, `llvm`, `binaryen`, `libclang-rt-18-dev-wasm32`); wasi-libc at commit `574b88da` (the `wasi-sdk-25` tag),
built by the script at `-mcpu=mvp` with `BULK_MEMORY_SOURCES=` emptied; CSPICE from
arturania/cspice at `53bce326`, compiled at `-O2`, then the linked module optimized
with `wasm-opt -O2 --mvp-features --enable-mutable-globals`; Node 22.23.5; Rust 1.97.0; `spacewasm`,
`spacewasm_tools` 0.8.0-rc.3 from crates.io with `wasi-common` and `wiggle` 28.0.1.

Commands, all from the repository root:

```sh
bash scripts/build-cspice-wasi.sh        # builds both modules, then the opcode scan
node scripts/check-wasm-mvp.mjs --json wasm/cspice.wasi.wasm   # static facts
node --no-warnings scripts/wasi-smoke.mjs                      # values against the oracle
cargo install spacewasm_tools --version =0.8.0-rc.3 --locked   # the `spacewasm` tool
bash scripts/wasi-measure.sh             # everything in sections 2 to 4, in one run
```

`scripts/wasi-measure.sh` runs the static scan, `spacewasm validate` (once with the
flight defaults, once with limits raised), `spacewasm run` on the command driver, the
host harness in `bench/spacewasm-host`, the operand stack probe, and the value check.
The CI job `wasi` in `.github/workflows/ci.yml` runs the same steps on a GitHub runner
and uploads the modules with `wasi-measurements.txt` and `wasi-measurements.json`.

The harness (`bench/spacewasm-host`, Rust) loads the reactor with the `spacewasm` crate
directly, binds WASI 0.1 through `wasi-common` (the same crates `spacewasm run` uses),
preopens `test/fixtures` as `/kernels`, and runs one scenario three times on fresh
engines: once with unlimited fuel (wall time), once in slices of 1000 instructions (the
`DEFAULT_INSTRUCTION_FUEL` of `Svc::WasmSequencer` in F Prime v4.4.0), and once with one
instruction of fuel per slice, which makes the slice count the exact number of
instructions executed. SpaceWasm's fuel unit is one instruction of its compiled IR, so
"instructions" below means SpaceWasm IR instructions, the same unit the sequencer
budgets; it is not a count of Wasm bytecode instructions.

The scenario: `_initialize`; `erract_c` and `errprt_c`; `furnsh_c` of `naif0012.tls`
and of `de440s-inner-cassini.bsp` by path; `str2et_c` twice; `spkezr_c` of EARTH
relative to SUN in J2000 with LT+S once cold, then 20 times at new epochs 600 s apart,
then 20 times with NONE, then once at the reference epoch 2004-07-01T00:00:00 UTC, and
last a second `furnsh_c` of the leapseconds kernel.

## 2. The module (static facts)

| Quantity | WASI, MVP reactor (`cspice.wasi.wasm`) | Emscripten artifact (`cspice.wasm`) |
| --- | --- | --- |
| Module size | 1,466,028 bytes | 1,410,935 bytes plus 75,228 bytes of JS glue |
| Defined functions | 1,070 | 1,092 |
| Exports | 124 (122 from `scripts/cspice-exports.txt`, `memory`, `_initialize`) | the same SPICE list, plus Emscripten runtime entries |
| Imports | 15, all `wasi_snapshot_preview1` | 22 (Emscripten `env` and WASI) |
| Declared initial memory | 1,544 pages = 101,187,584 bytes (96.5 MiB), no maximum | 2,560 pages = 160 MiB, maximum 2 GiB |
| Data segments | 431 segments, 269,805 bytes, ending at address 1,324,504 | 434 segments |
| Code section | 1,188,487 bytes, 491,301 Wasm instructions | 1,133,360 bytes |
| Guest stack | 1 MiB at the bottom of memory (`--stack-first`) | 5 MiB |
| Feature set | Wasm 1.0 plus mutable globals, every opcode scanned | includes the DataCount section (bulk memory) |

The 15 imports are `clock_time_get`, `fd_close`, `fd_fdstat_get`,
`fd_fdstat_set_flags`, `fd_filestat_get`, `fd_prestat_get`, `fd_prestat_dir_name`,
`fd_read`, `fd_renumber`, `fd_seek`, `fd_write`, `path_filestat_get`, `path_open`,
`path_unlink_file`, and `proc_exit`. The command driver (`cspice-driver.wasi.wasm`,
1,005,469 bytes, 682 functions, 1,405 initial pages) adds `args_get` and
`args_sizes_get`.

Of the 96.5 MiB of initial memory, 1 MiB is the guest stack and about 0.3 MiB is
initialized data; the remaining 95 MiB is zero-initialized static storage that CSPICE
declares for its own tables. Summing static symbols per object file in the library
shows where it goes: the kernel pool (`pool.o`) 27.6 MB; the EK query and scan buffers
(`zzekjsrt.o`, `zzeksca.o`) 22.3 MB; the DAF buffers (`daffa.o`) 10.3 MB; the SPK and
CK segment buffers (`spkbsr.o`, `ckbsr.o`) 9.3 MB each; the DSK buffers (`dskx02.o`,
`zzdsksbf.o`, `zzdskbsr.o`) 10.3 MB together; body name tables, the file handle
manager, the kernel keeper, and SCLK 6.4 MB together. These were not shrunk in this
session.

Strict MVP validation: `scripts/check-wasm-mvp.mjs` decodes every section and every
instruction and fails on anything outside Wasm 1.0 plus mutable globals; both modules
pass. Two independent checks agree: `wasm-opt --mvp-features --enable-mutable-globals`
(binaryen 108) accepts the module, and SpaceWasm's own decoder loads it. No lowering
pass (`spacewasm lower`, formerly `wasm2mvp.sh`) was needed or used, because the
post-MVP opcodes were kept out at the source instead (section 4).

Results match the Emscripten build bit for bit: `str2et_c` and all six state components
and the light time from `spkezr_c` have a relative difference of exactly 0 in Node's
WASI host (reactor and command driver) and in SpaceWasm.

## 3. Under SpaceWasm 0.8.0-rc.3, on the ground

It loads and it runs, with correct results, but not with flight-default limits and not
through the stock `spacewasm run` command.

| Quantity | Value | Source |
| --- | --- | --- |
| `spacewasm validate`, defaults (256 code pages, 16 heap pages of 8192 bytes) | fails: `AllocError(OutOfMemory)` at byte 0x13bd4 | `spacewasm validate` |
| Compiled IR | 940,021 16-bit words = 1,880,042 bytes, in 3,672 code pages of 512 bytes | `spacewasm validate`, harness agrees |
| IR bytes per code-section byte | 1.58 (1.28 per module byte) | derived |
| Interpreter heap the load takes (flight page layout) | 1,927,846 bytes in 4,715 allocations: 30 pages of 65,536 bytes | `spacewasm validate --heap-page-size 65536` |
| Largest single heap allocation | 34,240 bytes (function table), more than a default 8,192 byte heap page holds | `spacewasm validate` |
| Decoder stack needed | the default 64 frames, 256 operands suffices | `spacewasm validate`, harness |
| Interpreter operand stack needed | between 129 and 256 32-bit words for the scenario (128 traps with StackOverflow) | harness `--stack-words` |
| Interpreter heap, live after load, excluding the operand stack | 2,452,472 bytes | harness counting allocator |
| Guest linear memory | 101,187,584 bytes at load, unchanged after the calls (no `memory.grow`) | harness |
| Host process resident set | about 3,900 KiB before load, about 105,000 KiB right after load, no higher after the scenario | `/proc/self/status` VmRSS, VmHWM |
| Load time (decode, validate, compile, instantiate) | 68 to 77 ms | harness |
| `spacewasm run` on the command driver | fails: `FunctionImportNotFound` (its WASI host does not bind `fd_renumber`) | `spacewasm run` |

Per-call cost, from the harness (instructions are exact; slices are at 1000
instructions each; wall time is free-running on the host above):

| Call | Instructions | Slices of 1000 | Wall time | WASI calls |
| --- | --- | --- | --- | --- |
| `_initialize` | 7 | 1 | under 0.01 ms | 0 |
| `furnsh_c` of `naif0012.tls` (first SPICE file access) | 50,849,733 | 50,850 | 170 to 260 ms | 53 |
| `furnsh_c` of the de440s subset SPK | 1,675,399 | 1,676 | 5.7 to 8.9 ms | 80 |
| `str2et_c`, first call | 477,332 | 478 | 1.9 to 2.6 ms | 0 |
| `str2et_c`, second call | 217,168 | 218 | 0.9 to 1.2 ms | 0 |
| `spkezr_c` EARTH wrt SUN, J2000, LT+S, first call | 1,230,068 | 1,231 | 5.5 to 7.7 ms | 51 |
| `spkezr_c` LT+S, steady state (mean of 20 new epochs) | 81,606 | 82 | 0.47 to 0.57 ms | 0 |
| `spkezr_c` LT+S, reference epoch | 83,355 | 84 | 0.52 to 0.56 ms | 0 |
| `spkezr_c` NONE, steady state (mean of 20) | 31,660 | 32 | 0.18 to 0.21 ms | 0 |
| `furnsh_c` of `naif0012.tls` again, at the end | 8,547,708 | 8,548 | 33 to 36 ms | 48 |

So one steady-state `spkezr_c` call with LT+S takes 82 to 84 sequencer slices at the
default fuel of 1000, and the first such call after furnishing takes 1,231. Running in
slices of 1000 instead of free-running changed wall time by less than the run to run
noise. The whole declared linear memory is resident as soon as the module is
instantiated, before any guest code runs, so the 96.5 MiB is a real allocation in this
embedding and not only an address-space reservation. Loading the same leapseconds
kernel a second time costs 8.5 million instructions, which puts the one-time
initialization share of the first `furnsh_c` at about 42 million.

Every number in this section comes from SpaceWasm itself (the `spacewasm` crate inside
the harness, or the `spacewasm` ground tool). No stand-in interpreter was needed for
instruction counts. wasmtime was not available in the build sandbox, so there is no
wasmtime fuel figure to compare against; Node's WASI host was the second runtime, and
it reports no instruction counts.

## 4. What failed, what was worked around, what was not verified

Stock sysroots are not MVP. Ubuntu's `wasi-libc` package (0.0~git20230113) is built
with bulk memory and sign extension: a module linked against it at `-mcpu=mvp` carries
a DataCount section and `memory.copy`/`memory.fill` in `memcpy`, `memmove`, and
`memset`, and the checker rejects it. The build therefore compiles its own wasi-libc at
`-mcpu=mvp` (about 15 s). The `apt` compiler-rt builtins archive is tagged with sign
extension too; none of the builtins this link pulls in uses a sign-extension opcode
(the scan proves it), but the linker copies the tag into the `target_features` custom
section, so the script removes that section before optimizing and gives `wasm-opt` the
MVP feature set explicitly. A different export set could pull in a
builtin that does use one; the scan in the build script and in CI would then fail
loudly, and the fix would be to build the builtins from source at `-mcpu=mvp`. wasi-sdk
release sysroots were not downloaded or tested here.

An implicit optimizer made the first CI run differ from the local build: when a link
line carries `-O2`, clang 18 silently runs whatever `wasm-opt` is on `PATH`. The local
machine had binaryen and the CI runner did not, so CI produced the unoptimized link
(1,723,235 bytes, 1,518 functions) and the reproducibility check failed. The script now
links without `-O`, requires `wasm-opt`, and invokes it explicitly; the output is
byte-identical to the module measured here. For scale, the unoptimized link compiles to
974,209 IR words and costs 86,547 instructions per steady-state LT+S `spkezr_c`, and
`wasm-opt -Os` gives 1,451,836 bytes, 928,564 IR words, and 81,412 instructions, so the
optimizer level moves these numbers by a few percent at most.

`setjmp`, `longjmp`: not used by the CSPICE sources at this commit (searched), so
nothing needed stubbing. `signal` (libf2c's `sig_die.c`, `signal_.c`) and `clock`
compile against wasi-libc's emulation shims (`-D_WASI_EMULATED_SIGNAL`,
`-D_WASI_EMULATED_PROCESS_CLOCKS`). `tmpfile` does not exist in wasi-libc; it is
referenced from the libf2c scratch-unit paths, and `scripts/wasi/cspice_wasi_shim.c`
stubs it to fail with ENOSYS. The 15 libf2c files that rely on implicit `int` need
`-Wno-implicit-int`, as in the fork's own Emscripten script.

`spacewasm run` cannot load either module, because its WASI host module omits
`fd_renumber`, which wasi-libc's `freopen` imports and libf2c's `err.c` and `endfile.c`
call. The harness adds that one binding (`bench/spacewasm-host/src/wasi.rs`, adapted
from the upstream file); with it the interpreter runs the module unmodified.

Not verified: only the calls in the scenario were exercised in the WASI module. The
other exported routines (frames, CK, SCLK, DSK, geometry finders, the SPK, CK, and DSK
writers) link and validate but were not run under WASI, and the TypeScript test suite
does not drive this target. Writing kernels through WASI, and the 1 MiB guest stack
under the deeper call trees of the geometry finders and DSK routines, are untested;
the measured guest stack high-water mark for this scenario plus a PCK load was 15,440
bytes. Wall times are from one noisy virtual machine. No flight processor, no RTOS, and
no F Prime deployment was involved: the slice counts are what `Svc::WasmSequencer`
would need at its default fuel, computed with the same interpreter, not observed in a
sequencer.

## 5. Reading (interpretation, not measurement)

As a ground oracle, the WASI target is ready to use: one 1.47 MB file with no glue runs
in any WASI 0.1 host tried, reads kernels from a preopened directory, and reproduces
the Emscripten build to the last bit on the calls checked. It is a reasonable way to
give a Rust, Python, or C host, or a SpaceWasm-based ground test rig, the reference
SPICE answer without a native toolkit build.

As a flight guest, full CSPICE is the wrong shape, and memory is the reason rather than
compute. For scale, the defaults F Prime v4.4.0 ships for `Svc::WasmSequencer` are 8
heap pages of 8,192 bytes, 8,192 bytes of guest linear memory, a 1,024 word operand
stack, and 1,000 instructions per slice. Against those, this guest needs about 12,000
times the linear memory (96.5 MiB, nearly all of it static tables sized for ground
workstations, and all of it resident from instantiation), and about 30 times the interpreter
heap (1.9 MB, of which 1.88 MB is IR for 1,070 functions when a single ephemeris lookup
needs a small fraction of them). The operand stack and decoder stack defaults are
already enough. Those limits are configuration, not architecture, so a deployment could
raise them, but 100 MB of RAM for one guest is a large fraction of what flight
computers carry.

Compute is tolerable by comparison. A warm LT+S state lookup is about 82,000
interpreter instructions, 82 slices; a geometric one is about 32,000. How long that is
on a flight processor was not measured; an interpreter on a radiation-hardened CPU is
commonly orders of magnitude slower than this host, so 0.5 ms here plausibly becomes
tens of milliseconds there, which suits planning-rate use and not a control loop. The
one-time costs are the ones to schedule around: the first `furnsh_c` executes 51
million instructions (50,850 slices), about 42 million of them one-time
initialization, each later text kernel of that size about 8.5 million, and the first
`spkezr_c` 1.2 million.

The static tables are where a flight variant would start. The obvious knobs, none
touched here: the kernel pool dimensions in `pool.c` (MAXVAR, MAXVAL, MAXLIN; 27.6 MB),
dropping the EK subsystem that `furnsh_c` drags in through the kernel keeper (22 MB),
the DAF, SPK, CK, and DSK buffer sizes (about 40 MB together), and the export list
itself, since a guest that only needs `spkezr_c` and time conversion does not need DSK,
CK writers, or the geometry finders. A purpose-built SPK reader would be smaller still,
with full CSPICE kept on the ground as its oracle; this module is that oracle.
