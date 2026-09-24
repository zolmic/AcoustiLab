//! Domain couplers.
//!
//! * `motor` — moving-coil force factor Bl between an electrical port and a
//!   mechanical port. In the mobility form it is an ideal transformer:
//!   e = Bl·v (back-EMF) and F = Bl·i.
//! * `piston` — diaphragm of effective area Sd between a mechanical node and
//!   two acoustic nodes (front, rear). It is a gyrator: U = Sd·v is injected
//!   into the front node and drawn from the rear node, and the pressure
//!   difference loads the diaphragm with F = Sd·(p_front − p_rear).
//!
//! Sign convention (spec Section 3): positive voltage drives positive coil
//! current, positive force and velocity toward the ear, positive front
//! pressure.

use super::{Build, Constructor, Element, FreqCx};
use crate::error::Result;
use crate::mna::{potential, Mna, Unknown};
use crate::netlist::Domain;
use crate::units::Dim;
use crate::C64;
use std::any::Any;

pub const TYPES: &[&str] = &["motor", "piston"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    Some(match ty {
        "motor" => motor,
        "piston" => piston,
        _ => return None,
    })
}

/// Moving-coil motor. Nodes: [e+, e−, m+, m−]; e− and m− may be ground.
pub struct Motor {
    pub id: String,
    pub e: (Unknown, Unknown),
    pub m: (Unknown, Unknown),
    pub bl: f64,
}

impl Element for Motor {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "motor"
    }
    fn branch_count(&self) -> usize {
        1
    }
    fn stamp(&self, _cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        mna.transformer(self.e, self.m, br[0], C64::new(self.bl, 0.0));
    }
    /// Port 0: coil voltage (back-EMF); port 1: diaphragm velocity.
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        let (p, n) = match port {
            0 => self.e,
            1 => self.m,
            _ => return None,
        };
        Some(potential(x, p) - potential(x, n))
    }
    /// Port 0: coil current; port 1: force delivered to the diaphragm.
    fn port_flow(&self, _cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        match port {
            0 => Some(x[br[0]]),
            1 => Some(x[br[0]] * self.bl),
            _ => None,
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn motor(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(4, 4)?;
    let e = (
        b.terminal(0, Domain::Electrical)?,
        b.terminal(1, Domain::Electrical)?,
    );
    let m = (
        b.terminal(2, Domain::Mechanical)?,
        b.terminal(3, Domain::Mechanical)?,
    );
    let bl = b.params.positive("Bl", Dim::ForceFactor)?;
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(Motor { id, e, m, bl }))
}

/// Rigid piston coupling. Nodes: [m+, m−, a_front, a_rear].
pub struct Piston {
    pub id: String,
    pub m: (Unknown, Unknown),
    pub front: Unknown,
    pub rear: Unknown,
    pub sd: f64,
}

impl Element for Piston {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "piston"
    }
    fn stamp(&self, _cx: &FreqCx, mna: &mut Mna, _br: &[usize]) {
        let sd = C64::new(self.sd, 0.0);
        // U = Sd·v into front, out of rear.
        mna.vccs(self.front, self.rear, self.m.0, self.m.1, sd);
        // Reaction force Sd·(p_front − p_rear) opposes the drive.
        mna.vccs(self.m.0, self.m.1, self.front, self.rear, -sd);
    }
    /// Port 0: diaphragm velocity; port 1: front minus rear pressure.
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        match port {
            0 => Some(potential(x, self.m.0) - potential(x, self.m.1)),
            1 => Some(potential(x, self.front) - potential(x, self.rear)),
            _ => None,
        }
    }
    /// Port 0: acoustic reaction force on the diaphragm; port 1: volume
    /// velocity delivered into the front node.
    fn port_flow(&self, _cx: &FreqCx, x: &[C64], _br: &[usize], port: usize) -> Option<C64> {
        match port {
            0 => Some((potential(x, self.front) - potential(x, self.rear)) * self.sd),
            1 => Some((potential(x, self.m.0) - potential(x, self.m.1)) * self.sd),
            _ => None,
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn piston(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(4, 4)?;
    let m = (
        b.terminal(0, Domain::Mechanical)?,
        b.terminal(1, Domain::Mechanical)?,
    );
    let front = b.terminal(2, Domain::Acoustic)?;
    let rear = b.terminal(3, Domain::Acoustic)?;
    let sd = b.params.positive("Sd", Dim::Area)?;
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(Piston {
        id,
        m,
        front,
        rear,
        sd,
    }))
}
