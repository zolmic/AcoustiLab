//! Shelving filters as magnitude responses.
//!
//! These are the analog prototypes of the shelving filters in R. Bristow-
//! Johnson's "Audio EQ Cookbook" (the digital RBJ biquads are their bilinear
//! transforms, so they agree away from the Nyquist frequency). With
//! `s = j·f/f_c` and `A = 10^(G/40)`:
//!
//! * low shelf:  `H(s) = A·(s² + (√A/Q)·s + A) / (A·s² + (√A/Q)·s + 1)`
//! * high shelf: `H(s) = A·(A·s² + (√A/Q)·s + 1) / (s² + (√A/Q)·s + A)`
//!
//! The low shelf tends to G dB at DC and 0 dB at high frequency; the high
//! shelf the reverse. At `f = f_c` both are exactly G/2 dB, whatever Q. The
//! cookbook's shelf slope S relates to Q by `1/Q = √((A + 1/A)(1/S − 1) + 2)`,
//! so S = 1 is Q = 1/√2 for every gain: the steepest shelf that stays
//! monotonic.

use crate::C64;

fn shelf(f: f64, fc: f64, gain_db: f64, q: f64, high: bool) -> f64 {
    if gain_db == 0.0 {
        return 0.0;
    }
    let a = 10f64.powf(gain_db / 40.0);
    let s = C64::new(0.0, f / fc);
    let mid = s * (a.sqrt() / q);
    let h = if high {
        a * (a * s * s + mid + 1.0) / (s * s + mid + a)
    } else {
        a * (s * s + mid + a) / (a * s * s + mid + 1.0)
    };
    20.0 * h.norm().log10()
}

/// Low-shelf magnitude in dB at `f` (corner `fc`, gain `gain_db`, quality `q`).
pub fn low_shelf_db(f: f64, fc: f64, gain_db: f64, q: f64) -> f64 {
    shelf(f, fc, gain_db, q, false)
}

/// High-shelf magnitude in dB at `f` (corner `fc`, gain `gain_db`, quality `q`).
pub fn high_shelf_db(f: f64, fc: f64, gain_db: f64, q: f64) -> f64 {
    shelf(f, fc, gain_db, q, true)
}
