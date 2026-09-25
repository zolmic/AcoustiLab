// Measured curves for the Fit view: the curve and sidecar documents of the
// engine (docs/fitting.md, "Curves", "The sidecar"), reading dropped or
// picked files (a curve file pairs with FILE.sidecar.json), the sidecar
// summary and its form. Parsing and validation are the engine's
// (`import_curve`); this module only routes text to it and shows what it
// returns.

import { formatHz, formatNumber } from '../format';
import { defList, el, field, select, uniqueId } from './measure-kit';

export type Quantity = 'impedance' | 'pressure' | 'displacement' | 'velocity' | 'generic';

type Profile = number | { frequencies_Hz: number[]; values_dB?: number[]; values_deg?: number[] };

export interface Sidecar {
  schema: string;
  quantity?: Quantity;
  unit?: string;
  calibrated?: boolean;
  fixture?: string;
  ear_simulator?: string;
  pinna?: string;
  seatings?: number;
  averaging?: string;
  smoothing?: string;
  drive?: Record<string, unknown>;
  source_impedance_ohm?: number;
  compensation?: string;
  reference_point?: string;
  temperature_C?: number;
  date?: string;
  device?: string;
  provenance?: { origin?: string; source?: string; url?: string; licence?: string; tool?: string };
  uncertainty?: Record<string, Profile>;
  virtual_rig?: Record<string, unknown>;
  notes?: string;
}

/** A curve document (`acoustilab-curve/0.1`) as the engine writes it. */
export interface CurveDoc {
  schema: string;
  quantity: Quantity;
  frequencies_Hz: number[];
  level_dB?: number[];
  magnitude_ohm?: number[];
  magnitude_m?: number[];
  magnitude_m_per_s?: number[];
  magnitude?: number[];
  phase_deg?: number[];
  sidecar: Sidecar;
  comments?: string[];
}

export const SIDECAR_SCHEMA = 'acoustilab-curve-sidecar/0.1';
export const CURVE_SCHEMA = 'acoustilab-curve/0.1';

/** The plotted magnitude of a curve: dB SPL for pressure, the linear magnitude otherwise. */
export function magnitudeOf(c: CurveDoc): { values: number[]; unit: string; db: boolean } {
  switch (c.quantity) {
    case 'pressure':
      return { values: c.level_dB ?? [], unit: '', db: true };
    case 'impedance':
      return { values: c.magnitude_ohm ?? [], unit: 'ohm', db: false };
    case 'displacement':
      return { values: c.magnitude_m ?? [], unit: 'm', db: false };
    case 'velocity':
      return { values: c.magnitude_m_per_s ?? [], unit: 'm/s', db: false };
    default:
      return c.magnitude ? { values: c.magnitude, unit: c.sidecar.unit ?? '', db: false } : { values: c.level_dB ?? [], unit: '', db: true };
  }
}

/** Curve origin for labels: "virtual_rig", "measured", ... or null when unstated. */
export function originOf(c: CurveDoc): string | null {
  return c.sidecar.provenance?.origin ?? null;
}

export const isVirtual = (c: CurveDoc) => originOf(c) === 'virtual_rig' || c.sidecar.virtual_rig !== undefined;

/** A badge naming where a curve comes from; virtual-rig data is marked unmistakably. */
export function originBadge(c: CurveDoc): HTMLElement {
  if (isVirtual(c)) return el('span', { class: 'mv-badge', text: 'VIRTUAL RIG · synthetic, not measured' });
  const o = originOf(c);
  return el('span', { class: 'mv-badge plain', text: o ? `origin: ${o.replace(/_/g, ' ')}` : 'origin not stated' });
}

/** The curve format for a file name, as the engine's `Format::from_path` (else `auto`). */
export function formatFromName(name: string): string {
  const ext = name.toLowerCase().split('.').pop() ?? '';
  return { frd: 'frd', zma: 'zma', csv: 'csv', txt: 'rew', dat: 'rew' }[ext] ?? 'auto';
}

export interface FileText {
  name: string;
  text: string;
}

export interface PairedFile {
  name: string;
  text: string;
  /** A curve document given as JSON (instead of a text format). */
  doc?: unknown;
  /** Text of its FILE.sidecar.json, when one came with it. */
  sidecar?: { name: string; text: string };
}

/**
 * Pairs curve files with their sidecars (`FILE.sidecar.json` next to
 * `FILE`). A sidecar without its curve is returned in `sidecars`.
 */
export function pairFiles(files: FileText[]): { curves: PairedFile[]; sidecars: FileText[]; errors: string[] } {
  const side = new Map<string, FileText>();
  const curves: PairedFile[] = [];
  const errors: string[] = [];
  for (const f of files) {
    if (/\.sidecar\.json$/i.test(f.name)) side.set(f.name.replace(/\.sidecar\.json$/i, '').toLowerCase(), f);
  }
  for (const f of files) {
    if (/\.sidecar\.json$/i.test(f.name)) continue;
    if (/\.json$/i.test(f.name)) {
      let doc: unknown;
      try {
        doc = JSON.parse(f.text);
      } catch (e) {
        errors.push(`“${f.name}” is not valid JSON: ${(e as Error).message}`);
        continue;
      }
      const schema = (doc as { schema?: unknown })?.schema;
      if (schema === SIDECAR_SCHEMA) {
        side.set(f.name.replace(/\.json$/i, '').toLowerCase(), f);
        continue;
      }
      curves.push({ name: f.name, text: f.text, doc });
      continue;
    }
    const s = side.get(f.name.toLowerCase());
    if (s) side.delete(f.name.toLowerCase());
    curves.push({ name: f.name, text: f.text, sidecar: s });
  }
  return { curves, sidecars: [...side.values()], errors };
}

/** Reads dropped or picked files as text (curve files are small; larger than 20 MB is refused). */
export async function readFiles(list: FileList | File[]): Promise<{ files: FileText[]; errors: string[] }> {
  const files: FileText[] = [];
  const errors: string[] = [];
  for (const f of Array.from(list)) {
    if (f.size > 20e6) {
      errors.push(`“${f.name}” is larger than 20 MB and was not read.`);
      continue;
    }
    files.push({ name: f.name, text: await f.text() });
  }
  return { files, errors };
}

/** The line `n` (1-based) of a text, for quoting next to an error. */
export function lineOf(text: string, n: number): string | null {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  return n >= 1 && n <= lines.length ? lines[n - 1] : null;
}

/**
 * Parses "name=value, name=value" (commas, semicolons or new lines) into
 * parameter overrides: numbers, true/false, else text. Errors name the item.
 */
export function parseAssignments(text: string): { values: Record<string, number | boolean | string>; error: string | null } {
  const values: Record<string, number | boolean | string> = {};
  for (const raw of text.split(/[,;\n]/)) {
    const item = raw.trim();
    if (!item) continue;
    const m = /^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+)$/.exec(item);
    if (!m) return { values, error: `“${item}” is not name=value` };
    const v = m[2].trim();
    values[m[1]] = /^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/i.test(v) ? Number(v) : v === 'true' ? true : v === 'false' ? false : v.replace(/^"(.*)"$/, '$1');
  }
  return { values, error: null };
}

/** "leak_gap_mm=0.12, rear=open" */
export function assignmentsText(values: Record<string, unknown>): string {
  return Object.entries(values)
    .map(([k, v]) => `${k}=${String(v)}`)
    .join(', ');
}

/** A drive object in the netlist's keys, as text ("power_mW 1, rated_ohm 32"). */
export function driveText(d: Record<string, unknown> | undefined): string | null {
  if (!d) return null;
  return Object.entries(d)
    .map(([k, v]) => `${k} ${String(v)}`)
    .join(', ');
}

function profileText(p: Profile, unit: string): string {
  if (typeof p === 'number') return `${formatNumber(p, 4)} ${unit}`;
  const vals = p.values_dB ?? p.values_deg ?? [];
  return `table over ${formatHz(p.frequencies_Hz[0])}–${formatHz(p.frequencies_Hz[p.frequencies_Hz.length - 1])} (${vals.length} values)`;
}

/** The sidecar's statements as a definition list (unstated fields are left out). */
export function sidecarSummary(s: Sidecar): HTMLDListElement {
  const u = s.uncertainty ?? {};
  const terms = Object.entries(u).map(([k, p]) => `${k.replace(/_(dB|deg)$/, '').replace(/_/g, ' ')} ${profileText(p, k.endsWith('_deg') ? '°' : 'dB')}`);
  const prov = s.provenance;
  return defList([
    ['Fixture', s.fixture ?? null],
    ['Ear simulator', s.ear_simulator ?? null],
    ['Reference point', s.reference_point ?? null],
    ['Drive', driveText(s.drive)],
    ['Source impedance', s.source_impedance_ohm !== undefined ? `${formatNumber(s.source_impedance_ohm, 4)} Ω` : null],
    ['Levels', s.calibrated === undefined ? null : s.calibrated ? 'calibrated (absolute at the stated drive)' : 'not calibrated (a fit gives them a free offset)'],
    ['Seatings', s.seatings !== undefined ? `${s.seatings}${s.averaging ? `, averaged: ${s.averaging}` : ''}` : s.averaging ? `averaged: ${s.averaging}` : null],
    ['Smoothing', s.smoothing ?? null],
    ['Compensation', s.compensation ?? null],
    ['Uncertainty (1σ, per seating where it averages down)', terms.length ? terms.join('; ') : null],
    ['Provenance', prov ? [prov.origin, prov.source, prov.tool, prov.licence].filter(Boolean).join('; ') : null],
    ['Date', s.date ?? null],
    ['Notes', s.notes ?? null],
  ]);
}

const UNCERTAINTY_TERMS: [string, string][] = [
  ['noise_dB', 'Noise (dB, one seating)'],
  ['repositioning_dB', 'Repositioning (dB, one seating)'],
  ['microphone_calibration_dB', 'Sensor calibration (dB)'],
  ['coupler_dB', 'Coupler / simulator (dB)'],
  ['fixture_to_human_dB', 'Fixture to human (dB)'],
  ['numerical_dB', 'Numerical (dB)'],
  ['phase_deg', 'Phase (°, one seating)'],
];

/**
 * A form over the common sidecar fields. Fields it does not show
 * (provenance, virtual-rig settings, tabulated uncertainty terms, ...) are
 * kept as they are. `read()` returns the edited sidecar or an input error.
 */
export function sidecarForm(s: Sidecar, quantity: Quantity): { root: HTMLElement; read(): { sidecar: Sidecar | null; error: string | null } } {
  const text = (v: string | undefined, placeholder = 'not stated') => {
    const i = el('input', { attrs: { type: 'text', placeholder, autocomplete: 'off' } });
    i.value = v ?? '';
    return i;
  };
  const num = (v: number | undefined, step = 'any', min = '') => {
    const i = el('input', { attrs: { type: 'number', step, placeholder: 'not stated', inputmode: 'decimal', ...(min ? { min } : {}) } });
    i.value = v === undefined ? '' : String(v);
    return i;
  };
  const fixture = text(s.fixture);
  const ear = text(s.ear_simulator);
  const ref = select(
    [['', 'not stated'], ['drp', 'drp (drum reference point)'], ['eep', 'eep (ear entrance)'], ['erp', 'erp (ear reference point)'], ['coupler', 'coupler'], ['terminals', 'terminals']],
    s.reference_point ?? '',
  );
  if (s.reference_point && ![...ref.options].some((o) => o.value === s.reference_point)) {
    ref.append(el('option', { text: s.reference_point, attrs: { value: s.reference_point } }));
    ref.value = s.reference_point;
  }
  const cal = select([['', 'not stated'], ['true', 'calibrated (absolute)'], ['false', 'not calibrated']], s.calibrated === undefined ? '' : String(s.calibrated));
  const seat = num(s.seatings, '1', '1');
  const avg = select([['', 'not stated'], ['none', 'none'], ['magnitude', 'magnitude (power mean)'], ['db', 'dB (mean level)'], ['complex', 'complex (vector mean)']], s.averaging ?? '');
  const smooth = text(s.smoothing, 'none or 1/N');
  const comp = text(s.compensation, 'none, diffuse_field, free_field, ...');
  // Drive: the netlist's drive keys.
  const d = s.drive ?? {};
  const conv = 'voltage_V' in d ? 'voltage' : 'power_mW' in d ? 'power' : 'current_mA' in d ? 'current' : 'characteristic' in d ? 'characteristic' : Object.keys(d).length ? 'other' : '';
  const driveSel = select(
    [['', 'not stated'], ['voltage', 'voltage (V RMS EMF)'], ['power', 'power (mW into rated Ω)'], ['current', 'current (mA)'], ['characteristic', 'characteristic voltage']],
    conv === 'other' ? '' : conv,
  );
  const dv = num((d.voltage_V ?? d.power_mW ?? d.current_mA) as number | undefined);
  const rated = num(d.rated_ohm as number | undefined);
  const charProbe = text(typeof d.characteristic === 'string' ? d.characteristic : undefined, 'pressure probe');
  const zs = num(s.source_impedance_ohm, 'any', '0');
  const unc = s.uncertainty ?? {};
  const uncInputs = UNCERTAINTY_TERMS.map(([k, label]) => {
    const p = unc[k];
    const i = num(typeof p === 'number' ? p : undefined, 'any', '0');
    if (p !== undefined && typeof p !== 'number') {
      i.disabled = true;
      i.placeholder = 'table (kept)';
    }
    return { k, label, i };
  });
  const notes = text(s.notes, '');
  const drivePart = el(
    'fieldset',
    {},
    el('legend', { text: 'Drive (the netlist’s drive keys)' }),
    field('Convention', driveSel),
    field('Level (V, mW or mA)', dv),
    field('Rated impedance (Ω, power)', rated),
    field('Probe (characteristic)', charProbe),
    field('Source impedance (Ω)', zs),
  );
  const uncPart = el('fieldset', {}, el('legend', { text: 'Uncertainty budget (standard uncertainties, 1σ)' }), ...uncInputs.map((u) => field(u.label, u.i)));
  const root = el(
    'div',
    { class: 'mv-form', id: uniqueId('mv-sidecar') },
    field('Fixture', fixture),
    field('Ear simulator', ear),
    field('Reference point', ref),
    field('Levels', cal),
    field('Seatings', seat),
    field('Averaging', avg),
    field('Smoothing', smooth),
    field('Compensation', comp),
    drivePart,
    uncPart,
    el('div', { class: 'mv-wide' }, field('Notes', notes)),
  );
  if (quantity === 'impedance') drivePart.append(el('p', { class: 'hint mv-wide', text: 'An impedance curve does not depend on the drive level in a linear model.' }));
  const read = (): { sidecar: Sidecar | null; error: string | null } => {
    const out: Sidecar = { ...s, schema: SIDECAR_SCHEMA };
    const setText = (k: keyof Sidecar, v: string) => {
      if (v.trim()) (out as unknown as Record<string, unknown>)[k] = v.trim();
      else delete (out as unknown as Record<string, unknown>)[k];
    };
    const numberOf = (i: HTMLInputElement, what: string): number | undefined | string => {
      if (!i.value.trim()) return undefined;
      const v = Number(i.value);
      return Number.isFinite(v) ? v : `${what} must be a number`;
    };
    setText('fixture', fixture.value);
    setText('ear_simulator', ear.value);
    setText('reference_point', ref.value);
    setText('smoothing', smooth.value);
    setText('compensation', comp.value);
    setText('averaging', avg.value);
    setText('notes', notes.value);
    if (cal.value) out.calibrated = cal.value === 'true';
    else delete out.calibrated;
    const n = numberOf(seat, 'Seatings');
    if (typeof n === 'string') return { sidecar: null, error: n };
    if (n === undefined) delete out.seatings;
    else out.seatings = n;
    const z = numberOf(zs, 'Source impedance');
    if (typeof z === 'string') return { sidecar: null, error: z };
    if (z === undefined) delete out.source_impedance_ohm;
    else out.source_impedance_ohm = z;
    const level = numberOf(dv, 'The drive level');
    if (typeof level === 'string') return { sidecar: null, error: level };
    switch (driveSel.value) {
      case '':
        if (conv !== 'other') delete out.drive;
        break;
      case 'voltage':
        out.drive = { voltage_V: level };
        break;
      case 'power': {
        const r = numberOf(rated, 'The rated impedance');
        if (typeof r === 'string') return { sidecar: null, error: r };
        out.drive = { power_mW: level, rated_ohm: r };
        break;
      }
      case 'current':
        out.drive = { current_mA: level };
        break;
      case 'characteristic':
        out.drive = { characteristic: charProbe.value.trim() };
        break;
    }
    if (out.drive) out.drive = Object.fromEntries(Object.entries(out.drive).filter(([, v]) => v !== undefined));
    const u: Record<string, Profile> = { ...(s.uncertainty ?? {}) };
    for (const { k, label, i } of uncInputs) {
      if (i.disabled) continue;
      const v = numberOf(i, label);
      if (typeof v === 'string') return { sidecar: null, error: v };
      if (v === undefined) delete u[k];
      else u[k] = v;
    }
    if (Object.keys(u).length) out.uncertainty = u;
    else delete out.uncertainty;
    return { sidecar: out, error: null };
  };
  return { root, read };
}

/** The import options of the drop zone: format and quantity (auto by default). */
export function importOptions(): { root: HTMLElement; format: HTMLSelectElement; quantity: HTMLSelectElement } {
  const format = select(
    [['', 'from the file name'], ['auto', 'auto (detect)'], ['frd', 'FRD'], ['zma', 'ZMA'], ['rew', 'REW text'], ['csv', 'CSV']],
    '',
  );
  const quantity = select(
    [['', 'from the file'], ['pressure', 'pressure (dB SPL)'], ['impedance', 'impedance (Ω)'], ['displacement', 'displacement (m)'], ['velocity', 'velocity (m/s)'], ['generic', 'generic']],
    '',
  );
  return { root: el('div', { class: 'mv-bar' }, field('Format', format), field('Quantity', quantity)), format, quantity };
}

