// "Fit": the measurement round trip of spec Section 12 (docs/fitting.md).
// Measured curves come in as files (FRD, ZMA, REW text, CSV, or curve JSON,
// each with its FILE.sidecar.json) through a picker or drag and drop, or
// from the virtual rig, which measures the current design with chosen noise
// and seed and is labelled as such everywhere. Each curve is checked against
// the model's probe (`compare_curves`) and plotted against the model with
// its uncertainty band. The fit (`fit`) runs on this view's worker in
// bounded steps, each resuming from the last step's values, with progress
// and cancel; its report shows the fitted values, intervals, residuals and
// identifiability, and "Apply" writes chosen values into the netlist.

import { Cancelled } from '../engine';
import { formatHz, formatNumber, formatParam, paramUnit } from '../format';
import type { ParamDesc, ParamsDoc } from '../types';
import {
  CURVE_SCHEMA,
  assignmentsText,
  formatFromName,
  importOptions,
  isVirtual,
  lineOf,
  magnitudeOf,
  markVirtualText,
  originBadge,
  pairFiles,
  parseAssignments,
  readFiles,
  sidecarForm,
  sidecarSummary,
  type CurveDoc,
  type PairedFile,
  type Quantity,
  type Sidecar,
} from './fit-curves';
import { cautions, renderReport, statusClass, type FitReport, type RunInfo } from './fit-report';
import { paramValueNode, scan } from '../jsonscan';
import {
  FreqFigure,
  JobStatus,
  button,
  checkbox,
  download,
  el,
  errorBox,
  field,
  isEngineError,
  keepFocus,
  registerHooks,
  safeName,
  scrollRegion,
  select,
  table,
  uniqueId,
  valueOf,
  words,
  type FigPlot,
  type FigSeries,
} from './measure-kit';
import type { ResultView, ViewHost } from './types';

/** Iterations per fit call: the job reports progress and can be cancelled between calls. */
const ITERATIONS_PER_CALL = 4;

interface Comparison {
  ok: boolean;
  blocking: { field: string; a: string; b: string }[];
  notes: { field: string; a: string; b: string }[];
  message: string | null;
}

interface Measured {
  uid: number;
  name: string;
  /** File text and format the curve was read from (re-read with an edited sidecar). */
  text: string;
  format: string;
  quantityOption: string;
  curve: CurveDoc;
  probe: string;
  use: boolean;
  weight: number;
  phase: 'auto' | 'yes' | 'no';
  offset: 'auto' | 'none' | 'free';
  condition: string;
  allow: Set<string>;
  compare: Comparison | null;
  compareError: string | null;
  /** Combined level uncertainty per point (dB, 1σ); null: the budget has none; undefined: not asked yet. */
  uncertainty: number[] | null | undefined;
  model: ModelCurve | null;
  modelError: string | null;
  fitted: ModelCurve | null;
  gen: number;
  card: HTMLElement;
  fig: FreqFigure | null;
  editing: boolean;
  formError: string | null;
}

/** A probe of the model at a curve's frequencies: plotted magnitude, phase (degrees) and the drive's statement. */
interface ModelCurve {
  values: (number | null)[];
  phase: (number | null)[] | null;
  label: string;
  /** The curve's fitted level offset, dB (drawn with the fitted model, as the fit compares them). */
  offsetDb?: number;
}

/** A fit run: its inputs, kept for "Continue", and its totals over the calls. */
interface FitRun {
  text: string;
  ms: Measured[];
  names: string[];
  /** The spec without parameters and iteration limit. */
  base: Record<string, unknown>;
  /**
   * Scale of each parameter as the first call chose it. The engine picks a
   * parameter's default scale from its start (log for a positive start with
   * a non-negative minimum), so a linear parameter that started at 0 would
   * turn log once resumed from a positive value, changing its interval and
   * status; resumed calls therefore name the first call's scale.
   */
  scales: Record<string, string> | null;
  info: RunInfo;
}

/**
 * Text `a` with the value tokens of parameters `names` replaced by those of
 * text `b`, character for character; null when either text does not parse
 * or lacks one of them.
 */
function withValuesOf(a: string, b: string, names: string[]): string | null {
  try {
    const ra = scan(a);
    const rb = scan(b);
    const edits = names.map((n) => {
      const x = paramValueNode(ra, n);
      const y = paramValueNode(rb, n);
      if (!x || !y) throw new Error(n);
      return { start: x.start, end: x.end, text: b.slice(y.start, y.end) };
    });
    edits.sort((p, q) => q.start - p.start);
    let out = a;
    for (const e of edits) out = out.slice(0, e.start) + e.text + out.slice(e.end);
    return out;
  } catch {
    return null;
  }
}

interface RigResult {
  curve: CurveDoc;
  format: string;
  extension: string;
  text: string;
  sidecar: string;
  probe: string;
  seed: number;
  /** True values that differed from the netlist (the rig's overrides). */
  truth: Record<string, unknown>;
}

const QUANTITY_OF_PROBE: Record<string, Quantity> = {
  pressure: 'pressure',
  impedance: 'impedance',
  displacement: 'displacement',
  velocity: 'velocity',
};

class FitView implements ResultView {
  readonly id = 'fit';
  readonly label = 'Fit';
  readonly order = 93;
  private host!: ViewHost;
  private uid = 0;
  private curves: Measured[] = [];
  private doc: ParamsDoc | null = null;
  private free = new Set<string>();
  private solvedText: string | null = null;

  private importErrors!: HTMLElement;
  private format!: HTMLSelectElement;
  private quantity!: HTMLSelectElement;
  private fileInput!: HTMLInputElement;
  private pickBtn!: HTMLButtonElement;
  private curvesEl!: HTMLElement;
  private rig!: {
    probe: HTMLSelectElement;
    fmin: HTMLInputElement;
    fmax: HTMLInputElement;
    ppo: HTMLInputElement;
    seed: HTMLInputElement;
    level: HTMLInputElement;
    phase: HTMLInputElement;
    seatings: HTMLInputElement;
    averaging: HTMLSelectElement;
    repositioning: HTMLInputElement;
    delay: HTMLInputElement;
    micOffset: HTMLInputElement;
    micSlope: HTMLInputElement;
    coupler: HTMLInputElement;
    truth: HTMLInputElement;
    fileFormat: HTMLSelectElement;
    out: HTMLElement;
  };
  private rigLast: RigResult | null = null;
  private paramsEl!: HTMLElement;
  private suggestJob!: JobStatus;
  private suggestEl!: HTMLElement;
  private fmin!: HTMLInputElement;
  private fmax!: HTMLInputElement;
  private maxIt!: HTMLInputElement;
  private allowSpl!: HTMLInputElement;
  private runBtn!: HTMLButtonElement;
  private runReason!: HTMLElement;
  private fitJob!: JobStatus;
  private reportEl!: HTMLElement;
  private applyEl!: HTMLElement;
  private stale!: HTMLElement;
  private report: FitReport | null = null;
  private run: RunInfo | null = null;
  private fitRun: FitRun | null = null;
  private runUids: number[] = [];
  private jobRunning = false;
  /** Outcome of the last "Apply" (kept until the next fit). */
  private applied = '';
  /** Apply check boxes the user changed, by parameter (until the next fit). */
  private applyChoice = new Map<string, boolean>();

  mount(root: HTMLElement, host: ViewHost): void {
    this.host = host;
    root.classList.add('mv');
    root.append(
      el('p', {
        class: 'hint',
        text:
          'Measured curves against the model and parameter identification (spec Section 12, docs/fitting.md). The virtual rig makes a synthetic ' +
          'measurement of the current design, so the workflow can be tried without hardware. Everything runs on this view’s own worker.',
      }),
    );
    this.stale = el('p', { class: 'hint mv-stale', hidden: true, text: 'The netlist has been edited since it was last solved; models and fits use the solved text.' });
    root.append(this.stale);

    // ----- import
    const opts = importOptions();
    this.format = opts.format;
    this.quantity = opts.quantity;
    // Hidden: "Choose files…" opens it (a visually hidden file input would be an invisible tab stop).
    this.fileInput = el('input', { class: 'mv-file', id: uniqueId('mv-files'), hidden: true, attrs: { type: 'file', multiple: true, accept: '.frd,.zma,.txt,.dat,.csv,.json', 'data-input': 'curves' } });
    this.fileInput.addEventListener('change', () => {
      if (this.fileInput.files?.length) void this.importFiles(this.fileInput.files);
      this.fileInput.value = '';
    });
    const pick = button('Choose files…', () => this.fileInput.click());
    this.pickBtn = pick;
    const drop = el(
      'div',
      { class: 'mv-drop', attrs: { 'data-drop': 'curves' } },
      el('p', {}, 'Drop FRD, ZMA, REW text, CSV or curve JSON files here, each with its ', el('code', { text: 'FILE.sidecar.json' }), ' if you have one, or '),
      pick,
      this.fileInput,
    );
    drop.addEventListener('dragover', (ev) => {
      ev.preventDefault();
      drop.classList.add('over');
    });
    drop.addEventListener('dragleave', () => drop.classList.remove('over'));
    drop.addEventListener('drop', (ev) => {
      ev.preventDefault();
      drop.classList.remove('over');
      if (ev.dataTransfer?.files.length) void this.importFiles(ev.dataTransfer.files);
    });
    // Files dropped anywhere else on the view are refused rather than left to
    // the browser, which would open the file in place of the page.
    const files = (ev: DragEvent) => !!ev.dataTransfer && [...ev.dataTransfer.types].includes('Files');
    root.addEventListener('dragover', (ev) => {
      if (!files(ev) || drop.contains(ev.target as Node)) return;
      ev.preventDefault();
      ev.dataTransfer!.dropEffect = 'none';
    });
    root.addEventListener('drop', (ev) => {
      if (files(ev) && !drop.contains(ev.target as Node)) ev.preventDefault();
    });
    this.importErrors = el('div', { attrs: { 'aria-live': 'polite' } });
    root.append(el('h3', { class: 'mv-h', text: 'Measured curves' }), drop, opts.root, this.importErrors);

    // ----- virtual rig
    root.append(this.rigSection());

    // ----- curves
    this.curvesEl = el('ul', { class: 'mv-curves', attrs: { 'aria-label': 'Loaded curves' } });
    root.append(el('h3', { class: 'mv-h', text: 'Loaded curves against the model' }), this.curvesEl);

    // ----- fit set-up
    this.paramsEl = el('div');
    this.suggestJob = new JobStatus('Checking each parameter alone', () => this.cancelJob());
    this.suggestEl = el('div');
    const num = (v: string, min?: string) => {
      const i = el('input', { class: 'mv-num', attrs: { type: 'number', step: 'any', value: v, inputmode: 'decimal', ...(min ? { min } : {}) } });
      return i;
    };
    this.fmin = num('10', '0');
    this.fmax = num('20000', '0');
    this.maxIt = num('100', '1');
    this.maxIt.step = '1';
    const spl = checkbox('Fit driver parameters to pressure curves alone (refused unless allowed)');
    this.allowSpl = spl.input;
    this.runBtn = button('Run fit', () => void this.runFit(), { class: 'primary' });
    this.runReason = el('p', { class: 'hint', attrs: { 'aria-live': 'polite' } });
    this.fitJob = new JobStatus('Fitting', () => this.cancelJob());
    this.reportEl = el('div');
    this.applyEl = el('div');
    for (const i of [this.fmin, this.fmax, this.maxIt]) i.addEventListener('change', () => this.updateRunState());
    root.append(
      el('h3', { class: 'mv-h', text: 'Fit' }),
      el('p', { class: 'hint', text: 'Free parameters (continuous parameters of the netlist; none is free by default):' }),
      this.paramsEl,
      el('div', { class: 'mv-bar' }, button('Suggest: fit each parameter alone', () => void this.suggest())),
      this.suggestJob.root,
      this.suggestEl,
      el(
        'div',
        { class: 'mv-bar', attrs: { role: 'group', 'aria-label': 'Fit options' } },
        field('Band from (Hz)', this.fmin),
        field('to (Hz)', this.fmax),
        field('At most iterations', this.maxIt),
        spl.wrap,
      ),
      el('div', { class: 'mv-bar' }, this.runBtn),
      this.runReason,
      this.fitJob.root,
      el('h3', { class: 'mv-h', text: 'Fit report' }),
      this.reportEl,
      this.applyEl,
    );
    this.reportEl.append(el('p', { class: 'hint', text: 'No fit yet.' }));

    registerHooks('fit', {
      report: () => this.report,
      run: () => (this.run ? { ...this.run, starts: Object.fromEntries(this.run.starts) } : null),
      rig: () => this.rigLast,
      curves: () =>
        this.curves.map((m) => ({
          name: m.name,
          probe: m.probe,
          quantity: m.curve.quantity,
          points: m.curve.frequencies_Hz.length,
          virtual: isVirtual(m.curve),
          compare: m.compare,
          uncertainty: m.uncertainty,
          model: m.model?.values ?? null,
          fitted: m.fitted ? { values: m.fitted.values, offsetDb: m.fitted.offsetDb ?? null } : null,
          figure: m.fig?.hook() ?? null,
        })),
    });
  }

  refresh(): void {
    const cur = this.host.current();
    const doc = this.host.parameters();
    if (doc) this.doc = doc;
    if (!cur) return;
    this.stale.hidden = this.host.netlist() === cur.text;
    this.renderRigProbes();
    this.renderParams();
    if (cur.text !== this.solvedText) {
      this.solvedText = cur.text;
      for (const m of this.curves) {
        if (!m.probe || !cur.result.probes.some((p) => p.id === m.probe)) m.probe = this.defaultProbe(m.curve);
        void this.evaluate(m);
      }
    }
    this.renderApply();
    this.updateRunState();
  }

  // ----- probes -----------------------------------------------------------------

  private probes(): { id: string; quantity: string }[] {
    return this.host.current()?.result.probes.map((p) => ({ id: p.id, quantity: p.quantity })) ?? [];
  }

  private matching(q: Quantity): { id: string; quantity: string }[] {
    return this.probes().filter((p) => q === 'generic' || QUANTITY_OF_PROBE[p.quantity] === q);
  }

  private defaultProbe(c: CurveDoc): string {
    const ids = this.matching(c.quantity).map((p) => p.id);
    const rigProbe = (c.sidecar.virtual_rig as { probe?: string } | undefined)?.probe;
    if (rigProbe && ids.includes(rigProbe)) return rigProbe;
    const primary = this.doc?.ui?.primary_probe;
    if (primary && ids.includes(primary)) return primary;
    return ids[0] ?? '';
  }

  // ----- import -----------------------------------------------------------------

  private async importFiles(list: FileList | File[]): Promise<void> {
    const { files, errors } = await readFiles(list);
    const paired = pairFiles(files);
    errors.push(...paired.errors);
    for (const f of paired.curves) {
      const e = await this.importOne(f);
      if (e) errors.push(e);
    }
    // A lone sidecar applies to a loaded curve of that name.
    for (const s of paired.sidecars) {
      const base = s.name.replace(/(\.sidecar)?\.json$/i, '').toLowerCase();
      const m = this.curves.find((c) => c.name.toLowerCase() === base);
      if (!m) {
        errors.push(`“${s.name}”: no loaded curve named “${s.name.replace(/(\.sidecar)?\.json$/i, '')}” to apply this sidecar to.`);
        continue;
      }
      const e = await this.applySidecarText(m, s.text, s.name);
      if (e) errors.push(e);
    }
    this.importErrors.replaceChildren();
    if (errors.length) {
      this.importErrors.append(
        el(
          'div',
          { class: 'error-box mv-error', attrs: { role: 'alert' } },
          el('h3', {}, el('span', { text: '⚠', attrs: { 'aria-hidden': 'true' } }), ` ${errors.length === 1 ? 'A file' : `${errors.length} files`} could not be read`),
          el('ul', {}, ...errors.map((e) => el('li', { class: 'mv-error-message', text: e }))),
        ),
      );
    }
    this.host.announce(`${files.length} file${files.length === 1 ? '' : 's'} read${errors.length ? `, ${errors.length} with errors` : ''}.`);
  }

  /** Reads one curve file through the engine; returns an error sentence, or null. */
  private async importOne(f: PairedFile): Promise<string | null> {
    try {
      return await this.readOne(f);
    } catch (e) {
      if (e instanceof Cancelled) return `“${f.name}” was not read: a cancel stopped this view’s engine while it was reading; read the file again.`;
      throw e;
    }
  }

  private async readOne(f: PairedFile): Promise<string | null> {
    let sidecar: unknown = undefined;
    if (f.sidecar) {
      try {
        sidecar = JSON.parse(f.sidecar.text);
      } catch (e) {
        return `“${f.sidecar.name}” is not valid JSON: ${(e as Error).message}`;
      }
    }
    let text = f.text;
    let format = this.format.value || formatFromName(f.name);
    const quantity = this.quantity.value;
    if (f.doc !== undefined) {
      // A curve document: validated and put in the engine's canonical form
      // through a lossless CSV (shortest exact numbers).
      const d = f.doc as { schema?: unknown; sidecar?: unknown };
      if (d?.schema !== CURVE_SCHEMA) return `“${f.name}” is neither a curve (${CURVE_SCHEMA}) nor a sidecar document.`;
      const ex = valueOf(await this.call('export_curve', JSON.stringify(f.doc), 'csv'));
      if (isEngineError(ex)) return `“${f.name}” could not be read: ${ex.error}`;
      text = (ex as { text: string }).text;
      format = 'csv';
      sidecar ??= d.sidecar;
    }
    const o: Record<string, unknown> = { format };
    if (quantity) o.quantity = quantity;
    if (sidecar !== undefined) o.sidecar = sidecar;
    const v = valueOf(await this.call('import_curve', text, JSON.stringify(o)));
    if (isEngineError(v)) {
      const quoted = v.line ? lineOf(text, v.line) : null;
      return `“${f.name}” could not be read: ${v.error}${quoted !== null ? ` (line ${v.line}: “${quoted.trim()}”)` : ''}`;
    }
    this.addCurve(f.name, text, format, quantity, v as CurveDoc);
    return null;
  }

  private addCurve(name: string, text: string, format: string, quantityOption: string, curve: CurveDoc): Measured {
    const m: Measured = {
      uid: ++this.uid,
      name,
      text,
      format,
      quantityOption,
      curve,
      probe: this.defaultProbe(curve),
      use: true,
      weight: 1,
      phase: 'auto',
      offset: 'auto',
      condition: '',
      allow: new Set(),
      compare: null,
      compareError: null,
      uncertainty: undefined,
      model: null,
      modelError: null,
      fitted: null,
      gen: 0,
      card: el('li', { class: 'mv-curve' }),
      fig: null,
      editing: false,
      formError: null,
    };
    this.curves.push(m);
    this.curvesEl.append(m.card);
    this.renderCard(m);
    void this.evaluate(m);
    this.updateRunState();
    return m;
  }

  /** Re-reads a curve with a new sidecar (from the form or a file). Returns an error sentence, or null. */
  private async applySidecarText(m: Measured, sidecarText: string, source: string): Promise<string | null> {
    let sidecar: unknown;
    try {
      sidecar = JSON.parse(sidecarText);
    } catch (e) {
      return `“${source}” is not valid JSON: ${(e as Error).message}`;
    }
    return this.applySidecar(m, sidecar, source);
  }

  private async applySidecar(m: Measured, sidecar: unknown, source: string): Promise<string | null> {
    const o: Record<string, unknown> = { format: m.format, sidecar };
    if (m.quantityOption) o.quantity = m.quantityOption;
    let reply;
    try {
      reply = await this.call('import_curve', m.text, JSON.stringify(o));
    } catch (e) {
      if (e instanceof Cancelled) return `${source}: not applied, a cancel stopped this view’s engine; apply it again.`;
      throw e;
    }
    const v = valueOf(reply);
    if (isEngineError(v)) return `${source}: ${v.error}`;
    m.curve = v as CurveDoc;
    m.uncertainty = undefined;
    m.allow.clear();
    m.editing = false;
    m.formError = null;
    this.renderCard(m);
    void this.evaluate(m);
    return null;
  }

  private call(fn: string, ...args: string[]) {
    return this.host.call(fn, ...args);
  }

  // ----- the model for a curve ------------------------------------------------------

  /** The netlist solved at the curve's frequencies and, for a level curve, at the drive its sidecar states (as the fit does). */
  private modelNetlist(text: string, c: CurveDoc): string | null {
    let doc: Record<string, unknown>;
    try {
      doc = JSON.parse(text);
    } catch {
      return null;
    }
    doc.sweep = { frequencies_Hz: c.frequencies_Hz };
    if (c.quantity !== 'impedance' && c.sidecar.drive) doc.drive = c.sidecar.drive;
    return JSON.stringify(doc);
  }

  private async modelValues(text: string, m: Measured, overrides: Record<string, unknown>): Promise<ModelCurve | string> {
    const net = this.modelNetlist(text, m.curve);
    if (!net) return 'the netlist is not valid JSON';
    const v = valueOf(await this.call('probe_curve', net, JSON.stringify(overrides), m.probe));
    if (isEngineError(v)) return v.error;
    const c = v as CurveDoc;
    const d = c.sidecar.drive;
    return {
      values: magnitudeOf(c).values,
      phase: c.phase_deg ?? null,
      label: d ? Object.entries(d).map(([k, x]) => `${k} ${String(x)}`).join(', ') : 'the netlist’s sources',
    };
  }

  /** Compatibility check, uncertainty and model curve of one measured curve (on the worker). */
  private async evaluate(m: Measured): Promise<void> {
    const cur = this.host.current();
    if (!cur) return;
    const gen = ++m.gen;
    const text = cur.text;
    try {
      if (m.uncertainty === undefined) {
        const u = valueOf(await this.call('curve_uncertainty', JSON.stringify(m.curve)));
        if (!isEngineError(u)) m.uncertainty = (u as { level_dB: number[] | null }).level_dB;
      }
      let compare: Comparison | null = null;
      let compareError: string | null = null;
      let model: Measured['model'] = null;
      let modelError: string | null = null;
      if (m.probe) {
        const pc = valueOf(await this.call('probe_curve', text, '', m.probe));
        if (isEngineError(pc)) compareError = pc.error;
        else {
          const cmp = valueOf(await this.call('compare_curves', JSON.stringify(m.curve), JSON.stringify(pc), JSON.stringify([...m.allow])));
          if (isEngineError(cmp)) compareError = cmp.error;
          else compare = cmp as Comparison;
        }
        const cond = parseAssignments(m.condition);
        if (cond.error) modelError = `condition: ${cond.error}`;
        else {
          const r = await this.modelValues(text, m, cond.values);
          if (typeof r === 'string') modelError = r;
          else model = r;
        }
      }
      if (gen !== m.gen) return;
      m.compare = compare;
      m.compareError = compareError;
      m.model = model;
      m.modelError = modelError;
    } catch (e) {
      if (e instanceof Cancelled) return; // re-evaluated after the cancel
      throw e;
    }
    this.renderCard(m);
    this.updateRunState();
  }

  // ----- curve cards --------------------------------------------------------------

  private renderCard(m: Measured): void {
    // A curve removed while one of its engine calls ran is not drawn again
    // (its figure has been disposed).
    if (!this.curves.includes(m)) return;
    const c = m.curve;
    const f = c.frequencies_Hz;
    m.card.className = `mv-curve${isVirtual(c) ? ' virtual' : ''}`;
    m.card.dataset.curve = m.name;
    const head = el(
      'div',
      { class: 'mv-curve-head' },
      el('span', { class: 'mv-curve-name', text: m.name, attrs: { tabindex: -1, 'data-k': 'home' } }),
      originBadge(c),
      el('span', { class: 'hint', text: `${c.quantity}, ${f.length} points, ${formatHz(f[0])} to ${formatHz(f[f.length - 1])}, read as ${m.format.toUpperCase()}` }),
    );

    // Mapping and weights.
    const probeSel = select([['', 'not fitted (no probe)'], ...this.matching(c.quantity).map((p): [string, string] => [p.id, p.id])], m.probe);
    probeSel.dataset.k = 'probe';
    probeSel.addEventListener('change', () => {
      m.probe = probeSel.value;
      m.allow.clear();
      void this.evaluate(m);
      this.updateRunState();
    });
    const weight = el('input', { class: 'mv-num', attrs: { type: 'number', min: 0, step: 'any', value: m.weight, inputmode: 'decimal', 'data-k': 'weight' } });
    weight.addEventListener('change', () => {
      const w = Number(weight.value);
      m.weight = Number.isFinite(w) && w > 0 ? w : 1;
      weight.value = String(m.weight);
    });
    const phase = select([['auto', 'default (impedance yes, pressure no)'], ['yes', 'fit the phase'], ['no', 'level only']], m.phase);
    phase.dataset.k = 'phase';
    phase.addEventListener('change', () => {
      m.phase = phase.value as Measured['phase'];
      this.renderCard(m);
    });
    const offset = select([['auto', 'default (from the sidecar)'], ['none', 'none'], ['free', 'free']], m.offset);
    offset.dataset.k = 'offset';
    offset.addEventListener('change', () => (m.offset = offset.value as Measured['offset']));
    const cond = el('input', { id: uniqueId('mv-cond'), attrs: { type: 'text', placeholder: 'e.g. added_mass_mg=150', value: m.condition, autocomplete: 'off', spellcheck: 'false', 'data-k': 'cond' } });
    const condMsg = el('span', { class: 'hint', id: uniqueId('mv-cond-msg'), attrs: { 'aria-live': 'polite' } });
    cond.setAttribute('aria-describedby', condMsg.id);
    cond.addEventListener('change', () => {
      const p = parseAssignments(cond.value);
      cond.setAttribute('aria-invalid', String(!!p.error));
      condMsg.textContent = p.error ?? '';
      if (p.error) return;
      m.condition = cond.value;
      void this.evaluate(m);
    });
    const use = checkbox('Use in the fit', m.use);
    use.input.dataset.k = 'use';
    use.input.addEventListener('change', () => {
      m.use = use.input.checked;
      this.updateRunState();
    });
    const controls = el(
      'div',
      { class: 'mv-bar', attrs: { role: 'group', 'aria-label': `Fit settings of ${m.name}` } },
      field('Compare with probe', probeSel),
      field('Weight', weight),
      field('Phase', phase),
      field('Level offset', offset),
      el('div', { class: 'mv-field' }, el('label', { text: 'Measurement condition', attrs: { for: cond.id } }), cond, condMsg),
      use.wrap,
    );

    // Compatibility.
    const compat = el('div', { class: 'mv-compat', attrs: { 'data-field': 'compat' } });
    if (!m.probe) compat.append('Not compared: no probe of this quantity is chosen.');
    else if (m.compareError) compat.append(el('span', { class: 'mv-compat blocked', text: `Check failed: ${m.compareError}` }));
    else if (!m.compare) compat.append(el('span', { class: 'hint', text: 'Checking…' }));
    else {
      const cmp = m.compare;
      compat.classList.add(cmp.ok ? 'ok' : 'blocked');
      compat.append(
        cmp.ok
          ? `Compatible with the model’s probe ${m.probe} (engine check)${m.allow.size ? `, allowing ${[...m.allow].join(', ')}` : ''}.`
          : `Blocked against the model’s probe ${m.probe}: ${cmp.message ?? ''}`,
      );
      const blocking = cmp.blocking;
      if (blocking.length || m.allow.size) {
        const list = el('ul');
        for (const d of blocking) {
          const box = checkbox(`Allow the difference in ${words(d.field)} (curve: ${d.a}; model: ${d.b})`, m.allow.has(d.field));
          box.input.dataset.k = `allow-${d.field}`;
          box.input.addEventListener('change', () => {
            if (box.input.checked) m.allow.add(d.field);
            else m.allow.delete(d.field);
            void this.evaluate(m);
          });
          list.append(el('li', {}, box.wrap));
        }
        for (const a of m.allow) {
          if (blocking.some((d) => d.field === a)) continue;
          const box = checkbox(`Allow the difference in ${words(a)}`, true);
          box.input.dataset.k = `allow-${a}`;
          box.input.addEventListener('change', () => {
            m.allow.delete(a);
            void this.evaluate(m);
          });
          list.append(el('li', {}, box.wrap));
        }
        compat.append(list);
      }
      if (cmp.notes.length) {
        compat.append(
          el(
            'details',
            { class: 'mv-details' },
            el('summary', { text: `Notes (${cmp.notes.length}; not blocking)` }),
            el('ul', {}, ...cmp.notes.map((d) => el('li', { text: `${words(d.field)}: curve ${d.a}, model ${d.b}` }))),
          ),
        );
      }
    }

    // Actions and the sidecar form.
    const sideInput = el('input', { class: 'mv-file', hidden: true, attrs: { type: 'file', accept: '.json', 'data-input': 'sidecar' } });
    sideInput.addEventListener('change', async () => {
      const file = sideInput.files?.[0];
      sideInput.value = '';
      if (!file) return;
      m.formError = await this.applySidecarText(m, await file.text(), `“${file.name}”`);
      this.renderCard(m);
    });
    const actions = el(
      'div',
      { class: 'mv-actions' },
      button(m.editing ? 'Close the sidecar form' : 'Edit sidecar', () => {
        m.editing = !m.editing;
        m.formError = null;
        this.renderCard(m);
      }, { attrs: { 'aria-expanded': String(m.editing), 'data-k': 'edit' } }),
      button('Load sidecar JSON…', () => sideInput.click(), { attrs: { 'data-k': 'loadside' } }),
      sideInput,
      button('Download curve (CSV + sidecar)', () => void this.downloadCurve(m), { attrs: { 'data-k': 'download' } }),
      button('Remove', () => this.removeCurve(m)),
    );
    let form: HTMLElement | null = null;
    if (m.editing) {
      const f = sidecarForm(c.sidecar, c.quantity);
      const msg = el('p', { class: 'mv-compat blocked', attrs: { role: 'alert' }, text: m.formError ?? '' });
      form = el(
        'div',
        { class: 'mv-card' },
        el('h4', { text: `Sidecar of ${m.name}` }),
        f.root,
        el(
          'div',
          { class: 'mv-actions' },
          button('Apply sidecar', async () => {
            const r = f.read();
            if (r.error || !r.sidecar) {
              msg.textContent = r.error ?? '';
              return;
            }
            const e = await this.applySidecar(m, r.sidecar as Sidecar, 'The sidecar was not accepted');
            if (e) {
              m.formError = e;
              msg.textContent = e;
            }
          }),
        ),
        msg,
      );
    } else if (m.formError) {
      form = el('p', { class: 'mv-compat blocked', attrs: { role: 'alert' }, text: m.formError });
    }

    const summary = el('details', { class: 'mv-details' }, el('summary', { text: 'Sidecar (what the file states)' }), sidecarSummary(c.sidecar));
    const figure = this.renderFigure(m);
    keepFocus(m.card, () =>
      m.card.replaceChildren(head, controls, compat, summary, actions, ...(form ? [form] : []), ...(m.modelError ? [errorBox('Model not evaluated', m.modelError)] : []), figure),
    );
  }

  private renderFigure(m: Measured): HTMLElement {
    const c = m.curve;
    m.fig ??= new FreqFigure({
      id: `fit-${m.uid}`,
      label: `${m.name} against the model`,
      host: this.host,
      help: 'Plots: move the pointer or focus a plot and use ←/→ to read values; +/− zoom, Ctrl+←/→ pan, 0 default view, Esc hides the crosshair.',
    });
    const mag = magnitudeOf(c);
    const series: FigSeries[] = [{ id: isVirtual(c) ? 'measured (virtual rig)' : 'measured', slot: 0, values: mag.values, primary: true }];
    const u = m.uncertainty;
    if (u) {
      const up = mag.values.map((v, i) => (mag.db ? v + u[i] : v * 10 ** (u[i] / 20)));
      const dn = mag.values.map((v, i) => (mag.db ? v - u[i] : v * 10 ** (-u[i] / 20)));
      series.push({ id: 'measured +u (1σ)', slot: 8, values: up }, { id: 'measured −u (1σ)', slot: 16, values: dn });
    }
    if (m.model) series.push({ id: 'model', slot: 1, values: m.model.values });
    const off = m.fitted?.offsetDb ?? 0;
    if (m.fitted) {
      const shifted = off ? m.fitted.values.map((v) => (v === null ? null : mag.db ? v + off : v * 10 ** (off / 20))) : m.fitted.values;
      series.push({ id: off ? 'model, fitted values and offset' : 'model, fitted values', slot: 2, values: shifted });
    }
    const unitText = mag.db ? 'dB re 20 µPa' : mag.unit === 'ohm' ? 'Ω' : mag.unit;
    let lo = Infinity;
    let hi = -Infinity;
    for (const v of mag.values) if (v > 0) (lo = Math.min(lo, v)), (hi = Math.max(hi, v));
    const plots: FigPlot[] = [
      {
        key: 'level',
        title: mag.db ? 'Level' : `|${c.quantity}|`,
        symbol: mag.db ? 'SPL' : '|·|',
        unit: mag.db ? '' : mag.unit,
        axisUnit: unitText,
        kind: mag.db ? 'spl' : 'mag',
        scale: !mag.db && hi / lo > 10 ? 'log' : 'linear',
        series,
        format: (v) => (mag.db ? `${v.toFixed(2)} dB SPL` : `${formatNumber(v, 5)} ${unitText}`),
        columnUnit: mag.db ? 'dB SPL' : unitText,
        cell: mag.db ? (v) => v.toFixed(3) : undefined,
      },
    ];
    // Phase, where the fit uses it: impedance, displacement and velocity by
    // default, pressure only on request (its phase carries the time of flight).
    const usesPhase = m.phase === 'yes' || (m.phase === 'auto' && c.quantity !== 'pressure');
    if (c.phase_deg && usesPhase) {
      const ps: FigSeries[] = [{ id: isVirtual(c) ? 'measured phase (virtual rig)' : 'measured phase', slot: 0, values: c.phase_deg, primary: true }];
      if (m.model?.phase) ps.push({ id: 'model phase', slot: 1, values: m.model.phase });
      if (m.fitted?.phase) ps.push({ id: 'model phase, fitted values', slot: 2, values: m.fitted.phase });
      plots.push({
        key: 'phase',
        title: 'Phase',
        symbol: 'φ',
        unit: '',
        axisUnit: '°',
        kind: 'phase',
        height: 'small',
        series: ps,
        format: (v) => `${v.toFixed(1)}°`,
        columnUnit: '°',
        cell: (v) => v.toFixed(2),
      });
    }
    // Residuals of the last fit, on this curve's frequencies inside the band.
    const k = this.report ? this.runUids.indexOf(m.uid) : -1;
    const cr = k >= 0 ? this.report!.curves.find((x) => x.index === k) : undefined;
    if (cr) {
      const at = new Map(cr.residuals.frequencies_Hz.map((x, i) => [x, i]));
      const pick = (a: number[]) => c.frequencies_Hz.map((x) => (at.has(x) ? a[at.get(x)!] : null));
      plots.push({
        key: 'residual',
        title: 'Level residual (model − measurement, offset included)',
        symbol: 'ΔL',
        unit: '',
        axisUnit: 'dB',
        kind: 'delta',
        height: 'small',
        series: [{ id: 'level residual', slot: 0, values: pick(cr.residuals.level_dB), primary: true }],
        format: (v) => `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(3)} dB`,
        columnUnit: 'dB',
        cell: (v) => v.toFixed(4),
      });
      if (cr.residuals.phase_deg) {
        plots.push({
          key: 'phase-residual',
          title: 'Phase residual',
          symbol: 'Δφ',
          unit: '',
          axisUnit: '°',
          kind: 'delta',
          height: 'small',
          series: [{ id: 'phase residual', slot: 1, values: pick(cr.residuals.phase_deg) }],
          format: (v) => `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(2)}°`,
          columnUnit: '°',
          cell: (v) => v.toFixed(3),
        });
      }
    }
    m.fig.set(plots, c.frequencies_Hz, this.host.current()?.result.shading ?? null);
    const note = el(
      'p',
      { class: 'hint' },
      m.model ? `Model: probe ${m.probe} at the curve’s frequencies and drive (${m.model.label})${m.condition ? `, condition ${m.condition}` : ''}. ` : '',
      m.fitted && off ? `The fitted model includes the curve’s fitted level offset (${off >= 0 ? '+' : '−'}${Math.abs(off).toFixed(3)} dB), as the fit compares them. ` : '',
      u ? 'The dashed lines are the measured level ± the combined standard uncertainty of the sidecar’s budget (engine).' : 'The sidecar states no level uncertainty.',
    );
    return el('div', {}, note, m.fig.root);
  }

  private removeCurve(m: Measured): void {
    const k = this.curves.indexOf(m);
    this.curves = this.curves.filter((c) => c !== m);
    m.card.remove();
    m.fig?.panel.dispose();
    m.fig = null;
    // The focus goes to the next curve, else to the file picker.
    const next = this.curves[Math.min(k, this.curves.length - 1)];
    (next?.card.querySelector<HTMLElement>('[data-k="home"]') ?? this.pickBtn).focus();
    this.host.announce(`Removed ${m.name}.`);
    this.updateRunState();
  }

  private async downloadCurve(m: Measured): Promise<void> {
    let reply;
    try {
      reply = await this.call('export_curve', JSON.stringify(m.curve), 'csv');
    } catch (e) {
      if (!(e instanceof Cancelled)) throw e;
      m.formError = 'Not exported: a cancel stopped this view’s engine; export again.';
      this.renderCard(m);
      return;
    }
    const v = valueOf(reply);
    if (isEngineError(v)) {
      m.formError = `Not exported: ${v.error}`;
      this.renderCard(m);
      return;
    }
    const x = v as { text: string; sidecar: string; extension: string };
    const base = safeName(m.name.replace(/\.[^.]+$/, ''));
    download(`${base}.csv`, isVirtual(m.curve) ? markVirtualText(x.text, 'csv', `exported from ${m.name}`) : x.text, 'text/csv');
    download(`${base}.csv.sidecar.json`, x.sidecar, 'application/json');
  }

  // ----- virtual rig ---------------------------------------------------------------

  private rigSection(): HTMLElement {
    const n = (v: string, min = '0', step = 'any') => el('input', { class: 'mv-num', attrs: { type: 'number', value: v, min, step, inputmode: 'decimal' } });
    this.rig = {
      probe: select([]),
      fmin: n('10'),
      fmax: n('20000'),
      ppo: n('12', '1'),
      seed: n('1', '0', '1'),
      level: n('0.1'),
      phase: n('1'),
      seatings: n('1', '1', '1'),
      averaging: select([['complex', 'complex (vector)'], ['magnitude', 'magnitude (power)'], ['db', 'dB (level)']], 'complex'),
      repositioning: n('0'),
      delay: n('0'),
      micOffset: n('0', ''),
      micSlope: n('0', ''),
      coupler: n('0'),
      truth: el('input', { attrs: { type: 'text', placeholder: 'e.g. leak_gap_mm=0.12', autocomplete: 'off', spellcheck: 'false' } }),
      fileFormat: select([['', 'ZMA or FRD (by quantity)'], ['frd', 'FRD'], ['zma', 'ZMA'], ['rew', 'REW text'], ['csv', 'CSV']], ''),
      out: el('div', { attrs: { 'aria-live': 'polite' } }),
    };
    const r = this.rig;
    const form = el(
      'div',
      { class: 'mv-form', attrs: { role: 'group', 'aria-label': 'Virtual rig settings' } },
      field('Probe', r.probe),
      field('From (Hz)', r.fmin),
      field('To (Hz)', r.fmax),
      field('Points per octave', r.ppo),
      field('Seed', r.seed),
      field('File format', r.fileFormat),
      el(
        'fieldset',
        {},
        el('legend', { text: 'Noise (standard deviations)' }),
        field('Level noise (dB)', r.level),
        field('Phase noise (°)', r.phase),
        field('Seatings', r.seatings),
        field('Averaging', r.averaging),
        field('Repositioning (dB)', r.repositioning),
        field('Repositioning delay (µs)', r.delay),
        field('Sensor offset (dB)', r.micOffset),
        field('Sensor slope (dB/decade)', r.micSlope),
        field('Coupler ripple (dB RMS)', r.coupler),
      ),
      el('div', { class: 'mv-wide' }, field('True values that differ from the netlist', r.truth, 'Parameter overrides the rig measures with (name=value, comma separated); empty: the design as it is.')),
    );
    return el(
      'section',
      { attrs: { 'aria-labelledby': 'fit-rig-h' } },
      el('h3', { class: 'mv-h', id: 'fit-rig-h', text: 'Virtual rig (synthetic measurement)' }),
      el('p', {
        class: 'hint',
        text: 'Solves the netlist on a measurement grid and adds seeded noise, seatings and systematic errors (docs/fitting.md, "Virtual rig"). Its files say so in their sidecar (origin virtual_rig).',
      }),
      form,
      el('div', { class: 'mv-bar' }, button('Generate synthetic measurement', () => void this.generate())),
      r.out,
    );
  }

  private renderRigProbes(): void {
    const ids = this.probes().map((p) => `${p.id}|${p.quantity}`).join(',');
    const s = this.rig.probe;
    if (s.dataset.ids === ids) return;
    const keep = s.value;
    s.replaceChildren(...this.probes().map((p) => el('option', { text: `${p.id} (${p.quantity.replace(/_/g, ' ')})`, attrs: { value: p.id } })));
    const primary = this.doc?.ui?.primary_probe;
    s.value = this.probes().some((p) => p.id === keep) ? keep : primary && this.probes().some((p) => p.id === primary) ? primary : (this.probes()[0]?.id ?? '');
    s.dataset.ids = ids;
  }

  private async generate(): Promise<void> {
    const cur = this.host.current();
    const r = this.rig;
    if (!cur) return;
    const truth = parseAssignments(r.truth.value);
    r.truth.setAttribute('aria-invalid', String(!!truth.error));
    if (truth.error) {
      r.out.replaceChildren(errorBox('Not generated', `True values: ${truth.error}`));
      return;
    }
    const numv = (i: HTMLInputElement) => Number(i.value);
    const spec: Record<string, unknown> = {
      probe: r.probe.value,
      f_min_Hz: numv(r.fmin),
      f_max_Hz: numv(r.fmax),
      points_per_octave: numv(r.ppo),
      noise: {
        seed: Math.round(numv(r.seed)),
        level_dB: numv(r.level),
        phase_deg: numv(r.phase),
        seatings: Math.round(numv(r.seatings)),
        averaging: r.averaging.value,
        repositioning_dB: numv(r.repositioning),
        repositioning_delay_us: numv(r.delay),
        microphone_offset_dB: numv(r.micOffset),
        microphone_slope_dB_per_decade: numv(r.micSlope),
        coupler_dB: numv(r.coupler),
      },
    };
    if (Object.keys(truth.values).length) spec.overrides = truth.values;
    if (r.fileFormat.value) spec.format = r.fileFormat.value;
    r.out.replaceChildren(el('p', { class: 'hint', text: 'Measuring…' }));
    let v: unknown;
    try {
      v = valueOf(await this.call('virtual_measure', cur.text, JSON.stringify(spec)));
    } catch (e) {
      if (e instanceof Cancelled) {
        r.out.replaceChildren(el('p', { class: 'hint', text: 'Cancelled.' }));
        return;
      }
      throw e;
    }
    if (isEngineError(v)) {
      r.out.replaceChildren(errorBox('Not generated', v.error, v.kind ? `Kind: ${v.kind}.` : undefined));
      return;
    }
    const res = v as Omit<RigResult, 'probe' | 'seed' | 'truth'>;
    const seed = Math.round(numv(r.seed));
    const probe = String(spec.probe);
    // The data file says what it is even without its sidecar.
    const text = markVirtualText(res.text, res.format, `probe ${probe}, seed ${seed}${Object.keys(truth.values).length ? `, true values ${assignmentsText(truth.values)}` : ''}`);
    this.rigLast = { ...res, text, probe, seed, truth: truth.values };
    this.renderRigResult();
    this.host.announce('Synthetic measurement generated.');
  }

  private rigFileName(): string {
    const x = this.rigLast!;
    return `virtual-rig-${safeName(x.probe)}-seed${x.seed}.${x.extension}`;
  }

  private renderRigResult(): void {
    const x = this.rigLast;
    if (!x) return;
    const f = x.curve.frequencies_Hz;
    const noise = (x.curve.sidecar.virtual_rig as { noise?: Record<string, unknown> } | undefined)?.noise ?? {};
    const truth = Object.keys(x.truth).length ? x.truth : null;
    const name = this.rigFileName();
    this.rig.out.replaceChildren(
      el(
        'div',
        { class: 'mv-curve virtual', attrs: { 'data-rig': 'result' } },
        el(
          'div',
          { class: 'mv-curve-head' },
          el('span', { class: 'mv-curve-name', text: name }),
          el('span', { class: 'mv-badge', text: 'VIRTUAL RIG · synthetic, not measured' }),
        ),
        el(
          'p',
          { class: 'hint' },
          `${x.curve.quantity} of ${x.probe}: ${f.length} points, ${formatHz(f[0])} to ${formatHz(f[f.length - 1])}, ${x.format.toUpperCase()}; ` +
            `noise ${assignmentsText(noise)}` +
            (truth ? `; true values differing from the netlist: ${assignmentsText(truth)}` : ''),
        ),
        el(
          'div',
          { class: 'mv-actions' },
          button(`Download ${name}`, () => download(name, x.text, 'text/plain')),
          button(`Download ${name}.sidecar.json`, () => download(`${name}.sidecar.json`, x.sidecar, 'application/json')),
          button('Load as a measurement', () => void this.loadRig(), { class: 'primary' }),
        ),
      ),
    );
  }

  /** Reads the rig's file text with its sidecar back in, as a dropped pair of files would be. */
  private async loadRig(): Promise<void> {
    const x = this.rigLast;
    if (!x) return;
    const e = await this.importOne({ name: this.rigFileName(), text: x.text, sidecar: { name: `${this.rigFileName()}.sidecar.json`, text: x.sidecar } });
    this.importErrors.replaceChildren(...(e ? [errorBox('The synthetic measurement could not be read back', e)] : []));
    if (!e) this.host.announce('Synthetic measurement loaded as a curve.');
  }

  // ----- free parameters -----------------------------------------------------------

  private fittable(): ParamDesc[] {
    return (this.doc?.parameters ?? []).filter((p) => p.kind === 'number');
  }

  private used(): Set<string> {
    return new Set(this.host.current()?.result.meta.parameters_used ?? []);
  }

  private renderParams(): void {
    const ps = this.fittable();
    const used = this.used();
    const sig = JSON.stringify([ps.map((p) => [p.name, p.value]), [...used]]);
    if (this.paramsEl.dataset.sig === sig) return;
    this.paramsEl.dataset.sig = sig;
    if (!ps.length) {
      this.paramsEl.replaceChildren(el('p', { class: 'hint', text: 'This netlist declares no continuous parameter to fit.' }));
      return;
    }
    for (const n of [...this.free]) if (!ps.some((p) => p.name === n)) this.free.delete(n);
    const list = el('ul', { class: 'mv-params', attrs: { 'aria-label': 'Free parameters' } });
    for (const p of ps) {
      const unit = paramUnit(p.unit);
      const v = typeof p.value === 'number' ? `${formatParam(p.value)}${unit ? ` ${unit}` : ''}` : '';
      const box = checkbox(`${p.label} (${p.name}) = ${v}`, this.free.has(p.name));
      box.input.dataset.param = p.name;
      box.input.dataset.k = `param-${p.name}`;
      if (!used.has(p.name)) {
        box.input.disabled = true;
        box.wrap.append(el('span', { class: 'hint', text: ' not used by the netlist' }));
      }
      box.input.addEventListener('change', () => {
        if (box.input.checked) this.free.add(p.name);
        else this.free.delete(p.name);
        this.updateRunState();
      });
      list.append(el('li', {}, box.wrap));
    }
    keepFocus(this.paramsEl, () => this.paramsEl.replaceChildren(list));
  }

  // ----- fit ------------------------------------------------------------------------

  private fitCurves(): Measured[] {
    return this.curves.filter((m) => m.use && m.probe);
  }

  private curveSpecs(ms: Measured[]): Record<string, unknown>[] {
    return ms.map((m) => {
      const c: Record<string, unknown> = { probe: m.probe, curve: m.curve };
      if (m.weight !== 1) c.weight = m.weight;
      if (m.phase !== 'auto') c.use_phase = m.phase === 'yes';
      if (m.offset !== 'auto') c.offset = m.offset;
      const cond = parseAssignments(m.condition);
      if (!cond.error && Object.keys(cond.values).length) c.overrides = cond.values;
      if (m.allow.has('compensation')) c.allow = ['compensation'];
      return c;
    });
  }

  private band(): { f_min_Hz: number; f_max_Hz: number } | string {
    const a = Number(this.fmin.value);
    const b = Number(this.fmax.value);
    if (!(a > 0 && b > a)) return 'the band needs 0 < from < to';
    return { f_min_Hz: a, f_max_Hz: b };
  }

  /** Why the fit cannot run now, or null. */
  private blocker(): string | null {
    if (!this.host.current()) return 'Run a netlist first.';
    const ms = this.fitCurves();
    if (!ms.length) return 'Load a curve and choose the probe it is compared with.';
    const blocked = ms.filter((m) => !m.compare?.ok);
    if (blocked.length) return `Not compatible with the model (see each curve): ${blocked.map((m) => m.name).join(', ')}. Allow the differences, fix the sidecar, or leave the curve out.`;
    if (!this.free.size) return 'Choose at least one free parameter (or use the suggestion).';
    const b = this.band();
    if (typeof b === 'string') return `Band: ${b}.`;
    return null;
  }

  private updateRunState(): void {
    const why = this.jobRunning ? 'A job is running.' : this.blocker();
    this.runBtn.disabled = why !== null;
    this.runReason.textContent = why ?? `Ready: ${this.free.size} free parameter${this.free.size === 1 ? '' : 's'}, ${this.fitCurves().length} curve${this.fitCurves().length === 1 ? '' : 's'}.`;
  }

  private cancelJob(): void {
    this.host.cancel();
    this.jobRunning = false;
    // Cancel terminated every call on the worker: model curves are computed again.
    for (const m of this.curves) void this.evaluate(m);
    this.updateRunState();
  }

  /** The iteration cap from its field. */
  private cap(): number {
    return Math.max(1, Math.round(Number(this.maxIt.value)) || 100);
  }

  private async runFit(): Promise<void> {
    const cur = this.host.current();
    const band = this.band();
    if (!cur || typeof band === 'string' || this.jobRunning || this.blocker()) return;
    const ms = this.fitCurves();
    const names = [...this.free];
    const run: FitRun = {
      text: cur.text,
      ms,
      names,
      base: { schema: 'acoustilab-fit/0.1', curves: this.curveSpecs(ms), ...band, allow_spl_only: this.allowSpl.checked },
      scales: null,
      info: {
        calls: 0,
        iterations: 0,
        evaluations: 0,
        failed: 0,
        cap: this.cap(),
        starts: new Map(),
        curveNames: ms.map((m) => m.name),
        allowed: ms.map((m) => (m.compare?.blocking ?? []).filter((d) => m.allow.has(d.field))),
        virtual: ms.map((m) => isVirtual(m.curve)),
        cancelled: false,
      },
    };
    this.fitJob.start(`Fitting ${names.join(', ')} to ${ms.length} curve${ms.length === 1 ? '' : 's'}…`);
    this.host.announce('Fit started.');
    await this.iterate(run, null);
  }

  /** Resumes the last run from its fitted values, for up to the cap's iterations more. */
  private async continueFit(): Promise<void> {
    const run = this.fitRun;
    const r = this.report;
    if (!run || !r || r.converged || this.jobRunning) return;
    run.info.cap = run.info.iterations + this.cap();
    run.info.cancelled = false;
    this.fitJob.start(`Continuing the fit of ${run.names.join(', ')} from the fitted values…`);
    this.host.announce('Fit continued.');
    await this.iterate(run, r.fitted);
  }

  /**
   * Runs `fit` in calls of at most ITERATIONS_PER_CALL iterations, each
   * starting from the previous call's fitted values (docs/fitting.md,
   * "Bounded runtime"), until a call converges, the evaluations run out or
   * the run's cap is reached. `from`: start values of the first call (null:
   * the netlist's).
   */
  private async iterate(run: FitRun, from: Record<string, number> | null): Promise<void> {
    const info = run.info;
    let starts = from;
    let last: FitReport | null = null;
    let error: { error: string; kind: string } | null = null;
    this.jobRunning = true;
    this.updateRunState();
    this.fitJob.progress(info.iterations, info.cap);
    try {
      for (;;) {
        const spec = {
          ...run.base,
          parameters: run.names.map((n) =>
            starts && n in starts ? { name: n, start: starts[n], ...(run.scales?.[n] ? { scale: run.scales[n] } : {}) } : n,
          ),
          max_iterations: Math.min(ITERATIONS_PER_CALL, info.cap - info.iterations),
        };
        const v = valueOf(await this.call('fit', run.text, JSON.stringify(spec)));
        if (isEngineError(v)) {
          error = v;
          break;
        }
        const r = v as FitReport;
        last = r;
        info.calls++;
        info.iterations += r.iterations;
        info.evaluations += r.evaluations;
        info.failed += r.failed_evaluations;
        if (info.calls === 1) {
          for (const p of r.parameters) info.starts.set(p.name, p.start);
          run.scales = Object.fromEntries(r.parameters.map((p) => [p.name, p.scale]));
        }
        const vals = r.parameters.map((p) => `${p.name} ${formatParam(p.value, 5)}`).join(', ');
        this.fitJob.progress(info.iterations, info.cap, `Iteration ${info.iterations} of at most ${info.cap}: reduced χ² ${formatNumber(r.reduced_chi2, 4)}; ${vals}`);
        if (r.converged || r.stop === 'max_evaluations' || r.iterations === 0 || info.iterations >= info.cap) break;
        starts = r.fitted;
      }
    } catch (e) {
      if (!(e instanceof Cancelled)) throw e;
      info.cancelled = true;
    }
    this.jobRunning = false;
    this.updateRunState();
    if (error) {
      const refused = error.kind === 'fit_refused';
      this.fitJob.finish(refused ? 'The engine refused the fit (spec Section 12).' : 'The fit failed.', 'failed');
      this.reportEl.replaceChildren(errorBox(refused ? 'Fit refused' : 'Fit not run', error.error, `Kind: ${error.kind}.`));
      this.report = null;
      this.fitRun = null;
      this.renderApply();
      this.host.announce(refused ? 'Fit refused.' : 'Fit failed.');
      return;
    }
    if (!last) {
      // Cancelled before the first call returned: what was shown stays.
      this.fitJob.finish('Cancelled before the first step finished.', 'cancelled');
      return;
    }
    this.report = last;
    this.run = info;
    this.fitRun = run;
    this.applied = '';
    this.applyChoice.clear();
    this.runUids = run.ms.map((m) => m.uid);
    this.fitJob.finish(
      info.cancelled
        ? `Cancelled after ${info.iterations} iterations; the report is the last completed step’s.`
        : `${last.converged ? 'Converged' : 'Stopped without converging'} after ${info.iterations} iterations.`,
      info.cancelled ? 'cancelled' : 'done',
    );
    this.host.announce(info.cancelled ? 'Fit cancelled.' : 'Fit done.');
    const report = renderReport(last, info);
    if (!last.converged) {
      report.append(
        el(
          'div',
          { class: 'mv-bar' },
          button(`Continue from the fitted values (up to ${this.cap()} more iterations)`, () => void this.continueFit(), { attrs: { 'data-k': 'continue' } }),
        ),
      );
    }
    this.reportEl.replaceChildren(report);
    await this.fittedCurves(run.ms, run.text, last);
    this.renderApply();
  }

  /** The model with the fitted values at each fitted curve's frequencies (drawn with the curve). */
  private async fittedCurves(ms: Measured[], text: string, r: FitReport): Promise<void> {
    for (const m of this.curves) m.fitted = null;
    try {
      for (const [k, m] of ms.entries()) {
        const cond = parseAssignments(m.condition);
        const res = await this.modelValues(text, m, { ...cond.values, ...r.fitted });
        m.fitted = typeof res === 'string' ? null : { ...res, offsetDb: r.offsets.find((o) => o.curve === k)?.value_dB };
      }
    } catch (e) {
      if (!(e instanceof Cancelled)) throw e;
    }
    for (const m of this.curves) this.renderCard(m);
  }

  // ----- suggestion -----------------------------------------------------------------

  private async suggest(): Promise<void> {
    const cur = this.host.current();
    const band = this.band();
    if (!cur || this.jobRunning) return;
    const ms = this.fitCurves().filter((m) => m.compare?.ok);
    if (!ms.length) {
      this.suggestEl.replaceChildren(el('p', { class: 'hint', text: 'Load a compatible curve mapped to a probe first.' }));
      return;
    }
    if (typeof band === 'string') {
      this.suggestEl.replaceChildren(el('p', { class: 'hint', text: `Band: ${band}.` }));
      return;
    }
    const used = this.used();
    const cands = this.fittable().filter((p) => used.has(p.name));
    const rows: { p: ParamDesc; status: string | null; ci: [number, number] | null; error: { error: string; kind: string } | null }[] = [];
    this.jobRunning = true;
    this.updateRunState();
    this.suggestJob.start(`Fitting each of ${cands.length} parameters alone for one step…`);
    let cancelled = false;
    try {
      for (const [k, p] of cands.entries()) {
        this.suggestJob.progress(k, cands.length, `Parameter ${k + 1} of ${cands.length}: ${p.name}`);
        const spec = { schema: 'acoustilab-fit/0.1', curves: this.curveSpecs(ms), ...band, parameters: [p.name], max_iterations: 1, allow_spl_only: this.allowSpl.checked };
        const v = valueOf(await this.call('fit', cur.text, JSON.stringify(spec)));
        if (isEngineError(v)) rows.push({ p, status: null, ci: null, error: v });
        else {
          const pr = (v as FitReport).parameters[0];
          rows.push({ p, status: pr.status, ci: pr.ci95, error: null });
        }
      }
    } catch (e) {
      if (!(e instanceof Cancelled)) throw e;
      cancelled = true;
    }
    this.jobRunning = false;
    this.updateRunState();
    this.suggestJob.finish(cancelled ? `Cancelled after ${rows.length} of ${cands.length} parameters.` : `Checked ${rows.length} parameters.`, cancelled ? 'cancelled' : 'done');
    const good = rows.filter((r) => r.status === 'determined' || r.status === 'weakly_determined');
    const messages = [...new Set(rows.filter((r) => r.error).map((r) => r.error!.error))];
    const t = table(
      'Each parameter fitted alone for one step from its netlist value: its status in the engine’s report (a parameter determined alone may still be correlated with others in a joint fit)',
      ['Parameter', 'Alone', '95 % interval'],
      rows.map((r) => [
        `${r.p.label} (${r.p.name})`,
        r.error ? `${r.error.kind === 'fit_refused' ? 'refused' : 'error'} (note ${messages.indexOf(r.error.error) + 1})` : el('span', { class: statusClass(r.status!), text: words(r.status!) }),
        r.ci ? `${formatParam(r.ci[0], 5)} to ${formatParam(r.ci[1], 5)}` : '—',
      ]),
      { rowHead: true },
    );
    t.querySelectorAll('tbody tr').forEach((tr, k) => ((tr as HTMLElement).dataset.param = rows[k].p.name));
    this.suggestEl.replaceChildren(
      scrollRegion('Parameters fitted alone', t),
      messages.length ? el('ol', { class: 'mv-sentences', attrs: { 'aria-label': 'Notes' } }, ...messages.map((x) => el('li', { text: x }))) : '',
      good.length
        ? button(`Free the ${good.length} determined alone (${good.map((r) => r.p.name).join(', ')})`, () => {
            this.free = new Set(good.map((r) => r.p.name));
            this.paramsEl.dataset.sig = '';
            this.renderParams();
            this.updateRunState();
          })
        : el('p', { class: 'hint', text: 'No parameter is determined by these curves on its own.' }),
    );
  }

  // ----- apply ------------------------------------------------------------------------

  private renderApply(): void {
    keepFocus(this.applyEl, () => this.buildApply());
  }

  /**
   * How the netlist text differs from the one the run fitted: not at all,
   * only in the fitted parameters' value tokens (after "Apply", or an edit of
   * those values), or otherwise (the fitted values belong to another model).
   */
  private netlistChange(run: FitRun): 'none' | 'fitted-values' | 'other' {
    const text = this.host.netlist();
    if (text === run.text) return 'none';
    return withValuesOf(run.text, text, run.names) === text ? 'fitted-values' : 'other';
  }

  private buildApply(): void {
    const r = this.report;
    const run = this.fitRun;
    this.applyEl.replaceChildren();
    if (!r || !run) return;
    const doc = this.host.parameters();
    const current = new Map((doc?.parameters ?? this.doc?.parameters ?? []).map((p) => [p.name, p]));
    const change = this.netlistChange(run);
    const caution = cautions(run.info);
    // Values fitted to another set-up or another netlist are not proposed by default.
    const propose = !caution.modelDiffers && change !== 'other';
    const rows: { name: string; now: number | null; fitted: number; status: string; box: HTMLInputElement; unit: string }[] = [];
    for (const p of r.parameters) {
      const d = current.get(p.name);
      if (!d || d.kind !== 'number') continue;
      const box = el('input', { attrs: { type: 'checkbox', 'aria-label': `Apply ${p.name}`, 'data-param': p.name, 'data-k': `apply-${p.name}` } });
      box.checked = this.applyChoice.get(p.name) ?? (propose && (p.status === 'determined' || p.status === 'weakly_determined'));
      box.addEventListener('change', () => this.applyChoice.set(p.name, box.checked));
      rows.push({ name: p.name, now: typeof d.value === 'number' ? d.value : null, fitted: p.value, status: p.status, box, unit: paramUnit(p.unit) });
    }
    const missing = r.parameters.filter((p) => !current.get(p.name) || current.get(p.name)!.kind !== 'number').map((p) => p.name);
    const msg = el('p', { class: 'hint', attrs: { role: 'status' }, text: this.applied });
    const warnings = [...caution.sentences];
    if (change === 'other') {
      warnings.push('The netlist has changed since this fit in more than the fitted parameters’ values: the values were fitted to the earlier netlist. Fit again before applying them.');
    }
    const t = table(
      'What “Apply” writes into the netlist: the checked fitted values replace the parameters’ values' +
        (propose ? ' (determined and weakly determined ones are checked)' : ' (none is checked: see the notes above)'),
      ['Apply', 'Parameter', 'In the netlist now', 'Fitted', 'Change', 'Status'],
      rows.map((x) => [
        x.box,
        x.name,
        x.now === null ? '—' : `${formatParam(x.now, 6)}${x.unit ? ` ${x.unit}` : ''}`,
        `${formatParam(x.fitted, 6)}${x.unit ? ` ${x.unit}` : ''}`,
        x.now ? `${((x.fitted / x.now - 1) * 100 >= 0 ? '+' : '−')}${Math.abs((x.fitted / x.now - 1) * 100).toFixed(2)} %` : '—',
        el('span', { class: statusClass(x.status), text: words(x.status) }),
      ]),
    );
    t.querySelectorAll('tbody tr').forEach((tr, k) => ((tr as HTMLElement).dataset.param = rows[k].name));
    const apply = button('Apply fitted values', () => {
      const values = rows.filter((x) => x.box.checked).map((x): [string, number] => [x.name, x.fitted]);
      if (!values.length) {
        msg.textContent = 'Nothing is checked.';
        return;
      }
      const before = this.host.netlist();
      this.host.setParameters(values);
      if (this.host.netlist() === before) {
        msg.textContent = 'The netlist was not changed: it is being described again after an edit, or already holds these values. Try again in a moment.';
        return;
      }
      this.applied = `Wrote ${values.map(([n, v]) => `${n} = ${String(v)}`).join(', ')} into the netlist.`;
      msg.textContent = this.applied;
      this.host.announce(this.applied);
    }, { class: 'primary', attrs: { 'data-k': 'apply' } });
    this.applyEl.append(
      el(
        'section',
        { class: 'mv-apply', attrs: { 'aria-labelledby': 'fit-apply-h' } },
        el('h4', { id: 'fit-apply-h', text: 'Apply fitted values', attrs: { tabindex: -1, 'data-k': 'home' } }),
        warnings.length ? el('div', { class: 'mv-apply-warn', attrs: { 'data-field': 'apply-cautions' } }, ...warnings.map((x) => el('p', { text: x }))) : '',
        rows.length ? scrollRegion('Values to apply', t) : el('p', { class: 'hint', text: 'No fitted parameter is a number parameter of the current netlist.' }),
        missing.length ? el('p', { class: 'hint', text: `Not in the current netlist: ${missing.join(', ')}.` }) : '',
        change === 'fitted-values'
          ? el('p', { class: 'hint', text: 'The fitted parameters’ values in the netlist have changed since this fit (applied or edited); the rest of the netlist is as fitted.' })
          : '',
        rows.length ? apply : '',
        msg,
      ),
    );
  }
}

export const view = new FitView();
