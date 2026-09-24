//! Element library and the construction interface every family implements.
//!
//! An element is built from its netlist record by a family constructor,
//! declares how many MNA branch unknowns it needs, and stamps itself once per
//! frequency. Families live in their own files and register a
//! `constructor(type_name)` lookup in [`FAMILIES`].

pub mod acoustic;
pub mod cavity;
pub mod couplers;
pub mod ducts;
pub mod electrical;
pub mod mechanical;
pub mod radiation;

// Families developed in parallel; each file owns its constructor lookup.
pub mod driver;
pub mod ear;
pub mod materials;

use crate::air::AirState;
use crate::error::{Error, Result};
use crate::mna::{Mna, Unknown};
use crate::netlist::{Domain, NodeTable, RawElement};
use crate::units::Params;
use crate::validity::ValidityLimit;
use crate::C64;
use std::any::Any;

/// Per-frequency context handed to every stamp.
#[derive(Debug, Clone, Copy)]
pub struct FreqCx<'a> {
    pub f: f64,
    pub omega: f64,
    pub air: &'a AirState,
    /// Fidelity level: 0 lumped, 1 distributed closed-form.
    pub level: u8,
}

impl FreqCx<'_> {
    pub fn jw(&self) -> C64 {
        C64::new(0.0, self.omega)
    }
}

/// A network element.
pub trait Element: Send + Sync {
    fn id(&self) -> &str;
    fn type_name(&self) -> &'static str;

    /// Number of MNA branch unknowns this element needs.
    fn branch_count(&self) -> usize {
        0
    }

    /// Adds this element's contribution for one frequency. `br` holds the
    /// global indices of the element's branch unknowns.
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]);

    /// Number of ports this element exposes through `port_potential` and
    /// `port_flow`. Together the ports must account for every terminal
    /// connection, so that Σ Re(V·conj(I)) over all ports of all elements
    /// balances (Tellegen's theorem; see the energy-balance property test).
    fn port_count(&self) -> usize {
        0
    }

    /// True for independent sources, whose port flow is the flow they
    /// deliver (see `port_flow`).
    fn is_source(&self) -> bool {
        false
    }

    /// Across quantity of port `port` (voltage, velocity or pressure
    /// difference between the port's terminals).
    fn port_potential(&self, _x: &[C64], _port: usize) -> Option<C64> {
        None
    }

    /// Through quantity at port `port`, positive *entering* the element at
    /// the port's first terminal (and leaving at its second), so that
    /// Re(V·conj(I)) is the power the element absorbs there. Independent
    /// sources are the one exception: they report the flow they *deliver*
    /// into the network, so that potential/flow is the impedance they see.
    fn port_flow(&self, _cx: &FreqCx, _x: &[C64], _br: &[usize], _port: usize) -> Option<C64> {
        None
    }

    /// Validity limits of this element's representation at `level`.
    fn validity(&self, _air: &AirState, _level: u8) -> Vec<ValidityLimit> {
        Vec::new()
    }

    fn as_any(&self) -> &dyn Any;
}

/// Everything a family constructor needs to build one element.
pub struct Build<'a> {
    pub id: String,
    pub ty: String,
    pub terminals: Vec<String>,
    pub params: Params,
    pub nodes: &'a mut NodeTable,
    pub level: u8,
    pub air: AirState,
}

impl Build<'_> {
    pub fn err(&self, msg: impl Into<String>) -> Error {
        Error::element(self.id.clone(), msg)
    }

    /// Resolves terminal `i` in `domain`; errors if it was not given.
    pub fn terminal(&self, i: usize, domain: Domain) -> Result<Unknown> {
        let name = self
            .terminals
            .get(i)
            .ok_or_else(|| self.err(format!("needs at least {} node(s)", i + 1)))?;
        self.nodes
            .resolve(name, domain)
            .map_err(|e| self.err(e.to_string()))
    }

    /// Resolves terminal `i`, or ground if fewer terminals were given.
    pub fn terminal_or_ground(&self, i: usize, domain: Domain) -> Result<Unknown> {
        if i < self.terminals.len() {
            self.terminal(i, domain)
        } else {
            Ok(None)
        }
    }

    /// Checks the number of terminals is within `min..=max`.
    pub fn expect_terminals(&self, min: usize, max: usize) -> Result<()> {
        let n = self.terminals.len();
        if n < min || n > max {
            let want = if min == max {
                format!("{min}")
            } else {
                format!("{min} to {max}")
            };
            return Err(self.err(format!("expects {want} node(s), got {n}")));
        }
        Ok(())
    }

    /// Allocates an internal node `<id>.<local>` for macro elements.
    pub fn internal_node(&mut self, local: &str, domain: Domain) -> Result<Unknown> {
        let name = format!("{}.{}", self.id, local);
        Ok(Some(self.nodes.add(&name, domain)?))
    }

    /// Consumes the params, rejecting unused keys.
    pub fn finish(self) -> Result<()> {
        self.params.finish()
    }
}

pub type Constructor = fn(Build) -> Result<Box<dyn Element>>;

/// Family lookups, tried in order.
const FAMILIES: &[fn(&str) -> Option<Constructor>] = &[
    electrical::constructor,
    mechanical::constructor,
    couplers::constructor,
    acoustic::constructor,
    cavity::constructor,
    ducts::constructor,
    radiation::constructor,
    materials::constructor,
    driver::constructor,
    ear::constructor,
];

/// Builds an element from its raw record.
pub fn build(
    raw: RawElement,
    nodes: &mut NodeTable,
    level: u8,
    air: AirState,
) -> Result<Box<dyn Element>> {
    let ctor =
        FAMILIES
            .iter()
            .find_map(|f| f(&raw.ty))
            .ok_or_else(|| Error::UnknownElementType {
                id: raw.id.clone(),
                ty: raw.ty.clone(),
            })?;
    let b = Build {
        params: Params::new(raw.id.clone(), raw.params),
        id: raw.id,
        ty: raw.ty,
        terminals: raw.terminals,
        nodes,
        level,
        air,
    };
    ctor(b)
}

/// Type names every family accepts, for documentation and error messages.
pub fn known_types() -> Vec<&'static str> {
    let mut v = Vec::new();
    v.extend_from_slice(electrical::TYPES);
    v.extend_from_slice(mechanical::TYPES);
    v.extend_from_slice(couplers::TYPES);
    v.extend_from_slice(acoustic::TYPES);
    v.extend_from_slice(cavity::TYPES);
    v.extend_from_slice(ducts::TYPES);
    v.extend_from_slice(radiation::TYPES);
    v.extend_from_slice(materials::TYPES);
    v.extend_from_slice(driver::TYPES);
    v.extend_from_slice(ear::TYPES);
    v
}

// ----- Reusable building blocks -------------------------------------------

/// Frequency-dependent admittance of a one-port.
pub type AdmittanceFn = Box<dyn Fn(&FreqCx) -> C64 + Send + Sync>;
/// Frequency-dependent transfer matrix of a two-port.
pub type AbcdFn = Box<dyn Fn(&FreqCx) -> [C64; 4] + Send + Sync>;

/// A frequency-dependent two-terminal admittance between two nodes.
pub struct OnePort {
    pub id: String,
    pub type_name: &'static str,
    pub n1: Unknown,
    pub n2: Unknown,
    /// Admittance as a function of the frequency context.
    pub y: AdmittanceFn,
    pub limits: Vec<ValidityLimit>,
}

impl Element for OnePort {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        self.type_name
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, _br: &[usize]) {
        mna.admittance(self.n1, self.n2, (self.y)(cx));
    }
    fn port_count(&self) -> usize {
        1
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        (port == 0).then(|| crate::mna::potential(x, self.n1) - crate::mna::potential(x, self.n2))
    }
    fn port_flow(&self, cx: &FreqCx, x: &[C64], _br: &[usize], port: usize) -> Option<C64> {
        self.port_potential(x, port).map(|v| v * (self.y)(cx))
    }
    fn validity(&self, _air: &AirState, _level: u8) -> Vec<ValidityLimit> {
        self.limits.clone()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A frequency-dependent two-port given by its transfer (ABCD) matrix,
/// between (p1, n1) and (p2, n2). Uses two branch unknowns.
pub struct TwoPort {
    pub id: String,
    pub type_name: &'static str,
    pub port1: (Unknown, Unknown),
    pub port2: (Unknown, Unknown),
    pub abcd: AbcdFn,
    pub limits: Vec<ValidityLimit>,
}

impl Element for TwoPort {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        self.type_name
    }
    fn branch_count(&self) -> usize {
        2
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        mna.two_port_abcd(self.port1, self.port2, (br[0], br[1]), (self.abcd)(cx));
    }
    fn port_count(&self) -> usize {
        2
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        let (p, n) = match port {
            0 => self.port1,
            1 => self.port2,
            _ => return None,
        };
        Some(crate::mna::potential(x, p) - crate::mna::potential(x, n))
    }
    /// Flow entering at p1 (port 0) or at p2 (port 1; the negative of the
    /// flow the two-port delivers towards its load).
    fn port_flow(&self, _cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        match port {
            0 => Some(x[br[0]]),
            1 => Some(-x[br[1]]),
            _ => None,
        }
    }
    fn validity(&self, _air: &AirState, _level: u8) -> Vec<ValidityLimit> {
        self.limits.clone()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A macro element made of sub-elements (which may use internal nodes
/// allocated with [`Build::internal_node`]). Branch unknowns are allocated
/// contiguously in part order.
pub struct Composite {
    pub id: String,
    pub type_name: &'static str,
    pub parts: Vec<Box<dyn Element>>,
    /// Port definitions for probes: (potential, flow) readers.
    pub ports: Vec<CompositePort>,
}

/// How a composite exposes a port: the potential between two unknowns and
/// the flow through one of its parts. For the energy balance to hold, the
/// composite's ports must cover every external terminal connection (the
/// property test sums over the composite's ports, not its parts).
pub struct CompositePort {
    pub plus: Unknown,
    pub minus: Unknown,
    /// (part index, part port) whose flow is this port's flow.
    pub flow_from: (usize, usize),
}

impl Composite {
    fn offsets(&self) -> Vec<usize> {
        let mut acc = 0;
        self.parts
            .iter()
            .map(|p| {
                let o = acc;
                acc += p.branch_count();
                o
            })
            .collect()
    }
}

impl Element for Composite {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        self.type_name
    }
    fn branch_count(&self) -> usize {
        self.parts.iter().map(|p| p.branch_count()).sum()
    }
    fn port_count(&self) -> usize {
        self.ports.len()
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        for (p, o) in self.parts.iter().zip(self.offsets()) {
            p.stamp(cx, mna, &br[o..o + p.branch_count()]);
        }
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        self.ports
            .get(port)
            .map(|p| crate::mna::potential(x, p.plus) - crate::mna::potential(x, p.minus))
    }
    fn port_flow(&self, cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        let p = self.ports.get(port)?;
        let (part, pp) = p.flow_from;
        let o = self.offsets()[part];
        let e = &self.parts[part];
        e.port_flow(cx, x, &br[o..o + e.branch_count()], pp)
    }
    fn validity(&self, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        self.parts
            .iter()
            .flat_map(|p| p.validity(air, level))
            .collect()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
