// Histogram of one per-run metric of a Monte Carlo run: equal-width bins
// over the values the runs have (runs without the metric are counted
// apart, never binned), with the engine's median and 5 / 95 % points
// marked. The bin counts are listed in a table beside it.

import { formatParam, niceTicks } from '../format';
import type { Stats } from './analysis-types';

const NS = 'http://www.w3.org/2000/svg';

function svg<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>, text?: string): SVGElementTagNameMap[K] {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  if (text !== undefined) e.textContent = text;
  return e;
}

export interface Bins {
  lo: number;
  width: number;
  counts: number[];
}

/** Equal-width bins: √n of them (5 to 30), over [min, max] of the finite values. */
export function binValues(values: number[]): Bins | null {
  const v = values.filter((x) => Number.isFinite(x));
  if (!v.length) return null;
  let lo = Math.min(...v);
  let hi = Math.max(...v);
  if (hi === lo) {
    const d = Math.abs(lo) * 1e-3 || 1e-3;
    lo -= d;
    hi += d;
  }
  const n = Math.min(30, Math.max(5, Math.round(Math.sqrt(v.length))));
  const width = (hi - lo) / n;
  const counts = new Array<number>(n).fill(0);
  for (const x of v) counts[Math.min(n - 1, Math.floor((x - lo) / width))]++;
  return { lo, width, counts };
}

export function histogramSvg(b: Bins, stats: Stats, unit: string, width: number, label: string): SVGSVGElement {
  const H = 170;
  const M = { l: 40, r: 12, t: 16, b: 34 };
  const s = svg('svg', { width, height: H, viewBox: `0 0 ${width} ${H}` });
  s.setAttribute('role', 'img');
  const total = b.counts.reduce((a, c) => a + c, 0);
  s.setAttribute(
    'aria-label',
    `Histogram of ${label} over ${total} runs in ${b.counts.length} bins from ${formatParam(b.lo, 5)} to ${formatParam(b.lo + b.width * b.counts.length, 5)}${unit ? ` ${unit}` : ''}; ` +
      'a solid line marks the median, dashed lines the 5 and 95 % points. The bin counts are listed in the table beside it.',
  );
  const hi = b.lo + b.width * b.counts.length;
  const xOf = (x: number) => M.l + ((x - b.lo) / (hi - b.lo)) * (width - M.l - M.r);
  const cmax = Math.max(...b.counts, 1);
  const yOf = (c: number) => H - M.b - (c / cmax) * (H - M.t - M.b);
  // Count axis.
  for (const t of niceTicks(0, cmax, 4, [1, 2, 5]).ticks.filter((t) => Number.isInteger(t))) {
    s.append(svg('line', { x1: M.l, x2: width - M.r, y1: yOf(t), y2: yOf(t), class: 'an-t-grid' }));
    s.append(svg('text', { x: M.l - 6, y: yOf(t) + 4, 'text-anchor': 'end' }, String(t)));
  }
  // Bars, with a surface-coloured gap between neighbours.
  b.counts.forEach((c, i) => {
    if (!c) return;
    const x0 = xOf(b.lo + i * b.width) + 1;
    const x1 = xOf(b.lo + (i + 1) * b.width) - 1;
    s.append(svg('rect', { x: x0, y: yOf(c), width: Math.max(1, x1 - x0), height: H - M.b - yOf(c), rx: 1.5 }));
  });
  // Value axis.
  s.append(svg('line', { x1: M.l, x2: width - M.r, y1: H - M.b + 0.5, y2: H - M.b + 0.5, class: 'an-t-zero' }));
  const { ticks } = niceTicks(b.lo, hi, Math.max(2, Math.floor((width - M.l - M.r) / 70)));
  for (const t of ticks) {
    if (t < b.lo - 1e-12 || t > hi + 1e-12) continue;
    s.append(svg('text', { x: xOf(t), y: H - M.b + 14, 'text-anchor': 'middle' }, formatParam(t, 5)));
  }
  s.append(svg('text', { x: (M.l + width - M.r) / 2, y: H - 4, 'text-anchor': 'middle' }, `${label}${unit ? ` (${unit})` : ''}`));
  // Engine percentiles: the median solid, the 5 and 95 % points dashed.
  for (const [v, cls] of [
    [stats.p5, 'an-hist-pct'],
    [stats.p95, 'an-hist-pct'],
    [stats.median, 'an-hist-median'],
  ] as const) {
    if (v === null || v < b.lo || v > hi) continue;
    const x = xOf(v);
    s.append(svg('line', { x1: x, x2: x, y1: M.t - 6, y2: H - M.b, class: cls }));
  }
  return s;
}
