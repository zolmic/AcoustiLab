// Shared pieces of the analysis views (sensitivity, tolerance, target) and
// the Response tab's readouts block: engine calls that fail loudly, a run
// bar with progress and cancel, the stale banner, data tables, a diverging
// colour scale, and a read-only test hook.
//
// Nothing here writes explanatory text of its own: labels name things, and
// every number or sentence about the design comes from the engine.

import './analysis.css';
import { Cancelled } from '../engine';
import { formatParam } from '../format';
import { isError, type SolveResult } from '../types';
import type { ViewHost } from './types';

export { Cancelled };

export function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

export function button(text: string, cls?: string): HTMLButtonElement {
  const b = el('button', cls, text);
  b.type = 'button';
  return b;
}

let uid = 0;
/** A document-unique id with a readable prefix. */
export const nextId = (prefix: string) => `${prefix}-${++uid}`;

/** An engine error object, a crash of the engine worker, or bad input. */
export class EngineFailure extends Error {
  constructor(
    message: string,
    readonly kind: string,
  ) {
    super(message);
  }
}

/**
 * Calls a wasm export on the view's worker and returns its parsed value.
 * Throws `EngineFailure` for an engine error object or a crash, and
 * `Cancelled` when the view's worker is terminated meanwhile.
 */
export async function callJson<T>(host: ViewHost, fn: string, ...args: string[]): Promise<T> {
  const r = await host.call(fn, ...args);
  if (!r.ok) throw new EngineFailure(r.crash, 'panic');
  if (isError(r.value)) throw new EngineFailure(r.value.error, r.value.kind);
  return r.value as T;
}

/** "+1.23", "−0.40", "0.00": a signed number with a true minus sign. */
export function signed(x: number, digits = 2): string {
  const t = Math.abs(x).toFixed(digits);
  return `${Number(t) === 0 ? '' : x > 0 ? '+' : '−'}${t}`;
}

/** A signed value to `sig` significant figures ("+12.3", "−0.0412"). */
export function signedSig(x: number, sig = 3): string {
  if (x === 0) return '0';
  return `${x > 0 ? '+' : '−'}${formatParam(Math.abs(x), sig)}`;
}

/** Ids of the pressure probes of a result (those with a dB SPL curve). */
export function pressureProbes(r: SolveResult | null | undefined): string[] {
  return (r?.probes ?? []).filter((p) => p.spl_dB).map((p) => p.id);
}

/** The netlist's `ui.primary_probe`, read from its text. */
export function primaryProbeOf(text: string | null | undefined): string | null {
  if (!text) return null;
  try {
    const d = JSON.parse(text) as { ui?: { primary_probe?: unknown } };
    return typeof d?.ui?.primary_probe === 'string' ? d.ui.primary_probe : null;
  } catch {
    return null;
  }
}

/** Replaces the options of a select, keeping `keep` selected when it is still offered. */
export function setOptions(sel: HTMLSelectElement, options: [string, string][], keep: string | null): void {
  sel.replaceChildren(...options.map(([v, t]) => Object.assign(el('option', undefined, t), { value: v })));
  if (keep !== null && options.some(([v]) => v === keep)) sel.value = keep;
}

/** A labelled control: `<label>text</label>` then the control, in one wrapper. */
export function field(label: string, control: HTMLElement, cls = 'an-field'): HTMLElement {
  const w = el('span', cls);
  const l = el('label', undefined, label);
  control.id ||= nextId('an');
  l.htmlFor = control.id;
  w.append(l, control);
  return w;
}

/**
 * Run and cancel buttons with a progress bar and a status line (a polite
 * live region). One job at a time; `cancel()` terminates the view's worker,
 * which rejects the pending call with `Cancelled`.
 */
export class RunBar {
  readonly el = el('div', 'an-runbar');
  readonly run: HTMLButtonElement;
  readonly cancel = button('Cancel');
  private readonly progress = el('progress', 'an-progress');
  readonly status = el('p', 'an-status');
  private busy = false;

  constructor(runLabel: string, onRun: () => void, onCancel: () => void) {
    this.run = button(runLabel, 'primary');
    this.run.addEventListener('click', () => {
      if (!this.busy) onRun();
    });
    this.cancel.disabled = true;
    this.cancel.addEventListener('click', () => onCancel());
    this.progress.hidden = true;
    this.progress.max = 1;
    this.progress.value = 0;
    this.progress.setAttribute('aria-label', `${runLabel}: progress`);
    this.status.setAttribute('role', 'status');
    this.el.append(this.run, this.cancel, this.progress, this.status);
  }

  get running(): boolean {
    return this.busy;
  }

  start(total: number, text: string): void {
    this.busy = true;
    this.run.setAttribute('aria-disabled', 'true');
    this.cancel.disabled = false;
    this.progress.hidden = false;
    this.progress.max = Math.max(1, total);
    this.progress.value = 0;
    this.status.textContent = text;
  }

  step(done: number, text: string): void {
    this.progress.value = done;
    this.status.textContent = text;
  }

  finish(text: string): void {
    this.busy = false;
    this.run.removeAttribute('aria-disabled');
    // The focus leaves Cancel for Run before Cancel is disabled (a disabled
    // button drops the focus to the page).
    if (document.activeElement === this.cancel) this.run.focus();
    this.cancel.disabled = true;
    this.progress.hidden = true;
    this.status.textContent = text;
  }
}

/**
 * The banner of a view whose numbers belong to an earlier design, with a
 * one-click recompute. The view marks its output `.is-stale` meanwhile.
 */
export class StaleBanner {
  readonly el = el('div', 'an-stale');
  private readonly text = el('p');
  readonly recompute = button('Recompute');

  constructor(onRecompute: () => void) {
    this.el.setAttribute('role', 'status');
    const icon = el('span', 'an-stale-icon', '⟳');
    icon.setAttribute('aria-hidden', 'true');
    this.recompute.addEventListener('click', onRecompute);
    this.el.append(icon, this.text, this.recompute);
    this.el.hidden = true;
  }

  set(stale: boolean, what: string): void {
    this.el.hidden = !stale;
    if (stale) this.text.textContent = `Stale: ${what} computed for an earlier version of the design. The Response plots show the current one.`;
  }
}

/** A table with a caption, column headers and rows (first cell a row header). */
export function dataTable(caption: string, head: string[], rows: (string | Node)[][], cls = 'data-table'): HTMLTableElement {
  const t = el('table', cls);
  t.append(el('caption', undefined, caption));
  const thead = el('thead');
  const hr = el('tr');
  for (const h of head) {
    const c = el('th', undefined, h);
    c.scope = 'col';
    hr.append(c);
  }
  thead.append(hr);
  const tbody = el('tbody');
  for (const r of rows) {
    const tr = el('tr');
    r.forEach((v, k) => {
      const c = el(k === 0 ? 'th' : 'td');
      if (k === 0) (c as HTMLTableCellElement).scope = 'row';
      c.append(v);
      tr.append(c);
    });
    tbody.append(tr);
  }
  t.append(thead, tbody);
  return t;
}

/** A scrollable, keyboard-reachable region around a wide table (WCAG 2.1.1). */
export function scrollRegion(label: string, child: Node): HTMLElement {
  const w = el('div', 'table-wrap');
  w.tabIndex = 0;
  w.setAttribute('role', 'region');
  w.setAttribute('aria-label', label);
  w.append(child);
  return w;
}

/**
 * A table in its scroll region, with a short caption and, above it, a
 * description paragraph that wraps at the page width (a caption is as wide
 * as its table) and describes the table (`aria-describedby`).
 */
export function tableBlock(caption: string, description: string, head: string[], rows: (string | Node)[][]): { el: HTMLElement; table: HTMLTableElement } {
  const t = dataTable(caption, head, rows);
  const wrap = el('div', 'an-table-block');
  if (description) {
    const d = el('p', 'hint', description);
    d.id = nextId('an-desc');
    t.setAttribute('aria-describedby', d.id);
    wrap.append(d);
  }
  wrap.append(scrollRegion(caption, t));
  return { el: wrap, table: t };
}

/** A "Show …" / "Hide …" toggle for a region built on demand. */
export function disclosure(label: string, build: () => Node): { button: HTMLButtonElement; region: HTMLElement; refresh(): void } {
  const b = button(`Show ${label}`);
  const region = el('div', 'an-disclosed');
  region.id = nextId('an-region');
  region.hidden = true;
  b.setAttribute('aria-expanded', 'false');
  b.setAttribute('aria-controls', region.id);
  const refresh = () => {
    if (!region.hidden) region.replaceChildren(build());
  };
  b.addEventListener('click', () => {
    region.hidden = !region.hidden;
    b.setAttribute('aria-expanded', String(!region.hidden));
    b.textContent = `${region.hidden ? 'Show' : 'Hide'} ${label}`;
    refresh();
  });
  return { button: b, region, refresh };
}

/** Offers `text` to the viewer as a file. */
export function download(name: string, text: string, type = 'text/csv'): void {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = el('a');
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ----- diverging colour scale ---------------------------------------------

type RGB = [number, number, number];

const hexRgb = (h: string): RGB | null =>
  /^#[0-9a-f]{6}$/i.test(h) ? ([1, 3, 5].map((k) => parseInt(h.slice(k, k + 2), 16)) as RGB) : null;

const toLin = (c: number) => {
  const x = c / 255;
  return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
};
const fromLin = (x: number) => {
  const c = x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055;
  return Math.round(255 * Math.min(1, Math.max(0, c)));
};

/** sRGB to OKLab (Björn Ottosson 2020). */
function oklab([r8, g8, b8]: RGB): RGB {
  const [r, g, b] = [toLin(r8), toLin(g8), toLin(b8)];
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

function fromOklab([L, a, b]: RGB): RGB {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    fromLin(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    fromLin(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    fromLin(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
  ];
}

/**
 * Diverging scale from the theme's tokens (`--div-neg`, `--div-mid`,
 * `--div-pos`): two hues through a neutral grey midpoint, interpolated
 * in OKLab so equal steps look equal on both arms.
 */
export class Diverging {
  private neg: RGB = [28, 92, 171];
  private mid: RGB = [240, 239, 236];
  private pos: RGB = [176, 48, 47];
  hex = { neg: '', mid: '', pos: '' };

  constructor() {
    this.read();
  }

  read(): void {
    const cs = getComputedStyle(document.documentElement);
    const v = (n: string) => cs.getPropertyValue(n).trim();
    this.hex = { neg: v('--div-neg'), mid: v('--div-mid'), pos: v('--div-pos') };
    this.neg = hexRgb(this.hex.neg) ?? this.neg;
    this.mid = hexRgb(this.hex.mid) ?? this.mid;
    this.pos = hexRgb(this.hex.pos) ?? this.pos;
  }

  /** Colour of t in [−1, 1] (clamped). */
  at(t: number): RGB {
    const u = Math.min(1, Math.max(-1, t));
    const a = oklab(this.mid);
    const b = oklab(u < 0 ? this.neg : this.pos);
    const k = Math.abs(u);
    return fromOklab([a[0] + k * (b[0] - a[0]), a[1] + k * (b[1] - a[1]), a[2] + k * (b[2] - a[2])]);
  }

  css(t: number): string {
    const [r, g, b] = this.at(t);
    return `rgb(${r}, ${g}, ${b})`;
  }
}

/** Calls `fn` when the colour theme changes (OS setting or data-theme). */
export function onThemeChange(fn: () => void): void {
  new MutationObserver(fn).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', fn);
}

// ----- test hook --------------------------------------------------------------

const hooks: Record<string, () => unknown> = {};
Object.defineProperty(window, 'acoustilabAnalysis', { value: Object.freeze({ get: (name: string) => hooks[name]?.() ?? null }) });

/** Exposes read-only state of a view to the tests (`window.acoustilabAnalysis.get(name)`). */
export function exposeForTests(name: string, get: () => unknown): void {
  hooks[name] = get;
}
