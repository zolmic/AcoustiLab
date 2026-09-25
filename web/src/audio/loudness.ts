// Loudness per ITU-R BS.1770-5 (K-weighting, mean square, gating) and the
// meters of spec Section 16.
//
// K-weighting at any rate. BS.1770 defines the two K-weighting stages by
// their biquad coefficients at 48 kHz only. Both are bilinear transforms
// of analogue second-order sections with prewarping at their own
// frequency (the RBJ Audio EQ Cookbook forms), so the analogue prototype
// can be recovered exactly from the 48 kHz values and transformed again
// at another rate. With K = tan(π·f0/fs) and a0 = 1 + K/Q + K²:
//
//   high shelf:  b = [Vh + Vb·K/Q + K², 2(K² − Vh), Vh − Vb·K/Q + K²]/a0,
//                a = [1, 2(K² − 1)/a0, (1 − K/Q + K²)/a0]
//   high-pass:   b = G·[1, −2, 1]/a0, same a
//
// From a (normalised) biquad of that form: 1 + a1 + a2 = 4K²/a0 and
// 1 − a1 + a2 = 4/a0, so K² = (1 + a1 + a2)/(1 − a1 + a2),
// Q = K·(1 − a1 + a2)/(2·(1 − a2)), Vh = (b0 − b1 + b2)/(1 − a1 + a2),
// Vb = (b0 − b2)·a0·Q/(2K), and for the high-pass G = a0 (the published
// numerator is [1, −2, 1], so its passband gain is a0 = 1.00499, +0.043 dB
// at 48 kHz, which the −0.691 dB constant of BS.1770 absorbs). The results,
// f0 = 1681.974 Hz, Q = 0.70718, Vh = +3.99984 dB, Vb = Vh^0.49967 for the
// shelf and f0 = 38.135 Hz, Q = 0.50033 for the high-pass, agree to 12
// digits with the parameters libebur128 uses; re-deriving the 48 kHz
// coefficients from them reproduces the published values to 1e-14. At
// other rates this keeps G (libebur128 keeps the numerator [1, −2, 1],
// which lowers the passband by 0.022 dB at 96 kHz).
//
// Gating (BS.1770-5, Annex 1): blocks of 400 ms overlapping by 75 %; block
// loudness l_j = −0.691 + 10·log10(Σ_i G_i·z_ij) with G_L = G_R = 1;
// absolute gate −70 LKFS; relative gate 10 LU below the loudness of the
// blocks above the absolute gate; integrated loudness over the blocks above
// both gates. Only whole blocks count.

import { truePeak } from './oversample';

/** Published K-weighting coefficients at 48 kHz (ITU-R BS.1770-5, Tables 1 and 2). */
export const K48 = {
  shelf: { b: [1.53512485958697, -2.69169618940638, 1.19839281085285], a: [1, -1.69065929318241, 0.73248077421585] },
  highpass: { b: [1.0, -2.0, 1.0], a: [1, -1.99004745483398, 0.99007225036621] },
};

export interface Biquad {
  b: [number, number, number];
  a: [number, number, number];
}

export interface KPrototype {
  shelf: { f0: number; q: number; vh: number; vb: number };
  highpass: { f0: number; q: number; gain: number };
}

/** The analogue prototype recovered from the 48 kHz coefficients. */
export function kPrototype(): KPrototype {
  const fs = 48000;
  const shelf = (() => {
    const [b0, b1, b2] = K48.shelf.b;
    const [, a1, a2] = K48.shelf.a;
    const k = Math.sqrt((1 + a1 + a2) / (1 - a1 + a2));
    const q = (k * (1 - a1 + a2)) / (2 * (1 - a2));
    const a0 = 4 / (1 - a1 + a2);
    return {
      f0: (fs / Math.PI) * Math.atan(k),
      q,
      vh: (b0 - b1 + b2) / (1 - a1 + a2),
      vb: ((b0 - b2) * a0 * q) / (2 * k),
    };
  })();
  const highpass = (() => {
    const [, a1, a2] = K48.highpass.a;
    const k = Math.sqrt((1 + a1 + a2) / (1 - a1 + a2));
    return {
      f0: (fs / Math.PI) * Math.atan(k),
      q: (k * (1 - a1 + a2)) / (2 * (1 - a2)),
      gain: 4 / (1 - a1 + a2),
    };
  })();
  return { shelf, highpass };
}

/** K-weighting biquads at `fs`. */
export function kWeighting(fs: number): [Biquad, Biquad] {
  const p = kPrototype();
  const s = p.shelf;
  let k = Math.tan((Math.PI * s.f0) / fs);
  let a0 = 1 + k / s.q + k * k;
  const shelf: Biquad = {
    b: [(s.vh + (s.vb * k) / s.q + k * k) / a0, (2 * (k * k - s.vh)) / a0, (s.vh - (s.vb * k) / s.q + k * k) / a0],
    a: [1, (2 * (k * k - 1)) / a0, (1 - k / s.q + k * k) / a0],
  };
  const h = p.highpass;
  k = Math.tan((Math.PI * h.f0) / fs);
  a0 = 1 + k / h.q + k * k;
  const g = h.gain / a0;
  const highpass: Biquad = {
    b: [g, -2 * g, g],
    a: [1, (2 * (k * k - 1)) / a0, (1 - k / h.q + k * k) / a0],
  };
  return [shelf, highpass];
}

/** |H(e^{j2πf/fs})| of a biquad cascade. */
export function cascadeMagnitude(filters: Biquad[], fs: number, f: number): number {
  const w = (2 * Math.PI * f) / fs;
  let m = 1;
  for (const { b, a } of filters) {
    const num = [b[0] + b[1] * Math.cos(w) + b[2] * Math.cos(2 * w), -(b[1] * Math.sin(w) + b[2] * Math.sin(2 * w))];
    const den = [a[0] + a[1] * Math.cos(w) + a[2] * Math.cos(2 * w), -(a[1] * Math.sin(w) + a[2] * Math.sin(2 * w))];
    m *= Math.hypot(num[0], num[1]) / Math.hypot(den[0], den[1]);
  }
  return m;
}

/** Direct-form-I biquad cascade with its state, one channel. */
export class BiquadCascade {
  private readonly s: Float64Array;

  constructor(private readonly filters: Biquad[]) {
    this.s = new Float64Array(4 * filters.length);
  }

  reset(): void {
    this.s.fill(0);
  }

  step(x: number): number {
    let v = x;
    for (let i = 0; i < this.filters.length; i++) {
      const { b, a } = this.filters[i];
      const o = 4 * i;
      const y = b[0] * v + b[1] * this.s[o] + b[2] * this.s[o + 1] - a[1] * this.s[o + 2] - a[2] * this.s[o + 3];
      this.s[o + 1] = this.s[o];
      this.s[o] = v;
      this.s[o + 3] = this.s[o + 2];
      this.s[o + 2] = y;
      v = y;
    }
    return v;
  }

  /** Filters a whole signal (a fresh copy). */
  run(x: ArrayLike<number>): Float64Array {
    const out = new Float64Array(x.length);
    for (let i = 0; i < x.length; i++) out[i] = this.step(x[i]);
    return out;
  }
}

export const ABSOLUTE_GATE_LKFS = -70;
export const RELATIVE_GATE_LU = -10;
export const BLOCK_S = 0.4;
export const STEP_S = 0.1;

const toLkfs = (z: number) => (z > 0 ? -0.691 + 10 * Math.log10(z) : -Infinity);

/** Distinct channel arrays with the number of times each is given. */
function unique(channels: ArrayLike<number>[]): [ArrayLike<number>, number][] {
  const m = new Map<ArrayLike<number>, number>();
  for (const x of channels) m.set(x, (m.get(x) ?? 0) + 1);
  return [...m.entries()];
}

export interface Loudness {
  /** Integrated loudness, LUFS (−∞ when every block is gated out). */
  integrated: number;
  /** Blocks measured and kept by the gates. */
  blocks: number;
  kept: number;
  relativeGate: number;
  /** Largest momentary (400 ms) loudness, LUFS. */
  maxMomentary: number;
}

/**
 * Integrated loudness of a stereo (or mono) signal at `fs`. With `periodic`
 * the K-filters start from their state after one pass over the signal, as
 * for a programme that loops (its steady state).
 */
export function integratedLoudness(channels: ArrayLike<number>[], fs: number, periodic = false): Loudness {
  const n = channels[0].length;
  const step = Math.round(STEP_S * fs);
  const block = 4 * step;
  const count = n >= block ? Math.floor((n - block) / step) + 1 : 0;
  // Mean square per 100 ms step per channel, then per block.
  const steps = Math.floor(n / step);
  const z = new Float64Array(count);
  // A channel array given twice (a mono programme in both ears) is filtered once and counted twice.
  for (const [x, times] of unique(channels)) {
    const kw = new BiquadCascade(kWeighting(fs));
    if (periodic) for (let i = 0; i < n; i++) kw.step(x[i]);
    const sq = new Float64Array(steps);
    for (let s = 0; s < steps; s++) {
      let acc = 0;
      for (let i = s * step; i < (s + 1) * step; i++) {
        const y = kw.step(x[i]);
        acc += y * y;
      }
      sq[s] = acc;
    }
    const per = block / step;
    for (let j = 0; j < count; j++) {
      let acc = 0;
      for (let s = j; s < j + per; s++) acc += sq[s];
      z[j] += (times * acc) / block;
    }
  }
  let maxMomentary = -Infinity;
  const above: number[] = [];
  for (let j = 0; j < count; j++) {
    const l = toLkfs(z[j]);
    maxMomentary = Math.max(maxMomentary, l);
    if (l > ABSOLUTE_GATE_LKFS) above.push(z[j]);
  }
  if (!above.length) return { integrated: -Infinity, blocks: count, kept: 0, relativeGate: -Infinity, maxMomentary };
  const relativeGate = toLkfs(above.reduce((a, b) => a + b, 0) / above.length) + RELATIVE_GATE_LU;
  const kept = above.filter((v) => toLkfs(v) > relativeGate);
  const integrated = kept.length ? toLkfs(kept.reduce((a, b) => a + b, 0) / kept.length) : -Infinity;
  return { integrated, blocks: count, kept: kept.length, relativeGate, maxMomentary };
}

/** RMS level over all channels, dB re 1 (a full-scale sine reads −3.01 dB). */
export function rmsDb(channels: ArrayLike<number>[]): number {
  let acc = 0;
  let n = 0;
  for (const [x, times] of unique(channels)) {
    let a = 0;
    for (let i = 0; i < x.length; i++) a += x[i] * x[i];
    acc += times * a;
    n += times * x.length;
  }
  return n && acc > 0 ? 10 * Math.log10(acc / n) : -Infinity;
}

/** True peak (4× oversampled) over all channels, dBTP. */
export function truePeakDb(channels: ArrayLike<number>[]): number {
  let p = 0;
  for (const [x] of unique(channels)) p = Math.max(p, truePeak(x));
  return p > 0 ? 20 * Math.log10(p) : -Infinity;
}

/**
 * Momentary (400 ms) and short-term (3 s) loudness of a stream (EBU Tech
 * 3341 "EBU mode" windows), from 100 ms sub-blocks of K-weighted energy.
 * For meters: push() takes each new stretch of samples as it arrives.
 */
export class StreamingLoudness {
  private readonly kw: BiquadCascade[];
  private readonly step: number;
  private readonly sub = new Float64Array(30);
  private subPos = 0;
  private subCount = 0;
  private acc = 0;
  private inStep = 0;

  constructor(
    readonly fs: number,
    channels = 2,
  ) {
    this.kw = Array.from({ length: channels }, () => new BiquadCascade(kWeighting(fs)));
    this.step = Math.round(STEP_S * fs);
  }

  push(channels: ArrayLike<number>[], n: number): void {
    for (let i = 0; i < n; i++) {
      for (let c = 0; c < this.kw.length; c++) {
        const y = this.kw[c].step(channels[Math.min(c, channels.length - 1)][i]);
        this.acc += y * y;
      }
      if (++this.inStep === this.step) {
        this.sub[this.subPos] = this.acc / this.step;
        this.subPos = (this.subPos + 1) % this.sub.length;
        this.subCount = Math.min(this.subCount + 1, this.sub.length);
        this.acc = 0;
        this.inStep = 0;
      }
    }
  }

  private window(k: number): number {
    if (this.subCount < k) return -Infinity;
    let s = 0;
    for (let i = 1; i <= k; i++) s += this.sub[(this.subPos - i + this.sub.length) % this.sub.length];
    return toLkfs(s / k);
  }

  /** Loudness of the last 400 ms, LUFS. */
  momentary(): number {
    return this.window(4);
  }

  /** Loudness of the last 3 s, LUFS. */
  shortTerm(): number {
    return this.window(30);
  }

  reset(): void {
    for (const k of this.kw) k.reset();
    this.sub.fill(0);
    this.subPos = 0;
    this.subCount = 0;
    this.acc = 0;
    this.inStep = 0;
  }
}
