// Uniformly partitioned frequency-domain convolution (spec Section 16,
// "Convolution engine"): uniformly partitioned overlap-save (UPOLS) with the
// block equal to the render quantum (B = 128) and an FFT of 2B = 256.
//
// The filter h is cut into P partitions of B taps, h_p[n] = h[pB + n], each
// zero-padded to 2B and transformed once: H_p. Each block t of input, with
// the block before it, is transformed, X_t = FFT([x_{t−1}, x_t]), and kept
// in a frequency-domain delay line. The output block is the last B samples
// of IFFT(Σ_p X_{t−p}·H_p): the first B are circularly aliased and
// discarded. This equals the direct convolution y[n] = Σ_m h[m]·x[n − m]
// exactly (tests: to 1e-12 in double precision), with no latency beyond the
// block itself.
//
// Filter sets ("banks") hold both channels' partition spectra and a gain.
// Several banks share the delay line, so switching banks (A/B, a new filter
// during a slider drag) crossfades two outputs that are both exact
// convolutions of the same input history: out = (1 − g)·y_old + g·y_new,
// g rising linearly over the fade (two blocks by default). A new filter is
// loaded incrementally, a few partitions per block, so no block does the
// whole transform. Every buffer is allocated in the constructor; process()
// allocates nothing.

import { FFT } from './fft';

export const BLOCK = 128;
export const FFT_SIZE = 2 * BLOCK;
export const BINS = BLOCK + 1;

interface Bank {
  re: Float64Array;
  im: Float64Array;
  partitions: number;
  gain: number;
}

export class PartitionedConvolver {
  readonly channels: number;
  readonly maxPartitions: number;
  /** Crossfade length in samples. */
  fadeLength: number;
  private readonly fft = new FFT(FFT_SIZE);
  private readonly banks: Bank[];
  /** Input history per channel: [previous block, current block]. */
  private readonly history: Float64Array[];
  /** Frequency-domain delay line: [channel][slot][bin]. */
  private readonly fdlRe: Float64Array;
  private readonly fdlIm: Float64Array;
  private head = 0;
  private filled = 0;
  private readonly wRe = new Float64Array(FFT_SIZE);
  private readonly wIm = new Float64Array(FFT_SIZE);
  private readonly accRe = new Float64Array(BINS);
  private readonly accIm = new Float64Array(BINS);
  private readonly yOld: Float64Array;
  private readonly yNew: Float64Array;
  /** Bank playing now. */
  active = 0;
  /** Crossfade: target bank and samples done (−1 when not fading). */
  private fadeTo = -1;
  private fadePos = 0;
  // Incremental load state.
  private loadBank = -1;
  private loadNext = 0;
  private loadPartitions = 0;
  private readonly loadTaps: Float32Array[];
  private loadGain = 1;

  constructor(maxTaps: number, channels = 2, banks = 3, fadeLength = 2 * BLOCK) {
    this.channels = channels;
    this.maxPartitions = Math.max(1, Math.ceil(maxTaps / BLOCK));
    this.fadeLength = fadeLength;
    const size = channels * this.maxPartitions * BINS;
    this.banks = Array.from({ length: banks }, () => ({
      re: new Float64Array(size),
      im: new Float64Array(size),
      partitions: 0,
      gain: 1,
    }));
    this.history = Array.from({ length: channels }, () => new Float64Array(FFT_SIZE));
    this.fdlRe = new Float64Array(size);
    this.fdlIm = new Float64Array(size);
    this.yOld = new Float64Array(channels * BLOCK);
    this.yNew = new Float64Array(channels * BLOCK);
    this.loadTaps = Array.from({ length: channels }, () => new Float32Array(this.maxPartitions * BLOCK));
  }

  get bankCount(): number {
    return this.banks.length;
  }

  get fading(): boolean {
    return this.fadeTo >= 0;
  }

  get loading(): boolean {
    return this.loadBank >= 0;
  }

  /** Bank being loaded, or −1. */
  get loadingBank(): number {
    return this.loadBank;
  }

  gainOf(bank: number): number {
    return this.banks[bank].gain;
  }

  /** Transforms partition p of `taps` into bank `b`, channel `ch`. */
  private transformPartition(b: Bank, ch: number, taps: ArrayLike<number>, length: number, p: number): void {
    const re = this.wRe;
    const im = this.wIm;
    for (let i = 0; i < FFT_SIZE; i++) {
      const n = p * BLOCK + i;
      re[i] = i < BLOCK && n < length ? taps[n] : 0;
      im[i] = 0;
    }
    this.fft.forward(re, im);
    const off = (ch * this.maxPartitions + p) * BINS;
    for (let k = 0; k < BINS; k++) {
      b.re[off + k] = re[k];
      b.im[off + k] = im[k];
    }
  }

  /**
   * Loads a filter into a bank at once (setup, tests; not for the audio
   * thread): `taps[ch]` per channel (one array for all channels is reused).
   */
  setFilter(bank: number, taps: ArrayLike<number>[], gain = 1): void {
    const b = this.banks[bank];
    const length = Math.max(...taps.map((t) => t.length));
    const partitions = Math.ceil(length / BLOCK);
    if (partitions > this.maxPartitions) throw new Error(`filter of ${length} taps exceeds ${this.maxPartitions * BLOCK}`);
    for (let ch = 0; ch < this.channels; ch++) {
      const t = taps[Math.min(ch, taps.length - 1)];
      for (let p = 0; p < partitions; p++) this.transformPartition(b, ch, t, t.length, p);
    }
    b.partitions = partitions;
    b.gain = gain;
  }

  /**
   * Starts loading a filter into `bank` (copies the taps; the transform is
   * done by loadStep). A load already running is abandoned. Returns false
   * when the filter is too long.
   */
  beginLoad(bank: number, taps: ArrayLike<number>[], gain: number): boolean {
    let length = 0;
    for (let i = 0; i < taps.length; i++) length = Math.max(length, taps[i].length);
    const partitions = Math.ceil(length / BLOCK);
    if (partitions > this.maxPartitions || bank === this.active || bank === this.fadeTo) return false;
    for (let ch = 0; ch < this.channels; ch++) {
      const src = taps[Math.min(ch, taps.length - 1)];
      const dst = this.loadTaps[ch];
      for (let i = 0; i < partitions * BLOCK; i++) dst[i] = i < src.length ? src[i] : 0;
    }
    this.loadBank = bank;
    this.loadNext = 0;
    this.loadPartitions = partitions;
    this.loadGain = gain;
    return true;
  }

  /** Transforms up to `max` partitions per channel of the load in progress; true when it is complete. */
  loadStep(max: number): boolean {
    if (this.loadBank < 0) return false;
    const b = this.banks[this.loadBank];
    const end = Math.min(this.loadPartitions, this.loadNext + max);
    for (let p = this.loadNext; p < end; p++) {
      for (let ch = 0; ch < this.channels; ch++) {
        this.transformPartition(b, ch, this.loadTaps[ch], this.loadPartitions * BLOCK, p);
      }
    }
    this.loadNext = end;
    if (end < this.loadPartitions) return false;
    b.partitions = this.loadPartitions;
    b.gain = this.loadGain;
    this.loadBank = -1;
    return true;
  }

  /** Switches to `bank`: at once when nothing plays yet, else by crossfade. False while a fade is running. */
  crossfadeTo(bank: number, immediate = false): boolean {
    if (bank === this.active && this.fadeTo < 0) return true;
    if (this.fadeTo >= 0 || bank === this.loadBank) return false;
    if (immediate) {
      this.active = bank;
      return true;
    }
    this.fadeTo = bank;
    this.fadePos = 0;
    return true;
  }

  /** Sets a bank's gain at once (use a crossfade to another bank for a smooth change). */
  setGain(bank: number, gain: number): void {
    this.banks[bank].gain = gain;
  }

  /** Clears the input history. */
  reset(): void {
    for (const h of this.history) h.fill(0);
    this.fdlRe.fill(0);
    this.fdlIm.fill(0);
    this.head = 0;
    this.filled = 0;
  }

  /** Σ_p X_{t−p}·H_p for one channel, into the half-spectrum accumulators, then IFFT into y[ch·B ..]. */
  private convolve(b: Bank, ch: number, y: Float64Array): void {
    const P = this.maxPartitions;
    const accRe = this.accRe;
    const accIm = this.accIm;
    accRe.fill(0);
    accIm.fill(0);
    const parts = Math.min(b.partitions, this.filled);
    for (let p = 0; p < parts; p++) {
      const slot = (this.head - p + P) % P;
      const xo = (ch * P + slot) * BINS;
      const ho = (ch * P + p) * BINS;
      const xr = this.fdlRe;
      const xi = this.fdlIm;
      const hr = b.re;
      const hi = b.im;
      for (let k = 0; k < BINS; k++) {
        const ar = xr[xo + k];
        const ai = xi[xo + k];
        const br = hr[ho + k];
        const bi = hi[ho + k];
        accRe[k] += ar * br - ai * bi;
        accIm[k] += ar * bi + ai * br;
      }
    }
    const re = this.wRe;
    const im = this.wIm;
    re[0] = accRe[0];
    im[0] = 0;
    re[BLOCK] = accRe[BLOCK];
    im[BLOCK] = 0;
    for (let k = 1; k < BLOCK; k++) {
      re[k] = accRe[k];
      im[k] = accIm[k];
      re[FFT_SIZE - k] = accRe[k];
      im[FFT_SIZE - k] = -accIm[k];
    }
    this.fft.inverse(re, im);
    const g = b.gain;
    const o = ch * BLOCK;
    for (let i = 0; i < BLOCK; i++) y[o + i] = g * re[BLOCK + i];
  }

  /**
   * One block: `input[ch]` (BLOCK samples, or absent for silence) to
   * `output[ch]`. Crossfades when a fade is running.
   */
  process(input: ArrayLike<number>[] | null, output: Float32Array[]): void {
    const P = this.maxPartitions;
    this.head = (this.head + 1) % P;
    this.filled = Math.min(this.filled + 1, P);
    for (let ch = 0; ch < this.channels; ch++) {
      const h = this.history[ch];
      h.copyWithin(0, BLOCK);
      const x = input && input[Math.min(ch, input.length - 1)];
      for (let i = 0; i < BLOCK; i++) h[BLOCK + i] = x ? x[i] : 0;
      const re = this.wRe;
      const im = this.wIm;
      for (let i = 0; i < FFT_SIZE; i++) {
        re[i] = h[i];
        im[i] = 0;
      }
      this.fft.forward(re, im);
      const off = (ch * P + this.head) * BINS;
      for (let k = 0; k < BINS; k++) {
        this.fdlRe[off + k] = re[k];
        this.fdlIm[off + k] = im[k];
      }
    }
    const from = this.banks[this.active];
    for (let ch = 0; ch < this.channels; ch++) this.convolve(from, ch, this.yOld);
    if (this.fadeTo < 0) {
      for (let ch = 0; ch < Math.min(this.channels, output.length); ch++) {
        const out = output[ch];
        const o = ch * BLOCK;
        for (let i = 0; i < BLOCK; i++) out[i] = this.yOld[o + i];
      }
      return;
    }
    const to = this.banks[this.fadeTo];
    for (let ch = 0; ch < this.channels; ch++) this.convolve(to, ch, this.yNew);
    const L = this.fadeLength;
    for (let ch = 0; ch < Math.min(this.channels, output.length); ch++) {
      const out = output[ch];
      const o = ch * BLOCK;
      for (let i = 0; i < BLOCK; i++) {
        const g = Math.min(1, (this.fadePos + i + 1) / L);
        out[i] = (1 - g) * this.yOld[o + i] + g * this.yNew[o + i];
      }
    }
    this.fadePos += BLOCK;
    if (this.fadePos >= L) {
      this.active = this.fadeTo;
      this.fadeTo = -1;
    }
  }
}
