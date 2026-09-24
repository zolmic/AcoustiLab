//! Special functions for the thermoviscous and radiation elements.
//!
//! * Modified Bessel ratios I_{ν+1}(z)/I_ν(z) for complex z, by continued
//!   fraction (small and moderate |z|) and Hankel asymptotics (large |z|).
//! * The viscous/thermal shape functions of slits and circular ducts and
//!   their complements 1 − F, computed without cancellation.
//! * Bessel J1 and Struve H1 of real argument for the baffled-piston
//!   radiation impedance, by series and Gauss–Legendre quadrature.

use crate::C64;
use std::f64::consts::PI;
use std::sync::OnceLock;

const ZERO: C64 = C64::new(0.0, 0.0);
const ONE: C64 = C64::new(1.0, 0.0);

/// |z| above which the Hankel asymptotic ratio replaces the continued fraction.
pub const ASYMPTOTIC_SWITCH: f64 = 60.0;

/// I_{ν+1}(z) / I_ν(z) for complex z with Re z ≥ 0.
pub fn bessel_i_ratio(nu: u32, z: C64) -> C64 {
    if z == ZERO {
        return ZERO;
    }
    if z.norm() > ASYMPTOTIC_SWITCH {
        return hankel_i_series(nu + 1, z) / hankel_i_series(nu, z);
    }
    // r_ν = 1 / (2(ν+1)/z + 1 / (2(ν+2)/z + ...)), modified Lentz. `tiny`
    // must survive squaring inside complex division, hence 1e-30.
    let tiny = C64::new(1e-30, 0.0);
    let zi = z.inv();
    let mut f = tiny;
    let mut c = f;
    let mut d = ZERO;
    for k in 1..100_000u32 {
        let b = zi * (2.0 * (nu + k) as f64);
        d = b + d;
        if d == ZERO {
            d = tiny;
        }
        c = b + ONE / c;
        if c == ZERO {
            c = tiny;
        }
        d = d.inv();
        let delta = c * d;
        f *= delta;
        if (delta - ONE).norm() < 4e-16 {
            break;
        }
    }
    f
}

/// Σ_k (−1)^k a_k(ν) / z^k, the Hankel asymptotic series of
/// I_ν(z)·sqrt(2πz)·e^{−z}, summed until terms stop decreasing.
fn hankel_i_series(nu: u32, z: C64) -> C64 {
    let mu = 4.0 * (nu as f64).powi(2);
    let mut term = ONE;
    let mut sum = ONE;
    let zi = z.inv();
    let mut prev = f64::INFINITY;
    for k in 1..200 {
        let m = (2 * k - 1) as f64;
        term *= -(mu - m * m) / (k as f64 * 8.0) * zi;
        let t = term.norm();
        if t > prev {
            break;
        }
        sum += term;
        if t < 1e-17 * sum.norm() {
            break;
        }
        prev = t;
    }
    sum
}

/// Numerically stable complex tanh.
pub fn tanh(z: C64) -> C64 {
    if z.re > 20.0 {
        let e = (-2.0 * z).exp();
        (ONE - e) / (ONE + e)
    } else if z.re < -20.0 {
        let e = (2.0 * z).exp();
        (e - ONE) / (e + ONE)
    } else {
        z.tanh()
    }
}

/// Shape function of a duct cross-section. `F` is the transverse average of
/// the normalised velocity (or temperature) profile; `one_minus` is 1 − F,
/// returned separately because it is tiny at low frequency.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    pub f: C64,
    pub one_minus: C64,
}

/// Slit of half-gap h/2: F = tanh(z)/z with z = k·h/2.
pub fn shape_slit(z: C64) -> Shape {
    if z.norm() < 0.2 {
        // 1 − tanh(z)/z = z²/3 − 2z⁴/15 + 17z⁶/315 − 62z⁸/2835 + 1382z¹⁰/155925 − ...
        let z2 = z * z;
        let coeffs = [
            1.0 / 3.0,
            -2.0 / 15.0,
            17.0 / 315.0,
            -62.0 / 2835.0,
            1382.0 / 155_925.0,
            -21_844.0 / 6_081_075.0,
        ];
        let mut om = ZERO;
        let mut p = z2;
        for c in coeffs {
            om += p * c;
            p *= z2;
        }
        return Shape {
            f: ONE - om,
            one_minus: om,
        };
    }
    let f = tanh(z) / z;
    Shape {
        f,
        one_minus: ONE - f,
    }
}

/// Circular duct of radius a: F = 2 I1(z)/(z I0(z)) with z = k·a, and the
/// identity 1 − F = I2(z)/I0(z).
///
/// Note: with the e^{+jωt} convention and k = sqrt(jωρ/μ) the *modified*
/// Bessel functions are required; the J-form printed in spec Appendix D gives
/// negative resistance (see docs/spec-errata.md).
pub fn shape_circle(z: C64) -> Shape {
    let r1 = bessel_i_ratio(0, z);
    let r2 = bessel_i_ratio(1, z);
    let f = 2.0 * r1 / z;
    Shape {
        f,
        one_minus: r1 * r2,
    }
}

// ----- Real-argument functions for radiation impedance -------------------

/// Gauss–Legendre nodes and weights on [-1, 1] (24 points), computed once.
fn gauss_legendre() -> &'static (Vec<f64>, Vec<f64>) {
    static GL: OnceLock<(Vec<f64>, Vec<f64>)> = OnceLock::new();
    GL.get_or_init(|| {
        let n = 24usize;
        let mut x = vec![0.0; n];
        let mut w = vec![0.0; n];
        for i in 0..n.div_ceil(2) {
            let mut t = (PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
            let mut dp = 0.0;
            for _ in 0..100 {
                let (mut p0, mut p1) = (1.0, t);
                for k in 2..=n {
                    let p2 = ((2 * k - 1) as f64 * t * p1 - (k - 1) as f64 * p0) / k as f64;
                    p0 = p1;
                    p1 = p2;
                }
                dp = n as f64 * (t * p1 - p0) / (t * t - 1.0);
                let dt = p1 / dp;
                t -= dt;
                if dt.abs() < 1e-16 {
                    break;
                }
            }
            x[i] = -t;
            x[n - 1 - i] = t;
            let wi = 2.0 / ((1.0 - t * t) * dp * dp);
            w[i] = wi;
            w[n - 1 - i] = wi;
        }
        (x, w)
    })
}

/// ∫_a^b g(θ) dθ by composite 24-point Gauss–Legendre over `panels` panels.
fn integrate(a: f64, b: f64, panels: usize, g: impl Fn(f64) -> f64) -> f64 {
    let (x, w) = gauss_legendre();
    let h = (b - a) / panels as f64;
    let mut s = 0.0;
    for p in 0..panels {
        let lo = a + p as f64 * h;
        let mid = lo + 0.5 * h;
        for (xi, wi) in x.iter().zip(w) {
            s += wi * g(mid + 0.5 * h * xi);
        }
    }
    s * 0.5 * h
}

fn panels_for(x: f64) -> usize {
    (x.abs() / 6.0).ceil() as usize + 1
}

/// Bessel function J1(x), real x ≥ 0.
pub fn bessel_j1(x: f64) -> f64 {
    if x < 2.0 {
        // Σ (−1)^k (x/2)^{2k+1} / (k!(k+1)!)
        let h = 0.5 * x;
        let mut term = h;
        let mut sum = term;
        for k in 1..30 {
            term *= -h * h / (k as f64 * (k + 1) as f64);
            sum += term;
            if term.abs() < 1e-18 * sum.abs() {
                break;
            }
        }
        return sum;
    }
    // J1(x) = (1/π) ∫_0^π cos(θ − x sin θ) dθ
    integrate(0.0, PI, 2 * panels_for(x), |t| (t - x * t.sin()).cos()) / PI
}

/// Struve function H1(x), real x ≥ 0.
pub fn struve_h1(x: f64) -> f64 {
    if x < 2.0 {
        // Σ (−1)^k (x/2)^{2k+2} / (Γ(k+3/2) Γ(k+5/2))   (DLMF 11.2.1)
        let h = 0.5 * x;
        // Γ(3/2)Γ(5/2) = (√π/2)(3√π/4) = 3π/8
        let mut term = h * h / (3.0 * PI / 8.0);
        let mut sum = term;
        for k in 1..30 {
            let kf = k as f64;
            term *= -h * h / ((kf + 0.5) * (kf + 1.5));
            sum += term;
            if term.abs() < 1e-18 * sum.abs() {
                break;
            }
        }
        return sum;
    }
    // H1(x) = (2x/π) ∫_0^{π/2} cos²θ sin(x sin θ) dθ
    2.0 * x / PI
        * integrate(0.0, 0.5 * PI, panels_for(x), |t| {
            t.cos().powi(2) * (x * t.sin()).sin()
        })
}

/// Normalised radiation resistance of a baffled piston, 1 − 2J1(x)/x, x = 2ka.
pub fn piston_r1(x: f64) -> f64 {
    if x < 2.0 {
        // Σ_{k≥1} (−1)^{k+1} (x/2)^{2k} / (k!(k+1)!)
        let h2 = 0.25 * x * x;
        let mut term = h2 / 2.0;
        let mut sum = term;
        for k in 2..30 {
            term *= -h2 / (k as f64 * (k + 1) as f64);
            sum += term;
            if term.abs() < 1e-18 * sum.abs() {
                break;
            }
        }
        return sum;
    }
    1.0 - 2.0 * bessel_j1(x) / x
}

/// Normalised radiation reactance of a baffled piston, 2H1(x)/x, x = 2ka.
pub fn piston_x1(x: f64) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    2.0 * struve_h1(x) / x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_shape_low_frequency_limits() {
        // 1 − F ≈ z²/8 for small z (Poiseuille).
        let z = C64::new(1e-3, 1e-3);
        let s = shape_circle(z);
        let expected = z * z / 8.0;
        assert!((s.one_minus - expected).norm() < 1e-6 * expected.norm());
    }

    #[test]
    fn slit_shape_continuous_across_series_switch() {
        let theta = std::f64::consts::FRAC_PI_4;
        let below = shape_slit(C64::from_polar(0.2 - 1e-12, theta));
        let above = shape_slit(C64::from_polar(0.2 + 1e-12, theta));
        assert!((below.one_minus - above.one_minus).norm() < 1e-10 * below.one_minus.norm());
    }

    #[test]
    fn bessel_ratio_continuous_across_asymptotic_switch() {
        let theta = std::f64::consts::FRAC_PI_4;
        for nu in 0..2 {
            let a = bessel_i_ratio(nu, C64::from_polar(ASYMPTOTIC_SWITCH - 1e-9, theta));
            let b = bessel_i_ratio(nu, C64::from_polar(ASYMPTOTIC_SWITCH + 1e-9, theta));
            assert!((a - b).norm() < 1e-12, "nu={nu}: {a} vs {b}");
        }
    }

    #[test]
    fn j1_and_h1_reference_values() {
        // Reference values (Abramowitz & Stegun / mpmath).
        assert!((bessel_j1(1.0) - 0.440_050_585_744_933_5).abs() < 1e-13);
        assert!((bessel_j1(10.0) - 0.043_472_746_168_861_44).abs() < 1e-13);
        assert!((struve_h1(1.0) - 0.198_457_336_201_944_3).abs() < 1e-13);
        assert!((struve_h1(10.0) - 0.891_832_492_094_538).abs() < 1e-12);
    }
}
