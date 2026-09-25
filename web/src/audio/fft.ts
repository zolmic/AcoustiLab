// Radix-2 complex FFT on split real/imaginary Float64Arrays, in place and
// allocation-free once planned (the audio thread uses it inside process()).
//
// forward:  X_k = Σ_n x_n·e^{−j2πkn/N}
// inverse:  x_n = (1/N)·Σ_k X_k·e^{+j2πkn/N}
//
// Twiddles are evaluated directly as cos/sin(2πk/N) (no recurrence), and
// the bit-reversal permutation is tabulated. Checked against a direct DFT
// in tests/audio-dsp.spec.ts.

export class FFT {
  readonly n: number;
  private readonly cos: Float64Array;
  private readonly sin: Float64Array;
  private readonly rev: Uint32Array;

  constructor(n: number) {
    if (n < 1 || (n & (n - 1)) !== 0) throw new Error(`FFT length ${n} is not a power of two`);
    this.n = n;
    this.cos = new Float64Array(n / 2);
    this.sin = new Float64Array(n / 2);
    for (let k = 0; k < n / 2; k++) {
      this.cos[k] = Math.cos((2 * Math.PI * k) / n);
      this.sin[k] = -Math.sin((2 * Math.PI * k) / n);
    }
    this.rev = new Uint32Array(n);
    const bits = Math.round(Math.log2(n));
    for (let i = 0; i < n; i++) {
      let r = 0;
      for (let b = 0; b < bits; b++) r |= ((i >> b) & 1) << (bits - 1 - b);
      this.rev[i] = r;
    }
  }

  forward(re: Float64Array, im: Float64Array): void {
    this.transform(re, im, false);
  }

  /** Inverse transform including the 1/N factor. */
  inverse(re: Float64Array, im: Float64Array): void {
    this.transform(re, im, true);
    const k = 1 / this.n;
    for (let i = 0; i < this.n; i++) {
      re[i] *= k;
      im[i] *= k;
    }
  }

  private transform(re: Float64Array, im: Float64Array, inverse: boolean): void {
    const n = this.n;
    const rev = this.rev;
    for (let i = 0; i < n; i++) {
      const j = rev[i];
      if (j > i) {
        let t = re[i];
        re[i] = re[j];
        re[j] = t;
        t = im[i];
        im[i] = im[j];
        im[j] = t;
      }
    }
    const sgn = inverse ? -1 : 1;
    for (let len = 2; len <= n; len <<= 1) {
      const half = len >> 1;
      const stride = n / len;
      for (let start = 0; start < n; start += len) {
        for (let k = 0; k < half; k++) {
          const wr = this.cos[k * stride];
          const wi = sgn * this.sin[k * stride];
          const a = start + k;
          const b = a + half;
          const br = re[b] * wr - im[b] * wi;
          const bi = re[b] * wi + im[b] * wr;
          re[b] = re[a] - br;
          im[b] = im[a] - bi;
          re[a] += br;
          im[a] += bi;
        }
      }
    }
  }
}

const plans = new Map<number, FFT>();

/** A shared plan of length n (main thread and workers only). */
export function fftOf(n: number): FFT {
  let p = plans.get(n);
  if (!p) {
    p = new FFT(n);
    plans.set(n, p);
  }
  return p;
}

/** Smallest power of two ≥ n. */
export function nextPow2(n: number): number {
  let p = 1;
  while (p < n) p <<= 1;
  return p;
}
