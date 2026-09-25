//! Sensitivities: the Jacobian of every probe with respect to every
//! continuous parameter (spec Sections 3, 10 and 12; erratum E15).
//!
//! **Method: central differences of complete solves in log-parameter
//! space.** For parameter p (a number, not an integer, not derived, not
//! zero) the design is re-expanded, compiled and solved at p·e^{+h} and
//! p·e^{−h}, and for every probe y and frequency
//!
//! ```text
//! S_dB  = [20·log10|y(p·e^h)| − 20·log10|y(p·e^−h)|] / (2h) / 100   dB per %
//! S_deg = arg[y(p·e^h) / y(p·e^−h)] / (2h) / 100 (in degrees)        degrees per %
//! ```
//!
//! "Per percent" is the derivative with respect to ln p divided by 100
//! (spec Section 3). Probes are evaluated exactly as [`Circuit::solve`]
//! reports them, including the drive: under a characteristic drive the
//! level is renormalised at 500 Hz by a real factor, so every level
//! derivative is the voltage-drive derivative minus that of the reference
//! probe at 500 Hz, and phase derivatives are unchanged.
//!
//! **Step.** h = 1e-5 by default. The truncation error of the central
//! difference is h²/6·|g'''| and the rounding error about ε_g/h, where ε_g
//! is the error of one solve in dB. For a response whose derivatives grow
//! like (2Q)^k near a resonance of quality Q, the relative truncation error
//! is (2Q·h)²/6: 7e-9 for Q = 10 and 1e-6 for Q = 120. Measured on
//! `examples/design_over_ear.json` (docs/analysis.md), ε_g is about 1e-14
//! dB, so rounding contributes about 1e-10 of the largest derivative, while
//! h = 1e-4 left errors up to 1.6e-4 of it near the lightly damped
//! front-cavity depth resonance. Both error terms are checked against
//! closed forms in `tests/analysis.rs`.
//!
//! **Bounds.** When p·e^{±h} would leave [min, max], the second-order
//! one-sided formula is used, (−3g₀ + 4g₁ − g₂)/(2h) with points at 0, ±h,
//! ±2h, and the result says so (`scheme`).
//!
//! **Topology.** Integers, booleans and choices are excluded, as are
//! parameters whose step changes the structure of the expanded netlist (an
//! `enabled` condition, an element type or a string switching inside the
//! step; see [`super::structure`]), its number of unknowns or its
//! frequency grid: they are reported as not differentiable. A kink or jump
//! from `round`, `min`, `clamp`, `if` and the like that changes none of
//! these is flagged when the two one-sided differences disagree by more
//! than half the largest central derivative of that probe.
//!
//! **Forward sensitivities** (option `method: forward_sensitivity`) evaluate
//! the same stepped designs without solving them: at each frequency the base
//! matrix is factored once, and each stepped solution is the first-order
//! update x₀ + A₀⁻¹·(b − A·x₀), restamping only the elements whose records
//! the step changes (see `forward_sensitivities`). Its error, O(h²) in the
//! stepped solution, is even in h, so the differences keep their O(h²)
//! accuracy. On the template this is 3.7 to 4.4 times faster and agrees
//! with complete solves to 1e-6 of each parameter's largest sensitivity in
//! the credible band (4e-8 measured). Complete solves stay the default: they are the
//! reference the faster path is tested against. The adjoint method of spec
//! Section 3 would be cheaper still for few probes and many parameters.

use super::{db_ratio, nan_mat, options_error, select_probes, Design, Excluded, Point};
use crate::circuit::Circuit;
use crate::drive::{self, DriveInfo, DriveSpec};
use crate::error::{Error, Result};
use crate::expr::PValue;
use crate::linalg::{Lu, Matrix};
use crate::mna::Mna;
use crate::params::{Overrides, ParamDef};
use crate::solve::SolveResult;
use crate::validity::Shading;
use crate::C64;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default step in ln p.
pub const DEFAULT_STEP: f64 = 1e-5;

/// Options of [`jacobian`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitivityOptions {
    /// Parameters to differentiate (default: every non-derived one; those
    /// that are not continuous are listed as excluded).
    pub parameters: Option<Vec<String>>,
    /// Probes to report (default: all).
    pub probes: Option<Vec<String>>,
    /// Step in ln p (default 1e-5; at most 0.05).
    pub step: Option<f64>,
    /// How the stepped designs are evaluated (default `complete_solves`).
    pub method: Option<Method>,
}

/// How the stepped designs are evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Complete solves of each stepped design: the reference.
    #[default]
    CompleteSolves,
    /// One update per stepped design from the base factorisation
    /// (forward sensitivities; see `forward_sensitivities`).
    ForwardSensitivity,
}

impl Method {
    pub fn description(self) -> &'static str {
        match self {
            Method::CompleteSolves => METHOD,
            Method::ForwardSensitivity => METHOD_FORWARD,
        }
    }
}

impl SensitivityOptions {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(h) = self.step {
            if !(h > 0.0 && h <= 0.05) {
                return Err(format!("'step' must be in (0, 0.05], got {h}"));
            }
        }
        Ok(())
    }
}

/// Finite-difference scheme used for one parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Central,
    /// One-sided towards larger values (the parameter is at its minimum).
    Forward,
    /// One-sided towards smaller values (the parameter is at its maximum).
    Backward,
}

/// Sensitivities to one parameter.
#[derive(Debug, Clone, Serialize)]
pub struct ParamSensitivity {
    pub name: String,
    pub label: String,
    pub value: f64,
    pub unit: Option<String>,
    pub scheme: Scheme,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub warnings: Vec<String>,
    /// d(20·log10|y|)/d(ln p)/100 for each reported probe and frequency.
    #[serde(rename = "dB_per_pct", serialize_with = "nan_mat::serialize")]
    pub db_per_pct: Vec<Vec<f64>>,
    /// d(arg y, degrees)/d(ln p)/100.
    #[serde(rename = "deg_per_pct", serialize_with = "nan_mat::serialize")]
    pub deg_per_pct: Vec<Vec<f64>>,
}

/// The Jacobian of the reported probes.
#[derive(Debug, Clone, Serialize)]
pub struct Jacobian {
    pub method: &'static str,
    /// Step in ln p.
    pub step: f64,
    #[serde(rename = "frequencies_Hz")]
    pub freqs_hz: Vec<f64>,
    /// Reported probes, in the order of each parameter's rows.
    pub probes: Vec<String>,
    pub parameters: Vec<ParamSensitivity>,
    pub excluded: Vec<Excluded>,
    /// Validity shading of the base design.
    pub shading: Shading,
    pub drive: DriveInfo,
    /// Reproducibility hash of the base design's expanded netlist.
    pub hash: String,
    pub engine: &'static str,
}

pub const METHOD: &str = "central differences of complete solves in ln(p); second-order one-sided differences at a bound";
pub const METHOD_FORWARD: &str = "forward sensitivities: each stepped solution is x0 + A0^-1 (b - A x0) with the base LU reused, then central differences in ln(p); second-order one-sided differences at a bound";

/// One probe's sensitivities to every parameter, for a heat map.
#[derive(Debug, Clone, Serialize)]
pub struct HeatMap {
    pub probe: String,
    pub parameters: Vec<String>,
    pub labels: Vec<String>,
    #[serde(rename = "frequencies_Hz")]
    pub freqs_hz: Vec<f64>,
    /// dB per percent, one row per parameter.
    #[serde(rename = "dB_per_pct", serialize_with = "nan_mat::serialize")]
    pub db_per_pct: Vec<Vec<f64>>,
    pub shading: Shading,
}

impl Jacobian {
    pub fn parameter(&self, name: &str) -> Option<&ParamSensitivity> {
        self.parameters.iter().find(|p| p.name == name)
    }

    fn probe_index(&self, probe: &str) -> Result<usize> {
        self.probes
            .iter()
            .position(|p| p == probe)
            .ok_or_else(|| Error::Probe {
                id: probe.to_string(),
                msg: "not among the differentiated probes".into(),
            })
    }

    /// dB per percent of `probe` with respect to `param`.
    pub fn db(&self, param: &str, probe: &str) -> Option<&[f64]> {
        let j = self.probe_index(probe).ok()?;
        self.parameter(param).map(|p| p.db_per_pct[j].as_slice())
    }

    /// Degrees per percent of `probe` with respect to `param`.
    pub fn deg(&self, param: &str, probe: &str) -> Option<&[f64]> {
        let j = self.probe_index(probe).ok()?;
        self.parameter(param).map(|p| p.deg_per_pct[j].as_slice())
    }

    /// The heat map of one probe: parameter × frequency in dB per percent.
    pub fn heat_map(&self, probe: &str) -> Result<HeatMap> {
        let j = self.probe_index(probe)?;
        Ok(HeatMap {
            probe: probe.to_string(),
            parameters: self.parameters.iter().map(|p| p.name.clone()).collect(),
            labels: self.parameters.iter().map(|p| p.label.clone()).collect(),
            freqs_hz: self.freqs_hz.clone(),
            db_per_pct: self
                .parameters
                .iter()
                .map(|p| p.db_per_pct[j].clone())
                .collect(),
            shading: self.shading,
        })
    }
}

/// Computes the Jacobian of `design` (see the module documentation).
pub fn jacobian(design: &Design, opts: &SensitivityOptions) -> Result<Jacobian> {
    opts.validate()
        .map_err(|m| options_error("sensitivity", m))?;
    let h = opts.step.unwrap_or(DEFAULT_STEP);
    let method = opts.method.unwrap_or_default();
    let base = design.base_point()?;
    let r0 = base.solve()?;
    let pidx = select_probes(&r0, opts.probes.as_deref())?;
    let mut steps = Vec::new();
    let mut parameters = Vec::new();
    let mut excluded = Vec::new();
    for def in design.selected(opts.parameters.as_deref())? {
        let value = match design.continuous_value(def) {
            Ok(v) => v,
            Err(reason) => {
                excluded.push(Excluded {
                    name: def.name.clone(),
                    reason,
                });
                continue;
            }
        };
        if value == 0.0 {
            excluded.push(Excluded {
                name: def.name.clone(),
                reason: "value is 0: a relative (logarithmic) step is undefined".into(),
            });
            continue;
        }
        match prepare(design, &base, def, value, h) {
            // Complete solves go one parameter at a time, so only two
            // stepped designs are alive at once.
            Ok(s) if method == Method::CompleteSolves => match complete_solves(&s, &pidx) {
                Ok(y) => parameters.push(finish(&s, &r0, &pidx, &y, h)),
                Err(reason) => excluded.push(Excluded {
                    name: def.name.clone(),
                    reason,
                }),
            },
            Ok(s) => steps.push(s),
            Err(reason) => excluded.push(Excluded {
                name: def.name.clone(),
                reason,
            }),
        }
    }
    if method == Method::ForwardSensitivity {
        let values = forward_sensitivities(&base, &steps, &pidx)?;
        for (s, v) in steps.iter().zip(values) {
            match v {
                Ok(y) => parameters.push(finish(s, &r0, &pidx, &y, h)),
                Err(reason) => excluded.push(Excluded {
                    name: s.def.name.clone(),
                    reason,
                }),
            }
        }
    }
    Ok(Jacobian {
        method: method.description(),
        step: h,
        freqs_hz: r0.freqs_hz.clone(),
        probes: pidx.iter().map(|&i| r0.probes[i].id.clone()).collect(),
        parameters,
        excluded,
        shading: r0.shading,
        drive: r0.meta.drive.clone(),
        hash: base.hash(),
        engine: crate::solve::ENGINE,
    })
}

/// A parameter with its two stepped designs, compiled.
struct Step<'a> {
    def: &'a ParamDef,
    value: f64,
    scheme: Scheme,
    points: [Point; 2],
}

/// Probe values of the two stepped designs: `[point][probe][frequency]`.
type StepValues = [Vec<Vec<C64>>; 2];

/// Chooses the scheme and compiles the stepped designs, or says why the
/// parameter cannot be differentiated here.
fn prepare<'a>(
    design: &Design,
    base: &Point,
    def: &'a ParamDef,
    value: f64,
    h: f64,
) -> std::result::Result<Step<'a>, String> {
    let (lo, hi) = def.bounds();
    let inside = |x: f64| lo.is_none_or(|l| x >= l) && hi.is_none_or(|u| x <= u);
    let at = |k: f64| value * (k * h).exp();
    let (scheme, ks) = if inside(at(1.0)) && inside(at(-1.0)) {
        (Scheme::Central, [1.0, -1.0])
    } else if inside(at(-1.0)) && inside(at(-2.0)) {
        (Scheme::Backward, [-1.0, -2.0])
    } else if inside(at(1.0)) && inside(at(2.0)) {
        (Scheme::Forward, [1.0, 2.0])
    } else {
        return Err(format!(
            "its bounds leave no room for a step of {h} in ln p"
        ));
    };
    let point = |x: f64| -> std::result::Result<Point, String> {
        let mut ov = Overrides::new();
        ov.insert(def.name.clone(), PValue::Num(x));
        let p = design
            .point(&ov)
            .map_err(|e| format!("the design fails at {} = {x}: {e}", def.name))?;
        if let Some(d) = base.topology_change(&p) {
            return Err(format!(
                "not differentiable here: moving it to {x} {d} (a topology change)"
            ));
        }
        if p.circuit.freqs != base.circuit.freqs {
            return Err(format!(
                "not differentiable here: moving it to {x} changes the frequency grid"
            ));
        }
        Ok(p)
    };
    Ok(Step {
        def,
        value,
        scheme,
        points: [point(at(ks[0]))?, point(at(ks[1]))?],
    })
}

/// The stepped designs solved completely.
fn complete_solves(s: &Step, pidx: &[usize]) -> std::result::Result<StepValues, String> {
    let solve = |p: &Point| -> std::result::Result<Vec<Vec<C64>>, String> {
        let r = p.solve().map_err(|e| {
            format!(
                "the solve fails at {} = {}: {e}",
                s.def.name,
                stepped_value(p, s)
            )
        })?;
        Ok(pidx.iter().map(|&i| r.probes[i].values.clone()).collect())
    };
    Ok([solve(&s.points[0])?, solve(&s.points[1])?])
}

fn stepped_value(p: &Point, s: &Step) -> String {
    p.overrides
        .get(&s.def.name)
        .map_or_else(String::new, |v| v.to_string())
}

/// The factor [`Circuit::solve`] applies to a raw solution for the stated
/// drive, as there (`solve.rs`): constant for no drive, voltage, power and
/// characteristic drives, per frequency for constant current.
enum DriveFactor {
    Constant(f64),
    Current { amps: f64, source: usize },
}

fn drive_factor(c: &Circuit) -> Result<DriveFactor> {
    let Some(spec) = &c.drive else {
        return Ok(DriveFactor::Constant(1.0));
    };
    let v = drive::source_voltage(c)?;
    if !(v.is_finite() && v != 0.0) {
        return Err(Error::Netlist(
            "drive: the vsource's V_V must be non-zero to scale from".into(),
        ));
    }
    Ok(match spec {
        DriveSpec::Voltage(t) => DriveFactor::Constant(t / v),
        DriveSpec::Power { watts, rated_ohm } => {
            DriveFactor::Constant(drive::voltage_for_power(*watts, *rated_ohm) / v)
        }
        DriveSpec::Characteristic {
            probe,
            level_db,
            f_hz,
        } => DriveFactor::Constant(drive::characteristic_voltage(c, probe, *level_db, *f_hz)? / v),
        DriveSpec::Current(amps) => DriveFactor::Current {
            amps: *amps,
            source: c
                .elements
                .iter()
                .position(|e| e.is_source())
                .expect("source_voltage found the vsource"),
        },
    })
}

/// Stamps a circuit at one frequency.
fn assemble(c: &Circuit, f: f64) -> Mna {
    let cx = c.cx(f);
    let mut mna = Mna::new(c.nodes.len(), c.dim - c.nodes.len());
    for (i, e) in c.elements.iter().enumerate() {
        e.stamp(&cx, &mut mna, &c.branches(i));
    }
    mna
}

/// Indices of the elements whose records differ between two expanded
/// documents of the same structure, or `None` when something every stamp
/// reads (`air`, `level`) differs. Only these elements need restamping:
/// the MNA assembly is a sum of the elements' stamps, element order and
/// unknowns being the same in both.
fn changed_elements(base: &Value, stepped: &Value) -> Option<Vec<usize>> {
    for key in ["air", "level"] {
        if base.get(key) != stepped.get(key) {
            return None;
        }
    }
    let (a, b) = (
        base.get("elements")?.as_array()?,
        stepped.get("elements")?.as_array()?,
    );
    (a.len() == b.len()).then(|| (0..a.len()).filter(|&i| a[i] != b[i]).collect())
}

/// The part of b − A·x₀ that the listed elements contribute at one
/// frequency.
fn partial_residual(c: &Circuit, list: &[usize], f: f64, x0: &[C64]) -> Vec<C64> {
    let cx = c.cx(f);
    let mut m = Mna::new(c.nodes.len(), c.dim - c.nodes.len());
    for &e in list {
        c.elements[e].stamp(&cx, &mut m, &c.branches(e));
    }
    let ax = m.a.mul_vec(x0);
    m.rhs.iter().zip(&ax).map(|(b, a)| b - a).collect()
}

/// Residual b − A·x₀ of a stepped circuit at one frequency. With a list of
/// changed elements it is the base residual `res0` plus the difference of
/// those elements' contributions at the stepped value and at the base
/// value (`base_part`); without one the whole stepped system is assembled.
fn residual(
    c: &Circuit,
    changed: &Option<Vec<usize>>,
    f: f64,
    x0: &[C64],
    res0: &[C64],
    base_part: Option<&[C64]>,
) -> Vec<C64> {
    match (changed, base_part) {
        (Some(list), Some(rb)) if !list.is_empty() => {
            let rs = partial_residual(c, list, f, x0);
            res0.iter()
                .zip(rs.iter().zip(rb))
                .map(|(r0, (s, b))| r0 + (s - b))
                .collect()
        }
        (Some(list), _) if list.is_empty() => res0.to_vec(),
        _ => {
            let m = assemble(c, f);
            let ax = m.a.mul_vec(x0);
            m.rhs.iter().zip(&ax).map(|(b, a)| b - a).collect()
        }
    }
}

/// A⁻¹·b with the factors of A and up to two steps of iterative
/// refinement, as `linalg::solve` does it.
fn solve_refined(lu: &Lu, a: &Matrix, b: &[C64]) -> Vec<C64> {
    let mut x = lu.solve(b);
    for _ in 0..2 {
        let ax = a.mul_vec(&x);
        let r: Vec<C64> = b.iter().zip(&ax).map(|(bi, ai)| bi - ai).collect();
        let dx = lu.solve(&r);
        let mut changed = false;
        for (xi, di) in x.iter_mut().zip(&dx) {
            if di.norm() > 1e-17 * xi.norm() {
                changed = true;
            }
            *xi += di;
        }
        if !changed {
            break;
        }
    }
    x
}

/// A⁻¹·r with the factors of A and one step of iterative refinement
/// against A (the factors alone are accurate only norm-wise).
fn refined_solve(lu: &Lu, a: &Matrix, r: &[C64]) -> Vec<C64> {
    let mut d = lu.solve(r);
    let ad = a.mul_vec(&d);
    let res: Vec<C64> = r.iter().zip(&ad).map(|(x, y)| x - y).collect();
    for (di, ci) in d.iter_mut().zip(lu.solve(&res)) {
        *di += ci;
    }
    d
}

/// Forward sensitivities: at each frequency the base matrix A₀ is factored
/// once, and each stepped design's solution is x̃ = x₀ + A₀⁻¹·r, r = b − A·x₀,
/// with A and b stamped at the stepped parameter value (only the elements
/// whose records change are restamped, see [`residual`]). That is
/// x₀ + h·dx/d(ln p) with dx/dp = A₀⁻¹·(db/dp − (dA/dp)·x₀) and the
/// derivatives of the stamps taken by the same finite step: one step of a
/// chord (modified Newton) iteration. With A = A₀ + h·A₁ + O(h²) and
/// r = h·r₁ + O(h²), the exact stepped solution is x₀ + A⁻¹·r, so
/// x̃ − x = (A₀⁻¹ − A⁻¹)·r = h²·A₀⁻¹A₁A₀⁻¹r₁ + O(h³). The h² term is even in
/// h and cancels in the central difference, and in the one-sided formula
/// (errors h²E at h and 4h²E at 2h enter as 4·h²E − 4h²E); the O(h³)
/// remainder leaves an O(h²) error, the
/// same order as the difference's own truncation, so both schemes stay
/// second order. Measured on a lightly damped pressure chamber for h from
/// 1e-2 to 1e-5, the error of the forward path was 0.94 to 1.24 times that
/// of complete solves. The drive factor and the probes are evaluated on
/// each stepped circuit as [`Circuit::solve`] does.
fn forward_sensitivities(
    base: &Point,
    steps: &[Step],
    pidx: &[usize],
) -> Result<Vec<std::result::Result<StepValues, String>>> {
    let c0 = &base.circuit;
    let nf = c0.freqs.len();
    let changed: Vec<[Option<Vec<usize>>; 2]> = steps
        .iter()
        .map(|s| {
            s.points
                .each_ref()
                .map(|p| changed_elements(&base.doc, &p.doc))
        })
        .collect();
    let mut out: Vec<std::result::Result<StepValues, String>> = Vec::new();
    let mut factors: Vec<Option<[DriveFactor; 2]>> = Vec::new();
    for s in steps {
        let fac = |p: &Point| {
            drive_factor(&p.circuit).map_err(|e| {
                format!(
                    "the drive fails at {} = {}: {e}",
                    s.def.name,
                    stepped_value(p, s)
                )
            })
        };
        match fac(&s.points[0]).and_then(|a| Ok([a, fac(&s.points[1])?])) {
            Ok(f) => {
                let empty = || vec![vec![C64::new(0.0, 0.0); nf]; pidx.len()];
                factors.push(Some(f));
                out.push(Ok([empty(), empty()]));
            }
            Err(e) => {
                factors.push(None);
                out.push(Err(e));
            }
        }
    }
    for (k, &f) in c0.freqs.iter().enumerate() {
        let m0 = assemble(c0, f);
        let lu = match Lu::factor(&m0.a) {
            Ok(lu) => lu,
            // The engine's solve names the unknown of a singular matrix.
            Err(s) => {
                c0.solve_at(f)?;
                return Err(Error::Singular {
                    f_hz: f,
                    unknown: format!("unknown {}", s.0),
                });
            }
        };
        let x0 = solve_refined(&lu, &m0.a, &m0.rhs);
        let a0x0 = m0.a.mul_vec(&x0);
        let res0: Vec<C64> = m0.rhs.iter().zip(&a0x0).map(|(b, a)| b - a).collect();
        for (si, s) in steps.iter().enumerate() {
            let (Some(fac), Ok(vals)) = (&factors[si], &mut out[si]) else {
                continue;
            };
            let mut failure = None;
            // The base value's contribution of the changed elements, shared
            // by both stepped points when they change the same elements.
            let mut base_parts: [Option<Vec<C64>>; 2] = [None, None];
            for pt in 0..2 {
                if let Some(list) = changed[si][pt].as_ref().filter(|l| !l.is_empty()) {
                    base_parts[pt] = match (pt, &base_parts[0]) {
                        (1, Some(b)) if changed[si][0].as_ref() == Some(list) => Some(b.clone()),
                        _ => Some(partial_residual(c0, list, f, &x0)),
                    };
                }
            }
            for (pt, p) in s.points.iter().enumerate() {
                let c = &p.circuit;
                let r = residual(
                    c,
                    &changed[si][pt],
                    f,
                    &x0,
                    &res0,
                    base_parts[pt].as_deref(),
                );
                let d = refined_solve(&lu, &m0.a, &r);
                let mut x: Vec<C64> = x0.iter().zip(&d).map(|(a, b)| a + b).collect();
                let scale = match fac[pt] {
                    DriveFactor::Constant(v) => C64::new(v, 0.0),
                    DriveFactor::Current { amps, source } => {
                        let cx = c.cx(f);
                        let i = c.elements[source]
                            .port_flow(&cx, &x, &c.branches(source), 0)
                            .unwrap_or_default();
                        if i.norm() == 0.0 || !i.norm().is_finite() {
                            failure = Some(format!(
                                "drive: the source current is zero at {f} Hz; cannot hold it constant"
                            ));
                            break;
                        }
                        C64::new(amps, 0.0) / i
                    }
                };
                x.iter_mut().for_each(|v| *v *= scale);
                for (j, &pi) in pidx.iter().enumerate() {
                    match c.probe_value(&c.probes[pi], f, &x) {
                        Ok(y) => vals[pt][j][k] = y,
                        Err(e) => failure = Some(e.to_string()),
                    }
                }
            }
            if let Some(e) = failure {
                out[si] = Err(e);
            }
        }
    }
    Ok(out)
}

/// Derivatives from the probe values of the stepped designs.
fn finish(s: &Step, r0: &SolveResult, pidx: &[usize], y: &StepValues, h: f64) -> ParamSensitivity {
    let (lo, hi) = s.def.bounds();
    let scheme = s.scheme;
    let nf = r0.freqs_hz.len();
    let mut db = vec![vec![f64::NAN; nf]; pidx.len()];
    let mut deg = vec![vec![f64::NAN; nf]; pidx.len()];
    let mut warnings = Vec::new();
    for (j, &pi) in pidx.iter().enumerate() {
        let (y0, ya, yb) = (&r0.probes[pi].values, &y[0][j], &y[1][j]);
        // g values relative to the base point (g₀ = 0).
        let mut ga = vec![0.0; nf];
        let mut gb = vec![0.0; nf];
        let mut pa = vec![0.0; nf];
        let mut pb = vec![0.0; nf];
        for k in 0..nf {
            ga[k] = db_ratio(ya[k], y0[k]);
            gb[k] = db_ratio(yb[k], y0[k]);
            pa[k] = (ya[k] / y0[k]).arg().to_degrees();
            pb[k] = (yb[k] / y0[k]).arg().to_degrees();
            let d = |a: f64, b: f64| match scheme {
                Scheme::Central => (a - b) / (2.0 * h),
                Scheme::Forward => (4.0 * a - b) / (2.0 * h),
                Scheme::Backward => (b - 4.0 * a) / (2.0 * h),
            };
            db[j][k] = d(ga[k], gb[k]) / 100.0;
            deg[j][k] = d(pa[k], pb[k]) / 100.0;
        }
        if scheme == Scheme::Central {
            let id = &r0.probes[pi].id;
            for (what, a, b, sd) in [("level", &ga, &gb, &db[j]), ("phase", &pa, &pb, &deg[j])] {
                if let Some(w) = continuity(&r0.freqs_hz, a, b, sd, h) {
                    warnings.push(format!("{what} of '{id}': {w}"));
                }
            }
        }
    }
    let note = match scheme {
        Scheme::Central => None,
        Scheme::Forward => Some(format!(
            "at or near its minimum {}: one-sided (forward) difference",
            lo.unwrap_or(f64::NAN)
        )),
        Scheme::Backward => Some(format!(
            "at or near its maximum {}: one-sided (backward) difference",
            hi.unwrap_or(f64::NAN)
        )),
    };
    ParamSensitivity {
        name: s.def.name.clone(),
        label: s.def.label.clone().unwrap_or_else(|| s.def.name.clone()),
        value: s.value,
        unit: s.def.display_unit(),
        scheme,
        note,
        warnings,
        db_per_pct: db,
        deg_per_pct: deg,
    }
}

/// Compares the forward and backward one-sided differences (g₊/h and
/// −g₋/h, with g relative to the base point) with the central derivative
/// `s` (per percent). A smooth response gives |D₊ − D₋| = h·|g''|, far below
/// the derivative itself; a jump or kink inside the step gives a difference
/// of the order of the derivative. Returns a warning when the difference
/// exceeds half the largest central derivative of the probe (and 1e-6 per
/// unit of ln p).
fn continuity(freqs: &[f64], gp: &[f64], gm: &[f64], s: &[f64], h: f64) -> Option<String> {
    let scale = s
        .iter()
        .filter(|x| x.is_finite())
        .fold(0.0f64, |m, x| m.max(x.abs()))
        * 100.0;
    if scale == 0.0 {
        return None;
    }
    let mut count = 0;
    let mut worst = (0.0, 0.0);
    for k in 0..freqs.len() {
        let diff = (gp[k] / h + gm[k] / h).abs();
        if diff.is_finite() && diff > 0.5 * scale && diff > 1e-6 {
            count += 1;
            if diff > worst.0 {
                worst = (diff, freqs[k]);
            }
        }
    }
    (count > 0).then(|| {
        format!(
            "the one-sided differences disagree at {count} frequencies (worst at {:.4} Hz): the response has a kink or jump within the step (from round, floor, min, max, abs, clamp or if in an expression?)",
            worst.1
        )
    })
}
