//! Regularised inversion of a magnitude-only baseline: a target curve or an
//! imported measurement (spec Section 16, "Inverting measured responses").
//!
//! The audition filter is the candidate divided by the baseline. A model
//! baseline is divided exactly; a target or a measured curve is inverted
//! here, with the four rules of the spec, in this order, on a working grid
//! of [`GRID_POINTS_PER_OCTAVE`] log-spaced points over the curve's range:
//!
//! 1. **Pre-smoothing**: 1/6-octave power smoothing of the level B(f)
//!    ([`crate::targets::smoothing::smooth_power`], IEC 61260-1 base-10
//!    windows): B₆.
//! 2. **Notch rule**: the envelope E is B₆ smoothed again over one octave.
//!    A notch is a maximal interval where B₆ < E; one whose deepest point
//!    lies more than `notch_limit_dB` (15 dB) below E is not inverted: B₆ is
//!    replaced by E across the whole interval. At its ends B₆ = E, so the
//!    replacement is continuous. Shallower notches are inverted (within the
//!    boost cap).
//! 3. **Normalisation**: b(f) = 10^((B₆(f) − L_ref)/20), with L_ref the mean
//!    of B₆ in dB over the working-grid points from 500 Hz to 2 kHz (the
//!    whole inversion band if the curve does not cover that). "Boost" and
//!    "cut" are relative to L_ref, the band the audition filter itself is
//!    anchored in, so the cap limits the filter's gain relative to its
//!    mid-band level. The spec does not say what the cap is relative to.
//! 4. **Kirkeby–Nelson inverse** (Kirkeby, Nelson, Hamada and
//!    Orduña-Bustamante, IEEE Trans. Speech Audio Process. 6(2), 1998):
//!    c(f) = b(f)/(b(f)² + β(f)), the Tikhonov-regularised least-squares
//!    inverse of a zero-phase b. Its largest value over b is 1/(2√β),
//!    reached at b = √β, so the boost cap A = 10^(cap/20) fixes
//!    β_in = 1/(4A²): 0.01577 for 12 dB, reached where the baseline lies
//!    18.0 dB below L_ref. The cap is Kirkeby's own soft limit, not a clip,
//!    and it costs accuracy towards the cap: the regularised inverse
//!    differs from 1/b by 20·log10(b²/(b² + β)) ([`regularisation_error_db`]):
//!    −0.03 dB 6 dB above L_ref, −0.14 dB at L_ref, −0.53 dB 6 dB below,
//!    −1.94 dB 12 dB below and −6.02 dB at the cap point.
//!
//! **Regularisation profile** β(f): β_in inside the inversion band
//! [f_lo, f_hi] (default 20 Hz–10 kHz, clipped to the curve's range);
//! outside it the baseline is not inverted at all. The audition filter
//! (candidate × c) is held at its band-edge values there, which is the
//! limit β → ∞ of the Tikhonov problem regularised towards the band-edge
//! filter instead of towards zero. Classic Kirkeby regularisation towards
//! zero would low-pass the programme at 10 kHz; holding the band edge
//! leaves it full-band. Outside the band A then differs from B by the
//! held edge level (a constant, within the hold's 6 dB transition): the
//! filter has no shape there, not unity gain. The hold is applied where
//! the filter is built (`audition::audition`).
//!
//! Every constant is an option with the spec's value as default; the
//! report lists the notches found, the largest boost, and the largest
//! departure of the regularised inverse from the exact one.

use crate::targets::curve::Curve;
use crate::targets::smoothing::smooth_power;
use serde::Serialize;

/// Density of the working grid, points per octave. The 1/6-octave window
/// then spans 16 points, and the linear-in-log-f reading between points
/// is exact for the smoothed curve to well under 1e-3 dB.
pub const GRID_POINTS_PER_OCTAVE: f64 = 96.0;

/// The spec's constants (Section 16).
#[derive(Debug, Clone, PartialEq)]
pub struct InversionOptions {
    /// Band inside which the baseline is inverted, Hz.
    pub band_hz: (f64, f64),
    /// Pre-smoothing, 1/N octave.
    pub smoothing: u32,
    /// Envelope smoothing for the notch rule, 1/N octave.
    pub envelope_smoothing: u32,
    /// Largest boost, dB (Kirkeby's soft cap 1/(2√β)).
    pub boost_cap_db: f64,
    /// Notches deeper than this below the envelope are not inverted, dB.
    pub notch_limit_db: f64,
    /// Band whose mean level is the reference of boosts and cuts, Hz.
    pub reference_hz: (f64, f64),
}

impl Default for InversionOptions {
    fn default() -> Self {
        InversionOptions {
            band_hz: (20.0, 10_000.0),
            smoothing: 6,
            envelope_smoothing: 1,
            boost_cap_db: 12.0,
            notch_limit_db: 15.0,
            reference_hz: (500.0, 2000.0),
        }
    }
}

impl InversionOptions {
    /// β inside the band: 1/(4·10^(cap/10)).
    pub fn beta(&self) -> f64 {
        0.25 * 10f64.powf(-self.boost_cap_db / 10.0)
    }

    pub fn check(&self) -> Result<(), String> {
        let (lo, hi) = self.band_hz;
        if !(lo.is_finite() && hi.is_finite() && lo > 0.0 && hi > lo * 1.5) {
            return Err(format!(
                "inversion band {lo} to {hi} Hz must be positive and span more than half an octave"
            ));
        }
        for (name, n) in [
            ("smoothing", self.smoothing),
            ("envelope smoothing", self.envelope_smoothing),
        ] {
            if ![1, 2, 3, 6, 12, 24, 48].contains(&n) {
                return Err(format!(
                    "{name} must be 1/N octave with N in 1, 2, 3, 6, 12, 24, 48"
                ));
            }
        }
        if !(self.boost_cap_db.is_finite() && (0.0..=40.0).contains(&self.boost_cap_db)) {
            return Err("boost cap must be 0 to 40 dB".into());
        }
        if !(self.notch_limit_db.is_finite() && self.notch_limit_db > 0.0) {
            return Err("notch limit must be positive".into());
        }
        Ok(())
    }
}

/// 20·log10(b²/(b² + β)): the regularised inverse c = b/(b² + β) relative
/// to the exact 1/b, dB (≤ 0).
pub fn regularisation_error_db(b: f64, beta: f64) -> f64 {
    20.0 * (b * b / (b * b + beta)).log10()
}

/// A notch found by the notch rule.
#[derive(Debug, Clone, Serialize)]
pub struct Notch {
    #[serde(rename = "from_Hz")]
    pub from_hz: f64,
    #[serde(rename = "to_Hz")]
    pub to_hz: f64,
    /// Deepest point below the envelope, dB, and where.
    #[serde(rename = "depth_dB")]
    pub depth_db: f64,
    #[serde(rename = "at_Hz")]
    pub at_hz: f64,
    /// False when it was deeper than the limit and replaced by the envelope.
    pub inverted: bool,
}

/// The regularised inverse of a baseline on the working grid.
#[derive(Debug, Clone)]
pub struct Inverse {
    /// Working grid over the inversion band, Hz.
    pub freqs: Vec<f64>,
    /// 20·log10 c(f), dB re the baseline's reference level.
    pub inverse_db: Vec<f64>,
    /// The exact inverse of the unsmoothed baseline, −(B − L_ref), dB.
    pub exact_db: Vec<f64>,
    /// The smoothed, notch-processed baseline, B₆ − L_ref, dB.
    pub smoothed_db: Vec<f64>,
    /// Band actually inverted (the option clipped to the curve), Hz.
    pub band_hz: (f64, f64),
    /// First and last frequency of the baseline curve, Hz.
    pub curve_range_hz: (f64, f64),
    pub reference_db: f64,
    pub beta: f64,
    pub notches: Vec<Notch>,
    pub options: InversionOptions,
}

impl Inverse {
    /// c(f) in dB, linear in log f between grid points; the band-edge value
    /// outside the band.
    pub fn at(&self, f: f64) -> f64 {
        let fr = &self.freqs;
        if f <= fr[0] {
            return self.inverse_db[0];
        }
        let last = fr.len() - 1;
        if f >= fr[last] {
            return self.inverse_db[last];
        }
        let j = fr.partition_point(|&x| x < f);
        if fr[j] == f {
            return self.inverse_db[j];
        }
        let t = (f / fr[j - 1]).ln() / (fr[j] / fr[j - 1]).ln();
        self.inverse_db[j - 1] + t * (self.inverse_db[j] - self.inverse_db[j - 1])
    }

    /// Largest boost of the regularised inverse, dB, and where.
    pub fn max_boost(&self) -> (f64, f64) {
        self.inverse_db.iter().zip(&self.freqs).fold(
            (f64::NEG_INFINITY, 0.0),
            |(m, at), (&v, &f)| {
                if v > m {
                    (v, f)
                } else {
                    (m, at)
                }
            },
        )
    }

    /// Largest |regularised − exact| inverse over the band, dB, and where:
    /// the combined effect of smoothing, the notch rule and the cap.
    pub fn max_departure(&self) -> (f64, f64) {
        self.inverse_db
            .iter()
            .zip(&self.exact_db)
            .zip(&self.freqs)
            .fold((0.0, 0.0), |(m, at), ((&a, &b), &f)| {
                if (a - b).abs() > m {
                    ((a - b).abs(), f)
                } else {
                    (m, at)
                }
            })
    }
}

/// Log-spaced grid from `lo` to `hi` (both included) at `ppo` points per
/// octave.
pub fn log_grid(lo: f64, hi: f64, ppo: f64) -> Vec<f64> {
    let n = ((hi / lo).log2() * ppo).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| lo * (hi / lo).powf(i as f64 / n as f64))
        .collect()
}

/// Inverts `curve` (levels in dB) over the band of `opts` clipped to the
/// curve's range.
pub fn invert(curve: &Curve, opts: &InversionOptions) -> Result<Inverse, String> {
    opts.check()?;
    let (c_lo, c_hi) = curve.range();
    let lo = opts.band_hz.0.max(c_lo);
    let hi = opts.band_hz.1.min(c_hi);
    if hi < lo * 1.5 {
        return Err(format!(
            "the baseline covers {c_lo} to {c_hi} Hz, which leaves less than half an octave of the inversion band {} to {} Hz",
            opts.band_hz.0, opts.band_hz.1
        ));
    }
    // Smoothing works on the curve's whole range, so the windows near the
    // band edges see data on both sides where the curve has it.
    let mut grid = log_grid(c_lo, c_hi, GRID_POINTS_PER_OCTAVE);
    for edge in [lo, hi] {
        if !grid.iter().any(|&f| (f / edge - 1.0).abs() < 1e-9) {
            grid.push(edge);
        }
    }
    grid.sort_by(|a, b| a.total_cmp(b));
    let raw: Vec<f64> = grid
        .iter()
        .map(|&f| curve.at(f).expect("grid inside the curve's range"))
        .collect();
    let on_grid = Curve::new(grid.clone(), raw.clone()).map_err(|e| e.to_string())?;
    let smoothed = smooth_power(&on_grid, opts.smoothing).map_err(|e| e.to_string())?;
    let envelope = smooth_power(&smoothed, opts.envelope_smoothing).map_err(|e| e.to_string())?;
    let mut b6 = smoothed.db().to_vec();
    let env = envelope.db();
    let in_band = |f: f64| f >= lo * (1.0 - 1e-12) && f <= hi * (1.0 + 1e-12);

    // Notch rule: maximal runs with B6 < E.
    let mut notches = Vec::new();
    let mut i = 0;
    while i < grid.len() {
        if b6[i] >= env[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < grid.len() && b6[i] < env[i] {
            i += 1;
        }
        let (mut depth, mut at) = (0.0, grid[start]);
        for j in start..i {
            if env[j] - b6[j] > depth {
                depth = env[j] - b6[j];
                at = grid[j];
            }
        }
        // Only runs whose deepest point lies inside the band matter.
        if !in_band(at) || depth < 1e-9 {
            continue;
        }
        let inverted = depth <= opts.notch_limit_db;
        if !inverted {
            b6[start..i].copy_from_slice(&env[start..i]);
        }
        // Reported only when deep enough to be called a notch (3 dB), or
        // when replaced.
        if depth >= 3.0 || !inverted {
            notches.push(Notch {
                from_hz: grid[start],
                to_hz: grid[i - 1],
                depth_db: depth,
                at_hz: at,
                inverted,
            });
        }
    }

    let band: Vec<usize> = (0..grid.len()).filter(|&j| in_band(grid[j])).collect();
    let (r_lo, r_hi) = opts.reference_hz;
    let mut refs: Vec<usize> = (0..grid.len())
        .filter(|&j| grid[j] >= r_lo * (1.0 - 1e-12) && grid[j] <= r_hi * (1.0 + 1e-12))
        .collect();
    if c_lo > r_lo * (1.0 + 1e-9) || c_hi < r_hi * (1.0 - 1e-9) || refs.is_empty() {
        refs = band.clone();
    }
    let reference = refs.iter().map(|&j| b6[j]).sum::<f64>() / refs.len() as f64;
    let beta = opts.beta();
    let freqs: Vec<f64> = band.iter().map(|&j| grid[j]).collect();
    let smoothed_db: Vec<f64> = band.iter().map(|&j| b6[j] - reference).collect();
    let inverse_db = smoothed_db
        .iter()
        .map(|&l| {
            let b = 10f64.powf(l / 20.0);
            20.0 * (b / (b * b + beta)).log10()
        })
        .collect();
    let exact_db = band.iter().map(|&j| reference - raw[j]).collect();
    Ok(Inverse {
        freqs,
        inverse_db,
        exact_db,
        smoothed_db,
        band_hz: (lo, hi),
        curve_range_hz: (c_lo, c_hi),
        reference_db: reference,
        beta,
        notches,
        options: opts.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kirkeby_cap_and_error() {
        let o = InversionOptions::default();
        let beta = o.beta();
        // The maximum of b/(b² + β) is 1/(2√β) at b = √β: the cap.
        let peak = 20.0 * (1.0 / (2.0 * beta.sqrt())).log10();
        assert!((peak - 12.0).abs() < 1e-12);
        let b = beta.sqrt();
        assert!((20.0 * (b / (b * b + beta)).log10() - 12.0).abs() < 1e-12);
        // Values from an independent evaluation (Python, math module).
        assert!((beta - 0.015773933612004833).abs() < 1e-15);
        for (b, err) in [
            (2.0, -0.034185301258283736),
            (1.0, -0.13594127885494126),
            (0.5, -0.5314475119220499),
            (10f64.powf(-12.0 / 20.0), -1.938200260161128),
            (b, -6.020599913279624),
        ] {
            assert!(
                (regularisation_error_db(b, beta) - err).abs() < 1e-12,
                "b = {b}"
            );
        }
    }
}
