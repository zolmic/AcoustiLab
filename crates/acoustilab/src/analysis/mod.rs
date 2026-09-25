//! Design analysis over parametric netlists (docs/analysis.md):
//!
//! * [`sensitivity`]: the Jacobian of every probe with respect to every
//!   continuous parameter, in dB and degrees per percent, by central
//!   differences of complete solves in ln p (erratum E15 asks for the method
//!   to be named); heat-map slices of it.
//! * [`tornado`]: output changes at each parameter's tolerance limits, by
//!   re-solving, for a pinned frequency, a band mean or a readout.
//! * [`explain`]: sentences generated only from re-solved curves (spec
//!   Section 15; erratum E24).
//! * [`mc`]: Latin hypercube Monte Carlo over the parameter tolerances,
//!   factorial and user-given designs of experiments, envelopes, a
//!   reproducibility hash per run and CSV export (spec Sections 3 and 12).
//! * [`readouts`]: impedance, driver and response readouts (spec Sections 4
//!   and 10; erratum E32).
//!
//! Every analysis starts from a [`Design`]: a parsed parametric netlist and
//! the base overrides the user is looking at. Each evaluation is a
//! [`Point`]: the design with some parameters changed, expanded, compiled
//! and solved exactly as [`Circuit::solve`] does (drive scaling included).

pub mod canonical;
pub mod detmath;
pub mod explain;
pub mod mc;
pub mod readouts;
pub mod rng;
pub mod search;
pub mod sensitivity;
pub mod tornado;

use crate::circuit::{Circuit, Probe};
use crate::error::{Error, Result};
use crate::expr::PValue;
use crate::netlist::Document;
use crate::params::{Overrides, ParamDef, ParamKind, Parametric};
use crate::solve::{ProbeResult, SolveResult};
use serde::Serialize;
use serde_json::Value;

/// A parametric netlist with the base overrides every analysis starts from.
#[derive(Debug, Clone)]
pub struct Design {
    pub parametric: Parametric,
    pub base: Overrides,
    /// Every parameter's value under `base` (derived ones included).
    pub values: Vec<(String, PValue)>,
}

impl Design {
    pub fn new(parametric: Parametric, base: Overrides) -> Result<Design> {
        let values = parametric.values(&base)?;
        Ok(Design {
            parametric,
            base,
            values,
        })
    }

    pub fn parse(text: &str, base: &Overrides) -> Result<Design> {
        Design::new(Parametric::parse(text)?, base.clone())
    }

    /// Resolved value of a parameter under the base overrides.
    pub fn value(&self, name: &str) -> Option<&PValue> {
        self.values.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// Declaration of a parameter, or a `parameter` error.
    pub fn def(&self, name: &str) -> Result<&ParamDef> {
        self.parametric.def(name).ok_or_else(|| Error::Parameter {
            name: name.to_string(),
            msg: "no such parameter".into(),
        })
    }

    /// The parameter's label, else its name.
    pub fn label(&self, name: &str) -> String {
        self.parametric
            .def(name)
            .and_then(|d| d.label.clone())
            .unwrap_or_else(|| name.to_string())
    }

    /// Base overrides with `extra` on top.
    pub fn merged(&self, extra: &Overrides) -> Overrides {
        let mut o = self.base.clone();
        for (k, v) in extra {
            o.insert(k.clone(), v.clone());
        }
        o
    }

    /// Expands and compiles the design with `extra` overrides.
    pub fn point(&self, extra: &Overrides) -> Result<Point> {
        let overrides = self.merged(extra);
        let r = self.parametric.resolve(&overrides)?;
        let mut circuit = Circuit::from_document(Document::from_expanded(r.doc.clone())?)?;
        circuit.parameters = r.values;
        Ok(Point {
            overrides,
            doc: r.doc,
            circuit,
        })
    }

    /// The design at its base overrides.
    pub fn base_point(&self) -> Result<Point> {
        self.point(&Overrides::new())
    }

    /// `ui.primary_probe` of the netlist, if it names one.
    pub fn ui_primary_probe(&self) -> Option<String> {
        self.parametric
            .doc
            .get("ui")
            .and_then(|u| u.get("primary_probe"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The value of a continuous parameter (number kind, not integer, not
    /// derived), or why the parameter is not one.
    pub fn continuous_value(&self, def: &ParamDef) -> std::result::Result<f64, String> {
        match &def.kind {
            ParamKind::Derived { .. } => Err("derived (computed from other parameters)".into()),
            ParamKind::Bool { .. } => Err("boolean: not a continuous parameter".into()),
            ParamKind::Choice { .. } => Err("choice: not a continuous parameter".into()),
            ParamKind::Number { integer: true, .. } => {
                Err("integer: not a continuous parameter".into())
            }
            ParamKind::Number { .. } => self
                .value(&def.name)
                .and_then(PValue::as_num)
                .ok_or_else(|| "has no numeric value".into()),
        }
    }

    /// The declarations named in `names` (all non-derived ones when `None`),
    /// in declaration order for `None` and in the given order otherwise.
    pub fn selected(&self, names: Option<&[String]>) -> Result<Vec<&ParamDef>> {
        match names {
            None => Ok(self
                .parametric
                .defs
                .iter()
                .filter(|d| !d.is_derived())
                .collect()),
            Some(list) => list.iter().map(|n| self.def(n)).collect(),
        }
    }
}

/// The design at one set of overrides, expanded and compiled.
pub struct Point {
    /// Every override in force (base and extra).
    pub overrides: Overrides,
    /// The expanded netlist (see [`crate::params::Resolved::doc`]).
    pub doc: Value,
    pub circuit: Circuit,
}

impl Point {
    /// Reproducibility hash of the expanded netlist ([`canonical`]).
    pub fn hash(&self) -> String {
        canonical::netlist_hash(&self.doc)
    }

    pub fn solve(&self) -> Result<SolveResult> {
        self.circuit.solve()
    }

    /// Solves with `extra` probes appended to the netlist's; returns the
    /// usual result and the extra probes' results separately.
    pub fn solve_with(&mut self, extra: &[Probe]) -> Result<(SolveResult, Vec<ProbeResult>)> {
        let n = self.circuit.probes.len();
        self.circuit.probes.extend_from_slice(extra);
        let r = self.circuit.solve();
        self.circuit.probes.truncate(n);
        let mut r = r?;
        let extras = r.probes.split_off(n);
        Ok((r, extras))
    }

    /// Solves at the given frequencies instead of the netlist's sweep, with
    /// the drive applied exactly as [`Circuit::solve`] applies it.
    pub fn solve_at_freqs(&mut self, freqs: &[f64]) -> Result<SolveResult> {
        let saved = std::mem::replace(&mut self.circuit.freqs, freqs.to_vec());
        let r = self.circuit.solve();
        self.circuit.freqs = saved;
        r
    }

    /// Probe of the netlist by id.
    pub fn probe(&self, id: &str) -> Result<&Probe> {
        self.circuit
            .probes
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| Error::Probe {
                id: id.to_string(),
                msg: "no such probe".into(),
            })
    }

    /// Structure of the expanded netlist (see [`structure`]).
    pub fn structure(&self) -> Value {
        structure(&self.doc)
    }

    /// How `other` differs from this point in topology, or `None`: a
    /// different structure ([`structure_difference`]) or a different number
    /// of unknowns (an element whose internal nodes or branches depend on a
    /// numeric value, such as a segment count).
    pub fn topology_change(&self, other: &Point) -> Option<String> {
        structure_difference(&self.doc, &other.doc).or_else(|| {
            (self.circuit.dim != other.circuit.dim).then(|| {
                format!(
                    "changes the number of unknowns from {} to {}",
                    self.circuit.dim, other.circuit.dim
                )
            })
        })
    }
}

/// How a probe's curve is reported: dB SPL for acoustic pressures,
/// magnitude and phase for impedances, magnitude otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repr {
    Pressure,
    Impedance,
    Magnitude,
}

pub fn repr(p: &ProbeResult) -> Repr {
    if p.is_pressure {
        Repr::Pressure
    } else if p.quantity == "impedance" {
        Repr::Impedance
    } else {
        Repr::Magnitude
    }
}

/// SI unit of a probe. Node probes carry it; for element-port probes the
/// port's domain is found from the nodes its potential depends on (every
/// port potential is a difference of node potentials), as the wasm API
/// does. An impedance is V/I (ohm), p/U (Pa·s/m³) or, for a mechanical port
/// under the across/through convention, the mobility v/F (m/(N·s)).
pub fn probe_unit(c: &Circuit, p: &Probe) -> String {
    use crate::circuit::ProbeKind;
    use crate::netlist::Domain;
    if !p.unit.is_empty() {
        return p.unit.to_string();
    }
    let (element, port) = match p.kind {
        ProbeKind::Flow { element, port }
        | ProbeKind::PortPotential { element, port }
        | ProbeKind::Impedance { element, port } => (element, port),
        _ => return String::new(),
    };
    let e = &c.elements[element];
    let zero = crate::C64::new(0.0, 0.0);
    let mut x = vec![zero; c.dim];
    let mut domain = None;
    for j in 0..c.nodes.len() {
        x[j] = crate::C64::new(1.0, 0.0);
        let v = e.port_potential(&x, port);
        x[j] = zero;
        if v.is_some_and(|v| v != zero) {
            domain = Some(c.nodes.domain(j));
            break;
        }
    }
    let Some(d) = domain else {
        return String::new();
    };
    match p.kind {
        ProbeKind::Flow { .. } => d.flow().1.to_string(),
        ProbeKind::PortPotential { .. } => d.potential().1.to_string(),
        _ => match d {
            Domain::Electrical => "ohm".into(),
            Domain::Acoustic => "Pa*s/m^3".into(),
            Domain::Mechanical => "m/(N*s)".into(),
        },
    }
}

/// Indices of the probes named in `ids` (all when `None`).
pub fn select_probes(result: &SolveResult, ids: Option<&[String]>) -> Result<Vec<usize>> {
    match ids {
        None => Ok((0..result.probes.len()).collect()),
        Some(ids) => ids
            .iter()
            .map(|id| {
                result
                    .probes
                    .iter()
                    .position(|p| &p.id == id)
                    .ok_or_else(|| Error::Probe {
                        id: id.clone(),
                        msg: "no such probe".into(),
                    })
            })
            .collect(),
    }
}

/// A parameter an analysis left out, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Excluded {
    pub name: String,
    pub reason: String,
}

/// The expanded document with every number replaced by `null`. Two points
/// with equal structures have the same nodes, elements, element types,
/// keys, strings and booleans: only numeric values differ, so the same
/// matrix layout and probes. A parameter that changes the structure (an
/// `enabled` condition, an element type or a string chosen by an
/// expression) is not differentiable there.
pub fn structure(doc: &Value) -> Value {
    match doc {
        Value::Number(_) => Value::Null,
        Value::Array(a) => Value::Array(a.iter().map(structure).collect()),
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), structure(v))).collect())
        }
        v => v.clone(),
    }
}

/// Describes how two expanded documents differ in structure, or `None`.
pub fn structure_difference(a: &Value, b: &Value) -> Option<String> {
    let (sa, sb) = (structure(a), structure(b));
    if sa == sb {
        return None;
    }
    let ids = |v: &Value, list: &str| -> Vec<String> {
        v.get(list)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|i| {
                        i.get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("?")
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    for list in ["nodes", "elements", "probes"] {
        let (ia, ib) = (ids(&sa, list), ids(&sb, list));
        let added: Vec<&String> = ib.iter().filter(|x| !ia.contains(x)).collect();
        let removed: Vec<&String> = ia.iter().filter(|x| !ib.contains(x)).collect();
        if !added.is_empty() || !removed.is_empty() {
            let mut parts = Vec::new();
            if !removed.is_empty() {
                parts.push(format!("disables {list} {}", quoted(&removed)));
            }
            if !added.is_empty() {
                parts.push(format!("enables {list} {}", quoted(&added)));
            }
            return Some(parts.join(" and "));
        }
        let (la, lb) = (sa.get(list), sb.get(list));
        if la != lb {
            if let (Some(Value::Array(xa)), Some(Value::Array(xb))) = (la, lb) {
                for (x, y) in xa.iter().zip(xb) {
                    if x != y {
                        let id = x.get("id").and_then(Value::as_str).unwrap_or("?");
                        return Some(format!(
                            "changes the type, nodes, keys or a string of {} '{id}'",
                            list.trim_end_matches('s')
                        ));
                    }
                }
            }
        }
    }
    let oa = sa.as_object().cloned().unwrap_or_default();
    let ob = sb.as_object().cloned().unwrap_or_default();
    let key = oa
        .keys()
        .chain(ob.keys())
        .find(|k| oa.get(*k) != ob.get(*k))
        .cloned()
        .unwrap_or_default();
    Some(format!("changes the structure of '{key}'"))
}

fn quoted(v: &[&String]) -> String {
    v.iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// An error in an analysis's options.
pub(crate) fn options_error(what: &str, msg: impl std::fmt::Display) -> Error {
    Error::Netlist(format!("{what} options: {msg}"))
}

/// Parses an options object (`null` or absent means the defaults).
pub fn parse_options<T: serde::de::DeserializeOwned + Default>(
    what: &str,
    v: Option<&Value>,
) -> Result<T> {
    match v {
        None | Some(Value::Null) => Ok(T::default()),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| options_error(what, e)),
    }
}

/// 20·log10 of a magnitude ratio, NaN when either side is zero or not
/// finite.
pub(crate) fn db_ratio(a: crate::C64, b: crate::C64) -> f64 {
    let (x, y) = (a.norm(), b.norm());
    if x > 0.0 && y > 0.0 && x.is_finite() && y.is_finite() {
        20.0 * (x / y).log10()
    } else {
        f64::NAN
    }
}

/// Serialises `Overrides` as a JSON object of scalars.
pub fn overrides_json(o: &Overrides) -> Value {
    Value::Object(o.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
}

/// Parses a JSON object of scalars into `Overrides`.
pub fn overrides_from_json(v: &Value) -> std::result::Result<Overrides, String> {
    let obj = v
        .as_object()
        .ok_or("overrides must be an object of parameter values")?;
    obj.iter()
        .map(|(k, x)| {
            PValue::from_json(x).map(|p| (k.clone(), p)).ok_or_else(|| {
                format!("parameter '{k}': a value must be a number, boolean or string")
            })
        })
        .collect()
}

/// Serde helpers for `Overrides` fields.
pub(crate) mod overrides_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(o: &Overrides, s: S) -> std::result::Result<S::Ok, S::Error> {
        overrides_json(o).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<Overrides, D::Error> {
        let v = Value::deserialize(d)?;
        overrides_from_json(&v).map_err(serde::de::Error::custom)
    }
}

/// Serde helper for curves that may hold NaN or infinities (written as
/// `null`).
pub(crate) mod nan_vec {
    use serde::{Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[f64], s: S) -> std::result::Result<S::Ok, S::Error> {
        v.iter()
            .map(|x| x.is_finite().then_some(*x))
            .collect::<Vec<_>>()
            .serialize(s)
    }
}

/// [`nan_vec`] for `Option<Vec<f64>>`, read back with `null` as NaN.
pub(crate) mod opt_nan_vec {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        v: &Option<Vec<f64>>,
        s: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match v {
            Some(v) => super::nan_vec::serialize(v, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<Option<Vec<f64>>, D::Error> {
        let v: Option<Vec<Option<f64>>> = Option::deserialize(d)?;
        Ok(v.map(|v| v.into_iter().map(|x| x.unwrap_or(f64::NAN)).collect()))
    }
}

/// Serde helper for matrices of values that may be NaN.
pub(crate) mod nan_mat {
    use serde::{Serialize, Serializer};

    pub fn serialize<S: Serializer>(m: &[Vec<f64>], s: S) -> std::result::Result<S::Ok, S::Error> {
        m.iter()
            .map(|row| {
                row.iter()
                    .map(|x| x.is_finite().then_some(*x))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .serialize(s)
    }
}

/// Three significant digits, for sentences and labels.
pub fn sig3(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let mut e = x.abs().log10().floor() as i32;
    let scale = 10f64.powi(2 - e);
    let r = (x * scale).round() / scale;
    if r.abs() >= 10f64.powi(e + 1) {
        e += 1; // rounding carried into a new digit (99.96 → 100)
    }
    let d = 2 - e;
    if d > 0 {
        format!("{:.*}", d as usize, r)
    } else {
        format!("{}", r.round())
    }
}

/// A frequency with three significant digits and its unit, in Hz below
/// 1 kHz and in kHz above ("95.3 Hz", "1.20 kHz").
pub fn fmt_hz(f: f64) -> String {
    if f >= 999.5 {
        format!("{} kHz", sig3(f / 1000.0))
    } else {
        format!("{} Hz", sig3(f))
    }
}

/// A frequency range: "40.3 to 297 Hz", "1.20 to 3.40 kHz",
/// "297 Hz to 1.20 kHz".
pub fn fmt_hz_range(lo: f64, hi: f64) -> String {
    let (a, b) = (fmt_hz(lo), fmt_hz(hi));
    match (a.strip_suffix(" Hz"), b.strip_suffix(" Hz")) {
        (Some(x), Some(_)) => format!("{x} to {b}"),
        _ => match (a.strip_suffix(" kHz"), b.strip_suffix(" kHz")) {
            (Some(x), Some(_)) => format!("{x} to {b}"),
            _ => format!("{a} to {b}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_formatting() {
        assert_eq!(fmt_hz(95.27), "95.3 Hz");
        assert_eq!(fmt_hz(1204.0), "1.20 kHz");
        assert_eq!(fmt_hz(999.4), "999 Hz");
        assert_eq!(fmt_hz(999.6), "1.00 kHz");
        assert_eq!(fmt_hz_range(40.3, 297.0), "40.3 to 297 Hz");
        assert_eq!(fmt_hz_range(1200.0, 3400.0), "1.20 to 3.40 kHz");
        assert_eq!(fmt_hz_range(297.0, 1200.0), "297 Hz to 1.20 kHz");
        assert_eq!(sig3(12345.0), "12300");
        assert_eq!(sig3(99.96), "100");
        assert_eq!(sig3(-0.012345), "-0.0123");
    }

    #[test]
    fn structure_ignores_numbers_only() {
        let a = serde_json::json!({"elements": [{"id": "v", "type": "tube", "r_mm": 1}]});
        let b = serde_json::json!({"elements": [{"id": "v", "type": "tube", "r_mm": 2}]});
        let c = serde_json::json!({"elements": [{"id": "v", "type": "slit", "r_mm": 2}]});
        let d = serde_json::json!({"elements": []});
        assert_eq!(structure_difference(&a, &b), None);
        assert!(structure_difference(&a, &c)
            .unwrap()
            .contains("element 'v'"));
        assert!(structure_difference(&a, &d)
            .unwrap()
            .contains("disables elements 'v'"));
    }
}
