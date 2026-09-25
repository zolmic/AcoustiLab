// Unit tests of the audition DSP (web/src/audio/), run by Playwright in
// Node without a browser. The same modules run in the AudioWorklet and the
// level-match worker; listen.spec.ts runs them through Chromium's
// OfflineAudioContext.
//
// Oracles, each independent of the code under test: a direct DFT; direct
// (time-domain) convolution; the published BS.1770 coefficients and the
// analogue parameters libebur128 uses; EBU Tech 3341 test signals whose
// loudness is derived here from the K-weighting's magnitude at 1 kHz; a
// 16× windowed-sinc reconstruction written here for true peaks; the
// xoshiro128** test vector of the reference implementation (as in the
// rand_xoshiro crate) and a Python transcription of the seeding; Kellet's
// published coefficients.

import { expect, test } from '@playwright/test';
import { FFT } from '../src/audio/fft';
import { BLOCK, PartitionedConvolver } from '../src/audio/convolver';
import { cascadeMagnitude, integratedLoudness, K48, kPrototype, kWeighting, StreamingLoudness } from '../src/audio/loudness';
import { TruePeakLimiter } from '../src/audio/limiter';
import { phaseCoefficients, truePeak, PeakDetector, TAPS_PER_PHASE } from '../src/audio/oversample';
import { circularConvolve, delayTaps, matchGainDb, measure } from '../src/audio/level';
import {
  generate,
  kelletMagnitude,
  loopLength,
  pinkDeviationDb,
  pinkFilter,
  pinkKellet,
  pinkVoss,
  sineSweep,
  vossRows,
  vossSpectrum,
  whiteNoise,
  Xoshiro128,
} from '../src/audio/noise';
import { PINK_KELLET } from '../src/audio/pink-coefficients';

const db = (x: number) => 20 * Math.log10(x);

function rng(seed: number): () => number {
  const r = new Xoshiro128(seed);
  return () => 2 * r.uniform() - 1;
}

function directConvolve(x: ArrayLike<number>, h: ArrayLike<number>, n = x.length): Float64Array {
  const y = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    let s = 0;
    for (let m = 0; m < h.length && m <= i; m++) s += h[m] * x[i - m];
    y[i] = s;
  }
  return y;
}

/** 16× reconstruction (Kaiser-windowed sinc, 64 samples each side, β = 12): an independent true peak. */
function truePeak16(x: ArrayLike<number>): number {
  const half = 64;
  const beta = 12;
  const i0 = (v: number) => {
    let s = 1;
    let t = 1;
    for (let k = 1; k < 300; k++) {
      t *= (v * v) / (4 * k * k);
      s += t;
      if (t < 1e-18 * s) break;
    }
    return s;
  };
  const kernel = (t: number) => {
    if (Math.abs(t) >= half) return 0;
    const w = i0(beta * Math.sqrt(1 - (t / half) ** 2)) / i0(beta);
    return t === 0 ? 1 : (Math.sin(Math.PI * t) / (Math.PI * t)) * w;
  };
  const table = new Map<number, Float64Array>();
  for (let p = 1; p < 16; p++) {
    const c = new Float64Array(2 * half);
    for (let j = 0; j < 2 * half; j++) c[j] = kernel(p / 16 - (j - half + 1));
    table.set(p, c);
  }
  let peak = 0;
  for (let m = 0; m < x.length; m++) {
    peak = Math.max(peak, Math.abs(x[m]));
    for (let p = 1; p < 16; p++) {
      const c = table.get(p)!;
      let v = 0;
      for (let j = 0; j < 2 * half; j++) {
        const i = m + j - half + 1;
        if (i >= 0 && i < x.length) v += c[j] * x[i];
      }
      peak = Math.max(peak, Math.abs(v));
    }
  }
  return peak;
}

// ----- FFT and convolution ----------------------------------------------------

test('FFT equals a direct DFT and inverts', () => {
  for (const n of [1, 2, 8, 64, 256, 1024]) {
    const r = rng(n);
    const re = Float64Array.from({ length: n }, r);
    const im = Float64Array.from({ length: n }, r);
    const fr = re.slice();
    const fi = im.slice();
    new FFT(n).forward(fr, fi);
    let worst = 0;
    for (let k = 0; k < n; k++) {
      let sr = 0;
      let si = 0;
      for (let t = 0; t < n; t++) {
        const a = (-2 * Math.PI * ((k * t) % n)) / n;
        sr += re[t] * Math.cos(a) - im[t] * Math.sin(a);
        si += re[t] * Math.sin(a) + im[t] * Math.cos(a);
      }
      worst = Math.max(worst, Math.hypot(sr - fr[k], si - fi[k]));
    }
    expect(worst).toBeLessThan(1e-12 * n);
    new FFT(n).inverse(fr, fi);
    for (let t = 0; t < n; t++) expect(Math.abs(fr[t] - re[t]) + Math.abs(fi[t] - im[t])).toBeLessThan(1e-13);
  }
});

test('partitioned convolution equals direct convolution; loads and crossfades are exact', () => {
  const r = rng(7);
  const taps = 3000;
  const hA = Float64Array.from({ length: taps }, (_, i) => r() * Math.exp(-i / 600));
  const hB = Float64Array.from({ length: 1700 }, (_, i) => r() * Math.exp(-i / 300));
  const blocks = 60;
  const n = blocks * BLOCK;
  const xL = Float64Array.from({ length: n }, r);
  const xR = Float64Array.from({ length: n }, r);
  const conv = new PartitionedConvolver(4096, 2, 3);
  conv.setFilter(0, [hA, hB], 0.5);
  const yL = new Float64Array(n);
  const yR = new Float64Array(n);
  // Double-precision output buffers for the exactness check.
  const out64 = [new Float64Array(BLOCK), new Float64Array(BLOCK)];
  for (let b = 0; b < blocks; b++) {
    conv.process([xL.subarray(b * BLOCK, (b + 1) * BLOCK), xR.subarray(b * BLOCK, (b + 1) * BLOCK)], out64 as unknown as Float32Array[]);
    yL.set(out64[0], b * BLOCK);
    yR.set(out64[1], b * BLOCK);
  }
  const dL = directConvolve(xL, hA);
  const dR = directConvolve(xR, hB);
  let worst = 0;
  for (let i = 0; i < n; i++) worst = Math.max(worst, Math.abs(yL[i] - 0.5 * dL[i]), Math.abs(yR[i] - 0.5 * dR[i]));
  expect(worst).toBeLessThan(1e-12);

  // An incremental load (4 partitions per block) gives the same bank, and
  // the crossfade is exactly (1 − g)·y_A + g·y_B with g rising over 256
  // samples: no step, whatever the filters.
  const c2 = new PartitionedConvolver(4096, 2, 3);
  c2.setFilter(0, [hA], 1);
  expect(c2.beginLoad(1, [hB], 2)).toBe(true);
  let steps = 0;
  while (!c2.loadStep(4)) steps++;
  expect(steps).toBe(Math.ceil(1700 / BLOCK / 4) - 1);
  const y2 = new Float64Array(n);
  const start = 20;
  for (let b = 0; b < blocks; b++) {
    if (b === start) expect(c2.crossfadeTo(1)).toBe(true);
    c2.process([xL.subarray(b * BLOCK, (b + 1) * BLOCK)], out64 as unknown as Float32Array[]);
    y2.set(out64[0], b * BLOCK);
  }
  // An incremental load keeps the taps as f32 (as the worklet receives them).
  const yb = directConvolve(xL, Float64Array.from(hB, Math.fround));
  let worstFade = 0;
  for (let i = 0; i < n; i++) {
    const k = i - start * BLOCK;
    const g = k < 0 ? 0 : Math.min(1, (k + 1) / 256);
    worstFade = Math.max(worstFade, Math.abs(y2[i] - ((1 - g) * dL[i] + g * 2 * yb[i])));
  }
  expect(worstFade).toBeLessThan(1e-12);
  // Loading into the bank that plays is refused.
  expect(c2.beginLoad(1, [hA], 1)).toBe(false);
});

// ----- K-weighting and loudness ---------------------------------------------------

test('K-weighting: prototype from the published 48 kHz coefficients, other rates', () => {
  const p = kPrototype();
  // libebur128 (MIT) uses these analogue parameters for every rate.
  expect(p.shelf.f0 / 1681.974450955533 - 1).toBeLessThan(1e-12);
  expect(Math.abs(db(p.shelf.vh) - 3.999843853973347)).toBeLessThan(1e-11);
  expect(Math.abs(p.shelf.q / 0.7071752369554196 - 1)).toBeLessThan(1e-12);
  expect(Math.abs(Math.log(p.shelf.vb) / Math.log(p.shelf.vh) - 0.4996667741545416)).toBeLessThan(1e-12);
  expect(Math.abs(p.highpass.f0 / 38.13547087602444 - 1)).toBeLessThan(1e-11);
  expect(Math.abs(p.highpass.q / 0.5003270373238773 - 1)).toBeLessThan(1e-11);
  // Round trip at 48 kHz.
  const [shelf, hp] = kWeighting(48000);
  for (let i = 0; i < 3; i++) {
    expect(Math.abs(shelf.b[i] - K48.shelf.b[i])).toBeLessThan(1e-14);
    expect(Math.abs(shelf.a[i] - K48.shelf.a[i])).toBeLessThan(1e-14);
    expect(Math.abs(hp.b[i] - K48.highpass.b[i])).toBeLessThan(1e-14);
    expect(Math.abs(hp.a[i] - K48.highpass.a[i])).toBeLessThan(1e-14);
  }
  // Other rates keep the 48 kHz response over 20 Hz–20 kHz (or 0.9 of Nyquist).
  const ref = (f: number) => cascadeMagnitude(kWeighting(48000), 48000, f);
  const worst: Record<number, number> = {};
  for (const fs of [44100, 88200, 96000, 192000]) {
    const k = kWeighting(fs);
    let w = 0;
    for (let i = 0; i <= 300; i++) {
      const f = 20 * 1000 ** (i / 300);
      if (f > 0.9 * fs / 2) continue;
      w = Math.max(w, Math.abs(db(cascadeMagnitude(k, fs, f)) - db(ref(f))));
    }
    worst[fs] = w;
  }
  // The rates warp the shelf differently where it is still rising (its
  // f0 is 1.68 kHz). Observed over 20 Hz–20 kHz: 0.0015 dB at 44.1 kHz,
  // 0.0058 at 88.2, 0.0062 at 96 and 0.0077 at 192 kHz; at 1 kHz −0.0011
  // and +0.0043 dB at 44.1 and 96 kHz. Asserted 0.01 and 0.005 dB.
  for (const fs of [44100, 88200, 96000, 192000]) expect(worst[fs], `${fs}`).toBeLessThan(0.01);
  for (const fs of [44100, 96000]) {
    expect(Math.abs(db(cascadeMagnitude(kWeighting(fs), fs, 1000)) - db(ref(1000)))).toBeLessThan(0.005);
  }
});

/** A stereo 1 kHz sine whose level (dBFS peak) follows `segments` [dBFS, seconds]. */
function toneSegments(fs: number, segments: [number, number][]): Float64Array {
  const n = Math.round(segments.reduce((a, [, s]) => a + s, 0) * fs);
  const x = new Float64Array(n);
  let i = 0;
  for (const [level, s] of segments) {
    const a = 10 ** (level / 20);
    const end = i + Math.round(s * fs);
    for (; i < end; i++) x[i] = a * Math.sin((2 * Math.PI * 1000 * i) / fs);
  }
  return x;
}

/**
 * The BS.1770 integrated loudness of such a signal, derived: each 100 ms
 * step holds whole cycles, so its mean square per channel is
 * |K(1 kHz)|²·a²/2; blocks are four steps; then the two gates.
 */
function derivedLoudness(fs: number, segments: [number, number][]): number {
  const k2 = cascadeMagnitude(kWeighting(fs), fs, 1000) ** 2;
  const steps: number[] = [];
  for (const [level, s] of segments) for (let j = 0; j < Math.round(s * 10); j++) steps.push((2 * k2 * 10 ** (level / 10)) / 2);
  const z: number[] = [];
  for (let j = 0; j + 4 <= steps.length; j++) z.push((steps[j] + steps[j + 1] + steps[j + 2] + steps[j + 3]) / 4);
  const l = (v: number) => -0.691 + 10 * Math.log10(v);
  const abs = z.filter((v) => l(v) > -70);
  const rel = l(abs.reduce((a, b) => a + b, 0) / abs.length) - 10;
  const kept = abs.filter((v) => l(v) > rel);
  return l(kept.reduce((a, b) => a + b, 0) / kept.length);
}

test('integrated loudness on EBU Tech 3341 signals (derived values and −23.0 ± 0.1 LUFS)', () => {
  const cases: [string, [number, number][], number][] = [
    ['case 1', [[-23, 20]], -23],
    ['case 2', [[-33, 20]], -33],
    ['case 3', [[-36, 10], [-23, 60], [-36, 10]], -23],
    ['case 4', [[-72, 10], [-36, 10], [-23, 60], [-36, 10], [-72, 10]], -23],
    ['case 5', [[-26, 20], [-20, 20.1], [-26, 20]], -23],
  ];
  for (const fs of [48000, 44100, 96000]) {
    for (const [name, seg, ebu] of cases) {
      if (fs !== 48000 && name !== 'case 1' && name !== 'case 5') continue;
      const x = toneSegments(fs, seg);
      const l = integratedLoudness([x, x], fs).integrated;
      const want = derivedLoudness(fs, seg);
      expect(Math.abs(l - want), `${name} at ${fs}: ${l} vs derived ${want}`).toBeLessThan(0.01);
      expect(Math.abs(l - ebu), `${name} at ${fs}: ${l}`).toBeLessThanOrEqual(0.1);
    }
  }
  // Gates: silence reads −∞; the absolute gate drops blocks below −70 LKFS.
  expect(integratedLoudness([new Float64Array(48000), new Float64Array(48000)], 48000).integrated).toBe(-Infinity);
  const quiet = toneSegments(48000, [[-80, 5]]);
  expect(integratedLoudness([quiet, quiet], 48000).kept).toBe(0);
});

test('streaming momentary and short-term loudness', () => {
  const fs = 48000;
  const x = toneSegments(fs, [[-23, 4]]);
  const m = new StreamingLoudness(fs);
  for (let i = 0; i < x.length; i += 1000) m.push([x.subarray(i), x.subarray(i)], Math.min(1000, x.length - i));
  expect(Math.abs(m.momentary() + 23)).toBeLessThan(0.05);
  expect(Math.abs(m.shortTerm() + 23)).toBeLessThan(0.05);
});

// ----- true peak and limiter -------------------------------------------------------

test('true peak: 4× interpolation reads inter-sample peaks', () => {
  const fs = 48000;
  const n = 4800;
  // fs/4 at 45°: samples at ±0.707 of the peak (−3.01 dBFS), true peak 0 dBTP.
  const q = Float64Array.from({ length: n }, (_, i) => Math.sin((Math.PI / 2) * i + Math.PI / 4));
  const samplePeak = q.reduce((m, v) => Math.max(m, Math.abs(v)), 0);
  expect(Math.abs(db(samplePeak) + 3.0103)).toBeLessThan(1e-3);
  expect(Math.abs(db(truePeak(q, 100, n - 100)))).toBeLessThan(0.002);
  // 997 Hz at 0 dBFS reads 0 dBTP.
  const s = Float64Array.from({ length: n }, (_, i) => Math.sin((2 * Math.PI * 997 * i) / fs));
  expect(Math.abs(db(truePeak(s, 100, n - 100)))).toBeLessThan(0.002);
  // Cut off abruptly, the same signal overshoots near its ends (why
  // truePeak takes a range).
  expect(db(truePeak(q.subarray(100, n - 100)))).toBeGreaterThan(0.05);
  // The interpolator reconstructs sinusoids to 20 kHz within 0.002 dB.
  const c = phaseCoefficients();
  for (const f of [100, 1000, 10000, 15000, 20000]) {
    const w = (2 * Math.PI * f) / fs;
    c.forEach((ph, p) => {
      const d = (p + 1) / 4;
      let re = 0;
      let im = 0;
      for (let j = 0; j < TAPS_PER_PHASE; j++) {
        const i = j - TAPS_PER_PHASE / 2 + 1;
        re += ph[j] * Math.cos(w * (i - d));
        im += ph[j] * Math.sin(w * (i - d));
      }
      expect(Math.abs(db(Math.hypot(re, im))), `${f} Hz phase ${d}`).toBeLessThan(0.002);
    });
  }
  // The streaming detector sees the same peaks as the block function.
  const det = new PeakDetector();
  let peak = 0;
  for (let i = 0; i < n; i++) {
    const v = det.push(q[i]);
    if (i > 200) peak = Math.max(peak, v);
  }
  expect(Math.abs(db(peak))).toBeLessThan(0.002);
});

test('limiter: transparent below the ceiling, holds the true-peak ceiling above it', () => {
  const fs = 48000;
  const lim = new TruePeakLimiter(fs, { ceilingDb: -1 });
  const n = 48000;
  // Pink noise 6 dB above the ceiling in peaks, plus inter-sample-peak bursts.
  const pink = pinkKellet(loopLength(fs), 5, fs).subarray(0, n);
  const k = 10 ** (5 / 20) / pink.reduce((m, v) => Math.max(m, Math.abs(v)), 0);
  const xl = new Float64Array(n);
  const xr = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    xl[i] = pink[i] * k;
    xr[i] = pink[i] * k * 0.5;
  }
  for (let i = 20000; i < 22000; i++) xl[i] = 1.8 * Math.sin((Math.PI / 2) * i + Math.PI / 4);
  const yl = new Float32Array(n);
  const yr = new Float32Array(n);
  const g = new Float32Array(n);
  lim.process(xl, xr, yl, yr, n, g);
  const tp = db(Math.max(truePeak16(yl), truePeak16(yr)));
  expect(tp, `output true peak ${tp} dBTP`).toBeLessThanOrEqual(-1 + 1e-3);
  expect(g.reduce((m, v) => Math.min(m, v), 1)).toBeLessThan(0.5);
  // Below the ceiling it only delays (by its latency), exactly.
  const lim2 = new TruePeakLimiter(fs, { ceilingDb: -1 });
  const small = Float64Array.from({ length: 4000 }, (_, i) => 0.3 * Math.sin(i / 7));
  const a = new Float32Array(4000);
  const b = new Float32Array(4000);
  lim2.process(small, small, a, b, 4000);
  for (let i = lim2.latency; i < 4000; i++) expect(a[i]).toBe(Math.fround(small[i - lim2.latency]));
  expect(lim2.latency).toBe(TAPS_PER_PHASE / 2 + Math.round(0.002 * fs));
});

// ----- programme material ---------------------------------------------------------------

test('xoshiro128** and its seeding', () => {
  // Reference test vector (state 1, 2, 3, 4), as in the rand_xoshiro crate.
  const x = new Xoshiro128(Uint32Array.from([1, 2, 3, 4]));
  expect(Array.from({ length: 6 }, () => x.nextU32())).toEqual([11520, 0, 5927040, 70819200, 2031721883, 1637235492]);
  // Seeded by SplitMix64 (Python transcription, tools/audio: values below).
  const s1 = new Xoshiro128(1);
  expect(Array.from({ length: 6 }, () => s1.nextU32())).toEqual([1695105466, 1423115009, 634581793, 1068227753, 716759206, 4186505319]);
  const s2 = new Xoshiro128(123456789);
  expect(Array.from({ length: 4 }, () => s2.nextU32())).toEqual([1346704765, 2882174560, 1840973710, 4030037534]);
  // Normal deviates: mean 0, variance 1 (1e6 draws: 4σ bounds).
  const g = whiteNoise(1_000_000, 9);
  const mean = g.reduce((a, b) => a + b, 0) / g.length;
  const v = g.reduce((a, b) => a + (b - mean) ** 2, 0) / g.length;
  expect(Math.abs(mean)).toBeLessThan(0.004);
  expect(Math.abs(v - 1)).toBeLessThan(0.006);
});

test('pink noise (Kellet): −3.01 dB/octave within 0.05 dB from 20 Hz to 20 kHz; loops seamlessly', () => {
  // Kellet's published coefficients, as quoted, at 44.1 kHz.
  const k44 = PINK_KELLET[44100];
  expect(k44.poles).toEqual([0.99886, 0.99332, 0.969, 0.8665, 0.55, -0.7616]);
  const ratio = k44.gains.map((g, i) => g / [0.0555179, 0.0750759, 0.153852, 0.3104856, 0.5329522, -0.016898][i]);
  for (const q of ratio) expect(Math.abs(q / ratio[0] - 1)).toBeLessThan(1e-12);
  for (const fs of [44100, 48000, 88200, 96000]) {
    const c = PINK_KELLET[fs];
    const dev = pinkDeviationDb((f) => kelletMagnitude(c, fs, f), 20, Math.min(20000, 0.9 * fs / 2));
    expect(dev, `${fs} Hz`).toBeLessThan(0.05);
    expect(Math.abs(dev - c.maxErrorAudioDb)).toBeLessThan(0.005);
    expect(Math.abs(kelletMagnitude(c, fs, 1000) - 1)).toBeLessThan(1e-12);
  }
  // Another rate: no coefficients; spectral shaping, said so.
  expect(pinkFilter(32000)).toBeNull();
  const odd = generate('pink_kellet', 3, 32000);
  expect(odd.label).toBe('Pink noise (spectral shaping)');
  expect(odd.notes[0]).toContain('shaped by 1/√f');
  const oddBands = bandLevels(Float64Array.from(odd.channels[0]), 32000).filter((b) => b.f >= 100 && b.f <= 12500);
  const oddSpread = Math.max(...oddBands.map((b) => b.db)) - Math.min(...oddBands.map((b) => b.db));
  expect(oddSpread).toBeLessThan(1.5);
  // Generated noise: third-octave band powers of 2^21 samples follow the
  // line (least-squares slope −3.01 ± 0.05 dB/octave over 50 Hz–16 kHz;
  // each band within 0.3 dB above 200 Hz, where a band holds enough
  // spectral lines for the estimate's own scatter to stay under 0.1 dB).
  const fs = 48000;
  const n = 1 << 21;
  const x = pinkKellet(n, 11, fs);
  const bands = bandLevels(x, fs);
  const pts = bands.filter((b) => b.f >= 50 && b.f <= 16000);
  const u = pts.map((b) => Math.log2(b.f));
  const y = pts.map((b) => b.db);
  const um = u.reduce((a, b) => a + b, 0) / u.length;
  const ym = y.reduce((a, b) => a + b, 0) / y.length;
  const slope = u.reduce((a, ui, i) => a + (ui - um) * (y[i] - ym), 0) / u.reduce((a, ui) => a + (ui - um) ** 2, 0);
  // Band power of pink noise in a constant-percentage band is constant:
  // spectral density falls 3.01 dB/octave, band level stays flat.
  expect(Math.abs(slope), `band-level slope ${slope} dB/octave (density ${slope - 3.0103})`).toBeLessThan(0.05);
  for (const b of pts.filter((b) => b.f >= 200)) expect(Math.abs(b.db - (ym + slope * (Math.log2(b.f) - um))), `${b.f} Hz`).toBeLessThan(0.3);
  // Periodic: the filter run on over a second period repeats the loop.
  const m = 1 << 16;
  const loop = pinkKellet(m, 4, fs);
  const w = whiteNoise(m, 4);
  const c = pinkFilter(fs)!;
  const bs = new Float64Array(6);
  let prev = 0;
  const twice = new Float64Array(3 * m);
  for (let i = 0; i < 3 * m; i++) {
    const v = w[i % m];
    let s = c.direct * v + c.delayed * prev;
    for (let j = 0; j < 6; j++) {
      bs[j] = c.poles[j] * bs[j] + c.gains[j] * v;
      s += bs[j];
    }
    prev = v;
    twice[i] = s;
  }
  let worst = 0;
  for (let i = 0; i < m; i++) worst = Math.max(worst, Math.abs(twice[2 * m + i] - loop[i]));
  expect(worst).toBeLessThan(1e-9 * loop.reduce((m, v) => Math.max(m, Math.abs(v)), 0));
});

/** Third-octave band levels (IEC 61260 base-10 midbands) from one big periodogram. */
function bandLevels(x: Float64Array, fs: number): { f: number; db: number }[] {
  const n = x.length;
  const fft = new FFT(n);
  const re = Float64Array.from(x);
  const im = new Float64Array(n);
  fft.forward(re, im);
  const out: { f: number; db: number }[] = [];
  for (let k = 13; k <= 43; k++) {
    const fm = 10 ** (k / 10);
    const lo = fm * 10 ** (-1 / 20);
    const hi = fm * 10 ** (1 / 20);
    let p = 0;
    for (let b = Math.ceil((lo * n) / fs); b <= Math.floor((hi * n) / fs); b++) p += re[b] ** 2 + im[b] ** 2;
    out.push({ f: fm, db: 10 * Math.log10(p) });
  }
  return out;
}

test('pink noise (Voss–McCartney): spectrum as derived', () => {
  const fs = 48000;
  const rows = vossRows(fs);
  expect(rows).toBe(16);
  // Third-octave band powers of one periodogram of 2^21 samples against the
  // derived spectrum integrated over each band. A band's estimate scatters
  // by 1/√(lines in it): 2.8 % (0.12 dB) at 125 Hz, under 1 % above 1 kHz;
  // asserted 0.5 dB from 125 Hz and 0.15 dB from 1 kHz, to 16 kHz.
  const n = 1 << 21;
  const x = pinkVoss(n, 2, rows);
  const fft = new FFT(n);
  const re = Float64Array.from(x);
  const im = new Float64Array(n);
  fft.forward(re, im);
  const scale = 1 / (rows + 1);
  const offsets: number[] = [];
  const worst = { low: 0, high: 0 };
  for (let k = 21; k <= 42; k++) {
    const fm = 10 ** (k / 10);
    const lo = fm * 10 ** (-1 / 20);
    const hi = fm * 10 ** (1 / 20);
    let p = 0;
    let lines = 0;
    for (let b = Math.ceil((lo * n) / fs); b <= Math.floor((hi * n) / fs); b++) {
      p += (re[b] ** 2 + im[b] ** 2) / n;
      lines++;
    }
    // The model's mean over the same lines (trapezoid on a fine grid).
    let m = 0;
    const steps = 4000;
    for (let i = 0; i <= steps; i++) {
      const f = lo + ((hi - lo) * i) / steps;
      m += (i === 0 || i === steps ? 0.5 : 1) * vossSpectrum(f, fs, rows) * scale;
    }
    m /= steps;
    const e = 10 * Math.log10(p / lines / m);
    offsets.push(e);
    if (fm >= 1000) worst.high = Math.max(worst.high, Math.abs(e));
    else worst.low = Math.max(worst.low, Math.abs(e));
  }
  expect(worst.low, `${offsets}`).toBeLessThan(0.5);
  expect(worst.high, `${offsets}`).toBeLessThan(0.15);
  // Its deviation from −3.01 dB/octave is reported, not hidden.
  const p = generate('pink_voss', 2, fs);
  expect(p.notes[0]).toMatch(/departs from −3\.01 dB\/octave by up to \d+\.\d\d dB/);
});

test('sweep, programmes and level matching', () => {
  const fs = 48000;
  const s = sineSweep(1 << 19, fs);
  expect(s[0]).toBe(0);
  expect(Math.abs(s[s.length - 1])).toBeLessThan(1e-3);
  const p = generate('sweep', 0, fs);
  expect(p.matchBy).toBe('rms');
  expect(Math.abs(db(p.channels[0].reduce((m, v) => Math.max(m, Math.abs(v)), 0)) + 6)).toBeLessThan(0.01);
  const pk = generate('pink_kellet', 1, fs);
  expect(pk.channels[0].length).toBe(1 << 19);
  expect(pk.matchBy).toBe('bs1770');
  // Circular convolution: against a direct circular sum on a short loop.
  const r = rng(5);
  const x = Float64Array.from({ length: 777 }, r);
  const h = Float64Array.from({ length: 300 }, r);
  const y = circularConvolve(x, h);
  for (const i of [0, 1, 150, 299, 400, 776]) {
    let v = 0;
    for (let m = 0; m < h.length; m++) v += h[m] * x[(((i - m) % 777) + 777) % 777];
    expect(Math.abs(y[i] - v)).toBeLessThan(1e-12);
  }
  // A filter with a +6 dB treble shelf, matched: re-measured, A and B agree to 1e-6 LU.
  const prog = pk.channels;
  const shelf = new Float32Array(64);
  shelf[0] = 1.5;
  shelf[1] = -0.5;
  const delay = delayTaps(0);
  const plain = measure(prog, fs);
  const a = measure([circularConvolve(prog[0], shelf), circularConvolve(prog[1], shelf)], fs);
  const b = measure([circularConvolve(prog[0], delay), circularConvolve(prog[1], delay)], fs);
  const ga = matchGainDb(a, 'bs1770', plain);
  const gb = matchGainDb(b, 'bs1770', plain);
  const scaled = (sig: Float64Array, gDb: number) => sig.map((v) => v * 10 ** (gDb / 20));
  const la = integratedLoudness([scaled(circularConvolve(prog[0], shelf), ga), scaled(circularConvolve(prog[1], shelf), ga)], fs, true).integrated;
  const lb = integratedLoudness([scaled(circularConvolve(prog[0], delay), gb), scaled(circularConvolve(prog[1], delay), gb)], fs, true).integrated;
  expect(Math.abs(la - lb)).toBeLessThan(1e-6);
  expect(Math.abs(la + 23)).toBeLessThan(1e-6);
  expect(ga).toBeLessThan(gb);
});
