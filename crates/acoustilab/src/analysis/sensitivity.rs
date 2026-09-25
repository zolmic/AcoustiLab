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
//! Faster alternatives that reuse the factorisation (forward sensitivities
//! A·dx/dp = db/dp − (dA/dp)·x, or the adjoint method of spec Section 3)
//! would give the same numbers; see docs/analysis.md for why they are not
//! used yet.

use super::{db_ratio, nan_mat, options_error, select_probes, Design, Excluded, Point};
use crate::drive::DriveInfo;
use crate::error::{Error, Result};
use crate::expr::PValue;
use crate::params::{Overrides, ParamDef};
use crate::solve::SolveResult;
use crate::validity::Shading;
use serde::{Deserialize, Serialize};

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
    let base = design.base_point()?;
    let r0 = base.solve()?;
    let pidx = select_probes(&r0, opts.probes.as_deref())?;
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
        match differentiate(design, &base, &r0, &pidx, def, value, h) {
            Ok(p) => parameters.push(p),
            Err(reason) => excluded.push(Excluded {
                name: def.name.clone(),
                reason,
            }),
        }
    }
    Ok(Jacobian {
        method: METHOD,
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

/// Solves the design with one parameter moved to `x`, or says why the
/// result cannot be differenced against the base.
fn stepped(
    design: &Design,
    base: &Point,
    r0: &SolveResult,
    name: &str,
    x: f64,
) -> std::result::Result<SolveResult, String> {
    let mut ov = Overrides::new();
    ov.insert(name.to_string(), PValue::Num(x));
    let point = design
        .point(&ov)
        .map_err(|e| format!("the design fails at {name} = {x}: {e}"))?;
    if let Some(d) = base.topology_change(&point) {
        return Err(format!(
            "not differentiable here: moving it to {x} {d} (a topology change)"
        ));
    }
    let r = point
        .solve()
        .map_err(|e| format!("the solve fails at {name} = {x}: {e}"))?;
    if r.freqs_hz != r0.freqs_hz {
        return Err(format!(
            "not differentiable here: moving it to {x} changes the frequency grid"
        ));
    }
    Ok(r)
}

fn differentiate(
    design: &Design,
    base: &Point,
    r0: &SolveResult,
    pidx: &[usize],
    def: &ParamDef,
    value: f64,
    h: f64,
) -> std::result::Result<ParamSensitivity, String> {
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
    let ra = stepped(design, base, r0, &def.name, at(ks[0]))?;
    let rb = stepped(design, base, r0, &def.name, at(ks[1]))?;
    let nf = r0.freqs_hz.len();
    let mut db = vec![vec![f64::NAN; nf]; pidx.len()];
    let mut deg = vec![vec![f64::NAN; nf]; pidx.len()];
    let mut warnings = Vec::new();
    for (j, &pi) in pidx.iter().enumerate() {
        let (y0, ya, yb) = (
            &r0.probes[pi].values,
            &ra.probes[pi].values,
            &rb.probes[pi].values,
        );
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
            for (what, a, b, s) in [("level", &ga, &gb, &db[j]), ("phase", &pa, &pb, &deg[j])] {
                if let Some(w) = continuity(&r0.freqs_hz, a, b, s, h) {
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
    Ok(ParamSensitivity {
        name: def.name.clone(),
        label: def.label.clone().unwrap_or_else(|| def.name.clone()),
        value,
        unit: def.display_unit(),
        scheme,
        note,
        warnings,
        db_per_pct: db,
        deg_per_pct: deg,
    })
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
