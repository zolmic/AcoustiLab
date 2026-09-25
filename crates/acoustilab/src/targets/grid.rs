//! The evaluation grid of the metrics and scores, and band membership.
//!
//! The default grid is 1/12 octave in base 10: `f_k = 10^(k/40)` Hz for
//! k = 52..=172, i.e. `1000·G^(x/12)` with the IEC 61260-1 octave ratio
//! `G = 10^(3/10)`. That is 121 points from 19.95 Hz to 19.95 kHz, the exact
//! values behind the rounded R40 frequencies (20, 21.2, 22.4, ... 20 000 Hz)
//! that Listen's SoundCheck template and AutoEq evaluate the Harman models
//! on. (IEC 61260-1 places its own even-numbered 1/b-octave midbands half a
//! band away from these; the grid follows the preferred-number series, which
//! contains 1 kHz.)
//!
//! A band `[lo, hi]` holds the grid points from the one nearest `lo` to the
//! one nearest `hi` on a log axis, both included. On the default grid this
//! reproduces membership by nominal frequency: 20 Hz selects 19.95 Hz, 8 kHz
//! selects 7.94 kHz, 16 kHz selects 15.85 kHz.

/// Octave ratio of IEC 61260-1:2014, base 10.
pub const G: f64 = 1.995_262_314_968_879_5; // 10^(3/10)

/// The default 1/12-octave evaluation grid, 19.95 Hz to 19.95 kHz.
pub fn twelfth_octave() -> Vec<f64> {
    (52..=172).map(|k| 10f64.powf(k as f64 / 40.0)).collect()
}

/// Index of the grid point nearest to `f` on a log axis (ties go to the
/// lower index). `grid` must be non-empty and increasing.
pub fn nearest(grid: &[f64], f: f64) -> usize {
    let x = f.ln();
    let mut best = 0;
    let mut d = f64::INFINITY;
    for (i, g) in grid.iter().enumerate() {
        let di = (g.ln() - x).abs();
        if di < d {
            d = di;
            best = i;
        }
    }
    best
}

/// Indices of the grid points in the band `[lo, hi]` (see the module docs).
pub fn band(grid: &[f64], lo: f64, hi: f64) -> std::ops::Range<usize> {
    if grid.is_empty() {
        return 0..0;
    }
    let (a, b) = (nearest(grid, lo), nearest(grid, hi));
    if b < a {
        return a..a;
    }
    a..b + 1
}

/// True when points actually used, spanning `[first, last]`, reach the
/// band edges to within half a 1/12-octave step.
pub fn covers(first: f64, last: f64, lo: f64, hi: f64) -> bool {
    let half = 2f64.powf(1.0 / 24.0) * (1.0 + 1e-9);
    first <= lo * half && last >= hi / half
}

/// Checks a user-supplied grid: finite, positive, strictly increasing, at
/// least two points.
pub fn check(grid: &[f64]) -> Result<(), String> {
    if grid.len() < 2 {
        return Err("grid_Hz needs at least two frequencies".into());
    }
    for (i, &f) in grid.iter().enumerate() {
        if !(f.is_finite() && f > 0.0) {
            return Err(format!("grid_Hz: {f} is not a positive frequency"));
        }
        if i > 0 && f <= grid[i - 1] {
            return Err(format!("grid_Hz must increase strictly (at index {i})"));
        }
    }
    Ok(())
}
