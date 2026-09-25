//! Netlist document: nodes, domains, and the raw element/probe records.
//!
//! Top-level shape (see docs/netlist.md):
//! ```json
//! { "schema": "acoustilab-netlist/0.2", "parameters": {..}, "air": {..},
//!   "sweep": {..}, "drive": {..}, "level": 1,
//!   "nodes": [{"id": "a_front", "domain": "acoustic"}, ..],
//!   "elements": [{"id": "..", "type": "..", "nodes": [..], ..}, ..],
//!   "probes": [{"id": "..", "quantity": "..", "node" | "element": ..}] }
//! ```

use crate::error::{Error, Result};
use crate::mna::Unknown;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const SCHEMA: &str = "acoustilab-netlist/0.2";

/// Physical domain of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Domain {
    Electrical,
    Mechanical,
    Acoustic,
}

impl Domain {
    pub fn parse(s: &str) -> Option<Domain> {
        match s {
            "electrical" => Some(Domain::Electrical),
            "mechanical" => Some(Domain::Mechanical),
            "acoustic" => Some(Domain::Acoustic),
            _ => None,
        }
    }

    /// Name of the across quantity and its SI unit.
    pub fn potential(self) -> (&'static str, &'static str) {
        match self {
            Domain::Electrical => ("voltage", "V"),
            Domain::Mechanical => ("velocity", "m/s"),
            Domain::Acoustic => ("pressure", "Pa"),
        }
    }

    /// Name of the through quantity and its SI unit.
    pub fn flow(self) -> (&'static str, &'static str) {
        match self {
            Domain::Electrical => ("current", "A"),
            Domain::Mechanical => ("force", "N"),
            Domain::Acoustic => ("volume_velocity", "m^3/s"),
        }
    }
}

/// Reserved names of the reference node. `gnd` is valid in every domain;
/// the others are domain-specific aliases (spec Appendix B spellings).
pub fn ground_domain(name: &str) -> Option<Option<Domain>> {
    match name {
        "gnd" => Some(None),
        "e_gnd" => Some(Some(Domain::Electrical)),
        "m_gnd" | "frame" => Some(Some(Domain::Mechanical)),
        "a_amb" | "a_gnd" | "ambient" => Some(Some(Domain::Acoustic)),
        _ => None,
    }
}

/// Node names → unknown indices and domains. Macro elements add internal
/// nodes named `<element id>.<local name>`.
#[derive(Debug, Clone, Default)]
pub struct NodeTable {
    index: BTreeMap<String, usize>,
    names: Vec<String>,
    domains: Vec<Domain>,
}

impl NodeTable {
    pub fn add(&mut self, name: &str, domain: Domain) -> Result<usize> {
        if ground_domain(name).is_some() {
            return Err(Error::Netlist(format!("'{name}' is reserved for ground")));
        }
        if self.index.contains_key(name) {
            return Err(Error::Netlist(format!("duplicate node '{name}'")));
        }
        let i = self.names.len();
        self.index.insert(name.to_string(), i);
        self.names.push(name.to_string());
        self.domains.push(domain);
        Ok(i)
    }

    /// Resolves a node name, checking its domain. Ground names resolve to
    /// `None`.
    pub fn resolve(&self, name: &str, domain: Domain) -> Result<Unknown> {
        if let Some(gd) = ground_domain(name) {
            return match gd {
                Some(d) if d != domain => Err(Error::Netlist(format!(
                    "ground '{name}' is {d:?}, expected {domain:?}"
                ))),
                _ => Ok(None),
            };
        }
        let &i = self
            .index
            .get(name)
            .ok_or_else(|| Error::Netlist(format!("unknown node '{name}'")))?;
        if self.domains[i] != domain {
            return Err(Error::Netlist(format!(
                "node '{name}' is {:?}, expected {domain:?}",
                self.domains[i]
            )));
        }
        Ok(Some(i))
    }

    /// Resolves a node of any domain.
    pub fn lookup(&self, name: &str) -> Option<(usize, Domain)> {
        self.index.get(name).map(|&i| (i, self.domains[i]))
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn name(&self, i: usize) -> &str {
        &self.names[i]
    }

    pub fn domain(&self, i: usize) -> Domain {
        self.domains[i]
    }
}

/// An element record before type-specific parsing.
#[derive(Debug, Clone)]
pub struct RawElement {
    pub id: String,
    pub ty: String,
    pub terminals: Vec<String>,
    /// Remaining keys: the type-specific parameters.
    pub params: Map<String, Value>,
}

/// A probe record before resolution.
#[derive(Debug, Clone)]
pub struct RawProbe {
    pub id: String,
    pub quantity: String,
    pub node: Option<String>,
    pub element: Option<String>,
    pub port: usize,
}

/// The parsed top-level document.
#[derive(Debug, Clone)]
pub struct Document {
    pub air: Option<Value>,
    pub sweep: Option<Value>,
    /// The `drive` key (see `drive::DriveSpec`).
    pub drive: Option<Value>,
    pub level: u8,
    pub nodes: Vec<(String, Domain)>,
    pub elements: Vec<RawElement>,
    pub probes: Vec<RawProbe>,
}

fn str_field(obj: &Map<String, Value>, key: &str, ctx: &str) -> Result<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Netlist(format!("{ctx}: missing string '{key}'")))
}

impl Document {
    /// Parses a document, resolving its parameters at their defaults.
    pub fn parse(text: &str) -> Result<Document> {
        let v: Value = serde_json::from_str(text)?;
        Self::from_value(v)
    }

    /// Reads a document, resolving its parameters at their defaults.
    pub fn from_value(v: Value) -> Result<Document> {
        let p = crate::params::Parametric::from_value(v)?;
        let r = p.resolve(&crate::params::Overrides::new())?;
        Self::from_expanded(r.doc)
    }

    /// Reads a document whose parameters are already resolved
    /// ([`crate::params::Parametric::resolve`]).
    pub fn from_expanded(v: Value) -> Result<Document> {
        let mut top = match v {
            Value::Object(m) => m,
            _ => return Err(Error::Netlist("top level must be an object".into())),
        };
        if let Some(s) = top.remove("schema") {
            let s = s.as_str().unwrap_or_default();
            if !s.starts_with("acoustilab-netlist/0.") {
                return Err(Error::Netlist(format!(
                    "unsupported schema '{s}', expected {SCHEMA}"
                )));
            }
        }
        if top.contains_key("parameters") {
            return Err(Error::Netlist(
                "'parameters' must be resolved before the document is read".into(),
            ));
        }
        let air = top.remove("air");
        let sweep = top.remove("sweep");
        let drive = top.remove("drive");
        let level = match top.remove("level") {
            None => 1,
            Some(l) => match l.as_u64() {
                Some(l @ 0..=1) => l as u8,
                _ => {
                    return Err(Error::Netlist(
                        "'level' must be 0 (lumped) or 1 (distributed)".into(),
                    ))
                }
            },
        };
        let _ = top.remove("title");
        let _ = top.remove("description");
        // Presentation hints for user interfaces; the engine ignores them.
        let _ = top.remove("ui");

        let mut nodes = Vec::new();
        for n in take_array(&mut top, "nodes")? {
            let obj = n
                .as_object()
                .ok_or_else(|| Error::Netlist("node entries must be objects".into()))?;
            let id = str_field(obj, "id", "node")?;
            let d = str_field(obj, "domain", &format!("node '{id}'"))?;
            let domain = Domain::parse(&d)
                .ok_or_else(|| Error::Netlist(format!("node '{id}': unknown domain '{d}'")))?;
            if let Some(extra) = obj.keys().find(|k| *k != "id" && *k != "domain") {
                return Err(Error::Netlist(format!(
                    "node '{id}': unknown key '{extra}'"
                )));
            }
            nodes.push((id, domain));
        }

        let mut elements = Vec::new();
        for e in take_array(&mut top, "elements")? {
            let mut obj = match e {
                Value::Object(m) => m,
                _ => return Err(Error::Netlist("element entries must be objects".into())),
            };
            let id = str_field(&obj, "id", "element")?;
            let ty = str_field(&obj, "type", &format!("element '{id}'"))?;
            obj.remove("id");
            obj.remove("type");
            let terminals = match (obj.remove("nodes"), obj.remove("node")) {
                (Some(Value::Array(a)), None) => a
                    .iter()
                    .map(|x| x.as_str().map(str::to_string))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| Error::element(&id, "'nodes' must be strings"))?,
                (None, Some(Value::String(s))) => vec![s],
                (None, None) => Vec::new(),
                _ => {
                    return Err(Error::element(
                        &id,
                        "give either 'nodes': [..] or 'node': \"..\"",
                    ))
                }
            };
            elements.push(RawElement {
                id,
                ty,
                terminals,
                params: obj,
            });
        }

        let mut probes = Vec::new();
        for p in take_array(&mut top, "probes")? {
            let obj = p
                .as_object()
                .ok_or_else(|| Error::Netlist("probe entries must be objects".into()))?;
            let id = str_field(obj, "id", "probe")?;
            let quantity = str_field(obj, "quantity", &format!("probe '{id}'"))?;
            let node = obj.get("node").and_then(Value::as_str).map(str::to_string);
            let element = obj
                .get("element")
                .and_then(Value::as_str)
                .map(str::to_string);
            let port = match obj.get("port") {
                None => 0,
                Some(v) => v.as_u64().ok_or_else(|| Error::Probe {
                    id: id.clone(),
                    msg: "'port' must be a non-negative integer".into(),
                })? as usize,
            };
            if let Some(extra) = obj
                .keys()
                .find(|k| !["id", "quantity", "node", "element", "port"].contains(&k.as_str()))
            {
                return Err(Error::Probe {
                    id,
                    msg: format!("unknown key '{extra}'"),
                });
            }
            probes.push(RawProbe {
                id,
                quantity,
                node,
                element,
                port,
            });
        }

        if let Some(extra) = top.keys().next() {
            return Err(Error::Netlist(format!("unknown top-level key '{extra}'")));
        }
        Ok(Document {
            air,
            sweep,
            drive,
            level,
            nodes,
            elements,
            probes,
        })
    }
}

fn take_array(top: &mut Map<String, Value>, key: &str) -> Result<Vec<Value>> {
    match top.remove(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(Error::Netlist(format!("'{key}' must be an array"))),
    }
}
