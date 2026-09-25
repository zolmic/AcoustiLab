//! JSON-in, JSON-out API behind the wasm-bindgen exports.
//!
//! Everything here is plain Rust so it is tested natively (`cargo test`);
//! `lib.rs` only adds the `#[wasm_bindgen]` shims. The engine crate is used
//! through its public API and is not modified.
//!
//! * [`solve_value`] returns the engine's `SolveResult::to_json()` document
//!   with two per-probe annotations the engine leaves out (see
//!   [`annotate_probes`]): the probe's `domain`, and its `unit` where the
//!   engine reports an empty unit (element-port probes).
//! * [`check_value`] parses and compiles a netlist without solving it.
//! * Failures are `{"error": "<engine message>", "kind": .., ...}` with the
//!   offending element, probe, node or JSON position when the engine error
//!   carries one, so the UI can point at it.

use acoustilab::circuit::{Circuit, Probe, ProbeKind};
use acoustilab::netlist::Domain;
use acoustilab::{Error, C64};
use serde_json::{json, Map, Value};

/// Solves a netlist. Returns the result document or an error object.
pub fn solve_value(netlist_json: &str) -> Value {
    let circuit = match Circuit::from_json(netlist_json) {
        Ok(c) => c,
        Err(e) => return error_value(&e),
    };
    match circuit.solve() {
        Ok(r) => {
            let mut v = r.to_json();
            annotate_probes(&circuit, &mut v);
            v
        }
        Err(e) => error_value(&e),
    }
}

/// Parses and compiles a netlist without solving it.
///
/// Also evaluates every probe once on an all-zero solution vector: the
/// engine resolves a probe's element when it compiles the netlist, but only
/// finds out that the element has no such `port` when it first evaluates the
/// probe during the solve. Without this, `check` would call a netlist valid
/// that `solve` then rejects, with the same message.
pub fn check_value(netlist_json: &str) -> Value {
    match Circuit::from_json(netlist_json).and_then(|c| check_probes(&c).map(|()| c)) {
        Ok(c) => {
            let f_min = c.freqs.iter().copied().fold(f64::INFINITY, f64::min);
            let f_max = c.freqs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            json!({
                "ok": true,
                "nodes": c.nodes.len(),
                "elements": c.elements.len(),
                "unknowns": c.dim,
                "probes": c.probes.len(),
                "frequencies": c.freqs.len(),
                "f_min_Hz": f_min,
                "f_max_Hz": f_max,
                "level": c.level,
                "air": c.air,
                "shading": c.shading(),
            })
        }
        Err(e) => error_value(&e),
    }
}

/// Evaluates every probe on a zero solution vector at the first grid
/// frequency, returning the first error `Circuit::probe_value` reports (the
/// error `solve` would report for that probe). Values are discarded.
fn check_probes(c: &Circuit) -> acoustilab::Result<()> {
    let Some(&f) = c.freqs.first() else {
        return Ok(());
    };
    let x = vec![C64::new(0.0, 0.0); c.dim];
    for p in &c.probes {
        c.probe_value(p, f, &x)?;
    }
    Ok(())
}

/// Element type names the engine accepts, as a JSON array.
pub fn element_types_value() -> Value {
    json!(acoustilab::elements::known_types())
}

/// Engine name and version.
pub fn engine_version() -> &'static str {
    acoustilab::solve::ENGINE
}

/// Serialises a JSON value (non-finite floats are already `null` in a
/// `serde_json::Value`, so this cannot fail).
pub fn to_string(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("{{\"error\":\"serialise: {e}\"}}"))
}

/// Maps an engine error to `{"error", "kind", ...}`.
///
/// | kind | extra keys |
/// |---|---|
/// | `json` | `line`, `column` (1-based, from serde_json) |
/// | `netlist` | none |
/// | `element` | `element` |
/// | `unknown_type` | `element`, `type` |
/// | `singular` | `f_Hz`, `unknown`, and `node` or `element` when the undetermined unknown belongs to one |
/// | `probe` | `probe` |
pub fn error_value(e: &Error) -> Value {
    let mut o = Map::new();
    o.insert("error".into(), json!(e.to_string()));
    // The engine's Error enum may gain variants; unknown ones map to "other".
    #[allow(unreachable_patterns)]
    let kind = match e {
        Error::Json(j) => {
            o.insert("line".into(), json!(j.line()));
            o.insert("column".into(), json!(j.column()));
            "json"
        }
        Error::Netlist(_) => "netlist",
        Error::Element { id, .. } => {
            o.insert("element".into(), json!(id));
            "element"
        }
        Error::UnknownElementType { id, ty } => {
            o.insert("element".into(), json!(id));
            o.insert("type".into(), json!(ty));
            "unknown_type"
        }
        Error::Singular { f_hz, unknown } => {
            o.insert("f_Hz".into(), json!(f_hz));
            o.insert("unknown".into(), json!(unknown));
            // Circuit::unknown_name spells these "node <name>" and
            // "branch <k> of element <id>".
            if let Some(node) = unknown.strip_prefix("node ") {
                o.insert("node".into(), json!(node));
            } else if let Some((_, id)) = unknown.split_once(" of element ") {
                o.insert("element".into(), json!(id));
            }
            "singular"
        }
        Error::Probe { id, .. } => {
            o.insert("probe".into(), json!(id));
            "probe"
        }
        _ => "other",
    };
    o.insert("kind".into(), json!(kind));
    Value::Object(o)
}

/// Adds `domain` to every probe of a result document and fills `unit` where
/// the engine left it empty. Probes appear in the document in the order of
/// `circuit.probes`.
pub fn annotate_probes(circuit: &Circuit, doc: &mut Value) {
    let Some(probes) = doc.get_mut("probes").and_then(Value::as_array_mut) else {
        return;
    };
    for (p, out) in circuit.probes.iter().zip(probes.iter_mut()) {
        let domain = probe_domain(circuit, p);
        out["domain"] = domain.map_or(Value::Null, |d| json!(d));
        if out["unit"].as_str().is_some_and(str::is_empty) {
            if let Some(u) = domain.and_then(|d| element_probe_unit(&p.kind, d)) {
                out["unit"] = json!(u);
            }
        }
    }
}

/// Physical domain a probe reads: the node's domain for node probes, the
/// port's domain for element-port probes (see [`port_domain`]).
pub fn probe_domain(circuit: &Circuit, probe: &Probe) -> Option<Domain> {
    #[allow(unreachable_patterns)]
    match probe.kind {
        ProbeKind::Node(i) | ProbeKind::Displacement(i) | ProbeKind::Acceleration(i) => {
            Some(circuit.nodes.domain(i))
        }
        ProbeKind::Flow { element, port }
        | ProbeKind::PortPotential { element, port }
        | ProbeKind::Impedance { element, port } => port_domain(circuit, element, port),
        _ => None,
    }
}

/// SI unit of an element-port probe in `domain`, in the engine's spelling
/// ("m^3/s", "Pa").
///
/// An `impedance` probe is port potential over port flow. Under the
/// across/through convention (docs/conventions.md) that is V/I in ohm for
/// electrical ports and p/U in Pa·s/m³ for acoustic ports, but v/F for a
/// mechanical port: a *mobility*, in m/(N·s) = s/kg, not a mechanical
/// impedance in N·s/m.
pub fn element_probe_unit(kind: &ProbeKind, domain: Domain) -> Option<&'static str> {
    #[allow(unreachable_patterns)]
    match kind {
        ProbeKind::Flow { .. } => Some(domain.flow().1),
        ProbeKind::PortPotential { .. } => Some(domain.potential().1),
        ProbeKind::Impedance { .. } => Some(match domain {
            Domain::Electrical => "ohm",
            Domain::Mechanical => "m/(N*s)",
            Domain::Acoustic => "Pa*s/m^3",
        }),
        _ => None,
    }
}

/// Domain of port `port` of element `element`.
///
/// The `Element` trait does not expose port domains, but every port
/// potential is a linear combination of node potentials (a difference
/// between the port's terminals). Evaluating it on each unit vector of the
/// node unknowns finds the nodes it depends on; their common domain is the
/// port's domain. Returns `None` when the port does not exist, depends on
/// no node (both terminals grounded), or mixes domains.
pub fn port_domain(circuit: &Circuit, element: usize, port: usize) -> Option<Domain> {
    let e = circuit.elements.get(element)?;
    let zero = C64::new(0.0, 0.0);
    let mut x = vec![zero; circuit.dim];
    let mut found: Option<Domain> = None;
    for j in 0..circuit.nodes.len() {
        x[j] = C64::new(1.0, 0.0);
        let v = e.port_potential(&x, port);
        x[j] = zero;
        if v? != zero {
            let d = circuit.nodes.domain(j);
            match found {
                None => found = Some(d),
                Some(f) if f != d => return None,
                Some(_) => {}
            }
        }
    }
    found
}
