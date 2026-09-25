// Shapes of the analysis and target documents the engine returns
// (docs/analysis.md, docs/targets.md). Non-finite numbers arrive as null.

import type { DriveInfo, Shading, Tolerance } from '../types';

type Num = number | null;

// ----- readouts ------------------------------------------------------------

export interface AtFrequency {
  f_Hz: number;
  value: number;
  /** 0 unshaded, 1 light, 2 dark. */
  shading: number;
}

export interface ZPeak {
  f_Hz: number;
  z_ohm: number;
  prominence_dB: number;
}

export interface QEstimate {
  Re_ohm: number;
  f_Hz: number;
  r0: number;
  f1_Hz: number;
  f2_Hz: number;
  Qms: number;
  Qes: number;
  Qts: number;
  notes: string[];
}

export interface CoupledResonance {
  f_Hz: number;
  driver: string;
  prominence_dB: number;
  competing: { f_Hz: number; margin_dB: number } | null;
  ambiguous: boolean;
  robust: boolean;
  shading: number;
}

export interface Readouts {
  drive: DriveInfo;
  impedance: {
    probe: string;
    Re_ohm: number;
    re_source: string;
    peaks: ZPeak[];
    resonance: ZPeak | null;
    z_max: ZPeak | null;
    q: QEstimate | null;
    z_1kHz_ohm: Num;
    min_above_resonance: AtFrequency | null;
    min_at_sweep_end: boolean;
    rated_check: {
      rated_ohm: number;
      limit_ohm: number;
      z_min_ohm: number;
      z_min_Hz: number;
      pass: boolean;
      violations: { f_min_Hz: number; f_max_Hz: number }[];
    } | null;
    notes: string[];
  } | null;
  drivers: {
    element: string;
    model: string;
    d0: { fs_Hz: number; Qms: number; Qes: number; Qts: number; Re_ohm: number; Bl_Tm: number; Mms_kg: number; Cms_m_per_N: number; Rms_Ns_per_m: number; Sd_m2: number };
    free_air: QEstimate | null;
    notes: string[];
  }[];
  response: {
    probe: string;
    probe_source: string;
    level_500Hz_dB: number;
    level_1kHz_dB: number;
    sensitivity: { f_Hz: number; dB_per_V: number; dB_per_mW: Num; rated_ohm: Num; conversion_dB: Num }[];
    bass_extension: AtFrequency | null;
    coupled_resonance: CoupledResonance | null;
    notes: string[];
  } | null;
  notes: string[];
  methods: Record<string, string>;
  hash: string;
  engine: string;
}

// ----- sensitivity, tornado, explain ----------------------------------------

export interface Excluded {
  name: string;
  reason: string;
}

export interface Jacobian {
  method: string;
  step: number;
  frequencies_Hz: number[];
  probes: string[];
  parameters: {
    name: string;
    label: string;
    value: number;
    unit: string | null;
    scheme: 'central' | 'forward' | 'backward';
    note?: string;
    warnings: string[];
    dB_per_pct: Num[][];
    deg_per_pct: Num[][];
  }[];
  excluded: Excluded[];
  shading: Shading;
  drive: DriveInfo;
  hash: string;
  engine: string;
}

export type TornadoMetric =
  | { kind: 'level'; probe?: string; f_Hz: number }
  | { kind: 'band_mean'; probe?: string; f_min_Hz: number; f_max_Hz: number }
  | { kind: 'readout'; name: string };

export interface TornadoRow {
  name: string;
  label: string;
  value: number;
  low_value: number;
  high_value: number;
  basis: 'tolerance' | 'assumed';
  tolerance: Tolerance | null;
  clipped_low: boolean;
  clipped_high: boolean;
  metric_low: Num;
  metric_high: Num;
  delta_low: Num;
  delta_high: Num;
  effect: number;
  topology_changed: boolean;
  errors: string[];
}

export interface Tornado {
  metric: TornadoMetric;
  description: string;
  unit: string;
  base: number;
  shading: number | null;
  rows: TornadoRow[];
  excluded: Excluded[];
  drive: DriveInfo;
  hash: string;
  engine: string;
}

export interface ExplainBand {
  f_min_Hz: number;
  f_max_Hz: number;
  effect: 'raises' | 'lowers';
  mean_dB: number;
  max_abs_dB: number;
  at_Hz: number;
  first: number;
  last: number;
}

export interface Explanation {
  probe: string;
  step_pct: number;
  threshold_dB: number;
  statistic: string;
  frequencies_Hz: number[];
  credible: boolean[];
  base_resonance: CoupledResonance | null;
  sentences: {
    text: string;
    parameter: string;
    label: string;
    direction: 'raise' | 'lower';
    from_value: number;
    to_value: number;
    effect_dB: number;
    bands: ExplainBand[];
    stated: number[];
    resonance: { from_Hz: number; to_Hz: number } | null;
    delta_dB: Num[];
  }[];
  quiet: { name: string; effect_dB: number }[];
  skipped: Excluded[];
  drive: DriveInfo;
  hash: string;
  engine: string;
}

// ----- Monte Carlo ------------------------------------------------------------

export interface Distribution {
  name: string;
  dist: 'normal' | 'uniform' | 'lognormal';
  nominal: number;
  half_width: number;
  sigma: Num;
  sigma_ln: Num;
  min: Num;
  max: Num;
  clipped: number;
  source: string | null;
}

export interface Plan {
  method: string;
  seed: number | null;
  parameters: string[];
  distributions: Distribution[];
  samples: { index: number; overrides: Record<string, unknown>; clipped?: string[] }[];
  clipped: number;
  engine: string;
}

export interface RunSample {
  index: number;
  overrides: Record<string, unknown>;
  hash?: string;
  ok: boolean;
  error?: string;
  frequencies_Hz?: number[];
  curves?: { id: string; dB?: Num[]; magnitude?: Num[]; phase_deg?: Num[] }[];
  metrics?: Record<string, Num>;
}

export interface RunChunk {
  engine: string;
  frequencies_Hz: number[];
  samples: RunSample[];
}

export interface Stats {
  median: Num;
  p5: Num;
  p10: Num;
  p90: Num;
  p95: Num;
  min: Num;
  max: Num;
  n: number;
}

export interface CurveStats {
  median: Num[];
  p5: Num[];
  p10: Num[];
  p90: Num[];
  p95: Num[];
  min: Num[];
  max: Num[];
  n: number[];
}

export interface Envelope {
  frequencies_Hz: number[];
  runs: number;
  failed: number;
  other_grid: number;
  probes: { id: string; dB?: CurveStats; magnitude?: CurveStats; phase_deg?: CurveStats }[];
  metrics: Record<string, Stats>;
  percentile_method: string;
}

// ----- targets --------------------------------------------------------------------

export interface TargetSummary {
  name: string;
  label: string;
  group: string;
  primary: boolean;
  fixture: string;
  fixture_label: string;
  family: string;
  flags: string[];
  licence: string;
  provenance_class: string;
  valid_range_Hz: [number, number] | null;
}

export interface Fixture {
  id: string;
  label: string;
  ear_simulator: string;
  pinna: string;
  engine_types: string[];
  human_valid_Hz?: [number, number];
  source: string;
}

export interface TargetsList {
  targets: TargetSummary[];
  fixtures: Fixture[];
  models: { id: string; label: string; kind: string; training: { fixture: string; target_family: string }; source: string }[];
  smoothing_fractions: number[];
}

/** A full target object (`target()`, `import_target_csv()`). */
export interface TargetObject {
  name: string;
  label: string;
  version?: string;
  family: string;
  group?: string;
  fixture: string;
  reference_point?: string;
  baseline?: string;
  normalisation_Hz?: number;
  valid_range_Hz?: [number, number] | null;
  provenance: {
    class: string;
    source?: string | null;
    doi?: string | null;
    url?: string | null;
    licence?: string | null;
    retrieved?: string | null;
    attribution?: string | null;
  };
  flags: string[];
  notes?: string[] | string | null;
  frequencies_Hz: number[];
  dB: number[];
}

export interface Flag {
  code: string;
  message: string;
}

export interface Stat {
  band_Hz: [number, number];
  used_Hz: [number, number] | null;
  n: number;
  partial: boolean;
  rms_dB: Num;
  sd_dB?: Num;
  slope_dB_per_ln_f?: Num;
  abs_slope?: Num;
  slope_dB_per_octave?: Num;
  mean_dB: Num;
  mae_dB?: Num;
  max_abs_dB?: Num;
  max_abs_at_Hz?: Num;
}

export interface MaskStat {
  band_Hz: [number, number];
  n: number;
  within: number;
  compliance_percent: Num;
  partial: boolean;
  worst: { f_Hz: number; excess_dB: number } | null;
}

export interface Score {
  model: string;
  label: string;
  kind: string;
  score: Num;
  greyed: boolean;
  variables: { variable: string; value: Num; weight: number; band_Hz: [number, number]; used_Hz: [number, number] | null; n: number }[];
  formula: string;
  training: { fixture: string; target_family: string };
  fit: { r: number; rmse: number; observations?: number; listeners?: number } | null;
  source: string;
  flags: Flag[];
}

export interface Report {
  target: { name: string; label: string; fixture: string; family: string; flags: string[]; licence: string; provenance_class: string; valid_range_Hz: [number, number] | null };
  response: { label: string; fixture: string | null; fixture_label: string | null; drive: string | null; reference_level_dB: Num; measured: boolean };
  fixture_match: 'same' | 'same_ear_simulator' | 'different' | null;
  options: { normalisation: unknown; smoothing: string; tracking_smoothing: string; personalisation: unknown; grid: string };
  grid_Hz: number[];
  response_dB: Num[];
  target_dB: Num[];
  error_dB: Num[];
  target_offset_dB: Num;
  metrics: {
    main: Stat;
    above_10kHz: Stat;
    band_rms: Stat[];
    bs708_mask: MaskStat;
    preference_band: MaskStat;
  };
  preference_band: { lower_dB: Num[]; upper_dB: Num[]; about: { classes: { id: string; label: string; share: number; bass_dB: [number, number] }[]; widening: unknown; source: string; widening_basis: string } };
  tracking: unknown;
  scores: Score[];
  coupler_extrapolated_Hz: [number, number] | null;
  flags: Flag[];
}
