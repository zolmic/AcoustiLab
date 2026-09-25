// Linear-time plots for the Time view: impulse, step and energy-time curves
// of a uniformly sampled buffer (sample m at t0 + m·dt). The look and the
// controls follow plot.ts (theme tokens, grid, casing under each curve,
// crosshair with markers, keyboard, drag to zoom, double-click to reset);
// only the x axis differs: linear time in milliseconds.
//
// A buffer has up to 16 384 samples, far more than a plot has pixels, so a
// dense view is drawn as the minimum and maximum of each pixel column,
// which keeps every oscillation's envelope.

import { niceTicks, prefixable, prefixFor, prettyUnit, superscript } from '../format';
import { readTheme, type Theme } from '../plot';
import { DASHES, LIVE_WIDTH, PRIMARY_WIDTH, styleSlot } from '../series';
import { Figure, formatSeconds, type AxisPanel, type FigPlot, type FigureOptions } from './measure-kit';

/** A uniformly sampled time axis. */
export interface TimeData {
  t0: number;
  dt: number;
  n: number;
}

const M = { left: 64, right: 16, top: 10, bottom: 26 };
const HEIGHT = { main: 200, small: 140 };
/** Lowest level of an energy-time axis below its top (the engine floors at −300 dB). */
const DB_RANGE = 120;

interface YAxis {
  lo: number;
  hi: number;
  ticks: number[];
  labels: string[];
  unitText: string;
}

function finite(v: number | null | undefined): v is number {
  return v !== null && v !== undefined && Number.isFinite(v);
}

class TimePlot {
  readonly figure: HTMLElement;
  readonly canvas: HTMLCanvasElement;
  private readonly unitEl: HTMLElement;
  private cssW = 0;
  private readonly cssH: number;
  /** Last y of each visible series at the crosshair (for tests). */
  lastCursorY: Record<string, number> = {};

  constructor(readonly plot: FigPlot) {
    this.figure = document.createElement('figure');
    this.figure.className = `plot plot-${plot.height ?? 'main'} mv-time-plot`;
    this.figure.dataset.group = plot.key;
    const caption = document.createElement('figcaption');
    const title = document.createElement('span');
    title.className = 'plot-title';
    title.textContent = `${plot.title} ${plot.symbol}`;
    this.unitEl = document.createElement('span');
    this.unitEl.className = 'plot-unit';
    caption.append(title, ' ', this.unitEl);
    this.canvas = document.createElement('canvas');
    this.canvas.tabIndex = 0;
    this.canvas.setAttribute('role', 'img');
    this.cssH = HEIGHT[plot.height ?? 'main'];
    this.canvas.style.height = `${this.cssH}px`;
    this.figure.append(caption, this.canvas);
  }

  plotRect(): { x0: number; x1: number; y0: number; y1: number } {
    return { x0: M.left, x1: this.cssW - M.right, y0: M.top, y1: this.cssH - M.bottom };
  }

  xOf(t: number, lo: number, hi: number): number {
    const r = this.plotRect();
    return r.x0 + ((t - lo) / (hi - lo)) * (r.x1 - r.x0);
  }

  tOf(x: number, lo: number, hi: number): number {
    const r = this.plotRect();
    return lo + ((x - r.x0) / (r.x1 - r.x0)) * (hi - lo);
  }

  private describe(ids: string[]): void {
    const label =
      `${this.plot.title} ${this.plot.symbol} ${this.unitEl.textContent ?? ''} against time: ${ids.join(', ') || 'no visible curves'}. ` +
      'Arrow keys move the crosshair; values are read out above the plots and listed in the data table.';
    if (this.canvas.getAttribute('aria-label') !== label) this.canvas.setAttribute('aria-label', label);
  }

  private axis(visible: FigPlot['series'], i0: number, i1: number): YAxis {
    let vmin = Infinity;
    let vmax = -Infinity;
    for (const s of visible) {
      for (let i = i0; i <= i1; i++) {
        const v = s.values[i];
        if (!finite(v)) continue;
        if (v < vmin) vmin = v;
        if (v > vmax) vmax = v;
      }
    }
    if (!(vmax >= vmin)) {
      vmin = this.plot.kind === 'spl' ? -60 : -1;
      vmax = this.plot.kind === 'spl' ? 0 : 1;
    }
    if (this.plot.kind === 'spl') {
      // Levels in dB re the maximum: the top at the largest level in view,
      // at most DB_RANGE below it.
      const hi = Math.ceil(vmax / 10 - 1e-9) * 10;
      const lo = Math.max(Math.floor(vmin / 10 + 1e-9) * 10, hi - DB_RANGE);
      const { ticks, step } = niceTicks(lo, hi === lo ? lo + 10 : hi, 5, [1, 2, 5]);
      const a = Math.floor(lo / step + 1e-9) * step;
      const b = Math.max(Math.ceil(hi / step - 1e-9) * step, a + step);
      const all = ticks.filter((t) => t >= a - 1e-9 && t <= b + 1e-9);
      return { lo: a, hi: b, ticks: all, labels: all.map((t) => (t === 0 ? '0' : `${t < 0 ? '−' : ''}${Math.abs(t)}`)), unitText: this.plot.axisUnit };
    }
    // Amplitudes: zero always in view, 5 % padding.
    vmin = Math.min(vmin, 0);
    vmax = Math.max(vmax, 0);
    const pad = (vmax - vmin) * 0.05 || Math.abs(vmax) * 0.05 || 1e-12;
    vmin -= vmin < 0 ? pad : 0;
    vmax += vmax > 0 ? pad : 0;
    const { step } = niceTicks(vmin, vmax, 5, [1, 2, 2.5, 5]);
    const a = Math.floor(vmin / step + 1e-9) * step;
    const b = Math.ceil(vmax / step - 1e-9) * step;
    const ticks: number[] = [];
    for (let t = a; t <= b + step * 1e-6; t += step) ticks.push(Math.abs(t) < step * 1e-9 ? 0 : t);
    const big = Math.max(Math.abs(a), Math.abs(b));
    let factor = 1;
    let unitText = this.plot.axisUnit;
    if (big > 0 && this.plot.unit) {
      if (prefixable(this.plot.unit)) {
        const [f, p] = prefixFor(big);
        factor = f;
        unitText = `${p}${prettyUnit(this.plot.unit)}${this.plot.axisUnit.slice(prettyUnit(this.plot.unit).length)}`;
      } else if (big < 1e-3 || big >= 1e5) {
        const e = 3 * Math.floor(Math.log10(big) / 3);
        factor = 10 ** e;
        unitText = `× 10${superscript(e)} ${this.plot.axisUnit}`;
      }
    }
    const decimals = Math.max(0, -Math.floor(Math.log10(step / factor) + 1e-9));
    const labels = ticks.map((t) => (t / factor).toFixed(decimals).replace(/^-(0\.?0*)$/, '$1').replace(/^-/, '−'));
    return { lo: a, hi: b, ticks, labels, unitText };
  }

  private yOf(v: number, ax: YAxis): number {
    const r = this.plotRect();
    return r.y1 - ((v - ax.lo) / (ax.hi - ax.lo)) * (r.y1 - r.y0);
  }

  draw(o: { data: TimeData; lo: number; hi: number; hidden: Set<string>; cursor: number | null; selection: [number, number] | null; theme: Theme }): void {
    const { data, lo, hi, hidden, cursor, selection, theme: th } = o;
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
    const visible = this.plot.series.filter((s) => !hidden.has(s.id));
    const i0 = Math.max(0, Math.floor((lo - data.t0) / data.dt));
    const i1 = Math.min(data.n - 1, Math.ceil((hi - data.t0) / data.dt));
    const ax = this.axis(visible, i0, i1);
    if (this.unitEl.textContent !== `(${ax.unitText})`) this.unitEl.textContent = `(${ax.unitText})`;
    this.describe(visible.map((s) => s.id));

    ctx.fillStyle = th.bg;
    ctx.fillRect(0, 0, this.cssW, this.cssH);

    // Grid: time ticks every "nice" number of milliseconds.
    const target = Math.max(2, Math.min(10, Math.floor((r.x1 - r.x0) / 70)));
    const { ticks: tt, step: tstep } = niceTicks(lo * 1e3, hi * 1e3, target, [1, 2, 2.5, 5]);
    ctx.lineWidth = 1;
    const vline = (x: number, color: string) => {
      ctx.strokeStyle = color;
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, r.y0);
      ctx.lineTo(Math.round(x) + 0.5, r.y1);
      ctx.stroke();
    };
    for (const t of tt) vline(this.xOf(t / 1e3, lo, hi), Math.abs(t) < tstep * 1e-9 ? th.axis : th.grid);
    for (const t of ax.ticks) {
      const y = Math.round(this.yOf(t, ax)) + 0.5;
      ctx.strokeStyle = t === 0 && this.plot.kind !== 'spl' ? th.axis : th.grid;
      ctx.beginPath();
      ctx.moveTo(r.x0, y);
      ctx.lineTo(r.x1, y);
      ctx.stroke();
    }
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
    const decimals = Math.max(0, -Math.floor(Math.log10(tstep) + 1e-9));
    let lastRight = -Infinity;
    for (const t of tt) {
      const x = this.xOf(t / 1e3, lo, hi);
      const label = (Math.abs(t) < tstep * 1e-9 ? 0 : t).toFixed(decimals).replace(/^-/, '−');
      const half = ctx.measureText(label).width / 2;
      if (x - half < lastRight + 6 || x + half > this.cssW - 2 || x - half < 24) continue;
      ctx.fillText(label, x, r.y1 + 5);
      lastRight = x + half;
    }
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    let lastY = Infinity;
    for (let k = 0; k < ax.ticks.length; k++) {
      const y = this.yOf(ax.ticks[k], ax);
      if (lastY - y < 13) continue;
      ctx.fillText(ax.labels[k], r.x0 - 6, y);
      lastY = y;
    }
    ctx.textAlign = 'left';
    ctx.textBaseline = 'top';
    ctx.fillText('ms', 4, r.y1 + 5);

    // Curves (dense views: min and max per pixel column), each over a
    // surface-coloured casing, the primary last.
    ctx.save();
    ctx.beginPath();
    ctx.rect(r.x0, r.y0 - 1, r.x1 - r.x0, r.y1 - r.y0 + 2);
    ctx.clip();
    const yClamp = (v: number) => Math.max(r.y0 - 2, Math.min(r.y1 + 2, this.yOf(v, ax)));
    const path = (values: ArrayLike<number | null>) => {
      ctx.beginPath();
      const dense = i1 - i0 > 2 * (r.x1 - r.x0);
      let pen = false;
      if (!dense) {
        for (let i = i0; i <= i1; i++) {
          const v = values[i];
          if (!finite(v)) {
            pen = false;
            continue;
          }
          const x = this.xOf(data.t0 + i * data.dt, lo, hi);
          if (pen) ctx.lineTo(x, yClamp(v));
          else ctx.moveTo(x, yClamp(v));
          pen = true;
        }
        return;
      }
      let col = NaN;
      let mn = 0;
      let mx = 0;
      let first = 0;
      let last = 0;
      const flush = () => {
        if (Number.isNaN(col)) return;
        const x = col + 0.5;
        if (pen) ctx.lineTo(x, yClamp(first));
        else ctx.moveTo(x, yClamp(first));
        ctx.lineTo(x, yClamp(mn));
        ctx.lineTo(x, yClamp(mx));
        ctx.lineTo(x, yClamp(last));
        pen = true;
      };
      for (let i = i0; i <= i1; i++) {
        const v = values[i];
        if (!finite(v)) {
          flush();
          col = NaN;
          pen = false;
          continue;
        }
        const c = Math.floor(this.xOf(data.t0 + i * data.dt, lo, hi));
        if (c !== col) {
          flush();
          col = c;
          mn = mx = first = last = v;
        } else {
          if (v < mn) mn = v;
          if (v > mx) mx = v;
          last = v;
        }
      }
      flush();
    };
    ctx.lineJoin = 'round';
    const order = [...visible].sort((a, b) => Number(!!a.primary) - Number(!!b.primary));
    for (const s of order) {
      const st = styleSlot(s.slot);
      const lw = s.primary ? PRIMARY_WIDTH : LIVE_WIDTH;
      path(s.values);
      ctx.setLineDash([]);
      ctx.lineCap = 'round';
      ctx.strokeStyle = th.bg;
      ctx.lineWidth = lw + 3;
      ctx.stroke();
      ctx.setLineDash(DASHES[st.dash]);
      ctx.lineCap = 'butt';
      ctx.strokeStyle = th.series[st.color];
      ctx.lineWidth = lw;
      ctx.stroke();
    }
    ctx.setLineDash([]);
    ctx.restore();

    if (selection) {
      const xa = Math.max(r.x0, Math.min(r.x1, this.xOf(selection[0], lo, hi)));
      const xz = Math.max(r.x0, Math.min(r.x1, this.xOf(selection[1], lo, hi)));
      ctx.fillStyle = th.selection;
      ctx.fillRect(Math.min(xa, xz), r.y0, Math.abs(xz - xa), r.y1 - r.y0);
    }

    this.lastCursorY = {};
    if (cursor !== null && cursor < data.n) {
      const t = data.t0 + cursor * data.dt;
      if (t >= lo && t <= hi) {
        const x = this.xOf(t, lo, hi);
        ctx.strokeStyle = th.crosshair;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(Math.round(x) + 0.5, r.y0);
        ctx.lineTo(Math.round(x) + 0.5, r.y1);
        ctx.stroke();
        for (const s of visible) {
          const v = s.values[cursor];
          if (!finite(v)) continue;
          const y = this.yOf(v, ax);
          if (y < r.y0 - 1 || y > r.y1 + 1) continue;
          this.lastCursorY[s.id] = y;
          ctx.beginPath();
          ctx.arc(x, y, 4, 0, Math.PI * 2);
          ctx.fillStyle = th.series[styleSlot(s.slot).color];
          ctx.strokeStyle = th.bg;
          ctx.lineWidth = 2;
          ctx.fill();
          ctx.stroke();
        }
      }
    }
  }
}

/** A stack of time plots sharing view, visibility and crosshair (as plot.ts `PlotPanel`). */
export class TimePanel implements AxisPanel {
  plots: TimePlot[] = [];
  data: TimeData | null = null;
  lo = 0;
  hi = 0.01;
  hidden = new Set<string>();
  cursor: number | null = null;
  private selection: [number, number] | null = null;
  private drag: { x0: number; id: number } | null = null;
  private frame = 0;
  private theme: Theme;

  constructor(
    private readonly container: HTMLElement,
    private readonly cb: { onCursor(i: number | null, fromKeyboard: boolean): void; onView(lo: number, hi: number): void },
    private readonly defaultView: () => [number, number],
  ) {
    this.theme = readTheme();
    new ResizeObserver(() => this.render()).observe(container);
    new MutationObserver(() => {
      this.theme = readTheme();
      this.render();
    }).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
      this.theme = readTheme();
      this.render();
    });
  }

  bounds(): [number, number] {
    const d = this.data;
    return d ? [d.t0, d.t0 + (d.n - 1) * d.dt] : [0, 0.01];
  }

  setGroups(plots: FigPlot[], data: TimeData): void {
    const fresh = !this.data || this.data.t0 !== data.t0 || this.data.dt !== data.dt || this.data.n !== data.n;
    this.data = data;
    this.container.replaceChildren();
    this.plots = plots.map((p) => {
      const tp = new TimePlot(p);
      this.container.append(tp.figure);
      this.attach(tp);
      return tp;
    });
    if (this.cursor !== null && this.cursor >= data.n) this.cursor = null;
    if (fresh) this.setView(...this.defaultView());
    else this.setView(this.lo, this.hi);
  }

  setView(lo: number, hi: number): void {
    const [bmin, bmax] = this.bounds();
    const dt = this.data?.dt ?? 1e-5;
    let a = Math.max(lo, bmin);
    let b = Math.min(hi, bmax);
    const minSpan = 8 * dt;
    if (b - a < minSpan) {
      const c = (a + b) / 2;
      a = Math.max(bmin, c - minSpan / 2);
      b = Math.min(bmax, a + minSpan);
    }
    this.lo = a;
    this.hi = b;
    this.cb.onView(a, b);
    this.render();
  }

  zoom(factor: number): void {
    const d = this.data;
    const c = this.cursor !== null && d ? d.t0 + this.cursor * d.dt : (this.lo + this.hi) / 2;
    const cc = Math.min(Math.max(c, this.lo), this.hi);
    this.setView(cc - (cc - this.lo) * factor, cc + (this.hi - cc) * factor);
  }

  pan(fraction: number): void {
    const span = this.hi - this.lo;
    const [bmin, bmax] = this.bounds();
    const a = Math.min(Math.max(this.lo + fraction * span, bmin), bmax - span);
    this.setView(a, a + span);
  }

  setCursor(i: number | null, fromKeyboard = false): void {
    this.cursor = i;
    const d = this.data;
    if (i !== null && d) {
      const t = d.t0 + i * d.dt;
      if (t < this.lo || t > this.hi) {
        const span = this.hi - this.lo;
        const a = t < this.lo ? t - 0.05 * span : t - 0.95 * span;
        this.setView(a, a + span);
      }
    }
    this.cb.onCursor(i, fromKeyboard);
    this.render();
  }

  viewIndices(): [number, number] | null {
    const d = this.data;
    if (!d || !d.n) return null;
    const a = Math.max(0, Math.ceil((this.lo - d.t0) / d.dt - 1e-9));
    const b = Math.min(d.n - 1, Math.floor((this.hi - d.t0) / d.dt + 1e-9));
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
    const d = this.data;
    if (!d) return;
    for (const p of this.plots) {
      p.draw({ data: d, lo: this.lo, hi: this.hi, hidden: this.hidden, cursor: this.cursor, selection: this.selection, theme: this.theme });
    }
  }

  private index(t: number): number {
    const d = this.data!;
    return Math.min(d.n - 1, Math.max(0, Math.round((t - d.t0) / d.dt)));
  }

  private attach(p: TimePlot): void {
    const c = p.canvas;
    const localX = (ev: PointerEvent | MouseEvent) => ev.clientX - c.getBoundingClientRect().left;
    c.addEventListener('pointermove', (ev) => {
      if (!this.data) return;
      const x = localX(ev);
      const t = p.tOf(x, this.lo, this.hi);
      if (this.drag && this.drag.id === ev.pointerId && Math.abs(x - this.drag.x0) > 4) {
        this.selection = [p.tOf(this.drag.x0, this.lo, this.hi), t];
      }
      const r = p.plotRect();
      if (x >= r.x0 && x <= r.x1) {
        const i = this.index(t);
        if (i !== this.cursor || this.selection) {
          this.cursor = i;
          this.cb.onCursor(i, false);
        }
      }
      this.render();
    });
    c.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0) return;
      this.drag = { x0: localX(ev), id: ev.pointerId };
      c.setPointerCapture(ev.pointerId);
    });
    const end = (ev: PointerEvent) => {
      if (!this.drag || this.drag.id !== ev.pointerId) return;
      const sel = this.selection;
      this.drag = null;
      this.selection = null;
      if (sel) {
        const [a, b] = sel[0] < sel[1] ? sel : [sel[1], sel[0]];
        if (b - a > 2 * (this.data?.dt ?? 0)) this.setView(a, b);
      }
      this.render();
    };
    c.addEventListener('pointerup', end);
    c.addEventListener('pointercancel', end);
    c.addEventListener('dblclick', () => this.setView(...this.defaultView()));
    c.addEventListener('keydown', (ev) => this.onKey(ev));
  }

  private onKey(ev: KeyboardEvent): void {
    const d = this.data;
    if (!d) return;
    const vi = this.viewIndices();
    const start = () => (this.cursor !== null ? this.cursor : this.index((this.lo + this.hi) / 2));
    let handled = true;
    switch (ev.key) {
      case 'ArrowRight':
      case 'ArrowLeft': {
        const dir = ev.key === 'ArrowRight' ? 1 : -1;
        if (ev.ctrlKey || ev.metaKey) this.pan(0.25 * dir);
        else {
          const step = ev.shiftKey ? 10 : 1;
          const i = this.cursor === null ? start() : Math.min(d.n - 1, Math.max(0, start() + dir * step));
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
        this.setView(...this.defaultView());
        break;
      case 'Escape':
        this.setCursor(null, true);
        break;
      default:
        handled = false;
    }
    if (handled) ev.preventDefault();
  }

  /** Canvas of plot `key` (for tests). */
  canvasOf(key: string): { canvas: HTMLCanvasElement; rect: { x0: number; x1: number; y0: number; y1: number }; cursorY: Record<string, number> } | null {
    const p = this.plots.find((q) => q.plot.key === key);
    return p ? { canvas: p.canvas, rect: p.plotRect(), cursorY: { ...p.lastCursorY } } : null;
  }
}

/** "−5.33 ms" */
function timeText(t: number): string {
  return formatSeconds(t, 4).replace(/^-/, '−');
}

/** A figure over a linear time axis. */
export class TimeFigure extends Figure {
  readonly panel: TimePanel;
  protected readonly xHead = 'Time (ms)';

  constructor(opts: FigureOptions, presets: { label: string; key: string; range: () => [number, number] }[]) {
    super(opts);
    this.panel = new TimePanel(
      this.plotsEl,
      { onCursor: (i, k) => this.onCursor(i, k), onView: (lo, hi) => this.onView(lo, hi) },
      () => presets[0].range(),
    );
    this.addControls(presets);
    this.renderReadout(null);
  }

  protected panLabel(dir: number): string {
    return dir < 0 ? 'Pan to earlier times' : 'Pan to later times';
  }
  protected xText(t: number): string {
    return timeText(t);
  }
  protected xCell(t: number): string {
    return (t * 1e3).toFixed(4).replace(/^-/, '−');
  }

  set(plots: FigPlot[], data: TimeData): void {
    this.plots = plots;
    this.xs = Array.from({ length: data.n }, (_, i) => data.t0 + i * data.dt);
    const ids = new Set(plots.flatMap((p) => p.series.map((s) => s.id)));
    for (const h of [...this.panel.hidden]) if (!ids.has(h)) this.panel.hidden.delete(h);
    this.panel.setGroups(plots, data);
    this.renderLegend();
    this.renderReadout(this.panel.cursor);
  }

  hook(): Record<string, unknown> {
    return {
      view: [this.panel.lo, this.panel.hi],
      cursor: this.panel.cursor,
      plots: this.panel.plots.map((p) => ({ key: p.plot.key, rect: p.plotRect(), cursorY: { ...p.lastCursorY } })),
    };
  }
}
