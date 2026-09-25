// Sensitivity heat map: parameters (rows) × frequency (columns, log axis),
// each cell the engine's dB per % of one probe, on a diverging colour scale
// through a neutral grey at zero. The validity shading of the base design is
// marked as on the plots: a strip above the map with the plots' fills and
// labels, dashed band edges through the map, and a hatch over the shaded
// cells (their colour stays readable, the hatch says "not trusted").
//
// A crosshair cell is moved with the pointer or the arrow keys and read out
// in numbers; a highlighted frequency range (an explain band) gets solid
// edges and a bar in the strip, but no tint over the cells, whose colours
// carry the values.

import { formatHz, formatHzTick, freqTicks } from '../format';
import { readTheme, type Highlight, type Theme } from '../plot';
import { band, bandNote } from '../shading';
import type { Shading } from '../types';
import { Diverging, el, onThemeChange } from './analysis-ui';

export interface HeatRow {
  name: string;
  label: string;
  values: (number | null)[];
}

export interface HeatData {
  freqs: number[];
  rows: HeatRow[];
  shading: Shading;
}

export type RangeMode = 'credible' | 'all';

const ROW_H = 20;
const GAP = 2;
const STRIP_H = 18;
const AXIS_H = 26;
const RIGHT = 12;

const set = (x: number | null | undefined): x is number => x !== null && x !== undefined;

export class HeatMap {
  readonly el = el('figure', 'an-heat');
  readonly canvas = el('canvas');
  readonly readout = el('p', 'an-readout');
  /** Spoken readout (keyboard moves only). */
  private readonly spoken = el('p', 'visually-hidden');
  private data: HeatData | null = null;
  private theme: Theme = readTheme();
  private readonly div = new Diverging();
  private cssW = 0;
  private labelW = 120;
  cursor: { row: number; col: number } | null = null;
  highlight: Highlight | null = null;
  range: RangeMode = 'credible';
  /** Largest |value| the colours span, and over which columns (set by `render`). */
  scaleMax = 0;
  scaleBasis: RangeMode = 'credible';
  private frame = 0;
  /** Cell colours, recomputed only when the data, the scale or the theme change (not on crosshair moves). */
  private colors: string[][] = [];
  private colorKey = '';
  private version = 0;
  /** Scale last reported to `onScale` (the legend is rebuilt only when it changes). */
  private scaleKey = '';

  constructor(
    caption: HTMLElement,
    private readonly onScale: (max: number, basis: RangeMode) => void,
  ) {
    this.canvas.tabIndex = 0;
    this.canvas.setAttribute('role', 'img');
    this.spoken.setAttribute('aria-live', 'polite');
    this.el.append(caption, this.canvas, this.readout, this.spoken);
    new ResizeObserver(() => this.render()).observe(this.el);
    onThemeChange(() => {
      this.theme = readTheme();
      this.div.read();
      this.render();
    });
    this.canvas.addEventListener('pointermove', (ev) => {
      const c = this.cellAt(ev.offsetX, ev.offsetY);
      if (c && (c.row !== this.cursor?.row || c.col !== this.cursor?.col)) this.setCursor(c, false);
    });
    this.canvas.addEventListener('keydown', (ev) => this.onKey(ev));
    this.readText();
  }

  set(data: HeatData | null): void {
    this.data = data;
    this.version++;
    if (this.cursor && (!data || this.cursor.row >= data.rows.length || this.cursor.col >= data.freqs.length)) this.cursor = null;
    this.readText();
    this.render();
  }

  setHighlight(h: Highlight | null): void {
    this.highlight = h;
    this.render();
  }

  setRange(mode: RangeMode): void {
    this.range = mode;
    this.render();
  }

  /** Moves the crosshair cell (null hides it). */
  setCursor(c: { row: number; col: number } | null, fromKeyboard: boolean): void {
    this.cursor = c;
    this.readText(fromKeyboard);
    this.render();
  }

  /** Value, frequency and parameter of a cell. */
  cell(row: number, col: number): { f: number; value: number | null; row: HeatRow } | null {
    const d = this.data;
    if (!d || !d.rows[row] || col < 0 || col >= d.freqs.length) return null;
    return { f: d.freqs[col], value: d.rows[row].values[col], row: d.rows[row] };
  }

  private readText(spoken = false): void {
    const d = this.data;
    if (!d || !d.rows.length) {
      this.readout.textContent = '';
      return;
    }
    const c = this.cursor ? this.cell(this.cursor.row, this.cursor.col) : null;
    if (!c) {
      this.readout.textContent = 'Crosshair: point at the map, or focus it and use the arrow keys (↑ ↓ parameter, ← → frequency), to read a cell.';
      return;
    }
    const v = c.value === null ? 'no value' : `${c.value.toPrecision(4).replace('-', '−')} dB per %`;
    const text = `${c.row.label} (${c.row.name}) at ${formatHz(c.f)}: ${v} (grid point ${this.cursor!.col + 1} of ${d.freqs.length}), ${bandNote(d.shading, c.f)}.`;
    this.readout.textContent = text;
    this.readout.dataset.value = c.value === null ? '' : String(c.value);
    if (spoken) this.spoken.textContent = text;
  }

  // ----- geometry ------------------------------------------------------------

  private rect(): { x0: number; x1: number; y0: number; y1: number } {
    const n = this.data?.rows.length ?? 0;
    const y0 = STRIP_H;
    return { x0: this.labelW, x1: this.cssW - RIGHT, y0, y1: y0 + n * (ROW_H + GAP) - GAP };
  }

  private span(): [number, number] {
    const f = this.data!.freqs;
    return [f[0], f[f.length - 1]];
  }

  private xOf(f: number): number {
    const r = this.rect();
    const [lo, hi] = this.span();
    if (hi <= lo) return (r.x0 + r.x1) / 2;
    return r.x0 + ((Math.log(f) - Math.log(lo)) / (Math.log(hi) - Math.log(lo))) * (r.x1 - r.x0);
  }

  /** Column k spans the geometric midpoints to its neighbours. */
  private colEdges(k: number): [number, number] {
    const f = this.data!.freqs;
    const r = this.rect();
    if (f.length === 1) return [r.x0, r.x1];
    const a = k === 0 ? r.x0 : this.xOf(Math.sqrt(f[k - 1] * f[k]));
    const b = k === f.length - 1 ? r.x1 : this.xOf(Math.sqrt(f[k] * f[k + 1]));
    return [a, b];
  }

  private cellAt(x: number, y: number): { row: number; col: number } | null {
    const d = this.data;
    if (!d || !d.rows.length) return null;
    const r = this.rect();
    if (x < r.x0 || x > r.x1 || y < r.y0 || y > r.y1) return null;
    const row = Math.min(d.rows.length - 1, Math.floor((y - r.y0) / (ROW_H + GAP)));
    const [lo, hi] = this.span();
    const f = Math.exp(Math.log(lo) + ((x - r.x0) / (r.x1 - r.x0)) * (Math.log(hi) - Math.log(lo)));
    let col = 0;
    for (let k = 1; k < d.freqs.length; k++) if (Math.abs(Math.log(d.freqs[k] / f)) < Math.abs(Math.log(d.freqs[col] / f))) col = k;
    return { row, col };
  }

  private onKey(ev: KeyboardEvent): void {
    const d = this.data;
    if (!d || !d.rows.length) return;
    const c = this.cursor ?? { row: 0, col: Math.floor(d.freqs.length / 2) };
    const n = d.freqs.length;
    let next: { row: number; col: number } | null = { ...c };
    const step = ev.shiftKey ? 10 : 1;
    switch (ev.key) {
      case 'ArrowRight':
        if (this.cursor) next.col = Math.min(n - 1, c.col + step);
        break;
      case 'ArrowLeft':
        if (this.cursor) next.col = Math.max(0, c.col - step);
        break;
      case 'ArrowDown':
        if (this.cursor) next.row = Math.min(d.rows.length - 1, c.row + 1);
        break;
      case 'ArrowUp':
        if (this.cursor) next.row = Math.max(0, c.row - 1);
        break;
      case 'Home':
        next.col = 0;
        break;
      case 'End':
        next.col = n - 1;
        break;
      case 'PageUp':
        next.row = 0;
        break;
      case 'PageDown':
        next.row = d.rows.length - 1;
        break;
      case 'Escape':
        next = null;
        break;
      default:
        return;
    }
    ev.preventDefault();
    this.setCursor(next, true);
  }

  // ----- drawing -----------------------------------------------------------

  render(): void {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.draw();
    });
  }

  /**
   * Largest |value| over the unshaded columns ('credible') or all of them,
   * and which it is: the whole sweep when nothing unshaded has a value.
   */
  private computeScale(): { max: number; basis: RangeMode } {
    const d = this.data!;
    let m = 0;
    let credible = 0;
    for (const r of d.rows) {
      r.values.forEach((v, k) => {
        if (v === null || !Number.isFinite(v)) return;
        if (band(d.shading, d.freqs[k]) === 0) credible = Math.max(credible, Math.abs(v));
        m = Math.max(m, Math.abs(v));
      });
    }
    return this.range === 'credible' && credible > 0 ? { max: credible, basis: 'credible' } : { max: m, basis: 'all' };
  }

  draw(): void {
    const d = this.data;
    const th = this.theme;
    const n = d?.rows.length ?? 0;
    const cssH = STRIP_H + Math.max(1, n) * (ROW_H + GAP) - GAP + AXIS_H;
    this.canvas.style.height = `${cssH}px`;
    const w = this.canvas.clientWidth;
    if (w === 0) return;
    this.cssW = w;
    const dpr = window.devicePixelRatio || 1;
    if (this.canvas.width !== Math.round(w * dpr) || this.canvas.height !== Math.round(cssH * dpr)) {
      this.canvas.width = Math.round(w * dpr);
      this.canvas.height = Math.round(cssH * dpr);
    }
    const ctx = this.canvas.getContext('2d');
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = th.bg;
    ctx.fillRect(0, 0, w, cssH);
    if (!d || !n || !d.freqs.length) {
      this.canvas.setAttribute('aria-label', 'Sensitivity map: no data yet.');
      return;
    }
    ctx.font = `12px ${th.font}`;
    const widest = Math.max(...d.rows.map((r) => ctx.measureText(r.label).width));
    this.labelW = Math.round(Math.min(Math.max(90, widest + 12), w * 0.42));
    const r = this.rect();
    const { max, basis } = this.computeScale();
    this.scaleMax = max;
    this.scaleBasis = basis;
    if (`${max}|${basis}` !== this.scaleKey) {
      this.scaleKey = `${max}|${basis}`;
      this.onScale(max, basis);
    }

    // Cells.
    const key = `${this.version}|${max}|${this.div.hex.neg}|${this.div.hex.mid}|${this.div.hex.pos}|${th.bg}`;
    if (key !== this.colorKey) {
      this.colorKey = key;
      this.colors = d.rows.map((row) => row.values.map((v) => (v === null || !Number.isFinite(v) ? th.bg : this.div.css(max > 0 ? v / max : 0))));
    }
    for (let i = 0; i < n; i++) {
      const y = r.y0 + i * (ROW_H + GAP);
      for (let k = 0; k < d.freqs.length; k++) {
        const [a, b] = this.colEdges(k);
        ctx.fillStyle = this.colors[i][k];
        // Overdraw by half a pixel so no seam shows between columns.
        ctx.fillRect(a, y, b - a + 0.5, ROW_H);
      }
    }

    // Validity shading: the strip of fills and labels, a hatch over the
    // shaded cells, and dashed edges.
    const edges: number[] = [];
    const bandSpans: { a: number; b: number; level: number }[] = [];
    for (let k = 0; k < d.freqs.length; k++) {
      const lvl = band(d.shading, d.freqs[k]);
      const [a, b] = this.colEdges(k);
      const last = bandSpans[bandSpans.length - 1];
      if (last && last.level === lvl) last.b = b;
      else {
        if (last) edges.push(a);
        bandSpans.push({ a, b, level: lvl });
      }
    }
    ctx.save();
    ctx.beginPath();
    ctx.rect(r.x0, r.y0, r.x1 - r.x0, r.y1 - r.y0);
    ctx.clip();
    for (const s of bandSpans) {
      if (!s.level) continue;
      const pitch = s.level === 2 ? 4 : 8;
      ctx.strokeStyle = th.shadeEdge;
      ctx.globalAlpha = 0.45;
      ctx.lineWidth = 1;
      ctx.beginPath();
      for (let x = s.a - (r.y1 - r.y0); x < s.b; x += pitch) {
        ctx.moveTo(x, r.y1);
        ctx.lineTo(x + (r.y1 - r.y0), r.y0);
      }
      ctx.save();
      ctx.rect(s.a, r.y0, s.b - s.a, r.y1 - r.y0);
      ctx.clip();
      ctx.stroke();
      ctx.restore();
      ctx.globalAlpha = 1;
    }
    ctx.restore();
    for (const s of bandSpans) {
      ctx.fillStyle = s.level === 2 ? th.shade2 : s.level === 1 ? th.shade1 : th.bg;
      ctx.fillRect(s.a, 0, s.b - s.a, STRIP_H - 3);
    }
    ctx.setLineDash([3, 3]);
    ctx.strokeStyle = th.shadeEdge;
    ctx.lineWidth = 1;
    for (const x of edges) {
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, 0);
      ctx.lineTo(Math.round(x) + 0.5, r.y1);
      ctx.stroke();
    }
    ctx.setLineDash([]);
    ctx.font = `11px ${th.font}`;
    ctx.textBaseline = 'middle';
    ctx.fillStyle = th.ink2;
    for (const s of bandSpans) {
      if (!s.level) continue;
      const low = set(d.shading.low_begin_hz) && s.b <= this.xOf(d.shading.low_begin_hz) + 1;
      const text = low ? (s.level === 2 ? 'below hard limit' : 'below validated range') : s.level === 2 ? '≥36 %' : '≥10 %';
      const tw = ctx.measureText(text).width;
      if (tw + 6 > s.b - s.a) continue;
      ctx.textAlign = 'left';
      ctx.fillText(text, s.a + 3, (STRIP_H - 3) / 2);
    }

    // Highlighted range (an explain band).
    const hl = this.highlight;
    if (hl) {
      const [lo, hi] = this.span();
      if (hl.hi >= lo && hl.lo <= hi) {
        let xa = Math.max(r.x0, this.xOf(Math.max(hl.lo, lo)));
        let xz = Math.min(r.x1, this.xOf(Math.min(hl.hi, hi)));
        if (xz - xa < 4) {
          const c = (xa + xz) / 2;
          xa = c - 2;
          xz = c + 2;
        }
        // Tinted in the strip only: a tint over the cells would shift their colours.
        ctx.fillStyle = th.hlEdge;
        ctx.fillRect(xa, STRIP_H - 6, xz - xa, 4);
        ctx.strokeStyle = th.hlEdge;
        ctx.lineWidth = 2;
        for (const x of [xa, xz]) {
          ctx.beginPath();
          ctx.moveTo(x, 0);
          ctx.lineTo(x, r.y1);
          ctx.stroke();
        }
      }
    }

    // Row labels.
    ctx.font = `12px ${th.font}`;
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    for (let i = 0; i < n; i++) {
      const y = r.y0 + i * (ROW_H + GAP) + ROW_H / 2;
      let t = d.rows[i].label;
      const room = r.x0 - 8;
      if (ctx.measureText(t).width > room) {
        while (t.length > 1 && ctx.measureText(`${t}…`).width > room) t = t.slice(0, -1);
        t = `${t}…`;
      }
      const on = this.cursor?.row === i;
      ctx.font = `${on ? '700 ' : ''}12px ${th.font}`;
      ctx.fillStyle = th.ink;
      ctx.fillText(t, r.x0 - 6, y);
    }

    // Frequency axis.
    const [lo, hi] = this.span();
    ctx.strokeStyle = th.axis;
    ctx.beginPath();
    ctx.moveTo(r.x0, r.y1 + 0.5);
    ctx.lineTo(r.x1, r.y1 + 0.5);
    ctx.stroke();
    ctx.font = `11px ${th.font}`;
    ctx.fillStyle = th.ink2;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'top';
    const ft = freqTicks(lo, hi);
    let lastRight = -Infinity;
    for (const f of ft.labelled) {
      const x = this.xOf(f);
      const label = formatHzTick(f);
      const half = ctx.measureText(label).width / 2;
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, r.y1);
      ctx.lineTo(Math.round(x) + 0.5, r.y1 + 4);
      ctx.stroke();
      if (x - half < lastRight + 6 || x + half > w - 2 || x - half < r.x0 - 20) continue;
      ctx.fillText(label, x, r.y1 + 6);
      lastRight = x + half;
    }
    ctx.textAlign = 'right';
    ctx.fillText('Hz', r.x0 - 6, r.y1 + 6);

    // Crosshair.
    if (this.cursor) {
      const [a, b] = this.colEdges(this.cursor.col);
      const x = (a + b) / 2;
      ctx.strokeStyle = th.crosshair;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, r.y0);
      ctx.lineTo(Math.round(x) + 0.5, r.y1);
      ctx.stroke();
      const y = r.y0 + this.cursor.row * (ROW_H + GAP);
      const cw = Math.max(6, b - a);
      ctx.strokeStyle = th.bg;
      ctx.lineWidth = 4;
      ctx.strokeRect(x - cw / 2 - 1, y - 1, cw + 2, ROW_H + 2);
      ctx.strokeStyle = th.ink;
      ctx.lineWidth = 2;
      ctx.strokeRect(x - cw / 2 - 1, y - 1, cw + 2, ROW_H + 2);
    }

    this.canvas.setAttribute(
      'aria-label',
      `Sensitivity map: ${n} parameters by ${d.freqs.length} frequencies from ${formatHz(lo)} to ${formatHz(hi)}, ` +
        `dB per % on a diverging scale of ±${max.toPrecision(3)}; hatched columns lie in the validity shading. ` +
        'Arrow keys move the crosshair cell (up and down: parameter, left and right: frequency); values are read out below the map and listed in its data table.',
    );
  }
}
