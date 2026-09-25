// Shapes of the JSON documents the engine returns (crates/acoustilab-wasm).
// Numbers the engine could not represent (NaN, ±inf) arrive as null.

export type Domain = 'electrical' | 'mechanical' | 'acoustic';

export interface AirState {
  temperature_k: number;
  p0: number;
  rho: number;
  c: number;
  mu: number;
  gamma: number;
  prandtl: number;
}

/**
 * Validity shading of a result: light above `begin_hz` and below
 * `low_begin_hz`, dark above `deep_hz` and below `low_deep_hz` (the lowest
 * upper limits and the highest lower limits over all elements). The low
 * fields are absent in results of engines older than the parametric one.
 */
export interface Shading {
  begin_hz: number | null;
  deep_hz: number | null;
  low_begin_hz?: number | null;
  low_deep_hz?: number | null;
}

export interface ValidityLimit {
  element: string;
  criterion: string;
  begin_hz: number | null;
  deep_hz: number | null;
  low_begin_hz?: number | null;
  low_deep_hz?: number | null;
}

/** How a result is driven (`meta.drive`; docs/netlist.md, "Drive"). */
export interface DriveInfo {
  convention: 'voltage' | 'power' | 'characteristic' | 'current' | 'netlist';
  label: string;
  source_voltage_V: number | null;
  source_impedance_ohm: number | null;
  rated_ohm: number | null;
  probe: string | null;
}

/** An element note or an operating limit exceeded at the stated drive. */
export interface Warning {
  code: string;
  severity: 'info' | 'warning';
  element: string | null;
  message: string;
  f_min_Hz: number | null;
  f_max_Hz: number | null;
  value: number | null;
  at_Hz: number | null;
  limit: number | null;
  unit: string | null;
}

export type Scalar = number | boolean | string;

export interface Tolerance {
  rel?: number;
  abs?: number;
  dist: 'normal' | 'uniform' | 'lognormal';
  source?: string;
}

/** One entry of `parameters()` (docs/parameters.md). */
export interface ParamDesc {
  name: string;
  kind: 'number' | 'integer' | 'boolean' | 'choice' | 'derived';
  /** Resolved value (derived ones included). */
  value: Scalar | null;
  /** Value declared in the netlist (absent for derived parameters). */
  default?: Scalar;
  min?: number | null;
  max?: number | null;
  step?: number | null;
  log?: boolean;
  choices?: { value: string; label?: string }[];
  label: string;
  group: string | null;
  unit: string | null;
  description: string | null;
  advanced: boolean;
  tolerance: Tolerance | null;
  /** Expression of a derived parameter. */
  expr?: string;
}

/** Sketch binding in the `ui` block (docs/web.md, "The ui block"). */
export interface SketchSpec {
  kind: string;
  /** Sketch slot -> parameter name. */
  bind: Record<string, string>;
  /** Sketch part -> further parameters that drive it (hover highlighting). */
  parts?: Record<string, string[]>;
}

export interface UiBlock {
  template?: string;
  primary_probe?: string;
  /** Name of the parameter whose value (a choice label) names the ear load. */
  ear_load?: string;
  sketch?: SketchSpec;
}

export interface ParamsDoc {
  parameters: ParamDesc[];
  ui: UiBlock | null;
}

export type Num = number | null;

export interface ProbeResult {
  id: string;
  quantity: string;
  /** SI unit in the engine's spelling ("Pa", "m^3/s", "ohm"); "" if unknown. */
  unit: string;
  domain: Domain | null;
  re: Num[];
  im: Num[];
  magnitude: Num[];
  phase_deg: Num[];
  /** Present for acoustic pressures: 20·log10(|p| / 20 µPa), RMS. */
  spl_dB?: Num[];
}

export interface SolveResult {
  meta: {
    engine: string;
    level: number;
    air: AirState;
    drive?: DriveInfo;
    /** Resolved parameter values, derived ones included. */
    parameters?: Record<string, Scalar>;
  };
  frequencies_Hz: number[];
  probes: ProbeResult[];
  validity: ValidityLimit[];
  shading: Shading;
  warnings?: Warning[];
}

export interface CheckResult {
  ok: true;
  nodes: number;
  elements: number;
  unknowns: number;
  probes: number;
  frequencies: number;
  f_min_Hz: number;
  f_max_Hz: number;
  level: number;
  air: AirState;
  shading: Shading;
}

export interface EngineError {
  error: string;
  kind: 'json' | 'netlist' | 'element' | 'unknown_type' | 'singular' | 'probe' | 'parameter' | 'panic' | 'other';
  element?: string;
  type?: string;
  probe?: string;
  node?: string;
  line?: number;
  column?: number;
  f_Hz?: number;
  unknown?: string;
  parameter?: string;
}

export function isError(v: unknown): v is EngineError {
  return typeof v === 'object' && v !== null && typeof (v as EngineError).error === 'string';
}
