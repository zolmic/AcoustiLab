//! Monte Carlo tolerance analysis and design of experiments (spec Sections
//! 3, 10 and 12).
//!
//! Work is split so a user interface can report progress and cancel:
//! [`plan`] lists the runs (each a set of parameter overrides), [`run`]
//! solves any slice of them, and [`envelope`] summarises the collected
//! results. Every run carries the reproducibility hash of its expanded
//! netlist ([`super::canonical`]) and the table exports as CSV ([`to_csv`]).
//!
//! # Latin hypercube sampling
//!
//! For n samples of d toleranced parameters, with the generator
//! [`Xoshiro256`] seeded by `seed` and each parameter in plan order (the
//! declaration order, or the order given):
//!
//! 1. draw a permutation π of 0..n (Fisher–Yates, [`Xoshiro256::shuffle`]);
//! 2. draw n jitters v_i = ((x >> 32) + ½)·2⁻³², x the next 64-bit output;
//! 3. u_i = (π(i) + v_i)/n, so every one of the n equal strata of (0, 1) is
//!    hit exactly once;
//! 4. map u_i through the parameter's distribution, with μ the parameter's
//!    current value and t its tolerance half-width at μ:
//!    * `normal`: μ + (t/2)·Φ⁻¹(u): the tolerance is two standard
//!      deviations (95.45 % coverage);
//!    * `uniform`: μ + t·(2u − 1), flat over μ ± t;
//!    * `lognormal` (needs `rel`): μ·exp(σ_ln·Φ⁻¹(u)) with
//!      σ_ln = ln(1 + rel)/2, so ln x is normal with median μ and its 2σ
//!      points are μ·(1 + rel) and μ/(1 + rel): +rel above, −rel/(1 + rel)
//!      below (for rel = 0.5: 1.5μ and 0.667μ);
//! 5. clip to the parameter's [min, max]; clipped values are counted per
//!    parameter and listed per sample.
//!
//! Φ⁻¹ and the exponential are [`super::detmath`]'s, which use only IEEE
//! basic operations, so a seed gives bit-identical samples on every
//! platform.
//!
//! # Designs of experiments
//!
//! * `factorial`: the full factorial of the given levels, first factor
//!   varying slowest. Levels are a list of values of any kind (numbers,
//!   booleans, choices) or `{"levels": k, "from": a, "to": b}` for k evenly
//!   spaced numbers (`"log": true` for geometric spacing).
//! * `runs`: a list of override objects, one per run.
//!
//! # Results and envelopes
//!
//! Each run reports, per probe, dB SPL for pressures, |Z| and phase for
//! impedances, and the magnitude otherwise; and the scalar readouts
//! ([`super::readouts::SCALARS`]) as metrics. [`envelope`] gives per
//! frequency the median, 5, 10, 90 and 95 % points, minimum and maximum
//! (percentiles by linear interpolation between order statistics, numpy's
//! default, Hyndman and Fan type 7), and the same for every metric.

use super::readouts::{ReadoutOptions, SCALARS};
use super::rng::Xoshiro256;
use super::{
    detmath, opt_nan_vec, options_error, overrides_from_json, overrides_serde, Design, Repr,
};
use crate::error::{Error, Result};
use crate::expr::PValue;
use crate::params::{Dist, Overrides, ParamKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Largest plan (runs). A plan is returned whole (a wasm call returns it as
/// one JSON text): 100 000 runs of the template's ten toleranced
/// parameters make about 38 MB of JSON, planned in 0.2 s natively; the
/// runs themselves take about 20 ms each.
pub const MAX_RUNS: usize = 100_000;

/// What to sample.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanSpec {
    /// Latin hypercube over the toleranced parameters.
    Lhs {
        n: usize,
        #[serde(default)]
        seed: u64,
        /// Default: every continuous parameter with a tolerance.
        parameters: Option<Vec<String>>,
    },
    /// Full factorial over the given levels.
    Factorial { factors: Map<String, Value> },
    /// Explicit runs.
    Runs { runs: Vec<Value> },
}

/// One run of a plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub index: usize,
    #[serde(with = "overrides_serde")]
    pub overrides: Overrides,
    /// Parameters clipped to their bounds in this sample.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clipped: Vec<String>,
}

/// The sampling distribution of one parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Distribution {
    pub name: String,
    pub dist: Dist,
    /// μ: the parameter's current value.
    pub nominal: f64,
    /// Tolerance half-width at μ, in the parameter's unit.
    pub half_width: f64,
    /// Standard deviation (normal).
    pub sigma: Option<f64>,
    /// Standard deviation of ln x (lognormal).
    pub sigma_ln: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Samples clipped to min or max.
    pub clipped: usize,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Factor {
    pub name: String,
    pub levels: Vec<Value>,
}

/// A list of runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub method: &'static str,
    pub seed: Option<u64>,
    /// Parameters the plan varies, in column order.
    pub parameters: Vec<String>,
    pub distributions: Vec<Distribution>,
    pub factors: Vec<Factor>,
    pub samples: Vec<Sample>,
    /// Total clipped values.
    pub clipped: usize,
    pub engine: &'static str,
}

impl PlanSpec {
    pub fn validate(&self) -> std::result::Result<(), String> {
        match self {
            PlanSpec::Lhs { n, .. } if *n == 0 || *n > MAX_RUNS => {
                Err(format!("'n' must be 1 to {MAX_RUNS}, got {n}"))
            }
            PlanSpec::Factorial { factors } if factors.is_empty() => {
                Err("'factors' is empty".into())
            }
            PlanSpec::Runs { runs } if runs.len() > MAX_RUNS => {
                Err(format!("at most {MAX_RUNS} runs"))
            }
            _ => Ok(()),
        }
    }
}

/// Latin hypercube points in the unit cube: `n` rows of `d` coordinates
/// (see the module documentation, steps 1 to 3).
pub fn lhs_unit(n: usize, d: usize, seed: u64) -> Vec<Vec<f64>> {
    let mut rng = Xoshiro256::seed_from(seed);
    let mut u = vec![vec![0.0; d]; n];
    let nf = n as f64;
    for j in 0..d {
        let mut perm: Vec<usize> = (0..n).collect();
        rng.shuffle(&mut perm);
        for (i, row) in u.iter_mut().enumerate() {
            let v = ((rng.next_u64() >> 32) as f64 + 0.5) * (1.0 / 4_294_967_296.0);
            row[j] = ((perm[i] as f64 + v) / nf).min(1.0 - f64::EPSILON / 2.0);
        }
    }
    u
}

impl Distribution {
    /// The value at unit coordinate u, before clipping (step 4).
    pub fn quantile(&self, u: f64) -> f64 {
        match self.dist {
            Dist::Normal => self.nominal + self.sigma.unwrap_or(0.0) * detmath::norm_quantile(u),
            Dist::Uniform => self.nominal + self.half_width * (2.0 * u - 1.0),
            Dist::Lognormal => {
                self.nominal
                    * detmath::exp(self.sigma_ln.unwrap_or(0.0) * detmath::norm_quantile(u))
            }
        }
    }

    fn clip(&self, x: f64) -> (f64, bool) {
        let y = self.min.map_or(x, |m| x.max(m));
        let y = self.max.map_or(y, |m| y.min(m));
        (y, y != x)
    }
}

/// Lists the runs of a plan.
pub fn plan(design: &Design, spec: &PlanSpec) -> Result<Plan> {
    spec.validate().map_err(|m| options_error("plan", m))?;
    match spec {
        PlanSpec::Lhs {
            n,
            seed,
            parameters,
        } => lhs_plan(design, *n, *seed, parameters.as_deref()),
        PlanSpec::Factorial { factors } => factorial_plan(design, factors),
        PlanSpec::Runs { runs } => runs_plan(design, runs),
    }
}

fn lhs_plan(design: &Design, n: usize, seed: u64, names: Option<&[String]>) -> Result<Plan> {
    if let Some(list) = names {
        super::check_unique(list)?;
    }
    let defs = match names {
        Some(list) => list
            .iter()
            .map(|name| {
                let d = design.def(name)?;
                if d.tolerance.is_none() || design.continuous_value(d).is_err() {
                    return Err(Error::Parameter {
                        name: name.clone(),
                        msg: "Monte Carlo samples continuous parameters with a tolerance".into(),
                    });
                }
                Ok(d)
            })
            .collect::<Result<Vec<_>>>()?,
        None => design
            .parametric
            .defs
            .iter()
            .filter(|d| d.tolerance.is_some() && design.continuous_value(d).is_ok())
            .collect(),
    };
    if defs.is_empty() {
        return Err(options_error(
            "plan",
            "no continuous parameter with a tolerance to sample",
        ));
    }
    let mut dists: Vec<Distribution> = defs
        .iter()
        .map(|d| {
            let t = d.tolerance.as_ref().expect("filtered on tolerance");
            let mu = design.continuous_value(d).expect("filtered on kind");
            let hw = t.half_width(mu);
            let (min, max) = d.bounds();
            Distribution {
                name: d.name.clone(),
                dist: t.dist,
                nominal: mu,
                half_width: hw,
                sigma: (t.dist == Dist::Normal).then_some(hw / 2.0),
                sigma_ln: (t.dist == Dist::Lognormal)
                    .then(|| detmath::ln(1.0 + t.rel.unwrap_or(0.0)) / 2.0),
                min,
                max,
                clipped: 0,
                source: t.source.clone(),
            }
        })
        .collect();
    let u = lhs_unit(n, dists.len(), seed);
    let mut samples = Vec::with_capacity(n);
    for (i, row) in u.iter().enumerate() {
        let mut overrides = Overrides::new();
        let mut clipped = Vec::new();
        for (d, &uj) in dists.iter_mut().zip(row) {
            let (x, c) = d.clip(d.quantile(uj));
            if c {
                d.clipped += 1;
                clipped.push(d.name.clone());
            }
            overrides.insert(d.name.clone(), PValue::Num(x));
        }
        samples.push(Sample {
            index: i,
            overrides,
            clipped,
        });
    }
    Ok(Plan {
        method: "lhs",
        seed: Some(seed),
        parameters: dists.iter().map(|d| d.name.clone()).collect(),
        clipped: dists.iter().map(|d| d.clipped).sum(),
        distributions: dists,
        factors: Vec::new(),
        samples,
        engine: crate::solve::ENGINE,
    })
}

fn factor_levels(design: &Design, name: &str, v: &Value) -> Result<Vec<PValue>> {
    let def = design.def(name)?;
    let perr = |msg: String| Error::Parameter {
        name: name.to_string(),
        msg,
    };
    let levels: Vec<PValue> = match v {
        Value::Array(a) => a
            .iter()
            .map(|x| {
                PValue::from_json(x)
                    .ok_or_else(|| perr("levels must be numbers, booleans or strings".into()))
            })
            .collect::<Result<_>>()?,
        Value::Object(o) => {
            if let Some(k) = o
                .keys()
                .find(|k| !["levels", "from", "to", "log"].contains(&k.as_str()))
            {
                return Err(perr(format!("factor: unknown key '{k}'")));
            }
            let k = o
                .get("levels")
                .and_then(Value::as_u64)
                .filter(|k| (2..=MAX_RUNS as u64).contains(k))
                .ok_or_else(|| {
                    perr(format!(
                        "factor 'levels' must be an integer from 2 to {MAX_RUNS}"
                    ))
                })? as usize;
            let num = |key: &str| {
                o.get(key)
                    .and_then(Value::as_f64)
                    .ok_or_else(|| perr(format!("factor needs a numeric '{key}'")))
            };
            let (a, b) = (num("from")?, num("to")?);
            let log = o.get("log").and_then(Value::as_bool).unwrap_or(false);
            if log && !(a > 0.0 && b > 0.0) {
                return Err(perr("a log factor needs positive 'from' and 'to'".into()));
            }
            let integer = matches!(def.kind, ParamKind::Number { integer: true, .. });
            (0..k)
                .map(|i| {
                    let t = i as f64 / (k - 1) as f64;
                    let x = if i == k - 1 {
                        b
                    } else if log {
                        a * (b / a).powf(t)
                    } else {
                        a + (b - a) * t
                    };
                    PValue::Num(if integer { x.round() } else { x })
                })
                .collect()
        }
        _ => {
            return Err(perr(
                "a factor is a list of levels or {levels, from, to}".into(),
            ))
        }
    };
    if levels.is_empty() {
        return Err(perr("a factor needs at least one level".into()));
    }
    for l in &levels {
        def.check(l).map_err(perr)?;
    }
    Ok(levels)
}

fn factorial_plan(design: &Design, factors: &Map<String, Value>) -> Result<Plan> {
    let mut list = Vec::new();
    let mut total: usize = 1;
    for (name, v) in factors {
        let levels = factor_levels(design, name, v)?;
        total = total.saturating_mul(levels.len());
        list.push((name.clone(), levels));
    }
    if total > MAX_RUNS {
        return Err(options_error(
            "plan",
            format!("the factorial has {total} runs; the limit is {MAX_RUNS}"),
        ));
    }
    let mut samples = Vec::with_capacity(total);
    for index in 0..total {
        let mut rest = index;
        let mut overrides = Overrides::new();
        // First factor slowest: mixed-radix digits from the last factor.
        for (name, levels) in list.iter().rev() {
            overrides.insert(name.clone(), levels[rest % levels.len()].clone());
            rest /= levels.len();
        }
        design.parametric.values(&design.merged(&overrides))?;
        samples.push(Sample {
            index,
            overrides,
            clipped: Vec::new(),
        });
    }
    Ok(Plan {
        method: "factorial",
        seed: None,
        parameters: list.iter().map(|(n, _)| n.clone()).collect(),
        distributions: Vec::new(),
        factors: list
            .into_iter()
            .map(|(name, levels)| Factor {
                name,
                levels: levels.iter().map(PValue::to_json).collect(),
            })
            .collect(),
        samples,
        clipped: 0,
        engine: crate::solve::ENGINE,
    })
}

fn runs_plan(design: &Design, runs: &[Value]) -> Result<Plan> {
    let mut parameters: Vec<String> = Vec::new();
    let mut samples = Vec::with_capacity(runs.len());
    for (index, r) in runs.iter().enumerate() {
        let overrides = overrides_from_json(r)
            .map_err(|m| options_error("plan", format!("run {index}: {m}")))?;
        design.parametric.values(&design.merged(&overrides))?;
        for k in overrides.keys() {
            if !parameters.contains(k) {
                parameters.push(k.clone());
            }
        }
        samples.push(Sample {
            index,
            overrides,
            clipped: Vec::new(),
        });
    }
    Ok(Plan {
        method: "runs",
        seed: None,
        parameters,
        distributions: Vec::new(),
        factors: Vec::new(),
        samples,
        clipped: 0,
        engine: crate::solve::ENGINE,
    })
}

/// Options of [`run`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunOptions {
    /// Probes whose curves are reported (default: all).
    pub probes: Option<Vec<String>>,
    /// Compute the scalar readouts of each run (default true).
    pub metrics: Option<bool>,
    pub readouts: Option<ReadoutOptions>,
}

/// One probe's curve in a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeCurve {
    pub id: String,
    /// dB SPL (pressures).
    #[serde(
        rename = "dB",
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_nan_vec"
    )]
    pub db: Option<Vec<f64>>,
    /// |y| in the probe's unit (impedances and other probes).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_nan_vec")]
    pub magnitude: Option<Vec<f64>>,
    /// Phase in degrees (impedances).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_nan_vec")]
    pub phase_deg: Option<Vec<f64>>,
}

/// The result of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub index: usize,
    /// The run's overrides (on top of the base overrides).
    #[serde(with = "overrides_serde")]
    pub overrides: Overrides,
    /// Reproducibility hash of the expanded netlist.
    pub hash: Option<String>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The run's grid when it differs from the chunk's.
    #[serde(
        rename = "frequencies_Hz",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub freqs_hz: Option<Vec<f64>>,
    #[serde(default)]
    pub curves: Vec<ProbeCurve>,
    #[serde(default)]
    pub metrics: BTreeMap<String, Option<f64>>,
}

/// Results of a slice of runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunChunk {
    pub engine: String,
    /// Grid of the base design (runs with another grid carry their own).
    #[serde(rename = "frequencies_Hz")]
    pub freqs_hz: Vec<f64>,
    pub samples: Vec<RunResult>,
}

impl RunOptions {
    pub fn validate(&self) -> std::result::Result<(), String> {
        self.readouts.as_ref().map_or(Ok(()), |r| r.validate())
    }
}

/// Solves a slice of runs. A run that fails (a singular matrix, an element
/// rejecting a value) is reported with `ok: false` and its error; the
/// others go on.
pub fn run(design: &Design, samples: &[Sample], opts: &RunOptions) -> Result<RunChunk> {
    opts.validate().map_err(|m| options_error("run", m))?;
    let base = design.base_point()?;
    if let Some(ids) = &opts.probes {
        for id in ids {
            base.probe(id)?;
        }
    }
    let grid = base.circuit.freqs.clone();
    let ui = design.ui_primary_probe();
    let ropts = opts.readouts.clone().unwrap_or_default();
    let metrics = opts.metrics.unwrap_or(true);
    let samples = samples
        .iter()
        .map(|s| run_one(design, s, opts, &ropts, metrics, ui.as_deref(), &grid))
        .collect();
    Ok(RunChunk {
        engine: crate::solve::ENGINE.to_string(),
        freqs_hz: grid,
        samples,
    })
}

fn run_one(
    design: &Design,
    s: &Sample,
    opts: &RunOptions,
    ropts: &ReadoutOptions,
    metrics: bool,
    ui: Option<&str>,
    grid: &[f64],
) -> RunResult {
    let mut out = RunResult {
        index: s.index,
        overrides: s.overrides.clone(),
        hash: None,
        ok: false,
        error: None,
        freqs_hz: None,
        curves: Vec::new(),
        metrics: BTreeMap::new(),
    };
    let attempt = || -> Result<RunResult> {
        let mut r = out.clone();
        let mut point = design.point(&s.overrides)?;
        r.hash = Some(point.hash());
        let plan = if metrics {
            Some(super::readouts::Plan::new(&point, ropts, ui)?)
        } else {
            None
        };
        let extra = plan.as_ref().map_or(Vec::new(), |p| p.extras.clone());
        let (result, extras) = point.solve_with(&extra)?;
        if result.freqs_hz != grid {
            r.freqs_hz = Some(result.freqs_hz.clone());
        }
        for p in &result.probes {
            if opts.probes.as_ref().is_some_and(|ids| !ids.contains(&p.id)) {
                continue;
            }
            let mut c = ProbeCurve {
                id: p.id.clone(),
                db: None,
                magnitude: None,
                phase_deg: None,
            };
            match super::repr(p) {
                Repr::Pressure => c.db = Some(p.spl_db()),
                Repr::Impedance => {
                    c.magnitude = Some(p.magnitude());
                    c.phase_deg = Some(p.phase_deg());
                }
                Repr::Magnitude => c.magnitude = Some(p.magnitude()),
            }
            r.curves.push(c);
        }
        if let Some(plan) = plan {
            r.metrics = plan.compute(&mut point, &result, &extras)?.scalars();
        }
        r.ok = true;
        Ok(r)
    };
    match attempt() {
        Ok(r) => r,
        Err(e) => {
            out.error = Some(e.to_string());
            out
        }
    }
}

/// Runs every sample in chunks of `chunk`, calling `progress(done, total)`
/// after each chunk; stops early (returning the runs so far) when it
/// returns false.
pub fn run_chunked(
    design: &Design,
    samples: &[Sample],
    opts: &RunOptions,
    chunk: usize,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<RunChunk> {
    let mut all: Option<RunChunk> = None;
    for part in samples.chunks(chunk.max(1)) {
        let c = run(design, part, opts)?;
        match &mut all {
            None => all = Some(c),
            Some(a) => a.samples.extend(c.samples),
        }
        let done = all.as_ref().map_or(0, |a| a.samples.len());
        if !progress(done, samples.len()) {
            break;
        }
    }
    match all {
        Some(a) => Ok(a),
        None => run(design, &[], opts),
    }
}

/// Percentile by linear interpolation between order statistics of sorted
/// data (numpy's default "linear" method): h = (n − 1)·q, the value is
/// a[⌊h⌋] + (h − ⌊h⌋)·(a[⌊h⌋+1] − a[⌊h⌋]), written as numpy writes it.
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return f64::NAN;
    }
    let h = (n - 1) as f64 * q;
    let lo = (h.floor() as usize).min(n - 1);
    let hi = (lo + 1).min(n - 1);
    let t = h - lo as f64;
    let (a, b) = (sorted[lo], sorted[hi]);
    let d = b - a;
    if t >= 0.5 {
        b - d * (1.0 - t)
    } else {
        a + d * t
    }
}

/// Order statistics of one quantity at every frequency.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    pub median: Vec<f64>,
    pub p5: Vec<f64>,
    pub p10: Vec<f64>,
    pub p90: Vec<f64>,
    pub p95: Vec<f64>,
    pub min: Vec<f64>,
    pub max: Vec<f64>,
    /// Finite values used at each frequency.
    pub n: Vec<usize>,
}

/// Order statistics of a scalar metric.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScalarStats {
    pub median: f64,
    pub p5: f64,
    pub p10: f64,
    pub p90: f64,
    pub p95: f64,
    pub min: f64,
    pub max: f64,
    pub n: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeEnvelope {
    pub id: String,
    #[serde(rename = "dB", skip_serializing_if = "Option::is_none")]
    pub db: Option<Stats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magnitude: Option<Stats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase_deg: Option<Stats>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope {
    #[serde(rename = "frequencies_Hz")]
    pub freqs_hz: Vec<f64>,
    /// Runs used (successful, on the common grid).
    pub runs: usize,
    pub failed: usize,
    /// Successful runs left out because their grid differs.
    pub other_grid: usize,
    pub probes: Vec<ProbeEnvelope>,
    pub metrics: BTreeMap<String, ScalarStats>,
    pub percentile_method: &'static str,
}

fn scalar_stats(mut v: Vec<f64>) -> ScalarStats {
    v.retain(|x| x.is_finite());
    v.sort_by(f64::total_cmp);
    ScalarStats {
        median: percentile(&v, 0.5),
        p5: percentile(&v, 0.05),
        p10: percentile(&v, 0.1),
        p90: percentile(&v, 0.9),
        p95: percentile(&v, 0.95),
        min: v.first().copied().unwrap_or(f64::NAN),
        max: v.last().copied().unwrap_or(f64::NAN),
        n: v.len(),
    }
}

/// Per-frequency order statistics of curves (rows: runs).
pub fn curve_stats(curves: &[&[f64]], nf: usize) -> Stats {
    let mut s = Stats {
        median: Vec::with_capacity(nf),
        p5: Vec::with_capacity(nf),
        p10: Vec::with_capacity(nf),
        p90: Vec::with_capacity(nf),
        p95: Vec::with_capacity(nf),
        min: Vec::with_capacity(nf),
        max: Vec::with_capacity(nf),
        n: Vec::with_capacity(nf),
    };
    for k in 0..nf {
        let st = scalar_stats(curves.iter().filter_map(|c| c.get(k).copied()).collect());
        s.median.push(st.median);
        s.p5.push(st.p5);
        s.p10.push(st.p10);
        s.p90.push(st.p90);
        s.p95.push(st.p95);
        s.min.push(st.min);
        s.max.push(st.max);
        s.n.push(st.n);
    }
    s
}

/// Envelopes of every probe quantity and metric over the successful runs
/// on the grid `freqs`.
pub fn envelope(freqs: &[f64], samples: &[RunResult]) -> Envelope {
    let failed = samples.iter().filter(|s| !s.ok).count();
    let used: Vec<&RunResult> = samples
        .iter()
        .filter(|s| s.ok && s.freqs_hz.as_ref().is_none_or(|f| f == freqs))
        .collect();
    let other_grid = samples.iter().filter(|s| s.ok).count() - used.len();
    let nf = freqs.len();
    let mut ids: Vec<String> = Vec::new();
    for s in &used {
        for c in &s.curves {
            if !ids.contains(&c.id) {
                ids.push(c.id.clone());
            }
        }
    }
    let probes = ids
        .into_iter()
        .map(|id| {
            let pick = |f: fn(&ProbeCurve) -> Option<&Vec<f64>>| -> Option<Stats> {
                let rows: Vec<&[f64]> = used
                    .iter()
                    .filter_map(|s| s.curves.iter().find(|c| c.id == id))
                    .filter_map(|c| f(c).map(Vec::as_slice))
                    .collect();
                (!rows.is_empty()).then(|| curve_stats(&rows, nf))
            };
            ProbeEnvelope {
                db: pick(|c| c.db.as_ref()),
                magnitude: pick(|c| c.magnitude.as_ref()),
                phase_deg: pick(|c| c.phase_deg.as_ref()),
                id,
            }
        })
        .collect();
    let mut names: Vec<&String> = used.iter().flat_map(|s| s.metrics.keys()).collect();
    names.sort();
    names.dedup();
    let metrics = names
        .into_iter()
        .map(|m| {
            let v: Vec<f64> = used
                .iter()
                .filter_map(|s| s.metrics.get(m).copied().flatten())
                .collect();
            (m.clone(), scalar_stats(v))
        })
        .collect();
    Envelope {
        freqs_hz: freqs.to_vec(),
        runs: used.len(),
        failed,
        other_grid,
        probes,
        metrics,
        percentile_method:
            "linear interpolation between order statistics (numpy default; Hyndman and Fan type 7)",
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn csv_num(x: Option<f64>) -> String {
    match x {
        Some(v) if v.is_finite() => format!("{v:?}"),
        _ => String::new(),
    }
}

/// The design-of-experiments table as CSV (RFC 4180): one row per run with
/// its index, hash, engine, the plan's parameter values, every metric and
/// the error of a failed run. Numbers are written in Rust's shortest
/// round-trip form; missing values are empty.
pub fn to_csv(parameters: &[String], samples: &[RunResult], engine: &str) -> String {
    let mut params: Vec<String> = parameters.to_vec();
    for s in samples {
        for k in s.overrides.keys() {
            if !params.contains(k) {
                params.push(k.clone());
            }
        }
    }
    let mut metrics: Vec<String> = SCALARS.iter().map(|s| s.to_string()).collect();
    for s in samples {
        for k in s.metrics.keys() {
            if !metrics.contains(k) {
                metrics.push(k.clone());
            }
        }
    }
    let has_metrics = samples.iter().any(|s| !s.metrics.is_empty());
    let mut header = vec!["run".to_string(), "hash".into(), "engine".into()];
    header.extend(params.iter().cloned());
    if has_metrics {
        header.extend(metrics.iter().cloned());
    }
    header.push("error".into());
    let mut out = header
        .iter()
        .map(|h| csv_field(h))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    for s in samples {
        let mut row = vec![
            s.index.to_string(),
            s.hash.clone().unwrap_or_default(),
            csv_field(engine),
        ];
        for p in &params {
            row.push(match s.overrides.get(p) {
                None => String::new(),
                Some(PValue::Num(x)) => csv_num(Some(*x)),
                Some(PValue::Bool(b)) => b.to_string(),
                Some(PValue::Str(t)) => csv_field(t),
            });
        }
        if has_metrics {
            for m in &metrics {
                row.push(csv_num(s.metrics.get(m).copied().flatten()));
            }
        }
        row.push(csv_field(s.error.as_deref().unwrap_or("")));
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_matches_the_linear_rule() {
        let v = [1.0, 2.0, 4.0, 8.0];
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&v, 1.0), 8.0);
        assert_eq!(percentile(&v, 0.5), 3.0);
        assert!((percentile(&v, 0.1) - 1.3).abs() < 1e-15);
        assert_eq!(percentile(&[5.0], 0.9), 5.0);
        assert!(percentile(&[], 0.5).is_nan());
    }

    #[test]
    fn lhs_hits_every_stratum_once() {
        let n = 97;
        let u = lhs_unit(n, 4, 12345);
        for j in 0..4 {
            let mut hit = vec![false; n];
            for row in &u {
                let k = (row[j] * n as f64).floor() as usize;
                assert!(!hit[k], "stratum {k} twice");
                hit[k] = true;
            }
        }
        assert_eq!(u, lhs_unit(n, 4, 12345));
        assert_ne!(u, lhs_unit(n, 4, 12346));
    }
}
