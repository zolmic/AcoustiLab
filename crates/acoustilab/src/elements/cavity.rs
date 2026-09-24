//! Cavities (spec Sections 6 and 9).
//!
//! One object whose representation depends on the fidelity level:
//! * L0, or a single node: a lumped compliance with the thermal wall-layer
//!   correction C = V/(γP0)·[1 + ε(1 − j)], ε = (γ−1)·δt·A_wall/(2V).
//! * L1 with two nodes (driver face, far face): a thermoviscous
//!   transmission line along the depth, with the end faces' thermal loss as
//!   shunt admittances so the low-frequency limit equals the L0 element.
//!   At L0 the two faces are joined by an ideal short, so switching levels
//!   never changes the netlist.

use super::{Build, Constructor, Element, FreqCx};
use crate::air::AirState;
use crate::error::Result;
use crate::mna::{abcd_mul, abcd_shunt, potential, Mna, Unknown};
use crate::netlist::Domain;
use crate::thermoviscous::{self, Section};
use crate::units::Dim;
use crate::validity::{self, ValidityLimit};
use crate::C64;
use std::any::Any;
use std::f64::consts::PI;

pub const TYPES: &[&str] = &["cavity"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    match ty {
        "cavity" => Some(cavity),
        _ => None,
    }
}

/// Cavity geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Geometry {
    /// Rectangular box, `lz` being the depth (driver face to far face).
    Box { lx: f64, ly: f64, lz: f64 },
    /// Cylinder of given radius and depth.
    Cylinder { radius: f64, depth: f64 },
    /// Volume only, optionally with a depth (circular cross-section assumed).
    Volume { volume: f64, depth: Option<f64> },
}

impl Geometry {
    pub fn volume(&self) -> f64 {
        match *self {
            Geometry::Box { lx, ly, lz } => lx * ly * lz,
            Geometry::Cylinder { radius, depth } => PI * radius * radius * depth,
            Geometry::Volume { volume, .. } => volume,
        }
    }

    pub fn depth(&self) -> Option<f64> {
        match *self {
            Geometry::Box { lz, .. } => Some(lz),
            Geometry::Cylinder { depth, .. } => Some(depth),
            Geometry::Volume { depth, .. } => depth,
        }
    }

    /// Cross-section area and perimeter normal to the depth axis.
    pub fn cross_section(&self) -> Option<(f64, f64)> {
        match *self {
            Geometry::Box { lx, ly, .. } => Some((lx * ly, 2.0 * (lx + ly))),
            Geometry::Cylinder { radius, .. } => Some((PI * radius * radius, 2.0 * PI * radius)),
            Geometry::Volume { volume, depth } => depth.map(|d| {
                let s = volume / d;
                (s, 2.0 * (PI * s).sqrt())
            }),
        }
    }

    /// Total wall area, if the geometry defines it.
    pub fn wall_area(&self) -> Option<f64> {
        let (s, p) = self.cross_section()?;
        Some(2.0 * s + p * self.depth()?)
    }

    /// Default characteristic length for the lumped criterion: the largest
    /// dimension (conservative stand-in for the driver-to-observation distance).
    pub fn largest_dimension(&self) -> f64 {
        match *self {
            Geometry::Box { lx, ly, lz } => lx.max(ly).max(lz),
            Geometry::Cylinder { radius, depth } => (2.0 * radius).max(depth),
            Geometry::Volume { volume, depth } => match depth {
                Some(d) => d.max(2.0 * (volume / d / PI).sqrt()),
                None => 3f64.sqrt() * volume.cbrt(),
            },
        }
    }

    /// First transverse cut-on, above which the depth line misses modes.
    pub fn transverse_cut_on(&self, c: f64) -> Option<f64> {
        match *self {
            Geometry::Box { lx, ly, .. } => Some(c / (2.0 * lx.max(ly))),
            Geometry::Cylinder { radius, .. } => Some(validity::circular_cut_on(radius, c)),
            Geometry::Volume { .. } => self
                .cross_section()
                .map(|(s, _)| validity::circular_cut_on((s / PI).sqrt(), c)),
        }
    }
}

pub struct Cavity {
    pub id: String,
    pub n1: Unknown,
    pub n2: Option<Unknown>,
    pub geometry: Geometry,
    /// Wall area used for thermal loss (already multiplied by the surface factor).
    pub wall_area: f64,
    pub surface_factor: f64,
    pub wall_loss: bool,
    pub max_distance: f64,
}

impl Cavity {
    /// Lumped compliance C(ω) including the thermal wall layer.
    pub fn compliance(&self, air: &AirState, omega: f64) -> C64 {
        let v = self.geometry.volume();
        let c0 = v / air.bulk_modulus();
        if !self.wall_loss {
            return C64::new(c0, 0.0);
        }
        let eps = (air.gamma - 1.0) * air.thermal_layer(omega) * self.wall_area / (2.0 * v);
        c0 * (C64::new(1.0, 0.0) + eps * C64::new(1.0, -1.0))
    }

    fn distributed(&self, level: u8) -> bool {
        level >= 1 && self.n2.is_some()
    }

    /// Transfer matrix face 1 → face 2 at L1.
    fn line_abcd(&self, air: &AirState, omega: f64) -> [C64; 4] {
        let (s, p) = self
            .geometry
            .cross_section()
            .expect("two-node cavity has a depth");
        let d = self.geometry.depth().expect("two-node cavity has a depth");
        let jw = C64::new(0.0, omega);
        if !self.wall_loss {
            let k = omega / air.c;
            let zc = air.rho_c() / s;
            let (ch, sh) = (C64::new((k * d).cos(), 0.0), C64::new(0.0, (k * d).sin()));
            return [ch, zc * sh, sh / zc, ch];
        }
        let section = Section::Equivalent {
            area: s,
            perimeter: p * self.surface_factor,
        };
        let line = thermoviscous::abcd(&section, air, omega, d);
        // Thermal loss on each end face: extra compliance
        // (γ−1)·δt·S·(1 − j)/(2γP0), as a shunt admittance.
        let dc = (air.gamma - 1.0) * air.thermal_layer(omega) * s * self.surface_factor
            / (2.0 * air.bulk_modulus());
        let y_end = jw * dc * C64::new(1.0, -1.0);
        abcd_mul(abcd_mul(abcd_shunt(y_end), line), abcd_shunt(y_end))
    }
}

impl Element for Cavity {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "cavity"
    }
    fn branch_count(&self) -> usize {
        if self.n2.is_some() {
            2
        } else {
            0
        }
    }
    fn port_count(&self) -> usize {
        if self.n2.is_some() {
            2
        } else {
            1
        }
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        match self.n2 {
            Some(n2) if self.distributed(cx.level) => {
                mna.two_port_abcd(
                    (self.n1, None),
                    (n2, None),
                    (br[0], br[1]),
                    self.line_abcd(cx.air, cx.omega),
                );
            }
            Some(n2) => {
                mna.admittance(self.n1, None, cx.jw() * self.compliance(cx.air, cx.omega));
                mna.short(self.n1, n2, br[0]);
                // Keep the second branch unknown determined.
                mna.add(Some(br[1]), Some(br[1]), C64::new(1.0, 0.0));
            }
            None => {
                mna.admittance(self.n1, None, cx.jw() * self.compliance(cx.air, cx.omega));
            }
        }
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        match (port, self.n2) {
            (0, _) => Some(potential(x, self.n1)),
            (1, Some(n2)) => Some(potential(x, n2)),
            _ => None,
        }
    }
    /// Flow entering the cavity at each face (to ambient).
    fn port_flow(&self, cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        let lumped = cx.jw() * self.compliance(cx.air, cx.omega) * potential(x, self.n1);
        match (port, self.n2) {
            (0, None) => Some(lumped),
            // L1 depth line: branch 0 enters face 1, branch 1 leaves face 2.
            (0, Some(_)) if self.distributed(cx.level) => Some(x[br[0]]),
            (1, Some(_)) if self.distributed(cx.level) => Some(-x[br[1]]),
            // L0: the short's branch unknown is the flow it delivers into
            // face 1, i.e. it carries −x[br0] from face 1 to face 2.
            (0, Some(_)) => Some(lumped - x[br[0]]),
            (1, Some(_)) => Some(x[br[0]]),
            _ => None,
        }
    }
    fn validity(&self, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        if self.distributed(level) {
            let cut = self.geometry.transverse_cut_on(air.c);
            vec![ValidityLimit {
                element: self.id.clone(),
                criterion: "cavity depth line: first transverse mode",
                begin_hz: cut.map(|f| 0.7 * f),
                deep_hz: cut,
            }]
        } else {
            vec![validity::lumped_cavity(&self.id, self.max_distance, air.c)]
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn cavity(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let n1 = b.terminal(0, Domain::Acoustic)?;
    let n2 = if b.terminals.len() == 2 {
        Some(b.terminal(1, Domain::Acoustic)?)
    } else {
        None
    };
    let p = &mut b.params;
    let lx = p.positive_opt("lx", Dim::Length)?;
    let ly = p.positive_opt("ly", Dim::Length)?;
    let lz = p.positive_opt("lz", Dim::Length)?;
    let radius = p.positive_opt("radius", Dim::Length)?;
    let depth = p.positive_opt("depth", Dim::Length)?;
    let volume = p.positive_opt("volume", Dim::Volume)?;
    let geometry = match (lx, ly, lz, radius, volume) {
        (Some(lx), Some(ly), Some(lz), None, None) if depth.is_none() => Geometry::Box { lx, ly, lz },
        (None, None, None, Some(radius), None) => Geometry::Cylinder {
            radius,
            depth: depth.ok_or_else(|| b.err("cylinder needs 'depth'"))?,
        },
        (None, None, None, None, Some(volume)) => Geometry::Volume { volume, depth },
        _ => {
            return Err(b.err(
                "give exactly one geometry: lx/ly/lz (box), radius+depth (cylinder), or volume [+depth]",
            ))
        }
    };
    let p = &mut b.params;
    let surface_factor = p.number_or("surface_factor", 1.0)?;
    let wall_loss = p.bool_or("wall_loss", true)?;
    let wall_area_override = p.positive_opt("wall_area", Dim::Area)?;
    let max_distance = p.positive_opt("max_distance", Dim::Length)?;
    if !(1.0..=10.0).contains(&surface_factor) {
        return Err(b.err("'surface_factor' must be between 1 and 10"));
    }
    if n2.is_some() && geometry.depth().is_none() {
        return Err(b.err("a two-node cavity needs a depth"));
    }
    let wall_area = match (wall_area_override, geometry.wall_area()) {
        (Some(a), _) => a,
        (None, Some(a)) => a,
        // Volume only: wall area of a cube of the same volume.
        (None, None) => 6.0 * geometry.volume().powf(2.0 / 3.0),
    } * surface_factor;
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(Cavity {
        id,
        n1,
        n2,
        geometry,
        wall_area,
        surface_factor,
        wall_loss,
        max_distance: max_distance.unwrap_or_else(|| geometry.largest_dimension()),
    }))
}
