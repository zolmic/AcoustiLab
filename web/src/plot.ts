// Canvas 2D plots sharing one log-frequency axis, validity shading and a
// crosshair (spec Sections 2 and 15).

import type { OverlaySeries, PlotGroup, Series } from './series';
import { DASHES, LIVE_WIDTH, OVERLAY_DASHES, OVERLAY_MIX, OVERLAY_WIDTH, PRIMARY_WIDTH, styleSlot } from './series';
import {
  formatHzTick,
  freqTicks,
  logTicks,
  niceTicks,
  prefixable,
  prefixFor,
  prettyUnit,
  superscript,
  tickDecimals,
} from './format';
import type { Shading } from './types';

/** Internal frequency range of the engine (spec Section 2, rule 6). */
export const RANGE_FULL: [number, number] = [10, 40_000];
/** Default view. */
export const RANGE_AUDIO: [number, number] = [20, 20_000];

export interface Theme {
  bg: string;
  ink: string;
  ink2: string;
  grid: string;
  gridMinor: string;
  axis: string;
  shade1: string;
  shade2: string;
  shadeEdge: string;
  crosshair: string;
  selection: string;
  hlFill: string;
  hlEdge: string;
  series: string[];
  font: string;
}

export function readTheme(): Theme {
  const cs = getComputedStyle(document.documentElement);
  const v = (name: string) => cs.getPropertyValue(name).trim();
  return {
    bg: v('--plot-bg'),
    ink: v('--ink'),
    ink2: v('--ink-2'),
    grid: v('--grid'),
    gridMinor: v('--grid-minor'),
    axis: v('--axis'),
    shade1: v('--shade-1'),
    shade2: v('--shade-2'),
    shadeEdge: v('--shade-edge'),
    crosshair: v('--crosshair'),
    selection: v('--selection'),
    hlFill: v('--hl-fill'),
    hlEdge: v('--hl-edge'),
    series: Array.from({ length: 8 }, (_, i) => v(`--series-${i + 1}`)),
    font: v('--font-sans') || 'system-ui, sans-serif',
  };
}

/**
 * `a` mixed with `b` (both "#rrggbb"), weight `t` on `a`. Baseline overlays
 * use the live curve's colour half mixed with the secondary ink: the hue
 * still names the probe, the washed-out tone says "not the live design",
 * and the mix keeps >= 3.5:1 against the plot surface and both validity
 * shades in either theme (every series colour mixed with --ink-2 at 1/2).
 */
export function mixHex(a: string, b: string, t: number): string {
  const rgb = (h: string) => (/^#[0-9a-f]{6}$/i.test(h) ? [1, 3, 5].map((k) => parseInt(h.slice(k, k + 2), 16)) : null);
  const x = rgb(a);
  const y = rgb(b);
  if (!x || !y) return a;
  return `#${x.map((c, k) => Math.round(t * c + (1 - t) * y[k]).toString(16).padStart(2, '0')).join('')}`;
}

/** A frequency range picked out on every plot (e.g. where a warning applies). */
export interface Highlight {
  lo: number;
  hi: number;
  label: string;
}

export interface PlotData {
  freqs: number[]; // ascending
  shading: Shading;
  highlight?: Highlight | null;
}

interface Axis {
  lo: number;
  hi: number;
  scale: 'linear' | 'log';
  ticks: number[];
  minor: number[];
  labels: string[];
  /** Unit text for the caption, including any prefix or scale factor. */
  unitText: string;
}

const M = { left: 60, right: 16, top: 10, bottom: 26 };
const HEIGHT = { main: 230, small: 130 };

function finite(v: number | null): v is number {
  return v !== null && Number.isFinite(v);
}

/** Index of the frequency nearest to f on a log scale (freqs ascending). */
export function nearestIndex(freqs: number[], f: number): number {
  let lo = 0;
  let hi = freqs.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (freqs[mid] <= f) lo = mid;
    else hi = mid;
  }
  return Math.abs(Math.log(freqs[lo] / f)) <= Math.abs(Math.log(freqs[hi] / f)) ? lo : hi;
}

export class Plot {
  readonly figure: HTMLElement;
  readonly canvas: HTMLCanvasElement;
  private readonly caption: HTMLElement;
  private readonly unitEl: HTMLElement;
  private cssW = 0;
  private cssH: number;

  constructor(
    public readonly group: PlotGroup,
    public readonly first: boolean,
  ) {
    this.figure = document.createElement('figure');
    this.figure.className = `plot plot-${group.height}`;
    this.figure.dataset.group = group.key;
    this.caption = document.createElement('figcaption');
    const title = document.createElement('span');
    title.className = 'plot-title';
    title.textContent = `${group.title} ${group.symbol}`;
    this.unitEl = document.createElement('span');
    this.unitEl.className = 'plot-unit';
    this.caption.append(title, ' ', this.unitEl);
    this.canvas = document.createElement('canvas');
    this.canvas.tabIndex = 0;
    this.canvas.setAttribute('role', 'img');
    this.cssH = HEIGHT[group.height];
    this.canvas.style.height = `${this.cssH}px`;
    this.figure.append(this.caption, this.canvas);
  }

  describe(visible: Series[], overlays: OverlaySeries[]): void {
    const ids = visible.map((s) => (s.primary ? `${s.id} (primary)` : s.id)).join(', ') || 'no visible curves';
    const names = [...new Set(overlays.map((o) => `“${o.name}”`))];
    const label =
      `${this.group.title} ${this.group.symbol} ${this.unitEl.textContent ?? ''} against frequency: ${ids}` +
      (names.length ? `; frozen baselines, drawn as thin patterned lines: ${names.join(', ')}` : '') +
      '. Arrow keys move the crosshair; values are read out above the plots and listed in the data table.';
    if (this.canvas.getAttribute('aria-label') !== label) this.canvas.setAttribute('aria-label', label);
  }

  plotRect(): { x0: number; x1: number; y0: number; y1: number } {
    return { x0: M.left, x1: this.cssW - M.right, y0: M.top, y1: this.cssH - M.bottom };
  }

  xOf(f: number, lo: number, hi: number): number {
    const r = this.plotRect();
    return r.x0 + ((Math.log(f) - Math.log(lo)) / (Math.log(hi) - Math.log(lo))) * (r.x1 - r.x0);
  }

  fOf(x: number, lo: number, hi: number): number {
    const r = this.plotRect();
    const t = (x - r.x0) / (r.x1 - r.x0);
    return Math.exp(Math.log(lo) + t * (Math.log(hi) - Math.log(lo)));
  }

  private axis(visible: Series[], overlays: OverlaySeries[], freqs: number[], lo: number, hi: number): Axis {
    const g = this.group;
    let vmin = Infinity;
    let vmax = -Infinity;
    const scan = (f: number[], values: (number | null)[]) => {
      for (let i = 0; i < f.length; i++) {
        if (f[i] < lo || f[i] > hi) continue;
        const v = values[i];
        if (!finite(v) || (g.scale === 'log' && v <= 0)) continue;
        vmin = Math.min(vmin, v);
        vmax = Math.max(vmax, v);
      }
    };
    for (const s of visible) scan(freqs, s.values);
    for (const o of overlays) scan(o.freqs, o.values);
    if (!(vmax >= vmin)) {
      vmin = g.scale === 'log' ? 1 : 0;
      vmax = g.scale === 'log' ? 10 : 1;
    }

    if (g.scale === 'log') {
      const e0 = Math.floor(Math.log10(vmin) + 1e-9);
      let e1 = Math.ceil(Math.log10(vmax) - 1e-9);
      if (e1 <= e0) e1 = e0 + 1;
      const a = 10 ** e0;
      const b = 10 ** e1;
      const { major, minor } = logTicks(a, b);
      const every = e1 - e0 > 8 ? 2 : 1;
      const ticks = major.filter((t) => {
        const e = Math.round(Math.log10(t));
        return Math.abs(t - 10 ** e) > 1e-9 * t || (e - e0) % every === 0;
      });
      const labels = ticks.map((t) => {
        const e = Math.floor(Math.log10(t) + 1e-9);
        const m = Math.round(t / 10 ** e);
        return m === 1 ? `10${superscript(e)}` : `${m}·10${superscript(e)}`;
      });
      // Minor (2..9) gridlines only while they stay readable.
      return { lo: a, hi: b, scale: 'log', ticks, minor: e1 - e0 <= 3 ? minor : [], labels, unitText: g.axisUnit };
    }

    // Linear axes.
    let mults = [1, 2, 2.5, 5];
    let target = 5;
    if (g.kind === 'spl') {
      mults = [1, 2, 5];
      if (vmax - vmin < 10) {
        const c = (vmax + vmin) / 2;
        vmin = c - 5;
        vmax = c + 5;
      }
    } else if (g.kind === 'delta') {
      // Differences: zero always in view, at least ±1 dB.
      mults = [1, 2, 5];
      target = 4;
      vmin = Math.min(vmin, -1, 0);
      vmax = Math.max(vmax, 1, 0);
    } else if (g.kind === 'phase') {
      mults = [1, 1.5, 3, 4.5, 9];
      target = 4;
      if (vmax - vmin < 10) {
        const c = (vmax + vmin) / 2;
        vmin = c - 5;
        vmax = c + 5;
      }
    } else {
      const pad = (vmax - vmin) * 0.05 || Math.abs(vmax) * 0.05 || 1;
      // Magnitudes are never negative: padding must not invent a negative tick.
      vmin = vmin >= 0 ? Math.max(0, vmin - pad) : vmin - pad;
      vmax += pad;
    }
    const { step } = niceTicks(vmin, vmax, target, mults);
    let a = Math.floor(vmin / step + 1e-9) * step;
    let b = Math.ceil(vmax / step - 1e-9) * step;
    if (g.kind === 'phase') {
      a = Math.max(a, -180);
      b = Math.min(b, 180);
    }
    const ticks: number[] = [];
    for (let t = a; t <= b + step * 1e-6; t += step) ticks.push(Math.abs(t) < step * 1e-9 ? 0 : t);

    let factor = 1;
    let unitText = g.axisUnit;
    const big = Math.max(Math.abs(a), Math.abs(b));
    if (g.kind === 'mag' && big > 0) {
      if (prefixable(g.unit)) {
        const [f, p] = prefixFor(big);
        factor = f;
        unitText = `${p}${prettyUnit(g.unit)}`;
      } else if (big < 1e-3 || big >= 1e5) {
        const e = 3 * Math.floor(Math.log10(big) / 3);
        factor = 10 ** e;
        unitText = `× 10${superscript(e)} ${g.axisUnit}`;
      }
    }
    const decimals = tickDecimals(step / factor);
    const labels = ticks.map((t) => (t / factor).toFixed(decimals).replace(/^-(0\.?0*)$/, '$1'));
    return { lo: a, hi: b, scale: 'linear', ticks, minor: [], labels, unitText };
  }

  private yOf(v: number, ax: Axis): number {
    const r = this.plotRect();
    const t =
      ax.scale === 'log'
        ? (Math.log(v) - Math.log(ax.lo)) / (Math.log(ax.hi) - Math.log(ax.lo))
        : (v - ax.lo) / (ax.hi - ax.lo);
    return r.y1 - t * (r.y1 - r.y0);
  }

  /** Last computed y-position of each visible series at the cursor (for tests). */
  lastCursorY: Record<string, number> = {};

  draw(opts: {
    data: PlotData;
    lo: number;
    hi: number;
    hidden: Set<string>;
    cursor: number | null;
    selection: [number, number] | null;
    theme: Theme;
  }): void {
    const { data, lo, hi, hidden, cursor, selection, theme: th } = opts;
    const dpr = window.devicePixelRatio || 1;
    const w = this.canvas.clientWidth;
    if (w === 0) return;
    this.cssW = w;
    if (this.canvas.width !== Math.round(w * dpr) || this.canvas.height !== Math.round(this.cssH * dpr)) {
      this.canvas.width = Math.round(w * dpr);
      this.canvas.height = Math.round(this.cssH * dpr);
    }
    const ctx = this.canvas.getContext('2d');
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const r = this.plotRect();
    const visible = this.group.series.filter((s) => !hidden.has(s.id));
    const overlays = this.group.overlays.filter((o) => !hidden.has(o.id));
    const ax = this.axis(visible, overlays, data.freqs, lo, hi);
    if (this.unitEl.textContent !== `(${ax.unitText})`) this.unitEl.textContent = `(${ax.unitText})`;
    this.describe(visible, overlays);

    ctx.fillStyle = th.bg;
    ctx.fillRect(0, 0, this.cssW, this.cssH);

    // Validity shading: light from begin_hz, darker from deep_hz, and on the
    // low side light below low_begin_hz, darker below low_deep_hz.
    const band = (f: number | null | undefined) =>
      f === null || f === undefined ? null : Math.min(Math.max(this.xOf(f, lo, hi), r.x0), r.x1);
    const xb = band(data.shading.begin_hz);
    const xd = band(data.shading.deep_hz);
    const xlb = band(data.shading.low_begin_hz);
    const xld = band(data.shading.low_deep_hz);
    if (xb !== null && xb < r.x1) {
      ctx.fillStyle = th.shade1;
      ctx.fillRect(xb, r.y0, (xd ?? r.x1) - xb, r.y1 - r.y0);
    }
    if (xd !== null && xd < r.x1) {
      ctx.fillStyle = th.shade2;
      ctx.fillRect(xd, r.y0, r.x1 - xd, r.y1 - r.y0);
    }
    if (xlb !== null && xlb > r.x0) {
      const from = xld !== null ? Math.max(xld, r.x0) : r.x0;
      ctx.fillStyle = th.shade1;
      if (xlb > from) ctx.fillRect(from, r.y0, xlb - from, r.y1 - r.y0);
    }
    if (xld !== null && xld > r.x0) {
      ctx.fillStyle = th.shade2;
      ctx.fillRect(r.x0, r.y0, xld - r.x0, r.y1 - r.y0);
    }
    // Band edges: at least 3:1 against both neighbouring fills (WCAG 1.4.11,
    // spec Section 15; the fills themselves are kept faint so curves stay
    // legible), dashed so they never read as the solid crosshair.
    ctx.lineWidth = 1;
    ctx.setLineDash([3, 3]);
    for (const x of [xb, xd, xlb, xld]) {
      if (x !== null && x > r.x0 && x < r.x1) {
        ctx.strokeStyle = th.shadeEdge;
        ctx.beginPath();
        ctx.moveTo(Math.round(x) + 0.5, r.y0);
        ctx.lineTo(Math.round(x) + 0.5, r.y1);
        ctx.stroke();
      }
    }
    ctx.setLineDash([]);

    // Highlighted range (a selected warning): tinted, with solid edges.
    const hl = data.highlight;
    let hlBox: [number, number] | null = null;
    if (hl && hl.hi >= lo && hl.lo <= hi) {
      const xa = Math.max(r.x0, this.xOf(hl.lo, lo, hi));
      const xz = Math.min(r.x1, this.xOf(hl.hi, lo, hi));
      const pad = xz - xa < 4 ? (4 - (xz - xa)) / 2 : 0;
      hlBox = [xa - pad, xz + pad];
      ctx.fillStyle = th.hlFill;
      ctx.fillRect(hlBox[0], r.y0, hlBox[1] - hlBox[0], r.y1 - r.y0);
      ctx.strokeStyle = th.hlEdge;
      ctx.lineWidth = 1.5;
      for (const x of hlBox) {
        if (x < r.x0 || x > r.x1) continue;
        ctx.beginPath();
        ctx.moveTo(x, r.y0);
        ctx.lineTo(x, r.y1);
        ctx.stroke();
      }
      ctx.lineWidth = 1;
    }

    // Grid.
    const ft = freqTicks(lo, hi);
    const vline = (x: number, color: string) => {
      ctx.strokeStyle = color;
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, r.y0);
      ctx.lineTo(Math.round(x) + 0.5, r.y1);
      ctx.stroke();
    };
    for (const f of ft.grid) vline(this.xOf(f, lo, hi), th.gridMinor);
    for (const f of ft.decades) vline(this.xOf(f, lo, hi), th.grid);
    const hline = (y: number, color: string) => {
      ctx.strokeStyle = color;
      ctx.beginPath();
      ctx.moveTo(r.x0, Math.round(y) + 0.5);
      ctx.lineTo(r.x1, Math.round(y) + 0.5);
      ctx.stroke();
    };
    for (const t of ax.minor) hline(this.yOf(t, ax), th.gridMinor);
    for (const t of ax.ticks) hline(this.yOf(t, ax), th.grid);

    // Axes.
    ctx.strokeStyle = th.axis;
    ctx.beginPath();
    ctx.moveTo(r.x0 + 0.5, r.y0);
    ctx.lineTo(r.x0 + 0.5, r.y1 + 0.5);
    ctx.lineTo(r.x1, r.y1 + 0.5);
    ctx.stroke();

    // Tick labels.
    ctx.font = `11px ${th.font}`;
    ctx.fillStyle = th.ink2;
    ctx.textBaseline = 'top';
    ctx.textAlign = 'center';
    let lastRight = -Infinity;
    for (const f of ft.labelled) {
      const x = this.xOf(f, lo, hi);
      const label = formatHzTick(f);
      const half = ctx.measureText(label).width / 2;
      if (x - half < lastRight + 6 || x + half > this.cssW - 2) continue;
      ctx.fillText(label, x, r.y1 + 5);
      lastRight = x + half;
    }
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    let lastY = Infinity;
    for (let i = 0; i < ax.ticks.length; i++) {
      const y = this.yOf(ax.ticks[i], ax);
      if (lastY - y < 13) continue;
      ctx.fillText(ax.labels[i], r.x0 - 6, y);
      lastY = y;
    }
    // Frequency unit at the far left of the tick row, clear of the first label.
    ctx.textAlign = 'left';
    ctx.textBaseline = 'top';
    ctx.fillText('Hz', 4, r.y1 + 5);

    // Band labels on the first plot, so the shading is not colour-only.
    if (this.first) {
      ctx.textAlign = 'left';
      ctx.textBaseline = 'top';
      const tag = (x: number | null, text: string, limit: number) => {
        if (x === null || x >= r.x1 - 20) return;
        const tw = ctx.measureText(text).width;
        if (x + 8 + tw > limit) return;
        ctx.fillStyle = th.bg;
        ctx.globalAlpha = 0.85;
        ctx.fillRect(x + 2, r.y0 + 2, tw + 6, 15);
        ctx.globalAlpha = 1;
        ctx.fillStyle = th.ink2;
        ctx.fillText(text, x + 5, r.y0 + 4);
      };
      tag(xb, '≥10 %', xd ?? r.x1);
      tag(xd, '≥36 %', r.x1);
      // Low side: the tag sits at the left edge of the band.
      if (xlb !== null && xlb > r.x0 + 20) tag(r.x0, 'below validated range', xlb);
    }
    // The highlighted range's label, on every plot (it may be the only one in view).
    if (hl && hlBox) {
      ctx.font = `11px ${th.font}`;
      ctx.textBaseline = 'bottom';
      const tw = ctx.measureText(hl.label).width;
      let x = hlBox[0] + 3;
      if (x + tw + 6 > r.x1) x = Math.max(r.x0 + 2, hlBox[1] - tw - 9);
      ctx.fillStyle = th.bg;
      ctx.globalAlpha = 0.9;
      ctx.fillRect(x - 2, r.y1 - 17, tw + 6, 15);
      ctx.globalAlpha = 1;
      ctx.fillStyle = th.ink;
      ctx.textAlign = 'left';
      ctx.fillText(hl.label, x + 1, r.y1 - 3);
    }

    // Difference plots: a solid zero line.
    if (this.group.kind === 'delta') {
      const y0 = Math.round(this.yOf(0, ax)) + 0.5;
      ctx.strokeStyle = th.axis;
      ctx.beginPath();
      ctx.moveTo(r.x0, y0);
      ctx.lineTo(r.x1, y0);
      ctx.stroke();
    }

    // Curves, each over a surface-coloured casing so crossings stay legible.
    ctx.save();
    ctx.beginPath();
    ctx.rect(r.x0, r.y0 - 1, r.x1 - r.x0, r.y1 - r.y0 + 2);
    ctx.clip();
    const f = data.freqs;
    const range = (g: number[]): [number, number] => {
      let a = 0;
      while (a < g.length - 1 && g[a + 1] < lo) a++;
      let b = g.length - 1;
      while (b > 0 && g[b - 1] > hi) b--;
      return [a, b];
    };
    const [i0, i1] = range(f);
    const trace = (g: number[], values: (number | null)[], a: number, b: number) => {
      const out: ([number, number] | null)[] = [];
      for (let i = a; i <= b; i++) {
        const v = values[i];
        out.push(finite(v) && (ax.scale !== 'log' || v > 0) ? [this.xOf(g[i], lo, hi), this.yOf(v, ax)] : null);
      }
      return out;
    };
    const pts = (s: Series) => trace(f, s.values, i0, i1);
    const tracePath = (p: ([number, number] | null)[]) => {
      ctx.beginPath();
      let pen = false;
      for (const q of p) {
        if (!q) {
          pen = false;
          continue;
        }
        if (pen) ctx.lineTo(q[0], q[1]);
        else ctx.moveTo(q[0], q[1]);
        pen = true;
      }
    };
    ctx.lineJoin = 'round';
    // Baselines first, thin and patterned, so live curves stay on top.
    for (const o of overlays) {
      const [a, b] = range(o.freqs);
      tracePath(trace(o.freqs, o.values, a, b));
      ctx.setLineDash(OVERLAY_DASHES[o.slot % OVERLAY_DASHES.length]);
      ctx.lineCap = 'butt';
      ctx.strokeStyle = mixHex(th.series[styleSlot(o.probe).color], th.ink2, OVERLAY_MIX);
      ctx.lineWidth = OVERLAY_WIDTH;
      ctx.stroke();
    }
    // The primary curve last, so it is never covered.
    const order = [...visible].sort((a, b) => Number(!!a.primary) - Number(!!b.primary));
    for (const s of order) {
      const st = styleSlot(s.probe);
      const p = pts(s);
      const w = s.primary ? PRIMARY_WIDTH : LIVE_WIDTH;
      tracePath(p);
      ctx.setLineDash([]);
      ctx.lineCap = 'round';
      ctx.strokeStyle = th.bg;
      ctx.lineWidth = w + 3;
      ctx.stroke();
      ctx.setLineDash(DASHES[st.dash]);
      ctx.lineCap = 'butt';
      ctx.strokeStyle = th.series[st.color];
      ctx.lineWidth = w;
      ctx.stroke();
    }
    ctx.setLineDash([]);

    // Direct labels at the right end for 2-4 curves, unless they collide.
    if (visible.length >= 2 && visible.length <= 4) {
      const ends: { id: string; x: number; y: number }[] = [];
      for (const s of visible) {
        const p = pts(s);
        let q: [number, number] | null = null;
        for (let k = p.length - 1; k >= 0 && !q; k--) if (p[k] && p[k]![0] <= r.x1) q = p[k];
        if (q) ends.push({ id: s.id, x: q[0], y: q[1] });
      }
      const ys = ends.map((e) => e.y).sort((a, b) => a - b);
      const clear = ys.every((y, k) => k === 0 || y - ys[k - 1] >= 14);
      if (clear) {
        ctx.font = `11px ${th.font}`;
        ctx.textAlign = 'right';
        ctx.textBaseline = 'bottom';
        for (const e of ends) {
          const tw = ctx.measureText(e.id).width;
          const x = Math.min(e.x, r.x1) - 4;
          const y = Math.max(e.y - 3, r.y0 + 14);
          ctx.fillStyle = th.bg;
          ctx.globalAlpha = 0.85;
          ctx.fillRect(x - tw - 3, y - 13, tw + 6, 14);
          ctx.globalAlpha = 1;
          ctx.fillStyle = th.ink;
          ctx.fillText(e.id, x, y);
        }
      }
    }
    ctx.restore();

    // Drag-to-zoom selection.
    if (selection) {
      const xa = Math.max(r.x0, Math.min(r.x1, this.xOf(selection[0], lo, hi)));
      const xz = Math.max(r.x0, Math.min(r.x1, this.xOf(selection[1], lo, hi)));
      ctx.fillStyle = th.selection;
      ctx.fillRect(Math.min(xa, xz), r.y0, Math.abs(xz - xa), r.y1 - r.y0);
    }

    // Crosshair with a marker on every visible curve.
    this.lastCursorY = {};
    if (cursor !== null && cursor < f.length && f[cursor] >= lo && f[cursor] <= hi) {
      const x = this.xOf(f[cursor], lo, hi);
      ctx.strokeStyle = th.crosshair;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, r.y0);
      ctx.lineTo(Math.round(x) + 0.5, r.y1);
      ctx.stroke();
      for (const s of visible) {
        const v = s.values[cursor];
        if (!finite(v) || (ax.scale === 'log' && v <= 0)) continue;
        const y = this.yOf(v, ax);
        if (y < r.y0 - 1 || y > r.y1 + 1) continue;
        this.lastCursorY[s.id] = y;
        ctx.beginPath();
        ctx.arc(x, y, 4, 0, Math.PI * 2);
        ctx.fillStyle = th.series[styleSlot(s.probe).color];
        ctx.strokeStyle = th.bg;
        ctx.lineWidth = 2;
        ctx.fill();
        ctx.stroke();
      }
    }
  }
}

export interface PanelCallbacks {
  onCursor(index: number | null, fromKeyboard: boolean): void;
  onView(lo: number, hi: number): void;
}

/** A stack of plots sharing view, visibility and crosshair state. */
export class PlotPanel {
  plots: Plot[] = [];
  data: PlotData | null = null;
  lo = RANGE_AUDIO[0];
  hi = RANGE_AUDIO[1];
  hidden = new Set<string>();
  cursor: number | null = null;
  private selection: [number, number] | null = null;
  private drag: { x0: number; plot: Plot; id: number } | null = null;
  private frame = 0;
  private theme: Theme;

  private readonly resize: ResizeObserver;
  private readonly themeAttr: MutationObserver;
  private readonly scheme: MediaQueryList;
  private readonly onScheme = () => {
    this.theme = readTheme();
    this.render();
  };

  constructor(
    private readonly container: HTMLElement,
    private readonly cb: PanelCallbacks,
  ) {
    this.theme = readTheme();
    this.resize = new ResizeObserver(() => this.render());
    this.resize.observe(container);
    this.themeAttr = new MutationObserver(this.onScheme);
    this.themeAttr.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    this.scheme = window.matchMedia('(prefers-color-scheme: dark)');
    this.scheme.addEventListener('change', this.onScheme);
  }

  /**
   * Stops observing the page (size, theme). A panel whose plots are removed
   * for good must be disposed: the theme observers are held by the document
   * and would otherwise keep the panel, its data and its canvases alive.
   */
  dispose(): void {
    this.resize.disconnect();
    this.themeAttr.disconnect();
    this.scheme.removeEventListener('change', this.onScheme);
    if (this.frame) cancelAnimationFrame(this.frame);
    this.frame = 0;
    this.container.replaceChildren();
    this.plots = [];
    this.data = null;
  }

  /** Allowed view range: the internal range, widened to the data if needed. */
  bounds(): [number, number] {
    const f = this.data?.freqs ?? [];
    return [Math.min(RANGE_FULL[0], f[0] ?? Infinity), Math.max(RANGE_FULL[1], f[f.length - 1] ?? 0)];
  }

  setGroups(groups: PlotGroup[], data: PlotData): void {
    this.data = data;
    this.container.replaceChildren();
    this.plots = groups.map((g, k) => {
      const p = new Plot(g, k === 0);
      this.container.append(p.figure);
      this.attach(p);
      return p;
    });
    if (this.cursor !== null && this.cursor >= data.freqs.length) this.cursor = null;
    this.render();
  }

  /** Picks out a frequency range on every plot, widening the view to show it (null clears). */
  setHighlight(h: Highlight | null): void {
    if (!this.data) return;
    this.data.highlight = h;
    if (h && (h.lo < this.lo || h.hi > this.hi)) {
      const [bmin, bmax] = this.bounds();
      const lo = h.lo < this.lo ? h.lo / 1.25 : this.lo;
      const hi = h.hi > this.hi ? h.hi * 1.25 : this.hi;
      this.setView(Math.max(bmin, lo), Math.min(bmax, hi));
    } else {
      this.render();
    }
  }

  setView(lo: number, hi: number): void {
    const [bmin, bmax] = this.bounds();
    let a = Math.max(lo, bmin);
    let b = Math.min(hi, bmax);
    const minRatio = 1.1;
    if (b / a < minRatio) {
      const c = Math.sqrt(a * b);
      a = Math.max(bmin, c / Math.sqrt(minRatio));
      b = Math.min(bmax, a * minRatio);
    }
    this.lo = a;
    this.hi = b;
    this.cb.onView(a, b);
    this.render();
  }

  zoom(factor: number, around?: number): void {
    const c = around ?? (this.cursor !== null && this.data ? this.data.freqs[this.cursor] : Math.sqrt(this.lo * this.hi));
    const lc = Math.log(Math.min(Math.max(c, this.lo), this.hi));
    this.setView(Math.exp(lc - (lc - Math.log(this.lo)) * factor), Math.exp(lc + (Math.log(this.hi) - lc) * factor));
  }

  pan(fraction: number): void {
    const span = Math.log(this.hi / this.lo);
    const [bmin, bmax] = this.bounds();
    let a = Math.log(this.lo) + fraction * span;
    a = Math.min(Math.max(a, Math.log(bmin)), Math.log(bmax) - span);
    this.setView(Math.exp(a), Math.exp(a + span));
  }

  setCursor(i: number | null, fromKeyboard = false): void {
    this.cursor = i;
    if (i !== null && this.data) {
      const f = this.data.freqs[i];
      if (f < this.lo || f > this.hi) {
        const span = Math.log(this.hi / this.lo);
        const a = f < this.lo ? Math.log(f) - 0.05 * span : Math.log(f) - 0.95 * span;
        this.setView(Math.exp(a), Math.exp(a + span));
      }
    }
    this.cb.onCursor(i, fromKeyboard);
    this.render();
  }

  /** Indices of the first and last data points inside the view. */
  viewIndices(): [number, number] | null {
    const f = this.data?.freqs;
    if (!f?.length) return null;
    let a = 0;
    while (a < f.length && f[a] < this.lo * (1 - 1e-9)) a++;
    let b = f.length - 1;
    while (b >= 0 && f[b] > this.hi * (1 + 1e-9)) b--;
    return a <= b ? [a, b] : null;
  }

  render(): void {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.renderNow();
    });
  }

  renderNow(): void {
    if (!this.data) return;
    for (const p of this.plots) {
      p.draw({
        data: this.data,
        lo: this.lo,
        hi: this.hi,
        hidden: this.hidden,
        cursor: this.cursor,
        selection: this.selection,
        theme: this.theme,
      });
    }
  }

  private localX(p: Plot, ev: PointerEvent | MouseEvent): number {
    return ev.clientX - p.canvas.getBoundingClientRect().left;
  }

  private attach(p: Plot): void {
    const c = p.canvas;
    c.addEventListener('pointermove', (ev) => {
      if (!this.data?.freqs.length) return;
      const x = this.localX(p, ev);
      const f = p.fOf(x, this.lo, this.hi);
      if (this.drag && this.drag.id === ev.pointerId) {
        if (Math.abs(x - this.drag.x0) > 4) {
          this.selection = [p.fOf(this.drag.x0, this.lo, this.hi), f];
        }
      }
      const r = p.plotRect();
      if (x >= r.x0 && x <= r.x1) {
        const i = nearestIndex(this.data.freqs, f);
        if (i !== this.cursor || this.selection) {
          this.cursor = i;
          this.cb.onCursor(i, false);
        }
      }
      this.render();
    });
    c.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0) return;
      this.drag = { x0: this.localX(p, ev), plot: p, id: ev.pointerId };
      c.setPointerCapture(ev.pointerId);
    });
    const end = (ev: PointerEvent) => {
      if (!this.drag || this.drag.id !== ev.pointerId) return;
      const sel = this.selection;
      this.drag = null;
      this.selection = null;
      if (sel) {
        const [a, b] = sel[0] < sel[1] ? sel : [sel[1], sel[0]];
        if (b / a > 1.05) this.setView(a, b);
      }
      this.render();
    };
    c.addEventListener('pointerup', end);
    c.addEventListener('pointercancel', end);
    c.addEventListener('dblclick', () => this.setView(...RANGE_AUDIO));
    c.addEventListener('keydown', (ev) => this.onKey(ev));
  }

  private onKey(ev: KeyboardEvent): void {
    const f = this.data?.freqs;
    if (!f?.length) return;
    const vi = this.viewIndices();
    const start = () => {
      if (this.cursor !== null) return this.cursor;
      return nearestIndex(f, Math.sqrt(this.lo * this.hi));
    };
    let handled = true;
    switch (ev.key) {
      case 'ArrowRight':
      case 'ArrowLeft': {
        const dir = ev.key === 'ArrowRight' ? 1 : -1;
        if (ev.ctrlKey || ev.metaKey) {
          this.pan(0.25 * dir);
        } else {
          const step = ev.shiftKey ? 10 : 1;
          const i = this.cursor === null ? start() : Math.min(f.length - 1, Math.max(0, start() + dir * step));
          this.setCursor(i, true);
        }
        break;
      }
      case 'Home':
        if (vi) this.setCursor(vi[0], true);
        break;
      case 'End':
        if (vi) this.setCursor(vi[1], true);
        break;
      case '+':
      case '=':
        this.zoom(0.5);
        break;
      case '-':
      case '_':
        this.zoom(2);
        break;
      case '0':
        this.setView(...RANGE_AUDIO);
        break;
      case 'Escape':
        this.setCursor(null, true);
        break;
      default:
        handled = false;
    }
    if (handled) ev.preventDefault();
  }
}
