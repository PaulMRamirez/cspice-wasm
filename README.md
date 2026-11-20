# cspice-wasm

NAIF CSPICE compiled to WebAssembly, behind a typed, promise-based API. It runs the
SPICE toolkit either in-process or in a Web Worker, so callers can query ephemerides,
frames, and geometry events off the main thread, in browsers and in Node.

This is an independent, personal open-source project. It is not a product of, and is
not endorsed by, NAIF, NASA, JPL, or Caltech; see NOTICE for the provenance of the
CSPICE toolkit it compiles. The package began as the spice layer of the Bessel and
Cosmolabe projects and was extracted here with its history intact.

## Install and develop

The package is not yet published to npm (the name is reserved). Until the first
release, consume it as a git dependency pinned to a commit, for example
`"cspice-wasm": "github:PaulMRamirez/cspice-wasm#<sha>"`; the `prepare` script
compiles `dist/` on install, and the linked `wasm/` artifacts are committed.

For local work: `pnpm install`, then `pnpm verify` (typecheck, build, tests). Node 22
or newer.

## Public API

Engine factories:

- `createSpiceEngine(options?)`: in-process engine (loads cspice.wasm directly); used by unit tests and as the worker entry point.
- `createSpiceWorkerClient(...)`: main-thread proxy that drives the engine in a Web Worker.
- `createSpiceWorkerPool(...)`: pool of worker clients for parallel sweeps.
- `installSpiceWorker`, `dispatchSpice`, `JobCancelledError`: the worker-side runtime.

Engine surface (`SpiceEngine` / `SpiceComputeEngine`):

- Kernels: `furnsh`, `unload`, `kclear`, `ktotal`.
- Time: `str2et`, `et2utc`, `utc2et`.
- Ephemeris: `spkpos`, `spkezr`, `spkposBatch` (zero-copy n*3 positions), `spkezrBatch` (zero-copy n*6 states plus light times, the batched-states primitive that Cosmolabe's frames tier builds on).
- Two-body math: `oscelt`, `conics`, `prop2b`; SPK Type 13 writeback via `writeSpkType13`.
- Geometry events: `gfoclt`, `gfdist`, `occult`.
- Frames and bodies: `pxform`, `sxform`, `bodvrd`, `bodvcd`, `getfov`.
- Surface and illumination: `sincpt`, `subpnt`, `ilumin`, `recgeo`, `et2lst`, `readDsk`.
- Attitude: `twovec`, `m2q`, `q2m`, `raxisa`.
- Spacecraft clock and CK attitude: `sce2c`/`sct2e` (ET <-> encoded SCLK ticks), `ckgp` (CK pointing query), and `writeCk03`, which writes a CK Type 3 segment (discrete quaternions plus optional angular rate, linear interpolation) and furnshes it so the attitude history is queryable through the same `pxform`/`ckgp` path. A class-3 frame (defined in an FK with `FRAME_<id>_CLASS=3`, `CK_<id>_SCLK`, `CK_<id>_SPK`) drives the scene orientation from a furnished C-kernel. See `ck.test.ts` for the write-then-read round trip and `scripts/make-fixture-ck.mjs` for the demo CK generator.
- Time series: `evalSeries` plus `runEvalSpec`, `gridEpochs`, `PROVIDER_CATALOG` for one-round-trip, cancellable sweeps over a grid.

Types include `Vec3`, `StateVector`, `PositionResult`, `CartesianState`, `OsculatingElements`, `AberrationCorrection`, `Mat3`, `StateBatchArrays`, and the located `SpiceError`. For in-process synchronous consumers (the frames tier, conformance rigs) the package also exports `SpiceBindings` and `createSpiceBindings`, the typed marshaling layer beneath the promise surface, including `namfrm` and `frmnam` for frame id resolution.

```ts
const engine = await createSpiceEngine();
await engine.furnsh('naif0012.tls', lskBytes);
await engine.furnsh('de440s.bsp', spkBytes);
const et = await engine.str2et('2004-07-01T00:00:00');
const { position, lightTime } = await engine.spkpos('6', et, 'J2000', 'NONE', '10');
```

## Design rules

The package has no runtime dependencies and never imports a UI, a platform layer, or
a host application. Kernel bytes are passed in by the caller (`furnsh(name, bytes)`),
so the engine never reads kernel files directly and works the same wherever the bytes
come from. Kernels are never embedded in the build.

## Tests

Tests live in `src/*.test.ts` (spice, batch, dsk, geometry, geodetic,
occultation, propagation, eval-series, ck). The `ck.test.ts` round trip writes a known
attitude profile to a CK Type 3 segment, furnshes it, and asserts both `ckgp` and
`pxform(frame, J2000)` against the validated `q2m` quaternion convention. The acceptance
test (`spice.test.ts`) loads
`naif0012.tls` plus a de440s SPK and asserts `spkpos` of Saturn barycenter (6) relative
to the Sun (10) in J2000 at 2004-07-01 against a NAIF reference pinned from de440s,
agreeing well within 1 metre, and checks that unresolved bodies raise a typed `SpiceError`.

## Rebuilding the WASM

`pnpm cspice:build` (which runs `scripts/build-cspice.sh`) needs Emscripten (`emcc`),
`csh`, and `git`. It fetches the arturania/cspice fork at the commit pinned in the
script, compiles about 2000 C files into a static library under `vendor/` (ignored),
and links `wasm/cspice.mjs` plus `wasm/cspice.wasm`. To expose another SPICE routine,
add its symbol to `scripts/cspice-exports.txt` (shared by both build targets), rebuild,
and add the typed binding in `src/bindings.ts`. The bounded kernel fixtures under `test/fixtures` are committed,
so the tests run without any download; the `scripts/make-fixture-*.mjs` generators
that derive them need the full source kernels and are only for regenerating fixtures.

## Second target: WASI, Wasm MVP (no glue)

`wasm/cspice.wasi.wasm` is the same CSPICE source and the same export list
(`scripts/cspice-exports.txt`) built as a WASI 0.1 reactor restricted to the WebAssembly
MVP feature set, for WASI 0.1 runtimes that cannot load the Emscripten artifact. It
has been run in Node's `node:wasi` and in NASA's
[SpaceWasm](https://github.com/nasa/SpaceWasm) interpreter; wasmtime has not been
tried yet. It needs no JS: a host supplies the 15 `wasi_snapshot_preview1` imports,
calls `_initialize` once, then calls `malloc`, `free`, and the SPICE functions
directly, and kernels are read by path from a directory the host preopens. The typed
TypeScript API does not use this module; it is a separate artifact for other hosts.

`pnpm cspice:build:wasi` (`scripts/build-cspice-wasi.sh`) needs clang and wasm-ld with
the WebAssembly backend, the LLVM binutils, `make`, `git`, and the compiler-rt builtins
for wasm32 (on Ubuntu 24.04: `clang lld llvm libclang-rt-18-dev-wasm32`). It builds its
own wasi-libc at `-mcpu=mvp`, because stock sysroots use bulk memory, and it also links
`wasm/cspice-driver.wasi.wasm`, a command-style driver for hosts that only run
`_start`. `pnpm check:wasi` scans every opcode and fails on anything outside Wasm 1.0
plus mutable globals; `pnpm test:wasi` loads the module in Node's WASI host and checks
`str2et_c` and `spkezr_c` against the Emscripten build; `pnpm bench:spacewasm` runs the
Rust harness in `bench/spacewasm-host` and prints the measurements. The CI job `wasi`
runs all of it and confirms the committed module is byte-identical to a fresh build.

The module declares 96.5 MiB of initial memory, almost all of it CSPICE's static
tables, so it is a ground oracle and a measurement subject, not a flight-ready guest.
The numbers and what they imply are in
[docs/wasi-mvp-measurements.md](docs/wasi-mvp-measurements.md).

## Status / limitations

All calls are async even in-process. Geometry-event finders (`gfoclt`, `gfdist`)
require a search `step` shorter than the briefest event or they can miss intervals;
correctness follows CSPICE semantics.
