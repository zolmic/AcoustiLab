// Turns a solve result into plot groups: one plot per plotted quantity and
// unit, so no plot ever carries two y-scales.
//
// * Acoustic pressures (probes with spl_dB): dB SPL re 20 µPa, RMS.
// * Impedance probes: |Z| in one plot, phase in a second, smaller plot.
//   An impedance probe on a mechanical port reads v/F (a mobility) under the
//   across/through convention, and is labelled as such.
// * Everything else: magnitude of the phasor, grouped by unit.
//
// Frozen baselines add overlay series to the plots of the probes they share
// with the live result, and optionally a difference plot (live SPL minus a
// baseline's, in dB).

import type { Num, ProbeResult, SolveResult } from './types';
import { prettyUnit } from './format';

export type Kind = 'spl' | 'mag' | 'phase' | 'delta';
export type Scale = 'linear' | 'log';

export interface Series {
  /** Index of the probe in the result: fixes its colour and dash. */
  probe: number;
  id: string;
  values: Num[];
  /** The design's primary probe (`ui.primary_probe`): drawn heavier. */
  primary?: boolean;
}

/** A frozen snapshot of a result (a baseline for comparisons). */
export interface Baseline {
  id: number;
  name: string;
  /** Netlist text the frozen result was solved from (re-solvable by views, e.g. for audition). */
  text: string;
  freqs: number[];
  probes: Pick<ProbeResult, 'id' | 'quantity' | 'unit' | 'domain' | 'magnitude' | 'phase_deg' | 'spl_dB'>[];
}

/** One baseline curve drawn in a live plot. */
export interface OverlaySeries {
  /** Probe index of the live curve it belongs to (its colour). */
  probe: number;
  id: string;
  /** Position of the baseline in the list (its dash pattern). */
  slot: number;
  name: string;
  freqs: number[];
  values: Num[];
}

export interface PlotGroup {
  key: string;
  kind: Kind;
  title: string;
  /** Quantity symbol for the axis label, e.g. "|Z|". */
  symbol: string;
  /** Unit in engine spelling ("" for dB or degrees). */
  unit: string;
  /** Axis unit text as displayed ("dB SPL re 20 µPa", "°", "Ω"). */
  axisUnit: string;
  scale: Scale;
  series: Series[];
  /** Baseline curves drawn under the live ones. */
  overlays: OverlaySeries[];
  /** Filled ranges drawn under every curve (envelopes, tolerance bands). */
  bands?: BandSeries[];
  /** Relative height of the plot. */
  height: 'main' | 'small';
}

/**
 * A filled range between two curves on the plot's grid (a Monte Carlo
 * envelope, a preference band), in a translucent tone of a series colour,
 * with thin edge lines at full colour so the range stays readable where
 * the fill is faint. Hidden with its `id` like a curve.
 */
export interface BandSeries {
  /** Style slot whose colour the band takes (as `Series.probe`). */
  probe: number;
  id: string;
  /** Name in the plot's description, e.g. "5–95 % of runs". */
  label: string;
  lower: Num[];
  upper: Num[];
  /** Fill opacity, 0 to 1. */
  alpha: number;
  /** Dash pattern of the edge lines ([] solid), or null for no edges. */
  edge: number[] | null;
}

/**
 * Value of a curve at frequency f: the grid value when f is on the grid
 * (to 1e-9 relative), otherwise linear in log f between the neighbours.
 * Null outside the grid or next to a gap.
 */
export function valueAt(freqs: number[], values: Num[], f: number): { v: number | null; exact: boolean } {
  let lo = 0;
  let hi = freqs.length - 1;
  if (hi < 0 || f < freqs[0] * (1 - 1e-9) || f > freqs[hi] * (1 + 1e-9)) return { v: null, exact: false };
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (freqs[mid] <= f) lo = mid;
    else hi = mid;
  }
  for (const k of [lo, hi]) if (Math.abs(freqs[k] / f - 1) <= 1e-9) return { v: values[k], exact: true };
  const a = values[lo];
  const b = values[hi];
  if (a === null || b === null) return { v: null, exact: false };
  const t = Math.log(f / freqs[lo]) / Math.log(freqs[hi] / freqs[lo]);
  return { v: a + t * (b - a), exact: false };
}

/** Whether two frequency grids are the same (to 1e-9 relative). */
export function sameGrid(a: number[], b: number[]): boolean {
  return a.length === b.length && a.every((f, i) => Math.abs(f / b[i] - 1) <= 1e-9);
}

export interface GroupOptions {
  /** `ui.primary_probe`: its plot comes first, and its curve first and heavier. */
  primary?: string;
  baselines?: Baseline[];
  /** Adds a plot of live SPL minus this baseline's, after the SPL plot. */
  difference?: Baseline | null;
}

const QUANTITY_TITLES: Record<string, [string, string]> = {
  displacement: ['Displacement', '|x|'],
  velocity: ['Velocity', '|v|'],
  acceleration: ['Acceleration', '|a|'],
  voltage: ['Voltage', '|V|'],
  current: ['Current', '|I|'],
  force: ['Force', '|F|'],
  volume_velocity: ['Volume velocity', '|U|'],
  flow: ['Port flow', '|flow|'],
  port_potential: ['Port potential', '|Δ|'],
  potential: ['Node potential', '|potential|'],
  pressure: ['Pressure', '|p|'],
};

function impedanceTitle(p: ProbeResult): { title: string; symbol: string; phaseTitle: string; phaseSymbol: string } {
  switch (p.domain) {
    case 'electrical':
      return { title: 'Electrical impedance', symbol: '|Z|', phaseTitle: 'Electrical impedance phase', phaseSymbol: '∠Z' };
    case 'acoustic':
      return { title: 'Acoustic impedance', symbol: '|Z|', phaseTitle: 'Acoustic impedance phase', phaseSymbol: '∠Z' };
    case 'mechanical':
      return {
        title: 'Mechanical mobility (velocity / force)',
        symbol: '|v/F|',
        phaseTitle: 'Mechanical mobility phase',
        phaseSymbol: '∠(v/F)',
      };
    default:
      return {
        title: 'Port potential / port flow',
        symbol: '|Z|',
        phaseTitle: 'Port potential / port flow, phase',
        phaseSymbol: '∠Z',
      };
  }
}

/** Log scale when the finite positive values span more than a decade. */
export function chooseScale(series: Series[]): Scale {
  let lo = Infinity;
  let hi = -Infinity;
  for (const s of series) {
    for (const v of s.values) {
      if (v !== null && Number.isFinite(v) && v > 0) {
        lo = Math.min(lo, v);
        hi = Math.max(hi, v);
      }
    }
  }
  return hi / lo > 10 ? 'log' : 'linear';
}

export function buildGroups(r: SolveResult, opts: GroupOptions = {}): PlotGroup[] {
  const spl: PlotGroup = {
    key: 'spl',
    kind: 'spl',
    title: 'Sound pressure level',
    symbol: 'SPL',
    unit: '',
    axisUnit: 'dB re 20 µPa',
    scale: 'linear',
    series: [],
    overlays: [],
    height: 'main',
  };
  const impedance = new Map<string, [PlotGroup, PlotGroup]>();
  const magnitude = new Map<string, PlotGroup>();

  r.probes.forEach((p, i) => {
    if (p.spl_dB) {
      spl.series.push({ probe: i, id: p.id, values: p.spl_dB });
      return;
    }
    if (p.quantity === 'impedance') {
      const key = `imp:${p.domain ?? '?'}:${p.unit}`;
      let pair = impedance.get(key);
      if (!pair) {
        const t = impedanceTitle(p);
        pair = [
          {
            key,
            kind: 'mag',
            title: t.title,
            symbol: t.symbol,
            unit: p.unit,
            axisUnit: prettyUnit(p.unit),
            scale: 'linear',
            series: [],
            overlays: [],
            height: 'main',
          },
          {
            key: `${key}:phase`,
            kind: 'phase',
            title: t.phaseTitle,
            symbol: t.phaseSymbol,
            unit: '',
            axisUnit: '°',
            scale: 'linear',
            series: [],
            overlays: [],
            height: 'small',
          },
        ];
        impedance.set(key, pair);
      }
      pair[0].series.push({ probe: i, id: p.id, values: p.magnitude });
      pair[1].series.push({ probe: i, id: p.id, values: p.phase_deg });
      return;
    }
    const key = `mag:${p.unit}`;
    let g = magnitude.get(key);
    const [title, symbol] = QUANTITY_TITLES[p.quantity] ?? [p.quantity, '|·|'];
    if (!g) {
      g = {
        key,
        kind: 'mag',
        title,
        symbol,
        unit: p.unit,
        axisUnit: p.unit ? prettyUnit(p.unit) : 'unit unknown',
        scale: 'linear',
        series: [],
        overlays: [],
        height: 'main',
      };
      magnitude.set(key, g);
    } else if (!g.title.split(' / ').includes(title)) {
      g.title = `${g.title} / ${title}`;
      g.symbol = '|·|';
    }
    g.series.push({ probe: i, id: p.id, values: p.magnitude });
  });

  let groups: PlotGroup[] = [];
  if (spl.series.length) groups.push(spl);
  for (const [mag, phase] of impedance.values()) {
    mag.scale = chooseScale(mag.series);
    groups.push(mag, phase);
  }
  for (const g of magnitude.values()) {
    g.scale = chooseScale(g.series);
    groups.push(g);
  }

  // The primary probe's plot first, its curve first in it (colours and
  // dashes still follow the netlist order).
  if (opts.primary) {
    for (const g of groups) {
      const k = g.series.findIndex((s) => s.id === opts.primary);
      if (k < 0) continue;
      g.series[k].primary = true;
      g.series.unshift(...g.series.splice(k, 1));
    }
    const first = groups.findIndex((g) => g.series[0]?.primary);
    if (first > 0) {
      const lead = groups.splice(first, runLength(groups, first));
      groups = [...lead, ...groups];
    }
  }

  // Baseline curves: the same probe id and unit, in the matching quantity.
  (opts.baselines ?? []).forEach((b, slot) => {
    for (const g of groups) {
      for (const s of g.series) {
        const live = r.probes[s.probe];
        const q = b.probes.find((p) => p.id === s.id && p.unit === live.unit && p.quantity === live.quantity);
        if (!q) continue;
        const values = g.kind === 'spl' ? q.spl_dB : g.kind === 'phase' ? q.phase_deg : q.magnitude;
        if (values) g.overlays.push({ probe: s.probe, id: s.id, slot, name: b.name, freqs: b.freqs, values });
      }
    }
  });

  // Difference plot: live SPL minus the baseline's, on the live grid.
  const ref = opts.difference;
  if (ref && spl.series.length) {
    const delta: PlotGroup = {
      key: 'delta',
      kind: 'delta',
      title: `SPL difference, current − “${ref.name}”`,
      symbol: 'ΔSPL',
      unit: '',
      axisUnit: 'dB',
      scale: 'linear',
      series: [],
      overlays: [],
      height: 'small',
    };
    for (const s of spl.series) {
      const q = ref.probes.find((p) => p.id === s.id && p.spl_dB);
      if (!q) continue;
      const values = r.frequencies_Hz.map((f, i) => {
        const a = s.values[i];
        const b = valueAt(ref.freqs, q.spl_dB!, f).v;
        return a === null || b === null ? null : a - b;
      });
      delta.series.push({ probe: s.probe, id: s.id, values, primary: s.primary });
    }
    if (delta.series.length) groups.splice(groups.indexOf(spl) + 1, 0, delta);
  }
  return groups;
}

/** Length of the run of plots that belong together from index k (a |Z| plot and its phase). */
function runLength(groups: PlotGroup[], k: number): number {
  return groups[k + 1]?.key === `${groups[k].key}:phase` ? 2 : 1;
}

// ----- series styling ---------------------------------------------------

/**
 * Dash patterns (px at 2 px line width). Colour and dash both follow the
 * probe index, so identity never rests on hue alone; past 8 probes the
 * (colour, dash) pair stays unique up to 64 probes.
 */
export const DASHES: number[][] = [
  [],
  [9, 4],
  [2, 3],
  [12, 3, 2, 3],
  [5, 5],
  [14, 4, 4, 4],
  [1, 5],
  [7, 2, 2, 2, 2, 2],
];

export function styleSlot(probe: number): { color: number; dash: number } {
  return { color: probe % 8, dash: (probe + Math.floor(probe / 8)) % 8 };
}

/**
 * Baseline overlays: thin (1.25 px) lines in a muted tone of the live
 * curve's colour (OVERLAY_MIX of it, the rest the secondary ink; plot.ts
 * `mixHex`), with one of these patterns per baseline. None of them occurs
 * in DASHES, and no live curve is drawn thin or muted, so a baseline never
 * looks like a live curve; the legend, the plot descriptions and the
 * readout name them.
 */
export const OVERLAY_DASHES: number[][] = [
  [4, 2],
  [1, 2],
  [7, 2, 1, 2],
  [2, 1.5],
  [10, 2, 1, 2, 1, 2],
];

export const OVERLAY_WIDTH = 1.25;
/** Weight of the probe colour in an overlay's stroke. */
export const OVERLAY_MIX = 0.5;
export const LIVE_WIDTH = 2;
export const PRIMARY_WIDTH = 2.75;
