// AcoustiLab web UI: netlist editor, solve in a Web Worker, Canvas 2D plots.

import './style.css';
import { Cancelled, EngineWorker, type Reply } from './engine';
import { EXAMPLES } from './examples';
import { formatHz, formatNumber, formatValue, prettyUnit } from './format';
import { PlotPanel, RANGE_AUDIO, RANGE_FULL } from './plot';
import { buildGroups, DASHES, styleSlot, type PlotGroup } from './series';
import { isError, type CheckResult, type EngineError, type SolveResult } from './types';

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
const viewRange = $('view-range');
const tableToggle = $<HTMLButtonElement>('table-toggle');
const tableWrap = $('table-wrap');
const validityTable = $('validity-table');
const stripAir = $('strip-air');
const stripLevel = $('strip-level');
const stripDrive = $('strip-drive');
const stripEngine = $('strip-engine');

// ----- state ----------------------------------------------------------------

let result: SolveResult | null = null;
let groups: PlotGroup[] = [];
let solvedText: string | null = null;
let lastError: EngineError | null = null;
/** Netlist text of the run that produced `lastError`. */
let lastErrorText: string | null = null;

const solver = new EngineWorker();
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

// ----- persistence (per-viewer convenience only) ----------------------------

const store = {
  get(key: string): string | null {
    try {
      return localStorage.getItem(`acoustilab.${key}`);
    } catch {
      return null;
    }
  },
  set(key: string, value: string): void {
    try {
      localStorage.setItem(`acoustilab.${key}`, value);
    } catch {
      /* storage unavailable: nothing to remember */
    }
  },
};

// ----- netlist helpers ------------------------------------------------------

const SOURCE_TYPES = new Set(['vsource', 'isource', 'flow_source', 'pressure_source', 'force_source']);

/** Independent sources of a netlist, with their parameters as written. */
function driveSummary(text: string): string {
  try {
    const doc = JSON.parse(text);
    const els: unknown[] = Array.isArray(doc?.elements) ? doc.elements : [];
    const parts = els
      .filter((e): e is Record<string, unknown> => typeof e === 'object' && e !== null)
      .filter((e) => SOURCE_TYPES.has(String(e.type)))
      .map((e) => {
        const params = Object.entries(e)
          .filter(([k]) => !['id', 'type', 'node', 'nodes'].includes(k))
          .map(([k, v]) => `${k} ${JSON.stringify(v)}`);
        return `${String(e.id)} (${String(e.type)}${params.length ? `: ${params.join(', ')}` : ', default value'})`;
      });
    return parts.join('; ') || 'no independent source';
  } catch {
    return 'n/a';
  }
}

function netlistTitle(text: string): string {
  try {
    const t = JSON.parse(text)?.title;
    return typeof t === 'string' ? t : '';
  } catch {
    return '';
  }
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

function renderStrip(r: SolveResult, text: string): void {
  const a = r.meta.air;
  stripAir.textContent =
    `${(a.temperature_k - 273.15).toFixed(1)} °C, ${(a.p0 / 1000).toFixed(3)} kPa, ` +
    `ρ ${a.rho.toFixed(4)} kg/m³, c ${a.c.toFixed(2)} m/s`;
  stripLevel.textContent = r.meta.level === 0 ? 'L0 lumped' : `L${r.meta.level} distributed`;
  stripDrive.textContent = driveSummary(text);
  stripEngine.textContent = r.meta.engine;
}

function lineKey(probe: number, width = 28): SVGSVGElement {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('width', String(width));
  svg.setAttribute('height', '10');
  svg.setAttribute('aria-hidden', 'true');
  svg.classList.add('line-key');
  const line = document.createElementNS(ns, 'line');
  line.setAttribute('x1', '0');
  line.setAttribute('x2', String(width));
  line.setAttribute('y1', '5');
  line.setAttribute('y2', '5');
  const st = styleSlot(probe);
  line.setAttribute('style', `stroke: var(--series-${st.color + 1}); stroke-width: 2.5`);
  if (DASHES[st.dash].length) line.setAttribute('stroke-dasharray', DASHES[st.dash].join(' '));
  svg.append(line);
  return svg;
}

function probeUnitLabel(i: number): string {
  const p = result!.probes[i];
  const unit = p.unit ? prettyUnit(p.unit) : 'unit unknown';
  return p.spl_dB ? 'dB SPL' : unit;
}

function renderLegend(): void {
  legend.replaceChildren();
  if (!result) return;
  result.probes.forEach((p, i) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'legend-item';
    b.dataset.probe = p.id;
    b.setAttribute('aria-pressed', String(!panel.hidden.has(p.id)));
    const name = document.createElement('span');
    name.className = 'legend-name';
    name.textContent = p.id;
    const meta = document.createElement('span');
    meta.className = 'legend-meta';
    meta.textContent = `${p.quantity.replace(/_/g, ' ')}, ${probeUnitLabel(i)}`;
    b.append(lineKey(i), name, meta);
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
  });
}

function validityNote(f: number): string {
  const s = result?.shading;
  if (!s) return '';
  if (s.deep_hz !== null && f >= s.deep_hz) return 'in the dark band: lumped-model error ≥ 36 % or past a hard limit';
  if (s.begin_hz !== null && f >= s.begin_hz) return 'in the light band: lumped-model error ≥ 10 %';
  return 'below the shading: no element reports a validity limit here';
}

function cellValue(g: PlotGroup, v: number | null): string {
  if (v === null || !Number.isFinite(v)) return 'n/a';
  if (g.kind === 'spl') return `${v.toFixed(2)} dB SPL`;
  if (g.kind === 'phase') return `${v.toFixed(1)}°`;
  return formatValue(v, g.unit, 4);
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
  const head = document.createElement('p');
  head.className = 'readout-head';
  const fs = document.createElement('strong');
  fs.textContent = formatHz(f);
  head.append(fs, ` (grid point ${i + 1} of ${result.frequencies_Hz.length}), ${validityNote(f)}`);
  readout.append(head);
  const list = document.createElement('ul');
  list.className = 'readout-list';
  const spoken: string[] = [];
  for (const g of groups) {
    if (g.kind === 'phase') continue;
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
      if (g.kind === 'spl' && s !== ref) {
        const a = s.values[i];
        const b = ref.values[i];
        if (a !== null && b !== null) {
          const d = document.createElement('span');
          d.className = 'readout-delta';
          d.textContent = `Δ ${a - b >= 0 ? '+' : '−'}${Math.abs(a - b).toFixed(2)} dB vs ${ref.id}`;
          li.append(d);
        }
      }
      list.append(li);
      spoken.push(`${s.id} ${text}`);
    }
  }
  readout.append(list);
  if (fromKeyboard) srReadout.textContent = `${formatHz(f)}: ${spoken.join('; ')}. ${validityNote(f)}.`;
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

function renderTable(): void {
  tableWrap.replaceChildren();
  if (!result) return;
  const idx = panel.viewIndices();
  const table = document.createElement('table');
  table.className = 'data-table';
  const caption = document.createElement('caption');
  const n = idx ? idx[1] - idx[0] + 1 : 0;
  const { rows, stride } = idx ? tableRows(idx[0], idx[1]) : { rows: [], stride: 1 };
  caption.textContent =
    `Plotted values from ${formatHz(panel.lo)} to ${formatHz(panel.hi)} (${n} rows` +
    (stride > 1 ? `; ${rows.length} shown, one grid point in ${stride} and the last: zoom in to list them all` : '') +
    '). Validity: "light" = lumped-model error ≥ 10 %, "dark" = ≥ 36 % or past a hard limit.';
  table.append(caption);
  const cols: { g: PlotGroup; s: PlotGroup['series'][number] }[] = [];
  for (const g of groups) for (const s of g.series) if (!panel.hidden.has(s.id)) cols.push({ g, s });
  const thead = document.createElement('thead');
  const hr = document.createElement('tr');
  const th = (text: string) => {
    const c = document.createElement('th');
    c.scope = 'col';
    c.textContent = text;
    return c;
  };
  hr.append(th('Frequency (Hz)'), th('Validity'));
  for (const { g, s } of cols) {
    const unit = g.kind === 'spl' ? 'dB SPL' : g.kind === 'phase' ? '°' : g.unit ? prettyUnit(g.unit) : 'unit unknown';
    hr.append(th(`${s.id} ${g.symbol} (${unit})`));
  }
  thead.append(hr);
  const tbody = document.createElement('tbody');
  const sh = result.shading;
  for (const i of rows) {
    const f = result.frequencies_Hz[i];
    const tr = document.createElement('tr');
    const rh = document.createElement('th');
    rh.scope = 'row';
    rh.textContent = formatNumber(f, 6);
    const vd = document.createElement('td');
    vd.textContent =
      sh.deep_hz !== null && f >= sh.deep_hz ? 'dark' : sh.begin_hz !== null && f >= sh.begin_hz ? 'light' : '';
    tr.append(rh, vd);
    for (const { g, s } of cols) {
      const td = document.createElement('td');
      const v = s.values[i];
      td.textContent = g.kind === 'spl' ? (v === null ? '' : v.toFixed(2)) : g.kind === 'phase' ? (v === null ? '' : v.toFixed(2)) : formatNumber(v);
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
    'Shading uses the lowest limits over all elements. "Onset" is where the element’s error reaches 10 % (or its criterion starts to bite); "deep" is 36 % or a hard limit.';
  const head = document.createElement('tr');
  for (const t of ['Element', 'Criterion', 'Onset', 'Deep']) {
    const c = document.createElement('th');
    c.scope = 'col';
    c.textContent = t;
    head.append(c);
  }
  const thead = document.createElement('thead');
  thead.append(head);
  const tbody = document.createElement('tbody');
  const rows = [...r.validity].sort((a, b) => (a.begin_hz ?? Infinity) - (b.begin_hz ?? Infinity));
  for (const v of rows) {
    const tr = document.createElement('tr');
    for (const t of [v.element, v.criterion, v.begin_hz === null ? 'none' : formatHz(v.begin_hz), v.deep_hz === null ? 'none' : formatHz(v.deep_hz)]) {
      const td = document.createElement('td');
      td.textContent = t;
      tr.append(td);
    }
    tbody.append(tr);
  }
  table.append(caption, thead, tbody);
  validityTable.append(table);
}

// ----- errors ---------------------------------------------------------------

function errorDetailText(e: EngineError): string {
  const parts: string[] = [];
  if (e.element) parts.push(`element "${e.element}"`);
  if (e.type) parts.push(`type "${e.type}"`);
  if (e.probe) parts.push(`probe "${e.probe}"`);
  if (e.node) parts.push(`node "${e.node}"`);
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

function gotoError(): void {
  if (!lastError) return;
  const r = errorRange(lastError, editor.value);
  if (!r) return;
  editor.focus();
  editor.setSelectionRange(r[0], r[1]);
  const line = editor.value.slice(0, r[0]).split('\n').length - 1;
  const lh = parseFloat(getComputedStyle(editor).lineHeight) || 18;
  editor.scrollTop = Math.max(0, (line - 3) * lh);
  updateCursorPos();
}

// ----- solve / check ----------------------------------------------------------

function setBusy(busy: boolean): void {
  cancelBtn.disabled = !busy;
  runBtn.setAttribute('aria-busy', String(busy));
  plotsEl.setAttribute('aria-busy', String(busy));
}

function setResult(r: SolveResult, text: string, ms: number): void {
  result = sortResult(r);
  groups = buildGroups(result);
  solvedText = text;
  clearError();
  renderStrip(result, text);
  resultTitle.textContent = netlistTitle(text);
  // Keep hidden probes that still exist.
  const ids = new Set(result.probes.map((p) => p.id));
  for (const h of [...panel.hidden]) if (!ids.has(h)) panel.hidden.delete(h);
  panel.setGroups(groups, { freqs: result.frequencies_Hz, shading: result.shading });
  if (!groups.length) {
    const p = document.createElement('p');
    p.className = 'hint';
    p.textContent = 'This netlist has no probes. Add one to "probes" to plot results.';
    plotsEl.append(p);
  }
  panel.setView(panel.lo, panel.hi);
  renderLegend();
  renderReadout(panel.cursor, false);
  renderValidity(result);
  if (!tableWrap.hidden) renderTable();
  runStatus.textContent =
    `Solved ${result.frequencies_Hz.length} frequencies × ${result.probes.length} probes in ${ms.toFixed(0)} ms.`;
  document.body.dataset.state = 'solved';
}

async function run(): Promise<void> {
  const text = editor.value;
  if (solver.busy) solver.cancel();
  setBusy(true);
  runStatus.textContent = 'Solving…';
  document.body.dataset.state = 'solving';
  let reply: Reply;
  try {
    reply = await solver.call('solve', text);
  } catch (e) {
    if (e instanceof Cancelled) {
      if (!solver.busy) {
        setBusy(false);
        runStatus.textContent = 'Cancelled.';
        document.body.dataset.state = 'cancelled';
      }
      return;
    }
    throw e;
  }
  setBusy(false);
  if (!reply.ok) {
    showError({ error: reply.crash, kind: 'panic' }, text);
    runStatus.textContent = 'The engine failed; see the error below.';
    document.body.dataset.state = 'error';
    return;
  }
  if (isError(reply.value)) {
    showError(reply.value, text);
    runStatus.textContent = result ? 'Run failed; showing the last successful result (dimmed).' : 'Run failed.';
    document.body.dataset.state = 'error';
    return;
  }
  setResult(reply.value as SolveResult, text, reply.ms);
}

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

function loadExample(name: string): void {
  const ex = EXAMPLES.find((e) => e.name === name);
  if (!ex) return;
  const edited = editor.value.trim() !== '' && !EXAMPLES.some((e) => e.text === editor.value);
  if (edited) {
    replacedText = editor.value;
    restoreText.textContent = `Loaded the example "${name}" over your edited netlist.`;
    restoreNote.hidden = false;
  } else {
    hideRestore();
  }
  editor.value = ex.text;
  store.set('example', name);
  store.set('netlist', ex.text);
  updateCursorPos();
  scheduleCheck();
  void run();
}

for (const ex of EXAMPLES) {
  const o = document.createElement('option');
  o.value = ex.name;
  o.textContent = ex.title ? `${ex.name}: ${ex.title}` : ex.name;
  exampleSelect.append(o);
}
exampleSelect.addEventListener('change', () => loadExample(exampleSelect.value));

runBtn.addEventListener('click', () => void run());
cancelBtn.addEventListener('click', () => {
  solver.cancel();
});
errorGoto.addEventListener('click', gotoError);

let saveTimer = 0;
restoreBtn.addEventListener('click', () => {
  if (replacedText === null) return;
  editor.value = replacedText;
  hideRestore();
  store.set('netlist', editor.value);
  updateCursorPos();
  scheduleCheck();
  editor.focus();
  void run();
});

editor.addEventListener('input', () => {
  hideRestore();
  scheduleCheck();
  updateCursorPos();
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => store.set('netlist', editor.value), 500);
});
editor.addEventListener('keydown', (ev) => {
  if (ev.key === 'Enter' && (ev.ctrlKey || ev.metaKey)) {
    ev.preventDefault();
    void run();
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

// Test and debugging hook (read-only view of the current state).
Object.defineProperty(window, 'acoustilab', {
  value: Object.freeze({
    result: () => result,
    groups: () => groups.map((g) => ({ key: g.key, kind: g.kind, title: g.title, scale: g.scale, series: g.series.map((s) => s.id) })),
    view: () => [panel.lo, panel.hi],
    cursor: () => panel.cursor,
    cursorY: () => panel.plots.map((p) => ({ key: p.group.key, y: { ...p.lastCursorY } })),
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

void checker.call('version').then((r) => {
  if (r.ok) stripEngine.textContent = String(r.value);
});

panel.setView(...RANGE_AUDIO);
renderReadout(null, false);
const saved = store.get('netlist');
const savedExample = store.get('example');
if (savedExample && EXAMPLES.some((e) => e.name === savedExample)) exampleSelect.value = savedExample;
if (saved) {
  editor.value = saved;
  updateCursorPos();
  scheduleCheck();
  void run();
} else if (EXAMPLES.length) {
  const first = EXAMPLES.find((e) => e.name === 'sealed_cup') ?? EXAMPLES[0];
  exampleSelect.value = first.name;
  loadExample(first.name);
}
