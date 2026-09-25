// Number, unit and axis-tick formatting.

/** Engine unit spelling to display form: "m^3/s" -> "m³/s", "ohm" -> "Ω". */
export function prettyUnit(u: string): string {
  return u
    .replace(/ohm/g, 'Ω')
    .replace(/\^2/g, '²')
    .replace(/\^3/g, '³')
    .replace(/\^4/g, '⁴')
    .replace(/\*/g, '·');
}

/**
 * Whether an SI prefix may be attached to a unit. A prefix binds to the
 * first symbol, so it is only unambiguous when that symbol is not raised to
 * a power: "mm/s" is 1e-3 m/s, but "µm³/s" would be 1e-18 m³/s, not
 * 1e-6 m³/s.
 */
export function prefixable(unit: string): boolean {
  const m = /^[A-Za-z]+/.exec(unit);
  if (!m) return false;
  return unit[m[0].length] !== '^';
}

const PREFIXES: [number, string][] = [
  [1e12, 'T'],
  [1e9, 'G'],
  [1e6, 'M'],
  [1e3, 'k'],
  [1, ''],
  [1e-3, 'm'],
  [1e-6, 'µ'],
  [1e-9, 'n'],
  [1e-12, 'p'],
  [1e-15, 'f'],
];

/** Engineering prefix whose scaled magnitude falls in [1, 1000). */
export function prefixFor(a: number): [number, string] {
  for (const p of PREFIXES) {
    if (a >= p[0] * (1 - 1e-12)) return p;
  }
  return PREFIXES[PREFIXES.length - 1];
}

const SUP: Record<string, string> = {
  '-': '⁻',
  '0': '⁰',
  '1': '¹',
  '2': '²',
  '3': '³',
  '4': '⁴',
  '5': '⁵',
  '6': '⁶',
  '7': '⁷',
  '8': '⁸',
  '9': '⁹',
};

export function superscript(n: number): string {
  return String(n)
    .split('')
    .map((c) => SUP[c] ?? c)
    .join('');
}

/** Significant-figure formatting that never switches to "e" notation. */
function sig(x: number, digits: number): string {
  const s = x.toPrecision(digits);
  if (!s.includes('e')) return s;
  return x.toFixed(0);
}

/** "1.23 × 10⁻⁶" */
export function scientific(v: number, digits = 4): string {
  if (v === 0) return '0';
  const e = Math.floor(Math.log10(Math.abs(v)));
  let m = v / 10 ** e;
  let exp = e;
  if (Math.abs(Number(m.toPrecision(digits))) >= 10) {
    m /= 10;
    exp += 1;
  }
  return `${m.toPrecision(digits)} × 10${superscript(exp)}`;
}

/** A physical value with its unit: SI prefix where unambiguous, else scientific. */
export function formatValue(v: number | null, unit: string, digits = 4): string {
  if (v === null || !Number.isFinite(v)) return 'n/a';
  const pu = prettyUnit(unit);
  const sep = pu ? ' ' : '';
  if (v === 0) return `0${sep}${pu}`;
  const a = Math.abs(v);
  if (prefixable(unit)) {
    const [f, p] = prefixFor(a);
    return `${sig(v / f, digits)} ${p}${pu}`;
  }
  if (a >= 1e-3 && a < 1e5) return `${sig(v, digits)}${sep}${pu}`;
  return `${scientific(v, digits)}${sep}${pu}`;
}

/** Plain number for table cells: fixed-point or JS exponent form. */
export function formatNumber(v: number | null, digits = 5): string {
  if (v === null || !Number.isFinite(v)) return '';
  if (v === 0) return '0';
  const a = Math.abs(v);
  if (a >= 1e-3 && a < 1e6) return sig(v, digits);
  return v.toExponential(digits - 1);
}

export function formatHz(f: number): string {
  if (f >= 1000) return `${sig(f / 1000, 4)} kHz`;
  return `${sig(f, 4)} Hz`;
}

/** Compact axis label for a frequency: 20, 500, 1k, 2.5k, 20k. */
export function formatHzTick(f: number): string {
  if (f >= 1000) {
    const k = f / 1000;
    return `${Number(k.toPrecision(3))}k`;
  }
  return `${Number(f.toPrecision(3))}`;
}

// ----- ticks ------------------------------------------------------------

/** Evenly spaced ticks at a "nice" step chosen from `mults` × 10^n. */
export function niceTicks(
  lo: number,
  hi: number,
  target = 5,
  mults: number[] = [1, 2, 2.5, 5],
): { ticks: number[]; step: number } {
  if (!(hi > lo)) {
    const d = Math.abs(lo) > 0 ? Math.abs(lo) * 0.1 : 1;
    lo -= d;
    hi += d;
  }
  const raw = (hi - lo) / target;
  const e = Math.floor(Math.log10(raw));
  let step = 10 ** e * mults[mults.length - 1];
  for (const m of mults) {
    const s = m * 10 ** e;
    if (s >= raw) {
      step = s;
      break;
    }
  }
  const first = Math.ceil(lo / step - 1e-9) * step;
  const ticks: number[] = [];
  for (let t = first; t <= hi + step * 1e-9; t += step) ticks.push(Math.abs(t) < step * 1e-9 ? 0 : t);
  return { ticks, step };
}

/** Decade ticks, with 2 and 5 multiples when the span is short. */
export function logTicks(lo: number, hi: number): { major: number[]; minor: number[] } {
  const e0 = Math.floor(Math.log10(lo));
  const e1 = Math.ceil(Math.log10(hi));
  const span = Math.log10(hi / lo);
  const major: number[] = [];
  const minor: number[] = [];
  for (let e = e0; e <= e1; e++) {
    for (let m = 1; m <= 9; m++) {
      const v = m * 10 ** e;
      if (v < lo * (1 - 1e-9) || v > hi * (1 + 1e-9)) continue;
      const isMajor = m === 1 || (span < 2.5 && (m === 2 || m === 5)) || span < 0.7;
      (isMajor ? major : minor).push(v);
    }
  }
  return { major, minor };
}

/** Frequency ticks for a log axis: every m·10^k as a gridline; labelled ones by span. */
export function freqTicks(lo: number, hi: number): { labelled: number[]; grid: number[]; decades: number[] } {
  const span = Math.log10(hi / lo);
  const labelMults = span > 2.2 ? [1, 2, 5] : span > 0.9 ? [1, 2, 3, 5, 7] : [1, 2, 3, 4, 5, 6, 7, 8, 9];
  const labelled: number[] = [];
  const grid: number[] = [];
  const decades: number[] = [];
  for (let e = Math.floor(Math.log10(lo)); e <= Math.ceil(Math.log10(hi)); e++) {
    for (let m = 1; m <= 9; m++) {
      const f = m * 10 ** e;
      if (f < lo * (1 - 1e-9) || f > hi * (1 + 1e-9)) continue;
      if (m === 1) decades.push(f);
      else grid.push(f);
      if (labelMults.includes(m)) labelled.push(f);
    }
  }
  // 40 kHz is the top of the internal range; label it when in view.
  if (hi >= 39_999 && lo < 40_000 && !labelled.includes(40_000)) labelled.push(40_000);
  if (span < 0.3) {
    // Very narrow views: add intermediate labels between the gridlines.
    const { ticks } = niceTicks(lo, hi, 4);
    for (const t of ticks) if (t > 0 && !labelled.includes(t)) labelled.push(t);
    labelled.sort((a, b) => a - b);
  }
  return { labelled, grid, decades };
}
