//! Radiation impedance of an aperture (spec Section 6).
//!
//! `baffle: "infinite"` (default) is the exact rigid piston in an infinite
//! baffle, Z = ρc/S·[1 − 2J1(2ka)/(2ka) + j·2H1(2ka)/(2ka)]. Its low-ka
//! limit is R = (ka)²/2·ρc/S with the 8a/3π end correction.
//!
//! `baffle: "free"` is the open end of an unflanged, thin-walled circular
//! pipe: the exact Levine–Schwinger (1948) solution for the plane mode,
//! evaluated numerically (`special::unflanged_pipe`), Z = ρc/S·tanh(A/2 +
//! j·ka·L/a) with |R| = e^{−A}. Its low-ka limit is R = (ka)²/4·ρc/S with
//! the end correction L = 0.6127a (Levine & Schwinger printed 0.6133a). It
//! is exact below the first axisymmetric cut-on ka = j₁,₁ = 3.83; above
//! ka = 3.8 it is a passive continuation, flagged by the validity limit.

use super::acoustic::acoustic_terminals;
use super::{Build, Constructor, Element, FreqCx, OnePort};
use crate::air::AirState;
use crate::error::Result;
use crate::special::{piston_r1, piston_x1, unflanged_pipe, J1_FIRST_ZERO};
use crate::units::Dim;
use crate::validity::ValidityLimit;
use crate::C64;
use std::f64::consts::PI;

pub const TYPES: &[&str] = &["radiation"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    match ty {
        "radiation" => Some(radiation),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Baffle {
    Infinite,
    Free,
}

/// Fraction of the cut-on ka = j₁,₁ at which the `free` validity shading
/// begins: approaching cut-on, the evanescent (0,1) mode reaches further
/// into the pipe (its decay length is a/sqrt(j₁,₁² − (ka)²), 0.6a here), so
/// nearby discontinuities start to interact with the opening. A modelling
/// judgement, not a computed error level.
const FREE_SHADING_BEGIN: f64 = 0.9;

/// Acoustic radiation impedance (Pa·s/m³) of a circular aperture.
pub fn radiation_impedance(baffle: Baffle, radius: f64, air: &AirState, omega: f64) -> C64 {
    let s = PI * radius * radius;
    let ka = omega / air.c * radius;
    let z0 = air.rho_c() / s;
    match baffle {
        Baffle::Infinite => z0 * C64::new(piston_r1(2.0 * ka), piston_x1(2.0 * ka)),
        Baffle::Free => {
            let (attenuation, end) = unflanged_pipe(ka);
            z0 * crate::special::tanh(C64::new(0.5 * attenuation, ka * end))
        }
    }
}

fn radiation(mut b: Build) -> Result<Box<dyn Element>> {
    let p = &mut b.params;
    let radius = match (
        p.positive_opt("radius", Dim::Length)?,
        p.positive_opt("area", Dim::Area)?,
    ) {
        (Some(r), None) => r,
        (None, Some(a)) => (a / PI).sqrt(),
        _ => return Err(b.err("give exactly one of 'radius' or 'area'")),
    };
    let count = b.params.count_or("count", 1)? as f64;
    let baffle = match b.params.string_opt("baffle")?.as_deref() {
        None | Some("infinite") => Baffle::Infinite,
        Some("free") => Baffle::Free,
        Some(o) => return Err(b.err(format!("unknown baffle '{o}' (infinite, free)"))),
    };
    let (n1, n2) = acoustic_terminals(&b)?;
    let id = b.id.clone();
    let limits = if baffle == Baffle::Free {
        let f = J1_FIRST_ZERO * b.air.c / (2.0 * PI * radius);
        vec![ValidityLimit {
            element: id.clone(),
            criterion: "unflanged pipe radiation: first axisymmetric cut-on (ka = 3.83)",
            begin_hz: Some(FREE_SHADING_BEGIN * f),
            deep_hz: Some(f),
            ..Default::default()
        }]
    } else {
        Vec::new()
    };
    b.finish()?;
    Ok(Box::new(OnePort {
        id,
        type_name: "radiation",
        n1,
        n2,
        y: Box::new(move |cx: &FreqCx| {
            count / radiation_impedance(baffle, radius, cx.air, cx.omega)
        }),
        limits,
    }))
}
