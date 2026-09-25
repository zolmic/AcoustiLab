// "Sensitivity": which parameters move the response, where, and by how much.
//
// Three results of the engine for one pressure probe of the plotted design
// (docs/analysis.md), computed on this view's worker with progress and
// cancel, and marked stale once the design changes:
//
// * explain sentences: each continuous parameter raised 10 %, re-solved,
//   and the bands of credible frequencies where the level moves, in the
//   engine's own words. Each sentence links to its bands (highlighted on the
//   map and the Response plots) and to its parameter's control;
// * the sensitivity map, parameter × frequency in dB per %, by forward
//   sensitivities (the factorisation is reused; the engine checked them
//   against complete solves, docs/analysis.md), split into calls of a few
//   parameters for progress;
// * a tornado chart of one metric, with each parameter at the ends of its
//   tolerance (or an assumed ±10 %), re-solved.

import { formatParam, paramUnit } from '../format';
import { band, bandName } from '../shading';
import { hzRange } from '../warnings';
import {
  button,
  callJson,
  Cancelled,
  disclosure,
  Diverging,
  el,
  EngineFailure,
  exposeForTests,
  field,
  nextId,
  onThemeChange,
  pressureProbes,
  primaryProbeOf,
  RunBar,
  setOptions,
  signedSig,
  StaleBanner,
  tableBlock,
} from './analysis-ui';
import type { Explanation, Jacobian, Tornado, TornadoMetric } from './analysis-types';
import { HeatMap, scaleValue, type HeatData, type Mapping, type RangeMode, type Scale } from './sensitivity-heatmap';
import { deltaUnit, rangeNote, tornadoLegend, tornadoSvg } from './sensitivity-tornado';
import type { ParamDesc, ResultView, ViewHost } from './types';

/** Parameters per sensitivity call (each call also solves the base design once). */
const SENS_CHUNK = 4;
/** Parameters per tornado call when the metric is a readout (a readout per end). */
const READOUT_CHUNK = 3;
/** Most rows of the map's data table (as the Response table: one grid point in k beyond). */
const MAX_TABLE_ROWS = 1000;

/** Scalar readouts a tornado can use (docs/analysis.md, `Readouts::scalars()`). */
const SCALARS = [
  'coupled_resonance_Hz',
  'bass_extension_Hz',
  'level_500Hz_dB',
  'level_1kHz_dB',
  'sensitivity_500Hz_dB_per_V',
  'sensitivity_1kHz_dB_per_V',
  'sensitivity_500Hz_dB_per_mW',
  'sensitivity_1kHz_dB_per_mW',
  'z_resonance_Hz',
  'z_resonance_ohm',
  'z_max_Hz',
  'z_max_ohm',
  'Re_ohm',
  'Qms',
  'Qes',
  'Qts',
  'z_1kHz_ohm',
  'z_min_above_resonance_Hz',
  'z_min_above_resonance_ohm',
  'z_min_ohm',
  'z_min_over_rated',
];

interface Computed {
  /** The netlist text analysed (the plotted result's). */
  text: string;
  probe: string;
  params: string[];
  jac: Jacobian | null;
  jacError: string | null;
  tornado: Tornado | null;
  tornadoError: string | null;
  explain: Explanation | null;
  explainError: string | null;
}

interface Selected {
  key: string;
  lo: number;
  hi: number;
  label: string;
}

const message = (e: unknown) => (e instanceof EngineFailure ? `${e.message}${e.kind ? ` (${e.kind})` : ''}` : String(e));

/** Continuous parameters a user may choose (numbers; the engine excludes zeros and structure changes itself). */
const candidates = (doc: ParamDesc[]) => doc.filter((p) => p.kind === 'number');
const byDefault = (p: ParamDesc) => p.active !== false && typeof p.value === 'number' && p.value !== 0;

class SensitivityView implements ResultView {
  readonly id = 'sensitivity';
  readonly label = 'Sensitivity';
  readonly order = 20;
  private host!: ViewHost;
  private res: Computed | null = null;
  private params: ParamDesc[] = [];
  /** Chosen parameter names; null: the default (every used, non-zero continuous parameter). */
  private chosen: Set<string> | null = null;
  private selected: Selected | null = null;
  private shown = false;
  private retry = 0;
  private retries = 0;
  /** What the parameter list was last built from (rebuilt only when it changes, so focus stays). */
  private paramSig = '';

  private probeSel = el('select');
  private paramBox = el('details', 'an-params');
  private paramSummary = el('summary');
  private paramList = el('div', 'an-param-list');
  private runBar!: RunBar;
  private stale!: StaleBanner;
  private out = el('div', 'an-output');
  private explainBody = el('div');
  private heatHead = el('h3');
  private heatBody = el('div');
  private heat!: HeatMap;
  private rangeSel = el('select');
  private mappingSel = el('select');
  private scaleLegend = el('div', 'an-scale');
  private heatTable!: ReturnType<typeof disclosure>;
  private tornadoHead = el('h3');
  private tornadoBody = el('div');
  private tornadoChart = el('div', 'an-tornado');
  private tornadoTable!: ReturnType<typeof disclosure>;
  private metricKind = el('select');
  private metricF = el('input');
  private metricLo = el('input');
  private metricHi = el('input');
  private metricReadout = el('select');
  private metricBtn = button('Update tornado');
  private div = new Diverging();

  mount(root: HTMLElement, host: ViewHost): void {
    this.host = host;
    root.classList.add('an-view');
    const intro = el(
      'p',
      'hint',
      'Which parameters move the response of a pressure probe, where and by how much, for the design on the Response tab. ' +
        'Every number is the engine’s, from re-solving the design (docs/analysis.md); only continuous parameters are varied.',
    );

    // Controls.
    this.probeSel.addEventListener('change', () => this.syncControls());
    this.paramBox.append(this.paramSummary, this.paramList);
    this.runBar = new RunBar('Run sensitivity analysis', () => void this.runAll(), () => this.host.cancel());
    const controls = el('div', 'an-controls');
    controls.append(field('Probe', this.probeSel), this.paramBox);
    this.stale = new StaleBanner(() => void this.runAll());

    // Explain.
    const explainSec = this.section('Explain: what moves the level most', this.explainBody);

    // Heat map.
    setOptions(
      this.rangeSel,
      [
        ['credible', 'fit the unshaded band'],
        ['all', 'fit the whole sweep'],
      ],
      'credible',
    );
    this.rangeSel.addEventListener('change', () => this.heat.setRange(this.rangeSel.value as RangeMode));
    setOptions(
      this.mappingSel,
      [
        ['linear', 'linear'],
        ['sqrt', 'square root (small values visible)'],
      ],
      'linear',
    );
    this.mappingSel.addEventListener('change', () => this.heat.setMapping(this.mappingSel.value as Mapping));
    const caption = el('figcaption', 'visually-hidden', 'Sensitivity map');
    this.heat = new HeatMap(caption, (scale) => this.renderScale(scale));
    this.heatTable = disclosure('data table', () => this.heatTableEl());
    onThemeChange(() => {
      this.div.read();
      this.renderScale({ max: this.heat.scaleMax, basis: this.heat.scaleBasis, mapping: this.heat.mapping, saturated: this.heat.scaleSaturated });
    });
    const heatSec = el('section', 'an-section');
    this.heatHead.id = nextId('an-h');
    heatSec.setAttribute('aria-labelledby', this.heatHead.id);
    const heatBar = el('div', 'an-controls');
    heatBar.append(field('Colour scale', this.rangeSel), field('Mapping', this.mappingSel), this.heatTable.button);
    heatSec.append(this.heatHead, heatBar, this.scaleLegend, this.heat.el, this.heatBody, this.heatTable.region);

    // Tornado.
    setOptions(
      this.metricKind,
      [
        ['level', 'level at a frequency'],
        ['band_mean', 'mean level over a band'],
        ['readout', 'a readout'],
      ],
      'level',
    );
    for (const [inp, v] of [
      [this.metricF, '1000'],
      [this.metricLo, '100'],
      [this.metricHi, '1000'],
    ] as const) {
      inp.type = 'number';
      inp.min = '0';
      inp.step = 'any';
      inp.value = v;
      inp.className = 'an-num';
    }
    setOptions(this.metricReadout, SCALARS.map((s) => [s, s]), 'coupled_resonance_Hz');
    this.metricKind.addEventListener('change', () => this.syncControls());
    this.metricBtn.addEventListener('click', () => void this.runTornadoOnly());
    this.tornadoTable = disclosure('data table', () => this.tornadoTableEl());
    const tornadoSec = el('section', 'an-section');
    this.tornadoHead.id = nextId('an-h');
    tornadoSec.setAttribute('aria-labelledby', this.tornadoHead.id);
    const metricBar = el('div', 'an-controls');
    metricBar.append(
      field('Metric', this.metricKind),
      field('Frequency (Hz)', this.metricF, 'an-field an-f'),
      field('From (Hz)', this.metricLo, 'an-field an-band'),
      field('To (Hz)', this.metricHi, 'an-field an-band'),
      field('Readout', this.metricReadout, 'an-field an-readout-name'),
      this.metricBtn,
    );
    tornadoSec.append(this.tornadoHead, metricBar, this.tornadoBody, this.tornadoChart, this.tornadoTable.button, this.tornadoTable.region);
    new ResizeObserver(() => this.renderTornadoChart()).observe(this.tornadoChart);

    this.out.append(explainSec, heatSec, tornadoSec);
    root.append(intro, controls, this.runBar.el, this.stale.el, this.out);
    this.heatHead.textContent = 'Sensitivity map';
    this.tornadoHead.textContent = 'Tornado chart';
    this.syncControls();
    this.render();

    exposeForTests('sensitivity', () => this.testState());
  }

  private section(title: string, body: HTMLElement): HTMLElement {
    const s = el('section', 'an-section');
    const h = el('h3', undefined, title);
    h.id = nextId('an-h');
    s.setAttribute('aria-labelledby', h.id);
    s.append(h, body);
    return s;
  }

  refresh(): void {
    this.shown = true;
    const cur = this.host.current();
    const doc = this.host.parameters();
    // The description of a hand-edited text can land after its solve: look
    // again shortly rather than wait for the next result.
    if (!doc && cur && !this.retry && this.retries < 20) {
      this.retries++;
      this.retry = window.setTimeout(() => {
        this.retry = 0;
        if (this.shown) this.refresh();
      }, 300);
    }
    if (doc) this.retries = 0;
    if (doc) {
      const names = candidates(doc.parameters).map((p) => p.name);
      if (names.join() !== candidates(this.params).map((p) => p.name).join()) this.chosen = null;
      this.params = doc.parameters;
    }
    if (!this.runBar.running) {
      const probes = pressureProbes(cur?.result);
      const keep = this.probeSel.value || this.res?.probe || primaryProbeOf(cur?.text) || probes[0] || null;
      setOptions(this.probeSel, probes.map((p) => [p, p]), keep);
      this.renderParams();
    }
    const stale = this.res !== null && (!cur || cur.text !== this.res.text);
    // A new design's solve has already replaced the band mark on the
    // Response plots (with its own warnings'); the buttons and the map
    // follow, so no button stays pressed for a mark that is gone.
    if (stale && this.selected) this.select(null, '', 0, false);
    this.stale.set(stale, 'these results were');
    this.out.classList.toggle('is-stale', stale);
    this.syncControls();
  }

  hide(): void {
    this.shown = false;
    this.heat.setCursor(null, false);
  }

  // ----- controls ------------------------------------------------------------

  private isChosen(p: ParamDesc): boolean {
    return this.chosen ? this.chosen.has(p.name) : byDefault(p);
  }

  private chosenNames(): string[] {
    return candidates(this.params)
      .filter((p) => this.isChosen(p))
      .map((p) => p.name);
  }

  private paramSigNow(): string {
    return JSON.stringify(candidates(this.params).map((p) => [p.name, p.label, p.active, p.value === 0, this.isChosen(p)]));
  }

  private renderParams(): void {
    const list = candidates(this.params);
    const sig = this.paramSigNow();
    if (sig === this.paramSig) return;
    this.paramSig = sig;
    // The keyboard focus stays on the same control across the rebuild.
    const a = document.activeElement;
    const focusKey = a instanceof HTMLElement && this.paramList.contains(a) ? a.dataset.key : undefined;
    this.paramList.replaceChildren();
    if (!list.length) {
      this.paramSummary.textContent = 'Parameters: none continuous';
      this.paramList.append(el('p', 'hint', 'This netlist declares no continuous parameters (docs/parameters.md).'));
      return;
    }
    const n = list.filter((p) => this.isChosen(p)).length;
    this.paramSummary.textContent = `Parameters: ${n} of ${list.length} continuous`;
    const bar = el('div', 'an-param-bar');
    const all = button('Select all');
    const def = button('Default (used, non-zero)');
    all.dataset.key = 'all';
    def.dataset.key = 'default';
    all.addEventListener('click', () => {
      this.chosen = new Set(list.map((p) => p.name));
      this.renderParams();
      this.syncControls();
    });
    def.addEventListener('click', () => {
      this.chosen = null;
      this.renderParams();
      this.syncControls();
    });
    bar.append(all, def);
    const fs = el('fieldset');
    fs.append(el('legend', 'visually-hidden', 'Parameters to vary'));
    for (const p of list) {
      const l = el('label', 'check an-param');
      const c = el('input');
      c.type = 'checkbox';
      c.checked = this.isChosen(p);
      c.value = p.name;
      c.dataset.key = `p:${p.name}`;
      c.addEventListener('change', () => {
        this.chosen = new Set(this.chosenNames());
        if (c.checked) this.chosen.add(p.name);
        else this.chosen.delete(p.name);
        this.paramSig = this.paramSigNow();
        this.paramSummary.textContent = `Parameters: ${this.chosen.size} of ${list.length} continuous`;
        this.syncControls();
      });
      const note = p.active === false ? ' (not used by this design)' : p.value === 0 ? ' (value 0)' : '';
      l.append(c, `${p.label}${note}`);
      fs.append(l);
    }
    this.paramList.append(bar, fs);
    if (focusKey) this.paramList.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`)?.focus();
  }

  private syncControls(): void {
    const kind = this.metricKind.value;
    for (const [cls, on] of [
      ['an-f', kind === 'level'],
      ['an-band', kind === 'band_mean'],
      ['an-readout-name', kind === 'readout'],
    ] as const) {
      for (const f of this.metricBar().querySelectorAll<HTMLElement>(`.${cls}`)) f.hidden = !on;
    }
    const ready = !!this.host.current() && !!this.probeSel.value && this.chosenNames().length > 0;
    if (!this.runBar.running) {
      this.runBar.run.disabled = !ready;
      if (!this.host.current()) this.runBar.status.textContent = 'Run a netlist first: the analysis uses the design on the Response tab.';
      else if (!this.probeSel.value) this.runBar.status.textContent = 'This design has no pressure probe to analyse.';
      else if (!candidates(this.params).length) this.runBar.status.textContent = 'This netlist declares no continuous parameters to vary (docs/parameters.md).';
      else if (!this.chosenNames().length) this.runBar.status.textContent = 'Choose at least one parameter.';
      else if (!this.res) this.runBar.status.textContent = '';
    }
    this.metricBtn.disabled = !this.res || this.runBar.running;
  }

  private metricBar(): HTMLElement {
    return this.metricKind.closest('.an-controls') as HTMLElement;
  }

  private metric(probe: string): TornadoMetric | string {
    const num = (i: HTMLInputElement) => Number(i.value);
    switch (this.metricKind.value) {
      case 'level': {
        const f = num(this.metricF);
        return f > 0 ? { kind: 'level', probe, f_Hz: f } : 'Enter a frequency above 0 Hz.';
      }
      case 'band_mean': {
        const a = num(this.metricLo);
        const b = num(this.metricHi);
        return a > 0 && b > a ? { kind: 'band_mean', probe, f_min_Hz: a, f_max_Hz: b } : 'Enter a band with 0 < from < to.';
      }
      default:
        return { kind: 'readout', name: this.metricReadout.value };
    }
  }

  // ----- running -----------------------------------------------------------

  private async runAll(): Promise<void> {
    const cur = this.host.current();
    const probe = this.probeSel.value;
    const params = this.chosenNames();
    if (!cur || !probe || !params.length || this.runBar.running) return;
    const metric = this.metric(probe);
    const text = cur.text;
    const chunks: string[][] = [];
    for (let k = 0; k < params.length; k += SENS_CHUNK) chunks.push(params.slice(k, k + SENS_CHUNK));
    const tChunks = typeof metric !== 'string' && metric.kind === 'readout' ? Math.ceil(params.length / READOUT_CHUNK) : 1;
    const total = chunks.length + tChunks + 1;
    const res: Computed = { text, probe, params, jac: null, jacError: null, tornado: null, tornadoError: null, explain: null, explainError: null };
    this.clearHighlight();
    this.runBar.start(total, `Sensitivity map: 0 of ${params.length} parameters…`);
    this.syncControls();
    let done = 0;
    try {
      try {
        for (const c of chunks) {
          const part = await callJson<Jacobian>(
            this.host,
            'sensitivity',
            text,
            '',
            JSON.stringify({ parameters: c, probes: [probe], method: 'forward_sensitivity' }),
          );
          if (!res.jac) res.jac = part;
          else {
            res.jac.parameters.push(...part.parameters);
            res.jac.excluded.push(...part.excluded);
          }
          done++;
          this.runBar.step(done, `Sensitivity map: ${Math.min(params.length, done * SENS_CHUNK)} of ${params.length} parameters…`);
        }
      } catch (e) {
        if (e instanceof Cancelled) throw e;
        res.jacError = message(e);
        done = chunks.length;
      }
      this.runBar.step(done, `Tornado chart…`);
      if (typeof metric === 'string') res.tornadoError = metric;
      else {
        try {
          res.tornado = await this.tornado(text, metric, params, (k) => this.runBar.step(done + k, `Tornado chart: part ${k + 1} of ${tChunks}…`));
        } catch (e) {
          if (e instanceof Cancelled) throw e;
          res.tornadoError = message(e);
        }
      }
      done += tChunks;
      this.runBar.step(done, 'Explain sentences…');
      try {
        res.explain = await callJson<Explanation>(this.host, 'explain', text, '', JSON.stringify({ probe, parameters: params }));
      } catch (e) {
        if (e instanceof Cancelled) throw e;
        res.explainError = message(e);
      }
    } catch (e) {
      if (!(e instanceof Cancelled)) throw e;
      this.runBar.finish(this.res ? 'Cancelled; the results below are from the previous run.' : 'Cancelled.');
      this.host.announce('Sensitivity analysis cancelled.');
      this.syncControls();
      return;
    }
    this.res = res;
    this.runBar.finish(
      `Done: ${res.jac ? `${res.jac.parameters.length} parameters on ${res.jac.frequencies_Hz.length} frequencies` : 'no map'}` +
        `${res.jac?.excluded.length ? `, ${res.jac.excluded.length} excluded` : ''}; engine ${res.jac?.engine ?? res.explain?.engine ?? ''}.`,
    );
    this.host.announce('Sensitivity analysis finished.');
    this.render();
    this.refresh();
  }

  /** A tornado of `metric`; a readout metric is split into calls of a few parameters for progress. */
  private async tornado(text: string, metric: TornadoMetric, params: string[], progress: (k: number) => void): Promise<Tornado> {
    if (metric.kind !== 'readout') return callJson<Tornado>(this.host, 'tornado', text, '', JSON.stringify({ metric, parameters: params }));
    let merged: Tornado | null = null;
    for (let k = 0; k * READOUT_CHUNK < params.length; k++) {
      progress(k);
      const part = await callJson<Tornado>(
        this.host,
        'tornado',
        text,
        '',
        JSON.stringify({ metric, parameters: params.slice(k * READOUT_CHUNK, (k + 1) * READOUT_CHUNK) }),
      );
      if (!merged) merged = part;
      else {
        merged.rows.push(...part.rows);
        merged.excluded.push(...part.excluded);
      }
    }
    // The engine's order: larger absolute change first, ties in parameter
    // order (a stable sort over the rows in request order, as in one call).
    // A non-finite effect arrives as null and goes last.
    const e = (x: number | null) => (x !== null && Number.isFinite(x) ? x : -Infinity);
    merged!.rows.sort((a, b) => (e(b.effect) > e(a.effect) ? 1 : e(b.effect) < e(a.effect) ? -1 : 0));
    return merged!;
  }

  private async runTornadoOnly(): Promise<void> {
    const res = this.res;
    if (!res || this.runBar.running) return;
    const metric = this.metric(res.probe);
    if (typeof metric === 'string') {
      res.tornado = null;
      res.tornadoError = metric;
      this.renderTornado();
      return;
    }
    const n = metric.kind === 'readout' ? Math.ceil(res.params.length / READOUT_CHUNK) : 1;
    this.runBar.start(n, 'Tornado chart…');
    this.syncControls();
    try {
      res.tornado = await this.tornado(res.text, metric, res.params, (k) => this.runBar.step(k, `Tornado chart: part ${k + 1} of ${n}…`));
      res.tornadoError = null;
      this.runBar.finish('Tornado chart updated.');
    } catch (e) {
      if (!(e instanceof Cancelled)) {
        res.tornado = null;
        res.tornadoError = message(e);
        this.runBar.finish('The tornado chart failed; see below.');
      } else this.runBar.finish('Cancelled.');
    }
    this.syncControls();
    this.renderTornado();
  }

  // ----- rendering ------------------------------------------------------------

  private render(): void {
    this.renderExplain();
    this.renderHeat();
    this.renderTornado();
  }

  private renderExplain(): void {
    const b = this.explainBody;
    b.replaceChildren();
    const r = this.res;
    if (!r) {
      b.append(el('p', 'hint', 'Run the analysis to see the engine’s sentences.'));
      return;
    }
    if (r.explainError) {
      b.append(el('p', 'an-error', `The engine could not explain: ${r.explainError}`));
      return;
    }
    const x = r.explain!;
    b.append(
      el(
        'p',
        'hint',
        `Each parameter ${x.step_pct} % up (down where up would pass its maximum), re-solved; bands where ${x.probe} moves by more than ` +
          `${x.threshold_dB} dB, over credible (unshaded) frequencies only. Band buttons mark the band on the map and the Response plots.`,
      ),
    );
    if (!x.sentences.length) b.append(el('p', undefined, `No parameter moves ${x.probe} by ${x.threshold_dB} dB or more in the credible band.`));
    const ol = el('ol', 'an-explain');
    for (const s of x.sentences) {
      const li = el('li');
      li.dataset.param = s.parameter;
      li.append(el('p', 'an-sentence', s.text));
      const links = el('div', 'an-links');
      const go = button(`Go to “${s.label}”`, 'linkish');
      go.setAttribute('aria-label', `Go to the control of ${s.label} in the Design tab`);
      go.addEventListener('click', () => this.host.focusParameter(s.parameter));
      links.append(go);
      const bandButton = (k: number) => {
        const bd = s.bands[k];
        const key = `${s.parameter}#${k}`;
        const range = hzRange(bd.f_min_Hz, bd.f_max_Hz);
        const btn = button(range, 'an-band-btn');
        btn.dataset.key = key;
        btn.setAttribute('aria-pressed', String(this.selected?.key === key));
        btn.setAttribute('aria-label', `Mark ${range} on the map and the Response plots`);
        btn.addEventListener('click', () => {
          const label = `${s.label} ${s.direction === 'raise' ? '+' : '−'}${x.step_pct} %: ${range}`;
          this.select(this.selected?.key === key ? null : { key, lo: bd.f_min_Hz, hi: bd.f_max_Hz, label }, s.parameter, bd.at_Hz);
        });
        return btn;
      };
      const stated = new Set(s.stated);
      for (const k of s.stated) links.append(bandButton(k));
      const others = s.bands.map((_, k) => k).filter((k) => !stated.has(k));
      if (others.length) {
        const d = el('details', 'an-more-bands');
        d.append(el('summary', undefined, `${others.length} more band${others.length === 1 ? '' : 's'}`));
        for (const k of others) {
          const bd = s.bands[k];
          const row = el('p');
          row.append(bandButton(k), ` ${bd.effect} by ${Math.abs(bd.mean_dB).toFixed(2)} dB on average`);
          d.append(row);
        }
        links.append(d);
      }
      li.append(links);
      ol.append(li);
    }
    b.append(ol);
    if (x.quiet.length) {
      const labels = new Map(this.params.map((p) => [p.name, p.label]));
      b.append(
        el(
          'p',
          'hint',
          `Largest change below ${x.threshold_dB} dB, or not among the top sentences: ` +
            x.quiet.map((q) => `${labels.get(q.name) ?? q.name} (${q.effect_dB.toFixed(2)} dB)`).join(', ') +
            '.',
        ),
      );
    }
    if (x.skipped.length) b.append(el('p', 'hint', `Skipped: ${x.skipped.map((q) => `${q.name} (${q.reason})`).join('; ')}.`));
  }

  /** Marks an explain band (or clears it) on the map and the Response plots; the map's crosshair goes to its largest change. */
  private select(sel: Selected | null, param: string, atHz: number, toPlots = true): void {
    this.selected = sel;
    for (const b of this.explainBody.querySelectorAll<HTMLElement>('.an-band-btn')) b.setAttribute('aria-pressed', String(b.dataset.key === sel?.key));
    const h = sel ? { lo: sel.lo, hi: sel.hi, label: sel.label } : null;
    this.heat.setHighlight(h);
    if (toPlots) this.host.highlight(h);
    const jac = this.res?.jac;
    if (sel && jac) {
      const row = jac.parameters.findIndex((p) => p.name === param);
      const f = jac.frequencies_Hz;
      let col = 0;
      for (let k = 1; k < f.length; k++) if (Math.abs(Math.log(f[k] / atHz)) < Math.abs(Math.log(f[col] / atHz))) col = k;
      if (row >= 0) this.heat.setCursor({ row, col }, false);
    }
    if (toPlots) this.host.announce(sel ? `Marked ${sel.label}.` : 'Band mark cleared.');
  }

  private clearHighlight(): void {
    if (this.selected) this.select(null, '', 0);
  }

  private heatData(): HeatData | null {
    const r = this.res;
    if (!r?.jac) return null;
    const j = r.jac;
    const k = Math.max(0, j.probes.indexOf(r.probe));
    return {
      freqs: j.frequencies_Hz,
      rows: j.parameters.map((p) => ({ name: p.name, label: p.label, values: p.dB_per_pct[k] ?? [] })),
      shading: j.shading,
    };
  }

  private renderHeat(): void {
    const r = this.res;
    this.heatHead.textContent = r ? `Sensitivity map: dB per % of ${r.probe}` : 'Sensitivity map';
    this.heatBody.replaceChildren();
    this.heat.set(this.heatData());
    this.heat.el.hidden = !r?.jac;
    this.scaleLegend.hidden = !r?.jac;
    this.heatTable.button.hidden = !r?.jac;
    this.heatTable.refresh();
    if (!r) {
      this.heatBody.append(el('p', 'hint', 'Run the analysis to see the map.'));
      return;
    }
    if (r.jacError) {
      this.heatBody.append(el('p', 'an-error', `The sensitivity map failed: ${r.jacError}`));
      return;
    }
    const j = r.jac!;
    const notes = el('ul', 'an-notes');
    const labels = new Map(j.parameters.map((p) => [p.name, p.label]));
    for (const p of j.parameters) {
      if (p.scheme !== 'central') notes.append(el('li', undefined, `${p.label}: ${p.scheme} differences${p.note ? ` (${p.note})` : ''}.`));
      for (const w of p.warnings) notes.append(el('li', undefined, `${p.label}: ${w}`));
    }
    for (const x of j.excluded) notes.append(el('li', undefined, `Excluded: ${labels.get(x.name) ?? x.name} (${x.name}): ${x.reason}.`));
    this.heatBody.append(el('p', 'hint', `Method: ${j.method}; step ${j.step}.`));
    if (notes.childElementCount) this.heatBody.append(notes);
  }

  private renderScale({ max, basis, mapping, saturated }: Scale): void {
    const l = this.scaleLegend;
    l.replaceChildren();
    if (!(max > 0)) return;
    const stops = Array.from({ length: 21 }, (_, i) => `${this.div.css(-1 + i / 10)} ${i * 5}%`);
    const bar = el('div', 'an-scale-bar');
    bar.style.background = `linear-gradient(to right, ${stops.join(', ')})`;
    bar.setAttribute('aria-hidden', 'true');
    const ticks = el('div', 'an-scale-ticks');
    ticks.setAttribute('aria-hidden', 'true');
    // Ticks at the ends, halfway and the middle of the bar, labelled with
    // the values there under the chosen mapping; the ends say when cells
    // lie beyond them.
    const v = (u: number) => {
      const x = scaleValue(u, max, mapping);
      return x === 0 ? '0' : `${x < 0 ? '−' : '+'}${Math.abs(x).toPrecision(2)}`;
    };
    ticks.append(
      el('span', undefined, `${saturated ? '≤ ' : ''}${v(-1)}`),
      el('span', undefined, v(-0.5)),
      el('span', undefined, '0'),
      el('span', undefined, v(0.5)),
      el('span', undefined, `${saturated ? '≥ ' : ''}${v(1)}`),
    );
    const m = max.toPrecision(3);
    const text = el(
      'p',
      'hint',
      `Colour: dB per %, −${m} to +${m}: ${basis === 'credible' ? 'the largest magnitude in the unshaded band' : 'the largest magnitude in the sweep'}` +
        `${saturated ? '; cells beyond it (in the shaded band) show the end colours' : ''}. ` +
        (mapping === 'sqrt'
          ? `Square-root mapping: the colour's distance from grey is the square root of the value's share of ${m}, so ±${scaleValue(0.5, max, mapping).toPrecision(2)} is half-way. `
          : 'Linear mapping: the colour is proportional to the value. ') +
        'Positive (warm): the level rises as the parameter rises; negative (cool): it falls; grey: no change. Hatched columns: validity shading.',
    );
    l.append(bar, ticks, text);
  }

  private heatTableEl(): Node {
    const d = this.heatData();
    const r = this.res;
    if (!d || !r) return el('p');
    const n = d.freqs.length;
    const stride = Math.max(1, Math.ceil(n / MAX_TABLE_ROWS));
    const idx: number[] = [];
    for (let i = 0; i < n; i += stride) idx.push(i);
    if (idx[idx.length - 1] !== n - 1) idx.push(n - 1);
    const rows = idx.map((i) => [
      formatParam(d.freqs[i], 6),
      bandName(band(d.shading, d.freqs[i])),
      ...d.rows.map((row) => (row.values[i] === null ? '' : row.values[i]!.toPrecision(4))),
    ]);
    return tableBlock(
      `Sensitivity map of ${r.probe}, dB per %`,
      `dB per % of ${r.probe} at ${stride > 1 ? `every ${stride}th of the ${n}` : `each of the ${n}`} grid frequencies. Validity: "light" = lumped-model error ≥ 10 % or below an element’s validated range, "dark" = ≥ 36 % or past a hard limit.`,
      ['Frequency (Hz)', 'Validity', ...d.rows.map((x) => x.label)],
      rows,
    ).el;
  }

  private renderTornado(): void {
    const r = this.res;
    this.tornadoBody.replaceChildren();
    this.tornadoChart.replaceChildren();
    this.tornadoTable.button.hidden = !r?.tornado;
    this.tornadoTable.refresh();
    this.tornadoHead.textContent = r?.tornado ? `Tornado chart: ${r.tornado.description}` : 'Tornado chart';
    if (!r) {
      this.tornadoBody.append(el('p', 'hint', 'Run the analysis to see the tornado chart.'));
      return;
    }
    if (r.tornadoError) {
      this.tornadoBody.append(el('p', 'an-error', `The tornado chart failed: ${r.tornadoError}`));
      return;
    }
    const t = r.tornado!;
    const u = deltaUnit(t);
    this.tornadoBody.append(
      el(
        'p',
        'hint',
        `Base ${formatParam(t.base, 6)} ${u}${t.shading ? ` (in the ${t.shading === 2 ? 'dark' : 'light'} validity band)` : ''}. ` +
          'Each parameter at the ends of its tolerance (2σ for normal and log-normal, the full range for uniform), or ±10 % where it has none, re-solved; largest change first.',
      ),
      tornadoLegend(),
    );
    if (t.excluded.length) this.tornadoBody.append(el('p', 'hint', `Excluded: ${t.excluded.map((x) => `${x.name} (${x.reason})`).join('; ')}.`));
    const errs = t.rows.filter((x) => x.errors.length);
    if (errs.length) this.tornadoBody.append(el('p', 'an-error', errs.map((x) => `${x.label}: ${x.errors.join('; ')}`).join(' ')));
    this.renderTornadoChart();
  }

  private renderTornadoChart(): void {
    const t = this.res?.tornado;
    if (!t || this.res?.tornadoError) return;
    const w = this.tornadoChart.clientWidth;
    if (w === 0) return;
    const units = new Map(this.params.map((p) => [p.name, paramUnit(p.unit)]));
    this.tornadoChart.replaceChildren(tornadoSvg(t, w, units));
  }

  private tornadoTableEl(): Node {
    const t = this.res?.tornado;
    if (!t) return el('p');
    const u = deltaUnit(t);
    const units = new Map(this.params.map((p) => [p.name, paramUnit(p.unit)]));
    const v = (x: number, name: string) => `${formatParam(x, 5)}${units.get(name) ? ` ${units.get(name)}` : ''}`;
    const d = (x: number | null) => (x === null ? 'n/a' : signedSig(x, 4));
    const b = tableBlock(
      `Tornado chart of ${t.description}`,
      `Change of ${t.description} (${u}) with each parameter at the low and high end of its range; base ${formatParam(t.base, 6)} ${u}. Largest change first.`,
      ['Parameter', 'Range', 'Low end', 'High end', `Δ at low end (${u})`, `Δ at high end (${u})`],
      t.rows.map((r) => [`${r.label} (${r.name})`, rangeNote(r, units.get(r.name) ?? null), v(r.low_value, r.name), v(r.high_value, r.name), d(r.delta_low), d(r.delta_high)]),
    );
    b.table.dataset.table = 'tornado';
    return b.el;
  }

  private testState(): unknown {
    const r = this.res;
    return {
      text: r?.text ?? null,
      probe: r?.probe ?? null,
      params: r?.params ?? [],
      stale: !this.stale.el.hidden,
      rows: r?.jac?.parameters.map((p) => p.name) ?? [],
      freqs: r?.jac?.frequencies_Hz ?? [],
      /** The map's values as drawn: [row][frequency], dB per %. */
      map: this.heatData()?.rows.map((x) => x.values) ?? [],
      cursor: this.heat.cursor,
      scaleMax: this.heat.scaleMax,
      scale: { basis: this.heat.scaleBasis, mapping: this.heat.mapping, saturated: this.heat.scaleSaturated },
      geometry: this.heat.geometry(),
      selected: this.selected?.key ?? null,
      highlight: this.heat.highlight,
      tornado: r?.tornado?.rows.map((x) => x.name) ?? [],
      sentences: r?.explain?.sentences.map((s) => s.text) ?? [],
    };
  }
}

export const view = new SensitivityView();
