// Hard true-peak limiter at the output (spec Section 16, "Level matching
// and safety"): stereo-linked, look-ahead, so the gain is already down when
// a peak arrives. It never lets a 4× interpolated peak exceed the ceiling
// minus the inter-point margin (oversample.ts), which keeps the true peak
// of the output, measured by 16× reconstruction in the tests, at or below
// the ceiling.
//
// For each sample s the detector gives p[s], the largest 4× |value| over
// (s − 1, s + 1] across both channels, and the required gain is
// g_req[s] = min(1, T/p[s]) with T = 10^((ceiling − margin)/20). With a
// look-ahead of L samples:
//
//   r[i] = min(g_req[i − L .. i])            (running minimum)
//   r'[i] = min(r[i], r'[i−1] + (1 − r'[i−1])·(1 − e^{−1/(τ·fs)}))   (release)
//   g[i] = mean(r'[i − L .. i])              (moving average: the attack ramp)
//   y[i − L] = g[i]·x[i − L]
//
// For every i in [m − L, m], the window of r[i] contains m − L, so
// r'[i] ≤ r[i] ≤ g_req[m − L] and their mean g[m] ≤ g_req[m − L]: the
// sample leaving the delay line is never above the ceiling. The gain
// falls linearly over at most L samples (2 ms) and recovers with the
// release time constant (50 ms). Everything is preallocated.

import { INTER_POINT_MARGIN_DB, PeakDetector } from './oversample';

export interface LimiterOptions {
  /** Output ceiling, dBTP. */
  ceilingDb?: number;
  /** Look-ahead (attack) time, s. */
  lookahead?: number;
  /** Release time constant, s. */
  release?: number;
  /** Extra margin for peaks between the 4× points, dB. */
  marginDb?: number;
}

export class TruePeakLimiter {
  readonly channels = 2;
  readonly lookahead: number;
  /** Total delay of the audio, samples. */
  readonly latency: number;
  private threshold = 1;
  private ceilingDb: number;
  private readonly marginDb: number;
  private readonly releaseCoef: number;
  private readonly detectors = [new PeakDetector(), new PeakDetector()];
  private readonly detDelay: number;
  /** Interval peaks q[s] (max over channels), to form p[s] = max(q[s], q[s+1]). */
  private qPrev = 0;
  // Delay line of the audio (per channel) and of the gain pipeline.
  private readonly delayLen: number;
  private readonly dl: Float64Array[];
  private dlPos = 0;
  // Running minimum over L + 1 values: a monotonic deque in fixed rings.
  private readonly dqVal: Float64Array;
  private readonly dqIdx: Float64Array;
  private dqHead = 0;
  private dqLen = 0;
  private count = 0;
  private rel = 1;
  // Moving average over L + 1 values.
  private readonly box: Float64Array;
  private boxPos = 0;
  private boxSum: number;
  private boxAge = 0;
  enabled = true;
  /** Smallest gain applied since the last read (for metering). */
  minGain = 1;

  constructor(
    readonly sampleRate: number,
    opts: LimiterOptions = {},
  ) {
    this.ceilingDb = opts.ceilingDb ?? -1;
    this.marginDb = opts.marginDb ?? INTER_POINT_MARGIN_DB;
    this.lookahead = Math.max(1, Math.round((opts.lookahead ?? 0.002) * sampleRate));
    this.releaseCoef = 1 - Math.exp(-1 / ((opts.release ?? 0.05) * sampleRate));
    this.detDelay = this.detectors[0].delay + 1;
    this.delayLen = this.detDelay + this.lookahead + 1;
    this.latency = this.detDelay + this.lookahead;
    this.dl = [new Float64Array(this.delayLen), new Float64Array(this.delayLen)];
    const w = this.lookahead + 1;
    this.dqVal = new Float64Array(w + 1);
    this.dqIdx = new Float64Array(w + 1);
    this.box = new Float64Array(w).fill(1);
    this.boxSum = w;
    this.setCeiling(this.ceilingDb);
  }

  setCeiling(db: number): void {
    this.ceilingDb = db;
    this.threshold = 10 ** ((db - this.marginDb) / 20);
  }

  get ceiling(): number {
    return this.ceilingDb;
  }

  reset(): void {
    for (const d of this.detectors) d.reset();
    for (const d of this.dl) d.fill(0);
    this.qPrev = 0;
    this.dqLen = 0;
    this.count = 0;
    this.rel = 1;
    this.box.fill(1);
    this.boxSum = this.box.length;
    this.minGain = 1;
  }

  /** Running minimum: push g for index i, return min over the last L + 1. */
  private runningMin(g: number, i: number): number {
    const cap = this.dqVal.length;
    const w = this.lookahead + 1;
    while (this.dqLen > 0) {
      const back = (this.dqHead + this.dqLen - 1) % cap;
      if (this.dqVal[back] >= g) this.dqLen--;
      else break;
    }
    const at = (this.dqHead + this.dqLen) % cap;
    this.dqVal[at] = g;
    this.dqIdx[at] = i;
    this.dqLen++;
    while (this.dqIdx[this.dqHead] <= i - w) {
      this.dqHead = (this.dqHead + 1) % cap;
      this.dqLen--;
    }
    return this.dqVal[this.dqHead];
  }

  /**
   * Processes `n` samples of both channels (in place is allowed). `gainOut`,
   * if given, receives the gain applied to each output sample.
   */
  process(inL: ArrayLike<number>, inR: ArrayLike<number>, outL: Float32Array, outR: Float32Array, n: number, gainOut?: Float32Array): void {
    const L = this.lookahead;
    const len = this.delayLen;
    for (let t = 0; t < n; t++) {
      const xl = inL[t];
      const xr = inR[t];
      this.dl[0][this.dlPos] = xl;
      this.dl[1][this.dlPos] = xr;
      // Interval peak ending at s = t − (detector delay).
      const q = Math.max(this.detectors[0].push(xl), this.detectors[1].push(xr));
      // p for the sample before: max of its interval and the next.
      const p = Math.max(this.qPrev, q);
      this.qPrev = q;
      const i = this.count++;
      const req = p > this.threshold ? this.threshold / p : 1;
      const r = this.runningMin(req, i);
      const released = this.rel + (1 - this.rel) * this.releaseCoef;
      this.rel = r < released ? r : released;
      this.boxSum += this.rel - this.box[this.boxPos];
      this.box[this.boxPos] = this.rel;
      this.boxPos = (this.boxPos + 1) % this.box.length;
      if (++this.boxAge >= 4096) {
        // Recompute the running sum now and then (no drift).
        let s = 0;
        for (let k = 0; k < this.box.length; k++) s += this.box[k];
        this.boxSum = s;
        this.boxAge = 0;
      }
      const g = this.enabled ? Math.min(1, this.boxSum / this.box.length) : 1;
      // The audio sample the gain belongs to: detector delay + 1 + L back.
      const k = (this.dlPos - (this.detDelay + L) + len * 2) % len;
      outL[t] = g * this.dl[0][k];
      outR[t] = g * this.dl[1][k];
      if (gainOut) gainOut[t] = g;
      if (g < this.minGain) this.minGain = g;
      this.dlPos = (this.dlPos + 1) % len;
    }
  }
}
