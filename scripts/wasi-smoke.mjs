#!/usr/bin/env node
// Smoke test for the WASI (Wasm MVP) CSPICE build.
//
// Loads wasm/cspice.wasi.wasm in Node's built-in WASI host with no JS glue, preopens
// test/fixtures, furnishes the leapseconds kernel and the de440s subset by path, and
// checks str2et_c and spkezr_c (Earth relative to the Sun, J2000, LT+S) against the
// committed Emscripten build (wasm/cspice.mjs), which is the oracle. Agreement must be
// within 1e-12 relative. The command-style driver module is run too, and its printed
// numbers are held to the same tolerance.
//
// Usage: node scripts/wasi-smoke.mjs [--host-output <file>]
// With --host-output it also checks a file of `et ...`, `state ...`, `lt ...` lines
// printed by another host (the SpaceWasm harness in bench/spacewasm-host) the same way.

import { readFileSync, existsSync, openSync, mkdtempSync, rmSync } from 'node:fs';
import { devNull, tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { setFlagsFromString } from 'node:v8';
import { WASI } from 'node:wasi';

// Node 22 (V8 12.4) can segfault while a guest runs in node:wasi. Wasm code reaches
// the WASI imports through V8's fast API call path, which enters Node's C++ directly
// with no JavaScript frame in between. Every uvwasi allocation made inside such a call
// (path resolution, fd_read, fd_write) reports its size to V8 as external memory, and
// with this module's 96.5 MiB linear memory already counted, that report can pass V8's
// external memory limit and start a full garbage collection on the spot. The collector
// then crashes walking the frame that made the fast call (gdb: WasiFunction
// FastCallback, NgLibMemoryManager::ReallocImpl, Heap::ReportExternalMemoryPressure,
// MarkCompactCollector::MarkRoots, CommonFrame::IterateTurbofanOptimizedFrame). It is
// a host defect, independent of this module's contents, and intermittent because the
// collection only sometimes lands inside a WASI call. Turning fast API calls off sends
// every WASI import through the ordinary callback path, where a collection is safe.
// The flag must be set before any wasm module is compiled, so it is set here. An
// earlier version of this script set --liftoff-only instead, which lowered the crash
// rate and did not remove it. Node 24 (V8 13.6) does not crash either way. See
// docs/wasi-mvp-measurements.md.
setFlagsFromString('--no-turbo-fast-api-calls');

const root = fileURLToPath(new URL('..', import.meta.url));
const fixtures = `${root}test/fixtures`;
const UTC = '2004-07-01T00:00:00';
const LSK = 'naif0012.tls';
const SPK = 'de440s-inner-cassini.bsp';
const TOLERANCE = 1e-12;

// Every WASI object gets descriptors of its own instead of the process's stdio, so
// guest output cannot interleave with this script's report, and each object stays
// referenced until the process exits.
const keepAlive = [];
function makeWasi(options, stdout = openSync(devNull, 'w')) {
  const wasi = new WASI({
    version: 'preview1',
    stdin: openSync(devNull, 'r'),
    stdout,
    stderr: openSync(devNull, 'w'),
    ...options,
  });
  keepAlive.push(wasi);
  return wasi;
}

// The oracle: the Emscripten artifact, driven through its own glue.
async function oracle() {
  const { default: CSpice } = await import(`${root}wasm/cspice.mjs`);
  const m = await CSpice();
  for (const name of [LSK, SPK]) m.FS.writeFile(`/${name}`, readFileSync(`${fixtures}/${name}`));
  m.ccall('erract_c', null, ['string', 'number', 'string'], ['SET', 0, 'RETURN']);
  m.ccall('errprt_c', null, ['string', 'number', 'string'], ['SET', 0, 'NONE']);
  m.ccall('furnsh_c', null, ['string'], [`/${LSK}`]);
  m.ccall('furnsh_c', null, ['string'], [`/${SPK}`]);
  const out = m._malloc(64);
  m.ccall('str2et_c', null, ['string', 'number'], [UTC, out]);
  const et = m.getValue(out, 'double');
  m.ccall(
    'spkezr_c',
    null,
    ['string', 'number', 'string', 'string', 'string', 'number', 'number'],
    ['EARTH', et, 'J2000', 'LT+S', 'SUN', out, out + 48],
  );
  if (m.ccall('failed_c', 'number', [], [])) throw new Error('the Emscripten oracle reported a SPICE error');
  const state = Array.from({ length: 6 }, (_, i) => m.getValue(out + 8 * i, 'double'));
  const lt = m.getValue(out + 48, 'double');
  return { et, state, lt };
}

// The module under test: a WASI reactor, no glue beyond the standard WASI imports.
async function reactor() {
  const wasi = makeWasi({ preopens: { '/kernels': fixtures } });
  const bytes = readFileSync(`${root}wasm/cspice.wasi.wasm`);
  const { instance } = await WebAssembly.instantiate(bytes, wasi.getImportObject());
  wasi.initialize(instance);
  const x = instance.exports;
  const view = () => new DataView(x.memory.buffer);
  const cstr = (s) => {
    const utf8 = new TextEncoder().encode(`${s}\0`);
    const p = x.malloc(utf8.length);
    new Uint8Array(x.memory.buffer, p, utf8.length).set(utf8);
    return p;
  };
  x.erract_c(cstr('SET'), 0, cstr('RETURN'));
  x.errprt_c(cstr('SET'), 0, cstr('NONE'));
  const check = (what) => {
    if (x.failed_c()) {
      const buf = x.malloc(1841);
      x.getmsg_c(cstr('LONG'), 1841, buf);
      const raw = new Uint8Array(x.memory.buffer, buf, 1841);
      throw new Error(`${what}: ${new TextDecoder().decode(raw.subarray(0, raw.indexOf(0)))}`);
    }
  };
  x.furnsh_c(cstr(`/kernels/${LSK}`));
  check('furnsh_c(lsk)');
  x.furnsh_c(cstr(`/kernels/${SPK}`));
  check('furnsh_c(spk)');
  const out = x.malloc(64);
  x.str2et_c(cstr(UTC), out);
  check('str2et_c');
  const et = view().getFloat64(out, true);
  x.spkezr_c(cstr('EARTH'), et, cstr('J2000'), cstr('LT+S'), cstr('SUN'), out, out + 48);
  check('spkezr_c');
  const state = Array.from({ length: 6 }, (_, i) => view().getFloat64(out + 8 * i, true));
  const lt = view().getFloat64(out + 48, true);
  return { et, state, lt, pages: x.memory.buffer.byteLength / 65536 };
}

// The command module: run _start with stdout captured to a file, then parse it.
async function command() {
  const dir = mkdtempSync(`${tmpdir()}/cspice-wasi-`);
  const outPath = `${dir}/stdout.txt`;
  try {
    const wasi = makeWasi(
      {
        args: ['cspice-driver', `/kernels/${LSK}`, `/kernels/${SPK}`],
        preopens: { '/kernels': fixtures },
        returnOnExit: true,
      },
      openSync(outPath, 'w'),
    );
    const bytes = readFileSync(`${root}wasm/cspice-driver.wasi.wasm`);
    const { instance } = await WebAssembly.instantiate(bytes, wasi.getImportObject());
    const status = wasi.start(instance);
    const text = readFileSync(outPath, 'utf8');
    if (status !== 0) throw new Error(`the driver exited with status ${status}: ${text}`);
    return parseHostOutput(text);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function parseHostOutput(text) {
  const line = (key) => {
    const found = text.split('\n').find((l) => l.startsWith(`${key} `));
    if (!found) throw new Error(`no "${key}" line in host output:\n${text}`);
    return found.slice(key.length + 1).trim().split(/\s+/).map(Number);
  };
  return { et: line('et')[0], state: line('state'), lt: line('lt')[0] };
}

function compare(label, got, want) {
  const rel = (a, b) => Math.abs(a - b) / Math.max(Math.abs(b), Number.MIN_VALUE);
  const pairs = [['et', got.et, want.et], ['lt', got.lt, want.lt]];
  got.state.forEach((v, i) => pairs.push([`state[${i}]`, v, want.state[i]]));
  let worst = 0;
  for (const [name, a, b] of pairs) {
    const r = rel(a, b);
    worst = Math.max(worst, r);
    if (!(r <= TOLERANCE)) {
      throw new Error(`${label}: ${name} = ${a} differs from the oracle ${b} (relative ${r})`);
    }
  }
  console.log(`${label}: ok, worst relative difference from the Emscripten oracle ${worst.toExponential(2)}`);
}

const want = await oracle();
console.log(`oracle (Emscripten build): et ${want.et}, |r| ${Math.hypot(...want.state.slice(0, 3))} km, lt ${want.lt} s`);

const got = await reactor();
compare('reactor module in node:wasi', got, want);
console.log(`reactor linear memory after the calls: ${got.pages} pages (${(got.pages / 16).toFixed(1)} MiB)`);

if (existsSync(`${root}wasm/cspice-driver.wasi.wasm`)) {
  compare('command driver in node:wasi', await command(), want);
}

const hostFlag = process.argv.indexOf('--host-output');
if (hostFlag !== -1) {
  const file = process.argv[hostFlag + 1];
  compare(`host output ${file}`, parseHostOutput(readFileSync(file, 'utf8')), want);
}
