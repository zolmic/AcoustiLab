//! Auralization filters (spec Section 16). See `docs/auralization.md`.
//!
//! The audition filter is the candidate's response divided by a baseline's
//! at the same reference point. By default the listener hears the
//! programme through that filter (A) against the programme itself (B), so
//! what changes between A and B is the difference between the designs: the
//! listener's own headphones multiply both alike. The baseline is another
//! design (a netlist, divided exactly), a target curve or an imported
//! measurement (magnitude only, inverted with regularisation, see
//! [`inversion`]). The uncompensated absolute response is a flagged
//! diagnostic ([`Mode::Absolute`]).
//!
//! **Pipeline** ([`audition`]):
//!
//! 1. Both designs are solved exactly on a check grid, 48 points per octave
//!    over the audition band (20 Hz–20 kHz by default) plus the band edges.
//!    The filter's analytic magnitude D(f) is the ratio there, held at its
//!    band-edge values outside the band (the band limit; for a target or a
//!    measurement the band is the inversion band, 20 Hz–10 kHz), and
//!    normalised to 0 dB mean over 500 Hz–2 kHz (the mid-band anchor).
//! 2. **Length** (erratum E46): the ratio on the check grid is vector
//!    fitted (`time::poles::fit_values`), and each resonant pole of the
//!    filter (Q ≥ 1, in band, weight ≥ −20 dB) needs N/(2·fs) ≥ 2.93·Q/f.
//!    With `n` unset, N is the larger of the default (8192 at 48 kHz,
//!    scaled with the rate) and the smallest power of two that holds the
//!    binding pole, capped at twice the default.
//! 3. Both designs are re-solved on the linear FFT grid
//!    ([`crate::time::uniform`]: every bin in band an exact solve, never an
//!    interpolation of the log grid) and their minimum-phase analysis
//!    counterparts computed ([`crate::time::minphase`]). The bin
//!    magnitudes M_k are the ratio's, held and normalised as in step 1.
//! 4. **Phase** ([`PhaseRequest`]): the Section 16 decision is taken on the
//!    filter itself, whose excess phase is the candidate's minus the
//!    baseline's (a magnitude-only baseline counts as minimum phase).
//!    `minimum`: the cepstral minimum phase of M ([`min_phase_fir_spectrum`]),
//!    with the filter's polarity. `mixed`: below the validity frequency f_v
//!    (the lowest `begin_hz` of the designs' shading, capped at the band
//!    top) the model's own complex ratio, advanced by the pure-delay
//!    estimate (delay alignment); above f_v minimum phase, crossfaded in
//!    phase over 1/3 octave (the hybrid). `linear`: M delayed by N/2
//!    samples (a diagnostic: its pre-ringing would be blamed on the
//!    design). `auto` takes `minimum` when the decision passes, else
//!    `mixed`.
//! 5. The half spectrum (Hermitian, real DC and Nyquist bins) is inverse
//!    transformed; the mixed IR is rotated so that `pre` samples (N/32)
//!    hold negative times, and a Tukey window tapers its first `pre` and
//!    its last N/16 samples (minimum phase: the last N/16 only; linear
//!    phase: N/16 at both ends). The taps are f32.
//! 6. **Check**: the taps' own frequency response (their DTFT, evaluated
//!    directly from the f32 values) is compared with D(f) on the check
//!    grid. Section 16 asks for 0.1 dB from 20 Hz to 20 kHz; the report
//!    says where it is met and, where not, lists the frequencies and the
//!    frequency resolution fs/N.
//!
//! Every audition state serialises ([`Audition::state`]): the SHA-256 of
//! each netlist text with its overrides and probe, or the target's name or
//! the curve's hash, the mode, phase mode, rate and length. The page adds
//! the level match and the programme.

pub mod inversion;
pub mod report;
pub mod sha256;

use crate::circuit::Circuit;
use crate::drive::DriveInfo;
use crate::error::{Error, Result};
use crate::solve::Scale;
use crate::targets::fixture;
use crate::time::fft::irfft;
use crate::time::minphase::{
    excess_phase, min_phase, min_phase_fir_spectrum, phase_decision, DcAsymptote, ExcessPhase,
    MinPhase, MinPhaseOptions, PhaseDecision, EXCESS_GD_THRESHOLD_S, REFINE_DEFAULT,
};
use crate::time::poles::{fit_values, PoleFitOptions, E46_FACTOR};
pub use crate::time::uniform::GUARD_FRACTION;
use crate::time::uniform::{uniform_response, UniformOptions, UniformResponse};
use crate::validity::Shading;
use crate::C64;
use inversion::{Inverse, InversionOptions};
use serde::Serialize;
use serde_json::Value;
use std::f64::consts::PI;

/// Default rate and FFT length at that rate (Section 16).
pub const FS_DEFAULT: f64 = 48_000.0;
pub const N_DEFAULT_48K: usize = 8192;
/// Default audition band, Hz.
pub const BAND_DEFAULT_HZ: (f64, f64) = (20.0, 20_000.0);
/// Density of the check grid, points per octave.
pub const CHECK_POINTS_PER_OCTAVE: f64 = 48.0;
/// Mid-band anchor: the filter is normalised to 0 dB mean over this band.
pub const ANCHOR_HZ: (f64, f64) = (500.0, 2000.0);
/// Section 16: "the convolved chain must reproduce the analytic response
/// within 0.1 dB from 20 Hz to 20 kHz".
pub const TOLERANCE_DB: f64 = 0.1;
/// Width of the hybrid phase crossfade above the validity frequency.
pub const HYBRID_TRANSITION_OCTAVES: f64 = 1.0 / 3.0;
/// The window tapers the last N/TAIL_DIVISOR samples.
pub const TAIL_DIVISOR: usize = 16;

/// What the filter compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Candidate divided by baseline (the default).
    Difference,
    /// The candidate's own response, uncompensated: a flagged diagnostic.
    Absolute,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Difference => "difference",
            Mode::Absolute => "absolute",
        }
    }
}

/// Phase mode requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseRequest {
    /// Minimum phase when the Section 16 decision passes, else mixed.
    Auto,
    Minimum,
    Mixed,
    /// Diagnostic only.
    Linear,
}

impl PhaseRequest {
    pub fn as_str(self) -> &'static str {
        match self {
            PhaseRequest::Auto => "auto",
            PhaseRequest::Minimum => "minimum",
            PhaseRequest::Mixed => "mixed",
            PhaseRequest::Linear => "linear",
        }
    }

    pub fn parse(s: &str) -> Option<PhaseRequest> {
        Some(match s {
            "auto" => PhaseRequest::Auto,
            "minimum" => PhaseRequest::Minimum,
            "mixed" => PhaseRequest::Mixed,
            "linear" => PhaseRequest::Linear,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub mode: Mode,
    pub phase: PhaseRequest,
    pub fs_hz: f64,
    /// FFT length (= number of taps); `None` sizes it by E46.
    pub n: Option<usize>,
    /// Audition band, Hz: the ratio is exact inside, held outside.
    pub band_hz: (f64, f64),
    /// Excess group delay threshold of the phase decision, s.
    pub threshold_s: f64,
    pub inversion: InversionOptions,
    /// Samples of negative time in a mixed-phase filter (default N/32).
    pub pre: Option<usize>,
    /// Run the E46 fit (needed for `n: None`).
    pub length_check: bool,
    /// Check the taps against the analytic ratio.
    pub verify: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mode: Mode::Difference,
            phase: PhaseRequest::Auto,
            fs_hz: FS_DEFAULT,
            n: None,
            band_hz: BAND_DEFAULT_HZ,
            threshold_s: EXCESS_GD_THRESHOLD_S,
            inversion: InversionOptions::default(),
            pre: None,
            length_check: true,
            verify: true,
        }
    }
}

/// The default FFT length at `fs`: 8192 at 48 kHz (and 44.1 kHz), scaled
/// by the power of two nearest to fs/48 kHz, so the filter keeps its
/// duration (16384 at 96 kHz).
pub fn n_default(fs_hz: f64) -> usize {
    let k = (fs_hz / FS_DEFAULT).log2().round().clamp(-3.0, 4.0);
    ((N_DEFAULT_48K as f64) * 2f64.powf(k)) as usize
}

/// The allowed FFT lengths at `fs`: half to twice the default (4096 to
/// 16384 at 48 kHz, the spec's range).
pub fn n_range(fs_hz: f64) -> (usize, usize) {
    let d = n_default(fs_hz);
    (d / 2, d * 2)
}

impl Options {
    pub fn check(&self) -> std::result::Result<(), String> {
        if !(self.fs_hz.is_finite() && (8000.0..=384_000.0).contains(&self.fs_hz)) {
            return Err(format!(
                "fs = {} Hz must be between 8 kHz and 384 kHz",
                self.fs_hz
            ));
        }
        let (lo, hi) = self.band_hz;
        if !(lo.is_finite() && hi.is_finite() && lo > 0.0 && hi > 2.0 * lo) {
            return Err(format!(
                "band {lo} to {hi} Hz must be positive and span more than an octave"
            ));
        }
        if hi > GUARD_FRACTION * self.fs_hz / 2.0 {
            return Err(format!(
                "band top {hi} Hz must stay below {} Hz (0.9 of Nyquist at {} Hz)",
                GUARD_FRACTION * self.fs_hz / 2.0,
                self.fs_hz
            ));
        }
        if let Some(n) = self.n {
            let (a, b) = n_range(self.fs_hz);
            if !(n.is_power_of_two() && (a..=b).contains(&n)) {
                return Err(format!(
                    "n = {n} must be a power of two from {a} to {b} at {} Hz",
                    self.fs_hz
                ));
            }
        }
        if !(self.threshold_s.is_finite() && self.threshold_s > 0.0) {
            return Err("the excess-delay threshold must be positive".into());
        }
        self.inversion.check()
    }
}

/// One design (a netlist with overrides and a probe) prepared for
/// audition. It keeps its compiled circuit and caches its solves, so a
/// page that re-designs the filter while one design changes re-solves
/// only that one.
pub struct Design {
    circuit: Circuit,
    scale: Scale,
    pub probe: String,
    pub quantity: String,
    pub unit: String,
    pub drive: DriveInfo,
    pub shading: Shading,
    /// Fixture the probe reads (`targets::fixture::for_probe`).
    pub fixture: Option<String>,
    pub netlist_sha256: String,
    /// The overrides as given (a JSON object).
    pub overrides: Value,
    /// Resolved parameter values.
    pub parameters: Value,
    check: Option<(Vec<f64>, Vec<C64>)>,
    uniforms: Vec<Uniform>,
}

/// A design re-solved on the FFT grid with its minimum-phase analysis.
pub struct Uniform {
    pub response: UniformResponse,
    pub analysis: MinPhase,
    pub excess: ExcessPhase,
}

impl Design {
    /// Compiles `text` under `overrides` and reads `probe` (default: the
    /// netlist's `ui.primary_probe`, else its first pressure probe).
    pub fn new(
        text: &str,
        overrides: &crate::params::Overrides,
        overrides_json: Value,
        probe: Option<&str>,
    ) -> Result<Design> {
        let p = crate::params::Parametric::parse(text)?;
        let circuit = Circuit::from_parametric(&p, overrides)?;
        let probe = match probe {
            Some(id) => id.to_string(),
            None => crate::time::report::default_probe(&p, &circuit)?,
        };
        let pr = circuit
            .probes
            .iter()
            .find(|q| q.id == probe)
            .ok_or_else(|| Error::Probe {
                id: probe.clone(),
                msg: "no such probe".into(),
            })?;
        let (quantity, unit) = (pr.quantity.clone(), pr.unit.to_string());
        let (scale, drive) = circuit.drive_scale()?;
        let parameters = Value::Object(
            circuit
                .parameters
                .iter()
                .map(|(n, v)| (n.clone(), v.to_json()))
                .collect(),
        );
        Ok(Design {
            shading: circuit.shading(),
            fixture: fixture::for_probe(&circuit, &probe).map(|f| f.id.clone()),
            netlist_sha256: sha256::sha256_hex(text.as_bytes()),
            overrides: overrides_json,
            parameters,
            probe,
            quantity,
            unit,
            drive,
            scale,
            circuit,
            check: None,
            uniforms: Vec::new(),
        })
    }

    /// The probe at `f`, at the stated drive (as `Circuit::solve` gives it).
    pub fn value_at(&self, f: f64) -> Result<C64> {
        let mut x = self.circuit.solve_at(f)?;
        let s = self.scale.factor(&self.circuit, f, &x)?;
        if s != C64::new(1.0, 0.0) {
            x.iter_mut().for_each(|v| *v *= s);
        }
        let pr = self
            .circuit
            .probes
            .iter()
            .find(|q| q.id == self.probe)
            .expect("probe checked in Design::new");
        self.circuit.probe_value(pr, f, &x)
    }

    /// The probe on `grid` (cached for the last grid).
    pub fn on_grid(&mut self, grid: &[f64]) -> Result<Vec<C64>> {
        if let Some((g, v)) = &self.check {
            if g == grid {
                return Ok(v.clone());
            }
        }
        let v = grid
            .iter()
            .map(|&f| self.value_at(f))
            .collect::<Result<Vec<_>>>()?;
        self.check = Some((grid.to_vec(), v.clone()));
        Ok(v)
    }

    /// The uniform re-solve at (fs, n), solved up to 0.9 of Nyquist so
    /// that two designs with different sweeps share their solved band.
    pub fn uniform(&mut self, fs_hz: f64, n: usize) -> Result<&Uniform> {
        if let Some(i) = self
            .uniforms
            .iter()
            .position(|u| u.response.fs_hz == fs_hz && u.response.n == n)
        {
            return Ok(&self.uniforms[i]);
        }
        let opts = UniformOptions {
            fs_hz,
            n,
            f_min_hz: None,
            f_max_hz: Some(GUARD_FRACTION * fs_hz / 2.0),
        };
        let response = uniform_response(&self.circuit, &self.probe, &opts)?;
        let analysis = min_phase(&response, &MinPhaseOptions::default());
        let excess = excess_phase(
            &response.values,
            &analysis.values,
            response.df(),
            response.solved.0,
        );
        // Keep at most two lengths (a page switching rates).
        if self.uniforms.len() >= 2 {
            self.uniforms.remove(0);
        }
        self.uniforms.push(Uniform {
            response,
            analysis,
            excess,
        });
        Ok(self.uniforms.last().expect("just pushed"))
    }

    /// Label of the reference point: the probe and its fixture.
    pub fn reference_point(&self) -> String {
        match &self.fixture {
            Some(f) => format!("{} ({f})", self.probe),
            None => self.probe.clone(),
        }
    }
}

/// A magnitude-only baseline: a target curve or an imported measurement.
#[derive(Debug, Clone)]
pub struct MagnitudeBaseline {
    /// `target` or `curve`.
    pub kind: &'static str,
    pub name: String,
    pub label: String,
    /// Fixture the curve was taken on, when known.
    pub fixture: Option<String>,
    /// SHA-256 of the curve's serialised form.
    pub sha256: String,
    /// Levels in dB (their reference does not matter: the inversion
    /// normalises).
    pub curve: crate::targets::curve::Curve,
    /// Notes to show with the result (the target's flags, the curve's
    /// origin).
    pub notes: Vec<String>,
}

/// What the candidate is divided by.
pub enum Baseline<'a> {
    Design(&'a mut Design),
    Magnitude(&'a MagnitudeBaseline),
    /// No baseline (absolute mode only).
    None,
}

/// A flag with a stable code and a sentence for the interface.
#[derive(Debug, Clone, Serialize)]
pub struct Flag {
    pub code: &'static str,
    pub message: String,
}

fn flag(code: &'static str, message: impl Into<String>) -> Flag {
    Flag {
        code,
        message: message.into(),
    }
}

/// The E46 length check of the filter.
#[derive(Debug, Clone, Serialize)]
pub struct LengthCheck {
    /// What was fitted: `ratio` or `candidate`.
    pub fitted: &'static str,
    /// Resonant poles of the fit: (f, Q, needed s), lowest first.
    pub poles: Vec<PoleNeed>,
    /// The pole needing the longest buffer, if any.
    pub binding: Option<PoleNeed>,
    #[serde(rename = "needed_s")]
    pub needed_s: f64,
    /// Smallest power-of-two N at this rate with N/(2·fs) ≥ needed_s.
    pub recommended_n: usize,
    /// N used and whether its causal half N/(2·fs) holds needed_s.
    pub n: usize,
    #[serde(rename = "half_length_s")]
    pub half_length_s: f64,
    pub covered: bool,
    /// Right-half-plane zeros of the fit inside the decision band, Hz.
    #[serde(rename = "rhp_zeros_in_decision_band_Hz")]
    pub rhp_zeros_hz: Vec<f64>,
    /// The fit's largest dB error, and its notes.
    #[serde(rename = "fit_max_dB")]
    pub fit_max_db: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoleNeed {
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    pub q: f64,
    #[serde(rename = "needed_s")]
    pub needed_s: f64,
}

/// The magnitude check of the taps against the analytic filter.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    #[serde(rename = "band_Hz")]
    pub band_hz: (f64, f64),
    #[serde(rename = "tolerance_dB")]
    pub tolerance_db: f64,
    #[serde(rename = "max_abs_error_dB")]
    pub max_abs_error_db: f64,
    #[serde(rename = "at_Hz")]
    pub at_hz: f64,
    #[serde(rename = "rms_error_dB")]
    pub rms_error_db: f64,
    /// |error| ≤ tolerance over the whole band.
    pub met: bool,
    /// Frequency ranges of the check grid where |error| > tolerance, Hz.
    #[serde(rename = "exceeded_Hz")]
    pub exceeded_hz: Vec<(f64, f64)>,
    /// Bin spacing fs/N, Hz.
    #[serde(rename = "resolution_Hz")]
    pub resolution_hz: f64,
}

/// The phase mode used and why.
#[derive(Debug, Clone, Serialize)]
pub struct PhaseReport {
    pub requested: &'static str,
    pub used: &'static str,
    pub reason: String,
    /// The Section 16 decision on the filter's excess group delay.
    pub decision: PhaseDecision,
    /// +1, or −1 when the filter inverts polarity.
    pub polarity: f64,
    /// Pure delay removed from a mixed-phase filter, s.
    #[serde(rename = "delay_removed_s")]
    pub delay_removed_s: f64,
    /// Above this frequency a mixed filter is minimum phase (the hybrid),
    /// Hz; `None` for other modes.
    #[serde(rename = "hybrid_from_Hz")]
    pub hybrid_from_hz: Option<f64>,
}

/// An audition filter and its report.
#[derive(Debug, Clone)]
pub struct Audition {
    pub mode: Mode,
    pub fs_hz: f64,
    pub n: usize,
    pub taps: Vec<f32>,
    /// Delay of the filter's main response, samples: 0 for minimum phase,
    /// `pre` for mixed, N/2 for linear. A reference path (B) delays the
    /// programme by as much so that A/B switching does not shift in time.
    pub latency_samples: usize,
    /// Band where the ratio is exact (held outside), Hz.
    pub band_hz: (f64, f64),
    /// Gain removed to put the 500 Hz–2 kHz mean at 0 dB, dB.
    pub anchor_gain_db: f64,
    pub phase: PhaseReport,
    pub length: Option<LengthCheck>,
    pub length_error: Option<String>,
    pub check: Option<Check>,
    /// Energy of the last N/16 samples before windowing re the total, dB.
    pub tail_energy_db: f64,
    /// Energy of the first `pre` samples of a mixed filter re the total, dB.
    pub pre_energy_db: Option<f64>,
    /// Check grid and the filter on it: analytic (design), realised (taps)
    /// and their difference, dB.
    pub grid_hz: Vec<f64>,
    pub design_db: Vec<f64>,
    pub fir_db: Vec<f64>,
    pub error_db: Vec<f64>,
    /// Largest boost and cut of the filter within the band, dB (re the
    /// anchor).
    pub max_boost_db: (f64, f64),
    pub max_cut_db: (f64, f64),
    pub inverse: Option<Inverse>,
    pub flags: Vec<Flag>,
    pub notes: Vec<String>,
    pub state: Value,
    /// The candidate's and baseline's summaries for the report.
    pub candidate: Value,
    pub baseline: Value,
}

/// Largest phase step towards the multiple of 2π above the excess phase
/// that the hybrid accepts (a negative group delay); beyond it the phase
/// goes down to the multiple below.
pub const HYBRID_MAX_ADVANCE_RAD: f64 = PI / 4.0;

/// The phase of a mixed filter (Section 16, mode 2, with the hybrid of
/// mode 1): the excess phase d(f) (the model's, delay aligned) up to the
/// validity frequency f_v; a multiple of 2π (i.e. minimum phase) from
/// f_v·2^(1/3) (or from Nyquist, when that comes first); a raised-cosine
/// crossfade in log f between. The target multiple is the one below
/// d(f_v), so that the phase keeps falling and the group delay stays
/// non-negative through the crossfade, unless the one above is within
/// [`HYBRID_MAX_ADVANCE_RAD`]. For an all-pass lattice with RC = 0.5 ms,
/// whose excess phase is −178° at 20 kHz, returning it to 0 puts −5.7 dB
/// of the filter's energy before t = 0, returning it to −360° −12.2 dB
/// (in both cases the band-limited pulse's own neighbours within 0.13 ms
/// of t = 0, at 20–24 kHz; `tests/audition.rs`). Below the band the
/// excess phase is kept: it tends to 0 (or π, the polarity) at DC, and
/// crossfading it there over a few bins made a narrowband phase feature
/// whose decay outlasted N.
#[derive(Debug, Clone, Copy)]
struct HybridPhase {
    f_v: f64,
    width: f64,
    target: f64,
}

impl HybridPhase {
    fn new(f_v: f64, nyquist: f64, d_v: f64) -> HybridPhase {
        let two_pi = 2.0 * PI;
        let up = two_pi * (d_v / two_pi).ceil();
        let target = if up - d_v <= HYBRID_MAX_ADVANCE_RAD {
            up
        } else {
            two_pi * (d_v / two_pi).floor()
        };
        HybridPhase {
            f_v,
            width: HYBRID_TRANSITION_OCTAVES.min((nyquist / f_v).log2().max(1e-3)),
            target,
        }
    }

    fn phase(&self, f: f64, d: f64) -> f64 {
        if f <= self.f_v {
            return d;
        }
        let x = (f / self.f_v).log2() / self.width;
        let w = if x >= 1.0 {
            0.0
        } else {
            0.5 * (1.0 + (PI * x).cos())
        };
        w * d + (1.0 - w) * self.target
    }
}

/// Largest change the band-edge hold adds beyond the band edge, dB.
pub const EDGE_EXCURSION_DB: f64 = 6.0;

/// The filter outside its band: the band-edge level, reached with a
/// continuous slope. With s the filter's slope at the edge (dB/octave) and
/// x the distance beyond the edge in octaves, the level is
/// L_edge ± s·(x − x²/(2w)) for x ≤ w and L_edge ± s·w/2 beyond, with
/// w = min(1, 2·[`EDGE_EXCURSION_DB`]/|s|) octaves. Level and slope are
/// continuous at the edge (a hard hold would leave a kink there that a
/// finite filter reproduces only to about 0.15 dB at the edge on a steep
/// slope), and the hold adds at most 6 dB of boost or cut beyond it.
#[derive(Debug, Clone, Copy)]
struct EdgeHold {
    band: (f64, f64),
    /// (level dB, slope dB/octave) at each edge.
    lo: (f64, f64),
    hi: (f64, f64),
}

impl EdgeHold {
    fn outside(&self, f: f64) -> Option<f64> {
        let shape = |(level, s): (f64, f64), x: f64| {
            let w = if s == 0.0 {
                1.0
            } else {
                (2.0 * EDGE_EXCURSION_DB / s.abs()).min(1.0)
            };
            let x = x.min(w);
            level + s * (x - x * x / (2.0 * w))
        };
        if f < self.band.0 * (1.0 - 1e-12) {
            let x = if f > 0.0 {
                (self.band.0 / f).log2()
            } else {
                f64::INFINITY
            };
            let (l, s) = self.lo;
            Some(shape((l, -s), x))
        } else if f > self.band.1 * (1.0 + 1e-12) {
            Some(shape(self.hi, (f / self.band.1).log2()))
        } else {
            None
        }
    }
}

/// The check grid: 48 points per octave from the band's bottom to its
/// top, with the band edges, the inversion band edges and the anchor band
/// edges inserted.
pub fn check_grid(band: (f64, f64), extra: &[f64]) -> Vec<f64> {
    let mut g = inversion::log_grid(band.0, band.1, CHECK_POINTS_PER_OCTAVE);
    for &f in extra.iter().chain([ANCHOR_HZ.0, ANCHOR_HZ.1].iter()) {
        if f > band.0 && f < band.1 && !g.iter().any(|&x| (x / f - 1.0).abs() < 1e-9) {
            g.push(f);
        }
    }
    g.sort_by(|a, b| a.total_cmp(b));
    g
}

/// Mean of `db` over the grid points in the anchor band, weighted by the
/// log-frequency interval each point stands for.
fn anchor_mean(grid: &[f64], db: &[f64]) -> f64 {
    let idx: Vec<usize> = (0..grid.len())
        .filter(|&i| grid[i] >= ANCHOR_HZ.0 * (1.0 - 1e-9) && grid[i] <= ANCHOR_HZ.1 * (1.0 + 1e-9))
        .collect();
    let (mut sw, mut s) = (0.0, 0.0);
    for (j, &i) in idx.iter().enumerate() {
        let lo = if j == 0 {
            grid[i]
        } else {
            (grid[idx[j - 1]] * grid[i]).sqrt()
        };
        let hi = if j + 1 == idx.len() {
            grid[i]
        } else {
            (grid[i] * grid[idx[j + 1]]).sqrt()
        };
        let w = (hi / lo).ln();
        sw += w;
        s += w * db[i];
    }
    if sw > 0.0 {
        s / sw
    } else {
        db[idx[0]]
    }
}

/// Unwraps a phase sequence from index `k0` both ways.
fn unwrap_from(raw: &[f64], k0: usize) -> Vec<f64> {
    let mut ph = raw.to_vec();
    let wrap = |d: f64| d - 2.0 * PI * (d / (2.0 * PI)).round();
    for k in k0 + 1..raw.len() {
        ph[k] = ph[k - 1] + wrap(raw[k] - raw[k - 1]);
    }
    for k in (0..k0).rev() {
        ph[k] = ph[k + 1] + wrap(raw[k] - raw[k + 1]);
    }
    ph
}

/// The filter's DTFT at frequency `f` from its taps (Horner's scheme in
/// e^{−jω/fs}, double precision).
pub fn dtft(taps: &[f32], fs_hz: f64, f: f64) -> C64 {
    let w = C64::from_polar(1.0, -2.0 * PI * f / fs_hz);
    taps.iter()
        .rev()
        .fold(C64::new(0.0, 0.0), |acc, &h| acc * w + h as f64)
}

/// Frequency ranges of `grid` where `bad` holds, merged.
fn ranges(grid: &[f64], bad: &[bool]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut i = 0;
    while i < grid.len() {
        if !bad[i] {
            i += 1;
            continue;
        }
        let a = i;
        while i < grid.len() && bad[i] {
            i += 1;
        }
        out.push((grid[a], grid[i - 1]));
    }
    out
}

fn db(v: f64) -> f64 {
    20.0 * v.log10()
}

/// Designs the audition filter of `cand` against `base`.
pub fn audition(cand: &mut Design, base: Baseline, opts: &Options) -> Result<Audition> {
    opts.check()
        .map_err(|e| Error::Netlist(format!("audition options: {e}")))?;
    let mut flags = Vec::new();
    let mut notes = Vec::new();
    let mut base = base;
    if opts.mode == Mode::Absolute {
        base = Baseline::None;
        flags.push(flag(
            "absolute_diagnostic",
            "Absolute mode: the candidate's own response, uncompensated. The listener's own headphones multiply everything heard, so this is not what the design sounds like; it is a diagnostic only (spec Section 16).",
        ));
    } else if matches!(base, Baseline::None) {
        return Err(Error::Netlist(
            "audition: difference mode needs a baseline".into(),
        ));
    }
    if !cand.quantity.eq("pressure") {
        flags.push(flag(
            "not_pressure",
            format!(
                "The candidate's probe '{}' reads {} ({}), not a sound pressure; the filter is its ratio all the same.",
                cand.probe, cand.quantity, cand.unit
            ),
        ));
    }

    // The baseline's inverse, when it is a curve.
    let inverse = match &base {
        Baseline::Magnitude(m) => Some(
            inversion::invert(&m.curve, &opts.inversion)
                .map_err(|e| Error::Netlist(format!("audition baseline '{}': {e}", m.name)))?,
        ),
        _ => None,
    };
    // The band where the ratio is exact.
    let band = match &inverse {
        Some(inv) => (
            inv.band_hz.0.max(opts.band_hz.0),
            inv.band_hz.1.min(opts.band_hz.1),
        ),
        None => opts.band_hz,
    };
    if let Some(inv) = &inverse {
        if inv.band_hz != opts.inversion.band_hz {
            notes.push(format!(
                "The baseline covers {:.0} Hz to {:.0} Hz, so it is inverted over {:.0} Hz to {:.0} Hz only.",
                inv.curve_range_hz.0,
                inv.curve_range_hz.1,
                inv.band_hz.0,
                inv.band_hz.1
            ));
        }
        notes.push(format!(
            "Band limit: the baseline is inverted from {:.0} Hz to {:.0} Hz; outside that band the filter holds its band-edge level, so A and B differ only inside it (spec Section 16).",
            band.0, band.1
        ));
    }

    // 1. The analytic filter on the check grid.
    let mut edges = vec![band.0, band.1];
    edges.extend([opts.inversion.band_hz.0, opts.inversion.band_hz.1]);
    let grid = check_grid(opts.band_hz, &edges);
    let hc = cand.on_grid(&grid)?;
    let (ratio_grid, fitted): (Vec<C64>, &'static str) = match &mut base {
        Baseline::Design(b) => {
            if b.quantity != cand.quantity || b.unit != cand.unit {
                return Err(Error::Netlist(format!(
                    "audition: the candidate's probe '{}' reads {} ({}) but the baseline's '{}' reads {} ({})",
                    cand.probe, cand.quantity, cand.unit, b.probe, b.quantity, b.unit
                )));
            }
            let hb = b.on_grid(&grid)?;
            (hc.iter().zip(&hb).map(|(a, b)| a / b).collect(), "ratio")
        }
        _ => (hc.clone(), "candidate"),
    };
    let inv_at = |f: f64| inverse.as_ref().map_or(0.0, |i| i.at(f));
    let in_band = |f: f64| f >= band.0 * (1.0 - 1e-12) && f <= band.1 * (1.0 + 1e-12);
    let raw_db: Vec<f64> = grid
        .iter()
        .zip(&ratio_grid)
        .map(|(&f, r)| db(r.norm()) + inv_at(f))
        .collect();
    let i_lo = grid
        .iter()
        .position(|&f| (f / band.0 - 1.0).abs() < 1e-9)
        .expect("band edge on the grid");
    let i_hi = grid
        .iter()
        .position(|&f| (f / band.1 - 1.0).abs() < 1e-9)
        .expect("band edge on the grid");
    let slope_lo = (raw_db[i_lo + 1] - raw_db[i_lo]) / (grid[i_lo + 1] / grid[i_lo]).log2();
    let slope_hi = (raw_db[i_hi] - raw_db[i_hi - 1]) / (grid[i_hi] / grid[i_hi - 1]).log2();
    let edges = EdgeHold {
        band,
        lo: (raw_db[i_lo], slope_lo),
        hi: (raw_db[i_hi], slope_hi),
    };
    let unnormalised: Vec<f64> = grid
        .iter()
        .enumerate()
        .map(|(i, &f)| edges.outside(f).unwrap_or(raw_db[i]))
        .collect();
    if unnormalised.iter().any(|v| !v.is_finite()) {
        return Err(Error::Netlist(
            "audition: the filter is not finite on the check grid (a response vanishes there)"
                .into(),
        ));
    }
    let anchor = anchor_mean(&grid, &unnormalised);
    let design_db: Vec<f64> = unnormalised.iter().map(|v| v - anchor).collect();

    // 2. Length (E46) from a vector fit of the filter in band.
    let (fs, (n_min, n_max)) = (opts.fs_hz, n_range(opts.fs_hz));
    let (length_fit, length_error) = if opts.length_check {
        let (f, v): (Vec<f64>, Vec<C64>) = grid
            .iter()
            .zip(&ratio_grid)
            .filter(|(f, _)| in_band(**f))
            .map(|(f, v)| (*f, *v))
            .unzip();
        match fit_values(&f, &v, &PoleFitOptions::default()) {
            Ok((_, report, poles, zeros, fit_notes)) => {
                let mut needs: Vec<PoleNeed> = poles
                    .iter()
                    .filter(|p| p.resonant)
                    .map(|p| {
                        let q = p.q.unwrap_or(0.5);
                        PoleNeed {
                            f_hz: p.f_hz,
                            q,
                            needed_s: E46_FACTOR * q / p.f_hz,
                        }
                    })
                    .collect();
                needs.sort_by(|a, b| a.f_hz.total_cmp(&b.f_hz));
                let binding = needs
                    .iter()
                    .max_by(|a, b| a.needed_s.total_cmp(&b.needed_s))
                    .cloned();
                let needed = binding.as_ref().map_or(0.0, |b| b.needed_s);
                let mut rec = n_min;
                while (rec as f64) / (2.0 * fs) < needed && rec < (1 << 24) {
                    rec *= 2;
                }
                let rhp: Vec<(f64, bool)> = zeros.iter().map(|z| (z.f_hz, z.rhp)).collect();
                (
                    Some((
                        needs,
                        binding,
                        needed,
                        rec,
                        rhp,
                        report.error.max_db,
                        fit_notes,
                    )),
                    None,
                )
            }
            Err(e) => (None, Some(e)),
        }
    } else {
        (None, None)
    };
    let n = match (opts.n, &length_fit) {
        (Some(n), _) => n,
        (None, Some(l)) => l.3.max(n_default(fs)).min(n_max),
        (None, None) => n_default(fs),
    };
    let half = n / 2;
    let df = fs / n as f64;

    // 3. Both designs on the FFT grid.
    let cu = cand.uniform(fs, n)?;
    let hcu: Vec<C64> = cu.response.values.clone();
    let k0 = cu.response.solved.0;
    let mut trusted = cu.response.trusted();
    let mut validity = cu.response.shading.begin_hz;
    let (r_bins, r_min, pol) = match &mut base {
        Baseline::Design(b) => {
            let bu = b.uniform(fs, n)?;
            for (t, u) in trusted.iter_mut().zip(bu.response.trusted()) {
                *t = *t && u;
            }
            validity = match (validity, bu.response.shading.begin_hz) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let r: Vec<C64> = hcu
                .iter()
                .zip(&bu.response.values)
                .map(|(a, b)| a / b)
                .collect();
            let rm: Vec<C64> = cu
                .analysis
                .values
                .iter()
                .zip(&bu.analysis.values)
                .map(|(a, b)| a / b)
                .collect();
            (r, rm, cu.analysis.polarity * bu.analysis.polarity)
        }
        _ => (
            hcu.clone(),
            cu.analysis.values.clone(),
            cu.analysis.polarity,
        ),
    };
    // Magnitudes on the bins: exact in band, held outside, normalised.
    let m_db: Vec<f64> = (0..=half)
        .map(|k| {
            let f = k as f64 * df;
            match edges.outside(f) {
                Some(v) => v - anchor,
                None => db(r_bins[k].norm()) + inv_at(f) - anchor,
            }
        })
        .collect();
    if m_db.iter().any(|v| !v.is_finite()) {
        return Err(Error::Netlist(
            "audition: the filter is not finite on the FFT grid (a response vanishes there)".into(),
        ));
    }
    let mags: Vec<f64> = m_db.iter().map(|v| 10f64.powf(v / 20.0)).collect();

    // 4. Phase: the decision on the filter's excess phase.
    let excess = match &base {
        Baseline::Design(_) => excess_phase(&r_bins, &r_min, df, k0),
        _ => cu.excess.clone(),
    };
    let decision = phase_decision(&r_bins, &excess, &trusted, df, pol, opts.threshold_s);
    let (used, reason) = match opts.phase {
        PhaseRequest::Auto => {
            let band_txt = decision.band_hz.map_or("an empty trusted band".to_string(), |(a, b)| {
                format!("its trusted band ({a:.0} Hz to {b:.0} Hz)")
            });
            if decision.min_phase_default {
                (
                    PhaseRequest::Minimum,
                    format!(
                        "Minimum phase: the filter's largest excess group delay in {band_txt} is {:.3} ms, below the {:.1} ms threshold (spec Section 16).",
                        decision.max_excess_gd_s * 1e3,
                        opts.threshold_s * 1e3
                    ),
                )
            } else {
                (
                    PhaseRequest::Mixed,
                    format!(
                        "Mixed phase: the filter's largest excess group delay in {band_txt} is {:.3} ms{}, not below the {:.1} ms threshold (spec Section 16); its pure delay of {:.3} ms is removed.",
                        decision.max_excess_gd_s * 1e3,
                        decision.at_hz.map_or(String::new(), |f| format!(" at {f:.0} Hz")),
                        opts.threshold_s * 1e3,
                        decision.pure_delay_s * 1e3
                    ),
                )
            }
        }
        p => (
            p,
            format!(
                "{} phase, as requested (the decision would give {} phase: largest excess group delay {:.3} ms against {:.1} ms).",
                match p {
                    PhaseRequest::Minimum => "Minimum",
                    PhaseRequest::Mixed => "Mixed",
                    _ => "Linear",
                },
                decision.mode,
                decision.max_excess_gd_s * 1e3,
                opts.threshold_s * 1e3
            ),
        ),
    };
    if used == PhaseRequest::Linear {
        flags.push(flag(
            "linear_phase_diagnostic",
            format!(
                "Linear phase is a diagnostic: its pre-ringing ({:.1} ms before the main response) is not the design's and would be blamed on it (spec Section 16).",
                half as f64 / fs * 1e3
            ),
        ));
    }

    // The minimum-phase filter of M (with the filter's polarity).
    let zero_phase: Vec<C64> = mags.iter().map(|&m| C64::new(m, 0.0)).collect();
    let dc = DcAsymptote {
        order: 0,
        corner_hz: df,
        probe: None,
    };
    let s_min = min_phase_fir_spectrum(&zero_phase, (1, half), df, Some(dc), REFINE_DEFAULT, pol);
    let pre = match used {
        PhaseRequest::Mixed => opts.pre.unwrap_or(n / 32).min(n / 4),
        _ => 0,
    };
    let (spectrum, delay_removed, hybrid_from) = match used {
        PhaseRequest::Minimum | PhaseRequest::Auto => (s_min.clone(), 0.0, None),
        PhaseRequest::Linear => {
            let mut s: Vec<C64> = mags
                .iter()
                .enumerate()
                .map(|(k, &m)| C64::from_polar(m * pol, -PI * k as f64))
                .collect();
            s[half] = C64::new(s[half].re, 0.0);
            (s, 0.0, None)
        }
        PhaseRequest::Mixed => {
            let tau = decision.pure_delay_s;
            let f_v = validity.unwrap_or(band.1).min(band.1);
            // The target phase: the model's complex filter (a design
            // baseline) or the candidate's excess over the minimum phase of
            // M (a magnitude-only baseline, taken as minimum phase).
            let d: Vec<f64> = match &base {
                Baseline::Design(_) => {
                    let raw: Vec<f64> = (0..=half)
                        .map(|k| {
                            let aligned =
                                r_bins[k] * C64::from_polar(1.0, 2.0 * PI * k as f64 * df * tau);
                            if aligned.norm() > 0.0 && s_min[k].norm() > 0.0 {
                                (aligned / s_min[k]).arg()
                            } else {
                                0.0
                            }
                        })
                        .collect();
                    unwrap_from(&raw, k0)
                }
                _ => (0..=half)
                    .map(|k| excess.phase_rad[k] + 2.0 * PI * k as f64 * df * tau)
                    .collect(),
            };
            // The excess phase d is kept below f_v and crossfaded to a
            // multiple of 2π (minimum phase) above it ([`HybridPhase`]).
            let nyquist = half as f64 * df;
            let f_v = f_v.max(band.0);
            let phase_at = |f: f64| d[((f / df).round() as usize).clamp(1, half)];
            let blend = HybridPhase::new(f_v, nyquist, phase_at(f_v));
            let mut s: Vec<C64> = (0..=half)
                .map(|k| s_min[k] * C64::from_polar(1.0, blend.phase(k as f64 * df, d[k])))
                .collect();
            s[0] = C64::new(s[0].re, 0.0);
            s[half] = C64::new(s[half].re, 0.0);
            (s, tau, Some(f_v))
        }
    };

    // 5. Impulse response, rotation, window, taps.
    let ir = irfft(&spectrum);
    let shift = match used {
        PhaseRequest::Mixed => pre,
        _ => 0,
    };
    let mut h: Vec<f64> = (0..n).map(|m| ir[(m + n - shift) % n]).collect();
    let total: f64 = h.iter().map(|v| v * v).sum();
    let tail = n / TAIL_DIVISOR;
    let energy_db = |range: std::ops::Range<usize>| -> f64 {
        let e: f64 = h[range].iter().map(|v| v * v).sum();
        if e > 0.0 && total > 0.0 {
            10.0 * (e / total).log10()
        } else {
            -300.0
        }
    };
    let (tail_energy_db, pre_energy_db) = match used {
        PhaseRequest::Linear => (energy_db(0..tail).max(energy_db(n - tail..n)), None),
        PhaseRequest::Mixed => (energy_db(n - tail..n), Some(energy_db(0..pre))),
        _ => (energy_db(n - tail..n), None),
    };
    let fall = |m: usize| 0.5 * (1.0 + (PI * (m as f64 + 1.0) / tail as f64).cos());
    for (m, v) in h.iter_mut().enumerate() {
        let mut w = 1.0;
        if m >= n - tail {
            w *= fall(m - (n - tail));
        }
        let rise = match used {
            PhaseRequest::Mixed => pre,
            PhaseRequest::Linear => tail,
            _ => 0,
        };
        if m < rise {
            w *= 0.5 * (1.0 - (PI * m as f64 / rise as f64).cos());
        }
        *v *= w;
    }
    let taps: Vec<f32> = h.iter().map(|&v| v as f32).collect();
    let latency = match used {
        PhaseRequest::Mixed => pre,
        PhaseRequest::Linear => half,
        _ => 0,
    };

    // 6. The taps against the analytic filter.
    let fir_db: Vec<f64> = if opts.verify {
        grid.iter()
            .map(|&f| db(dtft(&taps, fs, f).norm()))
            .collect()
    } else {
        Vec::new()
    };
    let error_db: Vec<f64> = fir_db.iter().zip(&design_db).map(|(a, b)| a - b).collect();
    let check = opts.verify.then(|| {
        let mut worst = (0.0f64, grid[0]);
        let mut ss = 0.0;
        for (e, &f) in error_db.iter().zip(&grid) {
            if e.abs() > worst.0 {
                worst = (e.abs(), f);
            }
            ss += e * e;
        }
        let bad: Vec<bool> = error_db.iter().map(|e| e.abs() > TOLERANCE_DB).collect();
        Check {
            band_hz: opts.band_hz,
            tolerance_db: TOLERANCE_DB,
            max_abs_error_db: worst.0,
            at_hz: worst.1,
            rms_error_db: (ss / error_db.len() as f64).sqrt(),
            met: !bad.iter().any(|b| *b),
            exceeded_hz: ranges(&grid, &bad),
            resolution_hz: df,
        }
    });

    // Report pieces.
    let in_band_db = || {
        grid.iter()
            .zip(&design_db)
            .filter(|(f, _)| in_band(**f))
            .map(|(f, v)| (*f, *v))
    };
    let max_boost = in_band_db().fold(
        (f64::NEG_INFINITY, 0.0),
        |m, (f, v)| {
            if v > m.0 {
                (v, f)
            } else {
                m
            }
        },
    );
    let max_cut = in_band_db().fold(
        (f64::INFINITY, 0.0),
        |m, (f, v)| {
            if v < m.0 {
                (v, f)
            } else {
                m
            }
        },
    );
    let length = length_fit.map(|(poles, binding, needed, rec, zeros, fit_max, fit_notes)| {
        let rhp_zeros_hz = zeros
            .iter()
            .filter(|(f, rhp)| {
                *rhp && decision
                    .band_hz
                    .is_some_and(|(lo, hi)| *f >= lo && *f <= hi)
            })
            .map(|(f, _)| *f)
            .collect();
        LengthCheck {
            fitted,
            poles,
            binding,
            needed_s: needed,
            recommended_n: rec,
            n,
            half_length_s: n as f64 / (2.0 * fs),
            covered: n as f64 / (2.0 * fs) >= needed,
            rhp_zeros_hz,
            fit_max_db: fit_max,
            notes: fit_notes,
        }
    });
    if let Some(l) = &length {
        if !l.covered {
            let b = l.binding.as_ref().expect("a need implies a binding pole");
            flags.push(flag(
                "ir_length_short",
                format!(
                    "N = {n} holds {:.3} s of decay but the filter's pole at {:.1} Hz (Q = {:.1}) needs {:.3} s to fall 80 dB (erratum E46){}; its tail is cut and wraps around.",
                    l.half_length_s,
                    b.f_hz,
                    b.q,
                    b.needed_s,
                    if l.recommended_n > n_max {
                        format!(", which would take N = {}, beyond the audition limit of {n_max}", l.recommended_n)
                    } else {
                        format!("; N = {} would hold it", l.recommended_n)
                    }
                ),
            ));
        }
    }
    if let Some(c) = &check {
        if c.met {
            notes.push(format!(
                "The taps reproduce the analytic filter within {:.3} dB from {:.0} Hz to {:.0} Hz (largest error at {:.1} Hz), inside the {} dB of spec Section 16.",
                c.max_abs_error_db, c.band_hz.0, c.band_hz.1, c.at_hz, TOLERANCE_DB
            ));
        } else {
            let where_: Vec<String> = c
                .exceeded_hz
                .iter()
                .map(|(a, b)| {
                    if (b / a - 1.0).abs() < 1e-9 {
                        format!("{a:.1} Hz")
                    } else {
                        format!("{a:.1} Hz to {b:.1} Hz")
                    }
                })
                .collect();
            flags.push(flag(
                "tolerance_not_met",
                format!(
                    "The taps depart from the analytic filter by up to {:.2} dB (at {:.1} Hz), more than {} dB at {}. The filter's frequency resolution is fs/N = {:.2} Hz; detail narrower than that, or a decay longer than N/(2·fs) = {:.3} s, cannot be reproduced.",
                    c.max_abs_error_db,
                    c.at_hz,
                    TOLERANCE_DB,
                    where_.join(", "),
                    df,
                    n as f64 / (2.0 * fs)
                ),
            ));
        }
    }
    if tail_energy_db > -60.0 {
        flags.push(flag(
            "tail_not_decayed",
            format!(
                "The last {} samples of the impulse response hold {:.1} dB of its energy before the window: the filter has not decayed within N = {n}.",
                tail, tail_energy_db
            ),
        ));
    }
    if max_boost.0 > inversion::InversionOptions::default().boost_cap_db {
        flags.push(flag(
            "large_boost",
            format!(
                "The filter boosts by up to {:.1} dB (at {:.0} Hz) relative to its 500 Hz–2 kHz level; the level match and the output limiter take this into account.",
                max_boost.0, max_boost.1
            ),
        ));
    }

    // Baseline summaries, flags and state.
    let cand_summary = serde_json::json!({
        "probe": cand.probe,
        "quantity": cand.quantity,
        "unit": cand.unit,
        "fixture": cand.fixture,
        "drive": cand.drive,
        "shading": cand.shading,
        "netlist_sha256": cand.netlist_sha256,
        "overrides": cand.overrides,
        "parameters": cand.parameters,
    });
    let (base_summary, base_state) = match &base {
        Baseline::Design(b) => {
            if b.fixture != cand.fixture {
                flags.push(flag(
                    "reference_point_mismatch",
                    format!(
                        "The candidate reads {} and the baseline {}: the ratio compares different reference points.",
                        cand.reference_point(),
                        b.reference_point()
                    ),
                ));
            }
            if b.drive.label != cand.drive.label {
                flags.push(flag(
                    "drive_differs",
                    format!(
                        "The designs are stated at different drives (candidate: {}; baseline: {}). The level match removes the overall level difference, not any frequency dependence of the drive.",
                        cand.drive.label, b.drive.label
                    ),
                ));
            }
            (
                serde_json::json!({
                    "kind": "netlist",
                    "probe": b.probe,
                    "quantity": b.quantity,
                    "unit": b.unit,
                    "fixture": b.fixture,
                    "drive": b.drive,
                    "shading": b.shading,
                    "netlist_sha256": b.netlist_sha256,
                    "overrides": b.overrides,
                    "parameters": b.parameters,
                }),
                serde_json::json!({
                    "kind": "netlist",
                    "netlist_sha256": b.netlist_sha256,
                    "overrides": b.overrides,
                    "probe": b.probe,
                }),
            )
        }
        Baseline::Magnitude(m) => {
            match (&m.fixture, &cand.fixture) {
                (Some(a), Some(b)) if fixture::compare(a, b) != fixture::Match::Same => {
                    flags.push(flag(
                        "fixture_mismatch",
                        format!(
                            "The baseline is on fixture {a} and the candidate's probe reads {b}: responses on different fixtures differ by several dB above 2 kHz, so the filter contains that difference too."
                        ),
                    ));
                }
                (None, _) | (_, None) => flags.push(flag(
                    "fixture_unspecified",
                    "The fixture of the baseline or of the candidate's probe is not known, so it cannot be checked that they share a reference point.",
                )),
                _ => {}
            }
            notes.extend(m.notes.iter().cloned());
            (
                serde_json::json!({
                    "kind": m.kind,
                    "name": m.name,
                    "label": m.label,
                    "fixture": m.fixture,
                    "sha256": m.sha256,
                }),
                serde_json::json!({
                    "kind": m.kind,
                    "name": m.name,
                    "sha256": m.sha256,
                }),
            )
        }
        Baseline::None => (
            serde_json::json!({"kind": "none"}),
            serde_json::json!({"kind": "none"}),
        ),
    };
    let state = serde_json::json!({
        "schema": "acoustilab-audition/0.1",
        "engine": concat!("acoustilab ", env!("CARGO_PKG_VERSION")),
        "candidate": {
            "netlist_sha256": cand.netlist_sha256,
            "overrides": cand.overrides,
            "probe": cand.probe,
        },
        "baseline": base_state,
        "mode": opts.mode.as_str(),
        "phase": {"requested": opts.phase.as_str(), "used": used.as_str()},
        "fs_Hz": fs,
        "n": n,
        "band_Hz": [band.0, band.1],
    });

    Ok(Audition {
        mode: opts.mode,
        fs_hz: fs,
        n,
        taps,
        latency_samples: latency,
        band_hz: band,
        anchor_gain_db: -anchor,
        phase: PhaseReport {
            requested: opts.phase.as_str(),
            used: used.as_str(),
            reason,
            polarity: pol,
            decision,
            delay_removed_s: delay_removed,
            hybrid_from_hz: hybrid_from,
        },
        length,
        length_error,
        check,
        tail_energy_db,
        pre_energy_db,
        grid_hz: grid,
        design_db,
        fir_db,
        error_db,
        max_boost_db: max_boost,
        max_cut_db: max_cut,
        inverse,
        flags,
        notes,
        state,
        candidate: cand_summary,
        baseline: base_summary,
    })
}
