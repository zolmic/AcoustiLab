// The fit report (`acoustilab-fit-report/0.1`, docs/fitting.md, "The
// report") as tables and lists: fitted values with their 95 % intervals and
// statuses, level offsets, residual statistics per curve, the singular
// values and named directions, the Section 12 findings, the correlation
// matrix, and the engine's summary and warnings. Every sentence here is the
// engine's.

import { formatHz, formatNumber, formatParam, paramUnit } from '../format';
import { defList, el, scrollRegion, table, words } from './measure-kit';

export interface ParamReport {
  name: string;
  label: string | null;
  unit: string | null;
  scale: string;
  start: number;
  value: number;
  ci95: [number, number] | null;
  sd: number | null;
  status: string;
  roles: string[];
  notes: string[];
}

export interface CurveReport {
  index: number;
  probe: string;
  quantity: string;
  points: number;
  f_min_Hz: number;
  f_max_Hz: number;
  rms_dB: number;
  rms_deg: number | null;
  rms_ohm: number | null;
  weighted_rms: number;
  lag1_autocorrelation: number;
  runs: { observed: number; expected: number; z: number };
  structured: boolean;
  inflation: number;
  residuals: { frequencies_Hz: number[]; level_dB: number[]; phase_deg: number[] | null };
}

export interface FitReport {
  schema: string;
  converged: boolean;
  stop: string;
  stop_reason: string;
  iterations: number;
  evaluations: number;
  failed_evaluations: number;
  cost: number;
  degrees_of_freedom: number;
  reduced_chi2: number | null;
  covariance_scale: number;
  parameters: ParamReport[];
  offsets: { curve: number; probe: string; value_dB: number; ci95_dB: [number, number] | null; prior_dB: number | null }[];
  correlation: { names: string[]; matrix: (number | null)[][] };
  curves: CurveReport[];
  identifiability: {
    rank_tolerance: number;
    singular_values: number[];
    directions: { sigma: number; relative: number; sd: number | null; status: string; components: { name: string; weight: number }[]; text: string }[];
    rules: { code: string; parameters: string[]; message: string; resolve_with?: string[] }[];
  };
  fitted: Record<string, number>;
  summary: string[];
  warnings: string[];
}

/** Totals of a fit run in several bounded calls (each resuming from the last one's values). */
export interface RunInfo {
  calls: number;
  iterations: number;
  evaluations: number;
  failed: number;
  /** Start values of the first call. */
  starts: Map<string, number>;
  /** Names of the fitted curves, by index. */
  curveNames: string[];
  cancelled: boolean;
}

export function statusClass(status: string): string {
  if (status === 'determined') return 'mv-status-determined';
  if (status === 'weakly_determined') return 'mv-status-weak';
  return 'mv-status-bad';
}

const withUnit = (v: number, unit: string | null) => `${formatParam(v, 6)}${unit ? ` ${paramUnit(unit)}` : ''}`;

export function renderReport(r: FitReport, run: RunInfo): HTMLElement {
  const root = el('div', { class: 'mv-report' });
  root.append(
    el(
      'p',
      { class: 'mv-summary', attrs: { 'data-field': 'stop' } },
      el('strong', { text: run.cancelled ? 'Cancelled: ' : r.converged ? 'Converged: ' : 'Not converged: ' }),
      `${r.stop_reason}. `,
      `${run.iterations} iterations and ${run.evaluations} model evaluations in ${run.calls} call${run.calls === 1 ? '' : 's'}` +
        (run.failed ? ` (${run.failed} trial points could not be evaluated)` : '') +
        `; χ² ${formatNumber(r.cost, 4)} over ${r.degrees_of_freedom} degrees of freedom, reduced χ² ${formatNumber(r.reduced_chi2, 4)}` +
        (r.covariance_scale > 1 ? ` (the intervals are widened by √${formatNumber(r.covariance_scale, 3)})` : '') +
        '.',
    ),
  );
  if (r.summary.length) root.append(el('ul', { class: 'mv-sentences', attrs: { 'aria-label': 'Summary (engine)' } }, ...r.summary.map((s) => el('li', { text: s }))));

  // Parameters.
  const prow = r.parameters.map((p) => {
    const name = el('span', {}, p.label ? `${p.label} ` : '', el('code', { text: p.name }));
    const extra = [...p.roles.map((x) => `role: ${x}`), ...p.notes];
    const status = el('span', { class: statusClass(p.status), text: words(p.status) });
    return [
      extra.length ? el('span', {}, name, el('br'), el('span', { class: 'hint', text: extra.join('; ') })) : name,
      withUnit(run.starts.get(p.name) ?? p.start, p.unit),
      el('strong', { text: withUnit(p.value, p.unit) }),
      p.ci95 ? `${formatParam(p.ci95[0], 6)} to ${formatParam(p.ci95[1], 6)}` : '—',
      status,
      p.scale,
    ];
  });
  const pt = table(
    'Fitted parameters: value, 95 % interval (only for determined and weakly determined parameters), status',
    ['Parameter', 'Start', 'Fitted', '95 % interval', 'Status', 'Scale'],
    prow,
    { rowHead: true, cls: 'mv-fit-params' },
  );
  pt.querySelectorAll('tbody tr').forEach((tr, k) => ((tr as HTMLElement).dataset.param = r.parameters[k].name));
  root.append(scrollRegion('Fitted parameters', pt));

  if (r.offsets.length) {
    root.append(
      scrollRegion(
        'Level offsets',
        table(
          'Level offsets of the curves (nuisance parameters), dB',
          ['Curve', 'Offset', '95 % interval', 'Prior (1σ)'],
          r.offsets.map((o) => [
            run.curveNames[o.curve] ?? `curve ${o.curve} (${o.probe})`,
            `${formatNumber(o.value_dB, 4)} dB`,
            o.ci95_dB ? `${formatNumber(o.ci95_dB[0], 4)} to ${formatNumber(o.ci95_dB[1], 4)} dB` : '—',
            o.prior_dB === null ? 'free' : `${formatNumber(o.prior_dB, 3)} dB`,
          ]),
          { rowHead: true },
        ),
      ),
    );
  }

  // Curves.
  root.append(
    scrollRegion(
      'Residuals per curve',
      table(
        'Residuals per curve (model − measurement): RMS in dB, degrees and ohm; weighted RMS (about 1 when the budget is right); lag-1 autocorrelation; runs test',
        ['Curve', 'Points', 'Band', 'RMS', 'Weighted RMS', 'Lag-1 ρ', 'Runs z', 'Structured', 'Inflation'],
        r.curves.map((c) => [
          `${run.curveNames[c.index] ?? `curve ${c.index}`} → ${c.probe}`,
          String(c.points),
          `${formatHz(c.f_min_Hz)}–${formatHz(c.f_max_Hz)}`,
          [`${formatNumber(c.rms_dB, 3)} dB`, c.rms_deg !== null ? `${formatNumber(c.rms_deg, 3)}°` : null, c.rms_ohm !== null ? `${formatNumber(c.rms_ohm, 3)} Ω` : null]
            .filter(Boolean)
            .join(', '),
          formatNumber(c.weighted_rms, 3),
          formatNumber(c.lag1_autocorrelation, 3),
          formatNumber(c.runs.z, 3),
          c.structured ? el('strong', { text: 'yes' }) : 'no',
          formatNumber(c.inflation, 3),
        ]),
        { rowHead: true },
      ),
    ),
  );

  // Identifiability.
  const id = r.identifiability;
  const dirs = el(
    'ol',
    { class: 'mv-sentences', attrs: { 'aria-label': 'Singular directions' } },
    ...id.directions.map((d) =>
      el(
        'li',
        { attrs: { 'data-status': d.status } },
        el('span', { class: statusClass(d.status), text: `σ = ${formatNumber(d.sigma, 4)} (${formatNumber(d.relative, 3)} of the largest), ${words(d.status)}: ` }),
        d.text,
      ),
    ),
  );
  const rules = id.rules.length
    ? el(
        'ul',
        { class: 'mv-sentences', attrs: { 'aria-label': 'Section 12 findings' } },
        ...id.rules.map((f) =>
          el(
            'li',
            { attrs: { 'data-code': f.code } },
            el('strong', { text: `${words(f.code)}: ` }),
            f.message,
            f.resolve_with?.length ? el('ul', {}, ...f.resolve_with.map((x) => el('li', { text: `Resolve with ${x}` }))) : '',
          ),
        ),
      )
    : el('p', { class: 'hint', text: 'No Section 12 finding.' });
  root.append(
    el('h4', { class: 'mv-h', text: 'Identifiability' }),
    defList([
      ['Singular values', id.singular_values.map((s) => formatNumber(s, 4)).join(', ')],
      ['Null below', `${formatNumber(id.rank_tolerance, 2)} of the largest`],
    ]),
    dirs,
    el('h4', { class: 'mv-h', text: 'Section 12 findings (scale ambiguity, added mass, SPL-only)' }),
    rules,
  );

  const c = r.correlation;
  if (c.names.length > 1) {
    root.append(
      scrollRegion(
        'Correlation matrix',
        table(
          'Correlation of the fitted variables (blank: an unidentifiable variable)',
          ['', ...c.names],
          c.names.map((n, i) => [n, ...c.matrix[i].map((v) => (v === null ? '' : formatNumber(v, 3)))]),
          { rowHead: true },
        ),
      ),
    );
  }
  if (r.warnings.length) {
    root.append(
      el(
        'section',
        { class: 'mv-card mv-warn', attrs: { 'aria-labelledby': 'fit-warn-h' } },
        el('h4', { id: 'fit-warn-h', text: `Warnings (${r.warnings.length})` }),
        el('ul', {}, ...r.warnings.map((w) => el('li', { text: w }))),
      ),
    );
  }
  return root;
}
