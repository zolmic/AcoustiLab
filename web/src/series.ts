// Turns a solve result into plot groups: one plot per plotted quantity and
// unit, so no plot ever carries two y-scales.
//
// * Acoustic pressures (probes with spl_dB): dB SPL re 20 µPa, RMS.
// * Impedance probes: |Z| in one plot, phase in a second, smaller plot.
//   An impedance probe on a mechanical port reads v/F (a mobility) under the
//   across/through convention, and is labelled as such.
// * Everything else: magnitude of the phasor, grouped by unit.

import type { Num, ProbeResult, SolveResult } from './types';
import { prettyUnit } from './format';

export type Kind = 'spl' | 'mag' | 'phase';
export type Scale = 'linear' | 'log';

export interface Series {
  /** Index of the probe in the result: fixes its colour and dash. */
  probe: number;
  id: string;
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
  /** Relative height of the plot. */
  height: 'main' | 'small';
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

export function buildGroups(r: SolveResult): PlotGroup[] {
  const spl: PlotGroup = {
    key: 'spl',
    kind: 'spl',
    title: 'Sound pressure level',
    symbol: 'SPL',
    unit: '',
    axisUnit: 'dB re 20 µPa',
    scale: 'linear',
    series: [],
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
        height: 'main',
      };
      magnitude.set(key, g);
    } else if (!g.title.split(' / ').includes(title)) {
      g.title = `${g.title} / ${title}`;
      g.symbol = '|·|';
    }
    g.series.push({ probe: i, id: p.id, values: p.magnitude });
  });

  const groups: PlotGroup[] = [];
  if (spl.series.length) groups.push(spl);
  for (const [mag, phase] of impedance.values()) {
    mag.scale = chooseScale(mag.series);
    groups.push(mag, phase);
  }
  for (const g of magnitude.values()) {
    g.scale = chooseScale(g.series);
    groups.push(g);
  }
  return groups;
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
