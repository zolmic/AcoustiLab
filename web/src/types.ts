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

export interface Shading {
  begin_hz: number | null;
  deep_hz: number | null;
}

export interface ValidityLimit {
  element: string;
  criterion: string;
  begin_hz: number | null;
  deep_hz: number | null;
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
  meta: { engine: string; level: number; air: AirState };
  frequencies_Hz: number[];
  probes: ProbeResult[];
  validity: ValidityLimit[];
  shading: Shading;
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
  kind: 'json' | 'netlist' | 'element' | 'unknown_type' | 'singular' | 'probe' | 'panic' | 'other';
  element?: string;
  type?: string;
  probe?: string;
  node?: string;
  line?: number;
  column?: number;
  f_Hz?: number;
  unknown?: string;
}

export function isError(v: unknown): v is EngineError {
  return typeof v === 'object' && v !== null && typeof (v as EngineError).error === 'string';
}
