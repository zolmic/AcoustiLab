//! Radiation impedance of an aperture (spec Section 6).
//!
//! `baffle: "infinite"` (default) is the exact rigid piston in an infinite
//! baffle, Z = ρc/S·[1 − 2J1(2ka)/(2ka) + j·2H1(2ka)/(2ka)]. Its low-ka
//! reactance is the 8a/3π end correction. `baffle: "free"` is the
//! low-frequency approximation of an unflanged opening: half the resistance
//! and the 0.6133a end correction (Levine & Schwinger), marked approximate.

use super::acoustic::acoustic_terminals;
use super::{Build, Constructor, Element, FreqCx, OnePort};
use crate::air::AirState;
use crate::error::Result;
use crate::special::{piston_r1, piston_x1};
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

/// Acoustic radiation impedance (Pa·s/m³) of a circular aperture.
pub fn radiation_impedance(baffle: Baffle, radius: f64, air: &AirState, omega: f64) -> C64 {
    let s = PI * radius * radius;
    let ka = omega / air.c * radius;
    let z0 = air.rho_c() / s;
    match baffle {
        Baffle::Infinite => z0 * C64::new(piston_r1(2.0 * ka), piston_x1(2.0 * ka)),
        Baffle::Free => {
            // Half the baffled resistance; reactance scaled to the 0.6133a
            // end correction. Valid for ka ≲ 0.5.
            let scale = 0.6133 / (8.0 / (3.0 * PI));
            z0 * C64::new(0.5 * piston_r1(2.0 * ka), scale * piston_x1(2.0 * ka))
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
        let f = 0.5 * b.air.c / (2.0 * PI * radius);
        vec![ValidityLimit {
            element: id.clone(),
            criterion: "unflanged radiation approximation (ka < 0.5)",
            begin_hz: Some(f),
            deep_hz: Some(2.0 * f),
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
