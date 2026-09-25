//! Thermoviscous ducts: circular tubes and slits (spec Section 6).
//!
//! * L0: lumped series impedance jωρ_eff·l/S — frequency-dependent
//!   resistance plus inertance, compressibility neglected.
//! * L1: the full transmission-line two-port.
//!
//! Tubes take optional end corrections: `"flanged"` (0.8216a, Norris &
//! Sheng / Nomura et al.), `"piston"` (8a/3π ≈ 0.8488a, the low-frequency
//! baffled-piston limit the spec's worked examples use) or `"unflanged"`
//! (0.6133a, Levine & Schwinger). An outlet that radiates should use
//! `"none"` and connect a `radiation` element instead, whose reactance
//! already contains the end correction.

use super::{Build, Constructor, Element, FreqCx, OnePort, TwoPort};
use crate::air::AirState;
use crate::error::Result;
use crate::mna::{abcd_mul, abcd_series};
use crate::netlist::Domain;
use crate::thermoviscous::{self, Section};
use crate::units::Dim;
use crate::validity::{self, ValidityLimit};
use crate::C64;
use std::f64::consts::PI;

pub const TYPES: &[&str] = &["tube", "slit"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    Some(match ty {
        "tube" => tube,
        "slit" => slit,
        _ => return None,
    })
}

/// End-correction length for a tube end of radius `a`.
pub fn end_correction(kind: &str, a: f64) -> Option<f64> {
    Some(match kind {
        "none" => 0.0,
        "flanged" => 0.8216 * a,
        "piston" => 8.0 * a / (3.0 * PI),
        "unflanged" => 0.6133 * a,
        _ => return None,
    })
}

/// A duct: section, length, number of identical parallel copies, and the
/// total end-correction length (added as inertance ρδ/S).
#[derive(Debug, Clone, Copy)]
pub struct Duct {
    pub section: Section,
    pub length: f64,
    pub count: usize,
    pub end_length: f64,
}

impl Duct {
    /// Series impedance of the lumped representation (all copies in parallel).
    pub fn lumped_impedance(&self, air: &AirState, omega: f64) -> C64 {
        let s = self.section.area();
        let z = thermoviscous::lumped_series_impedance(&self.section, air, omega, self.length)
            + C64::new(0.0, omega * air.rho * self.end_length / s);
        z / self.count as f64
    }

    /// Transfer matrix of the distributed representation (all copies).
    pub fn abcd(&self, air: &AirState, omega: f64) -> [C64; 4] {
        let s = self.section.area();
        let half_end = abcd_series(C64::new(0.0, omega * air.rho * 0.5 * self.end_length / s));
        let line = thermoviscous::abcd(&self.section, air, omega, self.length);
        let [a, b, c, d] = abcd_mul(abcd_mul(half_end, line), half_end);
        let n = self.count as f64;
        [a, b / n, c * n, d]
    }

    pub fn limits(&self, id: &str, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        if level == 0 {
            return vec![validity::lumped_duct(
                id,
                self.length + self.end_length,
                air.c,
            )];
        }
        let (cut_on, stinson) = match self.section {
            Section::Circle { radius } => (
                validity::circular_cut_on(radius, air.c),
                validity::stinson_bound(radius),
            ),
            Section::Slit { gap, width } => {
                (air.c / (2.0 * width), validity::stinson_bound(0.5 * gap))
            }
            Section::Rect { a, b } => (
                air.c / (2.0 * a.max(b)),
                validity::stinson_bound(0.5 * a.min(b)),
            ),
            Section::Equivalent { area, perimeter } => {
                let r = 2.0 * area / perimeter;
                (
                    validity::circular_cut_on(r, air.c),
                    validity::stinson_bound(r),
                )
            }
        };
        vec![
            ValidityLimit {
                element: id.to_string(),
                criterion: "duct: first transverse mode",
                begin_hz: Some(0.7 * cut_on),
                deep_hz: Some(cut_on),
            },
            ValidityLimit {
                element: id.to_string(),
                criterion: "duct: Stinson low-reduced-frequency bound",
                begin_hz: Some(0.8 * stinson),
                deep_hz: Some(stinson),
            },
        ]
    }
}

/// Builds the element for a duct between the first two terminals.
fn duct_element(b: Build, type_name: &'static str, duct: Duct) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let n1 = b.terminal(0, Domain::Acoustic)?;
    let n2 = b.terminal_or_ground(1, Domain::Acoustic)?;
    let id = b.id.clone();
    let level = b.level;
    let limits = duct.limits(&id, &b.air, level);
    b.finish()?;
    if level == 0 {
        Ok(Box::new(OnePort {
            id,
            type_name,
            n1,
            n2,
            y: Box::new(move |cx: &FreqCx| duct.lumped_impedance(cx.air, cx.omega).inv()),
            limits,
        }))
    } else {
        Ok(Box::new(TwoPort {
            id,
            type_name,
            port1: (n1, None),
            port2: (n2, None),
            abcd: Box::new(move |cx: &FreqCx| duct.abcd(cx.air, cx.omega)),
            limits,
        }))
    }
}

fn tube(mut b: Build) -> Result<Box<dyn Element>> {
    let p = &mut b.params;
    let radius = match (
        p.positive_opt("radius", Dim::Length)?,
        p.positive_opt("diameter", Dim::Length)?,
    ) {
        (Some(r), None) => r,
        (None, Some(d)) => 0.5 * d,
        _ => return Err(b.err("give exactly one of 'radius' or 'diameter'")),
    };
    let p = &mut b.params;
    let length = p.positive("length", Dim::Length)?;
    let count = p.count_or("count", 1)?;
    let inlet = p.string_opt("inlet")?.unwrap_or_else(|| "none".into());
    let outlet = p.string_opt("outlet")?.unwrap_or_else(|| "none".into());
    let end_length = match (
        end_correction(&inlet, radius),
        end_correction(&outlet, radius),
    ) {
        (Some(i), Some(o)) => i + o,
        _ => return Err(b.err("end corrections must be none, flanged, piston or unflanged")),
    };
    let duct = Duct {
        section: Section::Circle { radius },
        length,
        count,
        end_length,
    };
    duct_element(b, "tube", duct)
}

fn slit(mut b: Build) -> Result<Box<dyn Element>> {
    let p = &mut b.params;
    let gap = p.positive("gap", Dim::Length)?;
    let width = p.positive("width", Dim::Length)?;
    let length = p.positive("length", Dim::Length)?;
    let count = p.count_or("count", 1)?;
    if width < 5.0 * gap {
        return Err(b.err("slit model needs width ≥ 5 × gap"));
    }
    let duct = Duct {
        section: Section::Slit { gap, width },
        length,
        count,
        end_length: 0.0,
    };
    duct_element(b, "slit", duct)
}
