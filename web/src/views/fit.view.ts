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
import { renderReport, statusClass, type FitReport, type RunInfo } from './fit-report';
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
  uncertainty: number[] | null;
  model: { values: (number | null)[]; label: string } | null;
  modelError: string | null;
  fitted: (number | null)[] | null;
  gen: number;
  card: HTMLElement;
  fig: FreqFigure | null;
  editing: boolean;
  formError: string | null;
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
  readonly order = 60;
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
  private runUids: number[] = [];
  private runText: string | null = null;
  private jobRunning = false;
  /** Outcome of the last "Apply" (kept until the next fit). */
  private applied = '';

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
      uncertainty: null,
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
    const v = valueOf(await this.call('import_curve', m.text, JSON.stringify(o)));
    if (isEngineError(v)) return `${source}: ${v.error}`;
    m.curve = v as CurveDoc;
    m.uncertainty = null;
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

  private async modelValues(text: string, m: Measured, overrides: Record<string, unknown>): Promise<{ values: (number | null)[]; label: string } | string> {
    const net = this.modelNetlist(text, m.curve);
    if (!net) return 'the netlist is not valid JSON';
    const v = valueOf(await this.call('probe_curve', net, JSON.stringify(overrides), m.probe));
    if (isEngineError(v)) return v.error;
    const c = v as CurveDoc;
    const d = c.sidecar.drive;
    return { values: magnitudeOf(c).values, label: d ? Object.entries(d).map(([k, x]) => `${k} ${String(x)}`).join(', ') : 'the netlist’s sources' };
  }

  /** Compatibility check, uncertainty and model curve of one measured curve (on the worker). */
  private async evaluate(m: Measured): Promise<void> {
    const cur = this.host.current();
    if (!cur) return;
    const gen = ++m.gen;
    const text = cur.text;
    try {
      if (m.uncertainty === null) {
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
    const c = m.curve;
    const f = c.frequencies_Hz;
    m.card.className = `mv-curve${isVirtual(c) ? ' virtual' : ''}`;
    m.card.dataset.curve = m.name;
    const head = el(
      'div',
      { class: 'mv-curve-head' },
      el('span', { class: 'mv-curve-name', text: m.name }),
      originBadge(c),
      el('span', { class: 'hint', text: `${c.quantity}, ${f.length} points, ${formatHz(f[0])} to ${formatHz(f[f.length - 1])}, read as ${m.format.toUpperCase()}` }),
    );

    // Mapping and weights.
    const probeSel = select([['', 'not fitted (no probe)'], ...this.matching(c.quantity).map((p): [string, string] => [p.id, p.id])], m.probe);
    probeSel.addEventListener('change', () => {
      m.probe = probeSel.value;
      m.allow.clear();
      void this.evaluate(m);
      this.updateRunState();
    });
    const weight = el('input', { class: 'mv-num', attrs: { type: 'number', min: 0, step: 'any', value: m.weight, inputmode: 'decimal' } });
    weight.addEventListener('change', () => {
      const w = Number(weight.value);
      m.weight = Number.isFinite(w) && w > 0 ? w : 1;
      weight.value = String(m.weight);
    });
    const phase = select([['auto', 'default (impedance yes, pressure no)'], ['yes', 'fit the phase'], ['no', 'level only']], m.phase);
    phase.addEventListener('change', () => (m.phase = phase.value as Measured['phase']));
    const offset = select([['auto', 'default (from the sidecar)'], ['none', 'none'], ['free', 'free']], m.offset);
    offset.addEventListener('change', () => (m.offset = offset.value as Measured['offset']));
    const cond = el('input', { id: uniqueId('mv-cond'), attrs: { type: 'text', placeholder: 'e.g. added_mass_mg=150', value: m.condition, autocomplete: 'off', spellcheck: 'false' } });
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
      }, { attrs: { 'aria-expanded': String(m.editing) } }),
      button('Load sidecar JSON…', () => sideInput.click()),
      sideInput,
      button('Download curve (CSV + sidecar)', () => void this.downloadCurve(m)),
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
    m.card.replaceChildren(head, controls, compat, summary, actions, ...(form ? [form] : []), ...(m.modelError ? [errorBox('Model not evaluated', m.modelError)] : []), figure);
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
    if (m.fitted) series.push({ id: 'model, fitted values', slot: 2, values: m.fitted });
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
      u ? 'The dashed lines are the measured level ± the combined standard uncertainty of the sidecar’s budget (engine).' : 'The sidecar states no level uncertainty.',
    );
    return el('div', {}, note, m.fig.root);
  }

  private removeCurve(m: Measured): void {
    this.curves = this.curves.filter((c) => c !== m);
    m.card.remove();
    this.host.announce(`Removed ${m.name}.`);
    this.updateRunState();
  }

  private async downloadCurve(m: Measured): Promise<void> {
    const v = valueOf(await this.call('export_curve', JSON.stringify(m.curve), 'csv'));
    if (isEngineError(v)) {
      m.formError = `Not exported: ${v.error}`;
      this.renderCard(m);
      return;
    }
    const x = v as { text: string; sidecar: string; extension: string };
    const base = safeName(m.name.replace(/\.[^.]+$/, ''));
    download(`${base}.csv`, x.text, 'text/csv');
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
    this.rigLast = { ...res, probe: String(spec.probe), seed: Math.round(numv(r.seed)), truth: truth.values };
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
    this.paramsEl.replaceChildren(list);
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

  private async runFit(): Promise<void> {
    const cur = this.host.current();
    const band = this.band();
    if (!cur || typeof band === 'string' || this.jobRunning || this.blocker()) return;
    const ms = this.fitCurves();
    const text = cur.text;
    const names = [...this.free];
    const maxIt = Math.max(1, Math.round(Number(this.maxIt.value)) || 100);
    const base = {
      schema: 'acoustilab-fit/0.1',
      curves: this.curveSpecs(ms),
      ...band,
      allow_spl_only: this.allowSpl.checked,
    };
    const run: RunInfo = { calls: 0, iterations: 0, evaluations: 0, failed: 0, starts: new Map(), curveNames: ms.map((m) => m.name), cancelled: false };
    let starts: Record<string, number> | null = null;
    let last: FitReport | null = null;
    this.jobRunning = true;
    this.updateRunState();
    this.fitJob.start(`Fitting ${names.join(', ')} to ${ms.length} curve${ms.length === 1 ? '' : 's'}…`);
    this.fitJob.progress(0, maxIt);
    this.host.announce('Fit started.');
    let error: { error: string; kind: string } | null = null;
    try {
      for (;;) {
        const spec = {
          ...base,
          parameters: names.map((n) => (starts && n in starts ? { name: n, start: starts[n] } : n)),
          max_iterations: Math.min(ITERATIONS_PER_CALL, maxIt - run.iterations),
        };
        const v = valueOf(await this.call('fit', text, JSON.stringify(spec)));
        if (isEngineError(v)) {
          error = v;
          break;
        }
        const r = v as FitReport;
        last = r;
        run.calls++;
        run.iterations += r.iterations;
        run.evaluations += r.evaluations;
        run.failed += r.failed_evaluations;
        if (run.calls === 1) for (const p of r.parameters) run.starts.set(p.name, p.start);
        const vals = r.parameters.map((p) => `${p.name} ${formatParam(p.value, 5)}`).join(', ');
        this.fitJob.progress(run.iterations, maxIt, `Iteration ${run.iterations} of at most ${maxIt}: reduced χ² ${formatNumber(r.reduced_chi2, 4)}; ${vals}`);
        if (r.converged || r.stop === 'max_evaluations' || r.iterations === 0 || run.iterations >= maxIt) break;
        starts = r.fitted;
      }
    } catch (e) {
      if (!(e instanceof Cancelled)) throw e;
      run.cancelled = true;
    }
    this.jobRunning = false;
    this.updateRunState();
    if (error) {
      const refused = error.kind === 'fit_refused';
      this.fitJob.finish(refused ? 'The engine refused the fit (spec Section 12).' : 'The fit failed.', 'failed');
      this.reportEl.replaceChildren(errorBox(refused ? 'Fit refused' : 'Fit not run', error.error, `Kind: ${error.kind}.`));
      this.host.announce(refused ? 'Fit refused.' : 'Fit failed.');
      return;
    }
    if (!last) {
      this.fitJob.finish('Cancelled before the first step finished.', 'cancelled');
      return;
    }
    this.report = last;
    this.run = run;
    this.applied = '';
    this.runUids = ms.map((m) => m.uid);
    this.runText = text;
    this.fitJob.finish(
      run.cancelled ? `Cancelled after ${run.iterations} iterations; the report is the last completed step’s.` : `${last.converged ? 'Converged' : 'Stopped'} after ${run.iterations} iterations.`,
      run.cancelled ? 'cancelled' : 'done',
    );
    this.host.announce(run.cancelled ? 'Fit cancelled.' : 'Fit done.');
    this.reportEl.replaceChildren(renderReport(last, run));
    await this.fittedCurves(ms, text, last);
    this.renderApply();
  }

  /** The model with the fitted values at each fitted curve's frequencies (drawn with the curve). */
  private async fittedCurves(ms: Measured[], text: string, r: FitReport): Promise<void> {
    for (const m of this.curves) m.fitted = null;
    try {
      for (const m of ms) {
        const cond = parseAssignments(m.condition);
        const res = await this.modelValues(text, m, { ...cond.values, ...r.fitted });
        m.fitted = typeof res === 'string' ? null : res.values;
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
    const r = this.report;
    this.applyEl.replaceChildren();
    if (!r) return;
    const doc = this.host.parameters();
    const current = new Map((doc?.parameters ?? this.doc?.parameters ?? []).map((p) => [p.name, p]));
    const rows: { name: string; now: number | null; fitted: number; status: string; box: HTMLInputElement; unit: string }[] = [];
    for (const p of r.parameters) {
      const d = current.get(p.name);
      if (!d || d.kind !== 'number') continue;
      const box = el('input', { attrs: { type: 'checkbox', 'aria-label': `Apply ${p.name}`, 'data-param': p.name } });
      box.checked = p.status === 'determined' || p.status === 'weakly_determined';
      rows.push({ name: p.name, now: typeof d.value === 'number' ? d.value : null, fitted: p.value, status: p.status, box, unit: paramUnit(p.unit) });
    }
    const missing = r.parameters.filter((p) => !current.get(p.name) || current.get(p.name)!.kind !== 'number').map((p) => p.name);
    const msg = el('p', { class: 'hint', attrs: { role: 'status' }, text: this.applied });
    const t = table(
      'What “Apply” writes into the netlist: the checked fitted values replace the parameters’ values (determined and weakly determined ones are checked)',
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
    }, { class: 'primary' });
    this.applyEl.append(
      el(
        'section',
        { class: 'mv-apply', attrs: { 'aria-labelledby': 'fit-apply-h' } },
        el('h4', { id: 'fit-apply-h', text: 'Apply fitted values' }),
        rows.length ? scrollRegion('Values to apply', t) : el('p', { class: 'hint', text: 'No fitted parameter is a number parameter of the current netlist.' }),
        missing.length ? el('p', { class: 'hint', text: `Not in the current netlist: ${missing.join(', ')}.` }) : '',
        this.runText !== null && this.runText !== this.host.current()?.text ? el('p', { class: 'hint', text: 'The netlist has changed since this fit was made.' }) : '',
        rows.length ? apply : '',
        msg,
      ),
    );
  }
}

export const view = new FitView();
