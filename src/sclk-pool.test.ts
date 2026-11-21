// SCLK and kernel pool readers: scpart, dtpool, gdpool, gipool, gcpool, scencd, scdecd,
// scs2e, sce2s, sctiks, scfmt, and unitim.
//
// Oracle: the kernel text itself. The test parses the assignments in the committed
// text kernels (cassini-demo.tsc, naif0012.tls, cassini-demo.tf) with a small reader
// of its own and compares what CSPICE returns against those parsed values, so nothing
// here is a number copied from a previous run. The SCLK string expectations are
// derived from the same parsed fields: the moduli give the field widths and the ticks
// per count, the delimiter code gives the separator, and the coefficient triple gives
// the tick to TDB mapping.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, it, expect, beforeAll } from 'vitest';
import {
  createSpiceBindings,
  dispatchSpice,
  spiceEngineOver,
  SpiceError,
  type SpiceBindings,
  type SpiceEngine,
  type SpiceWorkerRequest,
} from './index.ts';

const fixtureBytes = (name: string): Uint8Array =>
  new Uint8Array(readFileSync(fileURLToPath(new URL(`../test/fixtures/${name}`, import.meta.url))));
const fixtureText = (name: string): string => new TextDecoder().decode(fixtureBytes(name));

const SC = -82;
const TSC = 'cassini-demo.tsc';
const LSK = 'naif0012.tls';
const FK = 'cassini-demo.tf';

type PoolValue = number | string;

const MONTHS = ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG', 'SEP', 'OCT', 'NOV', 'DEC'];

// A text kernel date (@1972-JAN-1, @2004-01-01T00:00:00) is stored in the pool as
// seconds past J2000 on a uniform calendar, with no leap seconds applied.
function dateSeconds(token: string): number {
  const m = /^@(\d{4})-([A-Za-z]{3}|\d{1,2})-(\d{1,2})(?:T(\d{2}):(\d{2}):(\d{2}))?$/.exec(token);
  if (!m) throw new Error(`unreadable kernel date ${token}`);
  const month = /^\d+$/.test(m[2]!) ? Number(m[2]) - 1 : MONTHS.indexOf(m[2]!.toUpperCase());
  const ms = Date.UTC(Number(m[1]), month, Number(m[3]), Number(m[4] ?? 0), Number(m[5] ?? 0), Number(m[6] ?? 0));
  return (ms - Date.UTC(2000, 0, 1, 12)) / 1000;
}

// Read the assignments in a text kernel's data sections: NAME = value or
// NAME = ( value, value ... ), where a value is a number (Fortran D exponents
// allowed), an @date, or a quoted string.
function parseTextKernel(text: string): Map<string, PoolValue[]> {
  const data: string[] = [];
  let inData = false;
  for (const line of text.split(/\r?\n/)) {
    const marker = line.trim();
    if (marker === '\\begindata') inData = true;
    else if (marker === '\\begintext') inData = false;
    else if (inData) data.push(line);
  }
  const out = new Map<string, PoolValue[]>();
  const assignment = /([^\s=]+)\s*=\s*(\(([^)]*)\)|'[^']*'|[^\s]+)/g;
  for (const m of data.join('\n').matchAll(assignment)) {
    const body = m[3] ?? m[2]!;
    const values: PoolValue[] = [];
    for (const token of body.match(/'[^']*'|[^\s,]+/g) ?? []) {
      if (token.startsWith("'")) values.push(token.slice(1, -1));
      else if (token.startsWith('@')) values.push(dateSeconds(token));
      else values.push(Number(token.replace(/[dD]/, 'e')));
    }
    out.set(m[1]!, values);
  }
  return out;
}

const numbers = (kernel: Map<string, PoolValue[]>, name: string): number[] => {
  const values = kernel.get(name);
  if (!values || values.some((v) => typeof v !== 'number')) throw new Error(`${name} is not numeric in the kernel text`);
  return values as number[];
};

describe('SCLK and kernel pool readers', () => {
  let spice: SpiceBindings;
  let engine: SpiceEngine;
  const tsc = parseTextKernel(fixtureText(TSC));
  const lsk = parseTextKernel(fixtureText(LSK));
  const fk = parseTextKernel(fixtureText(FK));

  // Fields of the type 1 SCLK kernel, read from its text.
  const moduli = numbers(tsc, `SCLK01_MODULI_${-SC}`);
  const offsets = numbers(tsc, `SCLK01_OFFSETS_${-SC}`);
  const coefficients = numbers(tsc, `SCLK01_COEFFICIENTS_${-SC}`);
  const partitionStart = numbers(tsc, `SCLK_PARTITION_START_${-SC}`);
  const partitionEnd = numbers(tsc, `SCLK_PARTITION_END_${-SC}`);
  const delimiter = ['.', ':', '-', ',', ' '][numbers(tsc, `SCLK01_OUTPUT_DELIM_${-SC}`)[0]! - 1]!;
  // Ticks per count of the most significant field: the product of the lower moduli.
  const ticksPerCount = moduli.slice(1).reduce((a, b) => a * b, 1);
  // Each field prints zero padded to the width of its largest value.
  const widths = moduli.map((modulus, i) => String(modulus - 1 + offsets[i]!).length);
  const clockString = (fields: number[]): string =>
    fields.map((v, i) => String(v).padStart(widths[i]!, '0')).join(delimiter);
  // The type 1 mapping from ticks to the parallel time system, for a tick value inside
  // the first (here the only) coefficient record.
  const [sclk0, parallel0, rate] = coefficients as [number, number, number];
  const ticksToParallel = (ticks: number): number => parallel0 + (rate * (ticks - sclk0)) / ticksPerCount;

  beforeAll(async () => {
    spice = await createSpiceBindings();
    engine = spiceEngineOver(spice);
    spice.furnsh(LSK, fixtureBytes(LSK));
    spice.furnsh(TSC, fixtureBytes(TSC));
    spice.furnsh(FK, fixtureBytes(FK));
  });

  it('reads the fixture kernels the way this test expects', () => {
    expect(moduli).toEqual([1e12, 1000]);
    expect(offsets).toEqual([0, 0]);
    expect(coefficients).toEqual([0, 0, 1]);
    expect(numbers(tsc, `SCLK01_TIME_SYSTEM_${-SC}`)).toEqual([1]); // 1 is TDB
    expect(delimiter).toBe('.');
    expect(numbers(lsk, 'DELTET/DELTA_AT')).toHaveLength(56);
    expect(numbers(lsk, 'DELTET/DELTA_AT').slice(-2)).toEqual([37, dateSeconds('@2017-JAN-1')]);
  });

  it('dtpool reports the size and type of every variable in the kernel text', () => {
    for (const kernel of [tsc, lsk, fk]) {
      expect(kernel.size).toBeGreaterThan(0);
      for (const [name, values] of kernel) {
        const type = typeof values[0] === 'string' ? 'C' : 'N';
        expect(spice.dtpool(name), name).toEqual({ found: true, n: values.length, type });
      }
    }
    expect(spice.dtpool('NO_SUCH_VARIABLE')).toEqual({ found: false, n: 0, type: 'X' });
  });

  it('gdpool returns every numeric variable as the kernel text states it', () => {
    let checked = 0;
    for (const kernel of [tsc, lsk, fk]) {
      for (const [name, values] of kernel) {
        if (typeof values[0] !== 'number') continue;
        const got = spice.gdpool(name);
        expect(got, name).not.toBeNull();
        expect(got, name).toHaveLength(values.length);
        values.forEach((v, i) => expect(got![i], `${name}[${i}]`).toBeCloseTo(v as number, 9));
        checked += values.length;
      }
    }
    // 14 values in the SCLK kernel, 61 in the leapseconds kernel, 6 in the frame kernel.
    expect(checked).toBe(14 + 61 + 6);
    // The kernel id is a date: 2004-01-01T00:00:00, 1460.5 days after J2000.
    expect(spice.gdpool('SCLK_KERNEL_ID')).toEqual([1460.5 * 86400]);
  });

  it('gdpool honors start and room, and returns null for an absent variable', () => {
    const name = 'DELTET/DELTA_AT';
    const all = numbers(lsk, name);
    expect(spice.gdpool(name, 54)).toEqual(all.slice(54));
    expect(spice.gdpool(name, 2, 4)).toEqual(all.slice(2, 6));
    expect(spice.gdpool(name, 0, 1)).toEqual([10]);
    expect(spice.gdpool(`SCLK01_MODULI_${-SC}`, 1, 1)).toEqual([moduli[1]]);
    expect(spice.gdpool(name, all.length)).toEqual([]);
    expect(spice.gdpool('NO_SUCH_VARIABLE')).toBeNull();
    expect(spice.gdpool('NO_SUCH_VARIABLE', 0, 3)).toBeNull();
  });

  it('gipool returns whole number variables and rejects values past 32 bits', () => {
    expect(spice.gipool(`SCLK01_N_FIELDS_${-SC}`)).toEqual(numbers(tsc, `SCLK01_N_FIELDS_${-SC}`));
    expect(spice.gipool(`SCLK_DATA_TYPE_${-SC}`)).toEqual(numbers(tsc, `SCLK_DATA_TYPE_${-SC}`));
    expect(spice.gipool(`SCLK01_OFFSETS_${-SC}`)).toEqual(offsets);
    expect(spice.gipool('DELTET/DELTA_AT')).toEqual(numbers(lsk, 'DELTET/DELTA_AT'));
    expect(spice.gipool('DELTET/DELTA_AT', 54, 1)).toEqual([37]);
    expect(spice.gipool('FRAME_-82000_CLASS')).toEqual(numbers(fk, 'FRAME_-82000_CLASS'));
    expect(spice.gipool('NO_SUCH_VARIABLE')).toBeNull();
    // A fractional value is rounded to the nearest integer: DELTET/K is 1.657e-3.
    expect(spice.gipool('DELTET/K')).toEqual([Math.round(numbers(lsk, 'DELTET/K')[0]!)]);
    // The first modulus is 10^12, which no 32 bit integer holds.
    expect(() => spice.gipool(`SCLK01_MODULI_${-SC}`)).toThrow(SpiceError);
    // The failed call leaves the bindings usable.
    expect(spice.gipool(`SCLK01_N_FIELDS_${-SC}`)).toEqual([2]);
  });

  it('gcpool returns character variables as the kernel text states them', () => {
    const name = 'FRAME_-82000_NAME';
    expect(fk.get(name)).toEqual(['BESSEL_CASSINI_CK']);
    expect(spice.gcpool(name)).toEqual(fk.get(name));
    expect(spice.gcpool(name, 0, 5)).toEqual(fk.get(name));
    expect(spice.gcpool('NO_SUCH_VARIABLE')).toBeNull();
  });

  it('a reader of the wrong type returns null and does not raise', () => {
    expect(spice.gdpool('FRAME_-82000_NAME')).toBeNull();
    expect(spice.gipool('FRAME_-82000_NAME')).toBeNull();
    expect(spice.gdpool('FRAME_-82000_NAME', 0, 0)).toBeNull();
    expect(spice.gcpool('DELTET/K')).toBeNull();
    expect(spice.gcpool('DELTET/K', 0, 1)).toBeNull();
  });

  it('scpart returns the partition bounds in the SCLK kernel', () => {
    expect(spice.scpart(SC)).toEqual({ pstart: partitionStart, pstop: partitionEnd });
    expect(partitionStart).toEqual([0]);
    expect(partitionEnd).toEqual([1e15]);
  });

  it('sctiks and scfmt convert between clock strings and ticks', () => {
    for (const [count, fine] of [[0, 0], [100, 500], [86400, 1], [123456789, 999]] as const) {
      const ticks = count * ticksPerCount + fine;
      expect(spice.sctiks(SC, `${count}${delimiter}${fine}`)).toBe(ticks);
      expect(spice.scfmt(SC, ticks)).toBe(clockString([count, fine]));
      expect(spice.sctiks(SC, spice.scfmt(SC, ticks))).toBe(ticks);
    }
    // A missing low field counts as zero.
    expect(spice.sctiks(SC, '100')).toBe(100 * ticksPerCount);
  });

  it('scencd and scdecd convert between partitioned clock strings and encoded SCLK', () => {
    const ticks = 100 * ticksPerCount + 500;
    // One partition starting at tick 0, so encoded SCLK equals the tick count.
    const encoded = ticks - partitionStart[0]!;
    expect(spice.scencd(SC, `1/100${delimiter}500`)).toBe(encoded);
    expect(spice.scencd(SC, `100${delimiter}500`)).toBe(encoded);
    expect(spice.scdecd(SC, encoded)).toBe(`1/${clockString([100, 500])}`);
    expect(spice.scencd(SC, spice.scdecd(SC, 987654321))).toBe(987654321);
    // The kernel has one partition, so partition 2 does not exist.
    expect(() => spice.scencd(SC, '2/100.500')).toThrow(SpiceError);
  });

  it('scs2e and sce2s convert between clock strings and ET through the coefficients', () => {
    const ticks = 100 * ticksPerCount + 500;
    // The parallel time system is TDB, so the mapped value is ET directly.
    const et = ticksToParallel(ticks);
    expect(et).toBe(100.5);
    expect(spice.scs2e(SC, `1/100${delimiter}500`)).toBeCloseTo(et, 9);
    expect(spice.sce2s(SC, et)).toBe(`1/${clockString([100, 500])}`);
    // Agreement with the existing continuous tick bindings.
    expect(spice.sce2c(SC, et)).toBeCloseTo(ticks, 6);
    expect(spice.sct2e(SC, spice.scencd(SC, '1/100.500'))).toBeCloseTo(et, 9);
  });

  it('raises a SpiceError for a clock with no SCLK kernel loaded', () => {
    expect(() => spice.scpart(-12345)).toThrow(SpiceError);
    expect(() => spice.scencd(-12345, '1/100.500')).toThrow(SpiceError);
    expect(() => spice.scfmt(-12345, 100)).toThrow(SpiceError);
  });

  it('unitim follows the constants in the leapseconds kernel', () => {
    const deltaTA = numbers(lsk, 'DELTET/DELTA_T_A')[0]!;
    const k = numbers(lsk, 'DELTET/K')[0]!;
    const eb = numbers(lsk, 'DELTET/EB')[0]!;
    const [m0, m1] = numbers(lsk, 'DELTET/M') as [number, number];
    expect(deltaTA).toBe(32.184);
    for (const tdb of [0, 141912064.184, -5e8]) {
      // TDB - TDT = K sin(E), E = M + EB sin(M), M = M0 + M1 t.
      const m = m0 + m1 * tdb;
      const tdt = tdb - k * Math.sin(m + eb * Math.sin(m));
      expect(spice.unitim(tdb, 'TDB', 'TDT')).toBeCloseTo(tdt, 8);
      expect(spice.unitim(tdb, 'TDB', 'TAI')).toBeCloseTo(tdt - deltaTA, 8);
      expect(spice.unitim(tdb, 'TDT', 'TAI')).toBeCloseTo(tdb - deltaTA, 9);
      expect(spice.unitim(tdb, 'ET', 'TDB')).toBe(tdb);
    }
    expect(() => spice.unitim(0, 'TDB', 'NOT_A_SYSTEM')).toThrow(SpiceError);
  });

  it('the promise engine returns what the synchronous bindings return', async () => {
    const name = 'DELTET/DELTA_AT';
    expect(await engine.scpart(SC)).toEqual(spice.scpart(SC));
    expect(await engine.dtpool(name)).toEqual(spice.dtpool(name));
    expect(await engine.gdpool(name)).toEqual(spice.gdpool(name));
    expect(await engine.gdpool(name, 2, 4)).toEqual(spice.gdpool(name, 2, 4));
    expect(await engine.gipool(name, 54)).toEqual(spice.gipool(name, 54));
    expect(await engine.gcpool('FRAME_-82000_NAME')).toEqual(['BESSEL_CASSINI_CK']);
    expect(await engine.gdpool('NO_SUCH_VARIABLE')).toBeNull();
    expect(await engine.scencd(SC, '1/100.500')).toBe(spice.scencd(SC, '1/100.500'));
    expect(await engine.scdecd(SC, 100500)).toBe(spice.scdecd(SC, 100500));
    expect(await engine.scs2e(SC, '1/100.500')).toBe(spice.scs2e(SC, '1/100.500'));
    expect(await engine.sce2s(SC, 100.5)).toBe(spice.sce2s(SC, 100.5));
    expect(await engine.sctiks(SC, '100.500')).toBe(spice.sctiks(SC, '100.500'));
    expect(await engine.scfmt(SC, 100500)).toBe(spice.scfmt(SC, 100500));
    expect(await engine.unitim(0, 'TDB', 'TDT')).toBe(spice.unitim(0, 'TDB', 'TDT'));
    await expect(engine.gipool(`SCLK01_MODULI_${-SC}`)).rejects.toThrow(SpiceError);
  });

  it('the worker protocol dispatches every new method', async () => {
    const requests: [SpiceWorkerRequest, unknown][] = [
      [{ id: 1, method: 'scpart', sc: SC }, spice.scpart(SC)],
      [{ id: 2, method: 'dtpool', name: 'DELTET/K' }, { found: true, n: 1, type: 'N' }],
      [{ id: 3, method: 'gdpool', name: 'DELTET/DELTA_AT', start: 54, room: 2 }, spice.gdpool('DELTET/DELTA_AT', 54, 2)],
      [{ id: 4, method: 'gipool', name: 'DELTET/DELTA_AT', start: 54, room: 1 }, [37]],
      [{ id: 5, method: 'gcpool', name: 'FRAME_-82000_NAME' }, ['BESSEL_CASSINI_CK']],
      [{ id: 6, method: 'scencd', sc: SC, sclkch: '1/100.500' }, 100500],
      [{ id: 7, method: 'scdecd', sc: SC, sclkdp: 100500 }, spice.scdecd(SC, 100500)],
      [{ id: 8, method: 'scs2e', sc: SC, sclkch: '1/100.500' }, spice.scs2e(SC, '1/100.500')],
      [{ id: 9, method: 'sce2s', sc: SC, et: 100.5 }, spice.sce2s(SC, 100.5)],
      [{ id: 10, method: 'sctiks', sc: SC, clkstr: '100.500' }, 100500],
      [{ id: 11, method: 'scfmt', sc: SC, ticks: 100500 }, spice.scfmt(SC, 100500)],
      [{ id: 12, method: 'unitim', epoch: 0, insys: 'TDB', outsys: 'TDT' }, spice.unitim(0, 'TDB', 'TDT')],
    ];
    for (const [req, want] of requests) {
      expect(await dispatchSpice(engine, req), req.method).toEqual(want);
    }
  });
});
