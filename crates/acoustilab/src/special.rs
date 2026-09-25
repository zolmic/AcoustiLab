//! Special functions for the thermoviscous and radiation elements.
//!
//! * Modified Bessel ratios I_{ν+1}(z)/I_ν(z) for complex z with Re z ≥ 0:
//!   continued fraction below |z| = [`ASYMPTOTIC_SWITCH`], Hankel
//!   asymptotics with both exponentials above it.
//! * The viscous/thermal shape functions of slits, circular and rectangular
//!   ducts and their complements 1 − F, computed without cancellation.
//! * Bessel J1 and Struve H1 of real argument for the baffled-piston
//!   radiation impedance.
//! * The Levine–Schwinger radiation of an unflanged pipe.
//!
//! Every function is checked against mpmath in `tests/special_functions.rs`
//! (references from `tools/refgen/special_refs.py`); the accuracy each one
//! reaches is stated in its doc comment.

use crate::C64;
use std::f64::consts::{FRAC_2_PI, FRAC_PI_2, FRAC_PI_4, PI};
use std::sync::OnceLock;

const ZERO: C64 = C64::new(0.0, 0.0);
const ONE: C64 = C64::new(1.0, 0.0);

/// Euler–Mascheroni constant γ.
const EULER_GAMMA: f64 = 0.577_215_664_901_532_9;

/// |z| at and above which the Hankel asymptotic expansion replaces the
/// continued fraction. At |z| = 18 the smallest term of the expansion for
/// ν ≤ 3 is below 4e-17 (computed from a_k(ν), DLMF 10.17.1), so optimal
/// truncation is exact to double precision.
pub const ASYMPTOTIC_SWITCH: f64 = 18.0;

/// I_{ν+1}(z) / I_ν(z) for complex z with Re z ≥ 0.
///
/// Relative error against mpmath ≤ 1e-14 for |z| in [1e-4, 1e4] and
/// 0 ≤ arg z ≤ 0.49π (worst near the poles close to the imaginary axis,
/// where the ratio itself is ill-conditioned).
pub fn bessel_i_ratio(nu: u32, z: C64) -> C64 {
    if z == ZERO {
        return ZERO;
    }
    if z.norm() >= ASYMPTOTIC_SWITCH {
        hankel_i_ratio(nu, z)
    } else {
        i_ratio_cf(nu, z)
    }
}

/// Continued fraction r_ν = 1/(2(ν+1)/z + 1/(2(ν+2)/z + ...)), evaluated by
/// the modified Lentz method. It converges for every z ≠ 0 because I_ν is
/// the minimal solution of the recurrence (Perron–Pincherle).
fn i_ratio_cf(nu: u32, z: C64) -> C64 {
    // `tiny` must survive squaring inside complex division, hence 1e-30.
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
        if (delta - ONE).norm() < 2e-16 {
            break;
        }
    }
    f
}

/// Sums of the Hankel expansion of I_ν (DLMF 10.40.5):
/// S₋ = Σ (−1)^k a_k(ν)/z^k and S₊ = Σ a_k(ν)/z^k, truncated at the
/// smallest term.
fn hankel_i_sums(nu: u32, zi: C64) -> (C64, C64) {
    let mu = 4.0 * (nu as f64).powi(2);
    let mut term = ONE;
    let (mut even, mut odd) = (ONE, ZERO);
    let mut prev = f64::INFINITY;
    for k in 1..400u32 {
        let m = (2 * k - 1) as f64;
        let next = term * (-(mu - m * m) / (8.0 * k as f64)) * zi;
        let t = next.norm();
        if t >= prev {
            break;
        }
        term = next;
        if k % 2 == 0 {
            even += term;
        } else {
            odd += term;
        }
        if t < 1e-18 {
            break;
        }
        prev = t;
    }
    (even + odd, even - odd)
}

/// I_{ν+1}(z)/I_ν(z) from DLMF 10.40.5, keeping the subdominant e^{−z}
/// term: I_ν(z) ≈ e^z/sqrt(2πz)·[S₋(ν) + σ·i·(−1)^ν·e^{−2z}·S₊(ν)] with
/// σ = sign(Im z). Near the imaginary axis the two exponentials are of the
/// same size and both are needed; on the real axis (a Stokes line) the
/// subdominant term is dropped, where it is below e^{−2|z|} ≤ 2e-16.
fn hankel_i_ratio(nu: u32, z: C64) -> C64 {
    let zi = z.inv();
    let (a_minus, a_plus) = hankel_i_sums(nu, zi);
    let (b_minus, b_plus) = hankel_i_sums(nu + 1, zi);
    if z.im == 0.0 || z.re > 40.0 {
        return b_minus / a_minus;
    }
    let sigma = if z.im > 0.0 { 1.0 } else { -1.0 };
    let parity = if nu.is_multiple_of(2) { 1.0 } else { -1.0 };
    let e = (-2.0 * z).exp() * C64::new(0.0, sigma * parity);
    (b_minus - e * b_plus) / (a_minus + e * a_plus)
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

/// Shape function of a duct cross-section. `one_minus` = 1 − F is the
/// transverse average of the normalised velocity (or temperature
/// fluctuation) profile, which tends to 1 away from the walls; `f` is F. The
/// two are returned separately because each is tiny in one limit.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    pub f: C64,
    pub one_minus: C64,
}

/// |z| below which the slit uses its Taylor series.
const SLIT_SERIES: f64 = 0.3;

/// Taylor coefficients c_n of 1 − tanh(z)/z = Σ_{n≥1} c_n z^{2n},
/// c_n = −2^{2n+2}(2^{2n+2} − 1)·B_{2n+2}/(2n+2)! (Bernoulli numbers;
/// evaluated with mpmath). The ratio of successive terms tends to
/// −(2z/π)², so at |z| < 0.3 thirteen terms leave < 3e-19 relative.
const SLIT_COEFFS: [f64; 13] = [
    0.333_333_333_333_333_3,
    -0.133_333_333_333_333_33,
    0.053_968_253_968_253_97,
    -0.021_869_488_536_155_203,
    0.008_863_235_529_902_197,
    -0.003_592_128_036_572_481,
    0.001_455_834_387_051_318_3,
    -0.000_590_027_440_945_586,
    0.000_239_129_114_243_552_48,
    -9.691_537_956_929_451e-5,
    3.927_832_388_331_683e-5,
    -1.591_890_506_932_896_4e-5,
    6.451_689_215_655_431e-6,
];

/// Slit of half-gap h/2: F = tanh(z)/z with z = k·h/2.
///
/// Relative error of F and of 1 − F ≤ 1e-14 at z = sqrt(j)·x,
/// x in [1e-4, 1e4], and ≤ 2e-14 for 0 ≤ arg z ≤ 0.49π; the worst case is
/// 1 − F just above the series switch, where it costs a factor
/// |F/(1 − F)| ≈ 33 over the tanh rounding.
pub fn shape_slit(z: C64) -> Shape {
    if z.norm() < SLIT_SERIES {
        let z2 = z * z;
        let mut om = ZERO;
        for c in SLIT_COEFFS.iter().rev() {
            om = (om + *c) * z2;
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

/// Circular duct of radius a: F = 2 I1(z)/(z I0(z)) with z = k·a, and
/// 1 − F = I2(z)/I0(z).
///
/// Only one Bessel ratio is evaluated: with r = I2/I1 and the recurrence
/// I0 = I2 + (2/z)·I1, F = 2/(2 + z·r) and 1 − F = z·r/(2 + z·r), neither of
/// which cancels. Relative error ≤ 1e-14 for |z| in [1e-4, 1e4].
///
/// Note: with the e^{+jωt} convention and k = sqrt(jωρ/μ) the *modified*
/// Bessel functions are required; the J-form printed in spec Appendix D gives
/// negative resistance (see docs/spec-errata.md).
pub fn shape_circle(z: C64) -> Shape {
    let zr = z * bessel_i_ratio(1, z);
    let d = 2.0 + zr;
    Shape {
        f: 2.0 / d,
        one_minus: zr / d,
    }
}

/// Rectangular duct with sides `a` and `b` (full lengths), for the complex
/// wavenumber `k` (k_v for the viscous function, k_t for the thermal one).
///
/// Stinson (1991), JASA 89(2), 550–558, works the rectangle as his example
/// of an arbitrary section, and Stinson & Champoux (1992), JASA 91(2),
/// 685–695, use the same series for the thermal function. Expanding the
/// uniform forcing (pressure gradient, or heat input) of
/// (∇² − k²)v = const in the sine modes of the section, with v = 0 on the
/// walls, gives the double series
/// 1 − F = Σ_{m,n odd} 64/(π⁴m²n²) · k²/(k² + α_m² + β_n²),
/// α_m = mπ/a, β_n = nπ/b. Summing over n in closed form (the partial-
/// fraction series of tanh) leaves, with κ_m² = k² + α_m²,
/// 1 − F = Σ_{m odd} 8/(π²m²) · (k²/κ_m²) · [1 − tanh(κ_m b/2)/(κ_m b/2)]
///       = [1 − T(ka/2)] − (2/b)·(G − E),
/// where T(z) = tanh(z)/z is the slit function of the short side a,
/// G = Σ 8/(π²m²)·k²/κ_m³ and E = Σ 8/(π²m²)·(k²/κ_m³)·(1 − tanh(κ_m b/2)).
/// E converges like e^{−mπb/a}. G is summed directly up to m ≈ 3|ka|/π and
/// its tail by the binomial series in (ka/mπ)² with Hurwitz zeta tails; for
/// Re(ka) > 42 the Mellin transform of G gives the closed form
/// G = (1/k)(1 − 8/(πka)) up to terms of order e^{−ka}, which is the
/// boundary-layer result F = P/(kA) − 16/(πk²A) (perimeter P, area A).
///
/// Relative error ≤ 1e-13 against mpmath (tests/data/thermo_rect.json).
pub fn shape_rect(k: C64, a: f64, b: f64) -> Shape {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    if k == ZERO {
        return Shape {
            f: ONE,
            one_minus: ZERO,
        };
    }
    let slit = shape_slit(k * (0.5 * a));
    let c = (rect_g(k, a) - rect_e(k, a, b)) * (2.0 / b);
    Shape {
        f: slit.f + c,
        one_minus: slit.one_minus - c,
    }
}

/// B_{2p}/(2p)! for p = 1..=9 (Bernoulli numbers), for Euler–Maclaurin.
const BERNOULLI_RATIOS: [f64; 9] = [
    0.083_333_333_333_333_33,
    -0.001_388_888_888_888_889,
    3.306_878_306_878_307e-5,
    -8.267_195_767_195_768e-7,
    2.087_675_698_786_81e-8,
    -5.284_190_138_687_493e-10,
    1.338_253_653_068_467_9e-11,
    -3.389_680_296_322_582_7e-13,
    8.586_062_056_277_845e-15,
];

/// First odd m at which the rectangular-duct sums switch from direct
/// summation to Euler–Maclaurin tails.
const RECT_TAIL_START: f64 = 33.0;

/// Number of terms kept in the binomial tail series of [`rect_g`]; with
/// |ρ|/m² ≤ 1/9 the terms fall like 9^{−j}·sqrt(j), below 1e-18 by j = 24.
const RECT_TAIL_TERMS: usize = 30;

/// m^s·Σ_{i≥0} (m + 2i)^{−s} = m^s·2^{−s}·ζ(s, m/2) (Hurwitz zeta) for
/// m ≥ 33, by Euler–Maclaurin with q = m/2:
/// ζ(s, q)·q^s ≈ q/(s−1) + 1/2 + Σ_p B_{2p}/(2p)!·s(s+1)…(s+2p−2)·q^{1−2p}.
/// With q ≥ 16.5 and nine Bernoulli terms the relative error is below 1e-17
/// for s = 5 and grows with s, where the binomial weights 9^{−j} suppress it.
fn odd_power_tail_scaled(s: f64, m: f64) -> f64 {
    let q = 0.5 * m;
    let mut sum = q / (s - 1.0) + 0.5;
    let mut rising = s; // s(s+1)…(s+2p−2)
    let mut qp = 1.0 / q; // q^{1−2p}
    for (p, b) in BERNOULLI_RATIOS.iter().enumerate() {
        let t = b * rising * qp;
        sum += t;
        if t.abs() < 1e-18 * sum {
            break;
        }
        let n = 2.0 * (p + 1) as f64;
        rising *= (s + n - 1.0) * (s + n);
        qp /= q * q;
    }
    sum
}

/// λ(5 + 2j) = Σ_{m odd} m^{−5−2j}, j = 0..RECT_TAIL_TERMS, computed once.
fn odd_zeta_table() -> &'static [f64; RECT_TAIL_TERMS] {
    static TABLE: OnceLock<[f64; RECT_TAIL_TERMS]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0.0; RECT_TAIL_TERMS];
        for (j, v) in t.iter_mut().enumerate() {
            let s = 5.0 + 2.0 * j as f64;
            // Direct terms from the largest down, then the tail.
            let mut sum = RECT_TAIL_START.powf(-s) * odd_power_tail_scaled(s, RECT_TAIL_START);
            let mut m = RECT_TAIL_START - 2.0;
            while m >= 1.0 {
                sum += m.powf(-s);
                m -= 2.0;
            }
            *v = sum;
        }
        t
    })
}

/// G(k, a) = Σ_{m odd} 8/(π²m²)·k²/κ_m³ of [`shape_rect`].
///
/// With ρ = (ka/π)² and α_m = mπ/a, each term expands as
/// k²/(m²κ_m³) = k²(a/π)³ Σ_j binom(−3/2, j)·ρ^j·m^{−5−2j} for m² > |ρ|.
/// * |ρ| ≤ 1/9: that series from m = 1, with the tabulated λ(5 + 2j).
/// * otherwise: direct terms for odd m < M, M = max(33, 3|ka|/π), and the
///   series for the tail with Euler–Maclaurin sums Σ_{m odd ≥ M} m^{−s}.
/// * Re(ka) > 42: the closed form (1/k)(1 − 8/(πka)); the neglected terms
///   are of order e^{−Re ka} < 1e-18.
fn rect_g(k: C64, a: f64) -> C64 {
    let ka = k * a;
    if ka.re > 42.0 {
        return (ONE - 8.0 / (PI * ka)) / k;
    }
    let k2 = k * k;
    let rho = (ka / PI) * (ka / PI);
    let a_pi = a / PI;
    let scale = k2 * (a_pi * a_pi * a_pi * 8.0 / (PI * PI));
    let binomial = |weight: &dyn Fn(usize) -> f64| {
        let mut sum = ZERO;
        let mut coef = ONE; // binom(−3/2, j)·ρ^j
        for j in 0..RECT_TAIL_TERMS {
            let t = coef * weight(j);
            sum += t;
            if t.norm() < 1e-18 * sum.norm() {
                break;
            }
            let jf = j as f64;
            coef *= rho * ((-1.5 - jf) / (jf + 1.0));
        }
        sum
    };
    if rho.norm() <= 1.0 / 9.0 {
        let table = odd_zeta_table();
        return scale * binomial(&|j| table[j]);
    }
    let alpha = PI / a;
    // Direct terms for odd m < mm, with mm ≥ 3|ka|/π so that the tail's
    // binomial series converges at least as fast as 9^{−j}.
    let mut mm = (3.0 * ka.norm() / PI).ceil().max(RECT_TAIL_START);
    if mm % 2.0 == 0.0 {
        mm += 1.0;
    }
    let mut sum = ZERO;
    let mut m = 1.0;
    while m < mm {
        let am = m * alpha;
        let kappa2 = k2 + am * am;
        sum += k2 / (kappa2 * kappa2.sqrt() * (m * m));
        m += 2.0;
    }
    let m5 = mm.powi(-5);
    let inv_m2 = 1.0 / (mm * mm);
    let tail = binomial(&|j| {
        let s = 5.0 + 2.0 * j as f64;
        m5 * inv_m2.powi(j as i32) * odd_power_tail_scaled(s, mm)
    });
    sum * (8.0 / (PI * PI)) + scale * tail
}

/// E(k, a, b) = Σ_{m odd} 8/(π²m²)·(k²/κ_m³)·(1 − tanh(κ_m b/2)) of
/// [`shape_rect`]; Re κ_m ≥ mπ/a, so the terms fall at least like e^{−mπb/a}.
fn rect_e(k: C64, a: f64, b: f64) -> C64 {
    let k2 = k * k;
    let alpha = PI / a;
    let mut sum = ZERO;
    let mut m = 1.0;
    loop {
        let am = m * alpha;
        let kappa2 = k2 + am * am;
        let kappa = kappa2.sqrt();
        let e = (-kappa * b).exp();
        if e.norm() < 1e-18 {
            break;
        }
        // 1 − tanh(y) = 2e^{−2y}/(1 + e^{−2y}) with y = κb/2.
        sum += k2 / (kappa2 * kappa * (m * m)) * (2.0 * e / (1.0 + e));
        m += 2.0;
    }
    sum * (8.0 / (PI * PI))
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

/// Argument at and above which J1, Y1 and H1 use the Hankel expansion.
/// At x = 25 its smallest term is below 1e-22 (DLMF 10.17.3).
const BESSEL_ASYMPTOTIC: f64 = 25.0;

/// 3π/4 as an unevaluated sum of two doubles (hi + lo).
const THREE_PI_4_HI: f64 = 2.356_194_490_192_345;
const THREE_PI_4_LO: f64 = 9.184_850_993_605_148e-17;

/// (J1(x), Y1(x)) for x ≥ [`BESSEL_ASYMPTOTIC`] from the Hankel expansion
/// J1 = sqrt(2/(πx))·(P cos χ − Q sin χ), Y1 = sqrt(2/(πx))·(P sin χ + Q cos χ),
/// χ = x − 3π/4 (DLMF 10.17.3–4), in modulus–phase form M·cos(χ + φ),
/// M·sin(χ + φ) with φ = atan2(Q, P). The phase is carried in double-double
/// arithmetic, so both functions keep their relative accuracy near their
/// zeros.
fn hankel_j1_y1(x: f64) -> (f64, f64) {
    let (mut p, mut q) = (1.0, 0.0);
    let mut t = 1.0;
    let mut prev = f64::INFINITY;
    for k in 1..200u32 {
        let m = (2 * k - 1) as f64;
        let next = t * (4.0 - m * m) / (8.0 * k as f64 * x);
        if next.abs() >= prev {
            break;
        }
        t = next;
        // t = a_k(1)/x^k; P takes even k with sign (−1)^{k/2}, Q odd k with
        // sign (−1)^{(k−1)/2}.
        match k % 4 {
            0 => p += t,
            1 => q += t,
            2 => p -= t,
            _ => q -= t,
        }
        if t.abs() < 1e-18 {
            break;
        }
        prev = t.abs();
    }
    // χ + φ = (x − hi) − lo + φ, with x − hi split exactly (TwoSum).
    let s = x - THREE_PI_4_HI;
    let bb = s - x;
    let err = (x - (s - bb)) + (-THREE_PI_4_HI - bb);
    let lo = err - THREE_PI_4_LO + q.atan2(p);
    let th = s + lo;
    let tl = (s - th) + lo;
    let (sn, cs) = th.sin_cos();
    let amp = (FRAC_2_PI / x).sqrt() * p.hypot(q);
    (amp * (cs - tl * sn), amp * (sn + tl * cs))
}

/// Bessel function J1(x) of real argument (odd in x).
///
/// Series for x < 2, Gauss–Legendre on Bessel's integral for 2 ≤ x < 25,
/// Hankel expansion in modulus–phase form above. Against mpmath on [0, 500]:
/// relative error ≤ 1e-13 wherever the condition number |x·J1'/J1| ≤ 1e3,
/// and everywhere |ΔJ1| ≤ 1e-12·|J1| + 8u·|x·J1'(x)| (u = 2^{−53}), i.e. a
/// few ulps of backward error, which is all a double-precision argument
/// allows near the zeros of J1 (tests/special_functions.rs). Above x = 25
/// the error stays below 1e-3 of that bound even 1e-9 from a zero.
pub fn bessel_j1(x: f64) -> f64 {
    if x < 0.0 {
        return -bessel_j1(-x);
    }
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
    if x >= BESSEL_ASYMPTOTIC {
        return hankel_j1_y1(x).0;
    }
    // J1(x) = (1/π) ∫_0^π cos(θ − x sin θ) dθ
    integrate(0.0, PI, 2 * panels_for(x), |t| (t - x * t.sin()).cos()) / PI
}

/// Struve function H1(x), real x ≥ 0.
///
/// Series for x < 2, Gauss–Legendre on (2x/π)∫_0^{π/2} cos²θ sin(x sin θ) dθ
/// for 2 ≤ x < 25, and H1 = Y1 + K1 above, with Y1 from the Hankel
/// expansion and K1(x) = H1 − Y1 = (2/π)∫_0^∞ e^{−u}·sqrt(1 + u²/x²) du
/// (DLMF 11.5.2). H1 is positive for x > 0; relative error ≤ 1e-14. H1 is
/// even, so a negative argument is reflected rather than fed to the series.
pub fn struve_h1(x: f64) -> f64 {
    if x < 0.0 {
        return struve_h1(-x);
    }
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
    if x >= BESSEL_ASYMPTOTIC {
        // e^{−u} over [0, 40] (the rest is < 5e-18 relative) on two panels:
        // 24-point Gauss–Legendre integrates e^{−u} over a width of 20 to
        // ~1e-26, and sqrt(1 + u²/x²) is analytic within |u| < x.
        let g = |u: f64| (-u).exp() * (1.0 + (u / x) * (u / x)).sqrt();
        let k1 = FRAC_2_PI * integrate(0.0, 40.0, 2, g);
        return hankel_j1_y1(x).1 + k1;
    }
    // H1(x) = (2x/π) ∫_0^{π/2} cos²θ sin(x sin θ) dθ
    2.0 * x / PI
        * integrate(0.0, FRAC_PI_2, panels_for(x), |t| {
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

// ----- Unflanged pipe (Levine & Schwinger 1948) ---------------------------

/// First zero of J1, j₁,₁. In a circular pipe of radius a the first
/// axisymmetric higher mode cuts on at ka = j₁,₁.
pub const J1_FIRST_ZERO: f64 = 3.831_705_970_207_512_5;

/// Largest ka at which [`unflanged_pipe`] evaluates the Levine–Schwinger
/// integrals; above it the result is a continuation (see there).
pub const LEVINE_SCHWINGER_KA_MAX: f64 = 3.8;

/// Radiation of the plane mode from an unflanged, thin-walled, semi-infinite
/// circular pipe (Levine & Schwinger, Phys. Rev. 73, 383–406, 1948).
///
/// Returns (A, L/a), where the reflection coefficient at the open end is
/// R = −e^{−A}·e^{−2jkL} (e^{+jωt}), so |R| = e^{−A}, L is the end
/// correction, and the normalised radiation impedance is
/// Z·S/(ρc) = (1 + R)/(1 − R) = tanh(A/2 + j·ka·L/a).
///
/// With θ(x) = arg(−Y1(x) + jJ1(x)) (the continuous branch of
/// arctan(−J1/Y1)), for ka < j₁,₁:
///   A   = (2ka/π) ∫_0^{ka} θ(x) dx / (x·sqrt(k²a² − x²)),
///   L/a = (1/π) ∫_0^{ka} ln[πJ1(x)·sqrt(J1² + Y1²)] dx / (x·sqrt(k²a² − x²))
///       + (1/π) ∫_0^∞ ln[1/(2 I1(x) K1(x))] dx / (x·sqrt(x² + k²a²)).
/// The first two integrals use x = ka·sin φ and composite Gauss–Legendre; the
/// last is a trapezoid rule in ln x on nodes precomputed once. Agreement
/// with an mpmath evaluation is better than 1e-12 (tests/data/
/// special_unflanged.json). The ka → 0 limits are A = (ka)²/2 and
/// L/a = 0.6127010…; Levine & Schwinger printed 0.6133, but their integral
/// evaluates to 0.61270 (tools/refgen/radiation_refs.py; the value 0.6127
/// is also reported in AIP Conf. Proc. 2195, 020034, 2019).
///
/// Above [`LEVINE_SCHWINGER_KA_MAX`] = 3.8 the first higher mode is about to
/// propagate and the plane-wave result stops being a model of the pipe. The
/// function then continues with the (2,6) Padé fits of Silva, Guillemain,
/// Kergomard, Mallaroni & Norris, J. Sound Vib. 322, 255–263 (2009), Eqs.
/// (21)–(22) with the unflanged coefficients of their Table 1, scaled to be
/// continuous at 3.8. This keeps the element passive and smooth for
/// validity shading to flag; it is not an accurate model there.
pub fn unflanged_pipe(ka: f64) -> (f64, f64) {
    if ka <= 0.0 {
        return (0.0, lr_tail(0.0));
    }
    if ka <= LEVINE_SCHWINGER_KA_MAX {
        return levine_schwinger(ka);
    }
    static EDGE: OnceLock<(f64, f64, f64, f64)> = OnceLock::new();
    let &(a0, l0, r_s0, l_s0) = EDGE.get_or_init(|| {
        let (a0, l0) = levine_schwinger(LEVINE_SCHWINGER_KA_MAX);
        let (r_s0, l_s0) = silva_unflanged(LEVINE_SCHWINGER_KA_MAX);
        (a0, l0, r_s0, l_s0)
    });
    let (r_s, l_s) = silva_unflanged(ka);
    (a0 - (r_s / r_s0).ln(), l0 * l_s / l_s0)
}

/// Silva et al. (2009) Eqs. (21)–(22), unflanged column of Table 1:
/// |R| = (1 + a1 x²)/(1 + (β + a1)x² + a2 x⁴ + a3 x⁶),
/// L/a = η(1 + b1 x²)/(1 + b2 x² + b3 x⁴ + b4 x⁶), x = ka. The authors
/// state errors below 2 % for ka < 3 against Levine–Schwinger.
fn silva_unflanged(ka: f64) -> (f64, f64) {
    const BETA: f64 = 0.5;
    const ETA: f64 = 0.6133;
    const A: [f64; 3] = [0.800, 0.266, 0.0263];
    const B: [f64; 4] = [0.0599, 0.238, -0.0153, 0.00150];
    let x2 = ka * ka;
    let r = (1.0 + A[0] * x2) / (1.0 + x2 * ((BETA + A[0]) + x2 * (A[1] + x2 * A[2])));
    let l = ETA * (1.0 + B[0] * x2) / (1.0 + x2 * (B[1] + x2 * (B[2] + x2 * B[3])));
    (r, l)
}

/// The Levine–Schwinger integrals for 0 < ka ≤ 3.8.
///
/// Near φ = 0 the logarithm in Y1 makes the integrands behave like φ ln φ
/// (end correction) and φ³ ln φ (attenuation), on which Gauss–Legendre
/// converges only algebraically (errors of 1e-6 and 1e-11 with fixed
/// panels). On [0, π/4] the substitution φ = (π/4)·u⁴ turns them into
/// u⁷ ln u and u¹⁵ ln u. The two remaining panels are graded towards
/// φ = π/2, where ln J1(ka·sin φ) approaches the zero of J1 as ka → j₁,₁.
/// Measured against mpmath: ≤ 2e-14 for 0 < ka ≤ 3.8.
fn levine_schwinger(ka: f64) -> (f64, f64) {
    let (xg, wg) = gauss_legendre();
    let (mut a_int, mut l_int) = (0.0, 0.0);
    let mut add = |phi: f64, weight: f64| {
        let s = phi.sin();
        let x = ka * s;
        let (j1, y1, log_kernel) = j1_y1_small(x);
        a_int += weight * j1.atan2(-y1) / s;
        l_int += weight * log_kernel / x;
    };
    for (xi, wi) in xg.iter().zip(wg) {
        let u = 0.5 * (1.0 + xi);
        let u3 = u * u * u;
        // φ = (π/4)u⁴, dφ = π·u³ du, du = dξ/2.
        add(FRAC_PI_4 * u3 * u, wi * FRAC_PI_2 * u3);
    }
    for (lo, hi) in [(FRAC_PI_4, 3.0 * PI / 8.0), (3.0 * PI / 8.0, FRAC_PI_2)] {
        let (mid, half) = (0.5 * (lo + hi), 0.5 * (hi - lo));
        for (xi, wi) in xg.iter().zip(wg) {
            add(mid + half * xi, wi * half);
        }
    }
    (FRAC_2_PI * a_int, l_int / PI + lr_tail(ka))
}

/// (J1(x), Y1(x), ln[π·J1(x)·sqrt(J1(x)² + Y1(x)²)]) for 0 < x ≤ 4 from
/// the power series (DLMF 10.2.2, 10.8.1). All terms stay below ~2 in size,
/// so the absolute error is a few ulps. For x < 2 the logarithm is taken
/// as ln1p of π·J1·(−Y1) − 1, which the series give without cancellation.
fn j1_y1_small(x: f64) -> (f64, f64, f64) {
    let h = 0.25 * x * x;
    // term_k = (−h)^k/(k!(k+1)!), psi = ψ(k+1) + ψ(k+2)
    let mut term = 1.0;
    let (mut s1, mut sy) = (0.0, 1.0 - 2.0 * EULER_GAMMA);
    let mut psi1 = -EULER_GAMMA;
    for k in 1..60 {
        let kf = k as f64;
        term *= -h / (kf * (kf + 1.0));
        psi1 += 1.0 / kf; // ψ(k+1)
        let psi2 = psi1 + 1.0 / (kf + 1.0); // ψ(k+2)
        s1 += term;
        sy += (psi1 + psi2) * term;
        if term.abs() < 1e-18 {
            break;
        }
    }
    let lg = (0.5 * x).ln();
    let j1 = 0.5 * x * (1.0 + s1);
    // −π·Y1 = 2/x − 2·J1·ln(x/2) + (x/2)·Σ(ψ(k+1)+ψ(k+2))(−h)^k/(k!(k+1)!)
    let y1 = -(2.0 / x - 2.0 * j1 * lg + 0.5 * x * sy) / PI;
    let log_kernel = if x < 2.0 {
        // π·J1·(−Y1) − 1 = s1 − 2·J1²·ln(x/2) + (x/2)·J1·Σ(...)
        let w = s1 - 2.0 * j1 * j1 * lg + 0.5 * x * j1 * sy;
        w.ln_1p() + 0.5 * ((j1 / y1) * (j1 / y1)).ln_1p()
    } else {
        (PI * j1 * j1.hypot(y1)).ln()
    };
    (j1, y1, log_kernel)
}

/// Step of the trapezoid rule in t = ln x for the I1·K1 integral. The
/// integrand is analytic for |Im t| < π/2, so the error is ~e^{−π²/h}
/// ≈ 7e-18.
const LR_STEP: f64 = 0.25;

/// (x_i, ln[1/(2 I1(x_i) K1(x_i))]) at x_i = e^{i·h} for t = i·h in
/// [−36, 45]; outside, the integrand is below 1e-15 (x² ln x at the lower
/// end, ln x/x at the upper).
fn lr_nodes() -> &'static Vec<(f64, f64)> {
    static NODES: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    NODES.get_or_init(|| {
        let (lo, hi) = ((-36.0 / LR_STEP) as i32, (45.0 / LR_STEP) as i32);
        (lo..=hi)
            .map(|i| {
                let x = (i as f64 * LR_STEP).exp();
                (x, neg_log_2i1k1(x))
            })
            .collect()
    })
}

/// (1/π) ∫_0^∞ ln[1/(2 I1(x) K1(x))] dx / (x·sqrt(x² + k²a²)).
fn lr_tail(ka: f64) -> f64 {
    let k2 = ka * ka;
    let s: f64 = lr_nodes()
        .iter()
        .map(|&(x, v)| v / (x * x + k2).sqrt())
        .sum();
    s * LR_STEP / PI
}

/// −ln(2·I1(x)·K1(x)) for x > 0 (it tends to 0 as x → 0 and to ln x as
/// x → ∞).
///
/// * x < 2: from the series of I1 and K1 (DLMF 10.25.2, 10.31.1),
///   2I1K1 − 1 = s + 2I1²·ln(x/2) − (x/2)·I1·Σ(ψ(k+1)+ψ(k+2))h^k/(k!(k+1)!)
///   with h = x²/4 and s = 2I1/x − 1, so the small logarithm keeps its
///   relative accuracy.
/// * 2 ≤ x < 20: I1 by its series (positive terms) and e^x·K1(x) =
///   ∫_0^∞ e^{−x(cosh t − 1)} cosh t dt by the trapezoid rule, whose error
///   is below e^{−90} for step 0.1.
/// * x ≥ 20: I1K1 ~ (1/(2x))·Σ_k (−1)^k (2k−1)!!/(2k)!!·Π_{j≤k}(4 − (2j−1)²)
///   /(2x)^{2k} (DLMF 10.40.6); checked against mpmath to 4e-19 at x = 20.
fn neg_log_2i1k1(x: f64) -> f64 {
    if x < 2.0 {
        let h = 0.25 * x * x;
        let mut term = 1.0;
        let (mut s1, mut sk) = (0.0, 1.0 - 2.0 * EULER_GAMMA);
        let mut psi1 = -EULER_GAMMA;
        for k in 1..60 {
            let kf = k as f64;
            term *= h / (kf * (kf + 1.0));
            psi1 += 1.0 / kf;
            let psi2 = psi1 + 1.0 / (kf + 1.0);
            s1 += term;
            sk += (psi1 + psi2) * term;
            if term < 1e-18 * s1 {
                break;
            }
        }
        let i1 = 0.5 * x * (1.0 + s1);
        let w = s1 + 2.0 * i1 * i1 * (0.5 * x).ln() - 0.5 * x * i1 * sk;
        return -w.ln_1p();
    }
    if x < 20.0 {
        // e^{−x}·I1(x) = e^{−x} Σ (x/2)^{2k+1}/(k!(k+1)!)
        let h = 0.25 * x * x;
        let mut term = 0.5 * x;
        let mut i1 = term;
        for k in 1..200 {
            let kf = k as f64;
            term *= h / (kf * (kf + 1.0));
            i1 += term;
            if term < 1e-18 * i1 {
                break;
            }
        }
        let step = 0.1;
        let mut k1s = 0.5; // f(0)/2, f(0) = 1
        for n in 1..2000 {
            let t = n as f64 * step;
            let f = (-x * (t.cosh() - 1.0)).exp() * t.cosh();
            k1s += f;
            if f < 1e-18 * k1s {
                break;
            }
        }
        k1s *= step;
        return -(2.0 * i1 * (-x).exp() * k1s).ln();
    }
    let x2 = 4.0 * x * x;
    let mut t = 1.0;
    let mut s = 1.0;
    let mut prev = f64::INFINITY;
    for k in 1..200 {
        let m = (2 * k - 1) as f64;
        let next = -t * m / (2.0 * k as f64) * (4.0 - m * m) / x2;
        if next.abs() >= prev {
            break;
        }
        t = next;
        s += t;
        if t.abs() < 1e-18 {
            break;
        }
        prev = t.abs();
    }
    x.ln() - s.ln()
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

    /// The largest double below `x` (x > 0).
    fn below(x: f64) -> f64 {
        f64::from_bits(x.to_bits() - 1)
    }

    // Continuity across each switch between algorithms (spec Section 6 asks
    // for one across the asymptotic switch). The two branches are evaluated
    // at adjacent doubles, where the function itself moves by ~1e-16
    // relative, so any gap is the branches' disagreement.

    #[test]
    fn slit_shape_continuous_across_series_switch() {
        for theta in [0.0, FRAC_PI_4, 0.49 * PI] {
            let lo = shape_slit(C64::from_polar(below(SLIT_SERIES), theta));
            let hi = shape_slit(C64::from_polar(SLIT_SERIES, theta));
            let gap = (lo.one_minus - hi.one_minus).norm() / hi.one_minus.norm();
            assert!(gap < 2e-14, "theta = {theta}: {gap:e}");
            assert!((lo.f - hi.f).norm() < 1e-15);
        }
    }

    #[test]
    fn bessel_ratio_continuous_across_asymptotic_switch() {
        for theta in [0.0, FRAC_PI_4, 0.49 * PI] {
            for nu in 0..3 {
                let a = bessel_i_ratio(nu, C64::from_polar(below(ASYMPTOTIC_SWITCH), theta));
                let b = bessel_i_ratio(nu, C64::from_polar(ASYMPTOTIC_SWITCH, theta));
                assert!((a - b).norm() < 1e-14 * b.norm(), "nu={nu}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn j1_and_h1_continuous_across_switches() {
        for x0 in [2.0, BESSEL_ASYMPTOTIC] {
            let (a, b) = (below(x0), x0);
            assert!((bessel_j1(a) - bessel_j1(b)).abs() < 1e-15);
            assert!((struve_h1(a) - struve_h1(b)).abs() < 1e-15);
        }
    }

    #[test]
    fn j1_and_h1_reference_values() {
        // Reference values (Abramowitz & Stegun / mpmath).
        assert!((bessel_j1(1.0) - 0.440_050_585_744_933_5).abs() < 1e-13);
        assert!((bessel_j1(10.0) - 0.043_472_746_168_861_44).abs() < 1e-13);
        assert!((struve_h1(1.0) - 0.198_457_336_201_944_3).abs() < 1e-13);
        assert!((struve_h1(10.0) - 0.891_832_492_094_538).abs() < 1e-12);
        // J1 is odd and H1 even, including past the series ranges.
        for x in [0.5, 10.0, 30.0] {
            assert_eq!(bessel_j1(-x), -bessel_j1(x));
            assert_eq!(struve_h1(-x), struve_h1(x));
        }
    }

    #[test]
    fn i1k1_branches_agree_at_switches() {
        for x0 in [2.0, 20.0] {
            let (a, b) = (neg_log_2i1k1(below(x0)), neg_log_2i1k1(x0));
            assert!((a - b).abs() < 1e-14 * b.abs(), "{x0}: {a} vs {b}");
        }
    }
}
