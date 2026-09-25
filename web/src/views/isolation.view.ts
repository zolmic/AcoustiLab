// "Isolation": passive insertion loss at the drum (engine export
// `isolation`, docs/isolation.md): the loss against frequency, coherent and
// with the paths added in power, each ambient path's contribution, the
// open and occluded drum pressures, the 1/3-octave bands with the ETSI TS
// 103 640 summary, the fixture's self-insertion loss as a limit, the
// transformation's warnings, and on request the bleed at 0.3 m and 1 m with
// its ±6 dB band. The engine's convention statement is shown as it is.

import { formatHz, formatNumber } from '../format';
import {
  Coalesced,
  FreqFigure,
  JobStatus,
  checkbox,
  defList,
  el,
  errorBox,
  field,
  formatDb,
  isEngineError,
  registerHooks,
  scrollRegion,
  select,
  table,
  type FigPlot,
  type FigSeries,
} from './measure-kit';
import type { ResultView, ViewHost } from './types';
import type { Shading } from '../types';

interface Complex {
  re: number[];
  im: number[];
  spl_dB: (number | null)[];
}

interface FixtureBand {
  f_min_Hz: number;
  f_max_Hz: number;
  exceeds_dB: number;
}

interface IsolationReport {
  frequencies_Hz: number[];
  ambient_Pa: number;
  ear: { drum_probe: string; drum_node: string; entrance_node: string; elements: string[] };
  insertion_loss_dB: (number | null)[];
  insertion_loss_incoherent_dB: (number | null)[] | null;
  p_occluded: Complex;
  p_open: Complex;
  third_octave_bands: { nominal_Hz: number; center_Hz: number; insertion_loss_dB: number | null }[];
  summary: { max_dB: number; max_at_Hz: number; range_6dB_Hz: [number, number] | null; mean_dB: number } | null;
  driven: string[];
  paths: { element: string; re: number[]; im: number[]; level_re_open_dB: (number | null)[] }[];
  warnings: { code: string; element: string | null; message: string }[];
  shading: Shading;
  fixture_self_insertion_loss: { fixture: string; bands: FixtureBand[]; source: string; url: string; status: string; exceeded_at_Hz: number[] };
  convention: string;
  bleed:
    | {
        frequencies_Hz: number[];
        distances_m: number[];
        spl_dB: (number | null)[][];
        band_dB: number;
        drive: { label: string };
        model: string;
      }
    | { error: string; kind: string }
    | null;
}

/** Consecutive points of `freqs` (a grid) that are in `set`, as ranges. */
function ranges(freqs: number[], set: number[]): [number, number][] {
  const inSet = new Set(set);
  const out: [number, number][] = [];
  let start: number | null = null;
  let last = 0;
  for (const f of freqs) {
    if (inSet.has(f)) {
      if (start === null) start = f;
      last = f;
    } else if (start !== null) {
      out.push([start, last]);
      start = null;
    }
  }
  if (start !== null) out.push([start, last]);
  return out;
}

const rangeText = ([a, b]: [number, number]) => (a === b ? formatHz(a) : `${formatHz(a)}–${formatHz(b)}`);

class IsolationView implements ResultView {
  readonly id = 'isolation';
  readonly label = 'Isolation';
  readonly order = 55;
  private host!: ViewHost;
  private probeSel!: HTMLSelectElement;
  private entrance!: HTMLInputElement;
  private pathsBox!: HTMLInputElement;
  private bleedBox!: HTMLInputElement;
  private busy!: JobStatus;
  private status!: HTMLElement;
  private stale!: HTMLElement;
  private errorEl!: HTMLElement;
  private body!: HTMLElement;
  private conventions!: HTMLElement;
  private summary!: HTMLElement;
  private warnings!: HTMLElement;
  private fig!: FreqFigure;
  private exceed!: HTMLElement;
  private bands!: HTMLElement;
  private bleedSection!: HTMLElement;
  private bleedNote!: HTMLElement;
  private bleedFig!: FreqFigure;
  private call!: Coalesced;
  private report: IsolationReport | null = null;

  mount(root: HTMLElement, host: ViewHost): void {
    this.host = host;
    root.classList.add('mv');
    this.probeSel = select([['', 'Default']]);
    this.entrance = el('input', { attrs: { type: 'text', placeholder: 'inferred', autocomplete: 'off', spellcheck: 'false' } });
    const paths = checkbox('Per-path contributions', true);
    const bleed = checkbox('Bleed at 0.3 m and 1 m (±6 dB)');
    this.pathsBox = paths.input;
    this.bleedBox = bleed.input;
    for (const c of [this.probeSel, this.pathsBox, this.bleedBox]) c.addEventListener('change', () => this.update());
    this.entrance.addEventListener('change', () => this.update());
    const controls = el(
      'div',
      { class: 'mv-bar', attrs: { role: 'group', 'aria-label': 'Isolation options' } },
      field('Drum-point probe', this.probeSel),
      field('Ear entrance node', this.entrance),
      paths.wrap,
      bleed.wrap,
    );
    this.busy = new JobStatus('Computing the insertion loss', () => {
      this.host.cancel();
      this.call.abandon();
      this.busy.finish('Cancelled. Change an option, or solve again, to compute.', 'cancelled');
    });
    this.status = el('p', { class: 'hint mv-status', attrs: { role: 'status' } });
    this.stale = el('p', { class: 'hint mv-stale', hidden: true, text: 'The netlist has been edited since it was last solved; these results are for the solved text.' });
    this.errorEl = el('div');
    this.conventions = el('div', { class: 'mv-conventions', attrs: { role: 'note', 'aria-label': 'Conventions of the insertion loss' } });
    this.summary = el('div', { class: 'mv-cards' });
    this.warnings = el('div');
    const help =
      'Plots: move the pointer or focus a plot and use ←/→ (with Shift: 10 points) to read values; +/− zoom, Ctrl+←/→ pan, 0 default view (20 Hz–20 kHz), Esc hides the crosshair. Drag across a plot to zoom; double-click resets.';
    this.fig = new FreqFigure({ id: 'isolation', label: 'Insertion loss', host, help });
    this.exceed = el('p', { class: 'hint' });
    this.bands = el('div');
    this.bleedNote = el('p', { class: 'hint' });
    this.bleedFig = new FreqFigure({ id: 'bleed', label: 'Bleed', host, help });
    this.bleedSection = el('section', { hidden: true }, el('h3', { class: 'mv-h', text: 'Bleed' }), this.bleedNote, this.bleedFig.root);
    this.body = el(
      'div',
      { hidden: true },
      this.conventions,
      this.warnings,
      this.summary,
      el('h3', { class: 'mv-h', text: 'Insertion loss, paths and drum pressures' }),
      this.fig.root,
      this.exceed,
      el('h3', { class: 'mv-h', text: '1/3-octave bands' }),
      this.bands,
      this.bleedSection,
    );
    root.append(
      el('p', {
        class: 'hint',
        text:
          'Passive insertion loss at the drum point (docs/isolation.md): the outside pressure drives every path to the ambient terminals, the sources are zeroed, ' +
          'and the drum pressure is compared with the open ear’s. Computed on this view’s own worker for the netlist as last solved.',
      }),
      controls,
      this.busy.root,
      this.status,
      this.stale,
      this.errorEl,
      this.body,
    );
    this.call = new Coalesced(
      host,
      'isolation',
      (_k, v, ms) => this.onReport(v, ms),
      () => {
        if (this.call.busy && !this.busy.running) this.busy.start('Computing the insertion loss…');
        if (!this.call.busy && this.busy.running) this.busy.hide();
      },
    );
    registerHooks('isolation', {
      report: () => this.report,
      figure: () => this.fig.hook(),
      row: (key: string, y: number) => this.fig.row(key, y),
      column: (key: string, x: number) => this.fig.column(key, x),
      xOf: (key: string, f: number) => this.fig.xOf(key, f),
      setCursor: (i: number | null) => this.fig.panel.setCursor(i),
      bleedFigure: () => this.bleedFig.hook(),
    });
  }

  refresh(): void {
    const cur = this.host.current();
    if (!cur) {
      this.status.textContent = 'Run a netlist to compute its insertion loss.';
      return;
    }
    const pressure = cur.result.probes.filter((p) => p.quantity === 'pressure');
    const ids = pressure.map((p) => p.id).join(',');
    if (this.probeSel.dataset.ids !== ids) {
      const keep = this.probeSel.value;
      this.probeSel.replaceChildren(el('option', { text: 'Default (ui.primary_probe)', attrs: { value: '' } }));
      for (const p of pressure) this.probeSel.append(el('option', { text: p.id, attrs: { value: p.id } }));
      this.probeSel.value = pressure.some((p) => p.id === keep) ? keep : '';
      this.probeSel.dataset.ids = ids;
    }
    this.stale.hidden = this.host.netlist() === cur.text;
    this.update();
  }

  private update(): void {
    const cur = this.host.current();
    if (!cur) return;
    const o: Record<string, unknown> = { paths: this.pathsBox.checked, bleed: this.bleedBox.checked };
    if (this.probeSel.value) o.drum_probe = this.probeSel.value;
    const entrance = this.entrance.value.trim();
    if (entrance) o.entrance_node = entrance;
    this.call.request(JSON.stringify([cur.text, o]), [cur.text, '', JSON.stringify(o)]);
  }

  private onReport(v: unknown, ms: number): void {
    this.errorEl.replaceChildren();
    if (isEngineError(v)) {
      this.report = null;
      this.body.hidden = true;
      this.status.textContent = 'The engine could not compute the insertion loss.';
      this.errorEl.append(errorBox('Insertion loss not computed', v.error, v.kind ? `Kind: ${v.kind}.` : undefined));
      return;
    }
    const r = v as IsolationReport;
    this.report = r;
    this.body.hidden = false;
    this.status.textContent = `Insertion loss computed in ${ms.toFixed(0)} ms.`;
    this.renderConventions(r);
    this.renderSummary(r);
    this.renderWarnings(r);
    this.renderPlots(r);
    this.renderBands(r);
    this.renderBleed(r);
  }

  private renderConventions(r: IsolationReport): void {
    this.conventions.replaceChildren(
      el('p', {}, el('strong', { text: 'Conventions (engine): ' }), r.convention),
      el('p', { class: 'hint', text: `Outside pressure ${formatNumber(r.ambient_Pa, 4)} Pa at every ambient terminal, in phase; the drum SPLs below are for that pressure (94 dB SPL).` }),
    );
  }

  private renderSummary(r: IsolationReport): void {
    const s = r.summary;
    const ear = r.ear;
    const summary = el(
      'section',
      { class: 'mv-card', attrs: { 'aria-labelledby': 'iso-summary-h' } },
      el('h4', { id: 'iso-summary-h', text: 'Summary over the 1/3-octave bands (ETSI TS 103 640, 5.1.1)' }),
      s
        ? defList([
            ['Largest loss', el('span', { text: `${formatDb(s.max_dB)} in the ${formatHz(s.max_at_Hz)} band`, attrs: { 'data-field': 'max' } })],
            ['Loss ≥ 6 dB', s.range_6dB_Hz ? `${formatHz(s.range_6dB_Hz[0])} to ${formatHz(s.range_6dB_Hz[1])} bands` : 'in no band'],
            ['Mean over the bands', formatDb(s.mean_dB)],
            ['Bands inside the sweep', String(r.third_octave_bands.length)],
          ])
        : el('p', { class: 'hint', text: 'No 1/3-octave band lies inside the sweep.' }),
    );
    const earCard = el(
      'section',
      { class: 'mv-card', attrs: { 'aria-labelledby': 'iso-ear-h' } },
      el('h4', { id: 'iso-ear-h', text: 'Ear and paths' }),
      defList([
        ['Drum-point probe', `${ear.drum_probe} at ${ear.drum_node}`],
        ['Ear entrance', ear.entrance_node],
        ['Ear load (open-ear reference)', ear.elements.join(', ')],
        ['Driven paths (ambient terminals)', r.driven.length ? r.driven.join(', ') : 'none'],
      ]),
    );
    this.summary.replaceChildren(summary, earCard);
  }

  private renderWarnings(r: IsolationReport): void {
    this.warnings.replaceChildren();
    if (!r.warnings.length) return;
    this.warnings.append(
      el(
        'section',
        { class: 'mv-card mv-warn', attrs: { 'aria-labelledby': 'iso-warn-h' } },
        el('h4', { id: 'iso-warn-h', text: `Warnings (${r.warnings.length})` }),
        el(
          'ul',
          {},
          ...r.warnings.map((w) => el('li', { attrs: { 'data-code': w.code } }, el('strong', { text: w.code.replace(/_/g, ' ') }), w.element ? ` (${w.element})` : '', `: ${w.message}`)),
        ),
      ),
    );
  }

  /** Fixture bound at each frequency (null outside its bands). */
  private fixtureLimit(r: IsolationReport): (number | null)[] {
    const bands = r.fixture_self_insertion_loss.bands;
    return r.frequencies_Hz.map((f) => bands.find((b) => f >= b.f_min_Hz && f <= b.f_max_Hz)?.exceeds_dB ?? null);
  }

  private renderPlots(r: IsolationReport): void {
    const db = (v: number) => formatDb(v, 2);
    const fixture = r.fixture_self_insertion_loss;
    const il: FigSeries[] = [{ id: 'insertion loss', slot: 0, values: r.insertion_loss_dB, primary: true }];
    if (r.insertion_loss_incoherent_dB) il.push({ id: 'paths in power', slot: 2, values: r.insertion_loss_incoherent_dB });
    il.push({ id: 'fixture self-insertion loss', slot: 3, values: this.fixtureLimit(r), tag: 'limit' });
    const plots: FigPlot[] = [
      { key: 'il', title: 'Insertion loss', symbol: 'IL', unit: '', axisUnit: 'dB', kind: 'delta', series: il, format: db, columnUnit: 'dB', cell: (v) => v.toFixed(2) },
    ];
    if (r.paths.length) {
      plots.push({
        key: 'paths',
        title: 'Path contributions at the drum, re the open ear',
        symbol: 'L_path',
        unit: '',
        axisUnit: 'dB re open ear',
        kind: 'delta',
        series: r.paths.map((p, k) => ({ id: `path ${p.element}`, slot: 4 + k, values: p.level_re_open_dB })),
        format: db,
        columnUnit: 'dB',
        cell: (v) => v.toFixed(2),
      });
    }
    plots.push({
      key: 'drum',
      title: 'Drum pressure for 94 dB SPL outside',
      symbol: 'SPL',
      unit: '',
      axisUnit: 'dB re 20 µPa',
      kind: 'spl',
      series: [
        { id: 'open ear', slot: 1, values: r.p_open.spl_dB },
        { id: 'occluded', slot: 0, values: r.p_occluded.spl_dB, primary: true },
      ],
      format: (v) => `${v.toFixed(2)} dB SPL`,
      columnUnit: 'dB SPL',
      cell: (v) => v.toFixed(2),
    });
    this.fig.set(plots, r.frequencies_Hz, r.shading);
    const ex = fixture.exceeded_at_Hz;
    const rs = ranges(r.frequencies_Hz, ex);
    this.exceed.replaceChildren(
      `Limit: ${fixture.fixture}, self-insertion loss stated as lower bounds (${fixture.status}; ${fixture.source}, `,
      el('a', { text: 'source', attrs: { href: fixture.url, target: '_blank', rel: 'noopener' } }),
      `): ${fixture.bands.map((b) => `> ${b.exceeds_dB} dB from ${formatHz(b.f_min_Hz)} to ${formatHz(b.f_max_Hz)}`).join(', ')}. `,
      ex.length
        ? `The predicted loss exceeds it at ${ex.length} frequencies (${rs.map(rangeText).join(', ')}): a measurement on that fixture is not guaranteed to resolve the prediction there.`
        : 'The predicted loss stays below it everywhere.',
    );
  }

  private renderBands(r: IsolationReport): void {
    const bands = r.third_octave_bands;
    if (!bands.length) {
      this.bands.replaceChildren(el('p', { class: 'hint', text: 'No 1/3-octave band lies inside the sweep.' }));
      return;
    }
    const fb = r.fixture_self_insertion_loss.bands;
    const boundOf = (f: number) => fb.find((b) => f >= b.f_min_Hz && f <= b.f_max_Hz)?.exceeds_dB ?? null;
    const values = bands.map((b) => b.insertion_loss_dB).filter((v): v is number => v !== null && Number.isFinite(v));
    // The datasheet's band limits are nominal band frequencies (its "80 Hz" is the band centred on 79.4 Hz).
    const bounds = bands.map((b) => boundOf(b.nominal_Hz)).filter((v): v is number => v !== null);
    const lo = Math.min(0, ...values);
    const hi = Math.max(10, ...values, ...bounds);
    const top = Math.ceil(hi / 10) * 10;
    const bottom = Math.floor(lo / 10) * 10;
    const pos = (v: number) => ((v - bottom) / (top - bottom)) * 100;
    const rows = bands.map((b) => {
      const v = b.insertion_loss_dB;
      const bound = boundOf(b.nominal_Hz);
      const over = v !== null && bound !== null && v > bound;
      const bar = el('div', { class: 'mv-bandbar', attrs: { 'aria-hidden': 'true' } });
      if (v !== null && Number.isFinite(v)) {
        const a = pos(Math.min(0, v));
        const z = pos(Math.max(0, v));
        bar.append(el('span', { class: `mv-bandfill${over ? ' over' : ''}`, attrs: { style: `left:${a.toFixed(2)}%;width:${Math.max(0.5, z - a).toFixed(2)}%` } }));
      }
      if (bound !== null) bar.append(el('span', { class: 'mv-bandlimit', attrs: { style: `left:${pos(bound).toFixed(2)}%` } }));
      return [
        b.nominal_Hz >= 1000 ? `${Number((b.nominal_Hz / 1000).toPrecision(3))}k` : String(Number(b.nominal_Hz.toPrecision(3))),
        formatNumber(b.center_Hz, 5),
        v === null ? '∞' : v.toFixed(1),
        bar,
        bound === null ? '—' : `> ${bound}${over ? ' (exceeded)' : ''}`,
      ];
    });
    const t = table(
      `Insertion loss in the base-ten 1/3-octave bands of IEC 61260-1 (∫|p|² df over each band; bands entirely inside the sweep). ` +
        `Bars from 0 dB on a scale of ${bottom} to ${top} dB; the dashed mark is the fixture’s self-insertion-loss bound, and a hatched bar exceeds it.`,
      ['Band (Hz)', 'Centre (Hz)', 'IL (dB)', `Bar (${bottom} to ${top} dB)`, 'Fixture bound (dB)'],
      rows,
      { rowHead: true, cls: 'mv-bands' },
    );
    t.querySelectorAll('tbody tr').forEach((tr, k) => ((tr as HTMLElement).dataset.nominal = String(bands[k].nominal_Hz)));
    this.bands.replaceChildren(scrollRegion('1/3-octave band table', t));
  }

  private renderBleed(r: IsolationReport): void {
    const b = r.bleed;
    this.bleedSection.hidden = !this.bleedBox.checked || b === null;
    if (!b) return;
    if ('error' in b) {
      this.bleedNote.replaceChildren(errorBox('Bleed not computed', b.error));
      this.bleedFig.root.hidden = true;
      return;
    }
    this.bleedFig.root.hidden = false;
    this.bleedNote.textContent = `At the netlist’s drive (${b.drive.label}). Model (engine): ${b.model}. The dashed lines are the curves ±${b.band_dB} dB.`;
    const series: FigSeries[] = [];
    b.distances_m.forEach((d, k) => {
      const values = b.spl_dB[k];
      const name = `${Number(d.toPrecision(3))} m`;
      series.push({ id: name, slot: k, values, primary: k === 0 });
      series.push({ id: `${name} +${b.band_dB} dB`, slot: k + 8, values: values.map((v) => (v === null ? null : v + b.band_dB)) });
      series.push({ id: `${name} −${b.band_dB} dB`, slot: k + 16, values: values.map((v) => (v === null ? null : v - b.band_dB)) });
    });
    this.bleedFig.set(
      [
        {
          key: 'bleed',
          title: 'Bleed',
          symbol: 'SPL',
          unit: '',
          axisUnit: 'dB re 20 µPa',
          kind: 'spl',
          series,
          format: (v) => `${v.toFixed(1)} dB SPL`,
          columnUnit: 'dB SPL',
          cell: (v) => v.toFixed(2),
        },
      ],
      b.frequencies_Hz,
      r.shading,
    );
  }
}

export const view = new IsolationView();
