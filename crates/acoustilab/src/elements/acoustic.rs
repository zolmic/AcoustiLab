//! Lumped acoustic elements and acoustic sources.
//!
//! Fixed resistances, inertances and compliances exist as explicit user
//! overrides; geometry-derived elements (cavities, ducts, meshes, ...) live
//! in their own families (spec Section 6).

use super::electrical::FlowSourceElement;
use super::{Build, Constructor, Element, FreqCx, OnePort};
use crate::error::Result;
use crate::mna::{potential, Mna, Unknown};
use crate::netlist::Domain;
use crate::units::Dim;
use crate::C64;
use std::any::Any;

pub const TYPES: &[&str] = &[
    "acoustic_resistance",
    "acoustic_inertance",
    "acoustic_impedance",
    "acoustic_compliance",
    "flow_source",
    "pressure_source",
];

pub fn constructor(ty: &str) -> Option<Constructor> {
    Some(match ty {
        "acoustic_resistance" => resistance,
        "acoustic_inertance" => inertance,
        "acoustic_impedance" => impedance,
        "acoustic_compliance" => compliance,
        "flow_source" => flow_source,
        "pressure_source" => pressure_source,
        _ => return None,
    })
}

pub fn acoustic_terminals(b: &Build) -> Result<(Unknown, Unknown)> {
    b.expect_terminals(1, 2)?;
    Ok((
        b.terminal(0, Domain::Acoustic)?,
        b.terminal_or_ground(1, Domain::Acoustic)?,
    ))
}

fn one_port(
    b: Build,
    type_name: &'static str,
    y: impl Fn(&FreqCx) -> C64 + Send + Sync + 'static,
) -> Result<Box<dyn Element>> {
    let (n1, n2) = acoustic_terminals(&b)?;
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(OnePort {
        id,
        type_name,
        n1,
        n2,
        y: Box::new(y),
        limits: Vec::new(),
    }))
}

fn resistance(mut b: Build) -> Result<Box<dyn Element>> {
    let r = b.params.positive("R", Dim::AcousticResistance)?;
    one_port(b, "acoustic_resistance", move |_| C64::new(1.0 / r, 0.0))
}

fn inertance(mut b: Build) -> Result<Box<dyn Element>> {
    let m = b.params.positive("M", Dim::AcousticInertance)?;
    one_port(b, "acoustic_inertance", move |cx| (cx.jw() * m).inv())
}

/// Series resistance plus inertance.
fn impedance(mut b: Build) -> Result<Box<dyn Element>> {
    let r = b
        .params
        .quantity_opt("R", Dim::AcousticResistance)?
        .unwrap_or(0.0);
    let m = b
        .params
        .quantity_opt("M", Dim::AcousticInertance)?
        .unwrap_or(0.0);
    if r < 0.0 || m < 0.0 || (r == 0.0 && m == 0.0) {
        return Err(b.err("needs non-negative 'R' and/or 'M', not both zero"));
    }
    one_port(b, "acoustic_impedance", move |cx| (cx.jw() * m + r).inv())
}

/// Ideal lossless compliance (user override; use `cavity` for volumes).
fn compliance(mut b: Build) -> Result<Box<dyn Element>> {
    let c = b.params.positive("C", Dim::AcousticCompliance)?;
    one_port(b, "acoustic_compliance", move |cx| cx.jw() * c)
}

fn flow_source(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let into = b.terminal(0, Domain::Acoustic)?;
    let from = b.terminal_or_ground(1, Domain::Acoustic)?;
    let u = b
        .params
        .quantity_opt("U", Dim::VolumeVelocity)?
        .unwrap_or(1e-6);
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(FlowSourceElement {
        id,
        type_name: "flow_source",
        into,
        from,
        value: C64::new(u, 0.0),
    }))
}

/// Pressure source with optional series acoustic resistance, e.g. a diffuse
/// ambient field applied behind a leak for isolation analysis.
pub struct PressureSource {
    pub id: String,
    pub p: Unknown,
    pub n: Unknown,
    pub value: f64,
    pub zs: f64,
}

impl Element for PressureSource {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "pressure_source"
    }
    fn port_count(&self) -> usize {
        1
    }
    fn is_source(&self) -> bool {
        true
    }
    fn branch_count(&self) -> usize {
        1
    }
    fn stamp(&self, _cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        mna.potential_source(
            self.p,
            self.n,
            br[0],
            C64::new(self.value, 0.0),
            C64::new(self.zs, 0.0),
        );
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        (port == 0).then(|| potential(x, self.p) - potential(x, self.n))
    }
    fn port_flow(&self, _cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        (port == 0).then(|| x[br[0]])
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn pressure_source(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let p = b.terminal(0, Domain::Acoustic)?;
    let n = b.terminal_or_ground(1, Domain::Acoustic)?;
    let value = b.params.quantity_opt("p", Dim::Pressure)?.unwrap_or(1.0);
    let zs = b
        .params
        .quantity_opt("Zs", Dim::AcousticResistance)?
        .unwrap_or(0.0);
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(PressureSource {
        id,
        p,
        n,
        value,
        zs,
    }))
}
