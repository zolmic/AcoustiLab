//! Rational fit of a probe, its pole/zero table, the impulse-length check
//! of erratum E46, and the attribution of resonances to parameters (spec
//! Section 10: "Poles, zeros and Q table ... names each resonance and the
//! elements that set it").
//!
//! **Table.** A pole a (rad/s) is listed with f = |a|/(2π) and, for a
//! complex pair, Q = |a|/(−2·Re a). Its *weight* is the size of its own
//! term at its resonance relative to the whole model there,
//! |c/Re a| / |H(j·Im a)| for a pair (|c/a| / |H(j|a|)| for a real pole),
//! in dB: an isolated resonance reads about 0 dB, a pole that nearly
//! cancels against a zero (as fits of irrational responses produce) reads
//! far below. A pole is *resonant* when it is a pair with Q ≥ `q_min`
//! (default 1), inside the fitted band, with weight ≥ `weight_min_dB`
//! (default −20 dB). Zeros come from the state-space realisation
//! ([`RationalModel::zeros`]); a zero in the right half-plane inside the
//! band means the response is not minimum phase there (spec Section 17:
//! minimum phase is asserted only after checking the zeros of the fit).
//!
//! **Impulse length (E46).** A pole's envelope falls by 80 dB in
//! ln(10⁴)/|Re a| = (4·ln 10/π)·Q/f = 2.93·Q/f. The buffer must hold that
//! in its causal half: N/(2·fs) ≥ 2.93·Q/f for every resonant pole, so that
//! the energy left at negative times stays below −80 dB. Erratum E46
//! rounds the factor to 2.9, which leaves −79.1 dB on a Q = 20 resonator
//! (`tools/time/ir_refs.py`). The check reports the lowest-frequency
//! resonant pole, the binding one (largest 2.93·Q/f), and the smallest
//! power-of-two N that covers it.
//!
//! **Attribution.** For a parametric netlist each continuous parameter
//! that some enabled element depends on (directly or through derived
//! parameters) is perturbed by `step` (default +1 %, −1 % at an upper
//! bound), the probe is re-solved and re-fitted starting from the base
//! poles, and each resonant pole is matched to the nearest perturbed pole.
//! The logarithmic sensitivities d ln f/d ln p and d ln Q/d ln p rank the
//! parameters that set the resonance, and the elements that use the top
//! parameters are named. A resonance that no parameter moves (all
//! |d ln f/d ln p| < 0.05) is attributed to the enabled elements that
//! depend on no perturbed parameter, such as an ear simulator with fixed
//! values. Cost: one solve and one short fit per parameter.

use super::vfit::{pole_term, vector_fit, Asymptote, FitReport, Pole, RationalModel, VfOptions};
use crate::circuit::Circuit;
use crate::drive::DriveInfo;
use crate::error::{Error, Result};
use crate::expr::{Expr, PValue};
use crate::params::{Overrides, ParamKind, Parametric};
use crate::C64;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

/// E46: the causal half of the buffer must hold E46_FACTOR·Q/f, the time
/// for the pole's envelope to fall 80 dB (4·ln 10/π = 2.932; E46 prints
/// 2.9).
pub const E46_FACTOR: f64 = 4.0 * std::f64::consts::LN_10 / PI;
/// Largest FFT length the spec offers for audition (Section 16).
pub const N_AUDITION_MAX: usize = 16384;
/// Smallest FFT length the spec offers.
pub const N_AUDITION_MIN: usize = 4096;

#[derive(Debug, Clone)]
pub struct PoleFitOptions {
    pub vf: VfOptions,
    /// Band of the log grid to fit (default: the whole sweep).
    pub f_min_hz: Option<f64>,
    pub f_max_hz: Option<f64>,
    /// Smallest Q of a resonant pole.
    pub q_min: f64,
    /// Smallest weight of a resonant pole, dB.
    pub weight_min_db: f64,
}

impl Default for PoleFitOptions {
    fn default() -> Self {
        PoleFitOptions {
            vf: VfOptions::default(),
            f_min_hz: None,
            f_max_hz: None,
            q_min: 1.0,
            weight_min_db: -20.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PoleRow {
    /// Real and imaginary part, rad/s (a pair is listed once, Im > 0).
    #[serde(rename = "re_rad_per_s")]
    pub re: f64,
    #[serde(rename = "im_rad_per_s")]
    pub im: f64,
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    /// Q of a pair; `None` for a real pole.
    pub q: Option<f64>,
    pub real: bool,
    pub in_band: bool,
    #[serde(rename = "weight_dB")]
    pub weight_db: f64,
    pub resonant: bool,
    /// Time for the pole's envelope to fall 60 dB, 6.91/|Re a|, s.
    #[serde(rename = "t60_s")]
    pub t60_s: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ZeroRow {
    #[serde(rename = "re_rad_per_s")]
    pub re: f64,
    #[serde(rename = "im_rad_per_s")]
    pub im: f64,
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    /// |z|/(2|Re z|) of a complex zero.
    pub q: Option<f64>,
    pub real: bool,
    pub in_band: bool,
    /// In the right half-plane: a non-minimum-phase zero.
    pub rhp: bool,
}

/// A probe's rational fit and tables.
#[derive(Debug, Clone)]
pub struct PoleFit {
    pub probe: String,
    pub quantity: String,
    pub unit: String,
    /// Fitted samples (the solve's log grid within the band).
    pub freqs_hz: Vec<f64>,
    pub values: Vec<C64>,
    pub model: RationalModel,
    pub report: FitReport,
    pub poles: Vec<PoleRow>,
    pub zeros: Vec<ZeroRow>,
    /// Remarks: relocation stopped early, zeros not computed.
    pub notes: Vec<String>,
    pub drive: DriveInfo,
}

impl PoleFit {
    pub fn band(&self) -> (f64, f64) {
        (self.freqs_hz[0], self.freqs_hz[self.freqs_hz.len() - 1])
    }

    /// True when no zero of the fit lies in the right half-plane within
    /// the fitted band.
    pub fn min_phase_in_band(&self) -> bool {
        !self.zeros.iter().any(|z| z.in_band && z.rhp)
    }

    /// Group delay of the fitted model at `f` Hz, s.
    pub fn group_delay(&self, f_hz: f64) -> f64 {
        self.model.group_delay(0, f_hz)
    }

    /// Indices of the resonant poles, lowest frequency first.
    pub fn resonant(&self) -> Vec<usize> {
        (0..self.poles.len())
            .filter(|&i| self.poles[i].resonant)
            .collect()
    }
}

/// Solves the circuit on its sweep and fits one probe.
pub fn fit_probe(circuit: &Circuit, probe: &str, opts: &PoleFitOptions) -> Result<PoleFit> {
    let result = circuit.solve()?;
    let p = result.probe(probe).ok_or_else(|| Error::Probe {
        id: probe.to_string(),
        msg: "no such probe".into(),
    })?;
    let lo = opts.f_min_hz.unwrap_or(0.0);
    let hi = opts.f_max_hz.unwrap_or(f64::INFINITY);
    let (freqs, values): (Vec<f64>, Vec<C64>) = result
        .freqs_hz
        .iter()
        .zip(&p.values)
        .filter(|(f, _)| **f >= lo * (1.0 - 1e-12) && **f <= hi * (1.0 + 1e-12))
        .map(|(f, v)| (*f, *v))
        .unzip();
    let (model, report, poles, zeros, notes) =
        fit_values(&freqs, &values, opts).map_err(|e| Error::Probe {
            id: probe.to_string(),
            msg: e,
        })?;
    Ok(PoleFit {
        probe: probe.to_string(),
        quantity: p.quantity.clone(),
        unit: p.unit.clone(),
        freqs_hz: freqs,
        values,
        model,
        report,
        poles,
        zeros,
        notes,
        drive: result.meta.drive,
    })
}

type Fitted = (
    RationalModel,
    FitReport,
    Vec<PoleRow>,
    Vec<ZeroRow>,
    Vec<String>,
);

/// Fits sampled values and builds the tables.
pub fn fit_values(
    freqs: &[f64],
    values: &[C64],
    opts: &PoleFitOptions,
) -> std::result::Result<Fitted, String> {
    if freqs.len() < 4 {
        return Err("fewer than 4 frequencies in the fitted band".into());
    }
    let (model, report) = vector_fit(freqs, &[values.to_vec()], &opts.vf)?;
    let (f_lo, f_hi) = (freqs[0], freqs[freqs.len() - 1]);
    let poles = pole_rows(&model, f_lo, f_hi, opts);
    let peak = values.iter().map(|v| v.norm()).fold(0.0, f64::max);
    let mut notes = Vec::new();
    if let Some(s) = &report.stopped {
        notes.push(format!(
            "pole relocation stopped early ({s}); the best model so far is kept"
        ));
    }
    let zeros = model
        .zeros(0, peak)
        .unwrap_or_else(|e| {
            notes.push(format!("zeros not computed: {e}"));
            Vec::new()
        })
        .into_iter()
        .filter(|z| z.im >= 0.0)
        .map(|z| {
            let f = z.norm() / (2.0 * PI);
            let real = z.im == 0.0;
            ZeroRow {
                re: z.re,
                im: z.im,
                f_hz: f,
                q: (!real && z.re != 0.0).then(|| z.norm() / (2.0 * z.re.abs())),
                real,
                in_band: f >= f_lo && f <= f_hi,
                rhp: z.re > 0.0,
            }
        })
        .collect();
    Ok((model, report, poles, zeros, notes))
}

fn pole_rows(model: &RationalModel, f_lo: f64, f_hi: f64, opts: &PoleFitOptions) -> Vec<PoleRow> {
    model
        .poles
        .iter()
        .zip(&model.residues[0])
        .map(|(p, c)| {
            let a = p.value();
            let f = p.f_hz();
            let s = match p {
                Pole::Real(x) => C64::new(0.0, x.abs()),
                Pole::Pair(x) => C64::new(0.0, x.im),
            };
            let own = match p {
                Pole::Real(x) => c.norm() / x.abs(),
                Pole::Pair(x) => pole_term(*p, *c, s).norm().max(c.norm() / x.re.abs()),
            };
            let total = model.eval(0, s).norm();
            let weight_db = if total > 0.0 && own > 0.0 {
                20.0 * (own / total).log10()
            } else {
                -300.0
            };
            let q = p.q();
            let in_band = f >= f_lo && f <= f_hi;
            PoleRow {
                re: a.re,
                im: a.im,
                f_hz: f,
                q,
                real: q.is_none(),
                in_band,
                weight_db,
                resonant: in_band
                    && q.is_some_and(|q| q >= opts.q_min)
                    && weight_db >= opts.weight_min_db,
                t60_s: 3.0 * 10f64.ln() / a.re.abs(),
            }
        })
        .collect()
}

/// One pole in the impulse-length check.
#[derive(Debug, Clone, Serialize)]
pub struct PoleNeed {
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    pub q: f64,
    /// Time for its envelope to fall 60 dB, 2.2·Q/f, s.
    #[serde(rename = "t60_s")]
    pub t60_s: f64,
    /// 2.93·Q/f, s.
    #[serde(rename = "needed_s")]
    pub needed_s: f64,
}

/// Erratum E46 check of an FFT length.
#[derive(Debug, Clone, Serialize)]
pub struct IrLengthCheck {
    #[serde(rename = "fs_Hz")]
    pub fs_hz: f64,
    pub n: usize,
    /// N/(2·fs), s.
    #[serde(rename = "half_length_s")]
    pub half_length_s: f64,
    /// The lowest-frequency resonant pole.
    pub lowest: Option<PoleNeed>,
    /// The resonant pole needing the longest buffer.
    pub binding: Option<PoleNeed>,
    pub covered: bool,
    /// Smallest power-of-two N ≥ 4096 with N/(2·fs) ≥ 2.93·Q/f.
    pub recommended_n: usize,
    /// False when that N exceeds the largest audition length, 16384.
    pub within_audition_range: bool,
}

pub fn ir_length_check(fit: &PoleFit, fs_hz: f64, n: usize) -> IrLengthCheck {
    let needs: Vec<PoleNeed> = fit
        .resonant()
        .into_iter()
        .map(|i| {
            let r = &fit.poles[i];
            let q = r.q.unwrap_or(0.5);
            PoleNeed {
                f_hz: r.f_hz,
                q,
                t60_s: r.t60_s,
                needed_s: E46_FACTOR * q / r.f_hz,
            }
        })
        .collect();
    let lowest = needs
        .iter()
        .min_by(|a, b| a.f_hz.total_cmp(&b.f_hz))
        .cloned();
    let binding = needs
        .iter()
        .max_by(|a, b| a.needed_s.total_cmp(&b.needed_s))
        .cloned();
    let half = n as f64 / (2.0 * fs_hz);
    let needed = binding.as_ref().map_or(0.0, |b| b.needed_s);
    let mut rec = N_AUDITION_MIN;
    while (rec as f64) / (2.0 * fs_hz) < needed && rec < (1 << 30) {
        rec *= 2;
    }
    IrLengthCheck {
        fs_hz,
        n,
        half_length_s: half,
        lowest,
        binding,
        covered: half >= needed,
        recommended_n: rec,
        within_audition_range: rec <= N_AUDITION_MAX,
    }
}

// ----- Attribution -----------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SensitivityOptions {
    /// Relative parameter step.
    pub step: f64,
    /// At most this many parameters are perturbed (in declaration order).
    pub max_parameters: usize,
    /// Relocation iterations of each re-fit (warm-started).
    pub iterations: usize,
    /// Parameters listed per pole.
    pub top: usize,
}

impl Default for SensitivityOptions {
    fn default() -> Self {
        SensitivityOptions {
            step: 0.01,
            max_parameters: 64,
            iterations: 4,
            top: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ParamSensitivity {
    pub parameter: String,
    /// d ln f / d ln p.
    pub dlnf_dlnp: f64,
    /// d ln Q / d ln p.
    #[serde(rename = "dlnQ_dlnp")]
    pub dlnq_dlnp: f64,
    /// Enabled elements whose values depend on the parameter.
    pub elements: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoleAttribution {
    /// Index into [`PoleFit::poles`].
    pub pole: usize,
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    pub q: Option<f64>,
    /// Parameters by decreasing |d ln f/d ln p| (at most `top`).
    pub parameters: Vec<ParamSensitivity>,
    /// Parameters whose re-fit lost track of this pole.
    pub unmatched: Vec<String>,
    /// When no perturbed parameter moves the pole by at least
    /// [`ATTRIBUTION_MIN`] (in |d ln f/d ln p|), the enabled elements that
    /// depend on none of them: the resonance belongs to those (e.g. an ear
    /// simulator with fixed values). Empty otherwise.
    pub fixed_elements: Vec<String>,
}

/// Smallest |d ln f/d ln p| that counts as a parameter setting a pole.
pub const ATTRIBUTION_MIN: f64 = 0.05;

#[derive(Debug, Clone, Serialize)]
pub struct Attribution {
    pub method: &'static str,
    /// Parameters perturbed, and those skipped with the reason.
    pub perturbed: Vec<String>,
    pub skipped: Vec<(String, String)>,
    pub poles: Vec<PoleAttribution>,
}

/// Collects the parameter names an expression string tree refers to.
fn expr_names(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            if let Some(src) = s.strip_prefix('=') {
                if let Ok(e) = Expr::parse(src) {
                    out.extend(e.names());
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| expr_names(x, out)),
        Value::Object(o) => o.values().for_each(|x| expr_names(x, out)),
        _ => {}
    }
}

/// Ids of the enabled elements of a parametric netlist, in order.
pub fn enabled_elements(p: &Parametric, values: &[(String, PValue)]) -> Vec<String> {
    let lookup = |n: &str| values.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let Some(Value::Array(items)) = p.doc.get("elements") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_object)
        .filter(|obj| is_enabled(obj, &lookup))
        .filter_map(|obj| obj.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn is_enabled(
    obj: &serde_json::Map<String, Value>,
    lookup: &dyn Fn(&str) -> Option<PValue>,
) -> bool {
    match obj.get("enabled") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s
            .strip_prefix('=')
            .and_then(|src| Expr::parse(src).ok())
            .and_then(|e| e.eval(lookup).ok())
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        _ => false,
    }
}

/// Enabled elements of a parametric netlist that depend on each
/// (non-derived) parameter, directly or through derived ones.
pub fn parameter_elements(
    p: &Parametric,
    values: &[(String, PValue)],
) -> BTreeMap<String, Vec<String>> {
    let lookup = |n: &str| values.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(Value::Array(items)) = p.doc.get("elements") else {
        return out;
    };
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        if !is_enabled(obj, &lookup) {
            continue;
        }
        let id = obj.get("id").and_then(Value::as_str).unwrap_or_default();
        let mut names = BTreeSet::new();
        for (k, v) in obj {
            if k != "enabled" {
                expr_names(v, &mut names);
            }
        }
        // Close over derived parameters.
        let mut stack: Vec<String> = names.iter().cloned().collect();
        let mut seen = names.clone();
        while let Some(n) = stack.pop() {
            if let Some(d) = p.def(&n) {
                if let ParamKind::Derived { expr } = &d.kind {
                    for m in expr.names() {
                        if seen.insert(m.clone()) {
                            stack.push(m);
                        }
                    }
                }
            }
        }
        for n in seen {
            if p.def(&n).is_some_and(|d| !d.is_derived()) {
                let e = out.entry(n).or_default();
                if !e.iter().any(|x| x == id) {
                    e.push(id.to_string());
                }
            }
        }
    }
    out
}

/// Attributes the resonant poles of `base` (fitted on `circuit` built from
/// `p` with `overrides`) to parameters by re-fits (see the module docs).
pub fn attribute_poles(
    p: &Parametric,
    overrides: &Overrides,
    base: &PoleFit,
    opts: &PoleFitOptions,
    sopts: &SensitivityOptions,
) -> Result<Attribution> {
    let values = p.values(overrides)?;
    let uses = parameter_elements(p, &values);
    let resonant = base.resonant();
    let mut perturbed = Vec::new();
    let mut skipped = Vec::new();
    // Per resonant pole: (parameter, S_f, S_Q) or unmatched.
    let mut table: Vec<Vec<ParamSensitivity>> = vec![Vec::new(); resonant.len()];
    let mut lost: Vec<Vec<String>> = vec![Vec::new(); resonant.len()];
    if resonant.is_empty() {
        return Ok(Attribution {
            method: "parameter re-fit",
            perturbed,
            skipped,
            poles: Vec::new(),
        });
    }
    let mut fit_opts = opts.clone();
    fit_opts.vf.initial_poles = Some(base.model.poles.clone());
    fit_opts.vf.iterations = sopts.iterations.max(1);
    fit_opts.f_min_hz = Some(base.band().0);
    fit_opts.f_max_hz = Some(base.band().1);
    for d in &p.defs {
        let ParamKind::Number {
            value: _,
            integer: false,
            ..
        } = d.kind
        else {
            continue;
        };
        let Some(elements) = uses.get(&d.name) else {
            skipped.push((d.name.clone(), "no enabled element uses it".into()));
            continue;
        };
        let v = values
            .iter()
            .find(|(n, _)| *n == d.name)
            .and_then(|(_, v)| v.as_num())
            .unwrap_or(0.0);
        if v == 0.0 {
            skipped.push((d.name.clone(), "value is zero".into()));
            continue;
        }
        if perturbed.len() >= sopts.max_parameters {
            skipped.push((d.name.clone(), "parameter limit reached".into()));
            continue;
        }
        let mut v2 = v * (1.0 + sopts.step);
        if d.check(&PValue::Num(v2)).is_err() {
            v2 = v * (1.0 - sopts.step);
            if d.check(&PValue::Num(v2)).is_err() {
                skipped.push((d.name.clone(), "bounds too tight to perturb".into()));
                continue;
            }
        }
        let mut ov = overrides.clone();
        ov.insert(d.name.clone(), PValue::Num(v2));
        let c = Circuit::from_parametric(p, &ov)?;
        let fit = match fit_probe(&c, &base.probe, &fit_opts) {
            Ok(f) => f,
            Err(e) => {
                skipped.push((d.name.clone(), format!("re-fit failed: {e}")));
                continue;
            }
        };
        perturbed.push(d.name.clone());
        let dlnp = (v2 / v).ln();
        let cand: Vec<C64> = fit.model.poles.iter().map(|q| q.value()).collect();
        for (slot, &i) in resonant.iter().enumerate() {
            let a = base.model.poles[i].value();
            let dist = |z: &C64| (z - a).norm() / a.norm();
            let best = cand
                .iter()
                .enumerate()
                .min_by(|x, y| dist(x.1).total_cmp(&dist(y.1)));
            // Accept the nearest perturbed pole if no other base pole is
            // nearer to it (mutual nearest) and it moved less than 25 %.
            let matched = best.filter(|(_, z)| {
                dist(z) < 0.25
                    && base
                        .model
                        .poles
                        .iter()
                        .enumerate()
                        .all(|(k, b)| k == i || (**z - b.value()).norm() >= (**z - a).norm())
            });
            match matched {
                Some((_, z)) => {
                    let q0 = base.model.poles[i].q().unwrap_or(0.5);
                    let q1 = z.norm() / (-2.0 * z.re);
                    table[slot].push(ParamSensitivity {
                        parameter: d.name.clone(),
                        dlnf_dlnp: (z.norm() / a.norm()).ln() / dlnp,
                        dlnq_dlnp: (q1 / q0).ln() / dlnp,
                        elements: elements.clone(),
                    });
                }
                None => lost[slot].push(d.name.clone()),
            }
        }
    }
    let fixed: Vec<String> = enabled_elements(p, &values)
        .into_iter()
        .filter(|id| {
            !perturbed
                .iter()
                .any(|n| uses.get(n).is_some_and(|e| e.contains(id)))
        })
        .collect();
    let poles = resonant
        .iter()
        .zip(table.into_iter().zip(lost))
        .map(|(&i, (mut t, unmatched))| {
            t.sort_by(|a, b| b.dlnf_dlnp.abs().total_cmp(&a.dlnf_dlnp.abs()));
            t.truncate(sopts.top);
            let explained = t
                .first()
                .is_some_and(|s| s.dlnf_dlnp.abs() >= ATTRIBUTION_MIN);
            PoleAttribution {
                pole: i,
                f_hz: base.poles[i].f_hz,
                q: base.poles[i].q,
                parameters: t,
                unmatched,
                fixed_elements: if explained { Vec::new() } else { fixed.clone() },
            }
        })
        .collect();
    Ok(Attribution {
        method: "parameter re-fit",
        perturbed,
        skipped,
        poles,
    })
}

/// Parses an asymptote name for options.
pub fn asymptote(s: &str) -> Result<Asymptote> {
    Asymptote::parse(s).ok_or_else(|| {
        Error::Netlist(format!(
            "vector fit: asymptote '{s}' must be zero, constant or linear"
        ))
    })
}
