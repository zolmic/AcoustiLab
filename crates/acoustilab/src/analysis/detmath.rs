//! Deterministic elementary functions for sampling.
//!
//! Monte Carlo samples must be bit-identical on every platform (native and
//! wasm32), so the functions that turn uniform deviates into parameter
//! values use only IEEE-754 basic operations (+, −, ×, ÷, sqrt, round),
//! which are correctly rounded everywhere. The platform `ln` and `exp` come
//! from different libm implementations and may differ in the last bit.
//!
//! * [`ln`]: x = m·2^e with m in [√½, √2), ln m = 2·atanh(s), s = (m−1)/(m+1),
//!   |s| ≤ 0.1716, by its odd series to s²³ (truncation below 1e-18) in the
//!   arrangement of fdlibm's `__ieee754_log` (the exact term m − 1 kept
//!   apart), and e·ln 2 split into a 32-bit head and a tail. Within 2 ulp of
//!   the correctly rounded value (tested against the platform `ln`).
//! * [`exp`]: x = k·ln 2 + r, |r| ≤ ln2/2, Taylor series of e^r to r¹³
//!   (truncation 4e-18), scaled by 2^k exactly. Within 2 ulp.
//! * [`norm_quantile`]: Wichura's algorithm AS 241 (PPND16), "The percentage
//!   points of the normal distribution", Applied Statistics 37(3), 477–484
//!   (1988); relative accuracy about 1e-16. Coefficients as published
//!   (StatLib apstat/241).

use std::f64::consts::FRAC_1_SQRT_2;

/// ln 2 split as in fdlibm: a head with 32 significant bits, so that k·LN2_HI
/// is exact for |k| < 2^21, and the tail (LN2_HI + LN2_LO = ln 2 to 1e-26).
const LN2_HI: f64 = f64::from_bits(0x3FE6_2E42_FEE0_0000); // 6.93147180369123816490e-1
const LN2_LO: f64 = f64::from_bits(0x3DEA_39EF_3579_3C76); // 1.90821492927058770002e-10
const LOG2_E: f64 = std::f64::consts::LOG2_E;

/// Splits a finite positive x into (m, e) with x = m·2^e and m in [0.5, 1).
fn frexp(x: f64) -> (f64, i32) {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        // Subnormal: scale into the normal range first (exact).
        let (m, e) = frexp(x * 18_014_398_509_481_984.0); // 2^54
        return (m, e - 54);
    }
    let m = f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52));
    (m, exp - 1022)
}

/// 2^k for integer k, exact (including the subnormal range).
fn pow2(k: i32) -> f64 {
    if k > 1023 {
        f64::INFINITY
    } else if k >= -1022 {
        f64::from_bits(((k + 1023) as u64) << 52)
    } else if k >= -1074 {
        f64::from_bits(1u64 << (k + 1074))
    } else {
        0.0
    }
}

/// Natural logarithm (see the module documentation).
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    let (mut m, mut e) = frexp(x);
    if m < FRAC_1_SQRT_2 {
        m *= 2.0;
        e -= 1;
    }
    // f = m − 1 is exact (Sterbenz). With s = f/(2 + f), ln(1 + f) =
    // 2·atanh(s) = f − (f²/2 − s·(f²/2 + R)), R = 2·Σ_{k≥1} s^(2k)/(2k+1),
    // so the leading term f carries no rounding error.
    let f = m - 1.0;
    let s = f / (2.0 + f);
    let s2 = s * s;
    const C: [f64; 11] = [
        2.0 / 3.0,
        2.0 / 5.0,
        2.0 / 7.0,
        2.0 / 9.0,
        2.0 / 11.0,
        2.0 / 13.0,
        2.0 / 15.0,
        2.0 / 17.0,
        2.0 / 19.0,
        2.0 / 21.0,
        2.0 / 23.0,
    ];
    let mut r = 0.0;
    for c in C.iter().rev() {
        r = r * s2 + c;
    }
    r *= s2;
    let hfsq = 0.5 * f * f;
    let ef = e as f64;
    ef * LN2_HI - ((hfsq - (s * (hfsq + r) + ef * LN2_LO)) - f)
}

/// Exponential (see the module documentation).
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x > 709.782_712_893_384 {
        return f64::INFINITY;
    }
    if x < -745.133_219_101_941_2 {
        return 0.0;
    }
    let k = (x * LOG2_E).round();
    let r = (x - k * LN2_HI) - k * LN2_LO;
    // e^r = Σ r^n/n!, n = 0..13.
    const C: [f64; 14] = [
        1.0,
        1.0,
        1.0 / 2.0,
        1.0 / 6.0,
        1.0 / 24.0,
        1.0 / 120.0,
        1.0 / 720.0,
        1.0 / 5_040.0,
        1.0 / 40_320.0,
        1.0 / 362_880.0,
        1.0 / 3_628_800.0,
        1.0 / 39_916_800.0,
        1.0 / 479_001_600.0,
        1.0 / 6_227_020_800.0,
    ];
    let mut p = 0.0;
    for c in C.iter().rev() {
        p = p * r + c;
    }
    let k = k as i32;
    // Two exact scalings keep 2^k representable at both ends of the range.
    let half = k / 2;
    p * pow2(half) * pow2(k - half)
}

// AS 241 PPND16 coefficients, lowest degree first, with the digits as
// published (the paper gives 20 significant digits and hash sums to check
// the transcription; f64 keeps the nearest double).
#[allow(clippy::excessive_precision)]
const A: [f64; 8] = [
    3.387_132_872_796_366_608_0,
    1.331_416_678_917_843_774_5e2,
    1.971_590_950_306_551_442_7e3,
    1.373_169_376_550_946_112_5e4,
    4.592_195_393_154_987_145_7e4,
    6.726_577_092_700_870_085_3e4,
    3.343_057_558_358_812_810_5e4,
    2.509_080_928_730_122_672_7e3,
];
#[allow(clippy::excessive_precision)]
const B: [f64; 8] = [
    1.0,
    4.231_333_070_160_091_125_2e1,
    6.871_870_074_920_579_083_0e2,
    5.394_196_021_424_751_107_7e3,
    2.121_379_430_158_659_586_7e4,
    3.930_789_580_009_271_061_0e4,
    2.872_908_573_572_194_267_4e4,
    5.226_495_278_852_854_561_0e3,
];
#[allow(clippy::excessive_precision)]
const C: [f64; 8] = [
    1.423_437_110_749_683_577_34,
    4.630_337_846_156_545_295_90,
    5.769_497_221_460_691_405_50,
    3.647_848_324_763_204_605_04,
    1.270_458_252_452_368_382_58,
    2.417_807_251_774_506_117_70e-1,
    2.272_384_498_926_918_458_33e-2,
    7.745_450_142_783_414_076_40e-4,
];
#[allow(clippy::excessive_precision)]
const D: [f64; 8] = [
    1.0,
    2.053_191_626_637_758_821_87,
    1.676_384_830_183_803_849_40,
    6.897_673_349_851_000_045_50e-1,
    1.481_039_764_274_800_745_90e-1,
    1.519_866_656_361_645_719_66e-2,
    5.475_938_084_995_344_946_00e-4,
    1.050_750_071_644_416_843_24e-9,
];
#[allow(clippy::excessive_precision)]
const E: [f64; 8] = [
    6.657_904_643_501_103_777_20,
    5.463_784_911_164_114_369_90,
    1.784_826_539_917_291_335_80,
    2.965_605_718_285_048_912_30e-1,
    2.653_218_952_657_612_309_30e-2,
    1.242_660_947_388_078_438_60e-3,
    2.711_555_568_743_487_578_15e-5,
    2.010_334_399_292_288_132_65e-7,
];
#[allow(clippy::excessive_precision)]
const F: [f64; 8] = [
    1.0,
    5.998_322_065_558_879_376_90e-1,
    1.369_298_809_227_358_053_10e-1,
    1.487_536_129_085_061_485_25e-2,
    7.868_691_311_456_132_591_00e-4,
    1.846_318_317_510_054_681_80e-5,
    1.421_511_758_316_445_888_70e-7,
    2.044_263_103_389_939_785_64e-15,
];

/// Σ c_k·x^k by Horner's rule (the nesting of the published algorithm).
fn poly(c: &[f64; 8], x: f64) -> f64 {
    c.iter().rev().fold(0.0, |acc, &k| acc * x + k)
}

/// Standard normal quantile Φ⁻¹(p) for p in (0, 1) (AS 241, PPND16).
/// Returns ∓∞ at 0 and 1 and NaN outside [0, 1].
pub fn norm_quantile(p: f64) -> f64 {
    if !(0.0..=1.0).contains(&p) || p.is_nan() {
        return f64::NAN;
    }
    if p == 0.0 {
        return f64::NEG_INFINITY;
    }
    if p == 1.0 {
        return f64::INFINITY;
    }
    let q = p - 0.5;
    if q.abs() <= 0.425 {
        let r = 0.180_625 - q * q;
        return q * poly(&A, r) / poly(&B, r);
    }
    let tail = if q < 0.0 { p } else { 1.0 - p };
    let r = (-ln(tail)).sqrt();
    let v = if r <= 5.0 {
        poly(&C, r - 1.6) / poly(&D, r - 1.6)
    } else {
        poly(&E, r - 5.0) / poly(&F, r - 5.0)
    };
    if q < 0.0 {
        -v
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distance in units in the last place.
    fn ulps(a: f64, b: f64) -> u64 {
        if a == b {
            return 0;
        }
        let (ia, ib) = (a.to_bits() as i64, b.to_bits() as i64);
        (ia - ib).unsigned_abs()
    }

    #[test]
    fn ln_and_exp_are_within_two_ulp_of_the_platform() {
        // A deterministic spread of arguments over many binades.
        let mut x = 1.234e-300;
        while x < 1e300 {
            for m in [1.0, 1.1, 1.414, 1.5, 1.99, 2.71] {
                let v = x * m;
                assert!(ulps(ln(v), v.ln()) <= 2, "ln({v:e})");
            }
            x *= 7.3;
        }
        for v in [
            1.0,
            1.0 + 1e-15,
            1.0 - 1e-15,
            0.999,
            1.001,
            5e-324,
            f64::MAX,
        ] {
            assert!(ulps(ln(v), v.ln()) <= 2, "ln({v:e})");
        }
        let mut y = -745.0;
        while y < 709.7 {
            assert!(ulps(exp(y), y.exp()) <= 2, "exp({y})");
            y += 0.3711;
        }
        for v in [0.0, 1e-300, -1e-17, 0.5, -0.5, 1.0, 709.78, -744.0] {
            assert!(ulps(exp(v), v.exp()) <= 2, "exp({v})");
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(ln(1.0), 0.0);
        assert!(ln(-1.0).is_nan() && ln(0.0) == f64::NEG_INFINITY);
        assert_eq!(exp(1000.0), f64::INFINITY);
        assert_eq!(exp(-1000.0), 0.0);
    }

    #[test]
    fn normal_quantile_symmetry_and_known_points() {
        // Φ⁻¹(0.975) = 1.959963984540054 (mpmath: sqrt(2)·erfinv(0.95));
        // the fixture test in tests/analysis.rs covers the whole range.
        assert!((norm_quantile(0.975) - 1.959_963_984_540_054).abs() < 1e-15);
        assert_eq!(norm_quantile(0.5), 0.0);
        // Exactly symmetric where 1 − p is exact.
        for p in [0.25, 0.125, 0.0625, 0.375, 0.03125, 0.4375, 1.0 / 1024.0] {
            assert_eq!(1.0 - (1.0 - p), p);
            assert_eq!(norm_quantile(p), -norm_quantile(1.0 - p));
        }
        assert!(norm_quantile(-0.1).is_nan() && norm_quantile(1.5).is_nan());
        assert_eq!(norm_quantile(0.0), f64::NEG_INFINITY);
    }
}
