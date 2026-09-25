// Programme material (spec Section 16): seeded pink noise by Kellet's
// filter and by the Voss–McCartney method, white noise and a sine sweep,
// each a loop whose end joins its start without a step.
//
// * PRNG: xoshiro128** (Blackman and Vigna, https://prng.di.unimi.it/,
//   public domain), its 128-bit state seeded from the integer seed by two
//   outputs of SplitMix64, as its authors recommend. Uniform doubles take
//   53 bits from two outputs; normal deviates come from Box–Muller.
// * Pink (Kellet): white Gaussian noise through Kellet's "refined" filter,
//   H(z) = Σ g_i/(1 − p_i·z⁻¹) + d + e·z⁻¹, with coefficients for the rate
//   (pink-coefficients.ts: Kellet's own at 44.1 kHz, refitted at 48, 88.2
//   and 96 kHz, all within 0.04 dB of −3.01 dB/octave from 20 Hz to
//   20 kHz). The loop is made seamless by running the filter over the
//   white noise once to reach its periodic steady state (the slowest pole
//   decays in 20 ms), then again for the output. At a rate without
//   coefficients (the 48 kHz set with its poles moved to keep their
//   analogue corners errs by 1.2 dB at 32 kHz), pink noise is white noise
//   shaped by 1/√f in the frequency domain instead, which is exact and
//   periodic, and the programme notes say so.
// * Pink (Voss–McCartney): R rows of Gaussian values, row k held for
//   2^(k+1) samples and renewed when k equals the number of trailing zeros
//   of the sample index (McCartney's tree, one row per sample), plus a new
//   white value every sample (Music-DSP list, 1999, via the page above).
//   The rows are laid out circularly over a loop whose length is a
//   multiple of 2^R, so the loop is exact. Its spectrum is a sum of
//   sin(x)/x shapes, Σ_k (1/L_k)·[sin(πfL_k/fs)/sin(πf/fs)]² + 1 with
//   L_k = 2^(k+1), which ripples by about a decibel around −3 dB/octave
//   ([`vossSpectrum`], tests/audio-dsp.spec.ts).
// * Sweep: exponential sine sweep (Farina, AES 108th Convention, 2000,
//   preprint 5093) from 20 Hz to 20 kHz (or 0.45·fs at rates below
//   44.4 kHz, so it never folds over Nyquist) over the loop, with 10 ms
//   raised-cosine fades at both ends.
//
// Noise loops are 2^⌈log2(10·fs)⌉ samples (10.9 s at 48 kHz) and
// normalised to −23 dB RMS re full scale; the sweep peaks at −6 dBFS.

import { FFT } from './fft';
import { PINK_KELLET, type PinkCoefficients } from './pink-coefficients';

export type ProgrammeKind = 'pink_kellet' | 'pink_voss' | 'white' | 'sweep';

export const PROGRAMME_RMS_DB = -23;
export const SWEEP_PEAK_DB = -6;
export const SWEEP_RANGE_HZ: [number, number] = [20, 20000];

const M64 = (1n << 64n) - 1n;

/** SplitMix64 outputs from a seed (BigInt; used for seeding only). */
function splitMix64(seed: number, count: number): bigint[] {
  let x = BigInt.asUintN(64, BigInt(Math.trunc(seed)));
  const out: bigint[] = [];
  for (let i = 0; i < count; i++) {
    x = (x + 0x9e3779b97f4a7c15n) & M64;
    let z = x;
    z = ((z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n) & M64;
    z = ((z ^ (z >> 27n)) * 0x94d049bb133111ebn) & M64;
    out.push(z ^ (z >> 31n));
  }
  return out;
}

const rotl = (x: number, k: number) => ((x << k) | (x >>> (32 - k))) >>> 0;

/** xoshiro128** with 32-bit outputs. */
export class Xoshiro128 {
  private readonly s = new Uint32Array(4);
  private spare: number | null = null;

  constructor(seed: number | Uint32Array) {
    if (typeof seed === 'number') {
      const [a, b] = splitMix64(seed, 2);
      this.s[0] = Number(a & 0xffffffffn);
      this.s[1] = Number(a >> 32n);
      this.s[2] = Number(b & 0xffffffffn);
      this.s[3] = Number(b >> 32n);
    } else {
      this.s.set(seed);
    }
  }

  nextU32(): number {
    const s = this.s;
    const result = Math.imul(rotl(Math.imul(s[1], 5) >>> 0, 7), 9) >>> 0;
    const t = (s[1] << 9) >>> 0;
    s[2] ^= s[0];
    s[3] ^= s[1];
    s[1] ^= s[2];
    s[0] ^= s[3];
    s[2] ^= t;
    s[3] = rotl(s[3], 11);
    return result;
  }

  /** Uniform in [0, 1) with 53 bits. */
  uniform(): number {
    const a = this.nextU32() >>> 5;
    const b = this.nextU32() >>> 6;
    return (a * 67108864 + b) / 9007199254740992;
  }

  /** Standard normal deviate (Box–Muller, both values used). */
  normal(): number {
    if (this.spare !== null) {
      const v = this.spare;
      this.spare = null;
      return v;
    }
    const u1 = 1 - this.uniform();
    const u2 = this.uniform();
    const r = Math.sqrt(-2 * Math.log(u1));
    this.spare = r * Math.sin(2 * Math.PI * u2);
    return r * Math.cos(2 * Math.PI * u2);
  }
}

/** Loop length for noise at `fs`: the power of two at or above 10 s. */
export function loopLength(fs: number): number {
  let n = 1;
  while (n < 10 * fs) n *= 2;
  return n;
}

/** Kellet coefficients tabulated for `fs`, or null. */
export function pinkFilter(fs: number): PinkCoefficients | null {
  return PINK_KELLET[Math.round(fs)] ?? null;
}

/** |H(f)| of Kellet's structure. */
export function kelletMagnitude(c: PinkCoefficients, fs: number, f: number): number {
  const w = (2 * Math.PI * f) / fs;
  const zr = Math.cos(w);
  const zi = -Math.sin(w);
  let re = c.direct + c.delayed * zr;
  let im = c.delayed * zi;
  for (let i = 0; i < c.poles.length; i++) {
    // g/(1 − p·z⁻¹)
    const dr = 1 - c.poles[i] * zr;
    const di = -c.poles[i] * zi;
    const d2 = dr * dr + di * di;
    re += (c.gains[i] * dr) / d2;
    im += (-c.gains[i] * di) / d2;
  }
  return Math.hypot(re, im);
}

/** Largest deviation of |H| from C/√f, the best C, over [lo, hi] (dB). */
export function pinkDeviationDb(mag: (f: number) => number, lo: number, hi: number, ppo = 48): number {
  const n = Math.ceil(Math.log2(hi / lo) * ppo);
  const e: number[] = [];
  for (let i = 0; i <= n; i++) {
    const f = lo * (hi / lo) ** (i / n);
    e.push(20 * Math.log10(mag(f)) + 10 * Math.log10(f));
  }
  const mean = e.reduce((x, y) => x + y, 0) / e.length;
  return e.reduce((m, v) => Math.max(m, Math.abs(v - mean)), 0);
}

function normalise(x: Float32Array | Float64Array, rmsDb: number): void {
  let acc = 0;
  for (let i = 0; i < x.length; i++) acc += x[i] * x[i];
  const k = 10 ** (rmsDb / 20) / Math.sqrt(acc / x.length);
  for (let i = 0; i < x.length; i++) x[i] *= k;
}

/** White Gaussian noise, `n` samples. */
export function whiteNoise(n: number, seed: number): Float64Array {
  const r = new Xoshiro128(seed);
  const x = new Float64Array(n);
  for (let i = 0; i < n; i++) x[i] = r.normal();
  return x;
}

/** Pink noise by Kellet's filter, periodic with period `n` (rates with coefficients only). */
export function pinkKellet(n: number, seed: number, fs: number): Float64Array {
  const w = whiteNoise(n, seed);
  const c = pinkFilter(fs);
  if (!c) throw new Error(`no Kellet coefficients for ${fs} Hz`);
  const b = new Float64Array(c.poles.length);
  let prev = 0;
  const out = new Float64Array(n);
  for (let pass = 0; pass < 2; pass++) {
    for (let i = 0; i < n; i++) {
      const x = w[i];
      let y = c.direct * x + c.delayed * prev;
      for (let k = 0; k < b.length; k++) {
        b[k] = c.poles[k] * b[k] + c.gains[k] * x;
        y += b[k];
      }
      prev = x;
      if (pass === 1) out[i] = y;
    }
  }
  return out;
}

/** White noise shaped by 1/√f in the frequency domain (DC removed), periodic with period `n` (a power of two). */
export function pinkShaped(n: number, seed: number): Float64Array {
  const re = whiteNoise(n, seed);
  const im = new Float64Array(n);
  const fft = new FFT(n);
  fft.forward(re, im);
  re[0] = 0;
  im[0] = 0;
  for (let k = 1; k <= n / 2; k++) {
    const g = 1 / Math.sqrt(k);
    re[k] *= g;
    im[k] *= g;
    if (k < n / 2) {
      re[n - k] *= g;
      im[n - k] *= g;
    }
  }
  fft.inverse(re, im);
  return re;
}

/** Rows of the Voss–McCartney generator at `fs` (lowest row renewed every 2^R samples, 0.73 Hz at 48 kHz). */
export function vossRows(fs: number): number {
  return Math.max(1, Math.round(Math.log2((fs / 48000) * 65536)));
}

/** Voss–McCartney pink noise, periodic with period `n` (a multiple of 2^rows). */
export function pinkVoss(n: number, seed: number, rows: number): Float64Array {
  if (n % 2 ** rows !== 0) throw new Error(`loop of ${n} samples is not a multiple of 2^${rows}`);
  const r = new Xoshiro128(seed);
  const out = new Float64Array(n);
  for (let i = 0; i < n; i++) out[i] = r.normal();
  for (let k = 0; k < rows; k++) {
    const len = 2 ** (k + 1);
    const segs = n / len;
    const values = new Float64Array(segs);
    for (let s = 0; s < segs; s++) values[s] = r.normal();
    const offset = 2 ** k;
    // Row k is renewed at samples ≡ 2^k (mod 2^(k+1)); the segment
    // holding sample i starts at the last renewal at or before i.
    for (let i = 0; i < n; i++) out[i] += values[Math.floor((((i - offset) % n) + n) % n / len)];
  }
  const k = 1 / Math.sqrt(rows + 1);
  for (let i = 0; i < n; i++) out[i] *= k;
  return out;
}

/** Power spectrum of the Voss–McCartney process (per unit variance of each source), at f. */
export function vossSpectrum(f: number, fs: number, rows: number): number {
  const x = (Math.PI * f) / fs;
  const s = Math.sin(x);
  let p = 1;
  for (let k = 0; k < rows; k++) {
    const L = 2 ** (k + 1);
    const r = s === 0 ? L : Math.sin(x * L) / s;
    p += (r * r) / L;
  }
  return p;
}

/** Top of the sweep at `fs`: 20 kHz, or 0.45·fs when that is lower. */
export function sweepTop(fs: number): number {
  return Math.min(SWEEP_RANGE_HZ[1], 0.45 * fs);
}

/** Exponential sine sweep over the whole loop, peak 1 before scaling. */
export function sineSweep(n: number, fs: number, [f1, f2] = SWEEP_RANGE_HZ): Float64Array {
  const T = n / fs;
  const r = Math.log(f2 / f1);
  const out = new Float64Array(n);
  const fade = Math.round(0.01 * fs);
  for (let i = 0; i < n; i++) {
    const t = i / fs;
    let v = Math.sin(((2 * Math.PI * f1 * T) / r) * (Math.exp((t * r) / T) - 1));
    if (i < fade) v *= 0.5 * (1 - Math.cos((Math.PI * i) / fade));
    else if (i >= n - fade) v *= 0.5 * (1 - Math.cos((Math.PI * (n - 1 - i)) / fade));
    out[i] = v;
  }
  return out;
}

export interface Programme {
  kind: ProgrammeKind | 'file';
  label: string;
  fs: number;
  /** Channel data (two, the same array twice for mono programmes). */
  channels: Float32Array[];
  seed: number | null;
  /** SHA-256 of a user file, or null. */
  sha256: string | null;
  /** Level matching the programme calls for: loudness, or RMS for pure tones. */
  matchBy: 'bs1770' | 'rms';
  notes: string[];
}

/** A generated programme at `fs`. */
export function generate(kind: ProgrammeKind, seed: number, fs: number): Programme {
  const n = loopLength(fs);
  let x: Float64Array;
  const notes: string[] = [];
  let label: string;
  switch (kind) {
    case 'pink_kellet': {
      const c = pinkFilter(fs);
      if (c) {
        x = pinkKellet(n, seed, fs);
        label = 'Pink noise (Kellet filter)';
        notes.push(`Kellet filter coefficients: ${c.kind}; largest deviation from −3.01 dB/octave over 20 Hz–20 kHz: ${c.maxErrorAudioDb.toFixed(3)} dB.`);
      } else {
        x = pinkShaped(n, seed);
        label = 'Pink noise (spectral shaping)';
        notes.push(
          `Kellet coefficients exist for ${Object.keys(PINK_KELLET).map((r) => `${Number(r) / 1000} kHz`).join(', ')}; at ${fs / 1000} kHz the pink noise is white noise shaped by 1/√f in the frequency domain (exactly −3.01 dB/octave).`,
        );
      }
      break;
    }
    case 'pink_voss': {
      const rows = vossRows(fs);
      x = pinkVoss(n, seed, rows);
      label = 'Pink noise (Voss–McCartney)';
      const top = Math.min(20000, 0.9 * (fs / 2));
      const dev = pinkDeviationDb((f) => Math.sqrt(vossSpectrum(f, fs, rows)), 20, top);
      notes.push(`Voss–McCartney with ${rows} rows and a white term; its spectrum departs from −3.01 dB/octave by up to ${dev.toFixed(2)} dB over 20 Hz–${Math.round(top / 1000)} kHz.`);
      break;
    }
    case 'white':
      x = whiteNoise(n, seed);
      label = 'White noise';
      break;
    case 'sweep': {
      const top = sweepTop(fs);
      x = sineSweep(n, fs, [SWEEP_RANGE_HZ[0], top]);
      label = `Sine sweep ${SWEEP_RANGE_HZ[0]} Hz–${+(top / 1000).toFixed(2)} kHz, ${(n / fs).toFixed(1)} s`;
      break;
    }
  }
  if (kind === 'sweep') {
    const k = 10 ** (SWEEP_PEAK_DB / 20);
    for (let i = 0; i < n; i++) x[i] *= k;
  } else {
    normalise(x, PROGRAMME_RMS_DB);
  }
  const ch = Float32Array.from(x);
  return {
    kind,
    label,
    fs,
    channels: [ch, ch],
    seed: kind === 'sweep' ? null : seed,
    sha256: null,
    matchBy: kind === 'sweep' ? 'rms' : 'bs1770',
    notes,
  };
}
