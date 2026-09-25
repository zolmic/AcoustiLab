// 32-bit IEEE-float WAV of an impulse response, in the layout of the
// engine's writer (crates/acoustilab/src/time/wav.rs, spec Section 14),
// which the wasm build does not export: RIFF/WAVE, a `fmt ` chunk of 18
// bytes (format tag 3 = WAVE_FORMAT_IEEE_FLOAT, channels, sample rate, byte
// rate, block align, 32 bits, cbSize 0), the `fact` chunk that non-PCM
// formats require (the frame count), and `data`, all little-endian. Values
// are written as float32, without clipping.

/** Header bytes before the samples. */
export const WAV_HEADER_BYTES = 58;

/** One channel of samples as a float WAV file. Throws on a non-finite sample. */
export function floatWav(samples: ArrayLike<number>, sampleRate: number): Uint8Array {
  const n = samples.length;
  const dataLen = n * 4;
  const buf = new ArrayBuffer(WAV_HEADER_BYTES + dataLen);
  const v = new DataView(buf);
  const ascii = (at: number, s: string) => {
    for (let k = 0; k < s.length; k++) v.setUint8(at + k, s.charCodeAt(k));
  };
  const rate = Math.round(sampleRate);
  ascii(0, 'RIFF');
  v.setUint32(4, 4 + (8 + 18) + (8 + 4) + 8 + dataLen, true);
  ascii(8, 'WAVE');
  ascii(12, 'fmt ');
  v.setUint32(16, 18, true);
  v.setUint16(20, 3, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 4, true);
  v.setUint16(32, 4, true);
  v.setUint16(34, 32, true);
  v.setUint16(36, 0, true);
  ascii(38, 'fact');
  v.setUint32(42, 4, true);
  v.setUint32(46, n, true);
  ascii(50, 'data');
  v.setUint32(54, dataLen, true);
  for (let i = 0; i < n; i++) {
    const x = samples[i];
    if (!Number.isFinite(x)) throw new Error(`WAV: sample ${i} is not finite`);
    v.setFloat32(WAV_HEADER_BYTES + 4 * i, x, true);
  }
  return new Uint8Array(buf);
}

/**
 * The samples scaled as `acoustilab ir --wav` scales them: to a peak of 1,
 * or unchanged with `raw` (or when every sample is zero).
 */
export function wavGain(samples: ArrayLike<number>, raw: boolean): number {
  let peak = 0;
  for (let i = 0; i < samples.length; i++) peak = Math.max(peak, Math.abs(samples[i]));
  return raw || peak === 0 ? 1 : 1 / peak;
}
