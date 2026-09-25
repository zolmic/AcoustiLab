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
// Numbers only: each readout names its method (a short name here, the
// engine's full method text under "Methods"); the engine's notes say why a
// value is missing. Explanations of causes belong to the Sensitivity view.

import { EngineWorker } from './engine';
import { formatHz, formatParam } from './format';
import { isError } from './types';
import { el, exposeForTests, signed, signedSig } from './views/analysis-ui';
import type { Readouts } from './views/analysis-types';

type Entry = { ok: true; doc: Readouts } | { ok: false; message: string };

interface Cell {
  key: string;
  label: string;
  /** Short name of the method; the engine's `methods` entry is `methodKey`. */
  method: string;
  methodKey: string;
  /** Main value text, and the numbers compared against a baseline. */
  value(d: Readouts): { text: string; nums: (number | null)[]; extra?: string[]; flag?: string } | null;
  /** Units of `nums`, for the Δ line. */
  units: string[];
}

const hz = (f: number) => formatHz(f);
const ohm = (z: number) => `${formatParam(z, 4)} Ω`;
const q3 = (x: number) => formatParam(x, 3);
const shadeText = (s: number) => (s === 2 ? 'in the dark validity band' : s === 1 ? 'in the light validity band' : '');

const CELLS: Cell[] = [
  {
    key: 'coupled_resonance',
    label: 'Coupled resonance',
    method: 'max |v/i|',
    methodKey: 'coupled_resonance',
    units: ['Hz'],
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
    value: (d) => {
      const z = d.impedance;
      if (!z) return null;
      if (!z.q) return { text: 'not estimated', nums: [null, null, null], flag: 'no Q', extra: ['see the notes below'] };
      return { text: `${q3(z.q.Qms)} · ${q3(z.q.Qes)} · ${q3(z.q.Qts)}`, nums: [z.q.Qms, z.q.Qes, z.q.Qts] };
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
    method: '≥ 80 % of rated (spec §4)',
    methodKey: 'rated_check',
    units: ['Ω'],
    value: (d) => {
      const c = d.impedance?.rated_check;
      if (!c) return null;
      const extra = [
        c.pass
          ? `pass: above ${ohm(c.limit_ohm)} (80 % of ${ohm(c.rated_ohm)} rated)`
          : `below ${ohm(c.limit_ohm)} (80 % of ${ohm(c.rated_ohm)} rated) ${c.violations.map((v) => `${hz(v.f_min_Hz)}–${hz(v.f_max_Hz)}`).join(', ')}`,
      ];
      return { text: `${ohm(c.z_min_ohm)} at ${hz(c.z_min_Hz)}`, nums: [c.z_min_ohm], extra, flag: c.pass ? undefined : 'fails' };
    },
  },
  ...[500, 1000].map(
    (f): Cell => ({
      key: `sensitivity_${f}`,
      label: `Sensitivity at ${f === 500 ? '500 Hz' : '1 kHz'}`,
      method: 'dB/mW into rated Z (E32)',
      methodKey: 'sensitivity',
      units: ['dB', 'dB'],
      value: (d) => {
        const s = d.response?.sensitivity.find((x) => Math.abs(x.f_Hz - f) < 1e-9);
        if (!s) return null;
        const mw = s.dB_per_mW;
        return {
          text: `${s.dB_per_V.toFixed(1)} dB/V${mw === null ? '' : ` · ${mw.toFixed(1)} dB/mW`}`,
          nums: [s.dB_per_V, mw],
          extra: mw === null ? ['dB/V only (no rated impedance)'] : undefined,
        };
      },
    }),
  ),
  {
    key: 'bass_extension',
    label: 'Bass extension (−3 dB re 500 Hz)',
    method: 'grid walk, refined',
    methodKey: 'bass_extension',
    units: ['Hz'],
    value: (d) => {
      const r = d.response;
      if (!r) return null;
      const ref = `reference ${r.level_500Hz_dB.toFixed(1)} dB SPL at 500 Hz`;
      const b = r.bass_extension;
      if (!b) return { text: 'beyond the sweep', nums: [null], extra: [ref] };
      const extra = [ref];
      if (b.shading) extra.push(shadeText(b.shading));
      return { text: hz(b.f_Hz), nums: [b.f_Hz], extra };
    },
  },
];

/** Free-air values of each driver element (a cell per driver). */
function driverCells(d: Readouts): Cell[] {
  return d.drivers.map((drv, k) => ({
    key: `free_air_${k}`,
    label: `Free-air driver “${drv.element}”`,
    method: '√r0 of unloaded |Z|',
    methodKey: 'free_air',
    units: ['Hz', '', '', ''],
    value: (x) => {
      const fa = x.drivers[k]?.free_air;
      if (!fa) return { text: 'not estimated', nums: [null, null, null, null], extra: ['see the notes below'] };
      return {
        text: `fs ${hz(fa.f_Hz)} · Qms ${q3(fa.Qms)} · Qes ${q3(fa.Qes)} · Qts ${q3(fa.Qts)}`,
        nums: [fa.f_Hz, fa.Qms, fa.Qes, fa.Qts],
        extra: [`Re ${ohm(fa.Re_ohm)}`],
      };
    },
  }));
}

/** The Δ line of a cell: each number's change, in its unit. */
function deltaText(cur: (number | null)[], base: (number | null)[], units: string[]): string {
  const parts = cur.map((c, i) => {
    const b = base[i];
    if (c === null || b === null || b === undefined) return 'n/a';
    const u = units[i] ? ` ${units[i]}` : '';
    return `${units[i] === 'dB' ? signed(c - b, 2) : signedSig(c - b, 3)}${u}`;
  });
  return parts.every((p) => p === 'n/a') ? 'n/a' : parts.join(' · ');
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
  private readonly notes = el('ul', 'ro-notes');
  private readonly methods = el('details', 'ro-methods');
  private readonly methodList = el('dl');
  private readonly error = el('p', 'ro-error');

  constructor(private readonly root: HTMLElement) {
    const head = el('div', 'ro-head');
    const h = el('h3', undefined, 'Readouts');
    h.id = 'readouts-heading';
    root.setAttribute('aria-labelledby', h.id);
    head.append(h, this.status);
    this.methods.append(el('summary', undefined, 'Methods (as the engine states them)'), this.methodList);
    this.error.hidden = true;
    root.append(head, this.error, this.grid, this.notes, this.methods);
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

  private render(): void {
    const cur = this.current ? this.cache.get(this.current) : undefined;
    if (!this.current) {
      this.status.textContent = 'Run a netlist to see its readouts.';
      this.grid.replaceChildren();
      this.notes.replaceChildren();
      this.methods.hidden = true;
      this.error.hidden = true;
      this.shownText = null;
      return;
    }
    let entry = cur;
    let updating = false;
    if (!entry) {
      // Keep the previous numbers until these arrive, marked as updating.
      entry = this.shownText ? this.cache.get(this.shownText) : undefined;
      updating = true;
    } else {
      this.shownText = this.current;
    }
    this.root.classList.toggle('ro-updating', updating);
    if (!entry) {
      this.status.textContent = 'Computing readouts…';
      return;
    }
    if (!entry.ok) {
      this.error.hidden = false;
      this.error.textContent = `Readouts failed: ${entry.message}`;
      this.grid.replaceChildren();
      this.notes.replaceChildren();
      this.methods.hidden = true;
      this.status.textContent = updating ? 'Updating for the current design…' : '';
      return;
    }
    this.error.hidden = true;
    const d = entry.doc;
    const base = this.baseline ? this.cache.get(this.baseline.text) : undefined;
    const baseDoc = base?.ok ? base.doc : null;
    // The drive is the strip's; the readouts are at that drive.
    const parts = [updating ? 'Updating for the current design…' : 'At the stated drive.'];
    if (d.response) parts.push(`Response: ${d.response.probe} (${d.response.probe_source}).`);
    if (d.impedance) parts.push(`Impedance: ${d.impedance.probe}.`);
    if (this.baseline) {
      parts.push(
        baseDoc
          ? `Δ against baseline “${this.baseline.name}”.`
          : base && !base.ok
            ? `No Δ: the readouts of baseline “${this.baseline.name}” failed (${base.message}).`
            : `Δ against baseline “${this.baseline.name}”: computing…`,
      );
    }
    this.status.textContent = parts.join(' ');

    this.grid.replaceChildren();
    for (const c of [...CELLS, ...driverCells(d)]) {
      const v = c.value(d);
      if (!v) continue;
      const wrap = el('div', 'ro-cell');
      wrap.dataset.readout = c.key;
      const dt = el('dt', undefined, c.label);
      const m = el('span', 'ro-method', c.method);
      m.title = d.methods[c.methodKey] ?? '';
      dt.append(' ', m);
      const dd = el('dd');
      const val = el('span', 'ro-value', v.text);
      val.dataset.nums = JSON.stringify(v.nums);
      dd.append(val);
      if (v.flag) {
        const f = el('span', 'ro-flag');
        const icon = el('span', undefined, '⚠ ');
        icon.setAttribute('aria-hidden', 'true');
        f.append(icon, v.flag);
        dd.append(' ', f);
      }
      for (const x of v.extra ?? []) if (x) dd.append(el('span', 'ro-extra', x));
      if (baseDoc) {
        const bv = c.value(baseDoc);
        dd.append(el('span', 'ro-delta', `Δ ${bv ? deltaText(v.nums, bv.nums, c.units) : 'n/a'}`));
      }
      wrap.append(dt, dd);
      this.grid.append(wrap);
    }

    const notes = [
      ...d.notes,
      ...(d.impedance?.notes ?? []),
      ...(d.impedance?.q?.notes ?? []),
      ...(d.response?.notes ?? []),
      ...d.drivers.flatMap((x) => [...x.notes, ...(x.free_air?.notes ?? [])].map((n) => `${x.element}: ${n}`)),
    ];
    this.notes.replaceChildren(...notes.map((n) => el('li', undefined, n)));
    this.notes.hidden = notes.length === 0;
    this.methodList.replaceChildren();
    for (const [k, t] of Object.entries(d.methods)) this.methodList.append(el('dt', undefined, k), el('dd', undefined, t));
    this.methods.hidden = false;
  }
}
