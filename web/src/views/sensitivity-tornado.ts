// Tornado chart of one metric: for each parameter, the change of the metric
// with the parameter at the low and at the high end of its range (its
// tolerance, or the assumed ±10 % marked as such), each re-solved by the
// engine (docs/analysis.md, "Tornado charts"). Rows keep the engine's order,
// the larger absolute change first. The low-end bar sits in the upper half
// of a row and the high-end bar in the lower half, so the two never cover
// each other; each carries its value as text.

import { formatParam, niceTicks } from '../format';
import { toleranceText } from '../design';
import { el, signedSig } from './analysis-ui';
import type { Tornado, TornadoRow } from './analysis-types';

const NS = 'http://www.w3.org/2000/svg';
const ROW_H = 40;
const BAR_H = 12;
const TOP = 8;
const AXIS_H = 30;

function svg<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>, text?: string): SVGElementTagNameMap[K] {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  if (text !== undefined) e.textContent = text;
  return e;
}

/** What a row's range is: its tolerance, or the assumed default, and any clipping. */
export function rangeNote(r: TornadoRow, unit: string | null): string {
  const parts = [r.basis === 'assumed' || !r.tolerance ? 'assumed ±10 %' : toleranceText({ ...r.tolerance, source: undefined }, unit ?? '')];
  if (r.clipped_low) parts.push('low end clipped at the minimum');
  if (r.clipped_high) parts.push('high end clipped at the maximum');
  if (r.topology_changed) parts.push('topology changes');
  return parts.join('; ');
}

/** Short unit text of the metric's change ("dB", "Hz", "Ω"). */
export function deltaUnit(t: Tornado): string {
  return t.unit.replace(/^dB SPL$/, 'dB').replace(/ohm/g, 'Ω');
}

let measureCtx: CanvasRenderingContext2D | null = null;
function textWidth(text: string, font: string): number {
  measureCtx ??= document.createElement('canvas').getContext('2d');
  if (!measureCtx) return text.length * 7;
  measureCtx.font = font;
  return measureCtx.measureText(text).width;
}

/**
 * Draws the chart at `width` CSS pixels. `units` gives the unit of each
 * parameter for its range note.
 */
export function tornadoSvg(t: Tornado, width: number, units: Map<string, string | null>): SVGSVGElement {
  const rows = t.rows;
  const font = getComputedStyle(document.documentElement).getPropertyValue('--font-sans') || 'system-ui, sans-serif';
  const du = deltaUnit(t);
  // Narrow screens: each row's label above its bars, which then use the full width.
  const stacked = width < 520;
  const rowH = stacked ? ROW_H + 18 : ROW_H;
  const labelW = stacked ? 0 : Math.min(Math.max(...rows.map((r) => textWidth(r.label, `600 12px ${font}`)), 60) + 14, width * 0.42);
  const valueRoom = 52;
  const x0 = labelW + valueRoom;
  const x1 = width - valueRoom;
  const height = TOP + rows.length * rowH + AXIS_H;
  const s = svg('svg', { width, height, viewBox: `0 0 ${width} ${height}`, class: 'an-tornado-svg' });
  s.setAttribute('role', 'img');
  s.setAttribute(
    'aria-label',
    `Tornado chart of ${t.description} (${t.unit}; base ${formatParam(t.base, 6)}): ${rows.length} parameters, largest change first. ` +
      'The same values are listed in the table below the chart.',
  );
  let m = 0;
  for (const r of rows) for (const v of [r.delta_low, r.delta_high]) if (v !== null && Number.isFinite(v)) m = Math.max(m, Math.abs(v));
  if (!(m > 0)) m = 1;
  const { ticks } = niceTicks(-m, m, 6, [1, 2, 2.5, 5]);
  const lo = Math.min(-m, ticks[0]);
  const hi = Math.max(m, ticks[ticks.length - 1]);
  const xOf = (v: number) => x0 + ((v - lo) / (hi - lo)) * (x1 - x0);
  const yEnd = TOP + rows.length * rowH;

  // Grid and zero line.
  for (const tk of ticks) {
    const x = Math.round(xOf(tk)) + 0.5;
    s.append(svg('line', { x1: x, x2: x, y1: TOP, y2: yEnd, class: tk === 0 ? 'an-t-zero' : 'an-t-grid' }));
    s.append(svg('text', { x, y: yEnd + 14, 'text-anchor': 'middle', class: 'an-t-tick' }, signedSig(tk, 3)));
  }
  s.append(svg('text', { x: (x0 + x1) / 2, y: yEnd + 27, 'text-anchor': 'middle', class: 'an-t-tick' }, `change of ${t.description} (${du})`));

  rows.forEach((r, i) => {
    const y = TOP + i * rowH;
    const g = svg('g', { class: 'an-t-row' });
    g.dataset.param = r.name;
    if (i % 2 === 1) g.append(svg('rect', { x: 0, y, width, height: rowH, class: 'an-t-band' }));
    const note = rangeNote(r, units.get(r.name) ?? null);
    if (stacked) {
      g.append(svg('text', { x: 4, y: y + 14, class: 'an-t-label' }, r.label));
      g.append(svg('text', { x: 4, y: y + 28, class: 'an-t-note' }, note));
    } else {
      g.append(svg('text', { x: labelW - 8, y: y + 16, 'text-anchor': 'end', class: 'an-t-label' }, r.label));
      g.append(svg('text', { x: labelW - 8, y: y + 31, 'text-anchor': 'end', class: 'an-t-note' }, note));
    }
    const top = y + (stacked ? 32 : 6);
    const bar = (v: number | null, top: number, cls: string, end: string) => {
      if (v === null || !Number.isFinite(v)) {
        g.append(svg('text', { x: xOf(0) + 4, y: top + BAR_H - 2, class: 'an-t-value' }, `${end}: no value`));
        return;
      }
      const a = xOf(Math.min(0, v));
      const b = xOf(Math.max(0, v));
      g.append(svg('rect', { x: a, y: top, width: Math.max(1, b - a), height: BAR_H, rx: 2, class: cls }));
      const right = v >= 0;
      g.append(
        svg(
          'text',
          { x: right ? b + 4 : a - 4, y: top + BAR_H - 2, 'text-anchor': right ? 'start' : 'end', class: 'an-t-value' },
          signedSig(v, 3),
        ),
      );
    };
    bar(r.delta_low, top, 'an-t-low', 'low end');
    bar(r.delta_high, top + BAR_H + 2, 'an-t-high', 'high end');
    s.append(g);
  });
  return s;
}

/** The chart's legend: which bar is which end (colour and position). */
export function tornadoLegend(): HTMLElement {
  const l = el('p', 'an-legend an-t-legend');
  const item = (cls: string, text: string) => {
    const k = el('span', `an-swatch ${cls}`);
    k.setAttribute('aria-hidden', 'true');
    const s = el('span', 'an-legend-item');
    s.append(k, text);
    return s;
  };
  l.append(item('an-t-low', 'upper bar: parameter at the low end of its range'), item('an-t-high', 'lower bar: at the high end'));
  return l;
}
