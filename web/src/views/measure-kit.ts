// Shared parts of the time, isolation and fit views (docs/web.md, "Time",
// "Isolation", "Fit"): a DOM helper, figures (a plot panel with its view
// buttons, legend, crosshair readout and data table) over a log-frequency
// or a linear-time axis, coalesced engine calls, a job status line with
// progress and cancel, and file downloads.
//
// Nothing here computes a result: values come from the engine's reports
// and are only formatted.

import { Cancelled, type Reply } from '../engine';
import { formatHz, formatNumber } from '../format';
import { lineKey } from '../keys';
import { PlotPanel, RANGE_AUDIO, RANGE_FULL } from '../plot';
import type { Kind, PlotGroup, Scale } from '../series';
import { band, bandName, bandNote } from '../shading';
import type { Shading } from '../types';
import type { ViewHost } from './types';
import './measure.css';

// ----- DOM ------------------------------------------------------------------

type Child = Node | string | number | null | undefined | false;

interface Props {
  class?: string;
  text?: string;
  id?: string;
  hidden?: boolean;
  title?: string;
  /** Attributes set with setAttribute (aria-*, role, data-*, for, ...). */
  attrs?: Record<string, string | number | boolean>;
}

/** document.createElement with a class, text, attributes and children. */
export function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Props = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (props.class) e.className = props.class;
  if (props.id) e.id = props.id;
  if (props.title) e.title = props.title;
  if (props.hidden) e.hidden = true;
  if (props.text !== undefined) e.textContent = props.text;
  for (const [k, v] of Object.entries(props.attrs ?? {})) e.setAttribute(k, String(v));
  for (const c of children) if (c !== null && c !== undefined && c !== false) e.append(typeof c === 'number' ? String(c) : c);
  return e;
}

export function button(text: string, onClick: () => void, props: Props = {}): HTMLButtonElement {
  const b = el('button', { ...props, text });
  b.type = 'button';
  b.addEventListener('click', onClick);
  return b;
}

let uid = 0;
/** A document-unique id with a prefix. */
export function uniqueId(prefix: string): string {
  return `${prefix}-${++uid}`;
}

/** A labelled control: `<label for>` and the control in one wrapper. */
export function field(label: string, control: HTMLElement, hint?: string): HTMLElement {
  control.id ||= uniqueId('mv-f');
  const l = el('label', { text: label, attrs: { for: control.id } });
  const wrap = el('div', { class: 'mv-field' }, l, control);
  if (hint) {
    const h = el('span', { class: 'hint', id: uniqueId('mv-h'), text: hint });
    control.setAttribute('aria-describedby', h.id);
    wrap.append(h);
  }
  return wrap;
}

export function select(options: [string, string][], value?: string): HTMLSelectElement {
  const s = el('select');
  for (const [v, t] of options) s.append(el('option', { text: t, attrs: { value: v } }));
  if (value !== undefined) s.value = value;
  return s;
}

export function checkbox(label: string, checked = false): { wrap: HTMLLabelElement; input: HTMLInputElement } {
  const input = el('input', { attrs: { type: 'checkbox' } });
  input.checked = checked;
  return { wrap: el('label', { class: 'check' }, input, el('span', { text: label })), input };
}

/** A term/description list from pairs (null descriptions are left out). */
export function defList(rows: [string, Child | Child[]][], cls = ''): HTMLDListElement {
  const dl = el('dl', { class: `mv-dl ${cls}`.trim() });
  for (const [t, d] of rows) {
    if (d === null || d === undefined || d === false) continue;
    const dd = el('dd');
    for (const c of Array.isArray(d) ? d : [d]) if (c !== null && c !== undefined && c !== false) dd.append(typeof c === 'number' ? String(c) : c);
    dl.append(el('div', {}, el('dt', { text: t }), dd));
  }
  return dl;
}

/** A table with a caption and column headers; `rowHead` marks the first cell as a row header. */
export function table(caption: string, heads: string[], rows: Child[][], opts: { rowHead?: boolean; cls?: string } = {}): HTMLTableElement {
  const t = el('table', { class: `data-table ${opts.cls ?? ''}`.trim() });
  t.append(el('caption', { text: caption }));
  const hr = el('tr');
  for (const h of heads) hr.append(el('th', { text: h, attrs: { scope: 'col' } }));
  t.append(el('thead', {}, hr));
  const tb = el('tbody');
  for (const r of rows) {
    const tr = el('tr');
    r.forEach((c, k) => {
      const cell = k === 0 && opts.rowHead ? el('th', { attrs: { scope: 'row' } }) : el('td');
      if (c !== null && c !== undefined && c !== false) cell.append(typeof c === 'number' ? String(c) : c);
      tr.append(cell);
    });
    tb.append(tr);
  }
  t.append(tb);
  return t;
}

/** A keyboard-reachable scrolling region around a table (WCAG 2.1.1). */
export function scrollRegion(label: string, content: HTMLElement): HTMLElement {
  return el('div', { class: 'table-wrap mv-table-wrap', attrs: { role: 'region', 'aria-label': label, tabindex: 0 } }, content);
}

export function errorBox(title: string, message: string, detail?: string): HTMLElement {
  return el(
    'div',
    { class: 'error-box mv-error', attrs: { role: 'alert' } },
    el('h3', {}, el('span', { text: '⚠', attrs: { 'aria-hidden': 'true' } }), ` ${title}`),
    el('p', { class: 'mv-error-message', text: message }),
    detail ? el('p', { class: 'hint', text: detail }) : null,
  );
}

// ----- engine calls ------------------------------------------------------------

export interface EngineError {
  error: string;
  kind: string;
  line?: number;
  column?: number;
  [k: string]: unknown;
}

export function isEngineError(v: unknown): v is EngineError {
  return typeof v === 'object' && v !== null && typeof (v as EngineError).error === 'string';
}

/** A reply's value, or an error object for a crash. */
export function valueOf(r: Reply): unknown {
  return r.ok ? r.value : { error: `the engine failed: ${r.crash}`, kind: 'panic' };
}

/**
 * One kind of engine call on a view's worker, coalesced: at most one is in
 * flight, and while it runs only the newest request is kept (as for the
 * live solve). A request whose key equals the last delivered one is not
 * sent again.
 *
 * A cancel terminates every call on the worker. After one, `abandon()`
 * forgets the call in flight (the next request is sent afresh), and
 * `resend()` also sends the wanted request again (when the cancel was meant
 * for another job on the same worker).
 */
export class Coalesced {
  private inflight: string | null = null;
  private next: { key: string; args: string[] } | null = null;
  private wanted: { key: string; args: string[] } | null = null;
  private delivered: string | null = null;
  private gen = 0;

  constructor(
    private readonly host: ViewHost,
    private readonly fn: string,
    private readonly deliver: (key: string, value: unknown, ms: number) => void,
    /** Called when `busy` changes. */
    private readonly onBusy: () => void = () => {},
  ) {}

  /** A call is in flight. */
  get busy(): boolean {
    return this.inflight !== null;
  }

  request(key: string, args: string[]): void {
    this.wanted = { key, args };
    if (this.inflight === key) {
      this.next = null;
      return;
    }
    if (this.inflight !== null) {
      this.next = { key, args };
      return;
    }
    if (this.delivered === key) return;
    void this.send(key, args);
  }

  /** After a cancel: nothing is in flight or queued, and the last delivery is forgotten. */
  abandon(): void {
    this.gen++;
    this.next = null;
    this.delivered = null;
    this.setInflight(null);
  }

  /** After a cancel meant for another job: sends the wanted request again if the cancel interrupted it. */
  resend(): void {
    if (this.inflight === null) return;
    this.abandon();
    const w = this.wanted;
    if (w) void this.send(w.key, w.args);
  }

  private setInflight(key: string | null): void {
    const was = this.busy;
    this.inflight = key;
    if (was !== this.busy) this.onBusy();
  }

  private async send(key: string, args: string[]): Promise<void> {
    const gen = this.gen;
    this.setInflight(key);
    let reply: Reply;
    try {
      reply = await this.host.call(this.fn, ...args);
    } catch (e) {
      if (e instanceof Cancelled) return; // abandon() or resend() takes over
      throw e;
    }
    if (gen !== this.gen) return;
    const next = this.next;
    this.next = null;
    this.delivered = key;
    if (next && next.key !== key) {
      this.inflight = null;
      void this.send(next.key, next.args);
    } else {
      this.setInflight(null);
    }
    this.deliver(key, valueOf(reply), reply.ok ? reply.ms : 0);
  }
}

// ----- jobs --------------------------------------------------------------------

/**
 * Status line of a long job: what runs, a progress bar (determinate when
 * the job knows its length), the elapsed time, and a Cancel button.
 */
export class JobStatus {
  readonly root: HTMLElement;
  private readonly text: HTMLElement;
  private readonly bar: HTMLProgressElement;
  private readonly elapsed: HTMLElement;
  readonly cancelBtn: HTMLButtonElement;
  private t0 = 0;
  private timer = 0;
  running = false;

  constructor(label: string, onCancel: () => void) {
    this.text = el('span', { class: 'mv-job-text' });
    this.bar = el('progress', { attrs: { 'aria-label': label } });
    this.elapsed = el('span', { class: 'hint mv-job-time' });
    this.cancelBtn = button('Cancel', onCancel, { class: 'mv-job-cancel' });
    this.root = el('div', { class: 'mv-job', hidden: true }, this.text, this.bar, this.elapsed, this.cancelBtn);
  }

  start(text: string): void {
    this.running = true;
    this.root.hidden = false;
    this.root.dataset.state = 'running';
    this.text.textContent = text;
    this.bar.hidden = false;
    this.bar.removeAttribute('value');
    this.cancelBtn.hidden = false;
    this.t0 = performance.now();
    this.tick();
    clearInterval(this.timer);
    this.timer = window.setInterval(() => this.tick(), 250);
  }

  /** `total` null: indeterminate. */
  progress(done: number, total: number | null, text?: string): void {
    if (total !== null && total > 0) {
      this.bar.max = total;
      this.bar.value = Math.min(done, total);
    } else {
      this.bar.removeAttribute('value');
    }
    if (text) this.text.textContent = text;
  }

  finish(text: string, state: 'done' | 'cancelled' | 'failed' = 'done'): void {
    this.running = false;
    clearInterval(this.timer);
    this.tick();
    this.root.dataset.state = state;
    this.text.textContent = text;
    this.bar.hidden = true;
    this.cancelBtn.hidden = true;
  }

  hide(): void {
    this.finish('', 'done');
    this.root.hidden = true;
  }

  private tick(): void {
    this.elapsed.textContent = `${((performance.now() - this.t0) / 1000).toFixed(1)} s`;
  }
}

// ----- files -------------------------------------------------------------------

/** Offers `data` as a file download named `name`. */
export function download(name: string, data: BlobPart, type: string): void {
  const url = URL.createObjectURL(new Blob([data], { type }));
  const a = el('a', { attrs: { href: url, download: name } });
  a.hidden = true;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}

/** A file name made of safe characters. */
export function safeName(s: string): string {
  return s.replace(/[^A-Za-z0-9._-]+/g, '_').replace(/^_+|_+$/g, '') || 'curve';
}

// ----- figures -----------------------------------------------------------------

/** A curve of a figure; `slot` fixes its colour and dash (series.ts `styleSlot`). */
export interface FigSeries {
  id: string;
  slot: number;
  values: ArrayLike<number | null>;
  primary?: boolean;
  /** Short tag after the name in the legend ("decision", "limit"). */
  tag?: string;
}

/** One plot of a figure: a quantity with one unit. */
export interface FigPlot {
  key: string;
  title: string;
  symbol: string;
  /** Engine unit spelling (SI prefixes are applied to it on the axis); "" for dB or degrees. */
  unit: string;
  /** Axis unit text as displayed. */
  axisUnit: string;
  kind: Kind;
  scale?: Scale;
  height?: 'main' | 'small';
  series: FigSeries[];
  /** Readout text of a value. */
  format(v: number): string;
  /** Unit in the data table's column headers. */
  columnUnit: string;
  /** Table cell text of a value (default: 5 significant figures). */
  cell?(v: number): string;
}

/** The members of a plot panel a figure drives (PlotPanel and TimePanel). */
export interface AxisPanel {
  hidden: Set<string>;
  cursor: number | null;
  lo: number;
  hi: number;
  render(): void;
  renderNow(): void;
  setView(lo: number, hi: number): void;
  zoom(factor: number): void;
  pan(fraction: number): void;
  setCursor(i: number | null, fromKeyboard?: boolean): void;
  viewIndices(): [number, number] | null;
}

/** Most rows a figure's data table builds at once (as the Response table). */
export const MAX_TABLE_ROWS = 1000;

export interface FigureOptions {
  /** Stable id (test hooks, DOM ids). */
  id: string;
  /** Name of the figure for its toolbar, legend and table regions. */
  label: string;
  host: ViewHost;
  /** Keyboard help under the plots. */
  help: string;
}

/**
 * Plots with a shared crosshair and view, their view buttons, a legend that
 * shows or hides each curve, a readout of every visible curve at the
 * crosshair, and a data table of the values in view.
 */
export abstract class Figure {
  readonly root: HTMLElement;
  protected readonly plotsEl: HTMLElement;
  protected readonly legend: HTMLElement;
  protected readonly readout: HTMLElement;
  protected readonly tableWrap: HTMLElement;
  protected readonly tableBtn: HTMLButtonElement;
  protected readonly toolbar: HTMLElement;
  protected readonly rangeText: HTMLElement;
  protected plots: FigPlot[] = [];
  protected xs: number[] = [];
  abstract readonly panel: AxisPanel;

  constructor(protected readonly opts: FigureOptions) {
    this.rangeText = el('span', { class: 'hint' });
    this.toolbar = el('div', { class: 'view-controls', attrs: { role: 'toolbar', 'aria-label': `${opts.label}: view` } });
    this.legend = el('div', { class: 'legend', attrs: { role: 'group', 'aria-label': `${opts.label}: press to show or hide a curve` } });
    this.readout = el('div', { class: 'readout mv-readout' });
    this.plotsEl = el('div', { class: 'plots mv-plots' });
    this.tableWrap = el('div', {
      class: 'table-wrap',
      hidden: true,
      id: uniqueId(`mv-table-${opts.id}`),
      attrs: { role: 'region', 'aria-label': `${opts.label}: data table`, tabindex: 0 },
    });
    this.tableBtn = button('Show data table', () => this.toggleTable(), {
      attrs: { 'aria-expanded': 'false', 'aria-controls': this.tableWrap.id },
    });
    this.root = el(
      'div',
      { class: 'mv-figure', attrs: { 'data-figure': opts.id } },
      this.toolbar,
      this.legend,
      this.readout,
      this.plotsEl,
      el('p', { class: 'hint mv-plot-help', text: opts.help }),
      el('div', { class: 'table-controls' }, this.tableBtn),
      this.tableWrap,
    );
  }

  /** Adds the view buttons: presets, zoom and pan (call once the panel exists). */
  protected addControls(presets: { label: string; range: () => [number, number]; key: string }[]): void {
    this.toolbar.append(el('span', { class: 'group-label', text: 'View' }));
    for (const p of presets) {
      const b = button(p.label, () => this.panel.setView(...p.range()), { attrs: { 'aria-pressed': 'false', 'data-preset': p.key } });
      this.toolbar.append(b);
    }
    this.toolbar.append(
      button('+', () => this.panel.zoom(0.5), { attrs: { 'aria-label': 'Zoom in' } }),
      button('−', () => this.panel.zoom(2), { attrs: { 'aria-label': 'Zoom out' } }),
      button('←', () => this.panel.pan(-0.25), { attrs: { 'aria-label': this.panLabel(-1) } }),
      button('→', () => this.panel.pan(0.25), { attrs: { 'aria-label': this.panLabel(1) } }),
      this.rangeText,
    );
    this.presets = presets;
  }

  private presets: { label: string; range: () => [number, number]; key: string }[] = [];

  protected abstract panLabel(dir: number): string;
  /** Readout and table text of an x value. */
  protected abstract xText(x: number): string;
  protected abstract xCell(x: number): string;
  protected abstract xHead: string;
  /** Validity note (frequency figures) at an x value. */
  protected note(_x: number): string {
    return '';
  }
  protected validityCell(_x: number): string | null {
    return null;
  }

  /** Called by the panel when the view changes. */
  protected onView(lo: number, hi: number): void {
    this.rangeText.textContent = `Showing ${this.xText(lo)} to ${this.xText(hi)}`;
    const near = (u: number, v: number) => Math.abs(u - v) <= 1e-6 * Math.max(Math.abs(u), Math.abs(v), 1e-12);
    for (const b of this.toolbar.querySelectorAll<HTMLButtonElement>('[data-preset]')) {
      const p = this.presets.find((q) => q.key === b.dataset.preset);
      if (!p) continue;
      const [a, z] = p.range();
      b.setAttribute('aria-pressed', String(near(lo, a) && near(hi, z)));
    }
    if (!this.tableWrap.hidden) this.renderTable();
  }

  /** Called by the panel when the crosshair moves. */
  protected onCursor(i: number | null, fromKeyboard: boolean): void {
    this.renderReadout(i);
    if (fromKeyboard && i !== null) {
      const parts = this.visible().map(({ p, s }) => {
        const v = s.values[i];
        return `${s.id} ${v === null || v === undefined || !Number.isFinite(v) ? 'n/a' : p.format(v)}`;
      });
      const note = this.note(this.xs[i]);
      this.opts.host.announce(`${this.xText(this.xs[i])}: ${parts.join('; ')}${note ? `. ${note}` : ''}.`);
    }
  }

  protected visible(): { p: FigPlot; s: FigSeries }[] {
    const out: { p: FigPlot; s: FigSeries }[] = [];
    for (const p of this.plots) for (const s of p.series) if (!this.panel.hidden.has(s.id)) out.push({ p, s });
    return out;
  }

  protected renderLegend(): void {
    this.legend.replaceChildren();
    const seen = new Set<string>();
    for (const p of this.plots) {
      for (const s of p.series) {
        if (seen.has(s.id)) continue;
        seen.add(s.id);
        const b = el('button', { class: 'legend-item', attrs: { type: 'button', 'aria-pressed': String(!this.panel.hidden.has(s.id)), 'data-series': s.id } });
        b.append(lineKey(s.slot), el('span', { class: 'legend-name', text: s.id }));
        if (s.tag) b.append(el('span', { class: 'legend-tag', text: s.tag }));
        b.title = `Show or hide ${s.id}`;
        b.addEventListener('click', () => {
          if (this.panel.hidden.has(s.id)) this.panel.hidden.delete(s.id);
          else this.panel.hidden.add(s.id);
          b.setAttribute('aria-pressed', String(!this.panel.hidden.has(s.id)));
          this.panel.render();
          this.renderReadout(this.panel.cursor);
          if (!this.tableWrap.hidden) this.renderTable();
        });
        this.legend.append(b);
      }
    }
    this.legend.hidden = seen.size < 2;
  }

  protected renderReadout(i: number | null): void {
    this.readout.replaceChildren();
    if (i === null || i >= this.xs.length) {
      this.readout.append(el('p', { class: 'hint', text: 'Crosshair: point at a plot, or focus one and use the arrow keys, to read every visible curve.' }));
      return;
    }
    const x = this.xs[i];
    const note = this.note(x);
    this.readout.append(
      el('p', { class: 'readout-head' }, el('strong', { text: this.xText(x) }), ` (point ${i + 1} of ${this.xs.length})${note ? `, ${note}` : ''}`),
    );
    const list = el('ul', { class: 'readout-list' });
    for (const { p, s } of this.visible()) {
      const v = s.values[i];
      const text = v === null || v === undefined || !Number.isFinite(v) ? 'n/a' : p.format(v);
      list.append(el('li', { attrs: { 'data-series': s.id } }, lineKey(s.slot, 20), el('strong', { text }), el('span', { class: 'readout-name', text: `${s.id} · ${p.symbol}` })));
    }
    this.readout.append(list);
  }

  private toggleTable(): void {
    const open = this.tableWrap.hidden;
    this.tableWrap.hidden = !open;
    this.tableBtn.setAttribute('aria-expanded', String(open));
    this.tableBtn.textContent = open ? 'Hide data table' : 'Show data table';
    if (open) this.renderTable();
  }

  protected renderTable(): void {
    this.tableWrap.replaceChildren();
    const idx = this.panel.viewIndices();
    const [a, b] = idx ?? [0, -1];
    const n = b - a + 1;
    const stride = Math.max(1, Math.ceil(n / MAX_TABLE_ROWS));
    const rows: number[] = [];
    for (let i = a; i <= b; i += stride) rows.push(i);
    if (n > 0 && rows[rows.length - 1] !== b) rows.push(b);
    const cols = this.visible();
    const withValidity = this.validityCell(this.xs[0] ?? 0) !== null;
    const heads = [this.xHead, ...(withValidity ? ['Validity'] : []), ...cols.map(({ p, s }) => `${s.id} ${p.symbol} (${p.columnUnit})`)];
    const body = rows.map((i) => {
      const x = this.xs[i];
      const cells: Child[] = [this.xCell(x)];
      if (withValidity) cells.push(this.validityCell(x) ?? '');
      for (const { p, s } of cols) {
        const v = s.values[i];
        cells.push(v === null || v === undefined || !Number.isFinite(v) ? '' : p.cell ? p.cell(v) : formatNumber(v));
      }
      return cells;
    });
    const cap =
      `${this.opts.label}: values from ${this.xText(this.panel.lo)} to ${this.xText(this.panel.hi)} (${Math.max(0, n)} rows` +
      (stride > 1 ? `; ${rows.length} shown, one point in ${stride} and the last: zoom in to list them all` : '') +
      ')' +
      (withValidity ? '. Validity: "light" = lumped-model error ≥ 10 % or below an element’s validated range, "dark" = ≥ 36 % or past a hard limit.' : '.');
    this.tableWrap.append(table(cap, heads, body, { rowHead: true }));
  }

  /** Test hook data: the panel's view, cursor and plot boxes. */
  abstract hook(): Record<string, unknown>;
}

// ----- frequency figures ---------------------------------------------------------

/** A figure over the log-frequency axis of the Response tab (plot.ts `PlotPanel`). */
export class FreqFigure extends Figure {
  readonly panel: PlotPanel;
  private shading: Shading | null = null;
  protected readonly xHead = 'Frequency (Hz)';

  constructor(opts: FigureOptions) {
    super(opts);
    this.panel = new PlotPanel(this.plotsEl, {
      onCursor: (i, k) => this.onCursor(i, k),
      onView: (lo, hi) => this.onView(lo, hi),
    });
    this.addControls([
      { label: '20 Hz–20 kHz', range: () => RANGE_AUDIO, key: 'audio' },
      { label: '10 Hz–40 kHz', range: () => RANGE_FULL, key: 'full' },
    ]);
    this.renderReadout(null);
  }

  protected panLabel(dir: number): string {
    return dir < 0 ? 'Pan to lower frequencies' : 'Pan to higher frequencies';
  }
  protected xText(x: number): string {
    return formatHz(x);
  }
  protected xCell(x: number): string {
    return formatNumber(x, 6);
  }
  protected override note(x: number): string {
    return this.shading ? bandNote(this.shading, x) : '';
  }
  protected override validityCell(x: number): string | null {
    return this.shading ? bandName(band(this.shading, x)) : null;
  }

  /**
   * Shows `plots` over the ascending frequencies `freqs`, with the validity
   * shading of the result they come from.
   */
  set(plots: FigPlot[], freqs: number[], shading: Shading | null): void {
    this.plots = plots;
    this.xs = freqs;
    this.shading = shading;
    const ids = new Set(plots.flatMap((p) => p.series.map((s) => s.id)));
    for (const h of [...this.panel.hidden]) if (!ids.has(h)) this.panel.hidden.delete(h);
    const groups: PlotGroup[] = plots.map((p) => ({
      key: p.key,
      kind: p.kind,
      title: p.title,
      symbol: p.symbol,
      unit: p.unit,
      axisUnit: p.axisUnit,
      scale: p.scale ?? 'linear',
      series: p.series.map((s) => ({ probe: s.slot, id: s.id, values: Array.from(s.values), primary: s.primary })),
      overlays: [],
      height: p.height ?? 'main',
    }));
    const hl = this.panel.data?.highlight ?? null;
    this.panel.setGroups(groups, { freqs, shading: shading ?? { begin_hz: null, deep_hz: null }, highlight: hl });
    this.panel.setView(this.panel.lo, this.panel.hi);
    this.renderLegend();
    this.renderReadout(this.panel.cursor);
    if (!this.tableWrap.hidden) this.renderTable();
  }

  /** Marks frequency f (a narrow highlighted range with a label) on every plot; null clears. */
  mark(f: number | null, label = ''): void {
    this.panel.setHighlight(f === null ? null : { lo: f / 1.006, hi: f * 1.006, label });
  }

  hook(): Record<string, unknown> {
    return {
      view: [this.panel.lo, this.panel.hi],
      cursor: this.panel.cursor,
      highlight: this.panel.data?.highlight ?? null,
      plots: this.panel.plots.map((p) => ({ key: p.group.key, rect: p.plotRect(), cursorY: { ...p.lastCursorY } })),
    };
  }

  /** RGB pixels of one device-pixel row of plot `key`, at CSS y (for tests). */
  row(key: string, y: number): { dpr: number; px: number[][] } | null {
    const p = this.panel.plots.find((q) => q.group.key === key);
    if (!p) return null;
    this.panel.renderNow();
    const dpr = window.devicePixelRatio || 1;
    const d = p.canvas.getContext('2d')!.getImageData(0, Math.round(y * dpr), p.canvas.width, 1).data;
    const px: number[][] = [];
    for (let k = 0; k < d.length; k += 4) px.push([d[k], d[k + 1], d[k + 2]]);
    return { dpr, px };
  }

  /** RGB pixels of a column of plot `key` at CSS x (for tests). */
  column(key: string, x: number): { dpr: number; px: number[][] } | null {
    const p = this.panel.plots.find((q) => q.group.key === key);
    if (!p) return null;
    this.panel.renderNow();
    const dpr = window.devicePixelRatio || 1;
    const d = p.canvas.getContext('2d')!.getImageData(Math.round(x * dpr), 0, 1, p.canvas.height).data;
    const px: number[][] = [];
    for (let k = 0; k < d.length; k += 4) px.push([d[k], d[k + 1], d[k + 2]]);
    return { dpr, px };
  }

  /** CSS x of frequency f in plot `key` at the current view. */
  xOf(key: string, f: number): number | null {
    const p = this.panel.plots.find((q) => q.group.key === key);
    return p ? p.xOf(f, this.panel.lo, this.panel.hi) : null;
  }
}

// ----- test hooks ------------------------------------------------------------------

type Hooks = Record<string, Record<string, unknown>>;

/**
 * Registers read-only test and debugging hooks under
 * `window.acoustilabViews[view]` (the Response tab has `window.acoustilab`).
 */
export function registerHooks(view: string, hooks: Record<string, unknown>): void {
  const w = window as unknown as { acoustilabViews?: Hooks };
  w.acoustilabViews ??= {};
  w.acoustilabViews[view] = hooks;
}

// ----- formatting ----------------------------------------------------------------

/** "12.3 ms", "95.4 µs", "1.20 s" (three significant figures). */
export function formatSeconds(s: number | null | undefined, digits = 3): string {
  if (s === null || s === undefined || !Number.isFinite(s)) return 'n/a';
  const a = Math.abs(s);
  if (a === 0) return '0 s';
  if (a >= 1) return `${Number(s.toPrecision(digits))} s`;
  if (a >= 1e-3) return `${Number((s * 1e3).toPrecision(digits))} ms`;
  if (a >= 1e-6) return `${Number((s * 1e6).toPrecision(digits))} µs`;
  return `${Number((s * 1e9).toPrecision(digits))} ns`;
}

/** "+1.23 dB", "−0.40 dB". */
export function formatDb(v: number | null | undefined, digits = 1, signed = false): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return v === Infinity ? '∞ dB' : 'n/a';
  const t = Math.abs(v).toFixed(digits);
  const sign = v < 0 && Number(t) !== 0 ? '−' : signed && Number(t) !== 0 ? '+' : '';
  return `${sign}${t} dB`;
}

/** Engine names ("weakly_determined") to words. */
export function words(s: string): string {
  return s.replace(/_/g, ' ');
}
