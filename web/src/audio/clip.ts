// The ConvolverNode fallback's hard clip at the output ceiling (player.ts).

/**
 * WaveShaper curve of the fallback's hard clip: y = x for |x| ≤ c, ±c
 * beyond, c = 10^(ceiling/20). With 2^k + 1 points the identity points
 * are exact in f32 and the interpolation between them is exact; inputs
 * beyond ±1 take the end values.
 */
export function hardClipCurve(ceilingDb: number, points = 8193): Float32Array<ArrayBuffer> {
  const c = 10 ** (ceilingDb / 20);
  const curve = new Float32Array(points);
  for (let i = 0; i < points; i++) {
    const x = -1 + (2 * i) / (points - 1);
    curve[i] = Math.max(-c, Math.min(c, x));
  }
  return curve;
}
