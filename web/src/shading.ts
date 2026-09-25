// Validity bands of a result (spec Section 2; docs/netlist.md, "Validity
// limits and shading"), with both upper and lower limits.

import { formatHz } from './format';
import type { Shading } from './types';

export type Band = 0 | 1 | 2;

const set = (x: number | null | undefined): x is number => x !== null && x !== undefined;

/**
 * 0 unshaded, 1 light, 2 dark at frequency f: dark if f ≥ deep_hz or
 * f < low_deep_hz, light if f ≥ begin_hz or f < low_begin_hz. The same rule
 * as the engine's `Shading::band`.
 */
export function band(s: Shading | null | undefined, f: number): Band {
  if (!s) return 0;
  const above = (x: number | null | undefined) => set(x) && f >= x;
  const below = (x: number | null | undefined) => set(x) && f < x;
  if (above(s.deep_hz) || below(s.low_deep_hz)) return 2;
  if (above(s.begin_hz) || below(s.low_begin_hz)) return 1;
  return 0;
}

/** "light" / "dark" / "" for tables. */
export function bandName(b: Band): string {
  return b === 2 ? 'dark' : b === 1 ? 'light' : '';
}

/** Readout sentence: which band f is in, and what the band means. */
export function bandNote(s: Shading | null | undefined, f: number): string {
  if (!s) return '';
  const b = band(s, f);
  if (b === 2) {
    return set(s.low_deep_hz) && f < s.low_deep_hz
      ? `in the dark band below ${formatHz(s.low_deep_hz)}: past an element's lower hard limit`
      : 'in the dark band: lumped-model error ≥ 36 % or past a hard limit';
  }
  if (b === 1) {
    return set(s.low_begin_hz) && f < s.low_begin_hz
      ? `in the light band below ${formatHz(s.low_begin_hz)}: below the range an element is validated for`
      : 'in the light band: lumped-model error ≥ 10 %';
  }
  return 'outside the shading: no element reports a validity limit here';
}
