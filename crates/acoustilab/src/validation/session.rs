//! A measurement session against a frozen prediction set: import, sidecar
//! checks, averaging, blind comparison and the acceptance test (spec
//! Section 17; docs/reference-cup.md).
//!
//! **Files.** Seating k of measurement `M` is `M_s<k>.<ext>` with `ext` one
//! of `frd`, `zma`, `txt` (REW text) or `csv`, as the analyser exports it,
//! and its sidecar `M_s<k>.<ext>.sidecar.json` or `M_s<k>.sidecar.json`
//! (the name [`session_templates`] writes). Files that match no measurement
//! are listed, not read.
//!
//! **Sidecar checks.** Every file must state what the protocol prescribes:
//! the quantity; the configuration's fixture, ear simulator and pinna; the
//! reference point; the drive of the frozen prediction; `compensation:
//! "none"`; a calibrated level (pressure); a source impedance no higher
//! than the protocol's limit (pressure; for an impedance, which does not
//! depend on it, a warning); no smoothing coarser than 1/24 octave; an
//! origin of `measured` (or `virtual_rig`, which marks the session as
//! synthetic). Missing date, device or temperature, a temperature more than
//! 3 °C from the model's 23 °C, files that are not single seatings, or
//! points more than 1/12 octave apart above 20 Hz are warnings.
//!
//! **Averaging.** The seatings are resampled to the frozen grid within the
//! band they share ([`crate::io::Curve::resample`]) and averaged in dB (and
//! by the circular mean of the phase); their sample standard deviation s
//! is the observed repositioning spread. The measurement's standard
//! uncertainty is u_m = sqrt(s²/N + Σ systematic²), with the systematic
//! terms (coupler, calibration, fixture-to-human, numerical) of the first
//! sidecar's budget.
//!
//! **Blind comparison** (no fitting): the residual r = L_measured −
//! L_frozen nominal, the prediction's own spread u_p = (p95 − p5)/(2·1.645)
//! of the Monte Carlo envelope, and z = r/sqrt(u_p² + u_m²), reported per
//! band: RMS and largest |r|, the share of points with |z| ≤ 2 and of those
//! inside the 5–95 % envelope, and the largest |r| where the frozen solve
//! was not shaded.
//!
//! **Acceptance** (spec Section 17): for the pressure measurements of
//! configurations marked `acceptance`, the protocol's fit parameters (the
//! leak and the front volume) are fitted to the averaged curve with
//! [`crate::fit::fit`] (no level offset, the weights from u_m), and the
//! residual against the fitted model, at the drive the sidecars state,
//! must stay within each band's bound; above the last band it is reported
//! without a bound. A band the data do not reach at both ends is not
//! judged, and a curve that passes the bands it covers but misses one is
//! "not evaluated" rather than failed. With a
//! `driver_anchor` in the protocol, a second run first fits the driver to
//! its own free-air impedance and repeats the test with those values. It is
//! reported separately and is not the spec's criterion: it tells a driver
//! sample away from its datasheet apart from an error of the model.

use super::predict::{Envelope, Frozen};
use super::{
    netlist_on_grid, overrides_json, seating_stem, Configuration, Files, Measurement, VResult,
    ValidationError, CURVE_EXTENSIONS,
};
use crate::expr::PValue;
use crate::fit::{self, CurveSpec, FitParam, FitReport, FitSpec, Offset};
use crate::io::curve::wrap_deg;
use crate::io::sidecar::{Averaging, Origin, Profile, Smoothing, Uncertainty};
use crate::io::text::{self, Format};
use crate::io::{Curve, Quantity, Sidecar};
use crate::params::Overrides;
use crate::C64;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Schema tag of validation reports.
pub const REPORT_SCHEMA: &str = "acoustilab-validation-report/0.1";

/// z of the 5 and 95 % points of a normal distribution.
const Z95: f64 = 1.644_853_626_951_472_2;

/// Smallest standard uncertainty used in a z score, dB.
const MIN_U_DB: f64 = 1e-3;

/// Point density below which a measured curve is flagged: its points are
/// interpolated onto the 1/48-octave prediction grid.
const MIN_POINTS_PER_OCTAVE: f64 = 12.0;
/// The density is checked above this frequency (the acceptance band's
/// lower end).
const MIN_DENSE_HZ: f64 = 20.0;

/// Options of [`validate`].
#[derive(Debug, Clone)]
pub struct ValidateOptions {
    /// Overrides on top of every configuration's, for the fitted model
    /// (e.g. `fidelity` for a fast lumped check).
    pub extra_overrides: Overrides,
    /// Also run the acceptance test with the driver anchored by its own
    /// free-air impedance (needs the protocol's `driver_anchor`).
    pub anchor_driver: bool,
    /// Cap on model evaluations of one fit. A fit that reaches it is
    /// reported as not converged, and the bounds are checked where it
    /// stopped. A leak the data cannot see (a gasket that seals) is pushed
    /// towards its bound one geometric step at a time when the model misses
    /// something else, which is where the cap is usually reached.
    pub max_evaluations: usize,
    /// Fit on the exchange grid at this density instead of every point of
    /// the averaged curve (faster; the bounds are still checked at every
    /// point).
    pub fit_points_per_octave: Option<f64>,
}

impl Default for ValidateOptions {
    fn default() -> Self {
        ValidateOptions {
            extra_overrides: Overrides::new(),
            anchor_driver: true,
            max_evaluations: 200,
            fit_points_per_octave: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The file does not meet the protocol; its comparison is not trusted.
    Error,
    Warning,
    Note,
}

/// A deviation of a file (or of a measurement) from the protocol.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Issue {
    pub file: String,
    pub severity: Severity,
    pub field: String,
    pub message: String,
}

/// Residual statistics of one frequency band.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BandStats {
    #[serde(rename = "f_min_Hz")]
    pub f_min: f64,
    #[serde(rename = "f_max_Hz")]
    pub f_max: f64,
    pub points: usize,
    #[serde(rename = "rms_dB")]
    pub rms_db: f64,
    #[serde(rename = "max_abs_dB")]
    pub max_abs_db: f64,
    #[serde(rename = "at_Hz")]
    pub at_hz: f64,
    /// Share of points with |z| ≤ 2 (blind comparison).
    pub within_2u: Option<f64>,
    /// Share of points inside the 5–95 % Monte Carlo envelope.
    pub within_envelope: Option<f64>,
    /// The acceptance bound of the band, if it has one.
    #[serde(rename = "bound_dB")]
    pub bound_db: Option<f64>,
    pub pass: Option<bool>,
}

/// Residuals at every compared frequency.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Residual {
    #[serde(rename = "frequencies_Hz")]
    pub freqs: Vec<f64>,
    #[serde(rename = "level_dB")]
    pub level_db: Vec<f64>,
    /// z scores (blind comparison only).
    pub z: Option<Vec<f64>>,
    pub phase_deg: Option<Vec<f64>>,
}

/// Comparison with the frozen blind prediction.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Blind {
    pub bands: Vec<BandStats>,
    /// Largest |r| where the frozen solve was not shaded.
    #[serde(rename = "credible_max_abs_dB")]
    pub credible_max_abs_db: Option<f64>,
    pub max_abs_phase_deg: Option<f64>,
    pub residual: Residual,
}

/// A fitted parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fitted {
    pub name: String,
    pub start: f64,
    pub value: f64,
    pub ci95: Option<[f64; 2]>,
    pub status: fit::Status,
}

/// The acceptance test of one measurement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Acceptance {
    /// `spec` (datasheet driver) or `driver_anchored`.
    pub variant: &'static str,
    pub fitted: Vec<Fitted>,
    pub reduced_chi2: Option<f64>,
    pub converged: bool,
    pub stop_reason: String,
    /// Model evaluations of the fit.
    pub evaluations: usize,
    /// One entry per bound, then the unbounded band above.
    pub bands: Vec<BandStats>,
    /// `Some(false)` when a band the data cover exceeds its bound,
    /// `Some(true)` when every bounded band is covered and within its
    /// bound, `None` (not evaluated) when a bounded band is not covered and
    /// none fails: a curve that stops short of a band neither meets nor
    /// breaks its bound there.
    pub pass: Option<bool>,
    pub residual: Residual,
}

/// Everything found about one measurement of the protocol.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeasurementReport {
    pub id: String,
    pub configuration: String,
    pub label: String,
    pub quantity: &'static str,
    pub required_seatings: u32,
    pub files: Vec<String>,
    pub seatings: usize,
    /// Median and largest standard deviation between seatings over the
    /// compared band (pooled over ±1/6 octave), dB; two or more seatings.
    #[serde(rename = "seating_sd_median_dB")]
    pub seating_sd_median_db: Option<f64>,
    #[serde(rename = "seating_sd_max_dB")]
    pub seating_sd_max_db: Option<f64>,
    /// `missing`, `incomplete` or `complete`.
    pub status: &'static str,
    pub issues: Vec<Issue>,
    pub blind: Option<Blind>,
    pub acceptance: Vec<Acceptance>,
    /// Why a comparison or fit could not be made.
    pub errors: Vec<String>,
}

/// The driver fitted to its own free-air impedance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Anchor {
    pub measurement: String,
    pub fitted: Vec<Fitted>,
    pub reduced_chi2: Option<f64>,
    pub converged: bool,
    pub overrides: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    /// Every measurement has at least its required seatings.
    pub complete: bool,
    pub missing: Vec<String>,
    pub incomplete: Vec<String>,
    /// Files with sidecar errors.
    pub sidecar_errors: usize,
    /// Some files are virtual-rig curves: not a validation.
    pub synthetic: bool,
    /// Acceptance of the reference states (spec Section 17): false when one
    /// of them fails, true when every one of them passes, None otherwise
    /// (one is missing or does not cover an acceptance band).
    pub reference_pass: Option<bool>,
    pub text: Vec<String>,
}

/// The validation report (`acoustilab-validation-report/0.1`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub engine: &'static str,
    pub frozen: Value,
    pub protocol: String,
    pub acceptance_source: String,
    pub measurements: Vec<MeasurementReport>,
    pub driver_anchor: Option<Anchor>,
    pub unrecognised_files: Vec<String>,
    pub verdict: Verdict,
}

impl Report {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn measurement(&self, id: &str) -> Option<&MeasurementReport> {
        self.measurements.iter().find(|m| m.id == id)
    }
}

/// One seating as read.
struct Seating {
    file: String,
    curve: Curve,
}

/// Seating number of `name` for measurement `id` (`<id>_s<k>.<ext>`).
fn seating_of(name: &str, id: &str) -> Option<u32> {
    let rest = name.strip_prefix(id)?.strip_prefix("_s")?;
    let (k, ext) = rest.split_once('.')?;
    if !CURVE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
        return None;
    }
    k.parse::<u32>().ok().filter(|k| *k >= 1)
}

fn placeholder(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.contains("FILL IN") || t.contains("YYYY")
}

fn issue(file: &str, severity: Severity, field: &str, message: impl Into<String>) -> Issue {
    Issue {
        file: file.to_string(),
        severity,
        field: field.to_string(),
        message: message.into(),
    }
}

/// Checks one sidecar against the protocol and the frozen prediction's
/// sidecar (module documentation).
fn check_sidecar(
    file: &str,
    sc: &Sidecar,
    expected: &Sidecar,
    c: &Configuration,
    m: &Measurement,
    zs_max: f64,
) -> Vec<Issue> {
    use Severity::*;
    let mut out = Vec::new();
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    if sc.quantity != expected.quantity {
        out.push(issue(
            file,
            Error,
            "quantity",
            format!(
                "states {}, the protocol measures {}",
                sc.quantity.map_or("nothing", |q| q.name()),
                expected.quantity.map_or("?", |q| q.name())
            ),
        ));
    }
    for (k, v) in &c.sidecar {
        let want = v.as_str().unwrap_or("");
        let got = match k.as_str() {
            "fixture" => &sc.fixture,
            "ear_simulator" => &sc.ear_simulator,
            "pinna" => &sc.pinna,
            _ => continue,
        };
        match got {
            Some(g) if norm(g) == norm(want) => {}
            Some(g) => out.push(issue(
                file,
                Error,
                k,
                format!("states '{g}', the protocol prescribes '{want}'"),
            )),
            None => out.push(issue(
                file,
                Error,
                k,
                format!("not stated; the protocol prescribes '{want}'"),
            )),
        }
    }
    match &sc.reference_point {
        Some(r) if norm(r) == norm(&m.reference_point) => {}
        r => out.push(issue(
            file,
            Error,
            "reference_point",
            format!(
                "{}; the protocol prescribes '{}'",
                r.as_ref()
                    .map_or("not stated".to_string(), |r| format!("states '{r}'")),
                m.reference_point
            ),
        )),
    }
    match (&sc.drive, &expected.drive) {
        (Some(a), Some(b)) if a.same_as(b) => {}
        (a, b) => out.push(issue(
            file,
            Error,
            "drive",
            format!(
                "{}; the frozen prediction is at {}",
                a.as_ref().map_or("not stated".to_string(), |d| format!(
                    "states {}",
                    d.label()
                )),
                b.as_ref().map_or("?".to_string(), |d| d.label())
            ),
        )),
    }
    match sc.compensation.as_deref() {
        Some(x) if norm(x) == "none" => {}
        x => out.push(issue(
            file,
            Error,
            "compensation",
            format!(
                "{}; the protocol compares uncompensated curves ('none')",
                x.map_or("not stated".to_string(), |x| format!("states '{x}'"))
            ),
        )),
    }
    if expected.quantity == Some(Quantity::Pressure) && sc.calibrated != Some(true) {
        out.push(issue(
            file,
            Error,
            "calibrated",
            "the level must be calibrated (absolute dB SPL at the stated drive)",
        ));
    }
    // The source impedance sets the pressure at a given open-circuit drive,
    // but not the impedance at the terminals of a linear driver: a
    // series-resistor impedance jig is fine for an impedance-only file.
    let impedance = expected.quantity == Some(Quantity::Impedance);
    match sc.source_impedance_ohm {
        None => out.push(issue(
            file,
            if impedance { Warning } else { Error },
            "source_impedance_ohm",
            "not stated; measure the amplifier's output impedance and state it",
        )),
        Some(z) if z > zs_max => out.push(issue(
            file,
            if impedance { Warning } else { Error },
            "source_impedance_ohm",
            if impedance {
                format!("{z} ohm exceeds the protocol's {zs_max} ohm; the impedance does not depend on it, but a pressure taken in the same sweep does")
            } else {
                format!("{z} ohm exceeds the protocol's {zs_max} ohm")
            },
        )),
        _ => {}
    }
    match sc.smoothing {
        None => out.push(issue(
            file,
            Warning,
            "smoothing",
            "not stated; taken as none",
        )),
        Some(Smoothing::Octave(n)) if n < 24 => out.push(issue(
            file,
            Error,
            "smoothing",
            format!("1/{n} octave is coarser than the protocol allows (1/24 at most; export unsmoothed)"),
        )),
        Some(Smoothing::Octave(n)) => out.push(issue(
            file,
            Warning,
            "smoothing",
            format!("1/{n} octave; the protocol asks for none"),
        )),
        Some(Smoothing::None) => {}
    }
    if sc.seatings.is_some_and(|n| n != 1) {
        out.push(issue(
            file,
            Warning,
            "seatings",
            "each file should hold one seating; the tool averages them",
        ));
    }
    if sc.averaging.is_some_and(|a| a != Averaging::None) {
        out.push(issue(
            file,
            Warning,
            "averaging",
            "each file should hold one seating, not an average",
        ));
    }
    match &sc.date {
        Some(d) if !placeholder(d) => {}
        _ => out.push(issue(file, Warning, "date", "not filled in")),
    }
    match &sc.device {
        Some(d) if !placeholder(d) => {}
        _ => out.push(issue(file, Warning, "device", "not filled in")),
    }
    match sc.temperature_c {
        None => out.push(issue(file, Warning, "temperature_C", "not stated")),
        Some(t) if (t - 23.0).abs() > 3.0 => out.push(issue(
            file,
            Warning,
            "temperature_C",
            format!("{t} °C: the model's air is at 23 °C; resonances and modes shift by about 0.17 % per °C"),
        )),
        _ => {}
    }
    match sc.provenance.as_ref().and_then(|p| p.origin) {
        Some(Origin::Measured) => {}
        Some(Origin::VirtualRig) => out.push(issue(
            file,
            Note,
            "provenance",
            "virtual-rig curve: synthetic, not a measurement",
        )),
        Some(o) => out.push(issue(
            file,
            Error,
            "provenance",
            format!("origin '{}': a session holds measured curves", o.name()),
        )),
        None => out.push(issue(
            file,
            Warning,
            "provenance",
            "origin not stated; taken as measured",
        )),
    }
    out
}

/// Systematic level terms of a budget at f (they do not average down).
fn systematic_db(u: Option<&Uncertainty>, f: f64) -> f64 {
    let Some(u) = u else { return 0.0 };
    [
        &u.coupler_db,
        &u.microphone_calibration_db,
        &u.fixture_to_human_db,
        &u.numerical_db,
    ]
    .iter()
    .filter_map(|t| t.as_ref().map(|p| p.at(f).powi(2)))
    .sum::<f64>()
    .sqrt()
}

/// Per-seating level terms a sidecar states (used when a single seating
/// leaves the spread unobserved).
fn per_seating_db(u: Option<&Uncertainty>, f: f64) -> f64 {
    let Some(u) = u else { return 0.0 };
    [&u.repositioning_db, &u.noise_db]
        .iter()
        .filter_map(|t| t.as_ref().map(|p| p.at(f).powi(2)))
        .sum::<f64>()
        .sqrt()
}

/// Half-width of the window over which the seating spread is pooled,
/// octaves.
const SPREAD_WINDOW_OCT: f64 = 1.0 / 6.0;

/// The seating spread pooled over ±1/6 octave: sqrt of the mean variance
/// in the window. A standard deviation from five seatings has only four
/// degrees of freedom (its 95 % range spans a factor of 4), and the
/// repositioning spread varies smoothly with frequency, so pooling
/// neighbours gives weights a fit can trust.
fn smoothed_sd(freqs: &[f64], sd: &[f64]) -> Vec<f64> {
    let l: Vec<f64> = freqs.iter().map(|f| f.log2()).collect();
    (0..freqs.len())
        .map(|i| {
            let (mut sum, mut k) = (0.0, 0usize);
            for j in 0..freqs.len() {
                if (l[j] - l[i]).abs() <= SPREAD_WINDOW_OCT {
                    sum += sd[j] * sd[j];
                    k += 1;
                }
            }
            (sum / k as f64).sqrt()
        })
        .collect()
}

/// Seatings averaged on a grid.
struct Averaged {
    freqs: Vec<f64>,
    /// Indices into the frozen grid.
    index: Vec<usize>,
    level_db: Vec<f64>,
    phase_deg: Option<Vec<f64>>,
    /// Standard uncertainty of the mean level, dB.
    u_db: Vec<f64>,
    /// Sample standard deviation across seatings (NaN for one seating).
    sd_db: Vec<f64>,
    n: usize,
    sidecar: Sidecar,
}

fn average(seatings: &[Seating], env: &Envelope) -> VResult<Averaged> {
    let lo = seatings
        .iter()
        .map(|s| s.curve.freqs_hz[0])
        .fold(f64::MIN, f64::max);
    let hi = seatings
        .iter()
        .map(|s| s.curve.freqs_hz[s.curve.len() - 1])
        .fold(f64::MAX, f64::min);
    let index: Vec<usize> = (0..env.freqs.len())
        .filter(|&i| env.freqs[i] >= lo && env.freqs[i] <= hi)
        .collect();
    if index.len() < 2 {
        return Err(ValidationError(format!(
            "the seatings share fewer than two frequencies of the prediction grid (common band {lo} to {hi} Hz)"
        )));
    }
    let freqs: Vec<f64> = index.iter().map(|&i| env.freqs[i]).collect();
    let n = seatings.len();
    let mut levels = Vec::with_capacity(n);
    let mut phases = Vec::with_capacity(n);
    for s in seatings {
        let r = s.curve.resample(&freqs)?;
        levels.push(r.level_db());
        phases.push(r.phase_deg);
    }
    let with_phase =
        seatings[0].curve.quantity == Quantity::Impedance && phases.iter().all(Option::is_some);
    let m = freqs.len();
    let mut level_db = vec![0.0; m];
    let mut sd_db = vec![f64::NAN; m];
    for i in 0..m {
        let mean = levels.iter().map(|l| l[i]).sum::<f64>() / n as f64;
        level_db[i] = mean;
        if n >= 2 {
            let v = levels.iter().map(|l| (l[i] - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
            sd_db[i] = v.sqrt();
        }
    }
    if n >= 2 {
        sd_db = smoothed_sd(&freqs, &sd_db);
    }
    // Circular mean: the argument of the mean unit phasor.
    let phase_deg = with_phase.then(|| {
        (0..m)
            .map(|i| {
                phases
                    .iter()
                    .map(|p| C64::from_polar(1.0, p.as_ref().unwrap()[i].to_radians()))
                    .sum::<C64>()
                    .arg()
                    .to_degrees()
            })
            .collect()
    });
    let first = &seatings[0].curve.sidecar;
    let budget = first.uncertainty.as_ref();
    let u_db: Vec<f64> = freqs
        .iter()
        .enumerate()
        .map(|(i, &f)| {
            let spread = if n >= 2 {
                sd_db[i].powi(2) / n as f64
            } else {
                per_seating_db(budget, f).powi(2)
            };
            (spread + systematic_db(budget, f).powi(2)).sqrt()
        })
        .collect();
    // The averaged curve's budget: the stated systematic terms, and the
    // observed spread as the repositioning term (it includes the noise).
    let mut sc = first.clone();
    sc.seatings = Some(n as u32);
    sc.averaging = Some(Averaging::Db);
    let mut u = budget.cloned().unwrap_or_default();
    if n >= 2 {
        u.repositioning_db = Some(Profile::Table {
            freqs: freqs.clone(),
            values: sd_db.iter().map(|s| s.max(MIN_U_DB)).collect(),
        });
        u.noise_db = None;
    }
    sc.uncertainty = (!u.is_empty()).then_some(u);
    Ok(Averaged {
        freqs,
        index,
        level_db,
        phase_deg,
        u_db,
        sd_db,
        n,
        sidecar: sc,
    })
}

/// Bands of a report: below the first bound, each bound, above the last.
fn report_bands(protocol: &super::Protocol, f_lo: f64, f_hi: f64) -> Vec<(f64, f64, Option<f64>)> {
    let a = &protocol.acceptance;
    let mut out = Vec::new();
    let first = a.bands.first().map_or(a.report_above, |b| b.f_min);
    if f_lo < first {
        out.push((f_lo, first, None));
    }
    for b in &a.bands {
        out.push((b.f_min, b.f_max, Some(b.max_abs_db)));
    }
    if f_hi > a.report_above {
        out.push((a.report_above, f_hi, None));
    }
    out
}

/// Statistics of residuals `r` within [lo, hi]. For the band below the
/// first bound the upper edge is excluded, for the band above the last the
/// lower edge, so that no point is counted twice with a bound.
fn band_stats(
    freqs: &[f64],
    r: &[f64],
    z: Option<&[f64]>,
    inside: Option<&[bool]>,
    (lo, hi, bound): (f64, f64, Option<f64>),
    open_lo: bool,
    open_hi: bool,
) -> BandStats {
    let idx: Vec<usize> = (0..freqs.len())
        .filter(|&i| {
            let f = freqs[i];
            (if open_lo { f > lo } else { f >= lo }) && (if open_hi { f < hi } else { f <= hi })
        })
        .collect();
    let mut s = BandStats {
        f_min: lo,
        f_max: hi,
        points: idx.len(),
        rms_db: f64::NAN,
        max_abs_db: f64::NAN,
        at_hz: f64::NAN,
        within_2u: None,
        within_envelope: None,
        bound_db: bound,
        pass: None,
    };
    if idx.is_empty() {
        return s;
    }
    s.rms_db = (idx.iter().map(|&i| r[i] * r[i]).sum::<f64>() / idx.len() as f64).sqrt();
    let (mut worst, mut at) = (-1.0, f64::NAN);
    for &i in &idx {
        if r[i].abs() > worst {
            worst = r[i].abs();
            at = freqs[i];
        }
    }
    s.max_abs_db = worst;
    s.at_hz = at;
    let share = |ok: &dyn Fn(usize) -> bool| {
        idx.iter().filter(|&&i| ok(i)).count() as f64 / idx.len() as f64
    };
    if let Some(z) = z {
        s.within_2u = Some(share(&|i| z[i].abs() <= 2.0));
    }
    if let Some(e) = inside {
        s.within_envelope = Some(share(&|i| e[i]));
    }
    if let Some(b) = bound {
        s.pass = Some(worst <= b);
    }
    s
}

fn blind(a: &Averaged, env: &Envelope, protocol: &super::Protocol) -> Blind {
    let q = env.quantity;
    let lvl = |x: f64| match q {
        Quantity::Pressure => x,
        _ => 20.0 * x.log10(),
    };
    let m = a.freqs.len();
    let mut r = vec![0.0; m];
    let mut z = vec![0.0; m];
    let mut inside = vec![false; m];
    let mut credible: Option<f64> = None;
    for k in 0..m {
        let i = a.index[k];
        r[k] = a.level_db[k] - lvl(env.nominal[i]);
        let (p5, p95) = (lvl(env.p5[i]), lvl(env.p95[i]));
        let u_p = (p95 - p5).abs() / (2.0 * Z95);
        let u = (u_p * u_p + a.u_db[k] * a.u_db[k]).sqrt().max(MIN_U_DB);
        z[k] = r[k] / u;
        inside[k] = a.level_db[k] >= p5.min(p95) && a.level_db[k] <= p5.max(p95);
        if env.shading[i] == 0 {
            credible = Some(credible.unwrap_or(0.0).max(r[k].abs()));
        }
    }
    let phase = match (&a.phase_deg, &env.phase) {
        (Some(p), Some(ph)) => Some(
            (0..m)
                .map(|k| wrap_deg(p[k] - ph[0][a.index[k]]))
                .collect::<Vec<f64>>(),
        ),
        _ => None,
    };
    let bands_spec = report_bands(protocol, a.freqs[0], a.freqs[m - 1]);
    let nb = bands_spec.len();
    let bands = bands_spec
        .into_iter()
        .enumerate()
        .map(|(j, b)| {
            let mut s = band_stats(
                &a.freqs,
                &r,
                Some(&z),
                Some(&inside),
                b,
                b.2.is_none() && j + 1 == nb && nb > 1,
                b.2.is_none() && j == 0 && nb > 1,
            );
            // The blind comparison has no pass/fail: the bounds belong to
            // the fitted acceptance test.
            s.bound_db = None;
            s.pass = None;
            s
        })
        .collect();
    Blind {
        bands,
        credible_max_abs_db: credible,
        max_abs_phase_deg: phase
            .as_ref()
            .map(|p| p.iter().fold(0.0_f64, |a, x| a.max(x.abs()))),
        residual: Residual {
            freqs: a.freqs.clone(),
            level_db: r,
            z: Some(z),
            phase_deg: phase,
        },
    }
}

fn fitted_of(rep: &FitReport) -> Vec<Fitted> {
    rep.parameters
        .iter()
        .map(|p| Fitted {
            name: p.name.clone(),
            start: p.start,
            value: p.value,
            ci95: p.ci95,
            status: p.status,
        })
        .collect()
}

fn fitted_overrides(rep: &FitReport) -> Overrides {
    rep.fitted
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| PValue::from_json(v).map(|p| (k.clone(), p)))
        .collect()
}

/// `o` without the fitted parameters (a fit refuses a parameter that is
/// both fitted and fixed; their values under `o` are the starts).
fn without(o: &Overrides, fitted: &[super::FitParamSpec]) -> Overrides {
    o.iter()
        .filter(|(k, _)| !fitted.iter().any(|f| &f.name == *k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Mean source impedance the files state (None if none does).
fn stated_source_impedance(seatings: &[Seating]) -> Option<f64> {
    let z: Vec<f64> = seatings
        .iter()
        .filter_map(|s| s.curve.sidecar.source_impedance_ohm)
        .collect();
    (!z.is_empty()).then(|| z.iter().sum::<f64>() / z.len() as f64)
}

struct Ctx<'a> {
    frozen: &'a Frozen,
    opts: &'a ValidateOptions,
}

impl Ctx<'_> {
    fn base_overrides(
        &self,
        c: &Configuration,
        zs: Option<f64>,
        extra: &Overrides,
    ) -> VResult<Overrides> {
        let mut o = self.frozen.protocol.overrides_for(c);
        o.extend(self.opts.extra_overrides.clone());
        let p = self.frozen.parametric(c)?;
        if let Some(z) = zs {
            if p.def("source_impedance_ohm").is_some() {
                o.insert("source_impedance_ohm".into(), PValue::Num(z));
            }
        }
        o.extend(extra.clone());
        Ok(o)
    }

    /// The curve a fit sees ([`ValidateOptions::fit_points_per_octave`]).
    fn thinned(&self, curve: Curve) -> VResult<Curve> {
        match self.opts.fit_points_per_octave {
            None => Ok(curve),
            Some(ppo) => {
                let g = crate::io::curve::exchange_grid(
                    curve.freqs_hz[0],
                    curve.freqs_hz[curve.len() - 1],
                    ppo,
                );
                if g.len() < 2 {
                    return Ok(curve);
                }
                Ok(curve.resample(&g)?)
            }
        }
    }

    fn fit_params(
        &self,
        c: &Configuration,
        specs: &[super::FitParamSpec],
        o: &Overrides,
    ) -> VResult<Vec<FitParam>> {
        let p = self.frozen.parametric(c)?;
        let values = p.values(o)?;
        specs
            .iter()
            .map(|s| {
                let v = values
                    .iter()
                    .find(|(n, _)| *n == s.name)
                    .and_then(|(_, v)| v.as_num())
                    .ok_or_else(|| {
                        ValidationError(format!("fit parameter '{}' has no numeric value", s.name))
                    })?;
                let mut fp = FitParam::new(&s.name);
                fp.min = s.min;
                fp.max = s.max;
                // A start on or beyond a bound of a log-scale parameter
                // (a residual leak of 0) moves just inside it.
                fp.start = Some(match (s.min, s.max) {
                    (Some(lo), _) if v < lo => lo,
                    (_, Some(hi)) if v > hi => hi,
                    _ => v,
                });
                Ok(fp)
            })
            .collect()
    }

    /// The acceptance test of an averaged pressure curve.
    fn acceptance(
        &self,
        c: &Configuration,
        m: &Measurement,
        a: &Averaged,
        zs: Option<f64>,
        variant: &'static str,
        anchor: &Overrides,
    ) -> VResult<Acceptance> {
        let protocol = &self.frozen.protocol;
        let o = self.base_overrides(c, zs, anchor)?;
        let mut curve = Curve::from_db(Quantity::Pressure, &a.freqs, &a.level_db, None)?;
        curve.sidecar = a.sidecar.clone();
        let curve = self.thinned(curve)?;
        let mut cs = CurveSpec::new(&m.probe, curve);
        cs.offset = Some(Offset::None);
        cs.use_phase = Some(false);
        let params = self.fit_params(c, &protocol.fit.parameters, &o)?;
        let mut spec = FitSpec::new(params, vec![cs]);
        spec.overrides = without(&o, &protocol.fit.parameters);
        spec.f_min = protocol.fit.f_min;
        spec.f_max = protocol.fit.f_max;
        spec.max_evaluations = self.opts.max_evaluations;
        let p = self.frozen.parametric(c)?;
        let rep = fit::fit(&p, &spec)?;
        let mut fo = o;
        fo.extend(fitted_overrides(&rep));
        let pg = netlist_on_grid(&self.frozen.netlist_text, &a.freqs)?;
        let mut circuit = crate::Circuit::from_parametric(&pg, &fo)?;
        // The fit simulated the curve at the drive its sidecar states
        // (docs/fitting.md); the residual is taken at the same drive.
        if let Some(d) = &a.sidecar.drive {
            circuit.drive = Some(d.spec.clone());
        }
        let r = circuit.solve()?;
        let model = r
            .probe(&m.probe)
            .ok_or_else(|| ValidationError(format!("no probe '{}'", m.probe)))?
            .spl_db();
        let res: Vec<f64> = a.level_db.iter().zip(&model).map(|(x, y)| x - y).collect();
        let spec_bands = report_bands(protocol, a.freqs[0], a.freqs[a.freqs.len() - 1]);
        let nb = spec_bands.len();
        // A bound holds only where the data reach both ends of its band
        // (within 1/24 octave).
        let edge = 2f64.powf(1.0 / 24.0);
        let (f_first, f_last) = (a.freqs[0], a.freqs[a.freqs.len() - 1]);
        let bands: Vec<BandStats> = spec_bands
            .into_iter()
            .enumerate()
            .filter(|(_, b)| b.2.is_some() || b.0 >= protocol.acceptance.report_above)
            .map(|(j, b)| {
                let mut s = band_stats(
                    &a.freqs,
                    &res,
                    None,
                    None,
                    b,
                    b.2.is_none() && j + 1 == nb,
                    false,
                );
                if b.2.is_some() && (f_first > b.0 * edge || f_last < b.1 / edge) {
                    s.pass = None;
                }
                s
            })
            .collect();
        let bounded: Vec<&BandStats> = bands.iter().filter(|b| b.bound_db.is_some()).collect();
        let pass = if bounded.iter().any(|b| b.pass == Some(false)) {
            Some(false)
        } else if !bounded.is_empty() && bounded.iter().all(|b| b.pass == Some(true)) {
            Some(true)
        } else {
            None
        };
        Ok(Acceptance {
            variant,
            fitted: fitted_of(&rep),
            reduced_chi2: rep.reduced_chi2,
            converged: rep.converged,
            stop_reason: rep.stop_reason.to_string(),
            evaluations: rep.evaluations,
            bands,
            pass,
            residual: Residual {
                freqs: a.freqs.clone(),
                level_db: res,
                z: None,
                phase_deg: None,
            },
        })
    }

    /// The driver fitted to its free-air impedance.
    fn anchor(&self, c: &Configuration, m: &Measurement, a: &Averaged) -> VResult<Anchor> {
        let da = self
            .frozen
            .protocol
            .driver_anchor
            .as_ref()
            .expect("checked by the caller");
        // The impedance at the terminals does not depend on the source
        // impedance, which may be a series-resistor jig's (above the
        // netlist's range): the netlist's value stays.
        let o = self.base_overrides(c, None, &Overrides::new())?;
        let q = Quantity::Impedance;
        let mag: Vec<f64> = a.level_db.iter().map(|l| 10f64.powf(l / 20.0)).collect();
        let mut curve = Curve::new(q, &a.freqs, &mag, a.phase_deg.as_deref())?;
        curve.sidecar = a.sidecar.clone();
        let curve = self.thinned(curve)?;
        let cs = CurveSpec::new(&m.probe, curve);
        let mut spec = FitSpec::new(self.fit_params(c, &da.parameters, &o)?, vec![cs]);
        spec.overrides = without(&o, &da.parameters);
        spec.f_min = self.frozen.protocol.fit.f_min;
        spec.f_max = self.frozen.protocol.fit.f_max;
        spec.max_evaluations = self.opts.max_evaluations;
        let rep = fit::fit(&self.frozen.parametric(c)?, &spec)?;
        Ok(Anchor {
            measurement: m.id.clone(),
            fitted: fitted_of(&rep),
            reduced_chi2: rep.reduced_chi2,
            converged: rep.converged,
            overrides: rep.fitted.clone(),
        })
    }
}

/// Reads the seatings of one measurement; returns them with their issues.
fn read_seatings(
    files: &Files,
    frozen: &Frozen,
    c: &Configuration,
    m: &Measurement,
    used: &mut BTreeSet<String>,
) -> (Vec<Seating>, Vec<String>, Vec<Issue>) {
    let expected = &frozen.nominal[&m.id].sidecar;
    let q = frozen.nominal[&m.id].quantity;
    let mut names: Vec<(u32, &String)> = files
        .keys()
        .filter_map(|n| seating_of(n, &m.id).map(|k| (k, n)))
        .collect();
    names.sort();
    let mut seatings = Vec::new();
    let mut listed = Vec::new();
    let mut issues = Vec::new();
    let mut seen = BTreeSet::new();
    for (k, name) in names {
        used.insert(name.clone());
        listed.push(name.clone());
        if !seen.insert(k) {
            issues.push(issue(
                name,
                Severity::Error,
                "file",
                format!("a second file for seating {k}; only the first is used"),
            ));
            continue;
        }
        let sc_names = [
            format!("{name}.sidecar.json"),
            format!("{}.sidecar.json", seating_stem(&m.id, k)),
        ];
        let sc_name = sc_names.iter().find(|s| files.contains_key(*s));
        let sidecar = match sc_name {
            None => {
                issues.push(issue(
                    name,
                    Severity::Error,
                    "sidecar",
                    format!("no sidecar ({} or {})", sc_names[0], sc_names[1]),
                ));
                None
            }
            Some(s) => {
                used.insert(s.clone());
                match String::from_utf8(files[s].clone())
                    .map_err(|_| "not UTF-8".to_string())
                    .and_then(|t| Sidecar::parse(&t).map_err(|e| e.to_string()))
                {
                    Ok(sc) => Some(sc),
                    Err(e) => {
                        // Keyed to the curve file, which is left out.
                        issues.push(issue(name, Severity::Error, "sidecar", format!("{s}: {e}")));
                        None
                    }
                }
            }
        };
        let fmt = Format::from_path(name).unwrap_or(Format::Auto);
        let curve = String::from_utf8(files[name].clone())
            .map_err(|_| "not UTF-8".to_string())
            .and_then(|t| text::import(&t, fmt, Some(q)).map_err(|e| e.to_string()));
        let mut curve = match curve {
            Ok(c) => c,
            Err(e) => {
                issues.push(issue(name, Severity::Error, "curve", e));
                continue;
            }
        };
        if curve.quantity != q {
            issues.push(issue(
                name,
                Severity::Error,
                "quantity",
                format!(
                    "the file holds {}, the protocol measures {}",
                    curve.quantity.name(),
                    q.name()
                ),
            ));
            continue;
        }
        if let Some(sc) = sidecar {
            issues.extend(check_sidecar(
                name,
                &sc,
                expected,
                c,
                m,
                frozen.protocol.source_impedance_max_ohm,
            ));
            curve.sidecar = sc;
            curve.sidecar.quantity = Some(q);
        }
        // The seatings are interpolated onto the prediction grid: a sparse
        // export (third-octave points, a short FFT at low frequency) fills
        // in notches and peaks the model has.
        let widest = curve
            .freqs_hz
            .windows(2)
            .filter(|w| w[1] > MIN_DENSE_HZ)
            .map(|w| (w[1] / w[0]).log2())
            .fold(0.0, f64::max);
        // (1 % of slack: exports round their frequencies.)
        if widest > 1.01 / MIN_POINTS_PER_OCTAVE {
            issues.push(issue(
                name,
                Severity::Warning,
                "points",
                format!(
                    "points up to 1/{:.1} octave apart above {MIN_DENSE_HZ} Hz; the comparison interpolates between them (export 24 or more per octave)",
                    1.0 / widest
                ),
            ));
        }
        if q == Quantity::Impedance && curve.phase_deg.is_none() {
            issues.push(issue(
                name,
                Severity::Warning,
                "phase",
                "no phase: only |Z| is compared",
            ));
        }
        seatings.push(Seating {
            file: name.clone(),
            curve,
        });
    }
    (seatings, listed, issues)
}

/// Runs a session (module documentation). `files` holds every file of the
/// measurement directory by name; `progress` receives one line per fit.
pub fn validate(
    frozen: &Frozen,
    files: &Files,
    opts: &ValidateOptions,
    progress: &mut dyn FnMut(String),
) -> VResult<Report> {
    let protocol = &frozen.protocol;
    let ctx = Ctx { frozen, opts };
    let mut used = BTreeSet::new();
    let mut reports = Vec::new();
    let mut averaged: BTreeMap<String, (Averaged, Option<f64>)> = BTreeMap::new();
    for (c, m) in protocol.measurements() {
        let (seatings, listed, mut issues) = read_seatings(files, frozen, c, m, &mut used);
        let q = frozen.nominal[&m.id].quantity;
        let good: Vec<Seating> = seatings
            .into_iter()
            .filter(|s| {
                !issues
                    .iter()
                    .any(|i| i.file == s.file && i.severity == Severity::Error)
            })
            .collect();
        let rejected = listed.len() - good.len();
        if rejected > 0 {
            issues.push(issue(
                &m.id,
                Severity::Warning,
                "seatings",
                format!("{rejected} file(s) with errors are left out of the comparison"),
            ));
        }
        let status = match good.len() {
            0 => "missing",
            n if (n as u32) < c.seatings => "incomplete",
            _ => "complete",
        };
        let mut rep = MeasurementReport {
            id: m.id.clone(),
            configuration: c.id.clone(),
            label: c.label.clone(),
            quantity: q.name(),
            required_seatings: c.seatings,
            files: listed,
            seatings: good.len(),
            seating_sd_median_db: None,
            seating_sd_max_db: None,
            status,
            issues,
            blind: None,
            acceptance: Vec::new(),
            errors: Vec::new(),
        };
        if !good.is_empty() {
            let env = &frozen.envelopes[&m.id];
            match average(&good, env) {
                Ok(a) => {
                    rep.blind = Some(blind(&a, env, protocol));
                    if a.n >= 2 {
                        let mut sorted = a.sd_db.clone();
                        sorted.sort_by(f64::total_cmp);
                        rep.seating_sd_median_db =
                            Some(crate::analysis::mc::percentile(&sorted, 0.5));
                        rep.seating_sd_max_db = sorted.last().copied();
                    }
                    averaged.insert(m.id.clone(), (a, stated_source_impedance(&good)));
                }
                Err(e) => rep.errors.push(e.0),
            }
        }
        reports.push(rep);
    }
    // Driver anchor first: the anchored acceptance runs need it.
    let mut anchor: Option<Anchor> = None;
    if opts.anchor_driver {
        if let Some(da) = &protocol.driver_anchor {
            if let (Some((a, _)), Some((c, m))) = (
                averaged.get(&da.measurement),
                protocol.measurement(&da.measurement),
            ) {
                progress(format!("fitting the driver to '{}'", m.id));
                match ctx.anchor(c, m, a) {
                    Ok(x) => anchor = Some(x),
                    Err(e) => {
                        if let Some(r) = reports.iter_mut().find(|r| r.id == m.id) {
                            r.errors.push(format!("driver anchor fit: {e}"));
                        }
                    }
                }
            }
        }
    }
    let anchor_overrides: Option<Overrides> = anchor.as_ref().map(|a| {
        a.overrides
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| PValue::from_json(v).map(|p| (k.clone(), p)))
            .collect()
    });
    for rep in reports.iter_mut() {
        let (c, m) = protocol.measurement(&rep.id).expect("protocol measurement");
        if !(c.acceptance && rep.quantity == Quantity::Pressure.name()) {
            continue;
        }
        let Some((a, zs)) = averaged.get(&m.id) else {
            continue;
        };
        let mut variants: Vec<(&'static str, Overrides)> = vec![("spec", Overrides::new())];
        if let Some(ao) = &anchor_overrides {
            variants.push(("driver_anchored", ao.clone()));
        }
        for (variant, extra) in variants {
            progress(format!("acceptance fit of '{}' ({variant})", m.id));
            match ctx.acceptance(c, m, a, *zs, variant, &extra) {
                Ok(x) => rep.acceptance.push(x),
                Err(e) => rep.errors.push(format!("acceptance fit ({variant}): {e}")),
            }
        }
    }
    // Sidecar templates still waiting for their curve, and the checklist,
    // belong to the session.
    let template = |n: &str| {
        n == "SESSION.txt"
            || n.strip_suffix(".sidecar.json").is_some_and(|stem| {
                protocol.measurements().any(|(_, m)| {
                    stem.strip_prefix(m.id.as_str())
                        .and_then(|r| r.strip_prefix("_s"))
                        .is_some_and(|k| k.parse::<u32>().is_ok())
                })
            })
    };
    let unrecognised: Vec<String> = files
        .keys()
        .filter(|n| !used.contains(*n) && !template(n))
        .cloned()
        .collect();
    let verdict = verdict(protocol, &reports, anchor.is_some());
    Ok(Report {
        schema: REPORT_SCHEMA,
        engine: crate::solve::ENGINE,
        frozen: json!({
            "version": frozen.version,
            "engine": frozen.engine(),
            "manifest_sha256": frozen.manifest_sha256,
            "created": frozen.manifest.get("created"),
            "git": frozen.manifest.get("git"),
            "blind": frozen.blind(),
            "status": frozen.status(),
        }),
        protocol: protocol.title.clone(),
        acceptance_source: protocol.acceptance.source.clone(),
        measurements: reports,
        driver_anchor: anchor,
        unrecognised_files: unrecognised,
        verdict,
    })
}

fn verdict(protocol: &super::Protocol, reports: &[MeasurementReport], anchored: bool) -> Verdict {
    let missing: Vec<String> = reports
        .iter()
        .filter(|r| r.status == "missing")
        .map(|r| r.id.clone())
        .collect();
    let incomplete: Vec<String> = reports
        .iter()
        .filter(|r| r.status == "incomplete")
        .map(|r| r.id.clone())
        .collect();
    let error_files: BTreeSet<&str> = reports
        .iter()
        .flat_map(|r| r.issues.iter())
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.file.as_str())
        .collect();
    let synthetic = reports
        .iter()
        .flat_map(|r| r.issues.iter())
        .any(|i| i.field == "provenance" && i.severity == Severity::Note);
    let reference: Vec<&MeasurementReport> = reports
        .iter()
        .filter(|r| {
            protocol
                .measurement(&r.id)
                .is_some_and(|(c, _)| c.reference && c.acceptance)
                && r.quantity == Quantity::Pressure.name()
        })
        .collect();
    let spec_of = |r: &MeasurementReport| {
        r.acceptance
            .iter()
            .find(|a| a.variant == "spec")
            .and_then(|a| a.pass)
    };
    // A failed reference state decides the verdict; otherwise every one of
    // them must have been evaluated.
    let reference_pass = if reference.iter().any(|r| spec_of(r) == Some(false)) {
        Some(false)
    } else if !reference.is_empty() && reference.iter().all(|r| spec_of(r) == Some(true)) {
        Some(true)
    } else {
        None
    };
    let mut text = Vec::new();
    if synthetic {
        text.push("SYNTHETIC: some curves come from the virtual rig; this run exercises the pipeline and validates nothing.".to_string());
    }
    if missing.is_empty() && incomplete.is_empty() {
        text.push(format!(
            "Complete: all {} measurements have their seatings.",
            reports.len()
        ));
    } else {
        text.push(format!(
            "Incomplete session: {} missing ({}), {} with too few seatings ({}).",
            missing.len(),
            missing.join(", "),
            incomplete.len(),
            incomplete.join(", ")
        ));
    }
    if !error_files.is_empty() {
        text.push(format!(
            "{} file(s) do not meet the protocol and were left out (see the issues).",
            error_files.len()
        ));
    }
    let names = reference
        .iter()
        .map(|r| r.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    text.push(match reference_pass {
        Some(true) => format!("Reference headphone acceptance (spec Section 17): PASS on {names}."),
        Some(false) => {
            let failed: Vec<&str> = reference
                .iter()
                .filter(|r| spec_of(r) == Some(false))
                .map(|r| r.id.as_str())
                .collect();
            format!(
                "Reference headphone acceptance (spec Section 17): FAIL on {} (of {names}).",
                failed.join(", ")
            )
        }
        None => {
            let open: Vec<&str> = reference
                .iter()
                .filter(|r| spec_of(r).is_none())
                .map(|r| r.id.as_str())
                .collect();
            format!(
                "Reference headphone acceptance (spec Section 17): not evaluated; it needs {names}, and {} could not be evaluated (missing, or not covering an acceptance band).",
                open.join(", ")
            )
        }
    });
    if anchored {
        text.push("The driver-anchored runs use the driver's own free-air impedance; they are reported for diagnosis, not as the spec's criterion.".into());
    }
    Verdict {
        complete: missing.is_empty() && incomplete.is_empty(),
        missing,
        incomplete,
        sidecar_errors: error_files.len(),
        synthetic,
        reference_pass,
        text,
    }
}

fn fmt_band(b: &BandStats) -> String {
    let hi = if b.f_max >= 1000.0 {
        format!("{:.3} kHz", b.f_max / 1000.0)
    } else {
        format!("{:.0} Hz", b.f_max)
    };
    let lo = if b.f_min >= 1000.0 {
        format!("{:.3} kHz", b.f_min / 1000.0)
    } else {
        format!("{:.0} Hz", b.f_min)
    };
    format!("{lo}-{hi}")
}

impl Report {
    /// Human-readable report: completeness, sidecar issues, the blind
    /// comparison and the acceptance test, in separate sections.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "{}\n", self.protocol);
        let _ = writeln!(
            s,
            "Frozen predictions {} (engine {}, manifest SHA-256 {}); engine now {}.",
            self.frozen["version"].as_str().unwrap_or("?"),
            self.frozen["engine"].as_str().unwrap_or("?"),
            self.frozen["manifest_sha256"].as_str().unwrap_or("?"),
            self.engine
        );
        let _ = writeln!(s, "\nVERDICT");
        for t in &self.verdict.text {
            let _ = writeln!(s, "  {t}");
        }
        let _ = writeln!(s, "\n1. MEASUREMENTS AND SIDECARS");
        for m in &self.measurements {
            let _ = writeln!(
                s,
                "  {:<16} {:<10} {} of {} seatings{}  ({})",
                m.id,
                m.status,
                m.seatings,
                m.required_seatings,
                match (m.seating_sd_median_db, m.seating_sd_max_db) {
                    (Some(a), Some(b)) => format!(", spread {a:.2} dB median, {b:.2} dB max"),
                    _ => String::new(),
                },
                m.label
            );
            for i in m.issues.iter().filter(|i| i.severity != Severity::Note) {
                let _ = writeln!(
                    s,
                    "      {:?} {} [{}]: {}",
                    i.severity, i.file, i.field, i.message
                );
            }
            for e in &m.errors {
                let _ = writeln!(s, "      error: {e}");
            }
        }
        if !self.unrecognised_files.is_empty() {
            let _ = writeln!(
                s,
                "  Not part of the protocol (not read): {}",
                self.unrecognised_files.join(", ")
            );
        }
        let blind = self.frozen["blind"].as_bool() == Some(true);
        let _ = writeln!(
            s,
            "\n2. AGAINST THE FROZEN {} (no fitting; r = measured - predicted, z = r / combined standard uncertainty)",
            if blind {
                "BLIND PREDICTIONS"
            } else {
                "PREDICTIONS, NOT BLIND (a model revision; the earliest blind version is the blind record)"
            }
        );
        for m in &self.measurements {
            let Some(b) = &m.blind else { continue };
            let _ = writeln!(
                s,
                "  {} ({}; credible band max |r| {}{})",
                m.id,
                m.quantity,
                b.credible_max_abs_db
                    .map_or("-".to_string(), |x| format!("{x:.2} dB")),
                b.max_abs_phase_deg
                    .map_or(String::new(), |x| format!(", max |phase r| {x:.1} deg"))
            );
            for band in &b.bands {
                let _ = writeln!(
                    s,
                    "      {:<22} {:>4} pts  RMS {:>6.2} dB  max {:>6.2} dB at {:>7.0} Hz  |z|<=2: {:>4.0} %  in 5-95 %: {:>4.0} %",
                    fmt_band(band),
                    band.points,
                    band.rms_db,
                    band.max_abs_db,
                    band.at_hz,
                    band.within_2u.unwrap_or(f64::NAN) * 100.0,
                    band.within_envelope.unwrap_or(f64::NAN) * 100.0
                );
            }
        }
        let _ = writeln!(s, "\n3. ACCEPTANCE ({})", self.acceptance_source);
        if let Some(a) = &self.driver_anchor {
            let _ = writeln!(
                s,
                "  Driver anchor from '{}' (reduced chi-square {}): {}",
                a.measurement,
                a.reduced_chi2.map_or("n/a".into(), |x| format!("{x:.3}")),
                a.fitted
                    .iter()
                    .map(|f| format!("{} = {:.4}", f.name, f.value))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for m in &self.measurements {
            for a in &m.acceptance {
                let _ = writeln!(
                    s,
                    "  {} [{}]: {}  (fit: {}; reduced chi-square {}; {} evaluations{})",
                    m.id,
                    a.variant,
                    match a.pass {
                        Some(true) => "PASS",
                        Some(false) => "FAIL",
                        None => "NOT EVALUATED",
                    },
                    a.fitted
                        .iter()
                        .map(|f| format!("{} {:.4} (start {:.4})", f.name, f.value, f.start))
                        .collect::<Vec<_>>()
                        .join(", "),
                    a.reduced_chi2.map_or("n/a".into(), |x| format!("{x:.2}")),
                    a.evaluations,
                    if a.converged {
                        String::new()
                    } else {
                        format!(", not converged: {}", a.stop_reason)
                    }
                );
                for b in &a.bands {
                    let _ = writeln!(
                        s,
                        "      {:<22} max |r| {:>6.2} dB at {:>7.0} Hz  {}",
                        fmt_band(b),
                        b.max_abs_db,
                        b.at_hz,
                        match (b.bound_db, b.pass) {
                            (Some(x), Some(true)) => format!("within {x} dB"),
                            (Some(x), Some(false)) => format!("EXCEEDS {x} dB"),
                            (Some(_), None) => "not covered by the data".into(),
                            (None, _) => "no bound (reported only)".into(),
                        }
                    );
                }
            }
        }
        s
    }
}

/// A drive as the analyser is set: the convention and, for a power into
/// the rated impedance, the open-circuit voltage sqrt(P·R).
fn drive_text(d: &crate::io::sidecar::Drive) -> String {
    match d.spec {
        crate::drive::DriveSpec::Power { watts, rated_ohm } => format!(
            "{} ({:.2} mV RMS open-circuit)",
            d.label(),
            (watts * rated_ohm).sqrt() * 1e3
        ),
        _ => d.label(),
    }
}

/// The empty session: a sidecar template for every file to be measured,
/// and `SESSION.txt`, the ordered checklist (spec Section 17: "the tool
/// lists the measurements a session must produce").
pub fn session_templates(frozen: &Frozen) -> BTreeMap<String, String> {
    let protocol = &frozen.protocol;
    let mut out = BTreeMap::new();
    let mut list = format!(
        "{}\nFrozen predictions {} (manifest SHA-256 {}).\n\nSave each export as <file stem>.<frd|zma|txt|csv> next to its sidecar template, fill in the template's date, device, temperature_C, source_impedance_ohm and analyser, and state the uncertainty budget you know (docs/fitting.md). Then run: acoustilab validate <this directory>\n",
        protocol.title, frozen.version, frozen.manifest_sha256
    );
    // The protocol's own description names its written form, which
    // completes the short instructions below.
    if let Some(d) = &protocol.description {
        let _ = writeln!(list, "\n{d}");
    }
    for c in &protocol.configurations {
        let _ = writeln!(list, "\n[{}] {}  ({} seatings)", c.id, c.label, c.seatings);
        if let Some(i) = &c.instructions {
            let _ = writeln!(list, "    {i}");
        }
        for m in &c.measurements {
            let nominal = &frozen.nominal[&m.id];
            let exp = &nominal.sidecar;
            let stems: Vec<String> = (1..=c.seatings).map(|k| seating_stem(&m.id, k)).collect();
            let _ = writeln!(
                list,
                "    {}: {} at {}, drive {}: files {}",
                m.id,
                nominal.quantity.name(),
                m.reference_point,
                exp.drive.as_ref().map_or("?".into(), drive_text),
                stems.join(", ")
            );
            for stem in stems {
                let mut t = serde_json::Map::new();
                t.insert("schema".into(), json!(crate::io::sidecar::SIDECAR_SCHEMA));
                t.insert("quantity".into(), json!(nominal.quantity.name()));
                if nominal.quantity == Quantity::Pressure {
                    t.insert("calibrated".into(), json!(true));
                }
                for (k, v) in &c.sidecar {
                    t.insert(k.clone(), v.clone());
                }
                t.insert("reference_point".into(), json!(m.reference_point));
                if let Some(d) = &exp.drive {
                    t.insert("drive".into(), d.json.clone());
                }
                t.insert("source_impedance_ohm".into(), Value::Null);
                t.insert("compensation".into(), json!("none"));
                t.insert("smoothing".into(), json!("none"));
                t.insert("seatings".into(), json!(1));
                t.insert("averaging".into(), json!("none"));
                t.insert("temperature_C".into(), Value::Null);
                t.insert("date".into(), json!("YYYY-MM-DD"));
                t.insert("device".into(), json!("FILL IN: cup serial, left/right"));
                t.insert(
                    "provenance".into(),
                    json!({"origin": "measured", "tool": "FILL IN: analyser and version"}),
                );
                t.insert(
                    "notes".into(),
                    json!(format!(
                        "{} ({}); overrides {}",
                        c.label,
                        c.id,
                        overrides_json(&protocol.overrides_for(c))
                    )),
                );
                out.insert(
                    format!("{stem}.sidecar.json"),
                    format!("{:#}\n", Value::Object(t)),
                );
            }
        }
    }
    out.insert("SESSION.txt".into(), list);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seating_names() {
        assert_eq!(seating_of("iec_ref_p_s3.frd", "iec_ref_p"), Some(3));
        assert_eq!(seating_of("iec_ref_p_s12.TXT", "iec_ref_p"), Some(12));
        assert_eq!(seating_of("iec_ref_p_s0.frd", "iec_ref_p"), None);
        assert_eq!(
            seating_of("iec_ref_p_s1.frd.sidecar.json", "iec_ref_p"),
            None
        );
        assert_eq!(seating_of("iec_ref_p_s1.wav", "iec_ref_p"), None);
        assert_eq!(seating_of("iec_ref_z_s1.zma", "iec_ref_p"), None);
        assert_eq!(seating_of("iec_ref_p_hi_s1.frd", "iec_ref_p"), None);
    }

    #[test]
    fn band_stats_excludes_shared_edges_of_open_bands() {
        let f = [10.0, 20.0, 1000.0, 4000.0, 8000.0];
        let r = [5.0, 1.0, 1.5, -3.0, 9.0];
        let below = band_stats(&f, &r, None, None, (10.0, 20.0, None), false, true);
        assert_eq!(below.points, 1);
        let b1 = band_stats(&f, &r, None, None, (20.0, 1000.0, Some(2.0)), false, false);
        assert_eq!(
            (b1.points, b1.max_abs_db, b1.at_hz, b1.pass),
            (2, 1.5, 1000.0, Some(true))
        );
        let b2 = band_stats(
            &f,
            &r,
            None,
            None,
            (1000.0, 4000.0, Some(2.0)),
            false,
            false,
        );
        assert_eq!((b2.max_abs_db, b2.pass), (3.0, Some(false)));
        let above = band_stats(&f, &r, None, None, (4000.0, 8000.0, None), true, false);
        assert_eq!((above.points, above.max_abs_db, above.pass), (1, 9.0, None));
    }
}
