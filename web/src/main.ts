// AcoustiLab web UI: a design panel and a netlist editor over one netlist
// text, solves in a Web Worker, Canvas 2D plots.
//
// The netlist text (the editor's value) is the single source of truth (spec
// Section 2, principle 1). The design panel writes parameter values into it
// surgically (jsonscan.ts) and shows what the engine's `parameters()` reads
// back from it; the plots show what `solve()` makes of it.

import './style.css';
import { VIEWS } from './views/registry';
import type { ResultView, ViewHost } from './views/types';
import { Baselines } from './baselines';
import { Coalesced, EngineWorker, type Reply } from './engine';
import { EXAMPLES, hasParameters } from './examples';
import { DesignPanel } from './design';
import { formatHz, formatNumber, formatParam, formatValue, prettyUnit } from './format';
import { declaredValues, paramDeclSpan, scan, setParams } from './jsonscan';
import { lineKey } from './keys';
import { PlotPanel, RANGE_AUDIO, RANGE_FULL } from './plot';
import { buildGroups, sameGrid, valueAt, type PlotGroup } from './series';
import { band, bandName, bandNote } from './shading';
import { Sketch, type Part } from './sketch';
import { store } from './store';
import {
  isError,
  type CheckResult,
  type EngineError,
  type ParamsDoc,
  type Scalar,
  type SolveResult,
  type UiBlock,
} from './types';
import { highlightOf, isOperating, WarningsPanel } from './warnings';

// ----- DOM ------------------------------------------------------------------

const $ = <T extends HTMLElement>(id: string) => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing #${id}`);
  return el as T;
};

const editor = $<HTMLTextAreaElement>('netlist');
const exampleSelect = $<HTMLSelectElement>('example-select');
const runBtn = $<HTMLButtonElement>('run-btn');
const cancelBtn = $<HTMLButtonElement>('cancel-btn');
const checkStatus = $('check-status');
const cursorPos = $('cursor-pos');
const errorBox = $('error-box');
const errorMessage = $('error-message');
const errorDetail = $('error-detail');
const errorGoto = $<HTMLButtonElement>('error-goto');
const runStatus = $('run-status');
const resultTitle = $('result-title');
const legend = $('legend');
const readout = $('readout');
const srReadout = $('sr-readout');
const plotsEl = $('plots');
const plotPanelEl = plotsEl.closest<HTMLElement>('.plot-panel')!;
const viewRange = $('view-range');
const tableToggle = $<HTMLButtonElement>('table-toggle');
const tableWrap = $('table-wrap');
const validityTable = $('validity-table');
const stripAir = $('strip-air');
const stripLevel = $('strip-level');
const stripDrive = $('strip-drive');
const stripEar = $('strip-ear');
const stripRef = $('strip-ref');
const stripRefItem = $('strip-ref-item');
const stripEngine = $('strip-engine');
const tabDesign = $<HTMLButtonElement>('tab-design');
const tabNetlist = $<HTMLButtonElement>('tab-netlist');
const panelDesign = $('panel-design');
const panelNetlist = $('panel-netlist');
const freezeBtn = $<HTMLButtonElement>('freeze-btn');
const warnJumpLink = $<HTMLAnchorElement>('warn-jump-link');

// ----- state ----------------------------------------------------------------

let result: SolveResult | null = null;
let groups: PlotGroup[] = [];
let solvedText: string | null = null;
let lastError: EngineError | null = null;
/** Netlist text of the run that produced `lastError`. */
let lastErrorText: string | null = null;
/** The `ui` block of the solved netlist. */
let resultUi: UiBlock | null = null;
/** Last successful `parameters()` description and the text it describes. */
let paramsDoc: ParamsDoc | null = null;
let paramsText: string | null = null;
/** Solve time of the last result as the worker measured it (ms). */
let lastSolveMs = 0;

const solver = new EngineWorker();
/** Live checks and parameter descriptions; never blocked by a long solve. */
const checker = new EngineWorker();

const panel = new PlotPanel(plotsEl, {
  onCursor: (i, fromKeyboard) => renderReadout(i, fromKeyboard),
  onView: (lo, hi) => {
    viewRange.textContent = `Showing ${formatHz(lo)} to ${formatHz(hi)}`;
    for (const b of document.querySelectorAll<HTMLButtonElement>('[data-view]')) {
      const [a, z] = b.dataset.view === 'full' ? RANGE_FULL : RANGE_AUDIO;
      b.setAttribute('aria-pressed', String(Math.abs(lo / a - 1) < 1e-6 && Math.abs(hi / z - 1) < 1e-6));
    }
    if (!tableWrap.hidden) renderTable();
  },
});

// ----- netlist helpers ------------------------------------------------------

function parseDoc(text: string): Record<string, unknown> | null {
  try {
    const d = JSON.parse(text);
    return typeof d === 'object' && d !== null && !Array.isArray(d) ? d : null;
  } catch {
    return null;
  }
}

function uiOf(doc: Record<string, unknown> | null): UiBlock | null {
  const ui = doc?.ui;
  return typeof ui === 'object' && ui !== null ? (ui as UiBlock) : null;
}

/** Ear-load element types and how the strip names them. */
const EAR_TYPES: Record<string, string> = {
  iec60318_4: 'IEC 60318-4 ear simulator',
  type33: 'ITU-T P.57 Type 3.3 ear',
  type43: 'ITU-T P.57 Type 4.3 ear',
  canal: 'ear canal model',
  eardrum: 'eardrum model',
};

/**
 * The ear load of the solved netlist: the label of the parameter named by
 * `ui.ear_load` at its solved value, else the ear-load elements the netlist
 * enables unconditionally.
 */
function earLoadText(r: SolveResult, doc: Record<string, unknown> | null): string {
  const ui = uiOf(doc);
  const name = ui?.ear_load;
  const v = name ? r.meta.parameters?.[name] : undefined;
  if (name && v !== undefined) {
    const p = paramsDoc?.parameters.find((q) => q.name === name);
    const label = p?.choices?.find((c) => c.value === v)?.label;
    if (label) return label;
  }
  // The resolved netlist's own elements (exact, whatever the parameters).
  const resolved = (r.meta.elements ?? []).filter((e) => e.type in EAR_TYPES);
  if (resolved.length) return resolved.map((e) => `${EAR_TYPES[e.type]} (“${e.id}”)`).join(', ');
  if (name && v !== undefined) return String(v);
  const els = Array.isArray(doc?.elements) ? (doc!.elements as Record<string, unknown>[]) : [];
  const ears = els.filter((e) => typeof e === 'object' && e !== null && String(e.type) in EAR_TYPES);
  const on = ears.filter((e) => e.enabled === undefined || e.enabled === true);
  if (on.length) return on.map((e) => `${EAR_TYPES[String(e.type)]} (“${String(e.id)}”)`).join(', ');
  const cond = ears.filter((e) => typeof e.enabled === 'string');
  if (cond.length) return `${[...new Set(cond.map((e) => EAR_TYPES[String(e.type)]))].join(' or ')}, chosen by parameters`;
  return 'none (no ear-load element)';
}

/** "p_drp at ear.drp" for the primary probe. */
function referenceText(doc: Record<string, unknown> | null, id: string): string {
  const probes = Array.isArray(doc?.probes) ? (doc!.probes as Record<string, unknown>[]) : [];
  const p = probes.find((q) => typeof q === 'object' && q !== null && q.id === id && q.enabled !== false);
  const at = typeof p?.node === 'string' ? p.node : typeof p?.element === 'string' ? `element ${p.element}` : '';
  return at ? `${id} at ${at}` : id;
}

/** Returns a copy with frequencies ascending (a user list may be unsorted). */
function sortResult(r: SolveResult): SolveResult {
  const f = r.frequencies_Hz;
  if (f.every((x, i) => i === 0 || f[i - 1] <= x)) return r;
  const order = f.map((_, i) => i).sort((a, b) => f[a] - f[b]);
  const pick = <T>(a: T[] | undefined) => (a ? order.map((i) => a[i]) : undefined);
  return {
    ...r,
    frequencies_Hz: order.map((i) => f[i]),
    probes: r.probes.map((p) => ({
      ...p,
      re: pick(p.re)!,
      im: pick(p.im)!,
      magnitude: pick(p.magnitude)!,
      phase_deg: pick(p.phase_deg)!,
      spl_dB: pick(p.spl_dB),
    })),
  };
}

// ----- strip, legend, readout, tables --------------------------------------

function renderStrip(r: SolveResult, doc: Record<string, unknown> | null): void {
  const a = r.meta.air;
  stripAir.textContent =
    `${(a.temperature_k - 273.15).toFixed(1)} °C, ${(a.p0 / 1000).toFixed(3)} kPa, ` +
    `ρ ${a.rho.toFixed(4)} kg/m³, c ${a.c.toFixed(2)} m/s`;
  stripLevel.textContent = r.meta.level === 0 ? 'L0 lumped' : `L${r.meta.level} distributed`;
  stripDrive.textContent = r.meta.drive?.label ?? 'not reported by this engine';
  stripEar.textContent = earLoadText(r, doc);
  const primary = uiOf(doc)?.primary_probe;
  stripRefItem.hidden = !primary || !r.probes.some((p) => p.id === primary);
  if (!stripRefItem.hidden) stripRef.textContent = referenceText(doc, primary!);
  stripEngine.textContent = r.meta.engine;
}

function probeUnitLabel(i: number): string {
  const p = result!.probes[i];
  const unit = p.unit ? prettyUnit(p.unit) : 'unit unknown';
  return p.spl_dB ? 'dB SPL' : unit;
}

function primaryProbe(): string | undefined {
  const id = resultUi?.primary_probe;
  return id && result?.probes.some((p) => p.id === id) ? id : undefined;
}

function renderLegend(): void {
  legend.replaceChildren();
  if (!result) return;
  const primary = primaryProbe();
  const order = result.probes.map((_, i) => i);
  if (primary) order.sort((a, b) => Number(result!.probes[b].id === primary) - Number(result!.probes[a].id === primary));
  for (const i of order) {
    const p = result.probes[i];
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'legend-item';
    if (p.id === primary) b.classList.add('is-primary');
    b.dataset.probe = p.id;
    b.setAttribute('aria-pressed', String(!panel.hidden.has(p.id)));
    const name = document.createElement('span');
    name.className = 'legend-name';
    name.textContent = p.id;
    const meta = document.createElement('span');
    meta.className = 'legend-meta';
    meta.textContent = `${p.quantity.replace(/_/g, ' ')}, ${probeUnitLabel(i)}`;
    b.append(lineKey(i), name, meta);
    if (p.id === primary) {
      const tag = document.createElement('span');
      tag.className = 'legend-tag';
      tag.textContent = 'primary';
      b.append(tag);
    }
    b.title = `Show or hide ${p.id}`;
    b.addEventListener('click', () => {
      if (panel.hidden.has(p.id)) panel.hidden.delete(p.id);
      else panel.hidden.add(p.id);
      b.setAttribute('aria-pressed', String(!panel.hidden.has(p.id)));
      panel.render();
      renderReadout(panel.cursor, false);
      if (!tableWrap.hidden) renderTable();
    });
    legend.append(b);
  }
}

function cellValue(g: PlotGroup, v: number | null): string {
  if (v === null || !Number.isFinite(v)) return 'n/a';
  if (g.kind === 'spl') return `${v.toFixed(2)} dB SPL`;
  if (g.kind === 'delta') return `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(2)} dB`;
  if (g.kind === 'phase') return `${v.toFixed(1)}°`;
  return formatValue(v, g.unit, 4);
}

/** "+1.23 dB", "−0.40 dB", "0.00 dB" (no sign on what rounds to zero). */
function signedDb(d: number): string {
  const t = Math.abs(d).toFixed(2);
  return `${Number(t) === 0 ? '' : d > 0 ? '+' : '−'}${t} dB`;
}

function renderReadout(i: number | null, fromKeyboard: boolean): void {
  readout.replaceChildren();
  if (!result || i === null) {
    const p = document.createElement('p');
    p.className = 'hint';
    p.textContent = result
      ? 'Crosshair: point at a plot, or focus one and use the arrow keys, to read every visible curve.'
      : 'Run a netlist to see results.';
    readout.append(p);
    return;
  }
  const f = result.frequencies_Hz[i];
  const note = bandNote(result.shading, f);
  const head = document.createElement('p');
  head.className = 'readout-head';
  const fs = document.createElement('strong');
  fs.textContent = formatHz(f);
  head.append(fs, ` (grid point ${i + 1} of ${result.frequencies_Hz.length}), ${note}`);
  readout.append(head);
  const list = document.createElement('ul');
  list.className = 'readout-list';
  const spoken: string[] = [];
  const base = baselines.reference;
  for (const g of groups) {
    if (g.kind === 'phase' || g.kind === 'delta') continue;
    const vis = g.series.filter((s) => !panel.hidden.has(s.id));
    const ref = vis[0];
    for (const s of vis) {
      const li = document.createElement('li');
      const val = document.createElement('strong');
      let text = cellValue(g, s.values[i]);
      if (g.kind === 'mag' && result.probes[s.probe].quantity === 'impedance') {
        const ph = result.probes[s.probe].phase_deg[i];
        if (ph !== null) text += `, ∠ ${ph.toFixed(1)}°`;
      }
      val.textContent = text;
      const name = document.createElement('span');
      name.className = 'readout-name';
      name.textContent = `${s.id} · ${g.symbol}`;
      li.append(lineKey(s.probe, 20), val, name);
      let said = `${s.id} ${text}`;
      if (g.kind === 'spl' && s !== ref) {
        const a = s.values[i];
        const b = ref.values[i];
        if (a !== null && b !== null) {
          const d = document.createElement('span');
          d.className = 'readout-delta';
          d.textContent = `Δ ${signedDb(a - b)} vs ${ref.id}`;
          li.append(d);
        }
      }
      if (g.kind === 'spl' && base) {
        const q = base.probes.find((p) => p.id === s.id && p.spl_dB);
        const a = s.values[i];
        const b = q ? valueAt(base.freqs, q.spl_dB!, f) : null;
        if (a !== null && b && b.v !== null) {
          const d = document.createElement('span');
          d.className = 'readout-delta readout-base';
          d.textContent = `Δ ${b.exact ? '' : '≈ '}${signedDb(a - b.v)} vs baseline “${base.name}”`;
          li.append(d);
          said += `, ${d.textContent}`;
        }
      }
      list.append(li);
      spoken.push(said);
    }
  }
  readout.append(list);
  if (fromKeyboard) srReadout.textContent = `${formatHz(f)}: ${spoken.join('; ')}. ${note}.`;
}

/**
 * Most rows the data table builds at once. It is rebuilt on the main thread
 * at every zoom, pan and legend toggle; a dense sweep (tens of thousands of
 * points in view) would otherwise freeze the page for seconds, which the
 * worker solve exists to avoid.
 */
const MAX_TABLE_ROWS = 1000;

/** Grid indices the table lists for the view [a, b]: all, or every k-th plus the last. */
function tableRows(a: number, b: number): { rows: number[]; stride: number } {
  const n = b - a + 1;
  const stride = Math.max(1, Math.ceil(n / MAX_TABLE_ROWS));
  const rows: number[] = [];
  for (let i = a; i <= b; i += stride) rows.push(i);
  if (rows[rows.length - 1] !== b) rows.push(b);
  return { rows, stride };
}

function unitHeader(g: PlotGroup): string {
  if (g.kind === 'spl') return 'dB SPL';
  if (g.kind === 'delta') return 'dB';
  if (g.kind === 'phase') return '°';
  return g.unit ? prettyUnit(g.unit) : 'unit unknown';
}

function renderTable(): void {
  tableWrap.replaceChildren();
  if (!result) return;
  const idx = panel.viewIndices();
  const table = document.createElement('table');
  table.className = 'data-table';
  const caption = document.createElement('caption');
  const n = idx ? idx[1] - idx[0] + 1 : 0;
  const { rows, stride } = idx ? tableRows(idx[0], idx[1]) : { rows: [], stride: 1 };
  // Baseline columns only where a baseline shares the grid (no interpolated
  // values in the table).
  const onGrid = baselines.items.filter((b) => sameGrid(b.freqs, result!.frequencies_Hz));
  const offGrid = baselines.items.filter((b) => !onGrid.includes(b));
  caption.textContent =
    `Plotted values from ${formatHz(panel.lo)} to ${formatHz(panel.hi)} (${n} rows` +
    (stride > 1 ? `; ${rows.length} shown, one grid point in ${stride} and the last: zoom in to list them all` : '') +
    '). Validity: "light" = lumped-model error ≥ 10 % or below an element’s validated range, "dark" = ≥ 36 % or past a hard limit.' +
    (offGrid.length ? ` Not listed (different frequency grid): ${offGrid.map((b) => `“${b.name}”`).join(', ')}.` : '');
  table.append(caption);
  type Col = { head: string; kind: PlotGroup['kind']; values: (number | null)[] };
  const cols: Col[] = [];
  for (const g of groups) {
    for (const s of g.series) {
      if (panel.hidden.has(s.id)) continue;
      cols.push({ head: `${s.id} ${g.symbol} (${unitHeader(g)})`, kind: g.kind, values: s.values });
      for (const o of g.overlays) {
        if (o.id !== s.id || !onGrid.some((b) => b.name === o.name && b.freqs === o.freqs)) continue;
        cols.push({ head: `${s.id} ${g.symbol}, baseline “${o.name}” (${unitHeader(g)})`, kind: g.kind, values: o.values });
      }
    }
  }
  const thead = document.createElement('thead');
  const hr = document.createElement('tr');
  const th = (text: string) => {
    const c = document.createElement('th');
    c.scope = 'col';
    c.textContent = text;
    return c;
  };
  hr.append(th('Frequency (Hz)'), th('Validity'));
  for (const c of cols) hr.append(th(c.head));
  thead.append(hr);
  const tbody = document.createElement('tbody');
  for (const i of rows) {
    const f = result.frequencies_Hz[i];
    const tr = document.createElement('tr');
    const rh = document.createElement('th');
    rh.scope = 'row';
    rh.textContent = formatNumber(f, 6);
    const vd = document.createElement('td');
    vd.textContent = bandName(band(result.shading, f));
    tr.append(rh, vd);
    for (const c of cols) {
      const td = document.createElement('td');
      const v = c.values[i];
      td.textContent = c.kind === 'mag' ? formatNumber(v) : v === null ? '' : v.toFixed(2);
      tr.append(td);
    }
    tbody.append(tr);
  }
  table.append(thead, tbody);
  tableWrap.append(table);
}

function renderValidity(r: SolveResult): void {
  validityTable.replaceChildren();
  if (!r.validity.length) {
    validityTable.textContent = 'No element reports a validity limit.';
    return;
  }
  const table = document.createElement('table');
  table.className = 'data-table';
  const caption = document.createElement('caption');
  caption.textContent =
    'Shading uses the lowest upper limits and the highest lower limits over all elements. "Onset" is where the element’s error reaches 10 % (or its criterion starts to bite); "deep" is 36 % or a hard limit. Lower limits shade below their frequency.';
  const head = document.createElement('tr');
  for (const t of ['Element', 'Criterion', 'Onset', 'Deep', 'Lower onset', 'Lower deep']) {
    const c = document.createElement('th');
    c.scope = 'col';
    c.textContent = t;
    head.append(c);
  }
  const thead = document.createElement('thead');
  thead.append(head);
  const tbody = document.createElement('tbody');
  const hz = (f: number | null | undefined) => (f === null || f === undefined ? 'none' : formatHz(f));
  const rows = [...r.validity].sort((a, b) => (a.begin_hz ?? Infinity) - (b.begin_hz ?? Infinity));
  for (const v of rows) {
    const tr = document.createElement('tr');
    for (const t of [v.element, v.criterion, hz(v.begin_hz), hz(v.deep_hz), hz(v.low_begin_hz), hz(v.low_deep_hz)]) {
      const td = document.createElement('td');
      td.textContent = t;
      tr.append(td);
    }
    tbody.append(tr);
  }
  table.append(caption, thead, tbody);
  validityTable.append(table);
}

// ----- warnings -------------------------------------------------------------

const warnings = new WarningsPanel(
  {
    drive: $('warnings-drive'),
    list: $('warning-list'),
    none: $('warnings-none'),
    notes: $<HTMLDetailsElement>('notes-details'),
    notesSummary: $('notes-summary'),
    noteList: $('note-list'),
    jump: $('warn-jump'),
    jumpLink: warnJumpLink,
  },
  (w, fromClick) => {
    panel.setHighlight(w ? highlightOf(w) : null);
    // A click also puts the crosshair on the worst point.
    if (fromClick && w && w.at_Hz !== null && result) {
      const f = result.frequencies_Hz;
      let k = 0;
      for (let i = 1; i < f.length; i++) if (Math.abs(Math.log(f[i] / w.at_Hz)) < Math.abs(Math.log(f[k] / w.at_Hz))) k = i;
      panel.setCursor(k, false);
    }
  },
);

// ----- baselines ------------------------------------------------------------

const baselines = new Baselines(
  {
    list: $('baseline-list'),
    options: $('baseline-opts'),
    reference: $<HTMLSelectElement>('delta-ref'),
    difference: $<HTMLInputElement>('diff-toggle'),
    freeze: freezeBtn,
  },
  () => regroup(),
);

/** Default name of a snapshot: the parameters that differ from the template. */
function defaultBaselineName(): string {
  const described = paramsDoc !== null && paramsText === solvedText && paramsDoc.parameters.length > 0;
  if (!described) return `snapshot ${baselines.items.length + 1}`;
  const values = result?.meta.parameters ?? {};
  const parts = design
    .changed()
    .map(([n]) => `${n}=${typeof values[n] === 'number' ? formatParam(values[n] as number, 4) : String(values[n])}`);
  if (!parts.length) return 'template values';
  return parts.slice(0, 3).join(', ') + (parts.length > 3 ? ` +${parts.length - 3} more` : '');
}

/** Rebuilds the plot groups (live curves, baselines, difference plot) and everything drawn from them. */
function regroup(): void {
  if (!result) return;
  groups = buildGroups(result, {
    primary: primaryProbe(),
    baselines: baselines.items,
    difference: baselines.difference,
  });
  panel.setGroups(groups, { freqs: result.frequencies_Hz, shading: result.shading, highlight: panel.data?.highlight ?? null });
  if (!groups.length) {
    const p = document.createElement('p');
    p.className = 'hint';
    p.textContent = 'This netlist has no probes. Add one to "probes" to plot results.';
    plotsEl.append(p);
  }
  panel.setView(panel.lo, panel.hi);
  renderLegend();
  renderReadout(panel.cursor, false);
  if (!tableWrap.hidden) renderTable();
}

// ----- errors ---------------------------------------------------------------

function errorDetailText(e: EngineError): string {
  const parts: string[] = [];
  if (e.element) parts.push(`element "${e.element}"`);
  if (e.type) parts.push(`type "${e.type}"`);
  if (e.probe) parts.push(`probe "${e.probe}"`);
  if (e.node) parts.push(`node "${e.node}"`);
  if (e.parameter) parts.push(`parameter "${e.parameter}"`);
  if (e.line) parts.push(`line ${e.line}, column ${e.column ?? 1}`);
  if (e.f_Hz !== undefined) parts.push(`at ${formatHz(e.f_Hz)}`);
  return parts.length ? `Where: ${parts.join(', ')}.` : '';
}

/** Character range in the editor for an error, or null. */
function errorRange(e: EngineError, text: string): [number, number] | null {
  const lineRange = (offset: number): [number, number] => {
    const a = text.lastIndexOf('\n', offset - 1) + 1;
    const z = text.indexOf('\n', offset);
    return [a, z < 0 ? text.length : z];
  };
  if (e.line) {
    let off = 0;
    for (let l = 1; l < e.line; l++) off = text.indexOf('\n', off) + 1;
    return lineRange(off);
  }
  const find = (section: string, id: string) => {
    const start = Math.max(0, text.search(new RegExp(`"${section}"\\s*:`)));
    const esc = id.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const re = new RegExp(`"id"\\s*:\\s*"${esc}"`, 'g');
    re.lastIndex = start;
    const m = re.exec(text);
    return m ? lineRange(m.index) : null;
  };
  if (e.element) return find('elements', e.element);
  if (e.probe) return find('probes', e.probe);
  if (e.node) return find('nodes', e.node);
  if (e.parameter) {
    try {
      const s = paramDeclSpan(scan(text), e.parameter);
      return s ? lineRange(s.start) : null;
    } catch {
      return null;
    }
  }
  return null;
}

function showError(e: EngineError, text: string): void {
  lastError = e;
  lastErrorText = text;
  errorMessage.textContent = e.error;
  errorDetail.textContent = errorDetailText(e);
  errorGoto.hidden = errorRange(e, editor.value) === null;
  errorBox.hidden = false;
  plotsEl.classList.toggle('stale', result !== null);
}

function clearError(): void {
  lastError = null;
  lastErrorText = null;
  errorBox.hidden = true;
  plotsEl.classList.remove('stale');
}

/** Switches to the Netlist tab and selects [a, b) in the editor. */
function selectInEditor(a: number, b: number): void {
  selectTab('netlist');
  editor.focus();
  editor.setSelectionRange(a, b);
  const line = editor.value.slice(0, a).split('\n').length - 1;
  const lh = parseFloat(getComputedStyle(editor).lineHeight) || 18;
  editor.scrollTop = Math.max(0, (line - 3) * lh);
  updateCursorPos();
}

function gotoError(): void {
  if (!lastError) return;
  const r = errorRange(lastError, editor.value);
  if (r) selectInEditor(r[0], r[1]);
}

// ----- solve ----------------------------------------------------------------

function setBusy(busy: boolean): void {
  cancelBtn.disabled = !busy;
  runBtn.setAttribute('aria-busy', String(busy));
  plotsEl.setAttribute('aria-busy', String(busy));
  plotPanelEl.classList.toggle('busy', busy);
}

/** Shows a result; `settled`: no newer solve is on its way (only then is the status announced). */
function setResult(r: SolveResult, text: string, ms: number, settled: boolean): void {
  result = sortResult(r);
  solvedText = text;
  lastSolveMs = ms;
  const doc = parseDoc(text);
  resultUi = uiOf(doc);
  clearError();
  renderStrip(result, doc);
  resultTitle.textContent = typeof doc?.title === 'string' ? doc.title : '';
  // Keep hidden probes that still exist.
  const ids = new Set(result.probes.map((p) => p.id));
  for (const h of [...panel.hidden]) if (!ids.has(h)) panel.hidden.delete(h);
  freezeBtn.disabled = false;
  regroup();
  renderValidity(result);
  warnings.render(result);
  refreshActiveView();
  if (settled) {
    // The status region is announced: a design change that crosses an
    // operating limit is heard, not only seen in the warnings list.
    const ops = (result.warnings ?? []).filter(isOperating).length;
    runStatus.textContent =
      `Solved ${result.frequencies_Hz.length} frequencies × ${result.probes.length} probes in ${ms.toFixed(0)} ms.` +
      (ops ? ` ${ops} operating limit${ops === 1 ? '' : 's'} exceeded at the stated drive.` : '');
  }
}

/** A solve reply for `text`; `superseded`: a newer text is already being solved. */
function onSolved(text: string, reply: Reply, superseded: boolean): void {
  setBusy(superseded);
  if (!reply.ok) {
    showError({ error: reply.crash, kind: 'panic' }, text);
    runStatus.textContent = 'The engine failed; see the error below.';
    document.body.dataset.state = superseded ? 'solving' : 'error';
    return;
  }
  if (isError(reply.value)) {
    showError(reply.value, text);
    if (!superseded) runStatus.textContent = result ? 'Run failed; showing the last successful result (dimmed).' : 'Run failed.';
    document.body.dataset.state = superseded ? 'solving' : 'error';
    return;
  }
  setResult(reply.value as SolveResult, text, reply.ms, !superseded);
  document.body.dataset.state = superseded ? 'solving' : 'solved';
}

const live = new Coalesced(solver, 'solve', onSolved);

/** Solves the current text now, abandoning any solve in flight (Run, Ctrl+Enter, loading). */
function run(): void {
  setBusy(true);
  runStatus.textContent = 'Solving…';
  document.body.dataset.state = 'solving';
  live.restart(editor.value);
}

/** Solves the current text as soon as the engine is free (design edits: coalesced). */
function solveLive(): void {
  setBusy(true);
  document.body.dataset.state = 'solving';
  live.request(editor.value);
}

// ----- live check -----------------------------------------------------------

let checkTimer = 0;
let checkSeq = 0;
function scheduleCheck(): void {
  clearTimeout(checkTimer);
  checkTimer = window.setTimeout(async () => {
    const seq = ++checkSeq;
    const text = editor.value;
    const reply = await checker.call('check', text);
    // Drop replies overtaken by a newer check or by further edits.
    if (seq !== checkSeq || text !== editor.value) return;
    if (!reply.ok) {
      checkStatus.textContent = `Check failed: ${reply.crash}`;
      checkStatus.dataset.state = 'error';
      return;
    }
    const v = reply.value as CheckResult | EngineError;
    if (isError(v)) {
      const d = errorDetailText(v);
      checkStatus.textContent = `Not valid: ${v.error}${d ? ` ${d}` : ''}`;
      checkStatus.dataset.state = 'error';
    } else {
      checkStatus.textContent =
        `Valid: ${v.nodes} nodes, ${v.elements} elements, ${v.unknowns} unknowns, ` +
        `${v.probes} probes, ${v.frequencies} frequencies (${formatHz(v.f_min_Hz)} to ${formatHz(v.f_max_Hz)}), L${v.level}.` +
        (solvedText !== null && text !== solvedText ? ' Edited since the last run.' : '');
      checkStatus.dataset.state = 'ok';
      // A passing check only vouches for errors it can detect, and only for
      // edited text: a run's error on this very text (a singular system, a
      // panic, anything found only while solving) must stay visible.
      if (lastError && text !== lastErrorText && lastError.kind !== 'singular' && lastError.kind !== 'panic') {
        clearError();
      }
    }
  }, 300);
}

function updateCursorPos(): void {
  const before = editor.value.slice(0, editor.selectionStart);
  const line = before.split('\n').length;
  const col = editor.selectionStart - before.lastIndexOf('\n');
  cursorPos.textContent = `Ln ${line}, Col ${col}`;
}

// ----- design panel and sketch ----------------------------------------------

const design = new DesignPanel(
  $('design-groups'),
  $('design-message'),
  $<HTMLButtonElement>('detail-toggle'),
  $<HTMLButtonElement>('reset-all'),
  {
    set: (values) => applyParams(values),
    hover: (name) => sketch.highlightParam(name),
    showInNetlist: (name) => {
      try {
        const s = paramDeclSpan(scan(editor.value), name);
        if (s) selectInEditor(s.start, s.end);
      } catch {
        selectTab('netlist');
      }
    },
  },
);

const sketch = new Sketch($('sketch'), $('sketch-desc'), $('sketch-note'), {
  hoverPart: (part) => {
    sketch.highlightParts(part ? new Set([part]) : new Set());
    design.highlight(part ? sketch.paramsOf(part) : new Set());
  },
  clickPart: (part: Part) => {
    const names = sketch.paramsOf(part);
    const first = design.params.find((p) => names.has(p.name) && p.kind !== 'derived' && !p.advanced) ?? design.params.find((p) => names.has(p.name));
    if (first) design.focusParam(first.name);
  },
});

/** Signature of the sketch binding and parameter set, to reconfigure only on change. */
let sketchSig = '';

/** Netlist text as the design panel last wrote it. */
let designText: string | null = null;

/** Writes parameter values into the netlist text and solves it. */
function applyParams(values: [string, Scalar][]): void {
  // The panel may only edit text it has seen: the text its controls were
  // built from (`paramsText`) or its own last write. After a hand edit or a
  // load, until the engine has described the new text, a control still
  // shows the old values, and a stepper or reset computed from them would
  // overwrite the new text's values. The edit is dropped; the pending
  // description then puts the controls right.
  if (editor.value !== paramsText && editor.value !== designText) {
    params.request(editor.value);
    return;
  }
  let text: string;
  try {
    text = setParams(editor.value, values);
  } catch (e) {
    design.showMessage([`The netlist text could not be updated: ${(e as Error).message}. Fix it in the Netlist tab.`]);
    return;
  }
  if (text === editor.value) return;
  designText = text;
  editor.value = text;
  hideRestore();
  saveSoon();
  updateCursorPos();
  scheduleCheck();
  params.request(text);
  solveLive();
}

function onParams(text: string, reply: Reply): void {
  const fresh = text === editor.value;
  if (fresh) {
    // The netlist's own description (its provenance), as written.
    const d = parseDoc(text)?.description;
    $('design-about').hidden = typeof d !== 'string' || !d.trim();
    $('design-about-text').textContent = typeof d === 'string' ? d : '';
  }
  if (!reply.ok) {
    if (fresh) design.showMessage([`The engine could not read the parameters: ${reply.crash}`]);
    return;
  }
  const v = reply.value;
  if (isError(v)) {
    if (!fresh) return;
    const go = document.createElement('button');
    go.type = 'button';
    go.textContent = 'Open the Netlist tab';
    go.addEventListener('click', () => selectTab('netlist', true));
    const d = errorDetailText(v);
    design.showMessage([`The netlist cannot be read, so its parameters cannot be shown: ${v.error}${d ? ` ${d}` : ''} `, go]);
    sketch.configure(undefined, undefined, []);
    sketchSig = '';
    return;
  }
  const doc = v as ParamsDoc;
  paramsDoc = doc;
  paramsText = text;
  // A solve may have landed first: its strip then lacks the choice labels.
  if (result && solvedText === text) renderStrip(result, parseDoc(solvedText));
  if (!doc.parameters.length) {
    design.update(doc, fresh);
    design.showMessage([
      'This netlist declares no parameters. Open a design template, or declare a "parameters" block in the netlist (docs/parameters.md) to get controls here.',
    ]);
    sketch.configure(undefined, undefined, []);
    sketchSig = '';
    return;
  }
  design.showMessage(null);
  const sig = JSON.stringify([doc.ui?.sketch ?? null, doc.ui?.ear_load ?? null, doc.parameters.map((p) => [p.name, p.expr ?? '', p.choices ?? []])]);
  if (sig !== sketchSig) {
    sketchSig = sig;
    sketch.configure(doc.ui?.sketch, doc.ui?.ear_load, doc.parameters);
  }
  design.update(doc, fresh);
  sketch.update(design.values());
}

const params = new Coalesced(checker, 'parameters', onParams);

/** Template values of the example the netlist came from: what "Reset" restores and the sketch's scale. */
let referenceSeq = 0;
/** Declared parameter values of the loaded template (what "Reset" restores). */
let referenceValues: Map<string, Scalar> | null = null;
async function setReferenceFrom(exampleText: string | null): Promise<void> {
  const seq = ++referenceSeq;
  referenceValues = exampleText ? declaredValues(exampleText) : null;
  design.setReference(referenceValues ?? new Map());
  sketch.setReference(null);
  if (!exampleText || !hasParameters(exampleText)) return;
  const reply = await checker.call('parameters', exampleText);
  // A later load has set its own reference meanwhile.
  if (seq !== referenceSeq || !reply.ok || isError(reply.value)) return;
  const doc = reply.value as ParamsDoc;
  sketch.setReference(new Map(doc.parameters.filter((p) => p.value !== null).map((p) => [p.name, p.value as Scalar])));
  sketch.update(design.values());
}

// ----- tabs -----------------------------------------------------------------

type Tab = 'design' | 'netlist';

/** `refresh`: bring the design view up to the text (not when new text is about to be loaded). */
function selectTab(tab: Tab, focus = false, refresh = true): void {
  const isDesign = tab === 'design';
  tabDesign.setAttribute('aria-selected', String(isDesign));
  tabNetlist.setAttribute('aria-selected', String(!isDesign));
  tabDesign.tabIndex = isDesign ? 0 : -1;
  tabNetlist.tabIndex = isDesign ? -1 : 0;
  panelDesign.hidden = !isDesign;
  panelNetlist.hidden = isDesign;
  document.body.dataset.tab = tab;
  // A solve error shows beside what the user is looking at: above the
  // (dimmed) plots while designing, above the editor in the Netlist tab.
  if (isDesign) $('result-title').after(errorBox);
  else tabDesign.parentElement!.after(errorBox);
  if (focus) (isDesign ? tabDesign : tabNetlist).focus();
  if (isDesign && refresh) {
    // The design view is live: bring it and the plots up to the text. A
    // solve still running for older text (a Run from the Netlist tab) is
    // followed by one for this text, never left to stand for it.
    const text = editor.value;
    if (paramsText !== text) params.request(text);
    if (text.trim() && text !== solvedText && text !== lastErrorText) solveLive();
  }
}

for (const t of [tabDesign, tabNetlist]) {
  t.addEventListener('click', () => selectTab(t === tabDesign ? 'design' : 'netlist'));
  t.addEventListener('keydown', (ev) => {
    const keys: Record<string, Tab> = {
      ArrowLeft: t === tabDesign ? 'netlist' : 'design',
      ArrowRight: t === tabDesign ? 'netlist' : 'design',
      Home: 'design',
      End: 'netlist',
    };
    if (!(ev.key in keys)) return;
    ev.preventDefault();
    selectTab(keys[ev.key], true);
  });
}

// ----- wiring -----------------------------------------------------------------

// Loading an example over an edited netlist keeps the edits one click away
// instead of asking first: embedded viewers (such as a published artifact)
// suppress window.confirm, which would make the example picker silently do
// nothing.
const restoreNote = $<HTMLParagraphElement>('restore-note');
const restoreText = $<HTMLSpanElement>('restore-text');
const restoreBtn = $<HTMLButtonElement>('restore-btn');
let replacedText: string | null = null;

function hideRestore(): void {
  replacedText = null;
  restoreNote.hidden = true;
}

let saveTimer = 0;
function saveSoon(): void {
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => store.set('netlist', editor.value), 500);
}

/** Replaces the netlist text (loading or restoring) and solves it. */
function loadText(text: string): void {
  editor.value = text;
  store.set('netlist', text);
  updateCursorPos();
  scheduleCheck();
  params.request(text);
  run();
}

function loadExample(name: string): void {
  const ex = EXAMPLES.find((e) => e.name === name);
  if (!ex) return;
  const edited = editor.value.trim() !== '' && !EXAMPLES.some((e) => e.text === editor.value);
  if (edited) {
    replacedText = editor.value;
    restoreText.textContent = `Loaded "${name}" over your edited netlist.`;
    restoreNote.hidden = false;
  } else {
    hideRestore();
  }
  exampleSelect.value = name;
  store.set('example', name);
  selectTab(ex.parametric ? 'design' : 'netlist', false, false);
  void setReferenceFrom(ex.text);
  loadText(ex.text);
}

{
  const groupsOf: [string, boolean][] = [
    ['Design templates', true],
    ['Example netlists', false],
  ];
  for (const [label, template] of groupsOf) {
    const list = EXAMPLES.filter((e) => e.template === template);
    if (!list.length) continue;
    const og = document.createElement('optgroup');
    og.label = label;
    for (const ex of list) {
      const o = document.createElement('option');
      o.value = ex.name;
      o.textContent = ex.title ? `${ex.name}: ${ex.title}` : ex.name;
      og.append(o);
    }
    exampleSelect.append(og);
  }
}
exampleSelect.addEventListener('change', () => loadExample(exampleSelect.value));

runBtn.addEventListener('click', () => run());
cancelBtn.addEventListener('click', () => {
  live.cancel();
  setBusy(false);
  runStatus.textContent = 'Cancelled.';
  document.body.dataset.state = 'cancelled';
});
errorGoto.addEventListener('click', gotoError);

restoreBtn.addEventListener('click', () => {
  if (replacedText === null) return;
  const text = replacedText;
  hideRestore();
  selectTab(hasParameters(text) ? 'design' : 'netlist', false, false);
  loadText(text);
  if (!panelNetlist.hidden) editor.focus();
});

let editParamsTimer = 0;
editor.addEventListener('input', () => {
  hideRestore();
  scheduleCheck();
  updateCursorPos();
  saveSoon();
  // Hand edits reach the design panel too (debounced like the check).
  clearTimeout(editParamsTimer);
  editParamsTimer = window.setTimeout(() => params.request(editor.value), 250);
});
editor.addEventListener('keydown', (ev) => {
  if (ev.key === 'Enter' && (ev.ctrlKey || ev.metaKey)) {
    ev.preventDefault();
    run();
  }
});
for (const t of ['keyup', 'click', 'select']) editor.addEventListener(t, updateCursorPos);

for (const b of document.querySelectorAll<HTMLButtonElement>('[data-view]')) {
  b.addEventListener('click', () => panel.setView(...(b.dataset.view === 'full' ? RANGE_FULL : RANGE_AUDIO)));
}
const actions: Record<string, () => void> = {
  'zoom-in': () => panel.zoom(0.5),
  'zoom-out': () => panel.zoom(2),
  'pan-left': () => panel.pan(-0.25),
  'pan-right': () => panel.pan(0.25),
};
for (const b of document.querySelectorAll<HTMLButtonElement>('[data-action]')) {
  b.addEventListener('click', () => actions[b.dataset.action!]?.());
}

tableToggle.addEventListener('click', () => {
  const open = tableWrap.hidden;
  tableWrap.hidden = !open;
  tableToggle.setAttribute('aria-expanded', String(open));
  tableToggle.textContent = open ? 'Hide data table' : 'Show data table';
  if (open) renderTable();
});

freezeBtn.addEventListener('click', () => {
  if (result && solvedText !== null) baselines.add(result, solvedText, defaultBaselineName());
});
warnJumpLink.addEventListener('click', (ev) => {
  ev.preventDefault();
  const w = $('warnings');
  w.scrollIntoView({ block: 'start' });
  w.focus({ preventScroll: true });
});

// Test and debugging hook (read-only view of the current state).
Object.defineProperty(window, 'acoustilab', {
  value: Object.freeze({
    result: () => result,
    groups: () =>
      groups.map((g) => ({
        key: g.key,
        kind: g.kind,
        title: g.title,
        scale: g.scale,
        series: g.series.map((s) => s.id),
        overlays: g.overlays.map((o) => `${o.id}@${o.name}`),
      })),
    /** Values of one plotted series (live curves, and the difference plot's). */
    values: (key: string, id: string) => groups.find((g) => g.key === key)?.series.find((s) => s.id === id)?.values ?? null,
    view: () => [panel.lo, panel.hi],
    cursor: () => panel.cursor,
    cursorY: () => panel.plots.map((p) => ({ key: p.group.key, y: { ...p.lastCursorY } })),
    /** Requests posted to the solve worker (live edits are coalesced). */
    solves: () => solver.posted.solve ?? 0,
    solveMs: () => lastSolveMs,
    /** Parameter values the design panel shows, by name. */
    params: () => Object.fromEntries(design.values()),
    baselines: () => baselines.items.map((b) => ({ id: b.id, name: b.name })),
    highlight: () => panel.data?.highlight ?? null,
    /** Sketch parts drawn, and those glowing. */
    sketch: () => ({
      parts: [...document.querySelectorAll<SVGElement>('#sketch-svg [data-part]')].map((g) => g.dataset.part),
      glow: [...document.querySelectorAll<SVGElement>('#sketch-svg [data-part].glow')].map((g) => g.dataset.part),
    }),
    /** RGB pixels of a (2h+1)² block of a plot at frequency f, fraction t down the plot area. */
    sample: (key: string, f: number, t: number, h = 2): number[][] => {
      const p = panel.plots.find((q) => q.group.key === key);
      if (!p) return [];
      panel.renderNow();
      const dpr = window.devicePixelRatio || 1;
      const r = p.plotRect();
      const x = Math.round(p.xOf(f, panel.lo, panel.hi) * dpr);
      const y = Math.round((r.y0 + t * (r.y1 - r.y0)) * dpr);
      const d = p.canvas.getContext('2d')!.getImageData(x - h, y - h, 2 * h + 1, 2 * h + 1).data;
      const out: number[][] = [];
      for (let k = 0; k < d.length; k += 4) out.push([d[k], d[k + 1], d[k + 2]]);
      return out;
    },
    /**
     * One full-width row of device pixels of a plot, fraction t down the plot
     * area, with the plot-area box in CSS px. Lets a test locate what is drawn
     * without going through the plot's own frequency-to-x mapping.
     */
    row: (key: string, t: number): { dpr: number; x0: number; x1: number; y0: number; y1: number; px: number[][] } | null => {
      const p = panel.plots.find((q) => q.group.key === key);
      if (!p) return null;
      panel.renderNow();
      const dpr = window.devicePixelRatio || 1;
      const r = p.plotRect();
      const y = Math.round((r.y0 + t * (r.y1 - r.y0)) * dpr);
      const d = p.canvas.getContext('2d')!.getImageData(0, y, p.canvas.width, 1).data;
      const px: number[][] = [];
      for (let k = 0; k < d.length; k += 4) px.push([d[k], d[k + 1], d[k + 2]]);
      return { dpr, ...r, px };
    },
    /** Number of pixels of a plot within `tol` (sum of |ΔRGB|) of a CSS colour variable. */
    countColor: (key: string, cssVar: string, tol = 30): number => {
      const p = panel.plots.find((q) => q.group.key === key);
      if (!p) return 0;
      panel.renderNow();
      const hex = getComputedStyle(document.documentElement).getPropertyValue(cssVar).trim();
      const c = [1, 3, 5].map((k) => parseInt(hex.slice(k, k + 2), 16));
      const d = p.canvas.getContext('2d')!.getImageData(0, 0, p.canvas.width, p.canvas.height).data;
      let n = 0;
      for (let k = 0; k < d.length; k += 4) {
        if (Math.abs(d[k] - c[0]) + Math.abs(d[k + 1] - c[1]) + Math.abs(d[k + 2] - c[2]) <= tol) n++;
      }
      return n;
    },
  }),
});

// ----- start ------------------------------------------------------------------

// The sticky model panel sits below the sticky header, whose height varies
// with the strip's wrapping.
new ResizeObserver(([e]) => {
  document.documentElement.style.setProperty('--header-h', `${Math.ceil(e.borderBoxSize?.[0]?.blockSize ?? e.contentRect.height)}px`);
}).observe(document.querySelector('.app-header')!);
// ----- result views (src/views/*.view.ts) ------------------------------------

const resultTabs = $('result-tabs');
const responseTab = $<HTMLButtonElement>('view-tab-response');
const responsePanel = $('view-response');
const viewStatus = $('view-status');

interface MountedView {
  view: ResultView;
  tab: HTMLButtonElement;
  panel: HTMLElement;
  mounted: boolean;
  worker: EngineWorker | null;
}

const views: MountedView[] = VIEWS.map((view) => {
  const tab = document.createElement('button');
  tab.type = 'button';
  tab.setAttribute('role', 'tab');
  tab.id = `view-tab-${view.id}`;
  tab.setAttribute('aria-controls', `view-${view.id}`);
  tab.setAttribute('aria-selected', 'false');
  tab.tabIndex = -1;
  tab.textContent = view.label;
  resultTabs.append(tab);
  const panelEl = document.createElement('div');
  panelEl.setAttribute('role', 'tabpanel');
  panelEl.id = `view-${view.id}`;
  panelEl.className = 'tabpanel result-view';
  panelEl.setAttribute('aria-labelledby', tab.id);
  panelEl.hidden = true;
  responsePanel.parentElement!.append(panelEl);
  return { view, tab, panel: panelEl, mounted: false, worker: null };
});
resultTabs.hidden = views.length === 0;

let activeView: MountedView | null = null;

function hostFor(v: MountedView): ViewHost {
  return {
    netlist: () => editor.value,
    current: () => (result && solvedText !== null ? { result, text: solvedText } : null),
    parameters: () => (paramsDoc && paramsText === editor.value ? paramsDoc : null),
    reference: () => referenceValues,
    baselines: () => baselines.items.map((b) => ({ id: b.id, name: b.name, text: b.text })),
    deltaReference: () => baselines.reference?.id ?? null,
    call: (fn, ...args) => (v.worker ??= new EngineWorker()).invoke(fn, ...args),
    cancel: () => v.worker?.cancel(),
    setParameters: (values) => applyParams(values),
    announce: (message) => {
      viewStatus.textContent = message;
    },
  };
}

/** Shows the result view `id` ('response' for the plots). */
function selectView(id: string, focus = false): void {
  const next = views.find((v) => v.view.id === id) ?? null;
  if (activeView && activeView !== next) activeView.view.hide?.();
  activeView = next;
  responseTab.setAttribute('aria-selected', String(next === null));
  responseTab.tabIndex = next === null ? 0 : -1;
  responsePanel.hidden = next !== null;
  for (const v of views) {
    const on = v === next;
    v.tab.setAttribute('aria-selected', String(on));
    v.tab.tabIndex = on ? 0 : -1;
    v.panel.hidden = !on;
  }
  if (next && !next.mounted) {
    next.view.mount(next.panel, hostFor(next));
    next.mounted = true;
  }
  if (next) next.view.refresh();
  else panel.render();
  if (focus) (next?.tab ?? responseTab).focus();
  store.set('resultView', id);
}

function refreshActiveView(): void {
  activeView?.view.refresh();
}

{
  const allTabs = () => [responseTab, ...views.map((v) => v.tab)];
  const idOf = (t: HTMLElement) => (t === responseTab ? 'response' : t.id.replace(/^view-tab-/, ''));
  for (const t of allTabs()) {
    t.addEventListener('click', () => selectView(idOf(t)));
    t.addEventListener('keydown', (ev) => {
      const tabs = allTabs();
      const i = tabs.indexOf(t);
      const j =
        ev.key === 'ArrowRight' ? (i + 1) % tabs.length
        : ev.key === 'ArrowLeft' ? (i - 1 + tabs.length) % tabs.length
        : ev.key === 'Home' ? 0
        : ev.key === 'End' ? tabs.length - 1
        : -1;
      if (j < 0) return;
      ev.preventDefault();
      selectView(idOf(tabs[j]), true);
    });
  }
  const savedView = store.get('resultView');
  if (savedView && views.some((v) => v.view.id === savedView)) selectView(savedView);
}

// Height of the sticky sketch (0 while hidden), for the panel's scroll padding.
new ResizeObserver(([e]) => {
  const h = (e.target as HTMLElement).hidden ? 0 : Math.ceil(e.borderBoxSize?.[0]?.blockSize ?? e.contentRect.height);
  document.documentElement.style.setProperty('--sketch-h', `${h}px`);
}).observe($('sketch'));

void checker.call('version').then((r) => {
  if (r.ok) stripEngine.textContent = String(r.value);
});

panel.setView(...RANGE_AUDIO);
renderReadout(null, false);
const saved = store.get('netlist');
const savedExample = store.get('example');
const startExample = EXAMPLES.find((e) => e.name === savedExample);
if (startExample) exampleSelect.value = startExample.name;
if (saved) {
  selectTab(hasParameters(saved) ? 'design' : 'netlist', false, false);
  void setReferenceFrom(startExample?.text ?? null);
  loadText(saved);
} else if (EXAMPLES.length) {
  const first = EXAMPLES.find((e) => e.name === 'design_over_ear') ?? EXAMPLES.find((e) => e.name === 'sealed_cup') ?? EXAMPLES[0];
  loadExample(first.name);
}
