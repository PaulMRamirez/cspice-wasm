# cspice-wasm

NAIF CSPICE compiled to WebAssembly, behind a typed, promise-based API. It runs the
SPICE toolkit either in-process or in a Web Worker, so callers can query ephemerides,
frames, and geometry events off the main thread, in browsers and in Node.

This is an independent, personal open-source project. It is not a product of, and is
not endorsed by, NAIF, NASA, JPL, or Caltech; see NOTICE for the provenance of the
CSPICE toolkit it compiles. The package began as the spice layer of the Bessel and
Cosmolabe projects and was extracted here with its history intact.

To obtain the SPICE Toolkit itself, its documentation, and kernels, go to NAIF at
https://naif.jpl.nasa.gov/naif/toolkit.html. What this package compiles, and how that
source differs from NAIF's release, is recorded in docs/cspice-source-provenance.md.

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
`curl`, `gzip`, `tar`, and `patch`. It downloads NAIF's N0066 CSPICE package from the
NAIF server, verifies its SHA-256, applies the four labeled patches in `patches/`,
compiles about 2000 C files into a static library under `vendor/` (ignored), and links
`wasm/cspice.mjs` plus `wasm/cspice.wasm`. To expose another SPICE routine, add its
symbol to the `EXPORTS` list in the script, rebuild, and add the typed binding in
`src/bindings.ts`. The bounded kernel fixtures under `test/fixtures` are committed,
so the tests run without any download; the `scripts/make-fixture-*.mjs` generators
that derive them need the full source kernels and are only for regenerating fixtures.

## Status / limitations

All calls are async even in-process. Geometry-event finders (`gfoclt`, `gfdist`)
require a search `step` shorter than the briefest event or they can miss intervals;
correctness follows CSPICE semantics.
