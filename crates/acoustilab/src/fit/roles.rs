//! What the fitted parameters are, physically, and the identifiability
//! rules of spec Section 12 that follow from it.
//!
//! A parameter is linked to the element keys whose `=expression` names it,
//! directly or through derived parameters ([`elements`]). The keys of
//! transducer elements (`driver`, `motor`, `suspension`, `piston`, `coil`)
//! give a parameter a driver role ([`DriverRole`]).
//!
//! **The impedance-only rule.** The electrical impedance of a moving-coil
//! driver without acoustic load is
//! Z = Re + Z_L + Bl²/(jωMms + Rms + 1/(jωCms)), which is unchanged by
//! Bl → α·Bl, Mms → α²·Mms, Cms → Cms/α², Rms → α²·Rms for any α > 0: it
//! determines only Bl²/Mms, Bl²·Cms and Bl²/Rms (with Re and the
//! inductance terms). An acoustic load enters as Sd²·Z_a and keeps the
//! invariance when Sd → α·Sd as well. In the primary set (fs, Qms, Qes, Re,
//! Mms, Sd) the same transformation moves Mms by α² and Sd by α with fs,
//! Qms, Qes and Re fixed. Any fit whose free parameters span this
//! direction ([`scale_direction`]) and whose data are impedance curves only
//! cannot fix α, whatever the numerics say: a single impedance curve in a
//! modelled load pins α only through the load model. Such parameters are
//! marked scale-ambiguous, unless the data include a datum that fixes the
//! scale ([`Resolver`]):
//!
//! * `added_mass`: impedance curves under two conditions that differ in a
//!   known mass on the diaphragm (a `mass` element on the driver's
//!   mechanical node, not fitted). Unreliable below about 0.5 g of moving
//!   mass, where a 10 mg adhesive dot is 3 % of it ([`ADDED_MASS_MIN_MMS_KG`]).
//! * `known_volume`: impedance curves under two conditions that differ in a
//!   cavity on the driver's face (free air and a sealed box of known
//!   volume), with Sd not fitted (it must be measured).
//! * `displacement`: a displacement or velocity curve of the diaphragm at a
//!   stated drive (a laser transfer function).
//! * `spl_known_load`: a pressure curve at a stated drive with an absolute
//!   level, with Sd not fitted.
//!
//! **SPL-only fits.** A fit to pressure curves alone that frees any driver
//! parameter is refused: the level of a pressure response scales with
//! Bl·Sd/(Re·|Z_m|) and with the microphone calibration, so pressure data
//! cannot separate the driver's parameters from each other or from the
//! load (spec Section 12). The caller may override the refusal.

use crate::error::{Error, Result};
use crate::expr::{Expr, PValue};
use crate::params::{Overrides, ParamKind, Parametric};
use crate::units::unit_suffix;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Moving mass below which the added-mass method is flagged (spec
/// Section 12: "about half a gram").
pub const ADDED_MASS_MIN_MMS_KG: f64 = 0.5e-3;

/// A parameter that feeds an element key.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Link {
    pub param: String,
    /// Key path inside the element, e.g. `Mms_g` or `mesh.R_s_rayl`.
    pub key: String,
    /// The value is exactly `=param`.
    pub direct: bool,
}

/// An enabled element under one set of overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementInfo {
    pub id: String,
    pub ty: String,
    /// Terminal node names as resolved.
    pub nodes: Vec<String>,
    /// Keys the element sets (top level, as written).
    pub keys: Vec<String>,
    pub links: Vec<Link>,
}

fn collect_names(v: &Value, path: &str, out: &mut Vec<(String, String, bool)>) {
    match v {
        Value::String(s) => {
            if let Some(src) = s.strip_prefix('=') {
                if let Ok(e) = Expr::parse(src) {
                    let direct = e.as_name().is_some();
                    for n in e.names() {
                        out.push((n, path.to_string(), direct));
                    }
                }
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                collect_names(x, &format!("{path}[{i}]"), out);
            }
        }
        Value::Object(o) => {
            for (k, x) in o {
                collect_names(x, &format!("{path}.{k}"), out);
            }
        }
        _ => {}
    }
}

/// Base (non-derived) parameters a name depends on.
fn base_params(
    p: &Parametric,
    name: &str,
    seen: &mut BTreeSet<String>,
    out: &mut BTreeSet<String>,
) {
    if !seen.insert(name.to_string()) {
        return;
    }
    let Some(def) = p.def(name) else {
        return;
    };
    match &def.kind {
        ParamKind::Derived { expr } => {
            for n in expr.names() {
                base_params(p, &n, seen, out);
            }
        }
        _ => {
            out.insert(name.to_string());
        }
    }
}

/// The enabled elements of `p` under `overrides`, with their resolved
/// nodes and the parameters feeding each key.
pub fn elements(p: &Parametric, overrides: &Overrides) -> Result<Vec<ElementInfo>> {
    let resolved = p.resolve(overrides)?;
    let values: BTreeMap<&str, &PValue> = resolved
        .values
        .iter()
        .map(|(n, v)| (n.as_str(), v))
        .collect();
    let lookup = |n: &str| values.get(n).map(|v| (*v).clone());
    let raw = p
        .doc
        .get("elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let expanded = resolved
        .doc
        .get("elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut next = expanded.iter();
    for item in &raw {
        let Some(o) = item.as_object() else {
            next.next();
            continue;
        };
        let enabled = match o.get("enabled") {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) if s.starts_with('=') => Expr::parse(&s[1..])
                .ok()
                .and_then(|e| e.eval(&lookup).ok())
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            Some(_) => true,
        };
        if !enabled {
            continue;
        }
        let Some(ex) = next.next().and_then(Value::as_object) else {
            break;
        };
        let str_of = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
        let nodes: Vec<String> = match (ex.get("nodes"), ex.get("node")) {
            (Some(Value::Array(a)), _) => a
                .iter()
                .map(|x| x.as_str().unwrap_or("").to_string())
                .collect(),
            (_, Some(Value::String(s))) => vec![s.clone()],
            _ => Vec::new(),
        };
        let mut names = Vec::new();
        for (k, v) in o {
            if ["id", "type", "enabled", "nodes", "node"].contains(&k.as_str()) {
                continue;
            }
            collect_names(v, k, &mut names);
        }
        let mut links = Vec::new();
        for (n, key, direct) in names {
            let mut bases = BTreeSet::new();
            base_params(p, &n, &mut BTreeSet::new(), &mut bases);
            let is_base = bases.len() == 1 && bases.contains(&n);
            for b in bases {
                links.push(Link {
                    param: b,
                    key: key.clone(),
                    direct: direct && is_base,
                });
            }
        }
        out.push(ElementInfo {
            id: str_of(ex.get("id")),
            ty: str_of(ex.get("type")),
            nodes,
            keys: o
                .keys()
                .filter(|k| !["id", "type", "enabled", "nodes", "node"].contains(&k.as_str()))
                .cloned()
                .collect(),
            links,
        });
    }
    Ok(out)
}

/// Key name without its unit suffix (`Mms_g` → `Mms`, `Qms` → `Qms`).
pub fn key_base(key: &str) -> &str {
    match unit_suffix(key) {
        Some((s, _, _)) => &key[..key.len() - s.len() - 1],
        None => key,
    }
}

/// Role of a driver key in the scale transformation of the module
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverRole {
    Bl,
    Mms,
    /// Compliance Cms (or stiffness Kms).
    Cms,
    Kms,
    Rms,
    Sd,
    /// A key impedance determines directly (fs, Qms, Qes, Re, Le, creep, ...).
    Electrical,
}

impl DriverRole {
    /// Exponent of α in the scale transformation.
    pub fn exponent(self) -> f64 {
        match self {
            DriverRole::Bl | DriverRole::Sd => 1.0,
            DriverRole::Mms | DriverRole::Rms | DriverRole::Kms => 2.0,
            DriverRole::Cms => -2.0,
            DriverRole::Electrical => 0.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DriverRole::Bl => "Bl",
            DriverRole::Mms => "Mms",
            DriverRole::Cms => "Cms",
            DriverRole::Kms => "Kms",
            DriverRole::Rms => "Rms",
            DriverRole::Sd => "Sd",
            DriverRole::Electrical => "electrical",
        }
    }
}

/// Transducer element types whose keys are driver parameters.
pub const TRANSDUCER_TYPES: [&str; 5] = ["driver", "motor", "suspension", "piston", "coil"];

/// Driver role of `key` on an element of type `ty`, if it is a transducer
/// key.
pub fn driver_role(ty: &str, key: &str) -> Option<DriverRole> {
    if !TRANSDUCER_TYPES.contains(&ty) {
        return None;
    }
    let base = key_base(key.split(['.', '[']).next().unwrap_or(key));
    Some(match (ty, base) {
        ("driver" | "motor", "Bl") => DriverRole::Bl,
        ("driver" | "suspension", "Mms") => DriverRole::Mms,
        ("driver" | "suspension", "Cms") => DriverRole::Cms,
        ("driver" | "suspension", "Kms") => DriverRole::Kms,
        ("driver" | "suspension", "Rms") => DriverRole::Rms,
        ("driver" | "piston", "Sd") => DriverRole::Sd,
        _ => DriverRole::Electrical,
    })
}

/// A fitted parameter's role on one transducer element.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoleInfo {
    pub param: String,
    pub element: String,
    pub key: String,
    pub role: DriverRole,
}

/// Data that fix the scale of Bl, Mms and Cms (module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolver {
    AddedMass,
    KnownVolume,
    Displacement,
    SplKnownLoad,
}

impl Resolver {
    pub fn name(self) -> &'static str {
        match self {
            Resolver::AddedMass => "added_mass",
            Resolver::KnownVolume => "known_volume",
            Resolver::Displacement => "displacement",
            Resolver::SplKnownLoad => "spl_known_load",
        }
    }
}

/// What the rules need to know about one fitted curve.
#[derive(Debug, Clone)]
pub struct CurveUse {
    pub kind: DataKind,
    /// Overrides of the curve's condition (the fit's plus the curve's own).
    pub overrides: Overrides,
    /// The level is absolute: a stated drive and no free level offset.
    pub absolute: bool,
    /// Node of a displacement or velocity probe.
    pub node: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Impedance,
    Pressure,
    Mechanical,
    Other,
}

/// A finding of the rules, reported with the fit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub parameters: Vec<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resolve_with: Vec<String>,
}

/// Result of [`analyse`].
#[derive(Debug, Clone, Default)]
pub struct RuleReport {
    pub roles: Vec<RoleInfo>,
    /// Why the fit must be refused (SPL-only), if it must.
    pub refusal: Option<String>,
    /// Parameters the rules mark as scale-ambiguous.
    pub scale_ambiguous: Vec<String>,
    pub resolvers: Vec<Resolver>,
    pub findings: Vec<Finding>,
}

const RESOLVE_WITH: [&str; 4] = [
    "added mass: a second impedance curve with a known mass fixed to the diaphragm (a 'mass' element on the driver's mechanical node), fitted jointly",
    "known volume: impedance in free air and on a sealed box of known volume, with Sd measured (not fitted)",
    "laser: a diaphragm displacement or velocity transfer function at a stated drive",
    "a calibrated SPL curve at a stated drive in a known load, with Sd known",
];

fn mech_nodes(e: &ElementInfo) -> Vec<String> {
    match e.ty.as_str() {
        "driver" => vec![format!("{}.m", e.id), format!("{}.m2", e.id)],
        "motor" | "piston" => e.nodes.get(2).cloned().into_iter().collect(),
        "suspension" => e.nodes.first().cloned().into_iter().collect(),
        _ => Vec::new(),
    }
}

fn acoustic_nodes(e: &ElementInfo) -> Vec<String> {
    let is_ground = |n: &str| crate::netlist::ground_domain(n).is_some();
    match e.ty.as_str() {
        "driver" => e
            .nodes
            .iter()
            .skip(2)
            .filter(|n| !is_ground(n))
            .cloned()
            .collect(),
        "piston" => e
            .nodes
            .iter()
            .skip(2)
            .filter(|n| !is_ground(n))
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

/// Differing parameters between two override sets.
fn differing(a: &Overrides, b: &Overrides) -> BTreeSet<String> {
    let mut d = BTreeSet::new();
    for k in a.keys().chain(b.keys()) {
        if a.get(k) != b.get(k) {
            d.insert(k.clone());
        }
    }
    d
}

/// The fitted parameters of one driver element that span the scale
/// direction, or `None` if the fit cannot move along it (some parameter the
/// transformation needs is fixed). With the primary set only Mms (and Sd)
/// carry the scale; with the physical set, every one of Bl, Mms, Cms or
/// Kms, and Rms that the element sets must be free.
pub fn scale_direction(e: &ElementInfo, fitted: &[String]) -> Option<Vec<(String, DriverRole)>> {
    let mut free: Vec<(String, DriverRole)> = Vec::new();
    for l in &e.links {
        if !fitted.contains(&l.param) {
            continue;
        }
        if let Some(r) = driver_role(&e.ty, &l.key) {
            if r != DriverRole::Electrical && !free.iter().any(|(p, _)| p == &l.param) {
                free.push((l.param.clone(), r));
            }
        }
    }
    let has_role = |r: DriverRole| free.iter().any(|(_, x)| *x == r);
    let sets: Vec<DriverRole> = e
        .keys
        .iter()
        .filter_map(|k| driver_role(&e.ty, k))
        .collect();
    let primary = e.keys.iter().any(|k| key_base(k) == "fs");
    let needed: Vec<DriverRole> = if primary {
        vec![DriverRole::Mms]
    } else {
        [
            DriverRole::Bl,
            DriverRole::Mms,
            DriverRole::Cms,
            DriverRole::Kms,
            DriverRole::Rms,
        ]
        .into_iter()
        .filter(|r| sets.contains(r))
        .collect()
    };
    if needed.is_empty() || !needed.iter().all(|r| has_role(*r)) {
        return None;
    }
    Some(free)
}

/// Applies the rules of the module documentation.
pub fn analyse(
    p: &Parametric,
    fitted: &[String],
    curves: &[CurveUse],
    allow_spl_only: bool,
) -> Result<RuleReport> {
    let mut rep = RuleReport::default();
    // Element views per distinct condition.
    let mut conditions: Vec<(Overrides, Vec<ElementInfo>)> = Vec::new();
    for c in curves {
        if !conditions.iter().any(|(o, _)| o == &c.overrides) {
            conditions.push((c.overrides.clone(), elements(p, &c.overrides)?));
        }
    }
    let view = |o: &Overrides| -> &Vec<ElementInfo> {
        &conditions
            .iter()
            .find(|(x, _)| x == o)
            .expect("collected")
            .1
    };
    // Roles of the fitted parameters (union over conditions).
    for (_, els) in &conditions {
        for e in els {
            for l in &e.links {
                if !fitted.contains(&l.param) {
                    continue;
                }
                if let Some(role) = driver_role(&e.ty, &l.key) {
                    let r = RoleInfo {
                        param: l.param.clone(),
                        element: e.id.clone(),
                        key: l.key.clone(),
                        role,
                    };
                    if !rep.roles.contains(&r) {
                        rep.roles.push(r);
                    }
                }
            }
        }
    }
    let driver_params: BTreeSet<&str> = rep.roles.iter().map(|r| r.param.as_str()).collect();
    let kinds: BTreeSet<DataKind> = curves.iter().map(|c| c.kind).collect();
    let spl_only = !curves.is_empty() && kinds.iter().all(|k| *k == DataKind::Pressure);
    if spl_only && !driver_params.is_empty() {
        let names: Vec<String> = driver_params.iter().map(|s| s.to_string()).collect();
        let msg = format!(
            "refusing to fit driver parameters ({}) to pressure curves alone: a pressure response scales with Bl·Sd/(Re·|Zm|) and with the microphone calibration, so it cannot separate the driver's parameters from each other or from the load (spec Section 12). Fit an impedance curve jointly, or fix the driver parameters",
            names.join(", ")
        );
        if allow_spl_only {
            rep.findings.push(Finding {
                code: "spl_only_override",
                parameters: names,
                message: format!(
                    "{msg}. The refusal was overridden: treat the driver values as undetermined"
                ),
                resolve_with: Vec::new(),
            });
        } else {
            rep.refusal = Some(msg);
            return Ok(rep);
        }
    }
    let sd_fitted = rep.roles.iter().any(|r| r.role == DriverRole::Sd);
    // Resolvers present in the data.
    let imp: Vec<&CurveUse> = curves
        .iter()
        .filter(|c| c.kind == DataKind::Impedance)
        .collect();
    let add = |r: Resolver, list: &mut Vec<Resolver>| {
        if !list.contains(&r) {
            list.push(r);
        }
    };
    for (i, a) in imp.iter().enumerate() {
        for b in &imp[i + 1..] {
            let diff = differing(&a.overrides, &b.overrides);
            if diff.is_empty() {
                continue;
            }
            for o in [&a.overrides, &b.overrides] {
                let els = view(o);
                let mech: Vec<String> = els.iter().flat_map(mech_nodes).collect();
                let acoustic: Vec<String> = els.iter().flat_map(acoustic_nodes).collect();
                for e in els {
                    let changed = e
                        .links
                        .iter()
                        .any(|l| diff.contains(&l.param) && !fitted.contains(&l.param));
                    if !changed {
                        continue;
                    }
                    if e.ty == "mass" && e.nodes.iter().any(|n| mech.contains(n)) {
                        add(Resolver::AddedMass, &mut rep.resolvers);
                    }
                    let cavity =
                        ["cavity", "acoustic_compliance", "modal_cavity"].contains(&e.ty.as_str());
                    let on_face = e.nodes.iter().any(|n| acoustic.contains(n))
                        || (e.ty == "modal_cavity" && !acoustic.is_empty());
                    if cavity && on_face && !sd_fitted {
                        add(Resolver::KnownVolume, &mut rep.resolvers);
                    }
                }
            }
        }
    }
    for c in curves {
        match c.kind {
            DataKind::Mechanical if c.absolute => {
                let els = view(&c.overrides);
                let mech: Vec<String> = els.iter().flat_map(mech_nodes).collect();
                if c.node.as_ref().is_some_and(|n| mech.contains(n)) {
                    add(Resolver::Displacement, &mut rep.resolvers);
                }
            }
            DataKind::Pressure if c.absolute && !sd_fitted => {
                add(Resolver::SplKnownLoad, &mut rep.resolvers)
            }
            _ => {}
        }
    }
    // Scale directions inside the fitted set, per driver element.
    let mut ambiguous: Vec<(String, Vec<(String, DriverRole)>)> = Vec::new();
    for (_, els) in &conditions {
        for e in els
            .iter()
            .filter(|e| TRANSDUCER_TYPES.contains(&e.ty.as_str()))
        {
            if ambiguous.iter().any(|(id, _)| id == &e.id) {
                continue;
            }
            if let Some(dir) = scale_direction(e, fitted) {
                ambiguous.push((e.id.clone(), dir));
            }
        }
    }
    let resolvers_text = rep
        .resolvers
        .iter()
        .map(|r| r.name())
        .collect::<Vec<_>>()
        .join(", ");
    for (id, dir) in &ambiguous {
        let params: Vec<String> = dir.iter().map(|(p, _)| p.clone()).collect();
        let roles: Vec<&str> = dir.iter().map(|(_, r)| r.name()).collect();
        if rep.resolvers.is_empty() {
            let impedance_only = kinds.iter().all(|k| *k == DataKind::Impedance);
            let data = if impedance_only {
                "impedance data alone"
            } else {
                "these data (no scale datum among them)"
            };
            rep.findings.push(Finding {
                code: "scale_ambiguous",
                parameters: params.clone(),
                message: format!(
                    "driver '{id}': {data} determine only Bl²/Mms, Bl²·Cms and Bl²/Rms (with Re and the inductance), so {} ({}) are scale-ambiguous: scaling Bl by α, Mms and Rms by α², Cms by 1/α² (and Sd by α) leaves the impedance unchanged. A single impedance in a modelled acoustic load pins α only through the load model and Sd, so the values are not treated as determined",
                    params.join(", "),
                    roles.join(", ")
                ),
                resolve_with: RESOLVE_WITH.iter().map(|s| s.to_string()).collect(),
            });
            for p in params {
                if !rep.scale_ambiguous.contains(&p) {
                    rep.scale_ambiguous.push(p);
                }
            }
        } else {
            rep.findings.push(Finding {
                code: "scale_resolved",
                parameters: params,
                message: format!(
                    "driver '{id}': the scale of Bl, Mms and Cms is fixed by the data ({resolvers_text}); the numerical analysis below says how well"
                ),
                resolve_with: Vec::new(),
            });
        }
    }
    Ok(rep)
}

/// Moving mass of each driver in kg, from a parameter that sets its Mms key
/// directly, under `values` (the fitted or resolved parameter values).
pub fn moving_mass(
    els: &[ElementInfo],
    values: &dyn Fn(&str) -> Option<f64>,
) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for e in els {
        if !["driver", "suspension"].contains(&e.ty.as_str()) {
            continue;
        }
        for l in &e.links {
            if l.direct && key_base(&l.key) == "Mms" {
                if let (Some(v), Some((_, _, f))) = (values(&l.param), unit_suffix(&l.key)) {
                    out.push((e.id.clone(), v * f));
                }
            }
        }
    }
    out
}

/// Masses of `mass` elements set directly by a parameter, in kg, keyed by
/// element id.
pub fn test_masses(
    els: &[ElementInfo],
    values: &dyn Fn(&str) -> Option<f64>,
) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for e in els.iter().filter(|e| e.ty == "mass") {
        for l in &e.links {
            if l.direct && key_base(&l.key) == "M" {
                if let (Some(v), Some((_, _, f))) = (values(&l.param), unit_suffix(&l.key)) {
                    out.push((e.id.clone(), v * f));
                }
            }
        }
    }
    out
}

/// Error for a parameter the fit cannot use.
pub fn param_error(name: &str, msg: impl Into<String>) -> Error {
    Error::Parameter {
        name: name.to_string(),
        msg: msg.into(),
    }
}
