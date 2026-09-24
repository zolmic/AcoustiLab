//! Electrical elements: R, L, C, voice coil, voltage and current sources.

use super::{Build, Constructor, Element, FreqCx, OnePort};
use crate::error::Result;
use crate::mna::{potential, Mna, Unknown, ZERO};
use crate::netlist::Domain;
use crate::units::Dim;
use crate::C64;
use std::any::Any;

pub const TYPES: &[&str] = &[
    "resistor",
    "inductor",
    "capacitor",
    "coil",
    "vsource",
    "isource",
];

pub fn constructor(ty: &str) -> Option<Constructor> {
    Some(match ty {
        "resistor" => resistor,
        "inductor" => inductor,
        "capacitor" => capacitor,
        "coil" => coil,
        "vsource" => vsource,
        "isource" => isource,
        _ => return None,
    })
}

fn two_terminal(b: &Build) -> Result<(Unknown, Unknown)> {
    b.expect_terminals(1, 2)?;
    Ok((
        b.terminal(0, Domain::Electrical)?,
        b.terminal_or_ground(1, Domain::Electrical)?,
    ))
}

fn one_port(
    b: Build,
    type_name: &'static str,
    y: impl Fn(&FreqCx) -> C64 + Send + Sync + 'static,
) -> Result<Box<dyn Element>> {
    let (n1, n2) = two_terminal(&b)?;
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

fn resistor(mut b: Build) -> Result<Box<dyn Element>> {
    let r = b.params.positive("R", Dim::ElecResistance)?;
    one_port(b, "resistor", move |_| C64::new(1.0 / r, 0.0))
}

fn inductor(mut b: Build) -> Result<Box<dyn Element>> {
    let l = b.params.positive("L", Dim::Inductance)?;
    one_port(b, "inductor", move |cx| (cx.jw() * l).inv())
}

fn capacitor(mut b: Build) -> Result<Box<dyn Element>> {
    let c = b.params.positive("C", Dim::Capacitance)?;
    one_port(b, "capacitor", move |cx| cx.jw() * c)
}

/// Voice-coil impedance model.
///
/// Z = Re + jωLe + (R2 ∥ jωL2): the DC resistance, a series inductance and
/// the two-branch LR-2 eddy-current network (spec Sections 4 and 5), which
/// stays realisable in the time domain. All inductive terms default to zero
/// because many headphone drivers list no inductance.
pub struct CoilModel {
    pub re: f64,
    pub le: f64,
    pub lr2: Option<(f64, f64)>,
}

impl CoilModel {
    pub fn impedance(&self, omega: f64) -> C64 {
        let jw = C64::new(0.0, omega);
        let mut z = C64::new(self.re, 0.0) + jw * self.le;
        if let Some((l2, r2)) = self.lr2 {
            let zl = jw * l2;
            z += zl * r2 / (zl + r2);
        }
        z
    }
}

fn coil(mut b: Build) -> Result<Box<dyn Element>> {
    let re = b.params.positive("Re", Dim::ElecResistance)?;
    let le = b.params.quantity_opt("Le", Dim::Inductance)?.unwrap_or(0.0);
    let l2 = b.params.positive_opt("L2", Dim::Inductance)?;
    let r2 = b.params.positive_opt("R2", Dim::ElecResistance)?;
    let lr2 = match (l2, r2) {
        (Some(l), Some(r)) => Some((l, r)),
        (None, None) => None,
        _ => return Err(b.err("LR-2 needs both 'L2' and 'R2'")),
    };
    let model = CoilModel { re, le, lr2 };
    one_port(b, "coil", move |cx| model.impedance(cx.omega).inv())
}

/// Voltage source with series impedance. `V` is the RMS open-circuit
/// voltage (default 1 V); `Zs` the source (output) impedance.
pub struct VSource {
    pub id: String,
    pub p: Unknown,
    pub n: Unknown,
    pub v: f64,
    pub zs: f64,
}

impl Element for VSource {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "vsource"
    }
    fn branch_count(&self) -> usize {
        1
    }
    fn stamp(&self, _cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        mna.potential_source(
            self.p,
            self.n,
            br[0],
            C64::new(self.v, 0.0),
            C64::new(self.zs, 0.0),
        );
    }
    /// Voltage at the source terminals (after the series impedance).
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        (port == 0).then(|| potential(x, self.p) - potential(x, self.n))
    }
    /// Current delivered out of the positive terminal.
    fn port_flow(&self, _cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        (port == 0).then(|| x[br[0]])
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn vsource(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let p = b.terminal(0, Domain::Electrical)?;
    let n = b.terminal_or_ground(1, Domain::Electrical)?;
    let v = b.params.quantity_opt("V", Dim::Voltage)?.unwrap_or(1.0);
    let zs = b
        .params
        .quantity_opt("Zs", Dim::ElecResistance)?
        .unwrap_or(0.0);
    if zs < 0.0 {
        return Err(b.err("'Zs' must be non-negative"));
    }
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(VSource { id, p, n, v, zs }))
}

/// Independent current source delivering `I` (RMS) out of its first
/// terminal into the network.
pub struct FlowSourceElement {
    pub id: String,
    pub type_name: &'static str,
    pub into: Unknown,
    pub from: Unknown,
    pub value: C64,
}

impl Element for FlowSourceElement {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        self.type_name
    }
    fn stamp(&self, _cx: &FreqCx, mna: &mut Mna, _br: &[usize]) {
        mna.flow_source(self.into, self.from, self.value);
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        (port == 0).then(|| potential(x, self.into) - potential(x, self.from))
    }
    fn port_flow(&self, _cx: &FreqCx, _x: &[C64], _br: &[usize], port: usize) -> Option<C64> {
        (port == 0).then_some(self.value)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn isource(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let into = b.terminal(0, Domain::Electrical)?;
    let from = b.terminal_or_ground(1, Domain::Electrical)?;
    let i = b.params.quantity_opt("I", Dim::Current)?.unwrap_or(1.0);
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(FlowSourceElement {
        id,
        type_name: "isource",
        into,
        from,
        value: C64::new(i, 0.0) + ZERO,
    }))
}
