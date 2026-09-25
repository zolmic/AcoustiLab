//! A compiled network: nodes, elements with their branch unknowns, probes.

use crate::air::AirState;
use crate::elements::{self, Element, FreqCx};
use crate::error::{Error, Result};
use crate::linalg;
use crate::mna::{potential, Mna, Unknown};
use crate::netlist::{Document, Domain, NodeTable, RawProbe};
use crate::validity::{self, Shading, ValidityLimit};
use crate::{grid, C64};
use std::f64::consts::PI;

/// What a probe reads.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeKind {
    /// Across quantity of a node (pressure, voltage, velocity).
    Node(usize),
    /// Mechanical node velocity divided by jω.
    Displacement(usize),
    /// Mechanical node velocity times jω.
    Acceleration(usize),
    /// Through quantity at an element port.
    Flow { element: usize, port: usize },
    /// Across quantity at an element port.
    PortPotential { element: usize, port: usize },
    /// Port potential over port flow.
    Impedance { element: usize, port: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub id: String,
    pub quantity: String,
    pub unit: &'static str,
    pub kind: ProbeKind,
    /// True for acoustic pressures, which also report dB SPL.
    pub is_pressure: bool,
}

pub struct Circuit {
    pub air: AirState,
    pub freqs: Vec<f64>,
    pub level: u8,
    pub nodes: NodeTable,
    pub elements: Vec<Box<dyn Element>>,
    /// Global unknown index of each element's first branch unknown.
    pub branch_offsets: Vec<usize>,
    pub dim: usize,
    pub probes: Vec<Probe>,
}

impl Circuit {
    pub fn from_json(text: &str) -> Result<Circuit> {
        Self::from_document(Document::parse(text)?)
    }

    pub fn from_document(doc: Document) -> Result<Circuit> {
        let air = AirState::from_json(doc.air.as_ref())?;
        let freqs = grid::from_json(doc.sweep.as_ref())?;
        let mut nodes = NodeTable::default();
        for (name, domain) in &doc.nodes {
            nodes.add(name, *domain)?;
        }
        let mut elements: Vec<Box<dyn Element>> = Vec::with_capacity(doc.elements.len());
        for raw in doc.elements {
            if elements.iter().any(|e| e.id() == raw.id) {
                return Err(Error::Netlist(format!("duplicate element id '{}'", raw.id)));
            }
            elements.push(elements::build(raw, &mut nodes, doc.level, air)?);
        }
        // Branch unknowns follow the node unknowns, in element order.
        let mut next = nodes.len();
        let branch_offsets = elements
            .iter()
            .map(|e| {
                let o = next;
                next += e.branch_count();
                o
            })
            .collect();
        let mut c = Circuit {
            air,
            freqs,
            level: doc.level,
            nodes,
            elements,
            branch_offsets,
            dim: next,
            probes: Vec::new(),
        };
        c.probes = doc
            .probes
            .iter()
            .map(|p| c.resolve_probe(p))
            .collect::<Result<_>>()?;
        for (i, p) in c.probes.iter().enumerate() {
            if c.probes[..i].iter().any(|q| q.id == p.id) {
                return Err(Error::Probe {
                    id: p.id.clone(),
                    msg: "duplicate probe id".into(),
                });
            }
        }
        Ok(c)
    }

    pub fn element_index(&self, id: &str) -> Option<usize> {
        self.elements.iter().position(|e| e.id() == id)
    }

    /// Global indices of element `i`'s branch unknowns.
    pub fn branches(&self, i: usize) -> Vec<usize> {
        let o = self.branch_offsets[i];
        (o..o + self.elements[i].branch_count()).collect()
    }

    fn unknown_name(&self, k: usize) -> String {
        if k < self.nodes.len() {
            return format!("node {}", self.nodes.name(k));
        }
        for (i, &o) in self.branch_offsets.iter().enumerate() {
            let n = self.elements[i].branch_count();
            if k >= o && k < o + n {
                return format!("branch {} of element {}", k - o, self.elements[i].id());
            }
        }
        format!("unknown {k}")
    }

    pub fn cx(&self, f: f64) -> FreqCx<'_> {
        FreqCx {
            f,
            omega: 2.0 * PI * f,
            air: &self.air,
            level: self.level,
        }
    }

    /// Assembles and solves the network at one frequency.
    pub fn solve_at(&self, f: f64) -> Result<Vec<C64>> {
        let cx = self.cx(f);
        let mut mna = Mna::new(self.nodes.len(), self.dim - self.nodes.len());
        for (i, e) in self.elements.iter().enumerate() {
            e.stamp(&cx, &mut mna, &self.branches(i));
        }
        linalg::solve(mna.a, &mna.rhs).map_err(|s| Error::Singular {
            f_hz: f,
            unknown: self.unknown_name(s.0),
        })
    }

    /// Potential of a named node in a solution vector.
    pub fn node_value(&self, x: &[C64], name: &str) -> Option<C64> {
        self.nodes.lookup(name).map(|(i, _)| x[i])
    }

    /// Evaluates one probe from a solution vector.
    pub fn probe_value(&self, probe: &Probe, f: f64, x: &[C64]) -> Result<C64> {
        let cx = self.cx(f);
        let jw = cx.jw();
        let missing = || Error::Probe {
            id: probe.id.clone(),
            msg: "element does not expose that port".into(),
        };
        Ok(match probe.kind {
            ProbeKind::Node(i) => x[i],
            ProbeKind::Displacement(i) => x[i] / jw,
            ProbeKind::Acceleration(i) => x[i] * jw,
            ProbeKind::Flow { element, port } => self.elements[element]
                .port_flow(&cx, x, &self.branches(element), port)
                .ok_or_else(missing)?,
            ProbeKind::PortPotential { element, port } => self.elements[element]
                .port_potential(x, port)
                .ok_or_else(missing)?,
            ProbeKind::Impedance { element, port } => {
                let e = &self.elements[element];
                let v = e.port_potential(x, port).ok_or_else(missing)?;
                let i = e
                    .port_flow(&cx, x, &self.branches(element), port)
                    .ok_or_else(missing)?;
                v / i
            }
        })
    }

    fn resolve_probe(&self, p: &RawProbe) -> Result<Probe> {
        let perr = |msg: String| Error::Probe {
            id: p.id.clone(),
            msg,
        };
        let q = p.quantity.as_str();
        let node_quantities = [
            "pressure",
            "voltage",
            "velocity",
            "potential",
            "displacement",
            "acceleration",
        ];
        if node_quantities.contains(&q) {
            let name = p
                .node
                .as_deref()
                .ok_or_else(|| perr(format!("'{q}' needs a 'node'")))?;
            let (i, domain) = self
                .nodes
                .lookup(name)
                .ok_or_else(|| perr(format!("unknown node '{name}'")))?;
            let expect = match q {
                "pressure" => Some(Domain::Acoustic),
                "voltage" => Some(Domain::Electrical),
                "velocity" | "displacement" | "acceleration" => Some(Domain::Mechanical),
                _ => None,
            };
            if let Some(d) = expect {
                if d != domain {
                    return Err(perr(format!(
                        "node '{name}' is {domain:?}, '{q}' needs {d:?}"
                    )));
                }
            }
            let (kind, unit) = match q {
                "displacement" => (ProbeKind::Displacement(i), "m"),
                "acceleration" => (ProbeKind::Acceleration(i), "m/s^2"),
                _ => (ProbeKind::Node(i), domain.potential().1),
            };
            return Ok(Probe {
                id: p.id.clone(),
                quantity: q.to_string(),
                unit,
                kind,
                is_pressure: domain == Domain::Acoustic && q != "displacement",
            });
        }
        let name = p
            .element
            .as_deref()
            .ok_or_else(|| perr(format!("'{q}' needs an 'element'")))?;
        let element = self
            .element_index(name)
            .ok_or_else(|| perr(format!("unknown element '{name}'")))?;
        let port = p.port;
        let ports = self.elements[element].port_count();
        if port >= ports {
            return Err(perr(format!(
                "element '{name}' has {ports} port(s); port {port} does not exist"
            )));
        }
        let (kind, unit) = match q {
            "flow" | "current" | "force" | "volume_velocity" => {
                (ProbeKind::Flow { element, port }, "")
            }
            "port_potential" => (ProbeKind::PortPotential { element, port }, ""),
            "impedance" => (ProbeKind::Impedance { element, port }, ""),
            _ => return Err(perr(format!("unknown quantity '{q}'"))),
        };
        Ok(Probe {
            id: p.id.clone(),
            quantity: q.to_string(),
            unit,
            kind,
            is_pressure: false,
        })
    }

    /// Real power absorbed by each element at one frequency (RMS phasors),
    /// Σ over its ports of Re(V·conj(I_in)); independent sources report the
    /// negative of the power they deliver. By Tellegen's theorem the values
    /// sum to zero. Elements without ports are skipped (and listed as `None`).
    pub fn power_absorbed(&self, f: f64, x: &[C64]) -> Vec<(String, Option<f64>)> {
        let cx = self.cx(f);
        self.elements
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let br = self.branches(i);
                let mut total = 0.0;
                for port in 0..e.port_count() {
                    match (e.port_potential(x, port), e.port_flow(&cx, x, &br, port)) {
                        (Some(v), Some(i)) => total += (v * i.conj()).re,
                        _ => return (e.id().to_string(), None),
                    }
                }
                if e.is_source() {
                    total = -total;
                }
                (e.id().to_string(), (e.port_count() > 0).then_some(total))
            })
            .collect()
    }

    pub fn validity(&self) -> Vec<ValidityLimit> {
        self.elements
            .iter()
            .flat_map(|e| e.validity(&self.air, self.level))
            .collect()
    }

    pub fn shading(&self) -> Shading {
        validity::aggregate(&self.validity())
    }
}

/// Convenience for tests and macros: potential difference between two unknowns.
pub fn across(x: &[C64], a: Unknown, b: Unknown) -> C64 {
    potential(x, a) - potential(x, b)
}
