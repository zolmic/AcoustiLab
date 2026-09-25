// "Tolerance": a Monte Carlo run over the parameters' tolerances
// (docs/analysis.md, "Monte Carlo and design of experiments").
//
// The engine plans a Latin hypercube of N runs from a seed (`mc_plan`),
// solves them in chunks on this view's worker (`mc_run`, with progress and
// cancel between chunks), and summarises them (`mc_envelope`): per
// frequency the median, 10–90 %, 5–95 % and min–max of the chosen probe,
// drawn as nested bands around the nominal curve (the plotted design), and
// the same statistics of every scalar readout. The design-of-experiments
// table (`mc_csv`) carries each run's reproducibility hash.

import { formatHz, formatNumber, formatParam, paramUnit, prettyUnit } from '../format';
import { lineKey } from '../keys';
import { PlotPanel } from '../plot';
import { chooseScale, type BandSeries, type PlotGroup } from '../series';
import { band, bandName, bandNote } from '../shading';
import type { Num, ProbeResult } from '../types';
import {
  button,
  callJson,
  Cancelled,
  tableBlock,
  disclosure,
  el,
  EngineFailure,
  exposeForTests,
  field,
  nextId,
  primaryProbeOf,
  RunBar,
  setOptions,
  StaleBanner,
  download,
} from './analysis-ui';
import type { CurveStats, Envelope, Plan, RunChunk, RunSample } from './analysis-types';
import { binValues, histogramSvg } from './tolerance-histogram';
import type { ParamDesc, ResultView, SolveResult, ViewHost } from './types';

/** Runs per `mc_run` call (about half a second each on the template). */
const CHUNK = 20;
const MAX_RUNS = 5000;
const MAX_TABLE_ROWS = 1000;

interface Computed {
  text: string;
  spec: { method: 'lhs'; n: number; seed: number };
  plan: Plan;
  engine: string;
  freqs: number[];
  samples: RunSample[];
  envelope: Envelope;
  /** The plotted design's curves (the nominal), by probe id. */
  nominal: Map<string, { quantity: string; unit: string; values: Num[]; dB: boolean }>;
  shading: SolveResult['shading'];
}

/** The statistic of a probe the envelope reports: dB for pressures, else the magnitude. */
function curveStats(env: Envelope, id: string): { stats: CurveStats; dB: boolean } | null {
  const p = env.probes.find((q) => q.id === id);
  if (!p) return null;
  if (p.dB) return { stats: p.dB, dB: true };
  if (p.magnitude) return { stats: p.magnitude, dB: false };
  return null;
}

/** Probes a run records: the pressures and impedances (the rest would only grow the data). */
const runProbes = (r: SolveResult) => r.probes.filter((p) => p.spl_dB || p.quantity === 'impedance');

/** "normal ±12.27 Hz (2σ)", "log-normal ×/÷ 1.5 (2σ)", "uniform ±0.2 mm". */
function distText(d: Plan['distributions'][number], unit: string): string {
  const u = unit ? ` ${unit}` : '';
  if (d.dist === 'lognormal') return `log-normal ×/÷ ${formatParam(1 + d.half_width / d.nominal, 4)} (2σ)`;
  if (d.dist === 'uniform') return `uniform ±${formatParam(d.half_width, 4)}${u}`;
  return `normal ±${formatParam(d.half_width, 4)}${u} (2σ)`;
}

class ToleranceView implements ResultView {
  readonly id = 'tolerance';
  readonly label = 'Tolerance';
  readonly order = 30;
  private host!: ViewHost;
  private res: Computed | null = null;
  private params: ParamDesc[] = [];

  private probeSel = el('select');
  private nInput = el('input');
  private seedInput = el('input');
  private runBar!: RunBar;
  private stale!: StaleBanner;
  private out = el('div', 'an-output');
  private summary = el('div');
  private plotHead = el('h3');
  private legend = el('div', 'an-legend');
  private readout = el('p', 'an-readout');
  private spoken = el('p', 'visually-hidden');
  private plots = el('div', 'an-plots');
  private panel!: PlotPanel;
  private plotTable!: ReturnType<typeof disclosure>;
  private distBody = el('div');
  private metricBody = el('div');
  private metricSel = el('select');
  private hist = el('div', 'an-hist');
  private histTable!: ReturnType<typeof disclosure>;
  private doeBody = el('div');

  mount(root: HTMLElement, host: ViewHost): void {
    this.host = host;
    root.classList.add('an-view');
    const intro = el(
      'p',
      'hint',
      'How the design on the Response tab spreads when its parameters vary within their tolerances: a Latin hypercube of runs, ' +
        'each solved by the engine (docs/analysis.md). The same seed gives the same runs on every platform.',
    );
    this.nInput.type = 'number';
    this.nInput.min = '2';
    this.nInput.max = String(MAX_RUNS);
    this.nInput.step = '1';
    this.nInput.value = '200';
    this.nInput.className = 'an-num';
    this.seedInput.type = 'number';
    this.seedInput.min = '0';
    this.seedInput.step = '1';
    this.seedInput.value = '1';
    this.seedInput.className = 'an-num';
    const newSeed = button('New seed');
    newSeed.addEventListener('click', () => {
      this.seedInput.value = String(Math.floor(Math.random() * 2 ** 31));
    });
    this.probeSel.addEventListener('change', () => this.renderPlot());
    const controls = el('div', 'an-controls');
    controls.append(field('Probe', this.probeSel), field('Runs (N)', this.nInput), field('Seed', this.seedInput), newSeed);
    this.runBar = new RunBar('Run Monte Carlo', () => void this.run(), () => this.host.cancel());
    this.stale = new StaleBanner(() => void this.run());

    this.panel = new PlotPanel(this.plots, {
      onCursor: (i, fromKeyboard) => this.renderReadout(i, fromKeyboard),
      onView: () => this.plotTable?.refresh(),
    });
    this.spoken.setAttribute('aria-live', 'polite');
    this.plotTable = disclosure('data table', () => this.plotTableEl());
    const plotSec = this.section(this.plotHead, [this.summary, this.legend, this.readout, this.spoken, this.plots, this.plotTable.button, this.plotTable.region]);
    this.plotHead.textContent = 'Envelopes';

    const distSec = this.section(el('h3', undefined, 'Varied parameters'), [this.distBody]);
    this.metricSel.addEventListener('change', () => this.renderHistogram());
    this.histTable = disclosure('bin counts', () => this.histTableEl());
    const metricBar = el('div', 'an-controls');
    metricBar.append(field('Histogram of', this.metricSel), this.histTable.button);
    const metricSec = this.section(el('h3', undefined, 'Readouts over the runs'), [metricBar, this.hist, this.histTable.region, this.metricBody]);
    const doeSec = this.section(el('h3', undefined, 'Design-of-experiments table'), [this.doeBody]);
    new ResizeObserver(() => this.renderHistogram()).observe(this.hist);

    this.out.append(plotSec, distSec, metricSec, doeSec);
    root.append(intro, controls, this.runBar.el, this.stale.el, this.out);
    this.render();
    exposeForTests('tolerance', () => this.testState());
  }

  private section(head: HTMLElement, body: Node[]): HTMLElement {
    const s = el('section', 'an-section');
    head.id ||= nextId('an-h');
    s.setAttribute('aria-labelledby', head.id);
    s.append(head, ...body);
    return s;
  }

  refresh(): void {
    const cur = this.host.current();
    const doc = this.host.parameters();
    if (doc) this.params = doc.parameters;
    if (!this.runBar.running && !this.res) {
      const ids = cur ? runProbes(cur.result).map((p) => p.id) : [];
      setOptions(this.probeSel, ids.map((p) => [p, p]), this.probeSel.value || primaryProbeOf(cur?.text) || ids[0] || null);
    }
    const stale = this.res !== null && (!cur || cur.text !== this.res.text);
    this.stale.set(stale, 'these runs were');
    this.out.classList.toggle('is-stale', stale);
    if (!this.runBar.running) {
      this.runBar.run.disabled = !cur;
      if (!cur) this.runBar.status.textContent = 'Run a netlist first: the runs vary the design on the Response tab.';
      else if (!this.res && this.runBar.status.textContent?.startsWith('Run a netlist')) this.runBar.status.textContent = '';
    }
  }

  // ----- running -----------------------------------------------------------

  private async run(): Promise<void> {
    const cur = this.host.current();
    if (!cur || this.runBar.running) return;
    const n = Math.round(Number(this.nInput.value));
    const seed = Math.round(Number(this.seedInput.value));
    if (!(n >= 2 && n <= MAX_RUNS)) {
      this.runBar.status.textContent = `Enter between 2 and ${MAX_RUNS} runs.`;
      this.nInput.focus();
      return;
    }
    if (!(seed >= 0 && seed <= Number.MAX_SAFE_INTEGER)) {
      this.runBar.status.textContent = 'Enter a whole seed from 0 to 2⁵³ − 1.';
      this.seedInput.focus();
      return;
    }
    this.nInput.value = String(n);
    this.seedInput.value = String(seed);
    const text = cur.text;
    const spec = { method: 'lhs' as const, n, seed };
    const probes = runProbes(cur.result);
    const nominal = new Map(
      probes.map((p: ProbeResult) => [p.id, { quantity: p.quantity, unit: p.unit, values: p.spl_dB ?? p.magnitude, dB: !!p.spl_dB }]),
    );
    this.runBar.start(n + 2, 'Planning…');
    try {
      const plan = await callJson<Plan>(this.host, 'mc_plan', text, '', JSON.stringify(spec));
      const samples: RunSample[] = [];
      let freqs: number[] = [];
      let engine = plan.engine;
      for (let first = 0; first < n; first += CHUNK) {
        this.runBar.step(1 + first, `Run ${first} of ${n} solved…`);
        const chunk = await callJson<RunChunk>(
          this.host,
          'mc_run',
          text,
          '',
          JSON.stringify({ plan: spec, first, count: CHUNK }),
          JSON.stringify({ probes: probes.map((p) => p.id), metrics: true }),
        );
        freqs = chunk.frequencies_Hz;
        engine = chunk.engine;
        samples.push(...chunk.samples);
      }
      this.runBar.step(n + 1, `All ${n} runs solved; summarising…`);
      const envelope = await callJson<Envelope>(this.host, 'mc_envelope', JSON.stringify({ engine, frequencies_Hz: freqs, samples }));
      this.res = { text, spec, plan, engine, freqs, samples, envelope, nominal, shading: cur.result.shading };
      const failed = samples.filter((s) => !s.ok).length;
      this.runBar.finish(`Done: ${n} runs${failed ? `, ${failed} failed` : ''}, seed ${seed}.`);
      this.host.announce('Monte Carlo run finished.');
      if (![...this.probeSel.options].some((o) => o.value === this.probeSel.value)) this.probeSel.value = probes[0]?.id ?? '';
      this.render();
      this.refresh();
    } catch (e) {
      if (e instanceof Cancelled) {
        this.runBar.finish(this.res ? 'Cancelled; the results below are from the previous run.' : 'Cancelled.');
        this.host.announce('Monte Carlo run cancelled.');
      } else {
        this.runBar.finish(`The run failed: ${e instanceof EngineFailure ? e.message : String(e)}`);
      }
      this.refresh();
    }
  }

  // ----- rendering ------------------------------------------------------------

  private render(): void {
    this.renderSummary();
    this.renderPlot();
    this.renderDistributions();
    this.renderMetrics();
    this.renderDoe();
  }

  private renderSummary(): void {
    const r = this.res;
    this.summary.replaceChildren();
    if (!r) {
      this.summary.append(el('p', 'hint', 'Run the Monte Carlo to see the envelopes.'));
      return;
    }
    const e = r.envelope;
    const dl = el('dl', 'an-dl');
    const add = (k: string, v: string) => dl.append(el('dt', undefined, k), el('dd', undefined, v));
    add('Runs', `${e.runs} (${e.failed} failed${e.other_grid ? `, ${e.other_grid} on another frequency grid` : ''})`);
    add('Plan', `Latin hypercube, seed ${r.spec.seed}, ${r.plan.parameters.length} parameters with tolerances`);
    add('Clipped samples', `${r.plan.clipped} (values clipped to a parameter’s bounds)`);
    add('Percentiles', e.percentile_method);
    add('Engine', r.engine);
    this.summary.append(dl);
  }

  private renderPlot(): void {
    const r = this.res;
    const id = this.probeSel.value;
    this.legend.replaceChildren();
    this.plotTable.button.hidden = !r;
    const cs = r ? curveStats(r.envelope, id) : null;
    const nom = r?.nominal.get(id);
    if (!r || !cs || !nom) {
      this.plotHead.textContent = 'Envelopes';
      this.plots.replaceChildren();
      this.readout.hidden = true;
      this.plotTable.refresh();
      return;
    }
    this.readout.hidden = false;
    const s = cs.stats;
    this.plotHead.textContent = `Envelopes of ${id} over ${r.envelope.runs} runs`;
    const bands: BandSeries[] = [
      { probe: 0, id: 'minmax', label: 'min–max of the runs', lower: s.min, upper: s.max, alpha: 0.08, edge: [1, 3] },
      { probe: 0, id: 'p5p95', label: '5–95 % of the runs', lower: s.p5, upper: s.p95, alpha: 0.14, edge: [4, 3] },
      { probe: 0, id: 'p10p90', label: '10–90 % of the runs', lower: s.p10, upper: s.p90, alpha: 0.24, edge: [] },
    ];
    const g: PlotGroup = {
      key: 'mc',
      kind: cs.dB ? 'spl' : 'mag',
      title: cs.dB ? 'Sound pressure level' : nom.quantity === 'impedance' ? 'Impedance magnitude' : 'Magnitude',
      symbol: cs.dB ? 'SPL' : '|Z|',
      unit: cs.dB ? '' : nom.unit,
      axisUnit: cs.dB ? 'dB re 20 µPa' : prettyUnit(nom.unit),
      scale: 'linear',
      series: [
        { probe: 0, id: 'nominal', values: nom.values, primary: true },
        { probe: 1, id: 'median', values: s.median },
      ],
      overlays: [],
      bands,
      height: 'main',
    };
    if (!cs.dB) g.scale = chooseScale([{ probe: 0, id: 'x', values: [...s.min, ...s.max] }]);
    this.panel.setGroups([g], { freqs: r.freqs, shading: r.shading, highlight: null });
    this.panel.setView(this.panel.lo, this.panel.hi);

    const item = (key: Node, text: string) => {
      const sp = el('span', 'an-legend-item');
      sp.append(key, text);
      return sp;
    };
    const swatch = (alpha: number, dash: string) => {
      const k = el('span', 'an-swatch');
      k.style.background = `color-mix(in srgb, var(--series-1) ${Math.round(alpha * 100)}%, transparent)`;
      k.style.border = `1px ${dash} var(--series-1)`;
      k.setAttribute('aria-hidden', 'true');
      return k;
    };
    this.legend.append(
      item(lineKey(0), 'nominal: the plotted design'),
      item(lineKey(1), 'median of the runs'),
      item(swatch(0.24, 'solid'), '10–90 %'),
      item(swatch(0.14, 'dashed'), '5–95 %'),
      item(swatch(0.08, 'dotted'), 'min–max'),
    );
    this.renderReadout(this.panel.cursor, false);
    this.plotTable.refresh();
  }

  private renderReadout(i: number | null, fromKeyboard: boolean): void {
    const r = this.res;
    const cs = r ? curveStats(r.envelope, this.probeSel.value) : null;
    const nom = r?.nominal.get(this.probeSel.value);
    if (!r || !cs || !nom) return;
    if (i === null) {
      this.readout.textContent = 'Crosshair: point at the plot, or focus it and use the arrow keys, to read the envelopes.';
      return;
    }
    const f = r.freqs[i];
    const s = cs.stats;
    const v = (x: Num) => (x === null ? 'n/a' : cs.dB ? `${x.toFixed(2)} dB` : `${formatNumber(x, 4)} ${prettyUnit(nom.unit)}`);
    const text =
      `${formatHz(f)}: nominal ${v(nom.values[i])}, median ${v(s.median[i])}, 10–90 % ${v(s.p10[i])} to ${v(s.p90[i])}, ` +
      `5–95 % ${v(s.p5[i])} to ${v(s.p95[i])}, min–max ${v(s.min[i])} to ${v(s.max[i])} (${s.n[i]} runs); ${bandNote(r.shading, f)}.`;
    this.readout.textContent = text;
    if (fromKeyboard) this.spoken.textContent = text;
  }

  private plotTableEl(): Node {
    const r = this.res;
    const cs = r ? curveStats(r.envelope, this.probeSel.value) : null;
    const nom = r?.nominal.get(this.probeSel.value);
    if (!r || !cs || !nom) return el('p');
    const idx = this.panel.viewIndices();
    const rows: number[] = [];
    if (idx) {
      const stride = Math.max(1, Math.ceil((idx[1] - idx[0] + 1) / MAX_TABLE_ROWS));
      for (let i = idx[0]; i <= idx[1]; i += stride) rows.push(i);
      if (rows[rows.length - 1] !== idx[1]) rows.push(idx[1]);
    }
    const s = cs.stats;
    const fmt = (x: Num) => (x === null ? '' : cs.dB ? x.toFixed(2) : formatNumber(x, 5));
    const u = cs.dB ? 'dB SPL' : prettyUnit(nom.unit);
    const b = tableBlock(
      `Envelopes of ${this.probeSel.value} (${u})`,
      `From ${formatHz(this.panel.lo)} to ${formatHz(this.panel.hi)}, ${rows.length} grid points in view (zoom the plot to list others); n = runs with a value.`,
      ['Frequency (Hz)', 'Validity', 'Nominal', 'Median', '10 %', '90 %', '5 %', '95 %', 'Min', 'Max', 'n'],
      rows.map((i) => [
        formatParam(r.freqs[i], 6),
        bandName(band(r.shading, r.freqs[i])),
        fmt(nom.values[i]),
        fmt(s.median[i]),
        fmt(s.p10[i]),
        fmt(s.p90[i]),
        fmt(s.p5[i]),
        fmt(s.p95[i]),
        fmt(s.min[i]),
        fmt(s.max[i]),
        String(s.n[i]),
      ]),
    );
    b.table.dataset.table = 'envelope';
    return b.el;
  }

  private renderDistributions(): void {
    const b = this.distBody;
    b.replaceChildren();
    const r = this.res;
    if (!r) {
      b.append(el('p', 'hint', 'The parameters with a tolerance are varied (docs/parameters.md, "Tolerances").'));
      return;
    }
    const descs = new Map(this.params.map((p) => [p.name, p]));
    const rows = r.plan.distributions.map((d) => {
      const p = descs.get(d.name);
      const unit = paramUnit(p?.unit);
      const name = el('span');
      name.append(`${p?.label ?? d.name} (${d.name})`);
      if (p?.active === false) name.append(el('span', 'pinactive', 'not used by this design'));
      const bounds = [d.min, d.max].map((x) => (x === null ? '—' : formatParam(x, 5))).join(' to ');
      return [name, `${formatParam(d.nominal, 5)}${unit ? ` ${unit}` : ''}`, distText(d, unit), bounds, String(d.clipped), d.source ?? 'not stated'];
    });
    const t = tableBlock(
      'Varied parameters',
      `${r.plan.distributions.length} parameters varied around their values in the plotted design (docs/parameters.md: normal and log-normal tolerances are 2σ); clipped: samples moved to a bound.`,
      ['Parameter', 'Nominal', 'Distribution', 'Bounds', 'Clipped', 'Source'],
      rows,
    );
    for (const td of t.table.querySelectorAll('tbody td:nth-child(3), tbody td:nth-child(6)')) td.classList.add('an-left');
    b.append(t.el);
  }

  private renderMetrics(): void {
    const b = this.metricBody;
    b.replaceChildren();
    const r = this.res;
    const names = r ? Object.keys(r.envelope.metrics) : [];
    setOptions(this.metricSel, names.map((n) => [n, n]), this.metricSel.value || 'coupled_resonance_Hz');
    this.metricSel.disabled = !names.length;
    this.histTable.button.hidden = !names.length;
    if (!r) {
      b.append(el('p', 'hint', 'Each run’s scalar readouts (docs/analysis.md, "Readouts"), summarised over the runs that have them.'));
      this.renderHistogram();
      return;
    }
    const runs = r.envelope.runs;
    const f = (x: Num) => (x === null ? '—' : formatParam(x, 5));
    const t = tableBlock(
      'Readouts over the runs',
      `n: runs of the ${runs} that have the readout (a readout can be undefined for a run: no bass extension in the sweep, no Q for overlapping resonances, or an ambiguous coupled resonance).`,
      ['Readout', 'Median', '10–90 %', '5–95 %', 'Min–max', 'n'],
      names.map((k) => {
        const m = r.envelope.metrics[k];
        return [k, f(m.median), `${f(m.p10)} to ${f(m.p90)}`, `${f(m.p5)} to ${f(m.p95)}`, `${f(m.min)} to ${f(m.max)}`, `${m.n} of ${runs}`];
      }),
    );
    t.table.dataset.table = 'metrics';
    b.append(t.el);
    this.renderHistogram();
  }

  private metricValues(): number[] {
    const k = this.metricSel.value;
    return (this.res?.samples ?? []).map((s) => s.metrics?.[k]).filter((v): v is number => typeof v === 'number' && Number.isFinite(v));
  }

  private renderHistogram(): void {
    this.hist.replaceChildren();
    const r = this.res;
    const k = this.metricSel.value;
    const stats = r?.envelope.metrics[k];
    this.histTable.refresh();
    if (!r || !stats) return;
    const vals = this.metricValues();
    const bins = binValues(vals);
    const w = this.hist.clientWidth;
    if (!bins) {
      this.hist.append(el('p', 'hint', `No run has ${k}.`));
      return;
    }
    if (w === 0) return;
    const unit = /_Hz$/.test(k) ? 'Hz' : /_dB/.test(k) ? 'dB' : /_ohm$/.test(k) ? 'Ω' : '';
    this.hist.append(histogramSvg(bins, stats, unit, w, k));
    this.hist.append(el('p', 'hint', `${vals.length} of ${r.envelope.runs} runs have ${k}. Solid line: median; dashed: 5 and 95 % (the engine’s percentiles).`));
  }

  private histTableEl(): Node {
    const bins = binValues(this.metricValues());
    if (!bins) return el('p');
    return tableBlock(
      `Bin counts of ${this.metricSel.value}`,
      '',
      ['From', 'To', 'Runs'],
      bins.counts.map((c, i) => [formatParam(bins.lo + i * bins.width, 6), formatParam(bins.lo + (i + 1) * bins.width, 6), String(c)]),
    ).el;
  }

  private renderDoe(): void {
    const b = this.doeBody;
    b.replaceChildren();
    const r = this.res;
    if (!r) {
      b.append(el('p', 'hint', 'After a run: every run’s parameter values, readouts, error and reproducibility hash, as CSV.'));
      return;
    }
    const dl = button('Download CSV (runs, hashes, parameters, readouts)');
    const msg = el('p', 'an-status');
    msg.setAttribute('role', 'status');
    dl.addEventListener('click', async () => {
      try {
        const out = await callJson<{ csv: string }>(
          this.host,
          'mc_csv',
          JSON.stringify({ engine: r.engine, frequencies_Hz: r.freqs, samples: r.samples }),
          JSON.stringify(r.plan.parameters),
        );
        download(`acoustilab-doe-lhs-n${r.spec.n}-seed${r.spec.seed}.csv`, out.csv);
        msg.textContent = `Saved ${r.samples.length} rows.`;
      } catch (e) {
        msg.textContent = e instanceof Cancelled ? 'Cancelled.' : `The table could not be made: ${String(e)}`;
      }
    });
    const hashes = new Set(r.samples.map((s) => s.hash).filter(Boolean));
    b.append(
      el(
        'p',
        'hint',
        `Columns: run, hash (SHA-256 of the run’s expanded netlist; ${hashes.size} distinct), engine, the ${r.plan.parameters.length} sampled parameters, every readout, error.`,
      ),
      dl,
      msg,
    );
    const failed = r.samples.filter((s) => !s.ok);
    if (failed.length) {
      const ul = el('ul', 'an-notes');
      for (const s of failed) ul.append(el('li', undefined, `Run ${s.index}: ${s.error ?? 'failed'}`));
      b.append(el('p', 'an-error', `${failed.length} runs failed:`), ul);
    }
  }

  private testState(): unknown {
    const r = this.res;
    const cs = r ? curveStats(r.envelope, this.probeSel.value) : null;
    return {
      text: r?.text ?? null,
      spec: r?.spec ?? null,
      probe: this.probeSel.value,
      runs: r?.samples.length ?? 0,
      stale: !this.stale.el.hidden,
      freqs: r?.freqs ?? [],
      nominal: r?.nominal.get(this.probeSel.value)?.values ?? [],
      stats: cs?.stats ?? null,
      metrics: r?.envelope.metrics ?? null,
      cursor: this.panel.cursor,
    };
  }
}

export const view = new ToleranceView();
