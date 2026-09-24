//! Validity criteria: where each element's representation stops being
//! trustworthy (spec Section 2, with the single error metric chosen in
//! docs/conventions.md).
//!
//! Lumped cavities use the compliance error e(kL) = 1 − kL·cot(kL), the
//! worst case over positions in a closed duct driven at one end (3.0 % at
//! kL = 0.3, 8.5 % at 0.5, 35.8 % at 1.0). Lumped ducts use the inertance
//! error e(kl) = tan(kl)/kl − 1. Plot shading begins at the 10 % frequency
//! and deepens at the 36 % frequency.

use serde::Serialize;
use std::f64::consts::PI;

/// Error at which shading begins.
pub const BEGIN_ERROR: f64 = 0.10;
/// Error at which shading deepens.
pub const DEEP_ERROR: f64 = 0.36;
/// Error below which a representation counts as trusted (continuity checks).
pub const TRUST_ERROR: f64 = 0.03;

/// One element's validity limit for one criterion.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidityLimit {
    pub element: String,
    pub criterion: &'static str,
    /// Frequency where shading begins (10 % error or the criterion's onset).
    pub begin_hz: Option<f64>,
    /// Frequency where shading deepens (36 % error or hard limit).
    pub deep_hz: Option<f64>,
}

/// Shading bands aggregated over all elements in the network.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct Shading {
    pub begin_hz: Option<f64>,
    pub deep_hz: Option<f64>,
}

pub fn aggregate(limits: &[ValidityLimit]) -> Shading {
    let min = |it: &mut dyn Iterator<Item = f64>| {
        it.fold(None, |m: Option<f64>, f| Some(m.map_or(f, |m| m.min(f))))
    };
    Shading {
        begin_hz: min(&mut limits.iter().filter_map(|l| l.begin_hz)),
        deep_hz: min(&mut limits.iter().filter_map(|l| l.deep_hz)),
    }
}

/// Lumped-compliance error 1 − x·cot(x) for 0 ≤ x < π.
pub fn cavity_error(x: f64) -> f64 {
    if x < 1e-4 {
        x * x / 3.0
    } else {
        1.0 - x / x.tan()
    }
}

/// Lumped-inertance error tan(x)/x − 1 for 0 ≤ x < π/2.
pub fn duct_error(x: f64) -> f64 {
    if x < 1e-4 {
        x * x / 3.0
    } else {
        x.tan() / x - 1.0
    }
}

/// Smallest x in (0, x_max) where `metric(x)` reaches `target` (bisection;
/// both metrics are monotonic on their domains).
pub fn kl_at_error(metric: fn(f64) -> f64, target: f64, x_max: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, x_max * (1.0 - 1e-12));
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if metric(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Frequency at which kL reaches `x` for length `l` and sound speed `c`.
pub fn freq_at_kl(x: f64, l: f64, c: f64) -> f64 {
    x * c / (2.0 * PI * l)
}

/// Validity of a cavity treated as one lumped node, `l` being the largest
/// distance from the driver to the observation point.
pub fn lumped_cavity(element: &str, l: f64, c: f64) -> ValidityLimit {
    ValidityLimit {
        element: element.to_string(),
        criterion: "lumped cavity kL",
        begin_hz: Some(freq_at_kl(kl_at_error(cavity_error, BEGIN_ERROR, PI), l, c)),
        deep_hz: Some(freq_at_kl(kl_at_error(cavity_error, DEEP_ERROR, PI), l, c)),
    }
}

/// Validity of a duct treated as a lumped series impedance.
pub fn lumped_duct(element: &str, l: f64, c: f64) -> ValidityLimit {
    ValidityLimit {
        element: element.to_string(),
        criterion: "lumped duct kl",
        begin_hz: Some(freq_at_kl(
            kl_at_error(duct_error, BEGIN_ERROR, PI / 2.0),
            l,
            c,
        )),
        deep_hz: Some(freq_at_kl(
            kl_at_error(duct_error, DEEP_ERROR, PI / 2.0),
            l,
            c,
        )),
    }
}

/// First transverse cut-on of a circular duct, 1.8412·c/(2πa).
pub fn circular_cut_on(radius: f64, c: f64) -> f64 {
    1.841_183_781_340_659 * c / (2.0 * PI * radius)
}

/// Stinson's bound on the low-reduced-frequency model: r·f^1.5 < 1e6 with
/// r in cm and f in Hz.
pub fn stinson_bound(r: f64) -> f64 {
    (1e6 / (r * 100.0)).powf(2.0 / 3.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_matches_spec_table_for_60mm() {
        // 3.0 % at kL 0.3, 35.8 % at kL 1 (spec Section 2 table, corrected).
        assert!((cavity_error(0.3) - 0.0302).abs() < 1e-3);
        assert!((cavity_error(1.0) - 0.358).abs() < 1e-3);
        let lim = lumped_cavity("c", 0.060, 343.0);
        let b = lim.begin_hz.unwrap();
        let d = lim.deep_hz.unwrap();
        assert!((b - 493.0).abs() < 3.0, "10 % at {b}");
        assert!((d - 910.0).abs() < 5.0, "36 % at {d}");
    }

    #[test]
    fn spec_cut_on_and_stinson_examples() {
        assert!((circular_cut_on(3.75e-3, 343.0) - 26_803.0).abs() < 5.0);
        assert!((stinson_bound(3.75e-3) - 19_230.0).abs() < 20.0);
        assert!((stinson_bound(1e-3) - 46_416.0).abs() < 20.0);
    }
}
