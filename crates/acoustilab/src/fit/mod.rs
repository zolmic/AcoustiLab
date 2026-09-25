//! Parameter identification from measured curves (spec Section 12,
//! "Parameter identification from measurements", "Identifiability is
//! enforced, not assumed", "Measurement uncertainty budget"; case study 2
//! of Section 18). Full description in docs/fitting.md.
//!
//! [`fit`] adjusts chosen continuous parameters of a parametric netlist so
//! that its probes reproduce measured curves ([`crate::io::Curve`]), by
//! weighted least squares:
//!
//! * **Impedance** (and displacement or velocity) curves contribute the
//!   level error 20·log10|Z_model/Z| in dB and the phase error
//!   arg(Z_model/Z) in degrees, each divided by its standard uncertainty.
//!   Together they are the complex logarithm ln(Z_model/Z), which is the
//!   relative complex error (Z_model − Z)/Z to first order: every point
//!   counts in proportion to its relative error whether |Z| is 32 Ω or
//!   300 Ω, where residuals on the real and imaginary parts in ohm would
//!   let the resonance peak dominate. The two parts can be weighted
//!   separately, as analysers specify magnitude and phase accuracy
//!   separately.
//! * **Pressure** curves contribute the level error in dB. Their phase is
//!   used only on request, since a measured acoustic phase carries the
//!   unknown time of flight.
//! * The weights come from the curve's uncertainty budget
//!   ([`crate::io::sidecar::Uncertainty`]); without one, 1 % (0.086 dB) for
//!   impedance and 0.5 dB for other curves are assumed.
//! * A pressure curve without a stated drive, or declared uncalibrated,
//!   gets a free level offset (a nuisance parameter, reported).
//!
//! The optimiser is Levenberg–Marquardt ([`lm`]) in log-parameter space
//! (linear for parameters that may be zero or negative) within the bounds
//! of the parameter declarations, the Jacobian by central differences of
//! full solves ([`jacobian`]), and optional
//! multi-start from a Latin hypercube. The report ([`FitReport`]) gives
//! fitted values with 95 % intervals from the Jacobian covariance, the
//! correlation matrix, residual statistics per curve, the singular values
//! of the weighted Jacobian with the unidentifiable directions named
//! ([`identify`]), and the driver rules of Section 12 ([`roles`]).

pub mod dense;
pub mod identify;
pub mod jacobian;
pub mod lm;
pub mod rig;
pub mod rng;
pub mod roles;

use crate::circuit::{Circuit, ProbeKind};
use crate::drive::DriveSpec;
use crate::error::Error;
use crate::expr::PValue;
use crate::io::curve::wrap_deg;
use crate::io::sidecar::Drive;
use crate::io::{Curve, CurveError, Quantity};
use crate::params::{Overrides, Parametric};
use crate::C64;
use dense::Mat;
use identify::{Level, Z95};
use lm::{Bound, LmOptions, Problem, Stop};
use roles::{CurveUse, DataKind, Finding};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::f64::consts::LN_10;
use std::fmt;

/// Schema tag of fit specifications.
pub const FIT_SCHEMA: &str = "acoustilab-fit/0.1";
/// Schema tag of fit reports.
pub const REPORT_SCHEMA: &str = "acoustilab-fit-report/0.1";

/// Level uncertainty assumed for an impedance curve whose sidecar states
/// none: 1 % of |Z|, 20·log10(1.01) dB.
pub const DEFAULT_IMPEDANCE_DB: f64 = 0.086_427_475_653_970_6;
/// Level uncertainty assumed for other curves without a budget, dB.
pub const DEFAULT_LEVEL_DB: f64 = 0.5;
/// Smallest level uncertainty used as a weight, dB (and its phase
/// equivalent); guards against a budget of zero.
pub const MIN_LEVEL_DB: f64 = 1e-3;

/// Singular value of the weighted Jacobian below which the optimiser does
/// not step along a direction: moving e-fold along it changes χ² by less
/// than 1, so the data do not determine it and the fit leaves it at its
/// start ([`lm::LmOptions::min_sigma`]).
pub const MIN_SIGMA: f64 = 1.0;

/// The fit has converged when three accepted steps together lower χ² by
/// less than this: far below the Δχ² = 1 of a one-standard-deviation move.
pub const STALL_CHI2: f64 = 1e-3;

/// Phase uncertainty in degrees equivalent to a level uncertainty in dB
/// (the same relative size of a complex error): u_φ = u_L·(ln 10/20)·(180/π).
pub fn phase_equivalent_deg(level_db: f64) -> f64 {
    level_db * LN_10 / 20.0 * 180.0 / std::f64::consts::PI
}

/// Error of a fit or virtual-rig request.
#[derive(Debug)]
pub enum FitError {
    /// The fit or rig specification is malformed.
    Spec(String),
    /// A curve or sidecar problem.
    Curve(CurveError),
    /// The engine rejected the netlist or a parameter.
    Engine(Error),
    /// The data cannot determine what was asked, and the fit is refused
    /// (spec Section 12).
    Refused(String),
}

impl fmt::Display for FitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FitError::Spec(m) => write!(f, "fit: {m}"),
            FitError::Curve(e) => write!(f, "curve: {e}"),
            FitError::Engine(e) => write!(f, "{e}"),
            FitError::Refused(m) => write!(f, "fit refused: {m}"),
        }
    }
}

impl std::error::Error for FitError {}

impl From<Error> for FitError {
    fn from(e: Error) -> Self {
        FitError::Engine(e)
    }
}

impl From<CurveError> for FitError {
    fn from(e: CurveError) -> Self {
        FitError::Curve(e)
    }
}

fn spec_err(msg: impl Into<String>) -> FitError {
    FitError::Spec(msg.into())
}

/// How a parameter is varied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// In ln p: the parameter stays positive and steps are relative.
    Log,
    /// In p: for parameters that may be zero or negative.
    Linear,
}

/// A parameter to fit.
#[derive(Debug, Clone, PartialEq)]
pub struct FitParam {
    pub name: String,
    /// Starting value (default: the value under the fit's overrides).
    pub start: Option<f64>,
    /// Narrower bounds than the declaration's.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Default: log when the value is positive and the minimum is not
    /// negative, else linear.
    pub scale: Option<Scale>,
}

impl FitParam {
    pub fn new(name: &str) -> FitParam {
        FitParam {
            name: name.to_string(),
            start: None,
            min: None,
            max: None,
            scale: None,
        }
    }
}

/// Level offset of a curve (a nuisance parameter added to the model's
/// level in dB).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Offset {
    None,
    Free,
    /// Free with a Gaussian prior of this standard deviation, dB.
    Prior(f64),
}

/// A measured curve and the probe it is compared with.
#[derive(Debug, Clone)]
pub struct CurveSpec {
    pub probe: String,
    pub curve: Curve,
    /// Parameter values of the measurement condition (e.g. an added mass).
    pub overrides: Overrides,
    /// Extra weight on this curve's residuals (default 1).
    pub weight: f64,
    /// Fit the phase too (default: yes for impedance, displacement and
    /// velocity curves that have one, no for pressure).
    pub use_phase: Option<bool>,
    pub offset: Option<Offset>,
    pub f_min: Option<f64>,
    pub f_max: Option<f64>,
    /// Sidecar checks to override: `compensation` fits a compensated curve
    /// to the uncompensated model probe anyway.
    pub allow: Vec<String>,
}

impl CurveSpec {
    pub fn new(probe: &str, curve: Curve) -> CurveSpec {
        CurveSpec {
            probe: probe.to_string(),
            curve,
            overrides: Overrides::new(),
            weight: 1.0,
            use_phase: None,
            offset: None,
            f_min: None,
            f_max: None,
            allow: Vec::new(),
        }
    }
}

/// A fit request (JSON form `acoustilab-fit/0.1`, docs/fitting.md).
#[derive(Debug, Clone)]
pub struct FitSpec {
    pub parameters: Vec<FitParam>,
    /// Fixed parameter values for every curve.
    pub overrides: Overrides,
    pub curves: Vec<CurveSpec>,
    /// Band, Hz (default 10 Hz to 20 kHz, spec Section 12).
    pub f_min: f64,
    pub f_max: f64,
    pub max_iterations: usize,
    /// Cap on model evaluations over all starts, Jacobian columns included.
    pub max_evaluations: usize,
    /// Number of starts: the nominal one plus Latin-hypercube ones.
    pub starts: usize,
    pub seed: u64,
    /// Fit driver parameters to pressure curves alone (refused by default).
    pub allow_spl_only: bool,
    /// Relative singular value below which a direction is null.
    pub rank_tolerance: f64,
}

impl FitSpec {
    pub fn new(parameters: Vec<FitParam>, curves: Vec<CurveSpec>) -> FitSpec {
        FitSpec {
            parameters,
            overrides: Overrides::new(),
            curves,
            f_min: 10.0,
            f_max: 20_000.0,
            max_iterations: 100,
            max_evaluations: 5_000,
            starts: 1,
            seed: 1,
            allow_spl_only: false,
            rank_tolerance: 1e-6,
        }
    }

    /// Reads the JSON form (docs/fitting.md). Unknown keys are errors.
    pub fn from_json(v: &Value) -> Result<FitSpec, FitError> {
        let o = v
            .as_object()
            .ok_or_else(|| spec_err("the fit specification must be a JSON object"))?;
        const KEYS: [&str; 12] = [
            "schema",
            "parameters",
            "overrides",
            "curves",
            "f_min_Hz",
            "f_max_Hz",
            "max_iterations",
            "max_evaluations",
            "starts",
            "seed",
            "allow_spl_only",
            "rank_tolerance",
        ];
        if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(spec_err(format!(
                "unknown key '{k}' (known: {})",
                KEYS.join(", ")
            )));
        }
        if let Some(s) = o.get("schema") {
            if s.as_str() != Some(FIT_SCHEMA) {
                return Err(spec_err(format!("schema must be '{FIT_SCHEMA}', got {s}")));
            }
        }
        let params = o
            .get("parameters")
            .and_then(Value::as_array)
            .ok_or_else(|| spec_err("'parameters' must be an array of names or objects"))?
            .iter()
            .map(parse_param)
            .collect::<Result<Vec<_>, _>>()?;
        let curves = o
            .get("curves")
            .and_then(Value::as_array)
            .ok_or_else(|| spec_err("'curves' must be an array"))?
            .iter()
            .enumerate()
            .map(|(i, c)| parse_curve_spec(c).map_err(|e| prefix(e, &format!("curves[{i}]"))))
            .collect::<Result<Vec<_>, _>>()?;
        let mut s = FitSpec::new(params, curves);
        if let Some(ov) = o.get("overrides") {
            s.overrides = parse_overrides(ov, "overrides")?;
        }
        s.f_min = num_opt(o, "f_min_Hz")?.unwrap_or(s.f_min);
        s.f_max = num_opt(o, "f_max_Hz")?.unwrap_or(s.f_max);
        s.max_iterations = count_opt(o, "max_iterations")?.unwrap_or(s.max_iterations);
        s.max_evaluations = count_opt(o, "max_evaluations")?.unwrap_or(s.max_evaluations);
        s.starts = count_opt(o, "starts")?.unwrap_or(s.starts);
        s.seed = match o.get("seed") {
            None => s.seed,
            Some(v) => v
                .as_u64()
                .ok_or_else(|| spec_err("'seed' must be a non-negative integer"))?,
        };
        s.allow_spl_only = match o.get("allow_spl_only") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(spec_err("'allow_spl_only' must be true or false")),
        };
        if let Some(t) = num_opt(o, "rank_tolerance")? {
            if !(t > 0.0 && t < 1.0) {
                return Err(spec_err("'rank_tolerance' must be in (0, 1)"));
            }
            s.rank_tolerance = t;
        }
        Ok(s)
    }
}

fn prefix(e: FitError, at: &str) -> FitError {
    match e {
        FitError::Spec(m) => FitError::Spec(format!("{at}: {m}")),
        FitError::Curve(c) => FitError::Curve(CurveError {
            line: c.line,
            msg: format!("{at}: {}", c.msg),
        }),
        e => e,
    }
}

fn num_opt(o: &Map<String, Value>, k: &str) -> Result<Option<f64>, FitError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map(Some)
            .ok_or_else(|| spec_err(format!("'{k}' must be a finite number"))),
    }
}

fn count_opt(o: &Map<String, Value>, k: &str) -> Result<Option<usize>, FitError> {
    match o.get(k) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| Some(n as usize))
            .ok_or_else(|| spec_err(format!("'{k}' must be a positive integer"))),
    }
}

/// `{"name": value}` overrides.
pub fn parse_overrides(v: &Value, what: &str) -> Result<Overrides, FitError> {
    let o = v
        .as_object()
        .ok_or_else(|| spec_err(format!("'{what}' must be an object of parameter values")))?;
    o.iter()
        .map(|(k, x)| {
            PValue::from_json(x).map(|p| (k.clone(), p)).ok_or_else(|| {
                spec_err(format!("{what}: '{k}' must be a number, boolean or string"))
            })
        })
        .collect()
}

fn parse_param(v: &Value) -> Result<FitParam, FitError> {
    match v {
        Value::String(s) => Ok(FitParam::new(s)),
        Value::Object(o) => {
            const KEYS: [&str; 5] = ["name", "start", "min", "max", "scale"];
            if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
                return Err(spec_err(format!(
                    "parameter: unknown key '{k}' (known: {})",
                    KEYS.join(", ")
                )));
            }
            let name = o
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| spec_err("a parameter object needs 'name'"))?;
            let scale = match o.get("scale").map(|s| s.as_str()) {
                None => None,
                Some(Some("log")) => Some(Scale::Log),
                Some(Some("linear")) => Some(Scale::Linear),
                Some(_) => {
                    return Err(spec_err(format!(
                        "parameter '{name}': scale must be 'log' or 'linear'"
                    )))
                }
            };
            Ok(FitParam {
                name: name.to_string(),
                start: num_opt(o, "start")?,
                min: num_opt(o, "min")?,
                max: num_opt(o, "max")?,
                scale,
            })
        }
        _ => Err(spec_err("each parameter is a name or an object")),
    }
}

fn parse_curve_spec(v: &Value) -> Result<CurveSpec, FitError> {
    let o = v
        .as_object()
        .ok_or_else(|| spec_err("a curve entry must be an object"))?;
    const KEYS: [&str; 9] = [
        "probe",
        "curve",
        "overrides",
        "weight",
        "use_phase",
        "offset",
        "f_min_Hz",
        "f_max_Hz",
        "allow",
    ];
    if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(spec_err(format!(
            "unknown key '{k}' (known: {})",
            KEYS.join(", ")
        )));
    }
    let probe = o
        .get("probe")
        .and_then(Value::as_str)
        .ok_or_else(|| spec_err("a curve entry needs 'probe'"))?;
    let curve = Curve::from_json(
        o.get("curve")
            .ok_or_else(|| spec_err("a curve entry needs 'curve'"))?,
    )?;
    let mut c = CurveSpec::new(probe, curve);
    if let Some(ov) = o.get("overrides") {
        c.overrides = parse_overrides(ov, "overrides")?;
    }
    if let Some(w) = num_opt(o, "weight")? {
        c.weight = w;
    }
    c.use_phase = match o.get("use_phase") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(spec_err("'use_phase' must be true or false")),
    };
    c.offset = match o.get("offset") {
        None => None,
        Some(Value::String(s)) if s == "none" => Some(Offset::None),
        Some(Value::String(s)) if s == "free" => Some(Offset::Free),
        Some(Value::Object(p)) if p.len() == 1 && p.contains_key("prior_dB") => {
            let s = num_opt(p, "prior_dB")?.unwrap_or(0.0);
            if s <= 0.0 {
                return Err(spec_err("'prior_dB' must be positive"));
            }
            Some(Offset::Prior(s))
        }
        Some(_) => {
            return Err(spec_err(
                "'offset' must be \"none\", \"free\" or {\"prior_dB\": x}",
            ))
        }
    };
    c.f_min = num_opt(o, "f_min_Hz")?;
    c.f_max = num_opt(o, "f_max_Hz")?;
    if let Some(a) = o.get("allow") {
        c.allow = a
            .as_array()
            .and_then(|a| a.iter().map(|x| x.as_str().map(String::from)).collect())
            .ok_or_else(|| spec_err("'allow' must be an array of sidecar field names"))?;
        if let Some(f) = c.allow.iter().find(|f| f.as_str() != "compensation") {
            return Err(spec_err(format!(
                "'allow': '{f}' cannot be overridden (only 'compensation')"
            )));
        }
    }
    Ok(c)
}

// ----- The model behind the residuals ---------------------------------------

/// One fitted network parameter.
#[derive(Debug, Clone)]
struct Var {
    name: String,
    scale: Scale,
    /// Unit of u for a linear-scale parameter, u = p/unit: the bounded
    /// range, else max(|start|, 1). A step of one in u is then a large
    /// change, as a step of one in ln p is.
    unit_u: f64,
    /// Bound in u (ln p or p/unit_u).
    bound: Bound,
    /// Declared bounds in p, for clamping.
    lo: Option<f64>,
    hi: Option<f64>,
    step: f64,
    start: f64,
    unit: Option<String>,
    label: Option<String>,
}

impl Var {
    fn value(&self, u: f64) -> f64 {
        let v = match self.scale {
            Scale::Log => u.exp(),
            Scale::Linear => u * self.unit_u,
        };
        let v = self.lo.map_or(v, |lo| v.max(lo));
        self.hi.map_or(v, |hi| v.min(hi))
    }

    fn u(&self, v: f64) -> f64 {
        match self.scale {
            Scale::Log => v.ln(),
            Scale::Linear => v / self.unit_u,
        }
    }
}

/// A measurement condition: overrides and drive shared by some curves.
#[derive(Debug, Clone)]
struct Cond {
    overrides: Overrides,
    drive: Option<DriveSpec>,
    freqs: Vec<f64>,
}

/// A curve prepared for the residuals.
#[derive(Debug, Clone)]
struct Prepared {
    probe: String,
    quantity: Quantity,
    cond: usize,
    freqs: Vec<f64>,
    /// Position of each frequency in the condition's list.
    pos: Vec<usize>,
    level: Vec<f64>,
    mag: Vec<f64>,
    phase: Option<Vec<f64>>,
    u_level: Vec<f64>,
    u_phase: Vec<f64>,
    sqrt_w: f64,
    offset: Option<usize>,
    /// Row range of the curve's residuals.
    rows: std::ops::Range<usize>,
}

struct Model<'a> {
    p: &'a Parametric,
    vars: Vec<Var>,
    priors: Vec<Option<f64>>,
    conds: Vec<Cond>,
    curves: Vec<Prepared>,
    rows: usize,
    cache: Option<(Vec<u64>, Vec<Vec<C64>>)>,
}

impl Model<'_> {
    fn n_phys(&self) -> usize {
        self.vars.len()
    }

    fn overrides_for(&self, cond: usize, u_phys: &[f64]) -> Overrides {
        let mut ov = self.conds[cond].overrides.clone();
        for (v, u) in self.vars.iter().zip(u_phys) {
            ov.insert(v.name.clone(), PValue::Num(v.value(*u)));
        }
        ov
    }

    /// Model values of every curve at its frequencies.
    fn values(&mut self, u_phys: &[f64]) -> Result<Vec<Vec<C64>>, String> {
        let key: Vec<u64> = u_phys.iter().map(|x| x.to_bits()).collect();
        if let Some((k, v)) = &self.cache {
            if *k == key {
                return Ok(v.clone());
            }
        }
        let mut out = vec![Vec::new(); self.curves.len()];
        for ci in 0..self.conds.len() {
            let ov = self.overrides_for(ci, u_phys);
            let mut c = Circuit::from_parametric(self.p, &ov).map_err(|e| e.to_string())?;
            c.freqs = self.conds[ci].freqs.clone();
            if let Some(d) = &self.conds[ci].drive {
                c.drive = Some(d.clone());
            }
            let r = c.solve().map_err(|e| e.to_string())?;
            for (k, pc) in self
                .curves
                .iter()
                .enumerate()
                .filter(|(_, pc)| pc.cond == ci)
            {
                let pr = r
                    .probe(&pc.probe)
                    .ok_or_else(|| format!("probe '{}' is not in the network", pc.probe))?;
                out[k] = pc.pos.iter().map(|&i| pr.values[i]).collect();
            }
        }
        self.cache = Some((key, out.clone()));
        Ok(out)
    }

    fn residuals_from(&self, vals: &[Vec<C64>], offsets: &[f64]) -> Vec<f64> {
        let mut r = vec![0.0; self.rows];
        for (pc, v) in self.curves.iter().zip(vals) {
            let off = pc.offset.map_or(0.0, |o| offsets[o]);
            let reference = pc.quantity.db_reference();
            let n = pc.freqs.len();
            let base = pc.rows.start;
            for i in 0..n {
                let lm = 20.0 * (v[i].norm() / reference).log10() + off;
                r[base + i] = (lm - pc.level[i]) / pc.u_level[i] * pc.sqrt_w;
                if let Some(ph) = &pc.phase {
                    let d = wrap_deg(v[i].arg().to_degrees() - ph[i]);
                    r[base + n + i] = d / pc.u_phase[i] * pc.sqrt_w;
                }
            }
        }
        let prior_rows = self.rows - self.priors.iter().flatten().count();
        for (k, s) in self
            .priors
            .iter()
            .enumerate()
            .filter_map(|(k, s)| s.map(|s| (k, s)))
            .enumerate()
        {
            r[prior_rows + k] = offsets[s.0] / s.1;
        }
        r
    }

    /// Analytic Jacobian columns of the offsets.
    fn offset_columns(&self, jac: &mut Mat) {
        let n = self.n_phys();
        for pc in &self.curves {
            if let Some(o) = pc.offset {
                for i in 0..pc.freqs.len() {
                    jac.set(pc.rows.start + i, n + o, pc.sqrt_w / pc.u_level[i]);
                }
            }
        }
        let prior_rows = self.rows - self.priors.iter().flatten().count();
        for (k, (o, s)) in self
            .priors
            .iter()
            .enumerate()
            .filter_map(|(o, s)| s.map(|s| (o, s)))
            .enumerate()
        {
            jac.set(prior_rows + k, n + o, 1.0 / s);
        }
    }
}

impl Problem for Model<'_> {
    fn residuals(&mut self, u: &[f64]) -> Result<Vec<f64>, String> {
        let n = self.n_phys();
        let vals = self.values(&u[..n])?;
        Ok(self.residuals_from(&vals, &u[n..]))
    }

    fn jacobian(&mut self, u: &[f64], r: &[f64], bounds: &[Bound]) -> Result<(Mat, usize), String> {
        let n = self.n_phys();
        let offsets = u[n..].to_vec();
        let steps: Vec<f64> = self.vars.iter().map(|v| v.step).collect();
        let (phys, evals) = {
            let mut f = |x: &[f64]| -> Result<Vec<f64>, String> {
                let v = self.values(x)?;
                Ok(self.residuals_from(&v, &offsets))
            };
            jacobian::central_differences(&mut f, &u[..n], r, &steps, &|k, x| {
                bounds[k].contains(x)
            })?
        };
        let mut jac = Mat::zeros(r.len(), u.len());
        for row in 0..r.len() {
            for k in 0..n {
                jac.set(row, k, phys.get(row, k));
            }
        }
        self.offset_columns(&mut jac);
        Ok((jac, evals))
    }
}

// ----- Report -----------------------------------------------------------------

/// Status of a fitted parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// 95 % interval within ±25 %.
    Determined,
    /// 95 % interval within a factor of 2.
    WeaklyDetermined,
    /// Wider than a factor of 2.
    Undetermined,
    /// In a direction no curve responds to.
    Unidentifiable,
    /// The Section 12 rule: impedance alone cannot fix Bl, Mms, Cms (and Sd).
    ScaleAmbiguous,
    /// The optimum lies at or beyond a bound; the interval is not meaningful.
    AtBound,
}

impl Status {
    pub fn is_determined(self) -> bool {
        matches!(self, Status::Determined | Status::WeaklyDetermined)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ParamReport {
    pub name: String,
    pub label: Option<String>,
    pub unit: Option<String>,
    pub scale: &'static str,
    pub start: f64,
    pub value: f64,
    /// 95 % interval; absent unless the parameter is determined or weakly
    /// determined.
    pub ci95: Option<[f64; 2]>,
    /// Standard deviation of ln(value) (log scale) or of value/|value|
    /// (linear scale), from the covariance.
    pub sd: Option<f64>,
    pub status: Status,
    /// Driver roles of the parameter (e.g. "Mms of 'drv'").
    pub roles: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OffsetReport {
    pub curve: usize,
    pub probe: String,
    #[serde(rename = "value_dB")]
    pub value_db: f64,
    #[serde(rename = "ci95_dB")]
    pub ci95_db: Option<[f64; 2]>,
    #[serde(rename = "prior_dB")]
    pub prior_db: Option<f64>,
}

/// Wald–Wolfowitz runs test on the signs of a residual sequence.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Runs {
    pub observed: usize,
    pub expected: f64,
    /// (observed − expected)/σ; strongly negative means too few sign
    /// changes (structured residuals).
    pub z: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Residuals {
    #[serde(rename = "frequencies_Hz")]
    pub frequencies_hz: Vec<f64>,
    /// Model minus measurement, dB (offset included).
    #[serde(rename = "level_dB")]
    pub level_db: Vec<f64>,
    #[serde(rename = "phase_deg")]
    pub phase_deg: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CurveReport {
    pub index: usize,
    pub probe: String,
    pub quantity: &'static str,
    pub points: usize,
    #[serde(rename = "f_min_Hz")]
    pub f_min_hz: f64,
    #[serde(rename = "f_max_Hz")]
    pub f_max_hz: f64,
    /// RMS level residual, dB.
    #[serde(rename = "rms_dB")]
    pub rms_db: f64,
    #[serde(rename = "rms_deg")]
    pub rms_deg: Option<f64>,
    /// RMS of |Z_model| − |Z|, ohm (impedance curves).
    #[serde(rename = "rms_ohm")]
    pub rms_ohm: Option<f64>,
    /// RMS of the weighted residuals (about 1 when the budget is right).
    pub weighted_rms: f64,
    /// Lag-1 autocorrelation of the weighted residuals in frequency order.
    pub lag1_autocorrelation: f64,
    pub runs: Runs,
    /// The residuals follow a pattern over frequency rather than scatter:
    /// model-form error or correlated measurement error.
    pub structured: bool,
    /// Factor by which this curve's information is divided in the
    /// covariance, (1 + ρ)/(1 − ρ) for a significant lag-1 autocorrelation ρ.
    pub inflation: f64,
    pub residuals: Residuals,
}

#[derive(Debug, Clone, Serialize)]
pub struct Identifiability {
    pub rank_tolerance: f64,
    pub singular_values: Vec<f64>,
    pub directions: Vec<identify::Direction>,
    /// Section 12 rules: scale ambiguity and how it is resolved, added-mass
    /// reliability, SPL-only overrides.
    pub rules: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartReport {
    pub start: Vec<f64>,
    pub cost: Option<f64>,
    pub stop: Option<Stop>,
    pub iterations: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Correlation {
    pub names: Vec<String>,
    /// Absent entries involve an unidentifiable variable.
    pub matrix: Vec<Vec<Option<f64>>>,
}

/// The result of [`fit`] (JSON form `acoustilab-fit-report/0.1`).
#[derive(Debug, Clone, Serialize)]
pub struct FitReport {
    pub schema: &'static str,
    pub converged: bool,
    pub stop: Stop,
    pub stop_reason: &'static str,
    pub iterations: usize,
    pub evaluations: usize,
    /// Trial points the model could not be evaluated at (rejected steps).
    pub failed_evaluations: usize,
    pub starts: Vec<StartReport>,
    /// Σ of squared weighted residuals.
    pub cost: f64,
    pub degrees_of_freedom: i64,
    pub reduced_chi2: Option<f64>,
    /// s² applied to the covariance: max(reduced χ², 1).
    pub covariance_scale: f64,
    pub parameters: Vec<ParamReport>,
    pub offsets: Vec<OffsetReport>,
    pub correlation: Correlation,
    pub curves: Vec<CurveReport>,
    pub identifiability: Identifiability,
    /// `{name: fitted value}`, ready to use as overrides.
    pub fitted: Value,
    pub summary: Vec<String>,
    pub warnings: Vec<String>,
}

impl FitReport {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn parameter(&self, name: &str) -> Option<&ParamReport> {
        self.parameters.iter().find(|p| p.name == name)
    }
}

fn runs_test(e: &[f64]) -> Runs {
    let signs: Vec<bool> = e.iter().filter(|x| **x != 0.0).map(|x| *x > 0.0).collect();
    let n = signs.len();
    let np = signs.iter().filter(|s| **s).count() as f64;
    let nm = n as f64 - np;
    let observed = if n == 0 {
        0
    } else {
        1 + signs.windows(2).filter(|w| w[0] != w[1]).count()
    };
    if n < 2 || np == 0.0 || nm == 0.0 {
        return Runs {
            observed,
            expected: observed as f64,
            z: if n >= 2 { -(n as f64).sqrt() } else { 0.0 },
        };
    }
    let nf = n as f64;
    let mu = 2.0 * np * nm / nf + 1.0;
    let var = (mu - 1.0) * (mu - 2.0) / (nf - 1.0);
    Runs {
        observed,
        expected: mu,
        z: (observed as f64 - mu) / var.sqrt().max(1e-300),
    }
}

/// Uncentred lag-1 autocorrelation Σ e_i·e_{i+1} / Σ e_i².
fn lag1(e: &[f64]) -> f64 {
    let den: f64 = e.iter().map(|x| x * x).sum();
    if e.len() < 2 || den == 0.0 {
        return 0.0;
    }
    e.windows(2).map(|w| w[0] * w[1]).sum::<f64>() / den
}

// ----- The fit ------------------------------------------------------------------

fn probe_quantity(c: &Circuit, probe: &str) -> Option<(Quantity, DataKind, Option<String>)> {
    let pr = c.probes.iter().find(|p| p.id == probe)?;
    Some(match pr.kind {
        ProbeKind::Impedance { .. } => (Quantity::Impedance, DataKind::Impedance, None),
        _ if pr.is_pressure => (Quantity::Pressure, DataKind::Pressure, None),
        ProbeKind::Displacement(i) => (
            Quantity::Displacement,
            DataKind::Mechanical,
            Some(c.nodes.name(i).to_string()),
        ),
        ProbeKind::Node(i) if pr.quantity == "velocity" => (
            Quantity::Velocity,
            DataKind::Mechanical,
            Some(c.nodes.name(i).to_string()),
        ),
        _ => (Quantity::Generic, DataKind::Other, None),
    })
}

/// Fits the parameters of `spec` to its curves (module documentation).
pub fn fit(p: &Parametric, spec: &FitSpec) -> Result<FitReport, FitError> {
    if spec.parameters.is_empty() {
        return Err(spec_err("no parameters to fit"));
    }
    if spec.curves.is_empty() {
        return Err(spec_err("no curves to fit"));
    }
    if !(spec.f_min > 0.0 && spec.f_max > spec.f_min) {
        return Err(spec_err("the band needs 0 < f_min_Hz < f_max_Hz"));
    }
    let base_values = p.values(&spec.overrides)?;
    let mut warnings = Vec::new();
    // Fitted parameters.
    let mut vars = Vec::new();
    for fp in &spec.parameters {
        let name = &fp.name;
        let def = p
            .def(name)
            .ok_or_else(|| spec_err(format!("parameter '{name}' is not declared")))?;
        if !def.is_continuous() {
            return Err(spec_err(format!(
                "parameter '{name}' is not a continuous number (integers, choices, booleans and derived parameters cannot be fitted)"
            )));
        }
        if vars.iter().any(|v: &Var| &v.name == name) {
            return Err(spec_err(format!("parameter '{name}' is listed twice")));
        }
        if spec.overrides.contains_key(name)
            || spec.curves.iter().any(|c| c.overrides.contains_key(name))
        {
            return Err(spec_err(format!(
                "parameter '{name}' is both fitted and fixed by an override"
            )));
        }
        let (dmin, dmax) = def.bounds();
        let lo = match (dmin, fp.min) {
            (Some(a), Some(b)) if b < a => {
                return Err(spec_err(format!(
                    "'{name}': min {b} is below the declared minimum {a}"
                )))
            }
            (a, b) => b.or(a),
        };
        let hi = match (dmax, fp.max) {
            (Some(a), Some(b)) if b > a => {
                return Err(spec_err(format!(
                    "'{name}': max {b} is above the declared maximum {a}"
                )))
            }
            (a, b) => b.or(a),
        };
        if let (Some(a), Some(b)) = (lo, hi) {
            if a >= b {
                return Err(spec_err(format!(
                    "'{name}': the bounds [{a}, {b}] leave nothing to fit"
                )));
            }
        }
        let start = match fp.start {
            Some(s) => s,
            None => base_values
                .iter()
                .find(|(n, _)| n == name)
                .and_then(|(_, v)| v.as_num())
                .ok_or_else(|| spec_err(format!("parameter '{name}' has no numeric value")))?,
        };
        if lo.is_some_and(|a| start < a) || hi.is_some_and(|b| start > b) {
            return Err(spec_err(format!(
                "'{name}': start {start} is outside its bounds"
            )));
        }
        let scale = fp
            .scale
            .unwrap_or(if start > 0.0 && lo.is_none_or(|a| a >= 0.0) {
                Scale::Log
            } else {
                Scale::Linear
            });
        let (bound, unit_u) = match scale {
            Scale::Log => {
                if start <= 0.0 || lo.is_some_and(|a| a < 0.0) {
                    return Err(spec_err(format!(
                        "'{name}': a log-scale parameter needs a positive start and a non-negative minimum; give 'start' or use \"scale\": \"linear\""
                    )));
                }
                (
                    Bound::new(lo.filter(|a| *a > 0.0).map(f64::ln), hi.map(f64::ln)),
                    1.0,
                )
            }
            Scale::Linear => {
                let unit = match (lo, hi) {
                    (Some(a), Some(b)) => b - a,
                    _ => start.abs().max(1.0),
                };
                (Bound::new(lo.map(|a| a / unit), hi.map(|b| b / unit)), unit)
            }
        };
        let step = 1e-4;
        let var = Var {
            name: name.clone(),
            scale,
            unit_u,
            bound,
            lo,
            hi,
            step,
            start,
            unit: def.display_unit(),
            label: def.label.clone(),
        };
        vars.push(var);
    }
    let starts_ov: Overrides = vars
        .iter()
        .map(|v| (v.name.clone(), PValue::Num(v.start)))
        .collect();
    let with_starts = |ov: &Overrides| -> Overrides {
        let mut o = ov.clone();
        o.extend(starts_ov.clone());
        o
    };
    // Conditions and curves.
    let base_circuit = Circuit::from_parametric(p, &with_starts(&spec.overrides))?;
    let netlist_drive = base_circuit.drive.clone().map(|d| Drive::from_spec(&d));
    let mut conds: Vec<Cond> = Vec::new();
    let mut prepared = Vec::new();
    let mut priors: Vec<Option<f64>> = Vec::new();
    let mut uses = Vec::new();
    let mut rows = 0;
    for (ci, cs) in spec.curves.iter().enumerate() {
        let at = |m: String| spec_err(format!("curves[{ci}] (probe '{}'): {m}", cs.probe));
        if !(cs.weight.is_finite() && cs.weight > 0.0) {
            return Err(at("the weight must be positive".into()));
        }
        let mut ov = spec.overrides.clone();
        for (k, v) in &cs.overrides {
            ov.insert(k.clone(), v.clone());
        }
        let circuit = Circuit::from_parametric(p, &with_starts(&ov))?;
        let (q, kind, node) = probe_quantity(&circuit, &cs.probe)
            .ok_or_else(|| at("no such probe in the netlist".into()))?;
        let curve = &cs.curve;
        if curve.quantity != q {
            return Err(at(format!(
                "the curve is {} but the probe reads {}",
                curve.quantity.name(),
                q.name()
            )));
        }
        let sc = &curve.sidecar;
        // The model's probes are uncompensated, unsmoothed responses.
        if let Some(comp) = &sc.compensation {
            if comp.trim().to_lowercase() != "none" && !cs.allow.iter().any(|a| a == "compensation")
            {
                return Err(at(format!(
                    "the sidecar says the curve is compensated ('{comp}'), but the model's probe is not; fit the uncompensated measurement, or allow 'compensation' explicitly"
                )));
            }
        }
        if let Some(crate::io::sidecar::Smoothing::Octave(n)) = sc.smoothing {
            if n < 6 {
                warnings.push(format!(
                    "curve {ci} ({}): the measurement is smoothed to 1/{n} octave and the model is not; narrow peaks and dips will not match",
                    cs.probe
                ));
            }
        }
        // Drive and level offset.
        let mut offset = cs.offset;
        let mut drive = None;
        if q != Quantity::Impedance {
            match &sc.drive {
                Some(d) => {
                    if !netlist_drive.as_ref().is_some_and(|n| n.same_as(d)) {
                        drive = Some(d.spec.clone());
                    }
                }
                None => {
                    if offset.is_none() {
                        warnings.push(format!(
                            "curve {ci} ({}): the sidecar states no drive, so its level is fitted as a free offset",
                            cs.probe
                        ));
                        offset = Some(Offset::Free);
                    }
                }
            }
            if offset.is_none() && sc.calibrated == Some(false) {
                offset = Some(Offset::Free);
            }
        }
        let offset = offset.unwrap_or(Offset::None);
        let use_phase = cs
            .use_phase
            .unwrap_or(q != Quantity::Pressure && curve.phase_deg.is_some());
        if use_phase && curve.phase_deg.is_none() {
            return Err(at("use_phase is set but the curve has no phase".into()));
        }
        let (f_lo, f_hi) = (
            cs.f_min.unwrap_or(spec.f_min).max(spec.f_min),
            cs.f_max.unwrap_or(spec.f_max).min(spec.f_max),
        );
        let idx = curve.indices_within(f_lo, f_hi);
        if idx.len() < 2 {
            return Err(at(format!(
                "fewer than 2 points between {f_lo} and {f_hi} Hz"
            )));
        }
        let freqs: Vec<f64> = idx.iter().map(|&i| curve.freqs_hz[i]).collect();
        let shading = circuit.shading();
        let dark: Vec<f64> = freqs
            .iter()
            .copied()
            .filter(|&f| shading.band(f) == 2)
            .collect();
        if !dark.is_empty() {
            warnings.push(format!(
                "curve {ci} ({}): {} of {} points ({} to {} Hz) lie where the model is outside its validity (dark shading); residuals there measure the model, not the data: consider narrowing f_min_Hz/f_max_Hz",
                cs.probe,
                dark.len(),
                freqs.len(),
                dark[0],
                dark[dark.len() - 1]
            ));
        }
        let level_all = curve.level_db();
        let seat = sc.seatings_or_one();
        let default_db = if q == Quantity::Impedance {
            DEFAULT_IMPEDANCE_DB
        } else {
            DEFAULT_LEVEL_DB
        };
        let unc = sc.uncertainty.as_ref().filter(|u| !u.is_empty());
        let u_level: Vec<f64> = freqs
            .iter()
            .map(|&f| {
                unc.and_then(|u| u.level_db(f, seat))
                    .unwrap_or(default_db)
                    .max(MIN_LEVEL_DB)
            })
            .collect();
        let u_phase: Vec<f64> = freqs
            .iter()
            .zip(&u_level)
            .map(|(&f, &ul)| {
                unc.and_then(|u| u.phase_deg(f, seat))
                    .unwrap_or_else(|| phase_equivalent_deg(ul))
                    .max(phase_equivalent_deg(MIN_LEVEL_DB))
            })
            .collect();
        // Condition.
        let cond = match conds
            .iter()
            .position(|c| c.overrides == ov && c.drive == drive)
        {
            Some(k) => k,
            None => {
                conds.push(Cond {
                    overrides: ov.clone(),
                    drive: drive.clone(),
                    freqs: Vec::new(),
                });
                conds.len() - 1
            }
        };
        let offset_index = match offset {
            Offset::None => None,
            Offset::Free => {
                priors.push(None);
                Some(priors.len() - 1)
            }
            Offset::Prior(s) => {
                priors.push(Some(s));
                Some(priors.len() - 1)
            }
        };
        let n = freqs.len();
        let nrows = if use_phase { 2 * n } else { n };
        prepared.push(Prepared {
            probe: cs.probe.clone(),
            quantity: q,
            cond,
            pos: Vec::new(),
            level: idx.iter().map(|&i| level_all[i]).collect(),
            mag: idx.iter().map(|&i| curve.magnitude[i]).collect(),
            phase: use_phase.then(|| {
                idx.iter()
                    .map(|&i| curve.phase_deg.as_ref().unwrap()[i])
                    .collect()
            }),
            freqs,
            u_level,
            u_phase,
            sqrt_w: cs.weight.sqrt(),
            offset: offset_index,
            rows: rows..rows + nrows,
        });
        rows += nrows;
        uses.push(CurveUse {
            kind,
            overrides: with_starts(&ov),
            absolute: offset == Offset::None || matches!(offset, Offset::Prior(_)),
            node,
        });
        if let (Some(zs), Some(model_zs)) = (sc.source_impedance_ohm, source_impedance(&circuit)) {
            if (zs - model_zs).abs() > 1e-9 * zs.abs().max(model_zs.abs()) {
                warnings.push(format!(
                    "curve {ci} ({}): measured with a source impedance of {zs} ohm, but the netlist's source has {model_zs} ohm",
                    cs.probe
                ));
            }
        }
    }
    rows += priors.iter().flatten().count();
    for (k, c) in conds.iter_mut().enumerate() {
        let mut f: Vec<f64> = prepared
            .iter()
            .filter(|pc| pc.cond == k)
            .flat_map(|pc| pc.freqs.iter().copied())
            .collect();
        f.sort_by(f64::total_cmp);
        f.dedup();
        c.freqs = f;
    }
    for pc in &mut prepared {
        let cf = &conds[pc.cond].freqs;
        pc.pos = pc
            .freqs
            .iter()
            .map(|f| cf.binary_search_by(|x| x.total_cmp(f)).expect("collected"))
            .collect();
    }
    let n_vars = vars.len() + priors.len();
    if rows < n_vars {
        return Err(spec_err(format!(
            "{rows} residuals cannot determine {n_vars} unknowns"
        )));
    }
    // Section 12 rules before any solve.
    let names: Vec<String> = vars.iter().map(|v| v.name.clone()).collect();
    let rules = roles::analyse(p, &names, &uses, spec.allow_spl_only)?;
    if let Some(msg) = rules.refusal {
        return Err(FitError::Refused(msg));
    }
    let mut model = Model {
        p,
        vars,
        priors,
        conds,
        curves: prepared,
        rows,
        cache: None,
    };
    // Starts.
    let n_phys = model.n_phys();
    let u0: Vec<f64> = model
        .vars
        .iter()
        .map(|v| v.u(v.start))
        .chain(std::iter::repeat_n(0.0, model.priors.len()))
        .collect();
    let bounds: Vec<Bound> = model
        .vars
        .iter()
        .map(|v| v.bound)
        .chain(std::iter::repeat_n(Bound::Free, model.priors.len()))
        .collect();
    let mut starts = vec![u0.clone()];
    if spec.starts > 1 {
        let mut rng = rng::Rng::new(spec.seed);
        let lhs = rng::latin_hypercube(&mut rng, spec.starts - 1, n_phys);
        for s in lhs {
            let mut u = u0.clone();
            for (k, v) in model.vars.iter().enumerate() {
                u[k] = match v.bound {
                    Bound::Both(a, b) => a + (b - a) * s[k],
                    _ => match v.scale {
                        Scale::Log => u0[k] + (2.0 * s[k] - 1.0) * std::f64::consts::LN_2,
                        Scale::Linear => u0[k] + (2.0 * s[k] - 1.0) * u0[k].abs().max(1.0) * 0.5,
                    },
                };
            }
            starts.push(u);
        }
    }
    let mut best: Option<lm::LmResult> = None;
    let mut start_reports = Vec::new();
    let mut evaluations = 0;
    let mut failed = 0;
    for s in &starts {
        let remaining = spec.max_evaluations.saturating_sub(evaluations);
        let start_values: Vec<f64> = model.vars.iter().zip(s).map(|(v, u)| v.value(*u)).collect();
        if remaining == 0 {
            start_reports.push(StartReport {
                start: start_values,
                cost: None,
                stop: None,
                iterations: 0,
                error: Some("evaluation budget exhausted".into()),
            });
            continue;
        }
        let o = LmOptions {
            max_iterations: spec.max_iterations,
            max_evaluations: remaining,
            ftol: 1e-10,
            xtol: 1e-9,
            cost_floor: 0.0,
            null_tolerance: 0.1 * spec.rank_tolerance,
            min_sigma: MIN_SIGMA,
            stall: (3, STALL_CHI2),
            mu0: 1e-3,
        };
        match lm::minimize(&mut model, s, &bounds, &o) {
            Ok(r) => {
                evaluations += r.evaluations;
                failed += r.failed_evaluations;
                start_reports.push(StartReport {
                    start: start_values,
                    cost: Some(r.cost),
                    stop: Some(r.stop),
                    iterations: r.iterations,
                    error: None,
                });
                if best.as_ref().is_none_or(|b| r.cost < b.cost) {
                    best = Some(r);
                }
            }
            Err(e) => {
                evaluations += 1;
                start_reports.push(StartReport {
                    start: start_values,
                    cost: None,
                    stop: None,
                    iterations: 0,
                    error: Some(e),
                });
            }
        }
    }
    let best = best.ok_or_else(|| {
        spec_err(format!(
            "no start could be evaluated: {}",
            start_reports
                .iter()
                .filter_map(|s| s.error.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ))
    })?;
    report(
        &mut model,
        spec,
        best,
        start_reports,
        evaluations,
        failed,
        rules,
        warnings,
    )
}

fn source_impedance(c: &Circuit) -> Option<f64> {
    c.elements
        .iter()
        .find_map(|e| {
            e.as_any()
                .downcast_ref::<crate::elements::electrical::VSource>()
        })
        .map(|v| v.zs)
}

#[allow(clippy::too_many_arguments)]
fn report(
    model: &mut Model,
    spec: &FitSpec,
    best: lm::LmResult,
    starts: Vec<StartReport>,
    mut evaluations: usize,
    failed: usize,
    rules: roles::RuleReport,
    mut warnings: Vec<String>,
) -> Result<FitReport, FitError> {
    let u = best.u.clone();
    let n_phys = model.n_phys();
    let n = u.len();
    let r = model.residuals(&u).map_err(spec_err)?;
    let bounds: Vec<Bound> = model
        .vars
        .iter()
        .map(|v| v.bound)
        .chain(std::iter::repeat_n(Bound::Free, model.priors.len()))
        .collect();
    let (mut jac, used) = model
        .jacobian(&u, &r, &bounds)
        .map_err(|e| spec_err(format!("Jacobian at the fitted point: {e}")))?;
    evaluations += used;
    // Report space: linear parameters as relative changes.
    let values: Vec<f64> = model
        .vars
        .iter()
        .zip(&u)
        .map(|(v, u)| v.value(*u))
        .collect();
    let col_scale: Vec<f64> = (0..n)
        .map(|k| match model.vars.get(k) {
            Some(v) if v.scale == Scale::Linear => values[k].abs().max(1e-300) / v.unit_u,
            _ => 1.0,
        })
        .collect();
    for (k, s) in col_scale.iter().enumerate() {
        jac.scale_col(k, *s);
    }
    // Residual statistics and inflation per curve.
    let vals = model.values(&u[..n_phys]).map_err(spec_err)?;
    let mut curve_reports = Vec::new();
    for (ci, (pc, v)) in model.curves.iter().zip(&vals).enumerate() {
        let npts = pc.freqs.len();
        let lvl = &r[pc.rows.start..pc.rows.start + npts];
        let mut rho = lag1(lvl);
        let mut runs = runs_test(lvl);
        let mut rms_deg = None;
        let mut phase_res = None;
        if pc.phase.is_some() {
            let ph = &r[pc.rows.start + npts..pc.rows.end];
            rho = rho.max(lag1(ph));
            let rp = runs_test(ph);
            if rp.z < runs.z {
                runs = rp;
            }
            let d: Vec<f64> = ph
                .iter()
                .zip(&pc.u_phase)
                .map(|(x, u)| x * u / pc.sqrt_w)
                .collect();
            rms_deg = Some((d.iter().map(|x| x * x).sum::<f64>() / npts as f64).sqrt());
            phase_res = Some(d);
        }
        let level_res: Vec<f64> = lvl
            .iter()
            .zip(&pc.u_level)
            .map(|(x, u)| x * u / pc.sqrt_w)
            .collect();
        let rms_db = (level_res.iter().map(|x| x * x).sum::<f64>() / npts as f64).sqrt();
        let rms_ohm = (pc.quantity == Quantity::Impedance).then(|| {
            (v.iter()
                .zip(&pc.mag)
                .map(|(z, m)| (z.norm() - m).powi(2))
                .sum::<f64>()
                / npts as f64)
                .sqrt()
        });
        let wr = &r[pc.rows.clone()];
        let weighted_rms = (wr.iter().map(|x| x * x).sum::<f64>() / wr.len() as f64).sqrt();
        let significant = 2.0 / (npts as f64).sqrt();
        let structured = runs.z < -3.0 || (rho > 3.0 / (npts as f64).sqrt() && rho > 0.3);
        let rc = rho.min(0.98);
        let inflation = if rc > significant {
            (1.0 + rc) / (1.0 - rc)
        } else {
            1.0
        };
        for row in pc.rows.clone() {
            jac.scale_row(row, 1.0 / inflation.sqrt());
        }
        curve_reports.push(CurveReport {
            index: ci,
            probe: pc.probe.clone(),
            quantity: pc.quantity.name(),
            points: npts,
            f_min_hz: pc.freqs[0],
            f_max_hz: pc.freqs[npts - 1],
            rms_db,
            rms_deg,
            rms_ohm,
            weighted_rms,
            lag1_autocorrelation: rho,
            runs,
            structured,
            inflation,
            residuals: Residuals {
                frequencies_hz: pc.freqs.clone(),
                level_db: level_res,
                phase_deg: phase_res,
            },
        });
        if structured {
            warnings.push(format!(
                "curve {ci} ({}): the residuals are structured (lag-1 autocorrelation {rho:.3}, runs z {:.3}): the model cannot follow the data (model-form error) or the measurement errors are correlated; intervals are inflated by {inflation:.3} and remain optimistic",
                pc.probe, runs.z
            ));
        }
    }
    let m = r.len();
    let dof = m as i64 - n as i64;
    let reduced = (dof > 0).then(|| best.cost / dof as f64);
    let s2 = reduced.map_or(1.0, |x| x.max(1.0));
    if dof <= 0 {
        warnings.push("as many unknowns as residuals: the residual variance cannot be estimated; intervals assume the stated budget".into());
    }
    let mut names: Vec<String> = model.vars.iter().map(|v| v.name.clone()).collect();
    for (o, _) in model.priors.iter().enumerate() {
        let pc = model
            .curves
            .iter()
            .position(|pc| pc.offset == Some(o))
            .expect("each offset belongs to a curve");
        names.push(format!("offset_dB[{pc}:{}]", model.curves[pc].probe));
    }
    let an = identify::analyse(&names, &jac, s2, spec.rank_tolerance);
    // Parameters.
    let mut params = Vec::new();
    let mut fitted = Map::new();
    for (k, v) in model.vars.iter().enumerate() {
        let value = values[k];
        fitted.insert(v.name.clone(), json!(value));
        let var_sd = an.covariance.get(k, k).max(0.0).sqrt();
        let unidentifiable = an.null_loading[k] > identify::NULL_LOADING;
        let near = match v.bound {
            Bound::Both(a, b) => 1e-3 * (b - a),
            _ => 1e-3,
        };
        let at_bound = v.bound.near_bound(u[k], near.max(1e-12));
        let sd_ln = match v.scale {
            Scale::Log => var_sd,
            // relative change of p
            Scale::Linear => var_sd,
        };
        let mut status = if unidentifiable {
            Status::Unidentifiable
        } else {
            match Level::from_sd(sd_ln) {
                Level::Determined => Status::Determined,
                Level::WeaklyDetermined => Status::WeaklyDetermined,
                _ => Status::Undetermined,
            }
        };
        let mut notes = Vec::new();
        if at_bound {
            notes.push(format!(
                "the fit stopped within 0.1 % of a bound ({}): the optimum may lie beyond it",
                match (v.lo, v.hi) {
                    (Some(a), Some(b)) => format!("{a} to {b}"),
                    (Some(a), None) => format!("min {a}"),
                    (None, Some(b)) => format!("max {b}"),
                    _ => String::new(),
                }
            ));
            status = Status::AtBound;
        }
        if rules.scale_ambiguous.contains(&v.name) {
            if status.is_determined() {
                notes.push("numerically the fit pins this value, but only through the model of the acoustic load and Sd, not through a scale datum".into());
            }
            status = Status::ScaleAmbiguous;
        }
        let ci95 = status.is_determined().then(|| match v.scale {
            Scale::Log => [value * (-Z95 * sd_ln).exp(), value * (Z95 * sd_ln).exp()],
            Scale::Linear => {
                let h = Z95 * sd_ln * value.abs();
                [value - h, value + h]
            }
        });
        let role_list: Vec<String> = rules
            .roles
            .iter()
            .filter(|r| r.param == v.name)
            .map(|r| format!("{} ({}) of '{}'", r.role.name(), r.key, r.element))
            .collect();
        params.push(ParamReport {
            name: v.name.clone(),
            label: v.label.clone(),
            unit: v.unit.clone(),
            scale: match v.scale {
                Scale::Log => "log",
                Scale::Linear => "linear",
            },
            start: v.start,
            value,
            ci95,
            sd: (!unidentifiable).then_some(sd_ln),
            status,
            roles: role_list,
            notes,
        });
    }
    let mut offsets = Vec::new();
    for (o, prior) in model.priors.iter().enumerate() {
        let k = n_phys + o;
        let ci = model
            .curves
            .iter()
            .position(|pc| pc.offset == Some(o))
            .unwrap();
        let sd = an.covariance.get(k, k).max(0.0).sqrt();
        let ok = an.null_loading[k] <= identify::NULL_LOADING;
        offsets.push(OffsetReport {
            curve: ci,
            probe: model.curves[ci].probe.clone(),
            value_db: u[k],
            ci95_db: ok.then(|| [u[k] - Z95 * sd, u[k] + Z95 * sd]),
            prior_db: *prior,
        });
    }
    let corr: Vec<Vec<Option<f64>>> = (0..n)
        .map(|a| {
            (0..n)
                .map(|b| {
                    let ok = an.null_loading[a] <= identify::NULL_LOADING
                        && an.null_loading[b] <= identify::NULL_LOADING;
                    let d = (an.covariance.get(a, a) * an.covariance.get(b, b)).sqrt();
                    (ok && d > 0.0).then(|| an.covariance.get(a, b) / d)
                })
                .collect()
        })
        .collect();
    // Added-mass reliability.
    let mut findings = rules.findings.clone();
    if rules.resolvers.contains(&roles::Resolver::AddedMass) {
        let fitted_lookup = |name: &str| -> Option<f64> {
            model
                .vars
                .iter()
                .position(|v| v.name == name)
                .map(|k| values[k])
                .or_else(|| {
                    spec.overrides
                        .get(name)
                        .and_then(PValue::as_num)
                        .or_else(|| {
                            model
                                .p
                                .def(name)
                                .and_then(|d| d.default_value())
                                .and_then(|v| v.as_num())
                        })
                })
        };
        let mut masses: Vec<(String, f64)> = Vec::new();
        let mut added: Vec<f64> = Vec::new();
        for c in &model.conds {
            let els = roles::elements(model.p, &c.overrides)?;
            for m in roles::moving_mass(&els, &|n: &str| {
                fitted_lookup(n).or_else(|| c.overrides.get(n).and_then(PValue::as_num))
            }) {
                if !masses.iter().any(|x| x.0 == m.0) {
                    masses.push(m);
                }
            }
            for (_, dm) in roles::test_masses(&els, &|n: &str| {
                c.overrides
                    .get(n)
                    .and_then(PValue::as_num)
                    .or_else(|| fitted_lookup(n))
            }) {
                added.push(dm);
            }
        }
        let dm = added.iter().copied().fold(0.0, f64::max);
        for (id, mms) in masses {
            if mms < roles::ADDED_MASS_MIN_MMS_KG {
                let mut message = format!(
                    "driver '{id}': the moving mass is {:.4} g, below about 0.5 g, so the added-mass method is unreliable (spec Section 12): a 10 mg adhesive dot is {:.3} % of it",
                    mms * 1e3,
                    1e-5 / mms * 100.0
                );
                if dm > 0.0 {
                    // Mms = Δm/((fs/fs')² − 1), so δMms/Mms = δΔm/Δm, and
                    // Bl ∝ sqrt(Mms) moves by half as much.
                    message += &format!(
                        ", and an error of 10 mg in the {:.4} g test mass alone moves Mms by {:.3} % and Bl by {:.3} %, beyond the intervals above, which take the test mass as exact",
                        dm * 1e3,
                        1e-5 / dm * 100.0,
                        0.5e-5 / dm * 100.0
                    );
                }
                findings.push(Finding {
                    code: "added_mass_unreliable",
                    parameters: Vec::new(),
                    message,
                    resolve_with: Vec::new(),
                });
            }
            if dm > 0.0 && dm < 0.2 * mms {
                findings.push(Finding {
                    code: "added_mass_small",
                    parameters: Vec::new(),
                    message: format!(
                        "driver '{id}': the test mass ({:.4} g) is {:.3} % of the moving mass; masses of 50 to 100 % of Mms shift fs enough to resolve it",
                        dm * 1e3,
                        dm / mms * 100.0
                    ),
                    resolve_with: Vec::new(),
                });
            }
        }
    }
    // Summary.
    let count = |s: Status| params.iter().filter(|p| p.status == s).count();
    let mut summary = vec![format!(
        "{} parameter(s) fitted: {} determined, {} weakly determined, {} undetermined, {} unidentifiable, {} scale-ambiguous, {} at a bound",
        params.len(),
        count(Status::Determined),
        count(Status::WeaklyDetermined),
        count(Status::Undetermined),
        count(Status::Unidentifiable),
        count(Status::ScaleAmbiguous),
        count(Status::AtBound)
    )];
    for d in an
        .directions
        .iter()
        .filter(|d| d.status != Level::Determined)
    {
        summary.push(d.text.clone());
    }
    for f in &findings {
        summary.push(f.message.clone());
    }
    if let Some(x) = reduced.filter(|x| *x > 10.0) {
        warnings.push(format!(
            "the residuals are far above the stated uncertainty (reduced chi-square {x:.4}): a local minimum, a model that cannot follow the data, or an optimistic budget; try more starts or check the model"
        ));
    }
    if !best.stop.converged() {
        warnings.push(format!(
            "{}; run again from the fitted values to continue",
            best.stop.describe()
        ));
    }
    Ok(FitReport {
        schema: REPORT_SCHEMA,
        converged: best.stop.converged(),
        stop: best.stop,
        stop_reason: best.stop.describe(),
        iterations: best.iterations,
        evaluations,
        failed_evaluations: failed,
        starts,
        cost: best.cost,
        degrees_of_freedom: dof,
        reduced_chi2: reduced,
        covariance_scale: s2,
        parameters: params,
        offsets,
        correlation: Correlation {
            names,
            matrix: corr,
        },
        curves: curve_reports,
        identifiability: Identifiability {
            rank_tolerance: spec.rank_tolerance,
            singular_values: an.singular_values.clone(),
            directions: an.directions.clone(),
            rules: findings,
        },
        fitted: Value::Object(fitted),
        summary,
        warnings,
    })
}
