//! Mechanical elements (mobility form: velocity across, force through).
//!
//! A mass is referenced to the inertial frame (ground), springs and dampers
//! connect two nodes (or a node and the frame), so the netlist topology
//! mirrors the physical structure.

use super::electrical::FlowSourceElement;
use super::{Build, Constructor, Element, FreqCx, OnePort};
use crate::error::Result;
use crate::netlist::Domain;
use crate::units::Dim;
use crate::C64;

pub const TYPES: &[&str] = &["mass", "spring", "damper", "suspension", "force_source"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    Some(match ty {
        "mass" => mass,
        "spring" => spring,
        "damper" => damper,
        "suspension" => suspension,
        "force_source" => force_source,
        _ => return None,
    })
}

fn one_port(
    b: Build,
    type_name: &'static str,
    grounded_only: bool,
    y: impl Fn(&FreqCx) -> C64 + Send + Sync + 'static,
) -> Result<Box<dyn Element>> {
    if grounded_only {
        b.expect_terminals(1, 1)?;
    } else {
        b.expect_terminals(1, 2)?;
    }
    let n1 = b.terminal(0, Domain::Mechanical)?;
    let n2 = b.terminal_or_ground(1, Domain::Mechanical)?;
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

/// Reads a compliance given either as `C_*_per_N` or as stiffness `K_N_per_m`.
fn compliance(b: &mut Build, c_key: &str, k_key: &str) -> Result<f64> {
    let c = b.params.positive_opt(c_key, Dim::MechCompliance)?;
    let k = b.params.positive_opt(k_key, Dim::MechStiffness)?;
    match (c, k) {
        (Some(c), None) => Ok(c),
        (None, Some(k)) => Ok(1.0 / k),
        _ => Err(b.err(format!("give exactly one of '{c_key}' or '{k_key}'"))),
    }
}

/// Mass M: force = M·jω·v, referenced to the inertial frame.
fn mass(mut b: Build) -> Result<Box<dyn Element>> {
    let m = b.params.positive("M", Dim::Mass)?;
    one_port(b, "mass", true, move |cx| cx.jw() * m)
}

/// Spring of compliance C: force = v/(jωC).
fn spring(mut b: Build) -> Result<Box<dyn Element>> {
    let c = compliance(&mut b, "C", "K")?;
    one_port(b, "spring", false, move |cx| (cx.jw() * c).inv())
}

/// Viscous damper R: force = R·v.
fn damper(mut b: Build) -> Result<Box<dyn Element>> {
    let r = b.params.positive("R", Dim::MechResistance)?;
    one_port(b, "damper", false, move |_| C64::new(r, 0.0))
}

/// Moving mass, suspension compliance and loss of a diaphragm in one
/// element from a node to the frame: Y = jωMms + Rms + 1/(jωCms).
fn suspension(mut b: Build) -> Result<Box<dyn Element>> {
    let m = b.params.positive("Mms", Dim::Mass)?;
    let c = compliance(&mut b, "Cms", "Kms")?;
    let r = b
        .params
        .quantity_opt("Rms", Dim::MechResistance)?
        .unwrap_or(0.0);
    if r < 0.0 {
        return Err(b.err("'Rms' must be non-negative"));
    }
    one_port(b, "suspension", true, move |cx| {
        cx.jw() * m + r + (cx.jw() * c).inv()
    })
}

fn force_source(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let into = b.terminal(0, Domain::Mechanical)?;
    let from = b.terminal_or_ground(1, Domain::Mechanical)?;
    let f = b.params.quantity_opt("F", Dim::Force)?.unwrap_or(1.0);
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(FlowSourceElement {
        id,
        type_name: "force_source",
        into,
        from,
        value: C64::new(f, 0.0),
    }))
}
