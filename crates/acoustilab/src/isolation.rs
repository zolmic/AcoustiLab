//! Passive isolation and bleed (spec Section 10, "Passive isolation" and
//! "Bleed"; Section 8, leak model). See `docs/isolation.md`.
//!
//! **Convention.** Every terminal named `ambient` or `a_amb` is the
//! outside air. `gnd`, `a_gnd` and omitted terminals remain the reference
//! (zero acoustic pressure), as for a cavity's compliance or an ear
//! simulator's internals. In a normal solve the two coincide; for isolation
//! they differ.
//!
//! **Occluded ear.** The expanded netlist (after parameter resolution) is
//! transformed: the ambient terminals are rewired to a new node
//! `iso_outside`, driven by a 1 Pa pressure source to the reference (a
//! diffuse field simplified to one blocked pressure, equal in phase at
//! every opening); every independent source is set to zero with its source
//! impedance kept (a `vsource` keeps `Zs_ohm`, so the driver sees its
//! amplifier); the `drive` key is dropped. The drum-point pressure is
//! p_occluded.
//!
//! **Open ear.** The same ear load driven at its entrance node by the same
//! 1 Pa, with nothing else attached: p_open. The ear load is every element
//! reachable from the drum-point node without crossing the entrance node.
//! This ignores head and pinna diffraction and the radiation impedance of
//! the open entrance: the unoccluded entrance pressure is taken to be the
//! outside pressure.
//!
//! **Insertion loss** IL(f) = 20·log10(|p_open|/|p_occluded|), positive
//! when the headphone attenuates. Because the ear is a one-port at its
//! entrance, the ear's own transfer cancels, and IL is the attenuation of
//! the entrance pressure. Following ETSI TS 103 640 V1.4.1 (2026-04),
//! clause 5.1.3, it is also reported in 1/3-octave bands from 20 Hz to
//! 20 kHz (base-ten bands of IEC 61260-1, band powers ∫|p|² df of a flat
//! excitation over each band fully inside the sweep), with the summary of
//! clause 5.1.1: the largest loss and its band, the range where the loss is
//! at least 6 dB, and the mean over the bands. The netlist models one ear.
//!
//! **Paths.** By superposition, each element with an ambient terminal is
//! driven alone (the others' ambient terminals held at the reference); the
//! contributions sum to p_occluded.
//!
//! **Bleed.** At the netlist's drive, the volume velocity U leaving through
//! the ambient terminals (read from a 0 Pa source on the rewired outside
//! node, which leaves the solution unchanged) radiates as a monopole:
//! |p(r)| = ρ·f·|U|/(2r) (spec Appendix C9), given with a ±6 dB band. The
//! dipole correction of C9 needs the separation of the front and rear
//! openings, which the netlist does not carry; the complex sum over
//! openings already includes their cancellation.

use crate::circuit::Circuit;
use crate::drive::DriveInfo;
use crate::error::{Error, Result};
use crate::expr::PValue;
use crate::netlist::Document;
use crate::params::{Overrides, Parametric};
use crate::validity::Shading;
use crate::C64;
use serde::Serialize;
use serde_json::{json, Map, Value};

/// Terminal names that denote the outside air.
pub const AMBIENT: [&str; 2] = ["ambient", "a_amb"];
/// Terminal names that remain the reference.
pub const REFERENCE: [&str; 2] = ["gnd", "a_gnd"];
/// Name of the rewired outside node.
pub const OUTSIDE_NODE: &str = "iso_outside";
/// Id of the pressure source that drives it.
pub const OUTSIDE_SOURCE: &str = "iso_ambient";
/// Outside pressure, Pa RMS.
pub const AMBIENT_PA: f64 = 1.0;
/// ETSI TS 103 640 clause 5.1.1: the range where the loss is at least this.
pub const RANGE_THRESHOLD_DB: f64 = 6.0;
/// Half-width of the bleed band, dB (spec Section 10).
pub const BLEED_BAND_DB: f64 = 6.0;

/// Element types that form a path to the outside air when one of their
/// terminals is ambient, with the terminal count at which none is omitted.
const PATH_TYPES: &[(&str, usize)] = &[
    ("leak", 2),
    ("vent", 2),
    ("radiation", 2),
    ("mesh", 2),
    ("perforated_plate", 2),
    ("membrane_vent", 2),
    ("porous_layer", 2),
    ("shell", 2),
    ("tube", 2),
    ("slit", 2),
    ("rect_duct", 2),
    ("acoustic_resistance", 2),
    ("acoustic_inertance", 2),
    ("acoustic_impedance", 2),
    ("driver", 4),
];

/// Independent sources and the (base key, dimension) of their value.
const SOURCES: &[(&str, &str, crate::units::Dim)] = &[
    ("vsource", "V", crate::units::Dim::Voltage),
    ("isource", "I", crate::units::Dim::Current),
    ("flow_source", "U", crate::units::Dim::VolumeVelocity),
    ("pressure_source", "p", crate::units::Dim::Pressure),
    ("force_source", "F", crate::units::Dim::Force),
];

#[derive(Debug, Clone)]
pub struct IsolationOptions {
    /// Drum-point pressure probe (default: `ui.primary_probe`).
    pub drum_probe: Option<String>,
    /// Ear-entrance node (default: the node the ear-load element connects
    /// to; see [`find_ear`]).
    pub entrance_node: Option<String>,
    /// Solve each ambient path alone as well.
    pub paths: bool,
}

impl Default for IsolationOptions {
    fn default() -> Self {
        IsolationOptions {
            drum_probe: None,
            entrance_node: None,
            paths: true,
        }
    }
}

/// A remark about the transformation.
#[derive(Debug, Clone, Serialize)]
pub struct IsoWarning {
    pub code: &'static str,
    pub element: Option<String>,
    pub message: String,
}

/// The ear load identified in a netlist.
#[derive(Debug, Clone, Serialize)]
pub struct EarLoad {
    pub drum_probe: String,
    pub drum_node: String,
    pub entrance_node: String,
    /// Elements between the entrance and the drum point.
    pub elements: Vec<String>,
}

/// One 1/3-octave band.
#[derive(Debug, Clone, Serialize)]
pub struct Band {
    /// Nominal mid-band frequency, Hz.
    #[serde(rename = "nominal_Hz")]
    pub nominal_hz: f64,
    /// Exact mid-band frequency 1000·10^(x/10), Hz.
    #[serde(rename = "center_Hz")]
    pub center_hz: f64,
    #[serde(rename = "insertion_loss_dB")]
    pub insertion_loss_db: f64,
}

/// ETSI TS 103 640 clause 5.1.1 summary over the bands.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    #[serde(rename = "max_dB")]
    pub max_db: f64,
    #[serde(rename = "max_at_Hz")]
    pub max_at_hz: f64,
    /// Lowest and highest band with a loss of at least 6 dB.
    #[serde(rename = "range_6dB_Hz")]
    pub range_6db_hz: Option<(f64, f64)>,
    #[serde(rename = "mean_dB")]
    pub mean_db: f64,
}

/// One path's contribution to the occluded drum pressure.
#[derive(Debug, Clone)]
pub struct PathResult {
    pub element: String,
    pub p: Vec<C64>,
}

/// The fixture's self-insertion loss and where a prediction exceeds it.
#[derive(Debug, Clone, Serialize)]
pub struct FixtureNote {
    pub fixture: String,
    pub bands: Value,
    pub source: String,
    pub url: String,
    pub status: String,
    /// Frequencies where the predicted loss exceeds the fixture's stated
    /// self-insertion loss: a measurement on it could not confirm them.
    #[serde(rename = "exceeded_at_Hz")]
    pub exceeded_at_hz: Vec<f64>,
}

#[derive(Debug, Clone)]
pub struct Isolation {
    pub freqs_hz: Vec<f64>,
    pub ear: EarLoad,
    pub p_occluded: Vec<C64>,
    pub p_open: Vec<C64>,
    pub insertion_loss_db: Vec<f64>,
    pub bands: Vec<Band>,
    pub summary: Option<Summary>,
    /// Elements with an ambient terminal.
    pub driven: Vec<String>,
    pub paths: Vec<PathResult>,
    pub warnings: Vec<IsoWarning>,
    pub shading: Shading,
    pub parameters: Value,
    pub fixture: FixtureNote,
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Terminal names of an element record (`nodes`, `node`, and the `node` of
/// each `modal_cavity` port).
pub fn terminals(obj: &Map<String, Value>) -> Vec<String> {
    let mut t = str_list(obj.get("nodes"));
    t.extend(str_list(obj.get("node")));
    if let Some(Value::Array(ports)) = obj.get("ports") {
        for p in ports {
            t.extend(str_list(p.get("node")));
        }
    }
    t
}

fn is_ambient(name: &str) -> bool {
    AMBIENT.contains(&name)
}

/// Rewires the ambient terminals of one element record to `to`.
fn rewire(obj: &mut Map<String, Value>, to: &str) -> bool {
    let mut changed = false;
    let mut fix = |v: &mut Value| {
        if v.as_str().is_some_and(is_ambient) {
            *v = Value::String(to.to_string());
            changed = true;
        }
    };
    match obj.get_mut("nodes") {
        Some(Value::Array(a)) => a.iter_mut().for_each(&mut fix),
        Some(v) => fix(v),
        None => {}
    }
    if let Some(v) = obj.get_mut("node") {
        fix(v);
    }
    if let Some(Value::Array(ports)) = obj.get_mut("ports") {
        for p in ports {
            if let Some(v) = p.get_mut("node") {
                fix(v);
            }
        }
    }
    changed
}

/// Sets an independent source's value to zero (its impedance stays).
fn zero_source(obj: &mut Map<String, Value>) {
    let ty = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    if let Some((_, base, dim)) = SOURCES.iter().find(|(t, _, _)| *t == ty) {
        for (suffix, _) in dim.suffixes() {
            obj.remove(&format!("{base}_{suffix}"));
        }
        obj.insert(format!("{base}_{}", dim.suffixes()[0].0), json!(0.0));
    }
}

fn elements(doc: &Value) -> &[Value] {
    doc.get("elements")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn id_of(v: &Value) -> &str {
    v.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn type_of(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or_default()
}

/// Elements with an ambient terminal.
pub fn driven_elements(doc: &Value) -> Vec<String> {
    elements(doc)
        .iter()
        .filter(|e| {
            e.as_object()
                .is_some_and(|o| terminals(o).iter().any(|t| is_ambient(t)))
        })
        .map(|e| id_of(e).to_string())
        .collect()
}

/// Warnings for path elements ending at the reference, which isolation
/// does not drive.
pub fn undriven_paths(doc: &Value) -> Vec<IsoWarning> {
    let mut out = Vec::new();
    for e in elements(doc) {
        let Some(o) = e.as_object() else { continue };
        let ty = type_of(e);
        if ty == "porous_layer" && o.get("backing").and_then(Value::as_str) == Some("rigid") {
            continue;
        }
        if let Some((_, count)) = PATH_TYPES.iter().find(|(t, _)| *t == ty) {
            let t = str_list(o.get("nodes"))
                .into_iter()
                .chain(str_list(o.get("node")));
            let t: Vec<String> = t.collect();
            let omitted = t.len() < *count;
            let at_reference = t
                .iter()
                .skip(if ty == "driver" { 2 } else { 0 })
                .any(|n| REFERENCE.contains(&n.as_str()));
            if omitted || at_reference {
                out.push(IsoWarning {
                    code: "undriven_path",
                    element: Some(id_of(e).to_string()),
                    message: format!(
                        "{ty} '{}' ends at the reference ({}), which isolation does not drive; name its outer terminal 'ambient' if it opens to the outside air",
                        id_of(e),
                        if omitted { "an omitted terminal" } else { "'gnd' or 'a_gnd'" }
                    ),
                });
            }
        }
        if ty == "modal_cavity" {
            if let Some(Value::Array(ports)) = o.get("ports") {
                if ports.iter().any(|p| {
                    p.get("node")
                        .and_then(Value::as_str)
                        .is_some_and(|n| REFERENCE.contains(&n))
                }) {
                    out.push(IsoWarning {
                        code: "undriven_path",
                        element: Some(id_of(e).to_string()),
                        message: format!(
                            "a port of modal_cavity '{}' opens to the reference, which isolation does not drive; name it 'ambient' if it opens to the outside air",
                            id_of(e)
                        ),
                    });
                }
            }
        }
    }
    out
}

fn fresh_name(doc: &Value, base: &str) -> String {
    let taken = |n: &str| {
        doc.get("nodes")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|x| id_of(x) == n))
            || elements(doc).iter().any(|e| id_of(e) == n)
    };
    let mut name = base.to_string();
    let mut i = 1;
    while taken(&name) {
        name = format!("{base}{i}");
        i += 1;
    }
    name
}

/// The netlist with the ambient terminals of every element (or only of
/// `only`) rewired to a new node driven by `pressure` Pa, the independent
/// sources zeroed when `zero_sources`, the `drive` key removed (the netlist
/// now has a second source), and only `probe` kept when given. Returns the
/// document, the node name and the source id.
pub fn outside_document(
    resolved: &Value,
    probe: Option<&str>,
    only: Option<&str>,
    pressure: f64,
    zero_sources: bool,
) -> Result<(Value, String, String)> {
    let mut doc = resolved.clone();
    let node = fresh_name(&doc, OUTSIDE_NODE);
    let src = fresh_name(&doc, OUTSIDE_SOURCE);
    let top = doc
        .as_object_mut()
        .ok_or_else(|| Error::Netlist("top level must be an object".into()))?;
    top.remove("drive");
    if let Some(Value::Array(els)) = top.get_mut("elements") {
        for e in els.iter_mut() {
            let id = id_of(e).to_string();
            if let Some(o) = e.as_object_mut() {
                if only.is_none_or(|x| x == id) {
                    rewire(o, &node);
                }
                if zero_sources {
                    zero_source(o);
                }
            }
        }
        els.push(json!({"id": src, "type": "pressure_source", "nodes": [node], "p_Pa": pressure}));
    }
    match top.get_mut("nodes") {
        Some(Value::Array(ns)) => ns.push(json!({"id": node, "domain": "acoustic"})),
        _ => {
            top.insert("nodes".into(), json!([{"id": node, "domain": "acoustic"}]));
        }
    }
    if let Some(p) = probe {
        if let Some(Value::Array(ps)) = top.get_mut("probes") {
            ps.retain(|x| id_of(x) == p);
        }
    }
    Ok((doc, node, src))
}

/// Drum node of a pressure probe.
fn drum_node(doc: &Value, probe: &str) -> Result<String> {
    let p = doc
        .get("probes")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| id_of(x) == probe))
        .ok_or_else(|| Error::Probe {
            id: probe.to_string(),
            msg: "no such probe".into(),
        })?;
    if p.get("quantity").and_then(Value::as_str) != Some("pressure") {
        return Err(Error::Probe {
            id: probe.to_string(),
            msg: "the drum point needs a 'pressure' probe".into(),
        });
    }
    p.get("node")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Probe {
            id: probe.to_string(),
            msg: "the drum probe needs a 'node'".into(),
        })
}

fn is_ground(n: &str) -> bool {
    crate::netlist::ground_domain(n).is_some()
}

/// Identifies the ear load: the drum probe (`drum_probe`, else
/// `ui.primary_probe`), its node, the entrance node (`entrance`, else: the
/// first terminal of the element whose internal node the drum node is,
/// e.g. `ear` for `ear.drp`; else the entrance of a `canal` ending at the
/// drum node), and every element reachable from the drum node without
/// crossing the entrance.
pub fn find_ear(doc: &Value, drum_probe: Option<&str>, entrance: Option<&str>) -> Result<EarLoad> {
    let probe = match drum_probe {
        Some(p) => p.to_string(),
        None => doc
            .get("ui")
            .and_then(|u| u.get("primary_probe"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                Error::Netlist(
                    "isolation: name the drum-point probe (the netlist has no ui.primary_probe)"
                        .into(),
                )
            })?,
    };
    let drum = drum_node(doc, &probe)?;
    let els = elements(doc);
    let owner = |n: &str| {
        els.iter()
            .find(|e| n.strip_prefix(id_of(e)).is_some_and(|r| r.starts_with('.')))
    };
    let entrance = match entrance {
        Some(e) => e.to_string(),
        None => {
            let from_owner = owner(&drum)
                .and_then(|e| e.as_object().and_then(|o| terminals(o).into_iter().next()));
            let from_canal = || {
                els.iter().filter(|e| type_of(e) == "canal").find_map(|e| {
                    let t = e.as_object().map(terminals).unwrap_or_default();
                    (t.get(1) == Some(&drum)).then(|| t[0].clone())
                })
            };
            from_owner.or_else(from_canal).ok_or_else(|| {
                Error::Netlist(format!(
                    "isolation: cannot tell the ear entrance for drum node '{drum}'; name the entrance node"
                ))
            })?
        }
    };
    if is_ground(&entrance) {
        return Err(Error::Netlist(format!(
            "isolation: the ear entrance '{entrance}' is a ground name"
        )));
    }
    // Reachability from the drum node, not crossing the entrance.
    let belongs = |node: &str, e: &Value| {
        e.as_object()
            .is_some_and(|o| terminals(o).iter().any(|t| t == node))
            || node
                .strip_prefix(id_of(e))
                .is_some_and(|r| r.starts_with('.'))
    };
    let mut nodes: Vec<String> = Vec::new();
    if drum != entrance {
        nodes.push(drum.clone());
    }
    let mut visited: Vec<usize> = Vec::new();
    loop {
        let mut grew = false;
        for (i, e) in els.iter().enumerate() {
            if visited.contains(&i) {
                continue;
            }
            let touches = nodes.iter().any(|n| belongs(n, e))
                || e.as_object().is_some_and(|o| {
                    terminals(o).iter().any(|t| {
                        visited.iter().any(|&v| {
                            t.strip_prefix(id_of(&els[v]))
                                .is_some_and(|r| r.starts_with('.'))
                        })
                    })
                });
            if touches {
                visited.push(i);
                grew = true;
                for t in e.as_object().map(terminals).unwrap_or_default() {
                    if t != entrance && !is_ground(&t) && !nodes.contains(&t) {
                        nodes.push(t);
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }
    for &i in &visited {
        let e = &els[i];
        let t = e.as_object().map(terminals).unwrap_or_default();
        if SOURCES.iter().any(|(ty, _, _)| *ty == type_of(e))
            || type_of(e) == "driver"
            || t.iter().any(|n| is_ambient(n))
        {
            return Err(Error::Netlist(format!(
                "isolation: the entrance node '{entrance}' does not separate the ear from the headphone: '{}' is reachable from the drum point",
                id_of(e)
            )));
        }
    }
    Ok(EarLoad {
        drum_probe: probe,
        drum_node: drum,
        entrance_node: entrance,
        elements: visited
            .iter()
            .map(|&i| id_of(&els[i]).to_string())
            .collect(),
    })
}

/// The open-ear netlist: the ear-load elements driven at the entrance by
/// `pressure` Pa.
pub fn open_document(resolved: &Value, ear: &EarLoad, pressure: f64) -> Result<Value> {
    let els: Vec<Value> = elements(resolved)
        .iter()
        .filter(|e| ear.elements.iter().any(|x| x == id_of(e)))
        .cloned()
        .collect();
    let mut used: Vec<String> = vec![ear.entrance_node.clone()];
    for e in &els {
        used.extend(e.as_object().map(terminals).unwrap_or_default());
    }
    let nodes: Vec<Value> = resolved
        .get("nodes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|n| used.iter().any(|u| u == id_of(n)))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let probes: Vec<Value> = resolved
        .get("probes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|p| id_of(p) == ear.drum_probe)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut doc = Map::new();
    for k in ["schema", "air", "sweep", "level"] {
        if let Some(v) = resolved.get(k) {
            doc.insert(k.into(), v.clone());
        }
    }
    let mut all = els;
    let src = fresh_name(resolved, OUTSIDE_SOURCE);
    all.push(json!({"id": src, "type": "pressure_source", "nodes": [ear.entrance_node], "p_Pa": pressure}));
    doc.insert("nodes".into(), Value::Array(nodes));
    doc.insert("elements".into(), Value::Array(all));
    doc.insert("probes".into(), Value::Array(probes));
    Ok(Value::Object(doc))
}

fn compile(doc: Value, values: &[(String, PValue)]) -> Result<Circuit> {
    let mut c = Circuit::from_document(Document::from_expanded(doc)?)?;
    c.parameters = values.to_vec();
    Ok(c)
}

fn sweep_probe(c: &Circuit, probe: &str) -> Result<Vec<C64>> {
    let p = c
        .probes
        .iter()
        .find(|p| p.id == probe)
        .ok_or_else(|| Error::Probe {
            id: probe.to_string(),
            msg: "no such probe".into(),
        })?;
    c.freqs
        .iter()
        .map(|&f| {
            let x = c.solve_at(f)?;
            c.probe_value(p, f, &x)
        })
        .collect()
}

/// The occluded-ear circuit (for inspection, e.g. its energy balance).
pub fn occluded_circuit(
    p: &Parametric,
    overrides: &Overrides,
    opts: &IsolationOptions,
) -> Result<Circuit> {
    let r = p.resolve(overrides)?;
    let ear = find_ear(
        &r.doc,
        opts.drum_probe.as_deref(),
        opts.entrance_node.as_deref(),
    )?;
    let (doc, _, _) = outside_document(&r.doc, Some(&ear.drum_probe), None, AMBIENT_PA, true)?;
    compile(doc, &r.values)
}

/// Passive insertion loss at the drum (see the module docs).
pub fn insertion_loss(
    p: &Parametric,
    overrides: &Overrides,
    opts: &IsolationOptions,
) -> Result<Isolation> {
    let r = p.resolve(overrides)?;
    let ear = find_ear(
        &r.doc,
        opts.drum_probe.as_deref(),
        opts.entrance_node.as_deref(),
    )?;
    let driven = driven_elements(&r.doc);
    let mut warnings = undriven_paths(&r.doc);
    if driven.is_empty() {
        warnings.push(IsoWarning {
            code: "no_ambient",
            element: None,
            message: "no terminal is named 'ambient' or 'a_amb': nothing is driven from outside, so the insertion loss is infinite".into(),
        });
    }
    let (occ_doc, _, _) = outside_document(&r.doc, Some(&ear.drum_probe), None, AMBIENT_PA, true)?;
    let occ = compile(occ_doc, &r.values)?;
    let p_occ = sweep_probe(&occ, &ear.drum_probe)?;
    let open = compile(open_document(&r.doc, &ear, AMBIENT_PA)?, &r.values)?;
    let p_open = sweep_probe(&open, &ear.drum_probe)?;
    let freqs = occ.freqs.clone();
    let il: Vec<f64> = p_open
        .iter()
        .zip(&p_occ)
        .map(|(o, c)| 20.0 * (o.norm() / c.norm()).log10())
        .collect();
    let mut paths = Vec::new();
    if opts.paths && driven.len() > 1 {
        for e in &driven {
            let (doc, _, _) =
                outside_document(&r.doc, Some(&ear.drum_probe), Some(e), AMBIENT_PA, true)?;
            let c = compile(doc, &r.values)?;
            paths.push(PathResult {
                element: e.clone(),
                p: sweep_probe(&c, &ear.drum_probe)?,
            });
        }
    }
    let bands = third_octave_bands(&freqs, &p_open, &p_occ);
    let summary = summarise(&bands);
    let fixture = fixture_note(&freqs, &il);
    Ok(Isolation {
        freqs_hz: freqs,
        ear,
        p_occluded: p_occ,
        p_open,
        insertion_loss_db: il,
        bands,
        summary,
        driven,
        paths,
        warnings,
        shading: occ.shading(),
        parameters: Value::Object(
            r.values
                .iter()
                .map(|(n, v)| (n.clone(), v.to_json()))
                .collect(),
        ),
        fixture,
    })
}

/// ∫|p|² df over [lo, hi] by the trapezoid rule on the sweep, with |p|²
/// interpolated linearly at the band edges.
fn band_power(f: &[f64], p: &[C64], lo: f64, hi: f64) -> f64 {
    let at = |x: f64| {
        let k = f.iter().position(|&v| v >= x).unwrap_or(f.len() - 1).max(1);
        let t = (x - f[k - 1]) / (f[k] - f[k - 1]);
        p[k - 1].norm_sqr() * (1.0 - t) + p[k].norm_sqr() * t
    };
    let mut pts: Vec<(f64, f64)> = vec![(lo, at(lo))];
    pts.extend(
        f.iter()
            .zip(p)
            .filter(|(x, _)| **x > lo && **x < hi)
            .map(|(x, v)| (*x, v.norm_sqr())),
    );
    pts.push((hi, at(hi)));
    pts.windows(2)
        .map(|w| 0.5 * (w[0].1 + w[1].1) * (w[1].0 - w[0].0))
        .sum()
}

/// Preferred numbers of the R10 series (mantissas of the nominal
/// 1/3-octave mid-band frequencies).
const R10: [f64; 10] = [1.0, 1.25, 1.6, 2.0, 2.5, 3.15, 4.0, 5.0, 6.3, 8.0];

/// IL in the base-ten 1/3-octave bands (mid-band 1000·10^(x/10), edges at
/// 10^(±1/20) of it) from 20 Hz to 20 kHz that lie inside the sweep.
pub fn third_octave_bands(f: &[f64], p_open: &[C64], p_occ: &[C64]) -> Vec<Band> {
    let (f0, f1) = (f[0], f[f.len() - 1]);
    let mut out = Vec::new();
    for x in -17i32..=13 {
        let center = 1000.0 * 10f64.powf(x as f64 / 10.0);
        let (lo, hi) = (center * 10f64.powf(-0.05), center * 10f64.powf(0.05));
        if lo < f0 * (1.0 - 1e-9) || hi > f1 * (1.0 + 1e-9) || f.len() < 2 {
            continue;
        }
        let po = band_power(f, p_open, lo, hi);
        let pc = band_power(f, p_occ, lo, hi);
        let nominal = R10[x.rem_euclid(10) as usize] * 10f64.powi(x.div_euclid(10) + 3);
        out.push(Band {
            nominal_hz: nominal,
            center_hz: center,
            insertion_loss_db: 10.0 * (po / pc).log10(),
        });
    }
    out
}

fn summarise(bands: &[Band]) -> Option<Summary> {
    let best = bands
        .iter()
        .filter(|b| b.insertion_loss_db.is_finite())
        .max_by(|a, b| a.insertion_loss_db.total_cmp(&b.insertion_loss_db))?;
    let over: Vec<f64> = bands
        .iter()
        .filter(|b| b.insertion_loss_db >= RANGE_THRESHOLD_DB)
        .map(|b| b.nominal_hz)
        .collect();
    let finite: Vec<f64> = bands
        .iter()
        .map(|b| b.insertion_loss_db)
        .filter(|x| x.is_finite())
        .collect();
    Some(Summary {
        max_db: best.insertion_loss_db,
        max_at_hz: best.nominal_hz,
        range_6db_hz: over.first().zip(over.last()).map(|(a, b)| (*a, *b)),
        mean_db: finite.iter().sum::<f64>() / finite.len().max(1) as f64,
    })
}

/// The GRAS 45CA self-insertion loss (`data/fixtures`), and where `il`
/// exceeds it.
pub fn fixture_note(f: &[f64], il: &[f64]) -> FixtureNote {
    let rec: Value = serde_json::from_str(include_str!(
        "../../../data/fixtures/gras_45ca_self_insertion_loss.json"
    ))
    .expect("embedded fixture record");
    let bands = rec["bands"].clone();
    let exceeded = f
        .iter()
        .zip(il)
        .filter(|(f, il)| {
            bands.as_array().is_some_and(|b| {
                b.iter().any(|b| {
                    let lo = b["f_min_Hz"].as_f64().unwrap_or(f64::NAN);
                    let hi = b["f_max_Hz"].as_f64().unwrap_or(f64::NAN);
                    let db = b["exceeds_dB"].as_f64().unwrap_or(f64::NAN);
                    **f >= lo && **f <= hi && **il > db
                })
            })
        })
        .map(|(f, _)| *f)
        .collect();
    let prov = &rec["provenance"];
    FixtureNote {
        fixture: rec["fixture"].as_str().unwrap_or_default().to_string(),
        bands,
        source: prov["source"].as_str().unwrap_or_default().to_string(),
        url: prov["url"].as_str().unwrap_or_default().to_string(),
        status: prov["status"].as_str().unwrap_or_default().to_string(),
        exceeded_at_hz: exceeded,
    }
}

impl Isolation {
    /// Level of each path's contribution relative to the open-ear drum
    /// pressure, dB.
    pub fn path_levels_db(&self, path: &PathResult) -> Vec<f64> {
        path.p
            .iter()
            .zip(&self.p_open)
            .map(|(p, o)| 20.0 * (p.norm() / o.norm()).log10())
            .collect()
    }

    pub fn to_json(&self) -> Value {
        let cx = |v: &[C64]| {
            json!({
                "re": v.iter().map(|x| x.re).collect::<Vec<_>>(),
                "im": v.iter().map(|x| x.im).collect::<Vec<_>>(),
                "spl_dB": v.iter().map(|x| crate::drive::spl_db(*x)).collect::<Vec<_>>(),
            })
        };
        json!({
            "frequencies_Hz": self.freqs_hz,
            "ambient_Pa": AMBIENT_PA,
            "ear": self.ear,
            "insertion_loss_dB": self.insertion_loss_db,
            "p_occluded": cx(&self.p_occluded),
            "p_open": cx(&self.p_open),
            "third_octave_bands": self.bands,
            "summary": self.summary,
            "driven": self.driven,
            "paths": self.paths.iter().map(|p| json!({
                "element": p.element,
                "re": p.p.iter().map(|x| x.re).collect::<Vec<_>>(),
                "im": p.p.iter().map(|x| x.im).collect::<Vec<_>>(),
                "level_re_open_dB": self.path_levels_db(p),
            })).collect::<Vec<_>>(),
            "warnings": self.warnings,
            "shading": self.shading,
            "parameters": self.parameters,
            "fixture_self_insertion_loss": self.fixture,
            "convention": "IL = 20 log10(|p_open|/|p_occluded|) at the drum; 1 Pa at every 'ambient'/'a_amb' terminal (same phase); sources zeroed with their impedances kept; open ear = the ear load driven at its entrance by 1 Pa (no head or pinna diffraction)",
        })
    }
}

// ----- Bleed -----------------------------------------------------------------

/// Monopole bleed at the netlist's drive.
#[derive(Debug, Clone)]
pub struct Bleed {
    pub freqs_hz: Vec<f64>,
    /// Net volume velocity leaving through the ambient terminals, m³/s RMS.
    pub u_out: Vec<C64>,
    pub distances_m: Vec<f64>,
    /// dB SPL at each distance (outer index) and frequency.
    pub spl_db: Vec<Vec<f64>>,
    pub drive: DriveInfo,
}

/// Bleed estimate (see the module docs).
pub fn bleed(p: &Parametric, overrides: &Overrides, distances_m: &[f64]) -> Result<Bleed> {
    if distances_m.iter().any(|d| !(d.is_finite() && *d > 0.0)) {
        return Err(Error::Netlist("bleed: distances must be positive".into()));
    }
    let r = p.resolve(overrides)?;
    let original = compile(r.doc.clone(), &r.values)?;
    let (scale, drive) = original.drive_scale()?;
    // The 0 Pa source is a second independent source, so the drive key
    // goes: solve as written and apply the original drive's factor.
    let (doc, _, src) = outside_document(&r.doc, None, None, 0.0, false)?;
    let c = compile(doc, &r.values)?;
    let si = c
        .element_index(&src)
        .ok_or_else(|| Error::Netlist("bleed: outside source missing".into()))?;
    let scale = match scale {
        crate::solve::Scale::Current { amps, source } => {
            let id = original.elements[source].id().to_string();
            crate::solve::Scale::Current {
                amps,
                source: c.element_index(&id).unwrap_or(source),
            }
        }
        s => s,
    };
    let rho = c.air.rho;
    let mut u_out = Vec::with_capacity(c.freqs.len());
    for &f in &c.freqs {
        let x = c.solve_at(f)?;
        let k = scale.factor(&c, f, &x)?;
        let cx = c.cx(f);
        let delivered = c.elements[si]
            .port_flow(&cx, &x, &c.branches(si), 0)
            .unwrap_or_default();
        u_out.push(-delivered * k);
    }
    let spl_db = distances_m
        .iter()
        .map(|&d| {
            c.freqs
                .iter()
                .zip(&u_out)
                .map(|(f, u)| {
                    let pr = rho * f * u.norm() / (2.0 * d);
                    20.0 * (pr / crate::air::P_REF).log10()
                })
                .collect()
        })
        .collect();
    Ok(Bleed {
        freqs_hz: c.freqs.clone(),
        u_out,
        distances_m: distances_m.to_vec(),
        spl_db,
        drive,
    })
}

impl Bleed {
    pub fn to_json(&self) -> Value {
        json!({
            "frequencies_Hz": self.freqs_hz,
            "u_out": {
                "re": self.u_out.iter().map(|x| x.re).collect::<Vec<_>>(),
                "im": self.u_out.iter().map(|x| x.im).collect::<Vec<_>>(),
            },
            "distances_m": self.distances_m,
            "spl_dB": self.spl_db,
            "band_dB": BLEED_BAND_DB,
            "drive": self.drive,
            "model": "monopole |p| = rho f |U| / (2 r) of the net volume velocity leaving through the ambient terminals, +-6 dB (spec App. C9, without its dipole correction)",
        })
    }
}
