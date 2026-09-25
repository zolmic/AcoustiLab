// Level matching over the actual programme after convolution (spec
// Section 16, "Level matching and safety").
//
// The programme loops, so what the listener hears in steady state is its
// circular convolution with the filter. That is computed exactly here
// (overlap-save over the periodic extension, double precision), then
// measured as it will play: integrated loudness per BS.1770-5 with the
// K-filters in their periodic steady state, RMS, and the 4× true peak.
// Integrated loudness scales exactly with gain (both gates move with the
// signal), so one measurement per filter gives the gain that puts it at
// the target: A and B then match to rounding, and the Playwright tests
// re-measure the rendered output to confirm 0.1 LU.
//
// Methods: `bs1770` (default), `rms` (pure tones and sweeps, which fall
// outside the loudness algorithm's scope), `midband` (no programme
// measurement: the filters are already 0 dB mean over 500 Hz–2 kHz, the
// engine's anchor, so A and B get the same gain).

import { fftOf, nextPow2 } from './fft';
import { integratedLoudness, rmsDb, truePeakDb } from './loudness';

export type MatchMethod = 'bs1770' | 'rms' | 'midband';

/** Loudness the programme is matched to (EBU R 128's reference level). */
export const TARGET_LUFS = -23;
/** RMS the programme is matched to under `rms` (dB re 1; a stereo sine at this RMS reads about −23 LUFS). */
export const TARGET_RMS_DB = -26;

export interface Measurement {
  integratedLufs: number;
  rmsDb: number;
  truePeakDbtp: number;
  /** Largest 400 ms loudness, LUFS. */
  maxMomentaryLufs: number;
  blocks: number;
  kept: number;
}

/** y[n] = Σ_m h[m]·x[(n − m) mod N], exactly (FFT overlap-save). */
export function circularConvolve(x: ArrayLike<number>, h: ArrayLike<number>): Float64Array {
  const n = x.length;
  const l = h.length;
  const f = nextPow2(Math.max(2 * l, 1024));
  const step = f - l + 1;
  const fft = fftOf(f);
  const hr = new Float64Array(f);
  const hi = new Float64Array(f);
  for (let i = 0; i < l; i++) hr[i] = h[i];
  fft.forward(hr, hi);
  const re = new Float64Array(f);
  const im = new Float64Array(f);
  const y = new Float64Array(n);
  for (let n0 = 0; n0 < n; n0 += step) {
    for (let i = 0; i < f; i++) {
      const k = (((n0 - (l - 1) + i) % n) + n) % n;
      re[i] = x[k];
      im[i] = 0;
    }
    fft.forward(re, im);
    for (let k = 0; k < f; k++) {
      const a = re[k];
      const b = im[k];
      re[k] = a * hr[k] - b * hi[k];
      im[k] = a * hi[k] + b * hr[k];
    }
    fft.inverse(re, im);
    for (let j = 0; j < step && n0 + j < n; j++) y[n0 + j] = re[l - 1 + j];
  }
  return y;
}

/** Measures a looping stereo signal. */
export function measure(channels: ArrayLike<number>[], fs: number): Measurement {
  const l = integratedLoudness(channels, fs, true);
  return {
    integratedLufs: l.integrated,
    rmsDb: rmsDb(channels),
    truePeakDbtp: truePeakDb(channels),
    maxMomentaryLufs: l.maxMomentary,
    blocks: l.blocks,
    kept: l.kept,
  };
}

/** The programme through a filter (per channel taps; one array serves both). */
export function convolveProgramme(programme: ArrayLike<number>[], taps: ArrayLike<number>[]): Float64Array[] {
  const out: Float64Array[] = [];
  const cache = new Map<ArrayLike<number>, Map<ArrayLike<number>, Float64Array>>();
  programme.forEach((x, c) => {
    const h = taps[Math.min(c, taps.length - 1)];
    let byX = cache.get(x);
    if (!byX) cache.set(x, (byX = new Map()));
    let y = byX.get(h);
    if (!y) byX.set(h, (y = circularConvolve(x, h)));
    out.push(y);
  });
  return out;
}

/** Gain (dB) that brings a measurement to the method's target. */
export function matchGainDb(m: Measurement, method: MatchMethod, programme: Measurement): number {
  switch (method) {
    case 'bs1770':
      return TARGET_LUFS - m.integratedLufs;
    case 'rms':
      return TARGET_RMS_DB - m.rmsDb;
    case 'midband':
      return TARGET_LUFS - programme.integratedLufs;
  }
}

/** A delta (identity filter) delayed by `latency` samples. */
export function delayTaps(latency: number): Float32Array {
  const d = new Float32Array(Math.max(1, latency + 1));
  d[latency] = 1;
  return d;
}
