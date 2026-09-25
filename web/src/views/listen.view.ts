// "Listen": auralization of the current design against a baseline (spec
// Section 16; docs/auralization.md).
//
// The engine designs the audition filter (the `audition_filter` export, on
// this view's worker): candidate = the current design, baseline = a frozen
// baseline, the loaded template's values, a bundled target or an imported
// measurement. The page plays a looping programme through it (A) against
// the programme itself (B), both loudness matched over the actual
// programme after convolution (audio/level.ts, on its own worker), through
// an AudioWorklet with partitioned convolution, crossfades and a true-peak
// limiter (audio/processor.ts). Nothing plays until "Play" is pressed.
//
// Every number and sentence about the filter comes from the engine's
// report; the meters and level-match figures come from the audio code
// measuring the actual signal.

import './listen.css';
import { formatHz } from '../format';
import { lineKey } from '../keys';
import { PlotPanel } from '../plot';
import type { PlotGroup } from '../series';
import type { Shading } from '../types';
import { delayTaps, matchGainDb, TARGET_LUFS, TARGET_RMS_DB, type MatchMethod, type Measurement } from '../audio/level';
import { generate, type Programme, type ProgrammeKind } from '../audio/noise';
import { AuditionPlayer, CEILING_DBTP, MAX_GAIN_DB, START_VOLUME_DB, VOLUME_RANGE_DB, type Diagnostics, type Meters } from '../audio/player';
import type { ResultView, ViewHost } from './types';

/** The parts of the engine's filter report this view reads. */
interface FilterReport {
  mode: 'difference' | 'absolute';
  fs_Hz: number;
  n: number;
  latency_samples: number;
  latency_s: number;
  band_Hz: [number, number];
  anchor_gain_dB: number;
  phase: {
    requested: string;
    used: string;
    reason: string;
    delay_removed_s: number;
    hybrid_from_Hz: number | null;
    polarity: number;
    decision: { band_Hz: [number, number] | null; max_excess_group_delay_s: number; threshold_s: number };
  };
  ir_length: {
    n: number;
    needed_s: number;
    half_length_s: number;
    covered: boolean;
    recommended_n: number;
    binding: { f_Hz: number; q: number; needed_s: number } | null;
  } | null;
  ir_length_error: string | null;
  check: {
    band_Hz: [number, number];
    tolerance_dB: number;
    max_abs_error_dB: number;
    at_Hz: number;
    met: boolean;
    exceeded_Hz: [number, number][];
    resolution_Hz: number;
  } | null;
  tail_energy_dB: number;
  pre_energy_dB: number | null;
  max_boost_dB: { dB: number; at_Hz: number };
  max_cut_dB: { dB: number; at_Hz: number };
  frequencies_Hz: number[];
  design_dB: number[];
  fir_dB: number[];
  error_dB: number[];
  candidate: { probe: string; fixture: string | null; shading: Shading; drive: { label: string } };
  baseline: { kind: string; probe?: string; name?: string; label?: string; fixture?: string | null; shading?: Shading; drive?: { label: string } };
  inversion: {
    band_Hz: [number, number];
    boost_cap_dB: number;
    notch_limit_dB: number;
    smoothing: string;
    reference_dB: number;
    notches: { from_Hz: number; to_Hz: number; depth_dB: number; at_Hz: number; inverted: boolean }[];
    max_boost_dB: { dB: number; at_Hz: number };
    max_departure_from_exact_dB: { dB: number; at_Hz: number };
  } | null;
  flags: { code: string; message: string }[];
  notes: string[];
  state: Record<string, unknown>;
  taps: number[];
}

interface EngineError {
  error: string;
  kind: string;
  design?: string;
}

interface BaselineChoice {
  id: string;
  label: string;
  spec: () => Record<string, unknown> | null;
}

const PROGRAMMES: [ProgrammeKind | 'file', string][] = [
  ['pink_kellet', 'Pink noise (Kellet filter)'],
  ['pink_voss', 'Pink noise (Voss–McCartney)'],
  ['white', 'White noise'],
  ['sweep', 'Sine sweep 20 Hz–20 kHz'],
  ['file', 'Audio file…'],
];

const MATCH: [MatchMethod, string][] = [
  ['bs1770', 'Integrated loudness (ITU-R BS.1770)'],
  ['rms', 'RMS level'],
  ['midband', 'Mid-band anchor (500 Hz–2 kHz)'],
];

/** Longest audio file kept, s (the rest is cut). */
export const FILE_CAP_S = 30;

function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> & Record<string, unknown> = {}, ...children: (Node | string)[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k.startsWith('aria-') || k.startsWith('data-') || k === 'role' || k === 'for') e.setAttribute(k, String(v));
    else (e as unknown as Record<string, unknown>)[k] = v;
  }
  e.append(...children);
  return e;
}

const fmt = (v: number, d = 1) => (Number.isFinite(v) ? v.toFixed(d).replace(/^-(0\.?0*)$/, '$1') : '—');
const signed = (v: number, d = 1) => (Number.isFinite(v) ? `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(d)}` : '—');
const hz = (f: number) => formatHz(f);

async function sha256Hex(data: ArrayBuffer): Promise<string | null> {
  try {
    const d = await crypto.subtle.digest('SHA-256', data);
    return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, '0')).join('');
  } catch {
    return null;
  }
}

/** The level-match worker (audio/analysis.worker.ts). */
class LevelMatcher {
  private worker: Worker | null = null;
  private seq = 0;
  private sentKey: string | null = null;
  private readonly pending = new Map<number, (r: { programme: Measurement; filters: Measurement[]; ms: number } | { error: string }) => void>();

  private spawn(): Worker {
    const w = new Worker(new URL('../audio/analysis.worker.ts', import.meta.url), { type: 'module', name: 'acoustilab-level' });
    w.onmessage = (ev: MessageEvent<{ id: number } & ({ programme: Measurement; filters: Measurement[]; ms: number } | { error: string })>) => {
      const p = this.pending.get(ev.data.id);
      this.pending.delete(ev.data.id);
      p?.(ev.data);
    };
    w.onerror = (ev) => {
      ev.preventDefault();
      this.cancel('the level-match worker failed');
    };
    return w;
  }

  measure(key: string, programme: Programme, filters: { key?: string; taps: Float32Array[] }[]): Promise<{ programme: Measurement; filters: Measurement[]; ms: number }> {
    this.worker ??= this.spawn();
    const id = ++this.seq;
    const send: { key: string; channels?: Float32Array[]; mono?: boolean } = { key };
    if (this.sentKey !== key) {
      const [l, r] = programme.channels;
      send.mono = l === r;
      send.channels = send.mono ? [l.slice()] : [l.slice(), r.slice()];
      this.sentKey = key;
    }
    return new Promise((resolve, reject) => {
      this.pending.set(id, (r) => ('error' in r ? reject(new Error(r.error)) : resolve(r)));
      this.worker!.postMessage({ id, programme: send, filters, fs: programme.fs });
    });
  }

  cancel(reason = 'cancelled'): void {
    this.worker?.terminate();
    this.worker = null;
    this.sentKey = null;
    for (const p of this.pending.values()) p({ error: reason });
    this.pending.clear();
  }
}

class ListenView implements ResultView {
  readonly id = 'listen';
  readonly label = 'Listen';
  readonly order = 95;
  private host!: ViewHost;
  private readonly player = new AuditionPlayer();
  private readonly matcher = new LevelMatcher();
  private panel!: PlotPanel;
  // Controls.
  private baselineSel!: HTMLSelectElement;
  private curveInput!: HTMLInputElement;
  private modeRadios!: HTMLInputElement[];
  private phaseRadios!: HTMLInputElement[];
  private designBtn!: HTMLButtonElement;
  private cancelBtn!: HTMLButtonElement;
  private followBox!: HTMLInputElement;
  private status!: HTMLElement;
  private progress!: HTMLElement;
  private reportEl!: HTMLElement;
  private readoutEl!: HTMLElement;
  private legendEl!: HTMLElement;
  private tableWrap!: HTMLElement;
  private programmeSel!: HTMLSelectElement;
  private seedInput!: HTMLInputElement;
  private fileInput!: HTMLInputElement;
  private matchRadios!: HTMLInputElement[];
  private matchActive!: HTMLElement;
  private playBtn!: HTMLButtonElement;
  private abRadios!: HTMLInputElement[];
  private volume!: HTMLInputElement;
  private volumeNum!: HTMLInputElement;
  private meterEls!: Record<'momentary' | 'shortTerm' | 'truePeak' | 'gainReduction', { meter: HTMLMeterElement; text: HTMLElement }>;
  private playingEl!: HTMLElement;
  private levelEl!: HTMLElement;
  private diagEl!: HTMLElement;
  private stateText!: HTMLTextAreaElement;
  private programmeNotes!: HTMLElement;
  // State.
  private report: FilterReport | null = null;
  private reportKey: string | null = null;
  /** Name of the netlist baseline the report was designed against. */
  private baselineLabel = '';
  private synthesisMs: number | null = null;
  private busy = false;
  private queued: string | null = null;
  private curve: { doc: unknown; label: string } | null = null;
  private targets: { name: string; label: string }[] = [];
  private programme: Programme | null = null;
  private fileProgramme: Programme | null = null;
  private levels: { method: MatchMethod; programme: Measurement; a: Measurement; b: Measurement; gainA: number; gainB: number; ms: number } | null = null;
  private pollTimer = 0;
  private lastText: string | null = null;
  /** Incremented by Stop and by leaving the view: a Play still preparing gives up. */
  private playToken = 0;
  private limitText!: HTMLElement;
  private absoluteNote!: HTMLElement;

  mount(el: HTMLElement, host: ViewHost): void {
    this.host = host;
    el.classList.add('listen');
    el.append(
      h('p', { className: 'hint' },
        'Hear the current design against a baseline. The engine divides the design’s response by the baseline’s at the same reference point; ',
        'the programme plays through that filter (A) or unfiltered (B), both matched in loudness, so what changes between A and B is the difference between the designs, whatever headphones you listen on.'),
      this.buildFilterSection(),
      this.buildPlaybackSection(),
      this.buildDiagnostics(),
    );
    void this.loadTargets();
  }

  refresh(): void {
    this.fillBaselines();
    const cur = this.host.current();
    const text = cur?.text ?? null;
    this.designBtn.disabled = !cur;
    if (!cur) this.setStatus('Run a netlist to design an audition filter.');
    if (text !== this.lastText) {
      this.lastText = text;
      if (this.report && this.followBox.checked && text) this.requestDesign();
    }
    this.renderDiagnostics();
  }

  hide(): void {
    // Leaving the view stops the sound: its Stop button and meters would be
    // out of sight. A Play still preparing (designing, matching) gives up.
    this.playToken++;
    if (this.player.running) this.stop();
    else if (this.playBtn) {
      this.playBtn.disabled = false;
      this.player.mute();
    }
  }

  // ----- UI construction ---------------------------------------------------

  private radioGroup(name: string, legend: string, items: [string, string][], checked: string, onChange: (v: string) => void): { fs: HTMLFieldSetElement; radios: HTMLInputElement[] } {
    const fs = h('fieldset', { className: 'listen-radios' }, h('legend', {}, legend));
    const radios = items.map(([value, label]) => {
      const id = `listen-${name}-${value}`;
      const r = h('input', { type: 'radio', name: `listen-${name}`, id, value, checked: value === checked });
      r.addEventListener('change', () => r.checked && onChange(value));
      fs.append(h('div', { className: 'listen-radio' }, r, h('label', { for: id }, label)));
      return r;
    });
    return { fs, radios };
  }

  private buildFilterSection(): HTMLElement {
    const sec = h('section', { className: 'listen-section', 'aria-labelledby': 'listen-filter-h' }, h('h3', { id: 'listen-filter-h' }, 'Filter'));
    this.baselineSel = h('select', { id: 'listen-baseline' });
    this.baselineSel.addEventListener('change', () => {
      this.curveInput.hidden = this.baselineSel.value !== 'curve:import';
      if (this.baselineSel.value === 'curve:import' && !this.curve) this.curveInput.click();
      else if (this.report) this.requestDesign();
    });
    this.curveInput = h('input', { type: 'file', id: 'listen-curve', accept: '.frd,.txt,.csv,.zma,.json', hidden: true, 'aria-label': 'Measured curve file (FRD, REW text, CSV)' });
    this.curveInput.addEventListener('change', () => void this.importCurve());
    const mode = this.radioGroup('mode', 'Mode', [['difference', 'Difference (candidate ÷ baseline)'], ['absolute', 'Absolute (diagnostic)']], 'difference', () => this.onModeChange());
    this.modeRadios = mode.radios;
    const phase = this.radioGroup('phase', 'Phase', [['auto', 'Automatic (Section 16 decision)'], ['minimum', 'Minimum'], ['mixed', 'Mixed (model phase)'], ['linear', 'Linear (diagnostic)']], 'auto', () => this.report && this.requestDesign());
    this.phaseRadios = phase.radios;
    this.designBtn = h('button', { type: 'button', className: 'primary', textContent: 'Design filter' });
    this.designBtn.addEventListener('click', () => this.requestDesign());
    this.cancelBtn = h('button', { type: 'button', textContent: 'Cancel', disabled: true });
    this.cancelBtn.addEventListener('click', () => this.cancel());
    this.followBox = h('input', { type: 'checkbox', id: 'listen-follow', checked: true });
    this.status = h('p', { className: 'listen-status', role: 'status', 'aria-live': 'polite' });
    this.progress = h('div', { className: 'listen-busy', hidden: true, role: 'progressbar', 'aria-label': 'Designing the filter' });
    const plots = h('div', { className: 'plots listen-plots' });
    this.panel = new PlotPanel(plots, {
      onCursor: (i) => this.renderReadout(i),
      onView: () => undefined,
    });
    this.readoutEl = h('div', { className: 'readout listen-readout', 'aria-live': 'off' });
    this.legendEl = h('ul', { className: 'listen-legend', 'aria-label': 'Curves' });
    this.tableWrap = h('div');
    this.reportEl = h('div', { className: 'listen-report' });
    sec.append(
      h('div', { className: 'listen-row' },
        h('label', { for: 'listen-baseline' }, 'Baseline'), this.baselineSel, this.curveInput),
      mode.fs,
      phase.fs,
      h('div', { className: 'listen-row' }, this.designBtn, this.cancelBtn,
        h('span', { className: 'check' }, this.followBox, h('label', { for: 'listen-follow' }, 'Redesign when the design changes'))),
      this.progress,
      this.status,
      this.reportEl,
      this.legendEl,
      plots,
      this.readoutEl,
      this.tableWrap,
    );
    return sec;
  }

  private buildPlaybackSection(): HTMLElement {
    const sec = h('section', { className: 'listen-section', 'aria-labelledby': 'listen-play-h' }, h('h3', { id: 'listen-play-h' }, 'Playback'));
    this.limitText = h('span', {}, this.limitSentence());
    const warn = h('div', { className: 'listen-warning', role: 'note' },
      h('strong', {}, 'Level: '),
      'start with your headphone volume low. ', this.limitText,
      ' What reaches your ears depends on your headphones and their volume setting; a filter can boost some frequencies by more than 10 dB.');
    this.absoluteNote = h('p', { className: 'listen-absolute', role: 'note', hidden: true },
      h('strong', {}, 'Absolute diagnostic: '),
      'A is the candidate’s own response, uncompensated, not a difference between designs. Your headphones multiply it, so it is not what the design sounds like.');
    this.programmeSel = h('select', { id: 'listen-programme' });
    for (const [v, label] of PROGRAMMES) this.programmeSel.append(h('option', { value: v, textContent: label }));
    this.programmeSel.addEventListener('change', () => {
      this.fileInput.hidden = this.programmeSel.value !== 'file';
      if (this.programmeSel.value === 'file' && !this.fileProgramme) this.fileInput.click();
      void this.onProgrammeChange();
    });
    this.seedInput = h('input', { type: 'number', id: 'listen-seed', min: '0', max: '4294967295', step: '1', value: '1', className: 'listen-num' });
    this.seedInput.addEventListener('change', () => void this.onProgrammeChange());
    this.fileInput = h('input', { type: 'file', id: 'listen-file', accept: 'audio/*', hidden: true, 'aria-label': 'Audio file for the programme' });
    this.fileInput.addEventListener('change', () => void this.loadFile());
    this.programmeNotes = h('p', { className: 'hint listen-programme-notes' });
    const match = this.radioGroup('match', 'Level match', MATCH.map(([v, l]) => [v, l]), 'bs1770', () => void this.onLevelChange());
    this.matchRadios = match.radios;
    this.matchActive = h('p', { className: 'listen-match-active', 'aria-live': 'polite' });
    this.playBtn = h('button', { type: 'button', className: 'primary listen-play', textContent: 'Play' });
    this.playBtn.addEventListener('click', () => void (this.player.running ? this.stop() : this.play()));
    const ab = this.radioGroup('ab', 'Listen to', [['a', 'A: through the filter'], ['b', 'B: reference (programme alone)']], 'a', (v) => this.player.select(v === 'b' ? 1 : 0));
    this.abRadios = ab.radios;
    ab.fs.classList.add('listen-ab');
    this.volume = h('input', { type: 'range', id: 'listen-volume', min: String(VOLUME_RANGE_DB[0]), max: String(VOLUME_RANGE_DB[1]), step: '1', value: String(START_VOLUME_DB), 'aria-valuetext': `${START_VOLUME_DB} dB` });
    this.volumeNum = h('input', { type: 'number', id: 'listen-volume-num', min: String(VOLUME_RANGE_DB[0]), max: String(VOLUME_RANGE_DB[1]), step: '1', value: String(START_VOLUME_DB), className: 'listen-num', 'aria-label': 'Volume, dB' });
    const setVol = (v: number) => {
      // An empty or invalid entry (which Number() reads as 0 dB, the
      // loudest setting) leaves the volume where it was.
      if (!Number.isFinite(v)) {
        this.volumeNum.value = this.volume.value;
        return;
      }
      const c = Math.min(VOLUME_RANGE_DB[1], Math.max(VOLUME_RANGE_DB[0], Math.round(v)));
      this.volume.value = String(c);
      this.volumeNum.value = String(c);
      this.volume.setAttribute('aria-valuetext', `${c} dB`);
      this.player.setVolumeDb(c);
    };
    this.volume.addEventListener('input', () => setVol(Number(this.volume.value)));
    this.volumeNum.addEventListener('change', () => setVol(this.volumeNum.value.trim() === '' ? NaN : Number(this.volumeNum.value)));
    const meter = (key: 'momentary' | 'shortTerm' | 'truePeak' | 'gainReduction', label: string, min: number, max: number) => {
      const m = h('meter', { min, max, value: min, 'aria-label': label });
      const t = h('span', { className: 'listen-meter-value', textContent: '—' });
      this.meterEls[key] = { meter: m, text: t };
      return h('div', { className: 'listen-meter' }, h('span', { className: 'listen-meter-label' }, label), m, t);
    };
    this.meterEls = {} as typeof this.meterEls;
    const meters = h('div', { className: 'listen-meters', role: 'group', 'aria-label': 'Output meters' },
      meter('momentary', 'Momentary loudness (400 ms)', -60, 0),
      meter('shortTerm', 'Short-term loudness (3 s)', -60, 0),
      meter('truePeak', 'True peak (3 s hold)', -60, 0),
      meter('gainReduction', 'Limiter gain reduction', 0, 20));
    this.playingEl = h('p', { className: 'listen-playing', 'aria-live': 'polite' });
    this.levelEl = h('div', { className: 'listen-levels' });
    sec.append(
      warn,
      this.absoluteNote,
      h('div', { className: 'listen-row' },
        h('label', { for: 'listen-programme' }, 'Programme'), this.programmeSel,
        h('label', { for: 'listen-seed' }, 'Seed'), this.seedInput, this.fileInput),
      this.programmeNotes,
      match.fs,
      this.matchActive,
      h('div', { className: 'listen-row listen-transport' }, this.playBtn, ab.fs),
      h('div', { className: 'listen-row' }, h('label', { for: 'listen-volume' }, 'Volume'), this.volume, this.volumeNum, h('span', {}, 'dB')),
      this.playingEl,
      meters,
      this.levelEl,
    );
    this.renderMatchActive();
    return sec;
  }

  private buildDiagnostics(): HTMLElement {
    this.diagEl = h('div');
    this.stateText = h('textarea', { id: 'listen-state', readOnly: true, rows: 8, className: 'listen-state', 'aria-label': 'Audition state (JSON)' });
    const copy = h('button', { type: 'button', textContent: 'Copy state' });
    copy.addEventListener('click', () => {
      void navigator.clipboard?.writeText(this.stateText.value).then(
        () => this.host.announce('Audition state copied.'),
        () => this.host.announce('The clipboard is not available; select the text instead.'),
      );
    });
    const fallback = h('input', { type: 'checkbox', id: 'listen-fallback' });
    fallback.addEventListener('change', () => {
      this.player.forceFallback = fallback.checked;
      if (this.player.running) this.stop();
      this.playToken++;
      if (this.player.ctx) void this.player.close().then(() => this.renderDiagnostics());
      else this.renderDiagnostics();
    });
    return h('section', { className: 'listen-section' },
      h('details', { className: 'listen-details' },
        h('summary', {}, 'Diagnostics'),
        this.diagEl,
        h('span', { className: 'check' }, fallback, h('label', { for: 'listen-fallback' }, 'Use the ConvolverNode fallback (takes effect at the next Play)'))),
      h('details', { className: 'listen-details' },
        h('summary', {}, 'Audition state'),
        h('p', { className: 'hint' }, 'Everything needed to reproduce what you hear: the designs by the SHA-256 of their netlists, the phase mode, the level match and the programme.'),
        this.stateText,
        copy));
  }

  // ----- baselines -----------------------------------------------------------

  private async loadTargets(): Promise<void> {
    const r = await this.host.call('targets_list');
    if (!r.ok) return;
    const v = r.value as { targets?: { name: string; label: string; primary?: boolean }[] };
    this.targets = (v.targets ?? []).filter((t) => t.primary || t.name === 'ravizza2023_5128').map((t) => ({ name: t.name, label: t.label }));
    this.fillBaselines();
  }

  private baselineChoices(): BaselineChoice[] {
    const out: BaselineChoice[] = [];
    const cur = this.host.current();
    for (const b of this.host.baselines()) {
      out.push({ id: `frozen:${b.id}`, label: `Frozen baseline “${b.name}”`, spec: () => ({ kind: 'netlist', netlist: b.text, label: b.name }) });
    }
    const ref = this.host.reference();
    if (ref && cur) {
      out.push({
        id: 'template',
        label: 'Template values (the current netlist with the loaded template’s parameters)',
        spec: () => ({ kind: 'netlist', netlist: this.host.current()?.text ?? cur.text, overrides: Object.fromEntries(ref), label: 'template values' }),
      });
    }
    for (const t of this.targets) out.push({ id: `target:${t.name}`, label: `Target: ${t.label}`, spec: () => ({ kind: 'target', target: t.name }) });
    out.push({
      id: 'curve:import',
      label: this.curve ? `Measured curve: ${this.curve.label}` : 'Measured curve (import a file)…',
      spec: () => (this.curve ? { kind: 'curve', curve: this.curve.doc, label: this.curve.label } : null),
    });
    return out;
  }

  private fillBaselines(): void {
    const sel = this.baselineSel;
    const prev = sel.value;
    const choices = this.baselineChoices();
    const ids = choices.map((c) => c.id).join('|');
    if (sel.dataset.ids !== ids || [...sel.options].some((o, i) => o.textContent !== choices[i]?.label)) {
      sel.replaceChildren(...choices.map((c) => h('option', { value: c.id, textContent: c.label })));
      sel.dataset.ids = ids;
    }
    const delta = this.host.deltaReference();
    if (choices.some((c) => c.id === prev)) sel.value = prev;
    else if (delta !== null && choices.some((c) => c.id === `frozen:${delta}`)) sel.value = `frozen:${delta}`;
    else if (choices.some((c) => c.id === 'template')) sel.value = 'template';
    else if (choices.length) sel.value = choices[0].id;
    this.curveInput.hidden = sel.value !== 'curve:import';
  }

  private async importCurve(): Promise<void> {
    const f = this.curveInput.files?.[0];
    if (!f) return;
    const text = await f.text();
    const r = await this.host.call('import_curve', text, 'auto');
    const v = r.ok ? (r.value as { error?: string } & Record<string, unknown>) : { error: r.crash };
    if ('error' in v && v.error) {
      this.setStatus(`The curve could not be read: ${v.error}`, true);
      return;
    }
    this.curve = { doc: v, label: f.name };
    this.fillBaselines();
    this.baselineSel.value = 'curve:import';
    this.setStatus(`Imported ${f.name}.`);
    if (this.report) this.requestDesign();
  }

  private onModeChange(): void {
    const absolute = this.modeRadios.find((r) => r.checked)?.value === 'absolute';
    this.baselineSel.disabled = absolute;
    if (this.report) this.requestDesign();
  }

  // ----- filter design --------------------------------------------------------

  private request(): { cand: string; base: string; opts: string; label: string } | { error: string } {
    const cur = this.host.current();
    if (!cur) return { error: 'Run a netlist first.' };
    const mode = this.modeRadios.find((r) => r.checked)?.value ?? 'difference';
    const phase = this.phaseRadios.find((r) => r.checked)?.value ?? 'auto';
    let base: Record<string, unknown> | null = { kind: 'none' };
    if (mode === 'difference') {
      const choice = this.baselineChoices().find((c) => c.id === this.baselineSel.value);
      base = choice?.spec() ?? null;
      if (!base) return { error: 'Choose a baseline (import a measured curve first).' };
    }
    return {
      cand: JSON.stringify({ netlist: cur.text }),
      base: JSON.stringify(base),
      opts: JSON.stringify({ mode, phase, fs_Hz: this.player.sampleRate }),
      label: typeof base.label === 'string' ? base.label : '',
    };
  }

  /** Designs the filter now, or after the design in flight (only the newest request is kept). */
  private requestDesign(): void {
    const req = this.request();
    if ('error' in req) {
      this.setStatus(req.error, true);
      return;
    }
    const key = `${req.cand}\u0000${req.base}\u0000${req.opts}`;
    if (this.busy) {
      this.queued = key;
      return;
    }
    void this.design(key, req);
  }

  private async design(key: string, req: { cand: string; base: string; opts: string; label: string }): Promise<void> {
    this.busy = true;
    this.designBtn.disabled = true;
    this.cancelBtn.disabled = false;
    this.progress.hidden = false;
    this.setStatus('Designing the audition filter…');
    let reply;
    try {
      reply = await this.host.call('audition_filter', req.cand, req.base, req.opts);
    } catch {
      this.finishBusy();
      return;
    }
    this.finishBusy();
    if (!reply.ok) {
      this.setStatus(`The engine stopped: ${reply.crash}`, true);
    } else if ((reply.value as EngineError).error) {
      const e = reply.value as EngineError;
      this.setStatus(`${e.design ? `The ${e.design}: ` : ''}${e.error}`, true);
    } else {
      this.report = reply.value as FilterReport;
      this.reportKey = key;
      this.baselineLabel = req.label;
      this.synthesisMs = reply.ms;
      this.renderReport();
      this.setStatus(`Filter designed: ${this.report.n} taps at ${this.report.fs_Hz} Hz, ${this.report.phase.used} phase (${fmt(reply.ms, 0)} ms).`);
      this.host.announce(this.status.textContent ?? '');
      if (this.player.running) void this.applyFilters();
    }
    this.renderDiagnostics();
    const next = this.queued;
    this.queued = null;
    if (next && next !== this.reportKey) this.requestDesign();
  }

  private finishBusy(): void {
    this.busy = false;
    this.designBtn.disabled = !this.host.current();
    this.cancelBtn.disabled = true;
    this.progress.hidden = true;
  }

  private cancel(): void {
    this.host.cancel();
    this.queued = null;
    this.finishBusy();
    this.setStatus('Cancelled.');
  }

  private setStatus(text: string, error = false): void {
    this.status.textContent = text;
    this.status.classList.toggle('listen-error', error);
  }

  // ----- report ----------------------------------------------------------------

  private renderReport(): void {
    const r = this.report;
    if (!r) return;
    const dl = h('dl', { className: 'listen-facts' });
    const row = (k: string, ...v: (Node | string)[]) => dl.append(h('dt', {}, k), h('dd', {}, ...v));
    const baseName =
      r.baseline.kind === 'netlist' ? `design “${this.baselineLabel || 'baseline'}”`
      : r.baseline.kind === 'target' ? `target ${r.baseline.name}`
      : r.baseline.kind === 'curve' ? `curve ${r.baseline.label}`
      : 'none';
    row('Filter', r.mode === 'difference' ? `candidate ÷ ${baseName}, at ${r.candidate.probe}` : `the candidate’s own response at ${r.candidate.probe} (absolute)`);
    row('Phase', `${r.phase.used} (requested: ${r.phase.requested}). ${r.phase.reason}`);
    if (r.phase.hybrid_from_Hz) row('Hybrid', `minimum phase above ${hz(r.phase.hybrid_from_Hz)} (the validity frequency)`);
    if (r.phase.delay_removed_s) row('Delay removed', `${fmt(r.phase.delay_removed_s * 1e6, 1)} µs`);
    row('Length', `${r.n} taps at ${r.fs_Hz} Hz (${fmt((r.n / r.fs_Hz) * 1e3, 0)} ms); latency ${r.latency_samples} samples (${fmt(r.latency_s * 1e3, 2)} ms)`);
    if (r.ir_length) {
      const b = r.ir_length.binding;
      row('Decay (E46)', b
        ? `the slowest pole, ${hz(b.f_Hz)} with Q = ${fmt(b.q, 1)}, needs ${fmt(b.needed_s * 1e3, 1)} ms; the filter holds ${fmt(r.ir_length.half_length_s * 1e3, 1)} ms (${r.ir_length.covered ? 'covered' : `not covered: N = ${r.ir_length.recommended_n} would be needed`})`
        : 'no resonant pole in band');
    } else if (r.ir_length_error) row('Decay (E46)', `not checked: ${r.ir_length_error}`);
    if (r.check) {
      row('Check', `the taps differ from the analytic filter by at most ${fmt(r.check.max_abs_error_dB, 3)} dB (at ${hz(r.check.at_Hz)}) from ${hz(r.check.band_Hz[0])} to ${hz(r.check.band_Hz[1])}: ${r.check.met ? `within ${r.check.tolerance_dB} dB` : `beyond ${r.check.tolerance_dB} dB at ${r.check.exceeded_Hz.map(([a, b]) => (a === b ? hz(a) : `${hz(a)}–${hz(b)}`)).join(', ')}`}; resolution fs/N = ${fmt(r.check.resolution_Hz, 2)} Hz`);
    }
    row('Range', `boost up to ${signed(r.max_boost_dB.dB)} dB at ${hz(r.max_boost_dB.at_Hz)}, cut down to ${signed(r.max_cut_dB.dB)} dB at ${hz(r.max_cut_dB.at_Hz)} (re its 500 Hz–2 kHz level)`);
    row('Band', `exact from ${hz(r.band_Hz[0])} to ${hz(r.band_Hz[1])}, held outside`);
    if (r.inversion) {
      const i = r.inversion;
      row('Inversion', `Kirkeby–Nelson over ${hz(i.band_Hz[0])}–${hz(i.band_Hz[1])}, ${i.smoothing}-octave smoothing, boost cap ${i.boost_cap_dB} dB (largest boost ${fmt(i.max_boost_dB.dB, 1)} dB at ${hz(i.max_boost_dB.at_Hz)}), notches deeper than ${i.notch_limit_dB} dB not inverted${i.notches.length ? `: ${i.notches.map((n) => `${hz(n.at_Hz)} ${fmt(n.depth_dB, 1)} dB ${n.inverted ? 'inverted' : 'not inverted'}`).join('; ')}` : ''}; largest departure from the exact inverse ${fmt(i.max_departure_from_exact_dB.dB, 1)} dB at ${hz(i.max_departure_from_exact_dB.at_Hz)}`);
    }
    const flags = h('ul', { className: 'listen-flags' });
    for (const f of r.flags) flags.append(h('li', { 'data-code': f.code }, h('span', { className: 'visually-hidden' }, 'Notice: '), f.message));
    const notes = h('ul', { className: 'listen-notes' });
    for (const n of r.notes) notes.append(h('li', {}, n));
    this.reportEl.replaceChildren(dl, ...(r.flags.length ? [flags] : []), ...(r.notes.length ? [notes] : []));
    this.absoluteNote.hidden = r.mode !== 'absolute';
    this.renderPlot();
    this.renderState();
  }

  private renderPlot(): void {
    const r = this.report;
    if (!r) return;
    const tol = r.check?.tolerance_dB ?? 0.1;
    const filter: PlotGroup = {
      key: 'listen-filter',
      kind: 'delta',
      title: 'Audition filter',
      symbol: '|H|',
      unit: '',
      axisUnit: 'dB re 500 Hz–2 kHz',
      scale: 'linear',
      series: [
        { probe: 0, id: 'analytic', values: r.design_dB, primary: true },
        { probe: 1, id: 'taps', values: r.fir_dB },
      ],
      overlays: [],
      height: 'main',
    };
    const error: PlotGroup = {
      key: 'listen-error',
      kind: 'delta',
      title: 'Taps minus analytic',
      symbol: 'Δ',
      unit: '',
      axisUnit: 'dB',
      scale: 'linear',
      series: [
        { probe: 2, id: 'error', values: r.error_dB },
        { probe: 3, id: `+${tol} dB`, values: r.error_dB.map(() => tol) },
        { probe: 3, id: `−${tol} dB`, values: r.error_dB.map(() => -tol) },
      ],
      overlays: [],
      height: 'small',
    };
    this.panel.setGroups([filter, error], { freqs: r.frequencies_Hz, shading: r.candidate.shading });
    const key = (probe: number, text: string) => h('li', {}, lineKey(probe), text);
    this.legendEl.replaceChildren(
      key(0, 'analytic filter (the engine’s exact solves)'),
      key(1, 'taps (their own frequency response)'),
      key(2, 'taps minus analytic'),
      key(3, `±${tol} dB (spec Section 16)`),
    );
    this.renderTable();
    this.renderReadout(null);
  }

  private renderReadout(i: number | null): void {
    const r = this.report;
    if (!r || i === null) {
      this.readoutEl.textContent = r ? 'Point at a plot, or focus it and use the arrow keys, to read the filter.' : '';
      return;
    }
    this.readoutEl.textContent = `${hz(r.frequencies_Hz[i])}: analytic ${signed(r.design_dB[i], 2)} dB, taps ${signed(r.fir_dB[i], 2)} dB, difference ${signed(r.error_dB[i], 4)} dB`;
  }

  private renderTable(): void {
    const r = this.report;
    if (!r) return;
    const t = h('table', { className: 'data-table' });
    t.append(h('caption', {}, `Audition filter on its check grid (${r.frequencies_Hz.length} frequencies, 48 per octave)`));
    const head = h('tr');
    for (const c of ['Frequency', 'Analytic (dB)', 'Taps (dB)', 'Difference (dB)']) head.append(h('th', { scope: 'col', textContent: c }));
    t.append(h('thead', {}, head));
    const body = h('tbody');
    r.frequencies_Hz.forEach((f, i) => {
      body.append(h('tr', {},
        h('th', { scope: 'row', textContent: hz(f) }),
        h('td', { textContent: signed(r.design_dB[i], 3) }),
        h('td', { textContent: signed(r.fir_dB[i], 3) }),
        h('td', { textContent: signed(r.error_dB[i], 4) })));
    });
    t.append(body);
    const wrap = h('div', { className: 'table-wrap', tabIndex: 0, role: 'region', 'aria-label': 'Audition filter data' }, t);
    this.tableWrap.replaceChildren(h('details', { className: 'listen-details' }, h('summary', {}, 'Data table'), wrap));
  }

  // ----- playback ---------------------------------------------------------------

  private matchMethod(): MatchMethod {
    const chosen = (this.matchRadios.find((r) => r.checked)?.value ?? 'bs1770') as MatchMethod;
    // Pure tones fall outside the loudness algorithm's scope (Section 16).
    return chosen === 'bs1770' && this.programme?.matchBy === 'rms' ? 'rms' : chosen;
  }

  private renderMatchActive(): void {
    const m = this.matchMethod();
    const label = MATCH.find(([v]) => v === m)?.[1] ?? m;
    const forced = m === 'rms' && this.matchRadios.find((r) => r.checked)?.value === 'bs1770';
    this.matchActive.textContent = `Active level match: ${label}${forced ? ' (the sweep is a pure tone at every instant, outside the loudness algorithm’s scope)' : ''}; target ${m === 'rms' ? `${TARGET_RMS_DB} dB RMS` : `${TARGET_LUFS} LUFS`} before the volume control.`;
  }

  private async currentProgramme(fs: number): Promise<Programme | null> {
    const kind = this.programmeSel.value as ProgrammeKind | 'file';
    if (kind === 'file') {
      if (!this.fileProgramme) return null;
      if (this.fileProgramme.fs !== fs) await this.loadFile();
      return this.fileProgramme;
    }
    const seed = Math.max(0, Math.min(4294967295, Math.trunc(Number(this.seedInput.value) || 0)));
    const p = this.programme;
    if (p && p.kind === kind && p.seed === (kind === 'sweep' ? null : seed) && p.fs === fs) return p;
    return generate(kind, seed, fs);
  }

  private async loadFile(): Promise<void> {
    const f = this.fileInput.files?.[0];
    if (!f) return;
    const bytes = await f.arrayBuffer();
    const sha = await sha256Hex(bytes);
    const fs = this.player.sampleRate;
    try {
      const decoder = new OfflineAudioContext(2, 1, fs);
      const buf = await decoder.decodeAudioData(bytes.slice(0));
      const cap = Math.min(buf.length, Math.round(FILE_CAP_S * fs));
      const ch = [0, 1].map((c) => buf.getChannelData(Math.min(c, buf.numberOfChannels - 1)).slice(0, cap));
      // A float file can hold NaN or infinite samples; they would reach the output.
      if (!ch.every((x) => x.every(Number.isFinite))) throw new Error('it holds samples that are not finite numbers');
      this.fileProgramme = {
        kind: 'file',
        label: f.name,
        fs,
        channels: ch,
        seed: null,
        sha256: sha,
        matchBy: 'bs1770',
        notes: [`${f.name}: ${buf.numberOfChannels === 1 ? 'mono, played in both ears' : `${buf.numberOfChannels} channels, the first two played`}, decoded at ${fs} Hz${buf.length > cap ? `, cut to its first ${FILE_CAP_S} s` : ''}.`],
      };
      this.programmeNotes.textContent = this.fileProgramme.notes.join(' ');
      if (this.programmeSel.value === 'file') await this.onProgrammeChange();
    } catch (e) {
      this.fileProgramme = null;
      this.programmeNotes.textContent = `The browser could not decode ${f.name}, or it cannot be played: ${String(e)}`;
    }
  }

  private async onProgrammeChange(): Promise<void> {
    const p = await this.currentProgramme(this.player.sampleRate);
    if (!p) return;
    this.programme = p;
    this.programmeNotes.textContent = p.notes.join(' ');
    this.renderMatchActive();
    if (this.player.running) {
      // The new programme is silenced until its own level match is in
      // place: through the previous programme's gains it could be far
      // louder (a quiet file boosted by tens of dB, then white noise).
      const token = this.playToken;
      this.player.mute();
      this.player.setProgramme(p);
      if ((await this.applyFilters()) && token === this.playToken) this.player.unmute();
    }
    this.renderState();
  }

  private async onLevelChange(): Promise<void> {
    this.renderMatchActive();
    if (this.player.running) await this.applyFilters();
  }

  private async play(): Promise<void> {
    const token = ++this.playToken;
    const cancelled = () => token !== this.playToken;
    this.playBtn.disabled = true;
    try {
      await this.player.open();
    } catch (e) {
      this.playingEl.textContent = `Audio could not start: ${String(e)}`;
      this.playBtn.disabled = false;
      return;
    }
    if (cancelled()) return this.abandonPlay();
    // Nothing is let through until the filters and gains below are in place.
    this.player.mute();
    const fs = this.player.sampleRate;
    // Filters are designed for the context's actual rate.
    if (!this.report || this.report.fs_Hz !== fs) {
      this.requestDesign();
      await this.waitForDesign();
      if (cancelled()) return this.abandonPlay();
    }
    const p = await this.currentProgramme(fs);
    if (cancelled()) return this.abandonPlay();
    if (!p || !this.report) {
      this.playingEl.textContent = p ? 'No filter to play.' : 'Choose an audio file first.';
      this.playBtn.disabled = false;
      return;
    }
    this.programme = p;
    this.programmeNotes.textContent = p.notes.join(' ');
    this.player.setProgramme(p);
    const ok = await this.applyFilters();
    if (cancelled()) return this.abandonPlay();
    if (!ok) {
      this.playBtn.disabled = false;
      return;
    }
    this.player.select(this.abRadios[1].checked ? 1 : 0);
    this.player.play();
    this.playBtn.textContent = 'Stop';
    this.playBtn.classList.remove('primary');
    this.playBtn.disabled = false;
    this.pollTimer = window.setInterval(() => this.renderMeters(this.player.poll()), 100);
    this.playingEl.textContent = `Playing ${p.label}.`;
    this.renderDiagnostics();
    this.renderState();
  }

  /** A Play given up (Stop, leaving the view, switching the audio path) leaves the input muted. */
  private abandonPlay(): void {
    this.player.mute();
    this.playBtn.disabled = false;
  }

  private waitForDesign(): Promise<void> {
    return new Promise((resolve) => {
      const tick = () => (this.busy || this.queued ? setTimeout(tick, 50) : resolve());
      tick();
    });
  }

  private stop(): void {
    this.playToken++;
    this.player.stop();
    window.clearInterval(this.pollTimer);
    this.playBtn.textContent = 'Play';
    this.playBtn.classList.add('primary');
    this.playingEl.textContent = 'Stopped.';
    this.renderMeters(null);
    this.renderDiagnostics();
  }

  /**
   * Level-matches A and B over the programme and loads both into the
   * player. On a failure while playing, playback stops (with the reason
   * shown) rather than go on with gains that do not belong to what plays.
   */
  private async applyFilters(): Promise<boolean> {
    const r = this.report;
    const p = this.programme;
    if (!r || !p) return false;
    if (r.fs_Hz !== p.fs) return false;
    const fail = (why: string) => {
      if (this.player.running) this.stop();
      this.playingEl.textContent = why;
      return false;
    };
    const a = Float32Array.from(r.taps);
    if (!a.length || !a.every(Number.isFinite)) return fail('The filter holds values that are not finite numbers; nothing is played.');
    const b = delayTaps(r.latency_samples);
    const method = this.matchMethod();
    this.renderMatchActive();
    this.playingEl.textContent = 'Matching levels over the programme…';
    let m;
    try {
      m = await this.matcher.measure(`${p.kind}|${p.seed}|${p.fs}|${p.sha256}|${p.channels[0].length}`, p, [
        { taps: [a] },
        { key: `delay:${r.latency_samples}`, taps: [b] },
      ]);
    } catch (e) {
      return fail(`Level match failed: ${String(e)}`);
    }
    const gainA = matchGainDb(m.filters[0], method, m.programme);
    const gainB = matchGainDb(m.filters[1], method, m.programme);
    if (!Number.isFinite(gainA) || !Number.isFinite(gainB)) {
      return fail('The programme is silent: it has no level to match (BS.1770 gates out everything below −70 LKFS), so nothing is played.');
    }
    if (Math.max(gainA, gainB) > MAX_GAIN_DB) {
      return fail(`The programme is too quiet to match: it would need ${signed(Math.max(gainA, gainB), 1)} dB of gain, more than the +${MAX_GAIN_DB} dB allowed. Nothing is played.`);
    }
    this.levels = { method, programme: m.programme, a: m.filters[0], b: m.filters[1], gainA, gainB, ms: m.ms };
    if (!this.player.loadFilter(0, a, a, gainA) || !this.player.loadFilter(1, b, b, gainB)) {
      return fail('The filters could not be loaded.');
    }
    this.renderLevels();
    this.playingEl.textContent = `Playing ${p.label}.`;
    this.renderState();
    this.renderDiagnostics();
    return true;
  }

  private renderLevels(): void {
    const l = this.levels;
    if (!l) {
      this.levelEl.replaceChildren();
      return;
    }
    const t = h('table', { className: 'data-table listen-level-table' });
    t.append(h('caption', {}, `Level match (${MATCH.find(([v]) => v === l.method)?.[1]}), measured over the programme after convolution in ${fmt(l.ms, 0)} ms`));
    const head = h('tr');
    for (const c of ['', 'Loudness (LUFS)', 'RMS (dB)', 'True peak (dBTP)', 'Gain (dB)', 'Matched loudness (LUFS)']) head.append(h('th', { scope: 'col', textContent: c }));
    t.append(h('thead', {}, head));
    const body = h('tbody');
    for (const [name, m, g] of [['A (filtered)', l.a, l.gainA], ['B (reference)', l.b, l.gainB]] as [string, Measurement, number][]) {
      body.append(h('tr', {},
        h('th', { scope: 'row', textContent: name }),
        h('td', { textContent: fmt(m.integratedLufs, 2) }),
        h('td', { textContent: fmt(m.rmsDb, 2) }),
        h('td', { textContent: fmt(m.truePeakDbtp, 2) }),
        h('td', { textContent: signed(g, 2) }),
        h('td', { textContent: fmt(m.integratedLufs + g, 2) })));
    }
    t.append(body);
    this.levelEl.replaceChildren(h('div', { className: 'table-wrap', tabIndex: 0, role: 'region', 'aria-label': 'Level match' }, t));
  }

  private renderMeters(m: Meters | null): void {
    const set = (k: keyof typeof this.meterEls, v: number, text: string) => {
      const e = this.meterEls[k];
      e.meter.value = Number.isFinite(v) ? Math.max(e.meter.min, Math.min(e.meter.max, v)) : e.meter.min;
      e.text.textContent = text;
    };
    if (!m) {
      for (const k of Object.keys(this.meterEls) as (keyof typeof this.meterEls)[]) set(k, -Infinity, '—');
      return;
    }
    set('momentary', m.momentaryLufs, `${fmt(m.momentaryLufs)} LUFS`);
    set('shortTerm', m.shortTermLufs, `${fmt(m.shortTermLufs)} LUFS`);
    set('truePeak', m.truePeakDbtp, `${fmt(m.truePeakDbtp)} dBTP`);
    set('gainReduction', m.gainReductionDb, `${fmt(m.gainReductionDb)} dB`);
    const slot = m.slot === 1 ? 'B (reference)' : 'A (through the filter)';
    const text = `Playing ${slot}${m.loading ? '; switching filters' : ''}.`;
    if (this.playingEl.textContent !== text) this.playingEl.textContent = text;
  }

  // ----- diagnostics and state --------------------------------------------------

  /** What bounds the output on the path in use (or about to be used). */
  private limitSentence(): string {
    const d = this.player.diagnostics();
    const fallback = d.path === 'convolver' || (d.path === null && (this.player.forceFallback || !d.audioWorklet));
    return fallback
      ? `On this path (the ConvolverNode fallback) the output is hard-clipped at ${CEILING_DBTP} dBFS (sample peak); there is no true-peak limiter.`
      : `The output is limited to ${CEILING_DBTP} dBTP (true peak).`;
  }

  private renderDiagnostics(): void {
    if (this.limitText) this.limitText.textContent = this.limitSentence();
    const d: Diagnostics = this.player.diagnostics();
    const rows: [string, string][] = [
      ['Context sample rate', d.sampleRate === null ? `not started (${d.requestedRate} Hz will be requested)` : `${d.sampleRate} Hz${d.rateRefused ? ` (the device refused ${d.requestedRate} Hz; filters are designed for ${d.sampleRate} Hz)` : ''}`],
      ['Base latency', d.baseLatency === null ? 'not exposed' : `${fmt(d.baseLatency * 1e3, 2)} ms`],
      ['Output latency', d.outputLatency === null ? 'not exposed' : `${fmt(d.outputLatency * 1e3, 2)} ms`],
      ['Render quantum', `${d.renderQuantum} frames`],
      ['Cross-origin isolated', d.crossOriginIsolated ? 'yes' : 'no'],
      ['AudioWorklet', d.audioWorklet ? 'available' : 'not available'],
      ['Audio path', d.path === null ? '—' : d.path === 'worklet' ? 'AudioWorklet: partitioned convolution (block 128, FFT 256) and true-peak limiter' : 'ConvolverNode fallback (normalize = false); a hard clip at the ceiling (sample peak) stands in for the true-peak limiter'],
      ['Convolution arithmetic', 'JavaScript, double precision (not WebAssembly SIMD)'],
      ['WebAssembly SIMD', d.wasmSimd ? 'supported by this browser' : 'not supported'],
      ['Filter synthesis', this.synthesisMs === null ? '—' : `${fmt(this.synthesisMs, 0)} ms in the engine worker`],
      ['Level match', this.levels ? `${fmt(this.levels.ms, 0)} ms` : '—'],
      ['Audio state', d.state],
    ];
    const t = h('table', { className: 'data-table' });
    t.append(h('caption', {}, 'Audio diagnostics'));
    const body = h('tbody');
    for (const [k, v] of rows) body.append(h('tr', {}, h('th', { scope: 'row', textContent: k }), h('td', { textContent: v })));
    t.append(body);
    this.diagEl.replaceChildren(t);
  }

  private renderState(): void {
    const r = this.report;
    if (!r) {
      this.stateText.value = '';
      return;
    }
    const p = this.programme;
    const state = {
      ...r.state,
      level_match: this.levels
        ? { method: this.levels.method, target: this.levels.method === 'rms' ? `${TARGET_RMS_DB} dB RMS` : `${TARGET_LUFS} LUFS`, gain_A_dB: this.levels.gainA, gain_B_dB: this.levels.gainB, measured_A_LUFS: this.levels.a.integratedLufs, measured_B_LUFS: this.levels.b.integratedLufs }
        : null,
      programme: p ? { kind: p.kind, label: p.label, seed: p.seed, sha256: p.sha256, fs_Hz: p.fs, length_samples: p.channels[0].length } : null,
      playback: { volume_dB: Number(this.volume.value), limiter_ceiling_dBTP: CEILING_DBTP, listening_to: this.abRadios[1].checked ? 'B' : 'A' },
    };
    this.stateText.value = JSON.stringify(state, null, 2);
  }
}

export const view = new ListenView();
