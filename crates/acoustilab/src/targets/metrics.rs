//! The error metric set (spec Section 11, "never one number"), the ITU-R
//! BS.708 mask, left-right tracking, and [`evaluate`], which produces the
//! full report of a response against a target.
//!
//! Pipeline (docs/targets.md):
//!
//! 1. Each ear's curve is optionally smoothed (1/N-octave power smoothing)
//!    and sampled on the evaluation grid (1/12 octave by default).
//! 2. Left and right are averaged in dB where both exist.
//! 3. Response and target (with any personalisation shelves) are
//!    normalised: minus their level at 500 Hz (read between grid points,
//!    linear on log frequency), or minus their mean over the 200–500 Hz
//!    grid points.
//! 4. `e = response − target` where both exist and the target is valid.
//!
//! Every statistic states its band and the part of it the data covered.
//! Main metrics are over 20 Hz–10 kHz; the same set over 10–20 kHz is
//! reported separately; band RMS is over 20–200 Hz, 200 Hz–2 kHz, 2–8 kHz
//! and 8–20 kHz.

use super::curve::Curve;
use super::fixture::{self, Match};
use super::grid;
use super::scores::{self, Context, Score};
use super::smoothing::smooth_opt;
use super::target::{classes_json, preference_band, BandParams, Shelves, Target};
use super::{embedded_json, Flag, Result, TargetError};
use serde_json::{json, Value};
use std::sync::OnceLock;

// ------------------------------------------------------------ statistics

/// Statistics of a curve of values (typically the error) over one band.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub band_hz: (f64, f64),
    /// Points used: grid points in the band where the value exists.
    pub n: usize,
    /// First and last frequency used.
    pub used_hz: Option<(f64, f64)>,
    /// True when the points used stop short of the band edges by more than
    /// half a 1/12-octave step.
    pub partial: bool,
    pub mean: f64,
    /// Mean absolute value.
    pub mae: f64,
    pub rms: f64,
    /// Sample standard deviation (ddof = 1); `None` below two points.
    pub sd: Option<f64>,
    /// Least-squares slope against ln f, dB per neper of frequency.
    pub slope: Option<f64>,
    pub max_abs: f64,
    pub max_abs_hz: f64,
}

/// Statistics of `values` over the grid points of band `[lo, hi]`, or
/// `None` if no point in the band has a value.
pub fn stats(grid_hz: &[f64], values: &[Option<f64>], lo: f64, hi: f64) -> Option<Stats> {
    let pts: Vec<(f64, f64)> = grid::band(grid_hz, lo, hi)
        .filter_map(|k| values[k].map(|v| (grid_hz[k], v)))
        .collect();
    let n = pts.len();
    if n == 0 {
        return None;
    }
    let nf = n as f64;
    let mean = pts.iter().map(|p| p.1).sum::<f64>() / nf;
    let mae = pts.iter().map(|p| p.1.abs()).sum::<f64>() / nf;
    let rms = (pts.iter().map(|p| p.1 * p.1).sum::<f64>() / nf).sqrt();
    let (max_abs_hz, max_abs) = pts
        .iter()
        .map(|&(f, v)| (f, v.abs()))
        .fold((pts[0].0, -1.0), |a, b| if b.1 > a.1 { b } else { a });
    let (sd, slope) = if n >= 2 {
        let ss = pts.iter().map(|p| (p.1 - mean).powi(2)).sum::<f64>();
        let ubar = pts.iter().map(|p| p.0.ln()).sum::<f64>() / nf;
        let suu = pts.iter().map(|p| (p.0.ln() - ubar).powi(2)).sum::<f64>();
        let sue = pts
            .iter()
            .map(|p| (p.0.ln() - ubar) * (p.1 - mean))
            .sum::<f64>();
        (Some((ss / (nf - 1.0)).sqrt()), Some(sue / suu))
    } else {
        (None, None)
    };
    let (first, last) = (pts[0].0, pts[n - 1].0);
    Some(Stats {
        band_hz: (lo, hi),
        n,
        used_hz: Some((first, last)),
        partial: !grid::covers(first, last, lo, hi),
        mean,
        mae,
        rms,
        sd,
        slope,
        max_abs,
        max_abs_hz,
    })
}

fn stats_json(lo: f64, hi: f64, s: &Option<Stats>) -> Value {
    match s {
        None => json!({"band_Hz": [lo, hi], "n": 0, "partial": true}),
        Some(s) => json!({
            "band_Hz": [lo, hi],
            "used_Hz": s.used_hz.map(|(a, b)| [a, b]),
            "n": s.n,
            "partial": s.partial,
            "rms_dB": s.rms,
            "sd_dB": s.sd,
            "slope_dB_per_ln_f": s.slope,
            "abs_slope": s.slope.map(f64::abs),
            "slope_dB_per_octave": s.slope.map(|b| b * std::f64::consts::LN_2),
            "mean_dB": s.mean,
            "mae_dB": s.mae,
            "max_abs_dB": s.max_abs,
            "max_abs_at_Hz": s.max_abs_hz,
        }),
    }
}

// --------------------------------------------------------- normalisation

/// Level alignment before every comparison (spec Section 11, "Compensation
/// pipeline").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Normalisation {
    /// Subtract the level at this frequency (default 500 Hz).
    At(f64),
    /// Subtract the mean level over the grid points of this band (e.g.
    /// 200–500 Hz).
    BandMean(f64, f64),
}

impl Normalisation {
    /// The level to subtract from `values` on `grid_hz`, or `None` if it is
    /// undefined there.
    pub fn offset(&self, grid_hz: &[f64], values: &[Option<f64>]) -> Option<f64> {
        match *self {
            Normalisation::At(f) => {
                let k = grid_hz.partition_point(|&g| g < f);
                if k < grid_hz.len() && (grid_hz[k] / f - 1.0).abs() < 1e-12 {
                    return values[k];
                }
                if k == 0 || k == grid_hz.len() {
                    return None;
                }
                let (a, b) = (values[k - 1]?, values[k]?);
                let t = (f / grid_hz[k - 1]).ln() / (grid_hz[k] / grid_hz[k - 1]).ln();
                Some(a + t * (b - a))
            }
            Normalisation::BandMean(lo, hi) => {
                let v: Vec<f64> = grid::band(grid_hz, lo, hi)
                    .filter_map(|k| values[k])
                    .collect();
                (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
            }
        }
    }

    pub fn to_json(&self) -> Value {
        match *self {
            Normalisation::At(f) => json!({"at_Hz": f}),
            Normalisation::BandMean(a, b) => json!({"band_mean_Hz": [a, b]}),
        }
    }
}

fn normalised(v: &[Option<f64>], off: f64) -> Vec<Option<f64>> {
    v.iter().map(|x| x.map(|x| x - off)).collect()
}

// ---------------------------------------------------------------- BS.708

fn bs708_doc() -> &'static Value {
    static D: OnceLock<Value> = OnceLock::new();
    D.get_or_init(|| {
        embedded_json(
            "bs708.json",
            include_str!("../../../../data/targets/bs708.json"),
        )
    })
}

fn bs708_breakpoints() -> Vec<(f64, f64)> {
    bs708_doc()["mask"]["breakpoints"]
        .as_array()
        .expect("bs708.json breakpoints")
        .iter()
        .map(|b| {
            (
                b["f_Hz"].as_f64().expect("f_Hz"),
                b["tolerance_dB"].as_f64().expect("tolerance_dB"),
            )
        })
        .collect()
}

/// The ITU-R BS.708 tolerance (± dB) at `f`: linear in dB on log frequency
/// through (100 Hz, 2), (500 Hz, 1.5), (4 kHz, 1.5), (16 kHz, 4); `None`
/// outside 100 Hz–16 kHz (`data/targets/bs708.json`, erratum E47).
pub fn bs708_limit(f: f64) -> Option<f64> {
    let bp = bs708_breakpoints();
    let (f0, fl) = (bp[0].0, bp[bp.len() - 1].0);
    if f < f0 * (1.0 - 1e-9) || f > fl * (1.0 + 1e-9) {
        return None;
    }
    let f = f.clamp(f0, fl);
    let j = bp.partition_point(|b| b.0 < f).max(1);
    let ((a, ta), (b, tb)) = (bp[j - 1], bp[j]);
    Some(ta + (tb - ta) * (f / a).ln() / (b / a).ln())
}

/// Compliance of values with a band `[lower_k, upper_k]` at each grid point.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskResult {
    pub band_hz: (f64, f64),
    pub n: usize,
    pub within: usize,
    pub partial: bool,
    /// Largest excursion outside the mask: (frequency, dB beyond the limit).
    pub worst: Option<(f64, f64)>,
}

impl MaskResult {
    pub fn percent(&self) -> Option<f64> {
        (self.n > 0).then(|| 100.0 * self.within as f64 / self.n as f64)
    }

    fn to_json(&self) -> Value {
        json!({
            "band_Hz": [self.band_hz.0, self.band_hz.1],
            "n": self.n,
            "within": self.within,
            "compliance_percent": self.percent(),
            "partial": self.partial,
            "worst": self.worst.map(|(f, d)| json!({"f_Hz": f, "excess_dB": d})),
        })
    }
}

/// Compliance of `e` with `limits(k) = (lower, upper)` over band `[lo, hi]`.
fn mask(
    grid_hz: &[f64],
    e: &[Option<f64>],
    lo: f64,
    hi: f64,
    limits: impl Fn(usize) -> Option<(f64, f64)>,
) -> MaskResult {
    let mut r = MaskResult {
        band_hz: (lo, hi),
        n: 0,
        within: 0,
        partial: true,
        worst: None,
    };
    let (mut first, mut last) = (f64::NAN, f64::NAN);
    for k in grid::band(grid_hz, lo, hi) {
        let (Some(v), Some((l, u))) = (e[k], limits(k)) else {
            continue;
        };
        if r.n == 0 {
            first = grid_hz[k];
        }
        last = grid_hz[k];
        r.n += 1;
        let excess = (l - v).max(v - u);
        if excess <= 1e-12 {
            r.within += 1;
        } else if r.worst.is_none_or(|w| excess > w.1) {
            r.worst = Some((grid_hz[k], excess));
        }
    }
    r.partial = r.n == 0 || !grid::covers(first, last, lo, hi);
    r
}

/// BS.708 mask compliance of the error `e` over 100 Hz–16 kHz.
pub fn bs708_compliance(grid_hz: &[f64], e: &[Option<f64>]) -> MaskResult {
    let bp = bs708_breakpoints();
    mask(grid_hz, e, bp[0].0, bp[bp.len() - 1].0, |k| {
        bs708_limit(grid_hz[k]).map(|t| (-t, t))
    })
}

// -------------------------------------------------------------- tracking

/// One BS.708 tracking band.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingBand {
    pub band_hz: (f64, f64),
    pub limit_db: f64,
    pub result: MaskResult,
    /// Largest |L − R| and where.
    pub max_abs: Option<(f64, f64)>,
}

impl TrackingBand {
    pub fn pass(&self) -> Option<bool> {
        (self.result.n > 0).then_some(self.result.within == self.result.n)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tracking {
    pub smoothing: Option<u32>,
    /// L − R on the grid, dB (not normalised: tracking includes the level
    /// difference).
    pub difference: Vec<Option<f64>>,
    pub bands: Vec<TrackingBand>,
    /// Largest |L − R| between the limit bands (8–10 kHz), where BS.708
    /// states no limit: (frequency, dB).
    pub unspecified_max: Option<(f64, f64)>,
}

/// Left-right tracking against the BS.708 limits (1 dB over 100 Hz–8 kHz,
/// 2 dB over 10–16 kHz, `data/targets/bs708.json`). Both curves are
/// smoothed with `smoothing` (third-octave by default in [`Options`]), then
/// sampled on the grid.
pub fn tracking(
    left: &Curve,
    right: &Curve,
    grid_hz: &[f64],
    smoothing: Option<u32>,
) -> Result<Tracking> {
    let l = smooth_opt(left, smoothing)?.sample(grid_hz);
    let r = smooth_opt(right, smoothing)?.sample(grid_hz);
    let d: Vec<Option<f64>> = l.iter().zip(&r).map(|(a, b)| Some((*a)? - (*b)?)).collect();
    let limits = bs708_doc()["tracking"]["limits"]
        .as_array()
        .expect("bs708.json tracking limits");
    let mut bands = Vec::new();
    let mut ends = Vec::new();
    for lim in limits {
        let (lo, hi) = (
            lim["band_Hz"][0].as_f64().expect("band"),
            lim["band_Hz"][1].as_f64().expect("band"),
        );
        let t = lim["max_difference_dB"].as_f64().expect("limit");
        let result = mask(grid_hz, &d, lo, hi, |_| Some((-t, t)));
        let max_abs = grid::band(grid_hz, lo, hi)
            .filter_map(|k| d[k].map(|v| (grid_hz[k], v.abs())))
            .fold(None, |m: Option<(f64, f64)>, p| match m {
                Some(m) if m.1 >= p.1 => Some(m),
                _ => Some(p),
            });
        ends.push(grid::band(grid_hz, lo, hi));
        bands.push(TrackingBand {
            band_hz: (lo, hi),
            limit_db: t,
            result,
            max_abs,
        });
    }
    let unspecified_max = if ends.len() == 2 && ends[0].end < ends[1].start {
        (ends[0].end..ends[1].start)
            .filter_map(|k| d[k].map(|v| (grid_hz[k], v.abs())))
            .fold(None, |m: Option<(f64, f64)>, p| match m {
                Some(m) if m.1 >= p.1 => Some(m),
                _ => Some(p),
            })
    } else {
        None
    };
    Ok(Tracking {
        smoothing,
        difference: d,
        bands,
        unspecified_max,
    })
}

impl Tracking {
    pub fn to_json(&self, grid_hz: &[f64]) -> Value {
        json!({
            "smoothing": smoothing_json(self.smoothing),
            "grid_Hz": grid_hz,
            "difference_dB": self.difference,
            "bands": self.bands.iter().map(|b| json!({
                "band_Hz": [b.band_hz.0, b.band_hz.1],
                "limit_dB": b.limit_db,
                "pass": b.pass(),
                "max_abs_dB": b.max_abs.map(|m| m.1),
                "max_abs_at_Hz": b.max_abs.map(|m| m.0),
                "compliance_percent": b.result.percent(),
                "n": b.result.n,
                "partial": b.result.partial,
            })).collect::<Vec<_>>(),
            "unspecified_8_to_10kHz": self.unspecified_max.map(|(f, v)| json!({"max_abs_dB": v, "at_Hz": f})),
            "source": "ITU-R BS.708 recommends 3 (data/targets/bs708.json)",
        })
    }
}

fn smoothing_json(n: Option<u32>) -> Value {
    match n {
        None => json!("none"),
        Some(n) => json!(format!("1/{n} octave")),
    }
}

// ---------------------------------------------------------------- report

/// The curve(s) under test.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub left: Curve,
    /// The other ear, if any: the metrics use the left-right average and
    /// tracking compares the two.
    pub right: Option<Curve>,
    /// Fixture tag of the response (inferred from the netlist or stated).
    pub fixture: Option<String>,
    pub label: String,
    /// How the response was driven (a result's `meta.drive.label`).
    pub drive: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub grid_hz: Vec<f64>,
    pub normalisation: Normalisation,
    /// Smoothing of the response before sampling (`None`: none).
    pub smoothing: Option<u32>,
    /// Smoothing for left-right tracking (default third-octave, as BS.708
    /// measures in third-octave bands).
    pub tracking_smoothing: Option<u32>,
    /// Personalisation shelves added to the target for the error metrics.
    pub shelves: Option<Shelves>,
    pub band: BandParams,
    /// True for measured curves; everything the engine produces is
    /// simulated.
    pub measured: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            grid_hz: grid::twelfth_octave(),
            normalisation: Normalisation::At(500.0),
            smoothing: None,
            tracking_smoothing: Some(3),
            shelves: None,
            band: BandParams::default(),
            measured: false,
        }
    }
}

/// Bands of the metric set, Hz (spec Sections 10 and 11).
pub const MAIN_BAND: (f64, f64) = (20.0, 10000.0);
pub const ABOVE_10K_BAND: (f64, f64) = (10000.0, 20000.0);
pub const RMS_BANDS: [(f64, f64); 4] = [
    (20.0, 200.0),
    (200.0, 2000.0),
    (2000.0, 8000.0),
    (8000.0, 20000.0),
];
/// The whole audio band, for preference-band compliance.
pub const FULL_BAND: (f64, f64) = (20.0, 20000.0);

/// Everything computed for one response against one target.
#[derive(Debug, Clone)]
pub struct Report {
    pub grid_hz: Vec<f64>,
    pub options: Options,
    pub target: Target,
    pub response_label: String,
    pub response_fixture: Option<String>,
    pub drive: Option<String>,
    /// Normalised left-right average, target (with shelves) and error.
    pub response: Vec<Option<f64>>,
    pub target_db: Vec<Option<f64>>,
    pub error: Vec<Option<f64>>,
    /// Level subtracted from the response (its reference level) and target.
    pub response_offset_db: f64,
    pub target_offset_db: f64,
    pub main: Option<Stats>,
    pub above_10k: Option<Stats>,
    pub bands: Vec<Option<Stats>>,
    pub bs708: MaskResult,
    /// Preference band around the reference target (absolute, normalised).
    pub band_lower: Vec<Option<f64>>,
    pub band_upper: Vec<Option<f64>>,
    pub band_compliance: MaskResult,
    pub tracking: Option<Tracking>,
    pub scores: Vec<Score>,
    pub fixture_match: Option<Match>,
    pub flags: Vec<Flag>,
}

/// Samples `curve` after optional smoothing.
fn on_grid(curve: &Curve, n: Option<u32>, grid_hz: &[f64]) -> Result<Vec<Option<f64>>> {
    Ok(smooth_opt(curve, n)?.sample(grid_hz))
}

/// Evaluates a response against a target: error metrics, BS.708 mask,
/// preference band, left-right tracking (when both ears are given) and every
/// preference model, with the flags the interface must show.
pub fn evaluate(resp: &Response, target: &Target, opts: &Options) -> Result<Report> {
    let g = &opts.grid_hz;
    grid::check(g).map_err(TargetError::Options)?;
    let l = on_grid(&resp.left, opts.smoothing, g)?;
    let avg: Vec<Option<f64>> = match &resp.right {
        None => l,
        Some(r) => {
            let r = on_grid(r, opts.smoothing, g)?;
            l.iter()
                .zip(&r)
                .map(|(a, b)| Some(0.5 * ((*a)? + (*b)?)))
                .collect()
        }
    };
    let norm = opts.normalisation;
    let undefined = |what: &str| {
        TargetError::Options(format!(
            "the {what} is undefined at the normalisation ({}); it must cover it",
            norm.to_json()
        ))
    };
    let r_off = norm.offset(g, &avg).ok_or_else(|| undefined("response"))?;
    let response = normalised(&avg, r_off);
    let t_raw = target.on_grid(g, opts.shelves.as_ref());
    let t_off = norm.offset(g, &t_raw).ok_or_else(|| undefined("target"))?;
    let target_db = normalised(&t_raw, t_off);
    let diff = |a: &[Option<f64>], b: &[Option<f64>]| -> Vec<Option<f64>> {
        a.iter().zip(b).map(|(x, y)| Some((*x)? - (*y)?)).collect()
    };
    let error = diff(&response, &target_db);

    let main = stats(g, &error, MAIN_BAND.0, MAIN_BAND.1);
    let above_10k = stats(g, &error, ABOVE_10K_BAND.0, ABOVE_10K_BAND.1);
    let bands = RMS_BANDS
        .iter()
        .map(|&(a, b)| stats(g, &error, a, b))
        .collect();
    let bs708 = bs708_compliance(g, &error);

    // Preference band around the reference target (no personalisation).
    let t_ref_raw = target.on_grid(g, None);
    let t_ref = normalised(
        &t_ref_raw,
        norm.offset(g, &t_ref_raw)
            .ok_or_else(|| undefined("target"))?,
    );
    let band = preference_band(g, &opts.band, |v| norm.offset(g, v));
    let band_lower: Vec<Option<f64>> = t_ref
        .iter()
        .zip(&band.lower)
        .map(|(t, l)| t.map(|t| t + l))
        .collect();
    let band_upper: Vec<Option<f64>> = t_ref
        .iter()
        .zip(&band.upper)
        .map(|(t, u)| t.map(|t| t + u))
        .collect();
    let e_ref = diff(&response, &t_ref);
    let band_compliance = mask(g, &e_ref, FULL_BAND.0, FULL_BAND.1, |k| {
        Some((band.lower[k], band.upper[k]))
    });

    let tracking = match &resp.right {
        Some(r) => Some(tracking(&resp.left, r, g, opts.tracking_smoothing)?),
        None => None,
    };

    // Scores: the models are defined at 500 Hz against the reference target.
    let n500 = Normalisation::At(500.0);
    let e500 = match (n500.offset(g, &avg), n500.offset(g, &t_ref_raw)) {
        (Some(a), Some(b)) => diff(&normalised(&avg, a), &normalised(&t_ref_raw, b)),
        _ => vec![None; g.len()],
    };
    let cx = Context {
        fixture: resp.fixture.as_deref(),
        target,
        measured: opts.measured,
    };
    let scores: Vec<Score> = scores::models()
        .iter()
        .map(|m| scores::evaluate(m, g, &e500, &cx))
        .collect();

    // Flags for the interface.
    let mut flags = Vec::new();
    let fixture_match = resp
        .fixture
        .as_deref()
        .map(|f| fixture::compare(f, &target.fixture));
    match (resp.fixture.as_deref(), fixture_match) {
        (None, _) => flags.push(Flag::new(
            "fixture_unspecified",
            format!(
                "the response's fixture is not known; the target belongs to '{}'",
                target.fixture
            ),
        )),
        (Some(f), Some(Match::SameEarSimulator)) => flags.push(Flag::new(
            "fixture_mismatch",
            format!(
                "the response is on '{f}' and the target on '{}': same ear simulator, but pinna, head or canal extension differ, which moves the response by several dB above 2 kHz",
                target.fixture
            ),
        )),
        (Some(f), Some(Match::Different)) => flags.push(Flag::new(
            "fixture_mismatch",
            format!(
                "the response is on '{f}' and the target on '{}': different fixtures",
                target.fixture
            ),
        )),
        _ => {}
    }
    for tf in &target.flags {
        let msg = match tf.as_str() {
            "approximation" => "the target is a parametric approximation, not published data",
            "adaptation" => "the target is an adaptation of a third-party curve by its depositors",
            "third_octave" => "the target has third-octave resolution",
            "data_anomaly" => "the target's data contain a probable error (see its notes)",
            "user_supplied" => "the target was supplied by the user",
            _ => continue,
        };
        flags.push(Flag::new("target_note", format!("{tf}: {msg}")));
    }
    if !opts.measured {
        flags.push(Flag::new("simulated", "simulated, not measured"));
    }
    if resp
        .fixture
        .as_deref()
        .and_then(fixture::fixture)
        .is_some_and(|f| f.is_iec60318_4())
    {
        flags.push(Flag::new(
            "coupler_extrapolated",
            "20-100 Hz is coupler-extrapolated: IEC 60318-4 is not validated below 100 Hz",
        ));
    }
    if main.as_ref().is_none_or(|s| s.partial) {
        flags.push(Flag::new(
            "partial_range",
            "the response and target do not cover all of 20 Hz-10 kHz; each metric states the range it used",
        ));
    }
    if opts.shelves.is_some_and(|s| !s.is_flat()) {
        flags.push(Flag::new(
            "personalised",
            "the error metrics use the personalised target; the preference band and the scores use the reference target",
        ));
    }

    Ok(Report {
        grid_hz: g.clone(),
        options: opts.clone(),
        target: target.clone(),
        response_label: resp.label.clone(),
        response_fixture: resp.fixture.clone(),
        drive: resp.drive.clone(),
        response,
        target_db,
        error,
        response_offset_db: r_off,
        target_offset_db: t_off,
        main,
        above_10k,
        bands,
        bs708,
        band_lower,
        band_upper,
        band_compliance,
        tracking,
        scores,
        fixture_match,
        flags,
    })
}

impl Report {
    pub fn score(&self, model: &str) -> Option<&Score> {
        self.scores.iter().find(|s| s.model.id == model)
    }

    pub fn to_json(&self) -> Value {
        let t = &self.target;
        let o = &self.options;
        json!({
            "target": {
                "name": t.name,
                "label": t.label,
                "fixture": t.fixture,
                "family": t.family,
                "flags": t.flags,
                "licence": t.provenance.licence,
                "provenance_class": t.provenance.class.as_str(),
                "valid_range_Hz": [t.valid_range_hz.0, t.valid_range_hz.1],
            },
            "response": {
                "label": self.response_label,
                "fixture": self.response_fixture,
                "fixture_label": self.response_fixture.as_deref().and_then(fixture::fixture).map(|f| f.label.clone()),
                "drive": self.drive,
                "reference_level_dB": self.response_offset_db,
                "measured": o.measured,
            },
            "fixture_match": self.fixture_match,
            "options": {
                "normalisation": o.normalisation.to_json(),
                "smoothing": smoothing_json(o.smoothing),
                "tracking_smoothing": smoothing_json(o.tracking_smoothing),
                "personalisation": o.shelves.map(|s| s.to_json()),
                "grid": if o.grid_hz == grid::twelfth_octave() { json!("1/12 octave, 10^(k/40) Hz, 19.95 Hz-19.95 kHz") } else { json!("custom") },
            },
            "grid_Hz": self.grid_hz,
            "response_dB": self.response,
            "target_dB": self.target_db,
            "error_dB": self.error,
            "target_offset_dB": self.target_offset_db,
            "metrics": {
                "main": stats_json(MAIN_BAND.0, MAIN_BAND.1, &self.main),
                "above_10kHz": stats_json(ABOVE_10K_BAND.0, ABOVE_10K_BAND.1, &self.above_10k),
                "band_rms": RMS_BANDS.iter().zip(&self.bands).map(|(&(a, b), s)| stats_json(a, b, s)).collect::<Vec<_>>(),
                "bs708_mask": self.bs708.to_json(),
                "preference_band": self.band_compliance.to_json(),
            },
            "preference_band": {
                "lower_dB": self.band_lower,
                "upper_dB": self.band_upper,
                "about": classes_json(&o.band),
            },
            "tracking": self.tracking.as_ref().map(|tr| tr.to_json(&self.grid_hz)),
            "scores": self.scores.iter().map(Score::to_json).collect::<Vec<_>>(),
            "coupler_extrapolated_Hz": self.response_fixture.as_deref().and_then(fixture::fixture)
                .filter(|f| f.is_iec60318_4()).map(|_| [20.0, 100.0]),
            "flags": self.flags,
        })
    }
}
