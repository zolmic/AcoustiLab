// "Time": impulse, step and energy-time curve of a probe (mixed and minimum
// phase) with the engine's phase decision and impulse-length check (E46),
// group delay from the rational fit and excess group delay, the pole/zero/Q
// table of the vector fit with optional attribution of the resonances to
// parameters, and a float WAV of the impulse response. Engine exports
// `impulse` and `vector_fit` (docs/time-domain.md); every number shown comes
// from their reports.

import { Cancelled } from '../engine';
import { formatHz, formatNumber, prettyUnit } from '../format';
import { WAV_HEADER_BYTES, floatWav, wavGain } from './time-wav';
import { TimeFigure } from './time-plot';
import {
  Coalesced,
  FreqFigure,
  JobStatus,
  button,
  checkbox,
  defList,
  download,
  el,
  errorBox,
  field,
  formatDb,
  formatSeconds,
  isEngineError,
  keepFocus,
  registerHooks,
  safeName,
  scrollRegion,
  select,
  table,
  valueOf,
  type FigPlot,
} from './measure-kit';
import type { ResultView, ViewHost } from './types';
import type { Shading } from '../types';

// ----- report shapes (docs/time-domain.md, "Interfaces") -------------------------

interface Decision {
  mode: 'minimum' | 'mixed';
  threshold_s: number;
  band_Hz: [number, number] | null;
  bins: number;
  max_excess_group_delay_s: number | null;
  at_Hz: number | null;
  pure_delay_s: number | null;
  max_excess_group_delay_less_delay_s: number | null;
  polarity: number;
  min_phase_default: boolean;
}

interface PoleRef {
  f_Hz: number;
  q: number | null;
  t60_s: number;
  needed_s: number;
}

interface IrLength {
  fs_Hz: number;
  n: number;
  half_length_s: number;
  lowest: PoleRef | null;
  binding: PoleRef | null;
  covered: boolean;
  recommended_n: number | null;
  within_audition_range: boolean;
}

interface ImpulseReport {
  probe: string;
  quantity: string;
  unit: string;
  fs_Hz: number;
  n: number;
  df_Hz: number;
  t0_s: number;
  dt_s: number;
  pre_samples: number;
  ir: number[];
  ir_min_phase: number[];
  step: number[];
  step_min_phase: number[];
  etc_dB: number[];
  etc_min_phase_dB: number[];
  peak_s: number;
  late_energy_dB: number | null;
  late_energy_min_phase_dB: number | null;
  band_edge_energy_dB: number | null;
  causal_to_80dB: boolean;
  decision: Decision;
  min_phase: { dc_order: number; dc_corner_Hz: number | null; hf_slope: number; polarity: number; cepstrum_len: number };
  extrapolation: { solved_Hz: [number, number]; dc_rule: string; dc_order: number; dc_corner_Hz: number | null; hf_slope: number; taper_Hz: [number, number] | null };
  ir_length: IrLength | null;
  ir_length_error?: string | null;
  dc_tail: { order: number; corner_Hz: number; time_constant_s: number; needed_s: number; covered: boolean } | null;
  fit_rhp_zeros_in_decision_band_Hz: number[] | null;
  delay_removed_s: number;
  drive: { label: string } | null;
  shading: Shading;
  warnings: { code?: string; message: string }[];
  time_axis: string;
  scaling: string;
  frequencies_Hz?: number[];
  excess_group_delay_s?: (number | null)[];
  trusted?: boolean[];
}

interface Pole {
  re_rad_per_s: number;
  im_rad_per_s: number;
  f_Hz: number;
  q: number | null;
  real: boolean;
  in_band: boolean;
  weight_dB?: number | null;
  resonant?: boolean;
  t60_s?: number | null;
  rhp?: boolean;
}

interface Attribution {
  method: string;
  perturbed: string[];
  skipped: [string, string][];
  poles: {
    pole: number;
    f_Hz: number;
    q: number | null;
    parameters: { parameter: string; dlnf_dlnp: number; dlnQ_dlnp: number; elements: string[] }[];
    unmatched: string[];
    fixed_elements: string[];
  }[];
}

interface PolesReport {
  probe: string;
  quantity: string;
  unit: string;
  band_Hz: [number, number];
  order: number;
  asymptote: string;
  weighting: string;
  fit: { rms_relative: number | null; rms_dB: number | null; max_dB: number | null; max_deg: number | null; iterations: number; flipped: number };
  notes: string[];
  poles: Pole[];
  zeros: Pole[];
  min_phase_in_band: boolean | null;
  q_min: number;
  weight_min_dB: number;
  frequencies_Hz: number[];
  group_delay_s: (number | null)[];
  attribution: Attribution | null;
}

// ----- the view --------------------------------------------------------------------

/** Largest N the `impulse` export accepts. */
const N_OPTIONS = [4096, 8192, 16384];

const yesNo = (b: boolean | null | undefined) => (b === null || b === undefined ? 'n/a' : b ? 'yes' : 'no');
const qText = (q: number | null | undefined) => (q === null || q === undefined ? '—' : formatNumber(q, 4));

class TimeView implements ResultView {
  readonly id = 'time';
  readonly label = 'Time';
  readonly order = 91;
  private host!: ViewHost;

  private probeSel!: HTMLSelectElement;
  private nSel!: HTMLSelectElement;
  private fsSel!: HTMLSelectElement;
  private align!: HTMLInputElement;
  private orderIn!: HTMLInputElement;
  private status!: HTMLElement;
  private stale!: HTMLElement;
  private errorEl!: HTMLElement;
  private summary!: HTMLElement;
  private cards!: HTMLElement;
  private timeFig!: TimeFigure;
  private wavPhase!: HTMLSelectElement;
  private wavRaw!: HTMLInputElement;
  private wavBtn!: HTMLButtonElement;
  private wavNote!: HTMLElement;
  private gdFig!: FreqFigure;
  private exFig!: FreqFigure;
  private gdNote!: HTMLElement;
  private exNote!: HTMLElement;
  private polesEl!: HTMLElement;
  private attrBtn!: HTMLButtonElement;
  private attrJob!: JobStatus;
  private attrEl!: HTMLElement;
  private busy!: JobStatus;
  private impulseSection!: HTMLElement;
  private polesSection!: HTMLElement;

  private impulse!: Coalesced;
  private poles!: Coalesced;
  private report: ImpulseReport | null = null;
  private polesReport: PolesReport | null = null;
  private attrKey = '';
  private marked: number | null = null;

  mount(root: HTMLElement, host: ViewHost): void {
    this.host = host;
    root.classList.add('mv');
    const intro = el('p', {
      class: 'hint',
      text:
        'Impulse, step and energy-time curve of a probe from a uniform re-solve of the network (docs/time-domain.md), with the engine’s ' +
        'phase decision and impulse-length check, the group delay of its rational fit, and the fit’s poles. Computed on this view’s own worker ' +
        'for the netlist as last solved.',
    });

    this.probeSel = select([['', 'Default probe']]);
    this.nSel = select(N_OPTIONS.map((n) => [String(n), String(n)]), '8192');
    this.fsSel = select([['48000', '48 kHz'], ['96000', '96 kHz']], '48000');
    const alignBox = checkbox('Remove the pure delay from the mixed-phase response');
    this.align = alignBox.input;
    this.orderIn = el('input', { class: 'mv-num', attrs: { type: 'number', min: 1, max: 80, step: 1, value: 30, inputmode: 'numeric' } });
    for (const c of [this.probeSel, this.nSel, this.fsSel, this.align]) c.addEventListener('change', () => this.update());
    this.orderIn.addEventListener('change', () => {
      const v = Math.round(Number(this.orderIn.value));
      this.orderIn.value = String(Number.isFinite(v) ? Math.min(80, Math.max(1, v)) : 30);
      this.update();
    });
    const controls = el(
      'div',
      { class: 'mv-bar', attrs: { role: 'group', 'aria-label': 'Impulse response options' } },
      field('Probe', this.probeSel),
      field('FFT length N', this.nSel),
      field('Sample rate', this.fsSel),
      field('Fit order', this.orderIn),
      alignBox.wrap,
    );

    this.busy = new JobStatus('Computing the time-domain results', () => this.cancelAll());
    this.status = el('p', { class: 'hint mv-status', attrs: { role: 'status' } });
    this.stale = el('p', { class: 'hint mv-stale', hidden: true, text: 'The netlist has been edited since it was last solved; these results are for the solved text.' });
    this.errorEl = el('div');
    this.summary = el('p', { class: 'mv-summary' });
    this.cards = el('div', { class: 'mv-cards' });

    const peakView = (): [number, number] => {
      const r = this.report;
      if (!r) return [-0.002, 0.02];
      return [Math.max(r.t0_s, r.peak_s - 0.002), Math.min(r.t0_s + (r.n - 1) * r.dt_s, r.peak_s + 0.02)];
    };
    const fullView = (): [number, number] => {
      const r = this.report;
      return r ? [r.t0_s, r.t0_s + (r.n - 1) * r.dt_s] : [-0.002, 0.02];
    };
    this.timeFig = new TimeFigure(
      {
        id: 'time-ir',
        label: 'Impulse, step and energy-time curve',
        host,
        help:
          'Plots: move the pointer or focus a plot and use ←/→ (with Shift: 10 samples) to read values; +/− zoom, Ctrl+←/→ pan, 0 resets the view, Esc hides the crosshair. Drag across a plot to zoom; double-click resets.',
      },
      [
        { label: 'Peak −2 to +20 ms', key: 'peak', range: peakView },
        { label: 'Whole buffer', key: 'full', range: fullView },
      ],
    );

    this.wavPhase = select([['decision', 'Phase of the decision'], ['minimum', 'Minimum phase'], ['mixed', 'Mixed phase']], 'decision');
    const raw = checkbox('Raw values (not normalised to a peak of 1)');
    this.wavRaw = raw.input;
    this.wavBtn = button('Download WAV', () => this.downloadWav(), { attrs: { disabled: true } });
    this.wavNote = el('p', { class: 'hint', attrs: { role: 'status' } });
    const wav = el(
      'div',
      { class: 'mv-bar mv-wav', attrs: { role: 'group', 'aria-label': 'WAV download of the impulse response' } },
      field('WAV of', this.wavPhase),
      raw.wrap,
      this.wavBtn,
    );

    this.gdNote = el('p', { class: 'hint' });
    this.exNote = el('p', { class: 'hint' });
    const fhelp =
      'Plots: move the pointer or focus a plot and use ←/→ (with Shift: 10 points) to read values; +/− zoom, Ctrl+←/→ pan, 0 default view (20 Hz–20 kHz), Esc hides the crosshair. Drag across a plot to zoom; double-click resets.';
    this.gdFig = new FreqFigure({ id: 'time-gd', label: 'Group delay of the rational fit', host, help: fhelp });
    this.exFig = new FreqFigure({ id: 'time-excess', label: 'Excess group delay', host, help: fhelp });

    this.polesEl = el('div');
    this.attrJob = new JobStatus('Attributing the resonances to parameters', () => this.cancelAttribution());
    this.attrBtn = button('Attribute resonances to parameters', () => void this.attribute(), { attrs: { disabled: true } });
    this.attrEl = el('div');

    this.impulseSection = el(
      'div',
      { hidden: true },
      this.summary,
      this.cards,
      el('h3', { class: 'mv-h', text: 'Impulse, step and energy-time curve' }),
      this.timeFig.root,
      wav,
      this.wavNote,
      el('h3', { class: 'mv-h', text: 'Excess group delay (uniform grid)' }),
      this.exNote,
      this.exFig.root,
    );
    this.polesSection = el(
      'div',
      { hidden: true },
      el('h3', { class: 'mv-h', text: 'Group delay of the rational fit' }),
      this.gdNote,
      this.gdFig.root,
      el('h3', { class: 'mv-h', text: 'Poles, zeros and Q of the rational fit' }),
      this.polesEl,
      el('div', { class: 'mv-bar' }, this.attrBtn),
      this.attrJob.root,
      this.attrEl,
    );
    root.append(intro, controls, this.busy.root, this.status, this.stale, this.errorEl, this.impulseSection, this.polesSection);

    const onBusy = () => {
      const b = !!(this.impulse?.busy || this.poles?.busy);
      if (b && !this.busy.running) this.busy.start('Computing the impulse response and the rational fit…');
      if (!b && this.busy.running) this.busy.hide();
    };
    this.impulse = new Coalesced(host, 'impulse', (k, v, ms) => this.onImpulse(k, v, ms), onBusy);
    this.poles = new Coalesced(host, 'vector_fit', (k, v) => this.onPoles(k, v), onBusy);

    registerHooks('time', {
      report: () => this.report,
      poles: () => this.polesReport,
      timeFigure: () => this.timeFig.hook(),
      gdFigure: () => this.gdFig.hook(),
      excessFigure: () => this.exFig.hook(),
      /** Moves a crosshair as the arrow keys do (announced). */
      setTimeCursor: (i: number | null) => this.timeFig.panel.setCursor(i, true),
      setGdCursor: (i: number | null) => this.gdFig.panel.setCursor(i, true),
      gdRow: (key: string, y: number) => this.gdFig.row(key, y),
      gdX: (key: string, f: number) => this.gdFig.xOf(key, f),
    });
  }

  refresh(): void {
    const cur = this.host.current();
    if (!cur) {
      this.status.textContent = 'Run a netlist to compute its impulse response.';
      return;
    }
    // Probe choices follow the solved netlist; the choice is kept while it exists.
    const ids = cur.result.probes.map((p) => `${p.id}|${p.quantity}`).join(',');
    if (this.probeSel.dataset.ids !== ids) {
      const keep = this.probeSel.value;
      this.probeSel.replaceChildren(el('option', { text: 'Default (ui.primary_probe, else the first pressure probe)', attrs: { value: '' } }));
      for (const p of cur.result.probes) this.probeSel.append(el('option', { text: `${p.id} (${p.quantity.replace(/_/g, ' ')})`, attrs: { value: p.id } }));
      this.probeSel.value = cur.result.probes.some((p) => p.id === keep) ? keep : '';
      this.probeSel.dataset.ids = ids;
    }
    this.stale.hidden = this.host.netlist() === cur.text;
    this.update();
  }

  private options(): { impulse: Record<string, unknown>; poles: Record<string, unknown> } {
    const probe = this.probeSel.value || undefined;
    const n = Number(this.nSel.value);
    const fs = Number(this.fsSel.value);
    const order = Number(this.orderIn.value) || 30;
    return {
      impulse: { probe, n, fs_Hz: fs, align_delay: this.align.checked, order },
      poles: { probe, order, n, fs_Hz: fs },
    };
  }

  /** Requests the reports for the solved text and the current options (coalesced; nothing is sent twice). */
  private update(): void {
    const cur = this.host.current();
    if (!cur) return;
    const o = this.options();
    const ki = JSON.stringify([cur.text, o.impulse]);
    const kp = JSON.stringify([cur.text, o.poles]);
    this.impulse.request(ki, [cur.text, '', JSON.stringify(o.impulse)]);
    this.poles.request(kp, [cur.text, '', JSON.stringify(o.poles)]);
  }

  /** Cancels everything on the worker (the Cancel of the computing line). */
  private cancelAll(): void {
    this.host.cancel();
    this.impulse.abandon();
    this.poles.abandon();
    this.busy.finish('Cancelled. Change an option, or solve again, to compute.', 'cancelled');
    if (this.attrJob.running) this.attrJob.finish('Attribution cancelled.', 'cancelled');
    this.host.announce('Time-domain computation cancelled.');
  }

  /** Cancels the attribution; the reports it interrupted are computed again. */
  private cancelAttribution(): void {
    this.host.cancel();
    this.attrJob.finish('Attribution cancelled.', 'cancelled');
    this.host.announce('Attribution cancelled.');
    this.impulse.resend();
    this.poles.resend();
  }

  // ----- impulse report -----------------------------------------------------------

  private onImpulse(_key: string, v: unknown, ms: number): void {
    this.errorEl.replaceChildren();
    if (isEngineError(v)) {
      this.report = null;
      this.impulseSection.hidden = true;
      this.errorEl.append(errorBox('Impulse response not computed', v.error, v.kind ? `Kind: ${v.kind}.` : undefined));
      this.status.textContent = 'The engine could not compute the impulse response.';
      this.wavBtn.disabled = true;
      return;
    }
    const r = v as ImpulseReport;
    this.report = r;
    this.impulseSection.hidden = false;
    this.status.textContent = `Impulse response computed in ${ms.toFixed(0)} ms.`;
    this.wavBtn.disabled = false;
    this.renderSummary(r);
    this.renderCards(r);
    this.renderTime(r);
    this.renderExcess(r);
  }

  private renderSummary(r: ImpulseReport): void {
    const ex = r.extrapolation;
    const unit = r.unit ? prettyUnit(r.unit) : 'unit unknown';
    this.summary.replaceChildren(
      el('strong', { text: r.probe }),
      ` (${r.quantity.replace(/_/g, ' ')}, ${unit}) at ${r.drive?.label ?? 'the netlist’s drive'}. N = ${r.n} at ${formatHz(r.fs_Hz)}: `,
      `a ${formatSeconds(r.n * r.dt_s)} buffer from ${formatSeconds(r.t0_s).replace(/^-/, '−')}, bins every ${formatHz(r.df_Hz)}; solved from ${formatHz(ex.solved_Hz[0])} to ${formatHz(ex.solved_Hz[1])}`,
      ex.taper_Hz ? `, continued as |H| ∝ f^n (n = ${formatNumber(ex.hf_slope, 3)}) and tapered to zero at ${formatHz(ex.taper_Hz[1])}.` : '.',
      r.delay_removed_s ? ` Mixed phase advanced by ${formatSeconds(r.delay_removed_s)} (pure-delay estimate).` : '',
    );
  }

  private renderCards(r: ImpulseReport): void {
    const d = r.decision;
    const L = r.ir_length;
    const t = r.dc_tail;
    const rhp = r.fit_rhp_zeros_in_decision_band_Hz;
    const decision = el(
      'section',
      { class: 'mv-card', attrs: { 'aria-labelledby': 'time-decision-h' } },
      el('h4', { id: 'time-decision-h', text: 'Phase decision (spec Section 16)' }),
      defList([
        ['Mode', el('strong', { text: d.mode === 'minimum' ? 'minimum phase' : 'mixed phase', attrs: { 'data-field': 'mode' } })],
        [
          'Largest excess group delay',
          `${formatSeconds(d.max_excess_group_delay_s)}${d.at_Hz !== null ? ` at ${formatHz(d.at_Hz)}` : ''} (threshold ${formatSeconds(d.threshold_s)}: ${d.min_phase_default ? 'below' : 'not below'})`,
        ],
        ['Pure-delay estimate', formatSeconds(d.pure_delay_s)],
        ['Largest excess less the pure delay', formatSeconds(d.max_excess_group_delay_less_delay_s)],
        ['Threshold', formatSeconds(d.threshold_s)],
        ['Trusted band', d.band_Hz ? `${formatHz(d.band_Hz[0])} to ${formatHz(d.band_Hz[1])} (${d.bins} bins: solved, unshaded, within 60 dB of the peak)` : 'none (no bin qualifies)'],
        ['Polarity', d.polarity < 0 ? '−1 (inverted)' : '+1'],
        [
          'Fit zeros in the right half-plane, in the decision band',
          rhp === null ? 'not checked (no fit)' : rhp.length ? rhp.map((f) => formatHz(f)).join(', ') : 'none',
        ],
      ]),
    );
    const lengthRows: [string, string | HTMLElement | null][] = [];
    if (L) {
      lengthRows.push(
        ['Causal half of the buffer', `${formatSeconds(L.half_length_s)} (N/(2·fs))`],
        ['Binding pole', L.binding ? `${formatHz(L.binding.f_Hz)}, Q ${qText(L.binding.q)}, T60 ${formatSeconds(L.binding.t60_s)}: needs ${formatSeconds(L.binding.needed_s)}` : 'no resonant pole'],
        ['Lowest resonant pole', L.lowest ? `${formatHz(L.lowest.f_Hz)}, Q ${qText(L.lowest.q)}, needs ${formatSeconds(L.lowest.needed_s)}` : null],
        ['Covered', el('strong', { text: L.covered ? 'yes' : 'no', attrs: { 'data-field': 'covered' } })],
        [
          'Recommended N',
          L.recommended_n === null
            ? 'n/a'
            : `${L.recommended_n}${L.within_audition_range ? '' : ' (beyond the 16 384 limit of this export)'}${L.recommended_n === r.n ? ' (in use)' : ''}`,
        ],
      );
    } else {
      lengthRows.push(['Check', r.ir_length_error ? `not made: ${r.ir_length_error}` : 'not made']);
    }
    if (t) {
      lengthRows.push([
        'Tail below the band (DC corner)',
        `order ${t.order}, corner ${formatHz(t.corner_Hz)}, τ = ${formatSeconds(t.time_constant_s)}: 80 dB decay needs ${formatSeconds(t.needed_s)}; covered: ${yesNo(t.covered)}`,
      ]);
    }
    lengthRows.push(
      ['Late energy (second half of the buffer)', `mixed ${formatDb(r.late_energy_dB)}, minimum phase ${formatDb(r.late_energy_min_phase_dB)}`],
      ['Energy above the solved band', formatDb(r.band_edge_energy_dB)],
      ['Causal to −80 dB', yesNo(r.causal_to_80dB)],
    );
    const length = el(
      'section',
      { class: 'mv-card', attrs: { 'aria-labelledby': 'time-length-h' } },
      el('h4', { id: 'time-length-h', text: 'Impulse length (E46)' }),
      defList(lengthRows),
    );
    if (L && L.recommended_n !== null && L.recommended_n !== r.n && N_OPTIONS.includes(L.recommended_n)) {
      const n = L.recommended_n;
      length.append(
        button(`Use N = ${n}`, () => {
          this.nSel.value = String(n);
          this.update();
        }),
      );
    }
    const warnings = r.warnings ?? [];
    this.cards.replaceChildren(decision, length);
    if (warnings.length) {
      this.cards.append(
        el(
          'section',
          { class: 'mv-card mv-warn', attrs: { 'aria-labelledby': 'time-warn-h' } },
          el('h4', { id: 'time-warn-h', text: `Engine warnings (${warnings.length})` }),
          el('ul', {}, ...warnings.map((w) => el('li', { text: w.message }))),
        ),
      );
    }
    const conv = el(
      'details',
      { class: 'mv-card mv-conv' },
      el('summary', { text: 'Time axis and scaling (engine)' }),
      el('p', { text: r.time_axis }),
      el('p', { text: r.scaling }),
    );
    this.cards.append(conv);
  }

  private renderTime(r: ImpulseReport): void {
    const unit = r.unit ?? '';
    const pu = unit ? prettyUnit(unit) : 'unit unknown';
    const min = r.decision.mode === 'minimum';
    const series = (a: number[], b: number[]) => [
      { id: 'minimum phase', slot: 0, values: b, primary: min, tag: min ? 'decision' : undefined },
      { id: 'mixed phase', slot: 1, values: a, primary: !min, tag: min ? undefined : 'decision' },
    ];
    const amp = (v: number) => formatNumber(v, 5) + (unit ? ` ${pu}` : '');
    const plots: FigPlot[] = [
      {
        key: 'ir',
        title: 'Impulse response',
        symbol: 'h[n]',
        unit,
        axisUnit: pu,
        kind: 'mag',
        series: series(r.ir, r.ir_min_phase),
        format: amp,
        columnUnit: pu,
      },
      {
        key: 'step',
        title: 'Step response',
        symbol: 's[n]',
        unit,
        axisUnit: pu,
        kind: 'mag',
        series: series(r.step, r.step_min_phase),
        format: amp,
        columnUnit: pu,
      },
      {
        key: 'etc',
        title: 'Energy-time curve',
        symbol: 'ETC',
        unit: '',
        axisUnit: 'dB re max',
        kind: 'spl',
        series: series(r.etc_dB, r.etc_min_phase_dB),
        format: (v) => formatDb(v, 1),
        columnUnit: 'dB',
        cell: (v) => v.toFixed(2),
      },
    ];
    this.timeFig.set(plots, { t0: r.t0_s, dt: r.dt_s, n: r.n });
  }

  private renderExcess(r: ImpulseReport): void {
    const f = r.frequencies_Hz;
    const ex = r.excess_group_delay_s;
    const trusted = r.trusted;
    if (!f || !ex || !trusted) {
      this.exNote.textContent = 'The report carries no spectrum.';
      return;
    }
    const th = r.decision.threshold_s;
    // DC (bin 0) has no place on a log axis.
    const freqs = f.slice(1);
    const plots: FigPlot[] = [
      {
        key: 'excess',
        title: 'Excess group delay',
        symbol: 'τ_ex',
        unit: 's',
        axisUnit: 's',
        kind: 'mag',
        series: [
          { id: 'excess group delay', slot: 0, values: ex.slice(1), primary: true },
          { id: '+threshold, trusted band', slot: 3, values: trusted.slice(1).map((t) => (t ? th : null)) },
          { id: '−threshold, trusted band', slot: 11, values: trusted.slice(1).map((t) => (t ? -th : null)) },
        ],
        format: (v) => formatSeconds(v, 4),
        columnUnit: 's',
      },
    ];
    const d = r.decision;
    this.exNote.textContent =
      `From the ${r.n / 2 + 1}-bin uniform grid, against the minimum-phase counterpart. The threshold lines span the trusted band` +
      (d.band_Hz ? ` (${formatHz(d.band_Hz[0])} to ${formatHz(d.band_Hz[1])})` : ' (empty)') +
      `; the decision compares the largest |excess group delay| there with ${formatSeconds(th)}.`;
    this.exFig.set(plots, freqs, r.shading);
    if (this.marked !== null) this.exFig.mark(this.marked, this.markLabel(this.marked));
  }

  // ----- poles ----------------------------------------------------------------------

  private onPoles(key: string, v: unknown): void {
    if (isEngineError(v)) {
      this.polesReport = null;
      this.polesSection.hidden = false;
      this.gdFig.root.hidden = true;
      this.gdNote.textContent = '';
      this.attrEl.replaceChildren();
      this.polesEl.replaceChildren(errorBox('Rational fit not computed', v.error, v.kind ? `Kind: ${v.kind}.` : undefined));
      this.attrBtn.disabled = true;
      return;
    }
    const p = v as PolesReport;
    this.polesReport = p;
    this.polesSection.hidden = false;
    this.gdFig.root.hidden = false;
    this.attrBtn.disabled = false;
    if (this.attrKey !== key) {
      this.attrEl.replaceChildren();
      this.attrJob.hide();
    }
    const fit = p.fit;
    this.gdNote.textContent =
      `Order ${p.order} fit of ${p.probe} over ${formatHz(p.band_Hz[0])} to ${formatHz(p.band_Hz[1])} (${fit.iterations} iterations): ` +
      `RMS error ${formatNumber(fit.rms_dB, 2)} dB, largest ${formatNumber(fit.max_dB, 2)} dB and ${formatNumber(fit.max_deg, 2)}°. ` +
      'Group delay −Re(H′/H) from the poles and residues.';
    const current = this.host.current();
    const shading = this.report?.shading ?? current?.result.shading ?? null;
    this.gdFig.set(
      [
        {
          key: 'gd',
          title: 'Group delay (rational fit)',
          symbol: 'τg',
          unit: 's',
          axisUnit: 's',
          kind: 'mag',
          series: [{ id: 'group delay', slot: 0, values: p.group_delay_s, primary: true }],
          format: (v) => formatSeconds(v, 4),
          columnUnit: 's',
        },
      ],
      p.frequencies_Hz,
      shading,
    );
    this.renderPoles(p);
    if (this.marked !== null && !p.poles.some((q) => q.f_Hz === this.marked)) this.setMark(null);
  }

  private markLabel(f: number): string {
    const pole = this.polesReport?.poles.find((q) => q.f_Hz === f);
    return pole ? `pole ${formatHz(f)}${pole.q !== null ? `, Q ${qText(pole.q)}` : ''}` : formatHz(f);
  }

  private setMark(f: number | null): void {
    this.marked = f;
    const label = f === null ? '' : this.markLabel(f);
    this.gdFig.mark(f, label);
    this.exFig.mark(f, label);
    for (const b of this.polesEl.querySelectorAll<HTMLButtonElement>('button[data-pole]')) {
      b.setAttribute('aria-pressed', String(f !== null && Number(b.dataset.pole) === f));
    }
    this.host.announce(f === null ? 'Pole mark cleared.' : `Marked ${label} on the frequency plots.`);
  }

  private renderPoles(p: PolesReport): void {
    const sorted = [...p.poles].sort((a, b) => a.f_Hz - b.f_Hz);
    const resonant = sorted.filter((q) => q.resonant);
    const rows = sorted.map((q) => {
      const mark = button('Mark', () => this.setMark(this.marked === q.f_Hz ? null : q.f_Hz), {
        class: 'mv-mark',
        attrs: { 'aria-pressed': String(this.marked === q.f_Hz), 'data-pole': q.f_Hz, 'data-k': `mark-${q.f_Hz}`, 'aria-label': `Mark ${formatHz(q.f_Hz)} on the frequency plots` },
      });
      return [
        formatHz(q.f_Hz),
        q.real ? 'real' : 'pair',
        qText(q.q),
        formatSeconds(q.t60_s ?? null, 4),
        q.weight_dB === null || q.weight_dB === undefined ? 'n/a' : formatDb(q.weight_dB, 1),
        yesNo(q.in_band),
        q.resonant ? el('strong', { text: 'yes' }) : 'no',
        mark,
      ];
    });
    const caption =
      `Poles of the order-${p.order} fit (${p.poles.length}; ${resonant.length} resonant: a pair with Q ≥ ${formatNumber(p.q_min, 3)}, inside the band, ` +
      `weight ≥ ${formatNumber(p.weight_min_dB, 3)} dB). T60 = 6.91/|Re a|; weight: the pole’s own term at its resonance relative to the whole model. “Mark” shows the frequency on the plots.`;
    const t = table(caption, ['Frequency', 'Type', 'Q', 'T60', 'Weight', 'In band', 'Resonant', 'Plots'], rows, { rowHead: true, cls: 'mv-poles' });
    t.querySelectorAll('tbody tr').forEach((tr, k) => {
      if (sorted[k].resonant) tr.classList.add('mv-resonant');
      (tr as HTMLElement).dataset.f = String(sorted[k].f_Hz);
    });
    const zeros = [...p.zeros].sort((a, b) => a.f_Hz - b.f_Hz);
    const zt = table(
      `Zeros of the fit (${zeros.length}); a right-half-plane zero makes the model non-minimum-phase.`,
      ['Frequency', 'Type', 'Q', 'Half-plane', 'In band'],
      zeros.map((z) => [formatHz(z.f_Hz), z.real ? 'real' : 'pair', qText(z.q), z.rhp ? 'right' : 'left', yesNo(z.in_band)]),
      { rowHead: true },
    );
    const notes = p.notes.length ? el('ul', { class: 'mv-notes' }, ...p.notes.map((n) => el('li', { text: n }))) : null;
    const region = scrollRegion('Pole table', t);
    region.dataset.k = 'home';
    keepFocus(this.polesEl, () =>
      this.polesEl.replaceChildren(
        el('p', { class: 'hint' }, `Minimum phase within the band (no right-half-plane zero there): ${yesNo(p.min_phase_in_band)}.`),
        notes ?? '',
        region,
        el('details', { class: 'mv-details' }, el('summary', { text: `Zeros (${zeros.length})` }), scrollRegion('Zero table', zt)),
      ),
    );
  }

  private async attribute(): Promise<void> {
    const cur = this.host.current();
    if (!cur || this.attrJob.running) return;
    const o = { ...this.options().poles, attribute: true };
    const key = JSON.stringify([cur.text, this.options().poles]);
    this.attrBtn.disabled = true;
    this.attrJob.start('Re-fitting the probe once per continuous parameter (+1 % each)…');
    this.host.announce('Attribution started.');
    let reply;
    try {
      reply = await this.host.call('vector_fit', cur.text, '', JSON.stringify(o));
    } catch (e) {
      this.attrBtn.disabled = false;
      if (e instanceof Cancelled) return; // the cancel handler has taken over
      throw e;
    }
    this.attrBtn.disabled = false;
    const v = valueOf(reply);
    if (isEngineError(v)) {
      this.attrJob.finish(`Attribution failed: ${v.error}`, 'failed');
      return;
    }
    const a = (v as PolesReport).attribution;
    this.attrKey = key;
    this.attrJob.finish(`Attribution done: ${a?.perturbed.length ?? 0} parameters perturbed.`);
    this.host.announce('Attribution done.');
    this.renderAttribution(a);
  }

  private renderAttribution(a: Attribution | null): void {
    this.attrEl.replaceChildren();
    if (!a) return;
    if (!a.poles.length) {
      this.attrEl.append(el('p', { class: 'hint', text: 'The fit has no resonant pole to attribute.' }));
    }
    for (const p of a.poles) {
      const t = table(
        `Pole at ${formatHz(p.f_Hz)}, Q ${qText(p.q)}: the parameters that move it most (logarithmic sensitivities)`,
        ['Parameter', 'd ln f / d ln p', 'd ln Q / d ln p', 'Elements that use it'],
        p.parameters.map((s) => [s.parameter, formatNumber(s.dlnf_dlnp, 3), formatNumber(s.dlnQ_dlnp, 3), s.elements.join(', ')]),
        { rowHead: true },
      );
      const sec = el('section', { class: 'mv-attr', attrs: { 'data-f': p.f_Hz } }, scrollRegion(`Attribution of the pole at ${formatHz(p.f_Hz)}`, t));
      if (p.fixed_elements.length) {
        sec.append(el('p', { class: 'hint', text: `No perturbed parameter moves it (|d ln f/d ln p| < 0.05); enabled elements that depend on no perturbed parameter: ${p.fixed_elements.join(', ')}.` }));
      }
      if (p.unmatched.length) sec.append(el('p', { class: 'hint', text: `Not matched after perturbing: ${p.unmatched.join(', ')}.` }));
      this.attrEl.append(sec);
    }
    this.attrEl.append(
      el(
        'details',
        { class: 'mv-details' },
        el('summary', { text: `Parameters perturbed (${a.perturbed.length}) and skipped (${a.skipped.length})` }),
        el('p', { text: `Method: ${a.method}. Perturbed: ${a.perturbed.join(', ') || 'none'}.` }),
        a.skipped.length ? el('ul', {}, ...a.skipped.map(([n, why]) => el('li', { text: `${n}: ${why}` }))) : '',
      ),
    );
  }

  // ----- WAV ------------------------------------------------------------------------

  private downloadWav(): void {
    const r = this.report;
    if (!r) return;
    const mode = this.wavPhase.value === 'decision' ? r.decision.mode : (this.wavPhase.value as 'minimum' | 'mixed');
    const h = mode === 'minimum' ? r.ir_min_phase : r.ir;
    const raw = this.wavRaw.checked;
    const gain = wavGain(h, raw);
    let bytes: Uint8Array;
    try {
      bytes = floatWav(
        h.map((x) => x * gain),
        r.fs_Hz,
      );
    } catch (e) {
      this.wavNote.textContent = `No WAV written: ${(e as Error).message}.`;
      return;
    }
    const name = `${safeName(r.probe)}-${mode}-phase-${Math.round(r.fs_Hz)}Hz-N${r.n}.wav`;
    download(name, bytes as BlobPart, 'audio/wav');
    this.wavNote.textContent =
      `Wrote ${name}: ${mode}-phase impulse response of ${r.probe}, ${r.n} samples of 32-bit float at ${formatHz(r.fs_Hz)} ` +
      `(${bytes.length - WAV_HEADER_BYTES} bytes of samples), first sample at t = ${formatSeconds(r.t0_s, 4).replace(/^-/, '−')}, ` +
      (gain === 1 ? 'raw values.' : `normalised to a peak of 1 (gain ${gain.toExponential(4)}).`);
  }
}

export const view = new TimeView();
