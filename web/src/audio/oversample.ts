// 4× interpolation for true-peak measurement and limiting (ITU-R BS.1770,
// Annex 2: "true-peak" is the peak of the signal oversampled at least 4×).
//
// BS.1770 gives an example 48-tap filter; this module designs its own, so
// no table is copied: the values between samples n and n + 1 at fractions
// δ = 1/4, 1/2, 3/4 are
//
//   x(n + δ) ≈ Σ_{i=−K/2+1}^{K/2} x[n + i]·c_δ[i],
//   c_δ[i] = sinc(δ − i)·w((δ − i)/(K/2)),   w(t) = I0(β·√(1 − t²))/I0(β),
//
// a Kaiser-windowed sinc with K = 32 taps per phase and β = 8, each phase
// normalised to unit gain at DC. It reconstructs sinusoids up to 20 kHz at
// 48 kHz within 0.002 dB (with 24 taps the error reaches 0.13 dB at
// 20 kHz: the window's main lobe reaches into the band;
// tests/audio-dsp.spec.ts).
//
// Between 4× points a band-limited signal can still exceed the largest of
// them: by Bernstein's inequality |x''| ≤ (2πB)²·max|x|, so a peak lies at
// most 1/(8·fs) from a 4× point and is under-read by at most a factor
// 1 − ½(πB/(4·fs))², 0.70 dB for B = fs/2 (0.48 dB for B = 20 kHz at 48 kHz).
// The limiter keeps that margin; the meter reports the 4× peak, as
// BS.1770 does.

/** Taps per phase. */
export const TAPS_PER_PHASE = 32;
/** Kaiser β. */
export const KAISER_BETA = 8;
/** Largest under-read of a 4× peak for a signal band-limited to fs/2, dB. */
export const INTER_POINT_MARGIN_DB = 0.7;

/** Modified Bessel function I0 by its power series (converges for all x). */
export function besselI0(x: number): number {
  let sum = 1;
  let term = 1;
  const q = (x * x) / 4;
  for (let k = 1; k < 200; k++) {
    term *= q / (k * k);
    sum += term;
    if (term < 1e-17 * sum) break;
  }
  return sum;
}

function sinc(x: number): number {
  if (x === 0) return 1;
  const p = Math.PI * x;
  return Math.sin(p) / p;
}

/** Coefficients of the three fractional phases (δ = 1/4, 1/2, 3/4), each K taps for i = −K/2+1..K/2. */
export function phaseCoefficients(k = TAPS_PER_PHASE, beta = KAISER_BETA): Float64Array[] {
  const half = k / 2;
  const i0b = besselI0(beta);
  return [0.25, 0.5, 0.75].map((d) => {
    const c = new Float64Array(k);
    let sum = 0;
    for (let j = 0; j < k; j++) {
      const i = j - half + 1;
      const t = (d - i) / half;
      const w = Math.abs(t) <= 1 ? besselI0(beta * Math.sqrt(1 - t * t)) / i0b : 0;
      c[j] = sinc(d - i) * w;
      sum += c[j];
    }
    for (let j = 0; j < k; j++) c[j] /= sum;
    return c;
  });
}

const PHASES = phaseCoefficients();

/**
 * Streaming 4× peak detector for one channel. After push(x[n]) it returns
 * q[s], the largest |value| over the interval (s − 1, s] (sample s and the
 * three interpolated points before it), for s = n − delay: the points
 * between s − 1 and s need the K samples x[s − K/2 .. s + K/2 − 1], of
 * which x[n] is the newest. The last K samples sit twice in a ring of 2K,
 * so they are always contiguous; nothing is allocated per sample.
 */
export class PeakDetector {
  readonly delay = TAPS_PER_PHASE / 2 - 1;
  private readonly ring = new Float64Array(2 * TAPS_PER_PHASE);
  private pos = 0;

  push(x: number): number {
    const k = TAPS_PER_PHASE;
    const r = this.ring;
    r[this.pos] = x;
    r[this.pos + k] = x;
    this.pos = (this.pos + 1) % k;
    // r[o + j] = x[n − K + 1 + j], j = 0..K−1.
    const o = this.pos;
    let peak = Math.abs(r[o + k / 2]);
    for (let p = 0; p < 3; p++) {
      const c = PHASES[p];
      let v = 0;
      for (let j = 0; j < k; j++) v += c[j] * r[o + j];
      const a = Math.abs(v);
      if (a > peak) peak = a;
    }
    return peak;
  }

  reset(): void {
    this.ring.fill(0);
    this.pos = 0;
  }
}

/**
 * Largest 4× interpolated |value| of a signal: its samples from `from` to
 * `to` and the points after each of them. Samples outside [0, n) count as
 * zero; a signal cut off abruptly overshoots near its ends (Gibbs), so a
 * measurement of a stretch of a longer signal should pass the whole signal
 * and the stretch's range.
 */
export function truePeak(x: ArrayLike<number>, from = 0, to = x.length): number {
  const k = TAPS_PER_PHASE;
  const half = k / 2;
  const n = x.length;
  const at = (i: number) => (i >= 0 && i < n ? x[i] : 0);
  let peak = 0;
  for (let m = Math.max(0, from); m < Math.min(n, to); m++) {
    peak = Math.max(peak, Math.abs(x[m]));
    if (m + 1 >= n) break;
    for (let p = 0; p < 3; p++) {
      const c = PHASES[p];
      let v = 0;
      for (let j = 0; j < k; j++) v += c[j] * at(m + j - half + 1);
      peak = Math.max(peak, Math.abs(v));
    }
  }
  return peak;
}
