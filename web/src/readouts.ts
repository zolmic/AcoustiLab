// Readouts of the current design in the Response tab (spec Section 10,
// "Automated readouts"): the engine's `readouts` export (docs/analysis.md)
// for the text the plotted result was solved from, and, when a baseline is
// the Δ reference, the change of each readout against that baseline's
// readouts (computed from its netlist text).
//
// The block has its own engine worker, so it never delays the live solve,
// and its requests are coalesced like the solves: at most one call is in
// flight, and while it runs only the newest text is kept. A drag therefore
// never queues readouts. Until the readouts of the plotted text arrive, the
// previous numbers stay, marked as updating (never shown as current).
//
// The block sits above the plots and arrives after them, so its height
// must not depend on what the engine returns: the same cells in the same
// order every time (a dash where a design has no such readout), a status of
// one line (two below 600 px), a flag line in the readouts that can carry
// a flag (empty or not), a Δ line whenever a baseline is the reference, an
// extra line of at most two lines per cell (clamped on screen; the whole
// text stays in the page, in a tooltip and under "All readouts in full"),
// and the engine's notes behind a disclosure. The plots then never move
// when readouts land.
//
// Numbers only: each readout names its method (a short name here, the
// engine's method text under "Probes and methods"); the engine's notes say
// why a value is missing. Explanations of causes belong to the Sensitivity
// view.

import { EngineWorker } from './engine';
import { formatHz, formatParam } from './format';
import { store } from './store';
import { isError } from './types';
import { el, exposeForTests, signed, signedSig } from './views/analysis-ui';
import type { Readouts } from './views/analysis-types';

type Entry = { ok: true; doc: Readouts } | { ok: false; message: string };

interface Value {
  /** Main value text. */
  text: string;
  /** The numbers the Δ line compares, in the order the text shows them. */
  nums: (number | null)[];
  /** Qualifiers and references, one short line. */
  extra?: string[];
  /** A state that makes the value unusable or suspect ("ambiguous", "fails"). */
  flag?: string;
  /** Units of `nums` when they are not the cell's (another quantity). */
  units?: string[];
  /**
   * Numbers of another quantity the Δ line falls back to when the two
   * designs' `nums` are different quantities (the sensitivity in dB/V when
   * one of them has no rated impedance, so no dB/mW), with their units.
   */
  alt?: { nums: (number | null)[]; units: string[] };
}

interface Cell {
  key: string;
  label: string;
  /** Short name of the method; the engine's `methods` entry is `methodKey`. */
  method: string;
  methodKey: string;
  /** Units of `nums`, for the Δ line. */
  units: string[];
  /**
   * The readout can carry a flag ("ambiguous", "indicative", "fails"): it
   * gets a line of its own, kept (empty) when there is none, so the flag is
   * never cut and the block's height does not change with it.
   */
  flags?: true;
  /** The readout of a design, or null when the design has none. */
  value(d: Readouts): Value | null;
}

const hz = (f: number) => formatHz(f);
const ohm = (z: number) => `${formatParam(z, 4)} Ω`;
const q3 = (x: number) => formatParam(x, 3);
const shadeText = (s: number) => (s === 2 ? 'dark validity band' : s === 1 ? 'light validity band' : '');

const CELLS: Cell[] = [
  {
    key: 'coupled_resonance',
    label: 'Coupled resonance',
    method: 'max |v/i|',
    methodKey: 'coupled_resonance',
    units: ['Hz'],
    flags: true,
    value: (d) => {
      const c = d.response?.coupled_resonance;
      if (!c) return null;
      const extra: string[] = [];
      let flag: string | undefined;
      if (c.ambiguous) flag = 'ambiguous';
      else if (!c.robust) flag = 'not robust';
      if (c.competing) extra.push(`competing peak ${hz(c.competing.f_Hz)}, ${c.competing.margin_dB.toFixed(2)} dB lower`);
      if (c.shading) extra.push(shadeText(c.shading));
      return { text: hz(c.f_Hz), nums: [c.ambiguous ? null : c.f_Hz], extra, flag };
    },
  },
  {
    key: 'z_resonance',
    label: 'In-situ |Z| peak',
    method: 'first |Z| peak',
    methodKey: 'resonance',
    units: ['Hz', 'Ω'],
    value: (d) => {
      const z = d.impedance;
      if (!z?.resonance) return null;
      const extra: string[] = [];
      if (z.peaks.length > 1 && z.z_max) extra.push(`${z.peaks.length} peaks; highest ${hz(z.z_max.f_Hz)}, ${ohm(z.z_max.z_ohm)}`);
      return { text: `${hz(z.resonance.f_Hz)}, ${ohm(z.resonance.z_ohm)}`, nums: [z.resonance.f_Hz, z.resonance.z_ohm], extra };
    },
  },
  {
    key: 'q',
    label: 'Qms · Qes · Qts (in situ)',
    method: '√r0 (Small 1972)',
    methodKey: 'Q',
    units: ['', '', ''],
    flags: true,
    value: (d) => {
      const z = d.impedance;
      if (!z) return null;
      if (!z.q) return { text: 'not estimated', nums: [null, null, null], flag: 'no Q', extra: ['the engine notes say why'] };
      const text = `${q3(z.q.Qms)} · ${q3(z.q.Qes)} · ${q3(z.q.Qts)}`;
      const nums = [z.q.Qms, z.q.Qes, z.q.Qts];
      // The method assumes an isolated resonance; the engine notes when it
      // is not (several |Z| peaks, or √(f1·f2) off the peak), and calls the
      // values indicative.
      const caveats = [
        ...(z.peaks.length > 1 ? [`of the first of ${z.peaks.length} |Z| peaks (${hz(z.q.f_Hz)})`] : []),
        ...(z.q.notes.length ? ['not a single lumped resonance'] : []),
      ];
      if (!caveats.length) return { text, nums };
      return { text, nums, flag: 'indicative', extra: [`${caveats.join('; ')}; see the engine notes`] };
    },
  },
  {
    key: 'z_1khz',
    label: '|Z| at 1 kHz',
    method: 'exact solve',
    methodKey: 'z_1kHz',
    units: ['Ω'],
    value: (d) => {
      const v = d.impedance?.z_1kHz_ohm;
      return v === null || v === undefined ? null : { text: ohm(v), nums: [v] };
    },
  },
  {
    key: 'z_min',
    label: 'Minimum |Z|, rated check',
    method: '≥ 80 % of rated',
    methodKey: 'rated_check',
    units: ['Ω'],
    flags: true,
    value: (d) => {
      const c = d.impedance?.rated_check;
      if (!c) return null;
      const extra = [
        c.pass
          ? `rated ${ohm(c.rated_ohm)}: pass (limit ${ohm(c.limit_ohm)})`
          : `rated ${ohm(c.rated_ohm)}: below the ${ohm(c.limit_ohm)} limit at ${c.violations.map((v) => `${hz(v.f_min_Hz)}–${hz(v.f_max_Hz)}`).join(', ')}`,
      ];
      return { text: `${ohm(c.z_min_ohm)} at ${hz(c.z_min_Hz)}`, nums: [c.z_min_ohm], extra, flag: c.pass ? undefined : 'fails' };
    },
  },
  {
    key: 'sensitivity',
    label: 'Sensitivity, 500 Hz · 1 kHz',
    method: 'dB/mW into rated Z',
    methodKey: 'sensitivity',
    units: ['dB', 'dB'],
    value: (d) => {
      const at = (f: number) => d.response?.sensitivity.find((x) => Math.abs(x.f_Hz - f) < 1e-9);
      const a = at(500);
      const b = at(1000);
      if (!a || !b) return null;
      const perV = `${a.dB_per_V.toFixed(1)} · ${b.dB_per_V.toFixed(1)} dB/V`;
      // The Δ compares like with like: dB/mW when both designs have a rated
      // impedance, else dB/V (a baseline under another drive may have none).
      const alt = { nums: [a.dB_per_V, b.dB_per_V], units: ['dB/V', 'dB/V'] };
      // dB/mW needs the rated impedance (erratum E32); without it, dB/V only.
      if (a.dB_per_mW === null || b.dB_per_mW === null) return { text: perV, nums: alt.nums, units: alt.units, extra: ['dB/V only: no rated impedance'], alt };
      return { text: `${a.dB_per_mW.toFixed(1)} · ${b.dB_per_mW.toFixed(1)} dB/mW`, nums: [a.dB_per_mW, b.dB_per_mW], extra: [perV], alt };
    },
  },
  {
    key: 'bass_extension',
    label: 'Bass extension (−3 dB)',
    method: 'grid walk, refined',
    methodKey: 'bass_extension',
    units: ['Hz'],
    value: (d) => {
      const r = d.response;
      if (!r) return null;
      const ref = `re ${r.level_500Hz_dB.toFixed(1)} dB SPL at 500 Hz`;
      const b = r.bass_extension;
      if (!b) return { text: 'beyond the sweep', nums: [null], extra: [ref] };
      const extra = [ref];
      if (b.shading) extra.push(shadeText(b.shading));
      return { text: hz(b.f_Hz), nums: [b.f_Hz], extra };
    },
  },
  {
    key: 'free_air_0',
    label: 'Free-air driver',
    method: '√r0 of unloaded |Z|',
    methodKey: 'free_air',
    units: ['Hz', ''],
    value: (d) => {
      const drv = d.drivers[0];
      if (!drv) return null;
      const more = d.drivers.length > 1 ? `; ${d.drivers.length - 1} more driver${d.drivers.length > 2 ? 's' : ''} in the engine’s document` : '';
      const fa = drv.free_air;
      if (!fa) return { text: 'not estimated', nums: [null, null], extra: [`“${drv.element}”; the engine notes say why${more}`] };
      return {
        text: `fs ${hz(fa.f_Hz)} · Qts ${q3(fa.Qts)}`,
        nums: [fa.f_Hz, fa.Qts],
        extra: [`“${drv.element}”: Qms ${q3(fa.Qms)} · Qes ${q3(fa.Qes)} · Re ${ohm(fa.Re_ohm)}${more}`],
      };
    },
  },
];

/** The Δ line of a cell: each number's change, in its unit. */
function deltaText(cur: (number | null)[], base: (number | null)[], units: string[]): string {
  const parts = cur.map((c, i) => {
    const b = base[i];
    if (c === null || b === null || b === undefined) return 'n/a';
    const u = units[i] ? ` ${units[i]}` : '';
    return `${units[i].startsWith('dB') ? signed(c - b, 2) : signedSig(c - b, 3)}${u}`;
  });
  return parts.every((p) => p === 'n/a') ? 'n/a' : parts.join(' · ');
}

/**
 * The Δ line of a cell between the current and the baseline value: of
 * `nums` when both are the same quantity, else of `alt` (never a dB/mW
 * against a dB/V).
 */
function cellDelta(c: Cell, v: Value, b: Value): string {
  const vu = v.units ?? c.units;
  const bu = b.units ?? c.units;
  if (vu.join('|') === bu.join('|')) return deltaText(v.nums, b.nums, vu);
  if (v.alt && b.alt && v.alt.units.join('|') === b.alt.units.join('|')) return deltaText(v.alt.nums, b.alt.nums, v.alt.units);
  return 'n/a';
}

export class ReadoutsBlock {
  private readonly worker = new EngineWorker();
  private readonly cache = new Map<string, Entry>();
  private inflight: string | null = null;
  private current: string | null = null;
  private baseline: { name: string; text: string } | null = null;
  /** Text of the readouts on screen. */
  private shownText: string | null = null;
  /** Readouts calls made (a test hook: requests are coalesced). */
  private calls = 0;

  private readonly status = el('p', 'ro-status');
  private readonly grid = el('dl', 'ro-grid');
  private readonly notesBox = el('details', 'ro-notes-box');
  private readonly notesSummary = el('summary');
  private readonly notes = el('ul', 'ro-notes');
  private readonly methods = el('details', 'ro-methods');
  /** Every cell's whole text, unclamped (the cells clamp theirs to keep the block's height). */
  private readonly full = el('details', 'ro-full');
  private readonly fullList = el('dl');
  private readonly methodList = el('dl');
  private readonly error = el('p', 'ro-error');

  constructor(private readonly root: HTMLElement) {
    const head = el('div', 'ro-head');
    const h = el('h3', undefined, 'Readouts');
    h.id = 'readouts-heading';
    root.setAttribute('aria-labelledby', h.id);
    // Open by default; a viewer who wants the plots higher can fold it (remembered).
    const body = el('div', 'ro-body');
    body.id = 'readouts-body';
    const toggle = el('button', 'ro-toggle');
    toggle.type = 'button';
    toggle.setAttribute('aria-controls', body.id);
    const setOpen = (open: boolean) => {
      body.hidden = !open;
      toggle.setAttribute('aria-expanded', String(open));
      toggle.textContent = open ? 'Hide' : 'Show';
      toggle.setAttribute('aria-label', `${open ? 'Hide' : 'Show'} the readouts`);
      store.set('readouts.open', open ? '1' : '0');
    };
    toggle.addEventListener('click', () => setOpen(body.hidden));
    setOpen(store.get('readouts.open') !== '0');
    head.append(h, this.status, toggle);
    this.notesBox.append(this.notesSummary, this.notes);
    this.methods.append(el('summary', undefined, 'Probes and methods (as the engine states them)'), this.methodList);
    this.full.append(el('summary', undefined, 'All readouts in full'), this.fullList);
    const more = el('div', 'ro-more');
    more.append(this.full, this.notesBox, this.methods);
    this.error.hidden = true;
    body.append(this.error, this.grid, more);
    root.append(head, body);
    this.render();
    exposeForTests('readouts', () => ({
      calls: this.calls,
      current: this.current,
      shown: this.shownText,
      busy: this.inflight !== null,
      doc: this.shownText ? this.cache.get(this.shownText) : null,
    }));
  }

  /** The plotted result's text and the Δ reference baseline changed. */
  show(text: string | null, baseline: { name: string; text: string } | null): void {
    this.current = text;
    this.baseline = baseline;
    this.pump();
    this.render();
  }

  /** Starts the next missing readout (the plotted text first), unless one is in flight. */
  private pump(): void {
    if (this.inflight !== null) return;
    const want = [this.current, this.baseline?.text].find((t): t is string => !!t && !this.cache.has(t));
    if (!want) return;
    this.inflight = want;
    this.calls++;
    this.root.setAttribute('aria-busy', 'true');
    void this.worker.invoke('readouts', want, '', '').then((reply) => {
      this.inflight = null;
      const entry: Entry = !reply.ok
        ? { ok: false, message: reply.crash }
        : isError(reply.value)
          ? { ok: false, message: reply.value.error }
          : { ok: true, doc: reply.value as Readouts };
      this.cache.set(want, entry);
      this.trim();
      this.pump();
      if (this.inflight === null) this.root.setAttribute('aria-busy', 'false');
      this.render();
    });
  }

  /** Keeps the cache small: the plotted and baseline texts, and the newest few. */
  private trim(): void {
    const keep = new Set([this.current, this.baseline?.text, this.shownText]);
    for (const k of [...this.cache.keys()]) {
      if (this.cache.size <= 6) break;
      if (!keep.has(k)) this.cache.delete(k);
    }
  }

  private setStatus(text: string): void {
    this.status.textContent = text;
    this.status.title = text;
  }

  private render(): void {
    if (!this.current) {
      this.setStatus('Run a netlist to see its readouts.');
      this.grid.replaceChildren();
      this.notesBox.hidden = true;
      this.methods.hidden = true;
      this.full.hidden = true;
      this.error.hidden = true;
      this.shownText = null;
      return;
    }
    let entry = this.cache.get(this.current);
    let updating = false;
    if (!entry) {
      // Keep the previous numbers until these arrive, marked as updating.
      entry = this.shownText ? this.cache.get(this.shownText) : undefined;
      updating = true;
    } else {
      this.shownText = this.current;
    }
    this.root.classList.toggle('ro-updating', updating);
    const d = entry?.ok ? entry.doc : null;
    this.error.hidden = !entry || entry.ok;
    if (entry && !entry.ok) this.error.textContent = `Readouts failed: ${entry.message}`;
    const base = this.baseline ? this.cache.get(this.baseline.text) : undefined;
    const baseDoc = base?.ok ? base.doc : null;

    // The drive is the strip's; the readouts are at that drive.
    const parts = [updating ? 'Updating for the current design…' : `At the stated drive${d?.response ? `, response at ${d.response.probe}` : ''}.`];
    if (this.baseline) {
      parts.push(
        base && !base.ok
          ? `No Δ: the readouts of baseline “${this.baseline.name}” failed (${base.message}).`
          : `Δ against baseline “${this.baseline.name}”.`,
      );
    }
    this.setStatus(parts.join(' '));

    // The same cells every time: placeholders before the first readouts, a
    // dash for a readout this design does not have (or when they failed).
    const failed = !!entry && !entry.ok;
    this.grid.replaceChildren();
    const full: Node[] = [el('dt', undefined, 'Status'), el('dd', undefined, this.status.textContent ?? '')];
    for (const c of CELLS) {
      const v: Value = d
        ? (c.value(d) ?? { text: '—', nums: c.units.map(() => null), extra: ['not reported for this design; see the engine notes'] })
        : { text: failed ? '—' : '…', nums: c.units.map(() => null) };
      const wrap = el('div', 'ro-cell');
      wrap.dataset.readout = c.key;
      const dt = el('dt', undefined, c.label);
      const m = el('span', 'ro-method', c.method);
      m.title = d?.methods[c.methodKey] ?? '';
      dt.append(' ', m);
      const dd = el('dd');
      const line = el('span', 'ro-line');
      const val = el('span', 'ro-value', v.text);
      val.dataset.nums = JSON.stringify(v.nums);
      line.append(val);
      dd.append(line);
      const flagLine = el('span', 'ro-flagline');
      if (v.flag) {
        const f = el('span', 'ro-flag');
        const icon = el('span', undefined, '⚠ ');
        icon.setAttribute('aria-hidden', 'true');
        f.append(icon, v.flag);
        flagLine.append(f);
      }
      if (c.flags || v.flag) dd.append(flagLine);
      if (this.baseline) {
        const bv = baseDoc ? c.value(baseDoc) : null;
        const delta = el('span', 'ro-delta', `Δ ${!d ? (failed ? 'n/a' : '…') : !baseDoc ? (base ? 'n/a' : '…') : bv ? cellDelta(c, v, bv) : 'n/a'}`);
        delta.title = delta.textContent ?? '';
        dd.append(delta);
      }
      const extra = (v.extra ?? []).filter(Boolean).join('; ');
      const ex = el('span', 'ro-extra', extra);
      if (extra) ex.title = extra;
      dd.append(ex);
      wrap.append(dt, dd);
      this.grid.append(wrap);
      const whole = [v.text + (v.flag ? ` (${v.flag})` : ''), dd.querySelector('.ro-delta')?.textContent, extra].filter(Boolean).join('; ');
      full.push(el('dt', undefined, `${c.label} (${c.method})`), el('dd', undefined, whole));
    }

    const notes = d
      ? [
          ...d.notes,
          ...(d.impedance?.notes ?? []),
          ...(d.impedance?.q?.notes ?? []),
          ...(d.response?.notes ?? []),
          ...d.drivers.flatMap((x) => [...x.notes, ...(x.free_air?.notes ?? [])].map((n) => `${x.element}: ${n}`)),
        ]
      : [];
    this.notesSummary.textContent = `Engine notes (${notes.length})`;
    this.notes.replaceChildren(...notes.map((n) => el('li', undefined, n)));
    this.notesBox.hidden = false;
    this.methods.hidden = false;
    this.full.hidden = false;
    this.fullList.replaceChildren(...full);
    this.methodList.replaceChildren();
    if (!d) return;
    if (d.response) this.methodList.append(el('dt', undefined, 'response probe'), el('dd', undefined, `${d.response.probe} (${d.response.probe_source})`));
    if (d.impedance) this.methodList.append(el('dt', undefined, 'impedance'), el('dd', undefined, `${d.impedance.probe}; Re ${d.impedance.re_source}`));
    for (const [k, t] of Object.entries(d.methods)) this.methodList.append(el('dt', undefined, k), el('dd', undefined, t));
  }
}
