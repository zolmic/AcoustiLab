// "Target": the plotted design's response against a target curve, with the
// engine's error metrics, BS.708 mask compliance and preference scores
// (docs/targets.md). Recomputed for every new result (one call, coalesced).
//
// The fixture rule is shown first: a target is a drum-reference-point curve
// on one fixture, and the response's fixture (inferred by the engine from
// the ear load) is compared with it. Scores keep their value when greyed;
// the engine's flags say why, and each is shown in words.

import { formatHz, formatHzTick, formatParam } from '../format';
import { lineKey } from '../keys';
import { PlotPanel } from '../plot';
import type { PlotGroup } from '../series';
import { band, bandName, bandNote } from '../shading';
import type { Num } from '../types';
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
  pressureProbes,
  primaryProbeOf,
  setOptions,
  signed,
} from './analysis-ui';
import type { Flag, MaskStat, Report, Score, Stat, TargetObject, TargetsList } from './analysis-types';
import type { ResultView, ViewHost } from './types';

/** Provenance classes (docs/targets.md, "Target objects"). */
const CLASSES: Record<string, string> = {
  peer_reviewed: 'peer-reviewed as a complete manuscript',
  research: 'published research reviewed on a summary only',
  manufacturer: 'from a manufacturer',
  community: 'community curve, not peer-reviewed',
  user: 'supplied by the user',
};

/** Target flags (docs/targets.md, "Flags used"). */
const TARGET_FLAGS: Record<string, string> = {
  approximation: 'parametric approximation, not published data',
  adaptation: 'the depositors’ adaptation of a third-party curve',
  third_octave: 'third-octave resolution',
  selection: 'chosen from a set by this project’s rule',
  data_anomaly: 'probable error in the source data, kept as published',
  user_supplied: 'supplied by the user',
  shelf_q_assumed: 'the shelf Q is assumed, not published',
};

/** Score flags in a few words (docs/targets.md, "Greying rules"); the engine's message follows. */
const SCORE_FLAGS: Record<string, string> = {
  training_fixture_mismatch: 'fixture differs from the model’s training fixture',
  training_target_mismatch: 'target differs from the model’s training target',
  partial_range: 'the data do not cover the model’s band',
  simulated: 'simulated curve, not measured',
  coupler_extrapolated: 'part of the band is coupler-extrapolated',
  outside_scale: 'outside the 0–100 rating scale',
};
const GREYING = new Set(['training_fixture_mismatch', 'training_target_mismatch', 'partial_range']);

const MATCH: Record<string, string> = {
  same: 'Same fixture',
  same_ear_simulator: 'Same ear simulator, different pinna, head or canal extension',
  different: 'Fixture mismatch',
};

interface Imported {
  name: string;
  object: TargetObject;
}

const db = (x: Num | undefined, d = 2) => (x === null || x === undefined ? '—' : x.toFixed(d));
/** "20–10k Hz", compactly. */
const hzBand = (x: [number, number] | null) => (x ? `${formatHzTick(x[0])}–${formatHzTick(x[1])} Hz` : '—');

class TargetView implements ResultView {
  readonly id = 'target';
  readonly label = 'Target';
  readonly order = 40;
  private host!: ViewHost;
  private list: TargetsList | null = null;
  private listError: string | null = null;
  private imported: Imported[] = [];
  private report: Report | null = null;
  private reportText: string | null = null;
  private reportError: string | null = null;
  private targetInfo = new Map<string, TargetObject>();
  /** Request counter: only the newest reply is shown. */
  private seq = 0;
  private busy = false;
  private pending = false;

  private targetSel = el('select');
  private probeSel = el('select');
  private smoothSel = el('select');
  private status = el('p', 'an-status');
  private fixtureBox = el('div', 'an-fixture');
  private info = el('div');
  private legend = el('div', 'an-legend');
  private readout = el('p', 'an-readout');
  private spoken = el('p', 'visually-hidden');
  private plots = el('div', 'an-plots');
  private panel!: PlotPanel;
  private plotTable!: ReturnType<typeof disclosure>;
  private metricsBody = el('div');
  private scoresBody = el('div');
  private flagsBody = el('div');
  private importBox = el('details', 'an-import');

  mount(root: HTMLElement, host: ViewHost): void {
    this.host = host;
    root.classList.add('an-view');
    const intro = el(
      'p',
      'hint',
      'The design on the Response tab against a target curve: both normalised at 500 Hz, the error, its statistics per band, ' +
        'BS.708 mask compliance and the preference models’ scores, as the engine computes them (docs/targets.md).',
    );
    for (const s of [this.targetSel, this.probeSel, this.smoothSel]) s.addEventListener('change', () => this.request());
    this.status.setAttribute('role', 'status');
    const controls = el('div', 'an-controls');
    controls.append(field('Target', this.targetSel), field('Probe', this.probeSel), field('Smoothing', this.smoothSel));
    this.buildImport();

    this.panel = new PlotPanel(this.plots, {
      onCursor: (i, fromKeyboard) => this.renderReadout(i, fromKeyboard),
      onView: () => this.plotTable?.refresh(),
    });
    this.spoken.setAttribute('aria-live', 'polite');
    this.plotTable = disclosure('data table', () => this.plotTableEl());

    const sec = (title: string, body: Node[]) => {
      const s = el('section', 'an-section');
      const h = el('h3', undefined, title);
      h.id = nextId('an-h');
      s.setAttribute('aria-labelledby', h.id);
      s.append(h, ...body);
      return s;
    };
    root.append(
      intro,
      controls,
      this.status,
      this.fixtureBox,
      this.importBox,
      sec('Target', [this.info]),
      sec('Response against the target', [this.legend, this.readout, this.spoken, this.plots, this.plotTable.button, this.plotTable.region]),
      sec('Error metrics', [this.metricsBody]),
      sec('Preference scores', [this.scoresBody]),
      sec('Report flags', [this.flagsBody]),
    );
    this.fixtureBox.hidden = true;
    exposeForTests('target', () => ({
      report: this.report,
      text: this.reportText,
      target: this.targetSel.value,
      probe: this.probeSel.value,
      smoothing: this.smoothSel.value,
      error: this.reportError,
    }));
    void this.loadList();
  }

  private async loadList(): Promise<void> {
    try {
      this.list = await callJson<TargetsList>(this.host, 'targets_list');
    } catch (e) {
      this.listError = e instanceof EngineFailure ? e.message : String(e);
    }
    this.fillTargets();
    const fr = (this.list?.smoothing_fractions ?? []).filter((n) => n >= 3).sort((a, b) => b - a);
    setOptions(this.smoothSel, [['none', 'none'], ...fr.map((n): [string, string] => [`1/${n}`, `1/${n} octave`])], 'none');
    this.fillImportFixtures();
    this.refresh();
  }

  private fillTargets(): void {
    const keep = this.targetSel.value;
    this.targetSel.replaceChildren();
    const groups = new Map<string, [string, string][]>();
    const targets = [...(this.list?.targets ?? [])].sort((a, b) => Number(b.primary) - Number(a.primary));
    for (const t of targets) {
      const g = groups.get(t.group) ?? [];
      g.push([t.name, t.label]);
      groups.set(t.group, g);
    }
    if (this.imported.length) groups.set('Imported (this page only)', this.imported.map((t) => [`import:${t.name}`, `${t.object.label || t.name} (${t.object.fixture})`]));
    for (const [g, opts] of groups) {
      const og = el('optgroup');
      og.label = g;
      for (const [v, t] of opts) og.append(Object.assign(el('option', undefined, t), { value: v }));
      this.targetSel.append(og);
    }
    if (keep && [...this.targetSel.options].some((o) => o.value === keep)) this.targetSel.value = keep;
  }

  refresh(): void {
    const cur = this.host.current();
    const probes = pressureProbes(cur?.result);
    setOptions(this.probeSel, probes.map((p) => [p, p]), this.probeSel.value || primaryProbeOf(cur?.text) || probes[0] || null);
    if (this.list && cur && cur.text !== this.reportText) this.request();
    else this.render();
  }

  /** The target spec for the engine: a bundled name, or an imported target object. */
  private spec(): string {
    const v = this.targetSel.value;
    if (v.startsWith('import:')) return JSON.stringify(this.imported.find((t) => `import:${t.name}` === v)?.object ?? null);
    return v;
  }

  /** Computes the report for the current result and choices (coalesced: one call at a time, the newest last). */
  private request(): void {
    if (this.busy) {
      this.pending = true;
      return;
    }
    void this.compute();
  }

  private async compute(): Promise<void> {
    const cur = this.host.current();
    const probe = this.probeSel.value;
    if (!cur || !probe || !this.targetSel.value) {
      this.render();
      return;
    }
    const seq = ++this.seq;
    this.busy = true;
    this.status.textContent = 'Computing…';
    const text = cur.text;
    try {
      const fx = await callJson<{ fixture: string | null }>(this.host, 'probe_fixture', text, '', probe);
      const opts: Record<string, unknown> = { probe, smoothing: this.smoothSel.value || 'none' };
      if (fx.fixture) opts.fixture = fx.fixture;
      const spec = this.spec();
      const report = await callJson<Report>(this.host, 'target_metrics', JSON.stringify(cur.result), spec, JSON.stringify(opts));
      if (!this.targetInfo.has(spec)) this.targetInfo.set(spec, await callJson<TargetObject>(this.host, 'target', spec));
      if (seq === this.seq) {
        this.report = report;
        this.reportText = text;
        this.reportError = null;
      }
    } catch (e) {
      if (seq === this.seq) {
        this.report = null;
        this.reportText = text;
        this.reportError = e instanceof Cancelled ? 'cancelled' : e instanceof EngineFailure ? e.message : String(e);
      }
    }
    this.busy = false;
    if (this.pending) {
      this.pending = false;
      void this.compute();
      return;
    }
    this.render();
  }

  // ----- rendering ------------------------------------------------------------

  private render(): void {
    const r = this.report;
    const cur = this.host.current();
    if (this.listError) this.status.textContent = `The target list could not be read: ${this.listError}`;
    else if (!cur) this.status.textContent = 'Run a netlist first: the view compares the design on the Response tab.';
    else if (!this.probeSel.value) this.status.textContent = 'This design has no pressure probe to compare.';
    else if (this.reportError) this.status.textContent = `The comparison failed: ${this.reportError}`;
    else if (r) this.status.textContent = `${r.response.label} at the stated drive (${r.response.drive ?? 'as written'}); 0 dB = its level at 500 Hz, ${formatParam(r.response.reference_level_dB ?? NaN, 5)} dB SPL.`;
    this.renderFixture();
    this.renderInfo();
    this.renderPlot();
    this.renderMetrics();
    this.renderScores();
    this.renderFlags();
  }

  private renderFixture(): void {
    const r = this.report;
    const b = this.fixtureBox;
    b.replaceChildren();
    b.hidden = !r;
    if (!r) return;
    const label = (id: string | null) => (id ? (this.list?.fixtures.find((f) => f.id === id)?.label ?? id) : 'unknown');
    const ok = r.fixture_match === 'same';
    b.classList.toggle('is-mismatch', !ok);
    const icon = el('span', 'an-fixture-icon', ok ? '✓' : '⚠');
    icon.setAttribute('aria-hidden', 'true');
    const body = el('div');
    const head = el('p');
    head.append(el('strong', undefined, r.fixture_match ? MATCH[r.fixture_match] : 'Response fixture unknown'));
    body.append(head);
    const dl = el('dl', 'an-dl');
    dl.append(
      el('dt', undefined, 'Response'),
      el('dd', undefined, `${r.response.fixture_label ?? label(r.response.fixture)}${r.response.fixture ? ` (${r.response.fixture}, inferred from the ear load)` : ''}`),
      el('dt', undefined, 'Target'),
      el('dd', undefined, `${label(r.target.fixture)} (${r.target.fixture})`),
    );
    body.append(dl);
    for (const f of r.flags.filter((x) => x.code === 'fixture_mismatch' || x.code === 'fixture_unspecified')) body.append(el('p', undefined, `Engine: ${f.message}.`));
    body.append(
      el(
        'p',
        'hint',
        'Fixture rule (docs/targets.md): every published target is a drum-reference-point curve on one fixture; a response on another ' +
          'fixture differs from it by several dB above 2 kHz, so the comparison is flagged, not refused. The engine models ear simulators without pinna or head, so a head-and-torso target is always flagged.',
      ),
    );
    b.append(icon, body);
  }

  private renderInfo(): void {
    const b = this.info;
    b.replaceChildren();
    const r = this.report;
    const t = this.targetInfo.get(this.spec());
    if (!r || !t) {
      if (!this.list && !this.listError) b.append(el('p', 'hint', 'Loading the target list…'));
      return;
    }
    const dl = el('dl', 'an-dl');
    const add = (k: string, v: string | Node) => {
      const dd = el('dd');
      dd.append(v);
      dl.append(el('dt', undefined, k), dd);
    };
    add('Curve', t.label || t.name);
    add('Provenance', `${t.provenance.class}${CLASSES[t.provenance.class] ? `: ${CLASSES[t.provenance.class]}` : ''}`);
    add('Licence', t.provenance.licence ?? 'not stated');
    if (t.provenance.source) add('Source', t.provenance.source);
    if (t.provenance.doi) add('DOI', t.provenance.doi);
    if (t.provenance.url) {
      const a = el('a', undefined, t.provenance.url);
      a.href = t.provenance.url;
      a.rel = 'noopener noreferrer';
      a.target = '_blank';
      add('URL', a);
    }
    if (t.provenance.attribution) add('Attribution', t.provenance.attribution);
    add('Valid range', t.valid_range_Hz ? `${formatHz(t.valid_range_Hz[0])} to ${formatHz(t.valid_range_Hz[1])}` : 'not stated');
    add('Flags', t.flags.length ? t.flags.map((f) => `${f}${TARGET_FLAGS[f] ? ` (${TARGET_FLAGS[f]})` : ''}`).join('; ') : 'none');
    b.append(dl);
  }

  private renderPlot(): void {
    const r = this.report;
    const cur = this.host.current();
    this.legend.replaceChildren();
    this.plotTable.button.hidden = !r;
    if (!r || !cur) {
      this.plots.replaceChildren();
      this.readout.hidden = true;
      this.plotTable.refresh();
      return;
    }
    this.readout.hidden = false;
    const level: PlotGroup = {
      key: 'target',
      kind: 'delta',
      title: 'Level',
      symbol: 're 500 Hz',
      unit: '',
      axisUnit: 'dB',
      scale: 'linear',
      series: [
        { probe: 0, id: r.response.label, values: r.response_dB, primary: true },
        { probe: 1, id: 'target', values: r.target_dB },
      ],
      overlays: [],
      bands: [{ probe: 1, id: 'band', label: 'preference band of the target', lower: r.preference_band.lower_dB, upper: r.preference_band.upper_dB, alpha: 0.14, edge: [2, 2] }],
      height: 'main',
    };
    const error: PlotGroup = {
      key: 'error',
      kind: 'delta',
      title: 'Error',
      symbol: 'response − target',
      unit: '',
      axisUnit: 'dB',
      scale: 'linear',
      series: [{ probe: 2, id: 'error', values: r.error_dB }],
      overlays: [],
      height: 'small',
    };
    this.panel.setGroups([level, error], { freqs: r.grid_Hz, shading: cur.result.shading, highlight: null });
    this.panel.setView(this.panel.lo, this.panel.hi);
    const item = (key: Node, text: string) => {
      const sp = el('span', 'an-legend-item');
      sp.append(key, text);
      return sp;
    };
    const sw = el('span', 'an-swatch');
    sw.style.background = 'color-mix(in srgb, var(--series-2) 14%, transparent)';
    sw.style.border = '1px dashed var(--series-2)';
    sw.setAttribute('aria-hidden', 'true');
    this.legend.append(
      item(lineKey(0), `${r.response.label} (response)`),
      item(lineKey(1), 'target'),
      item(sw, 'preference band (listener classes, widened above 2 and 8 kHz)'),
      item(lineKey(2), 'error (lower plot)'),
    );
    this.renderReadout(this.panel.cursor, false);
    this.plotTable.refresh();
  }

  private renderReadout(i: number | null, fromKeyboard: boolean): void {
    const r = this.report;
    const cur = this.host.current();
    if (!r || !cur) return;
    if (i === null) {
      this.readout.textContent = 'Crosshair: point at a plot, or focus one and use the arrow keys, to read the curves.';
      return;
    }
    const f = r.grid_Hz[i];
    const v = (x: Num) => (x === null ? 'n/a' : `${signed(x, 2)} dB`);
    const text =
      `${formatHz(f)}: ${r.response.label} ${v(r.response_dB[i])}, target ${v(r.target_dB[i])}, ` +
      `band ${v(r.preference_band.lower_dB[i])} to ${v(r.preference_band.upper_dB[i])}, error ${v(r.error_dB[i])}; ${bandNote(cur.result.shading, f)}.`;
    this.readout.textContent = text;
    if (fromKeyboard) this.spoken.textContent = text;
  }

  private plotTableEl(): Node {
    const r = this.report;
    const cur = this.host.current();
    if (!r || !cur) return el('p');
    const b = tableBlock(
      `${r.response.label} against the target, dB re 500 Hz`,
      `On the evaluation grid (${r.options.grid}); smoothing ${r.options.smoothing}; "band" is the preference band.`,
      ['Frequency (Hz)', 'Validity', 'Response', 'Target', 'Band low', 'Band high', 'Error'],
      r.grid_Hz.map((f, i) => [
        formatParam(f, 6),
        bandName(band(cur.result.shading, f)),
        db(r.response_dB[i]),
        db(r.target_dB[i]),
        db(r.preference_band.lower_dB[i]),
        db(r.preference_band.upper_dB[i]),
        db(r.error_dB[i]),
      ]),
    );
    b.table.dataset.table = 'curves';
    return b.el;
  }

  private renderMetrics(): void {
    const b = this.metricsBody;
    b.replaceChildren();
    const r = this.report;
    if (!r) return;
    const range = hzBand;
    const row = (name: string, s: Stat) => [
      name,
      range(s.band_Hz),
      `${range(s.used_Hz)}${s.partial ? ' (partial)' : ''}`,
      String(s.n),
      db(s.rms_dB),
      db(s.sd_dB),
      db(s.slope_dB_per_octave),
      db(s.mean_dB),
      db(s.mae_dB),
      s.max_abs_dB === null || s.max_abs_dB === undefined ? '—' : `${s.max_abs_dB.toFixed(2)} at ${formatHz(s.max_abs_at_Hz ?? NaN)}`,
    ];
    const m = r.metrics;
    const t = tableBlock(
      'Error statistics per band (dB)',
      'RMS, standard deviation (sample), slope of the error against log frequency (dB per octave), mean, mean absolute and largest error. "Used" is the range the grid points with a value cover; "partial" when it stops short of the band.',
      ['Statistic', 'Band', 'Used', 'n', 'RMS', 'SD', 'Slope (dB/oct)', 'Mean', 'Mean |e|', 'Max |e|'],
      [row('main', m.main), row('above 10 kHz', m.above_10kHz), ...m.band_rms.map((s) => row('per band', s))],
    );
    t.table.dataset.table = 'metrics';
    b.append(t.el);
    const mask = (name: string, s: MaskStat) => [
      name,
      range(s.band_Hz),
      `${s.within} of ${s.n}${s.partial ? ' (partial)' : ''}`,
      s.compliance_percent === null ? '—' : `${s.compliance_percent.toFixed(1)} %`,
      s.worst ? `${s.worst.excess_dB.toFixed(2)} dB beyond, at ${formatHz(s.worst.f_Hz)}` : 'none',
    ];
    const t2 = tableBlock(
      'Mask compliance',
      'The share of grid points where the error lies inside the mask. BS.708: ±2 dB at 100 Hz to ±4 dB at 16 kHz (Rec. ITU-R BS.708 Figure 1, as traced in data/targets/bs708.json), applied here to the error against the selected target; preference band: the listener-class band around the target.',
      ['Mask', 'Band', 'Points within', 'Compliance', 'Worst excursion'],
      [mask('ITU-R BS.708', m.bs708_mask), mask('preference band', m.preference_band)],
    );
    t2.table.dataset.table = 'masks';
    b.append(t2.el);
  }

  private renderScores(): void {
    const b = this.scoresBody;
    b.replaceChildren();
    const r = this.report;
    if (!r) return;
    b.append(
      el(
        'p',
        'hint',
        'A greyed score keeps its value but is not a valid prediction here: its model was fitted on another fixture or target, or the data do not cover its band. Notices do not grey a score.',
      ),
    );
    const ul = el('ul', 'an-scores');
    for (const s of r.scores) ul.append(this.scoreItem(s));
    b.append(ul);
  }

  /** One model: its score and state in words, the flags behind it, and its formula, variables, fit and source. */
  private scoreItem(s: Score): HTMLElement {
    const li = el('li', `an-score${s.greyed ? ' an-score-greyed' : ''}`);
    li.dataset.model = s.model;
    li.dataset.score = s.score === null ? '' : String(s.score);
    li.dataset.greyed = String(s.greyed);
    li.append(el('p', 'an-score-model', s.label));
    const head = el('p', 'an-score-head');
    head.append('Score ', el('span', 'an-score-value', s.score === null ? '—' : s.score.toFixed(1)), ' · ', el('strong', undefined, s.greyed ? 'greyed' : 'applies'));
    li.append(head);
    const list = (flags: Flag[], title: string) => {
      if (!flags.length) return;
      li.append(el('p', 'an-score-sub', title));
      const ul = el('ul', 'an-flag-list');
      for (const f of flags) {
        const item = el('li');
        item.append(el('strong', undefined, SCORE_FLAGS[f.code] ?? f.code), `: ${f.message}`);
        ul.append(item);
      }
      li.append(ul);
    };
    list(s.flags.filter((f) => GREYING.has(f.code)), 'Greyed because:');
    list(s.flags.filter((f) => !GREYING.has(f.code)), 'Notices:');
    const more = el('details');
    more.append(el('summary', undefined, 'Formula, variables, fit and source'));
    more.append(el('p', undefined, s.formula));
    const vars = el('ul', 'an-flag-list');
    for (const v of s.variables) {
      vars.append(el('li', undefined, `${v.variable} = ${v.value === null ? '—' : v.value.toFixed(3)} (weight ${v.weight}; ${hzBand(v.band_Hz)}, n ${v.n})`));
    }
    more.append(vars);
    if (s.fit) more.append(el('p', undefined, `Fit: r = ${s.fit.r}, RMSE ${s.fit.rmse} points${s.fit.observations ? `, ${s.fit.observations} headphones` : ''}.`));
    more.append(el('p', 'hint', s.source));
    li.append(more);
    return li;
  }

  private renderFlags(): void {
    const b = this.flagsBody;
    b.replaceChildren();
    const r = this.report;
    if (!r) return;
    const ul = el('ul', 'an-notes');
    for (const f of r.flags) ul.append(el('li', undefined, `${f.code}: ${f.message}`));
    b.append(r.flags.length ? ul : el('p', 'hint', 'No flags.'));
  }

  // ----- import ------------------------------------------------------------------

  private importFixture = el('select');

  private buildImport(): void {
    const d = this.importBox;
    d.append(el('summary', undefined, 'Import a target curve (CSV)'));
    const help = el(
      'p',
      'hint',
      'Rows "frequency_Hz,dB"; tags as "# key: value" lines. The fixture is required, as a "# fixture:" tag or chosen here (if both, they must agree). ' +
        'An imported target lives in this page only, with provenance class "user".',
    );
    const file = el('input');
    file.type = 'file';
    file.accept = '.csv,.txt,text/csv,text/plain';
    const area = el('textarea');
    area.spellcheck = false;
    area.placeholder = '# name: My target\n# fixture: bk5128\n# licence: CC-BY-4.0\nfrequency_Hz,dB\n20,0.5\n…';
    const name = el('input');
    name.type = 'text';
    name.className = 'an-wide';
    const go = button('Import');
    const msg = el('p', 'an-status');
    msg.setAttribute('role', 'status');
    file.addEventListener('change', async () => {
      const f = file.files?.[0];
      if (f) area.value = await f.text();
    });
    go.addEventListener('click', async () => {
      if (!area.value.trim()) {
        msg.textContent = 'Paste a CSV or choose a file first.';
        return;
      }
      try {
        const t = await callJson<TargetObject>(this.host, 'import_target_csv', area.value, this.importFixture.value, name.value.trim());
        const key = t.name || `target ${this.imported.length + 1}`;
        this.imported = this.imported.filter((x) => x.name !== key);
        this.imported.push({ name: key, object: t });
        this.fillTargets();
        this.targetSel.value = `import:${key}`;
        msg.textContent = `Imported “${t.label || key}” on fixture ${t.fixture}; it is now the chosen target.`;
        this.request();
      } catch (e) {
        msg.textContent = `Not imported: ${e instanceof EngineFailure ? e.message : String(e)}`;
      }
    });
    const text = field('CSV text', area, 'an-field an-block');
    const row = el('div', 'an-controls');
    row.append(field('File', file), field('Fixture', this.importFixture), field('Name (optional)', name), go);
    d.append(help, row, text, msg);
  }

  private fillImportFixtures(): void {
    setOptions(
      this.importFixture,
      [['', 'from the file’s “# fixture:” tag'], ...(this.list?.fixtures ?? []).map((f): [string, string] => [f.id, `${f.id}: ${f.label}`])],
      '',
    );
  }
}

export const view = new TargetView();
