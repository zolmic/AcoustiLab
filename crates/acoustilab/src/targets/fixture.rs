//! Measurement fixtures (`data/targets/fixtures.json`).
//!
//! A target belongs to the fixture it was measured or defined on (spec
//! Section 11), and so does a response. Two fixtures match when they are the
//! same; they partly match when they share the ear simulator but differ in
//! pinna, head or canal extension; otherwise they differ. A fixture tag that
//! is not in the registry (a user's own) matches only itself.
//!
//! In a netlist, the drum reference point of an ear-load macro (`iec60318_4`,
//! `type33`, `type43`) stands for the fixture whose `engine_types` list that
//! macro. The engine models the ear simulator without pinna or head, so a
//! simulated response is never on a head and torso simulator.

use super::embedded_json;
use crate::circuit::{Circuit, ProbeKind};
use serde::Serialize;
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize)]
pub struct Fixture {
    pub id: String,
    pub label: String,
    pub ear_simulator: String,
    pub pinna: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    pub engine_types: Vec<String>,
    /// Range over which the ear simulator represents a human ear.
    #[serde(rename = "human_valid_Hz", skip_serializing_if = "Option::is_none")]
    pub human_valid_hz: Option<[f64; 2]>,
    pub source: String,
}

impl Fixture {
    /// True for fixtures built on the IEC 60318-4 simulator, which the
    /// standard does not validate below 100 Hz (spec Section 11).
    pub fn is_iec60318_4(&self) -> bool {
        self.ear_simulator == "iec60318_4"
    }
}

fn parse() -> Vec<Fixture> {
    let doc = embedded_json(
        "fixtures.json",
        include_str!("../../../../data/targets/fixtures.json"),
    );
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    doc["fixtures"]
        .as_array()
        .expect("fixtures.json: 'fixtures' array")
        .iter()
        .map(|v| Fixture {
            id: s(v, "id"),
            label: s(v, "label"),
            ear_simulator: s(v, "ear_simulator"),
            pinna: s(v, "pinna"),
            extension: v["extension"].as_str().map(str::to_string),
            engine_types: v["engine_types"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            human_valid_hz: v["human_valid_Hz"]
                .as_array()
                .and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])),
            source: s(v, "source"),
        })
        .collect()
}

/// Every registered fixture.
pub fn fixtures() -> &'static [Fixture] {
    static F: OnceLock<Vec<Fixture>> = OnceLock::new();
    F.get_or_init(parse)
}

pub fn fixture(id: &str) -> Option<&'static Fixture> {
    fixtures().iter().find(|f| f.id == id)
}

/// How two fixture tags relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    Same,
    /// Same ear simulator; pinna, head or canal extension differ.
    SameEarSimulator,
    Different,
}

pub fn compare(a: &str, b: &str) -> Match {
    if a == b {
        return Match::Same;
    }
    match (fixture(a), fixture(b)) {
        (Some(x), Some(y)) if x.ear_simulator == y.ear_simulator => Match::SameEarSimulator,
        _ => Match::Different,
    }
}

/// The fixture a pressure probe of a compiled netlist reads, if the probe
/// sits on an internal node (`<id>.drp`, `<id>.eep`, `<id>.ref`) of an ear
/// macro whose type a fixture lists.
pub fn for_probe(circuit: &Circuit, probe_id: &str) -> Option<&'static Fixture> {
    let probe = circuit.probes.iter().find(|p| p.id == probe_id)?;
    let ProbeKind::Node(i) = probe.kind else {
        return None;
    };
    let node = circuit.nodes.name(i);
    let (owner, _) = node.rsplit_once('.')?;
    let ty = circuit
        .elements
        .iter()
        .find(|e| e.id() == owner)?
        .type_name();
    fixtures()
        .iter()
        .find(|f| f.engine_types.iter().any(|t| t == ty))
}
