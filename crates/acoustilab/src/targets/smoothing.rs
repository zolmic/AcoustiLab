//! Fractional-octave smoothing.
//!
//! **Power smoothing** ([`smooth_power`]): at every frequency f_i of the
//! curve, the smoothed level is
//!
//! `S_i = 10·log10( (1/2h_i) ∫_{x_i−h_i}^{x_i+h_i} P(x) dx )`,  x = log10 f,
//!
//! where P is the power `10^(L/10)` interpolated linearly in x between the
//! samples, and the window is the IEC 61260-1:2014 base-10 1/N-octave band
//! centred on f_i: edges `f·G^(±1/2N)`, `G = 10^(3/10)`, so `h = 3/(20N)`
//! decades. Near the ends the window shrinks symmetrically,
//! `h_i = min(h, x_i − x_0, x_{n−1} − x_i)`, so a sloped response is not
//! tilted by a one-sided window; the end points are left unsmoothed. The
//! integral of the piecewise-linear power is exact (trapezoids on the
//! window's pieces).
//!
//! **Complex smoothing** ([`smooth_complex`]) applies the same window to the
//! real and imaginary parts of a complex response. It is a different
//! operation: where the phase turns inside the window the complex average
//! falls below the power average. Use it only when phase must be kept.

use super::curve::Curve;
use super::{Result, TargetError};
use crate::C64;

/// The accepted fractions: 1/N octave for these N.
pub const FRACTIONS: [u32; 7] = [1, 2, 3, 6, 12, 24, 48];

/// Half-width of the 1/N-octave window in decades of frequency.
pub fn half_width_decades(n: u32) -> f64 {
    3.0 / (20.0 * n as f64)
}

fn check_fraction(n: u32) -> Result<()> {
    if FRACTIONS.contains(&n) {
        Ok(())
    } else {
        Err(TargetError::Options(format!(
            "smoothing 1/{n} octave is not offered; use one of 1, 2, 3, 6, 12, 24, 48 or none"
        )))
    }
}

/// Mean over `[c − h, c + h]` of the function that is linear between the
/// points (x_j, y_j). The window must lie inside `[x_0, x_{n−1}]`.
fn window_mean(x: &[f64], y: &[f64], c: f64, h: f64) -> f64 {
    let (a, b) = (c - h, c + h);
    let lerp = |j: usize, u: f64| y[j] + (y[j + 1] - y[j]) * (u - x[j]) / (x[j + 1] - x[j]);
    // First segment whose right end is beyond a.
    let mut j = x.partition_point(|&v| v <= a).saturating_sub(1);
    let mut sum = 0.0;
    while j + 1 < x.len() && x[j] < b {
        let u0 = a.max(x[j]);
        let u1 = b.min(x[j + 1]);
        if u1 > u0 {
            sum += (u1 - u0) * 0.5 * (lerp(j, u0) + lerp(j, u1));
        }
        j += 1;
    }
    sum / (b - a)
}

/// Applies the window to `y` sampled at `x = log10 f` (see the module docs).
fn smooth_values(x: &[f64], y: &[f64], h: f64) -> Vec<f64> {
    let (x0, xn) = (x[0], x[x.len() - 1]);
    x.iter()
        .zip(y)
        .map(|(&c, &v)| {
            let hi = h.min(c - x0).min(xn - c);
            if hi > 0.0 {
                window_mean(x, y, c, hi)
            } else {
                v
            }
        })
        .collect()
}

/// 1/N-octave power smoothing of a magnitude response, at the curve's own
/// frequencies.
pub fn smooth_power(curve: &Curve, n: u32) -> Result<Curve> {
    check_fraction(n)?;
    let x: Vec<f64> = curve.freqs().iter().map(|f| f.log10()).collect();
    let p: Vec<f64> = curve.db().iter().map(|l| 10f64.powf(l / 10.0)).collect();
    let s = smooth_values(&x, &p, half_width_decades(n));
    let last = s.len() - 1;
    // The end points have no window; keep their levels exactly rather than
    // round-tripping them through power.
    let db = s
        .iter()
        .enumerate()
        .map(|(i, v)| {
            if i == 0 || i == last {
                curve.db()[i]
            } else {
                10.0 * v.log10()
            }
        })
        .collect();
    Curve::new(curve.freqs().to_vec(), db)
}

/// Optional smoothing: `None` returns the curve unchanged.
pub fn smooth_opt(curve: &Curve, n: Option<u32>) -> Result<Curve> {
    match n {
        None => Ok(curve.clone()),
        Some(n) => smooth_power(curve, n),
    }
}

/// 1/N-octave smoothing of a complex response (real and imaginary parts
/// averaged over the same window as [`smooth_power`]).
pub fn smooth_complex(f: &[f64], h: &[C64], n: u32) -> Result<Vec<C64>> {
    check_fraction(n)?;
    if f.len() != h.len() || f.len() < 2 {
        return Err(TargetError::Curve(
            "complex smoothing needs equal-length frequency and value arrays of at least two points"
                .into(),
        ));
    }
    // Validates the frequency axis.
    Curve::new(f.to_vec(), vec![0.0; f.len()])?;
    let x: Vec<f64> = f.iter().map(|v| v.log10()).collect();
    let re: Vec<f64> = h.iter().map(|z| z.re).collect();
    let im: Vec<f64> = h.iter().map(|z| z.im).collect();
    let w = half_width_decades(n);
    let (sr, si) = (smooth_values(&x, &re, w), smooth_values(&x, &im, w));
    Ok(sr
        .into_iter()
        .zip(si)
        .map(|(r, i)| C64::new(r, i))
        .collect())
}
