//! Validation against the open reference headphone (spec Section 17, "Open
//! reference headphone" and "Validation under limited fixture access";
//! docs/reference-cup.md).
//!
//! A **protocol** (`acoustilab-validation-protocol/0.1`, e.g.
//! `validation/reference_cup/protocol.json`) lists the configurations a
//! measurement session must cover, each as netlist overrides with the
//! measurements taken in it (a probe, a reference point, a number of
//! seatings). From it:
//!
//! * [`predict`] freezes the blind predictions: for every measurement the
//!   nominal curve and a Monte Carlo envelope over the tolerances, as CSV
//!   with sidecars, plus a manifest that records the engine, the commit, the
//!   date, the command and the SHA-256 of every file
//!   (`acoustilab-frozen-predictions/0.1`). [`predict::verify`] checks a
//!   frozen set against its manifest and [`predict::drift`] reports, without
//!   failing, how far the current engine has moved from it.
//! * [`session`] imports a directory of measured curves (FRD, ZMA, REW text,
//!   CSV, each with a sidecar), checks every sidecar against the protocol,
//!   averages the seatings, compares them with the frozen predictions, and
//!   applies the acceptance test: after fitting only the leak and the front
//!   volume, the residual stays within the protocol's bounds (2 dB to 1 kHz,
//!   4 dB from 1 to 4 kHz).
//! * [`simulate`] writes a synthetic session with the virtual rig
//!   ([`crate::fit::rig`]), so that the whole pipeline runs before a rig
//!   exists.
//!
//! Every function works on in-memory file sets (name to content), so that
//! the command line and a browser can both drive it.

pub mod predict;
pub mod session;
pub mod simulate;

use crate::expr::PValue;
use crate::fit::FitError;
use crate::io::curve::exchange_grid;
use crate::io::CurveError;
use crate::params::{Overrides, Parametric};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fmt;

/// Schema tag of validation protocols.
pub const PROTOCOL_SCHEMA: &str = "acoustilab-validation-protocol/0.1";

/// A set of files: name to content.
pub type Files = BTreeMap<String, Vec<u8>>;

/// An error of the validation pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError(pub String);

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ValidationError {}

impl From<crate::Error> for ValidationError {
    fn from(e: crate::Error) -> Self {
        ValidationError(e.to_string())
    }
}

impl From<CurveError> for ValidationError {
    fn from(e: CurveError) -> Self {
        ValidationError(format!("curve: {e}"))
    }
}

impl From<FitError> for ValidationError {
    fn from(e: FitError) -> Self {
        ValidationError(e.to_string())
    }
}

pub type VResult<T> = std::result::Result<T, ValidationError>;

fn err<T>(msg: impl Into<String>) -> VResult<T> {
    Err(ValidationError(msg.into()))
}

/// Rejects keys of `o` outside `known`.
fn check_keys(o: &Map<String, Value>, known: &[&str], ctx: &str) -> VResult<()> {
    match o.keys().find(|k| !known.contains(&k.as_str())) {
        Some(k) => err(format!(
            "{ctx}: unknown key '{k}' (known: {})",
            known.join(", ")
        )),
        None => Ok(()),
    }
}

fn obj<'a>(v: &'a Value, ctx: &str) -> VResult<&'a Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| ValidationError(format!("{ctx} must be a JSON object")))
}

fn num(o: &Map<String, Value>, k: &str, ctx: &str) -> VResult<f64> {
    o.get(k)
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .ok_or_else(|| ValidationError(format!("{ctx}: '{k}' must be a finite number")))
}

fn text(o: &Map<String, Value>, k: &str, ctx: &str) -> VResult<String> {
    o.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ValidationError(format!("{ctx}: '{k}' must be a string")))
}

fn opt_text(o: &Map<String, Value>, k: &str, ctx: &str) -> VResult<Option<String>> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => err(format!("{ctx}: '{k}' must be a string")),
    }
}

fn overrides(v: Option<&Value>, ctx: &str) -> VResult<Overrides> {
    match v {
        None => Ok(Overrides::new()),
        Some(v) => obj(v, ctx)?
            .iter()
            .map(|(k, x)| {
                PValue::from_json(x).map(|p| (k.clone(), p)).ok_or_else(|| {
                    ValidationError(format!("{ctx}: '{k}' must be a number, boolean or string"))
                })
            })
            .collect(),
    }
}

/// JSON object of overrides.
pub fn overrides_json(o: &Overrides) -> Value {
    Value::Object(o.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
}

/// Identifiers become file names: lower-case letters, digits and `_`.
fn check_id(id: &str, ctx: &str) -> VResult<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return err(format!(
            "{ctx}: id '{id}' must be lower-case letters, digits and '_' (it names files)"
        ));
    }
    Ok(())
}

/// The prediction grid: the exchange grid (1 kHz·2^(k/n), spec Section 14)
/// between two frequencies.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub f_min: f64,
    pub f_max: f64,
    pub points_per_octave: f64,
}

impl Grid {
    pub fn freqs(&self) -> Vec<f64> {
        exchange_grid(self.f_min, self.f_max, self.points_per_octave)
    }
}

/// A parameter the acceptance test (or the driver anchor) fits, with bounds
/// narrower than the declaration's.
#[derive(Debug, Clone, PartialEq)]
pub struct FitParamSpec {
    pub name: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FitSettings {
    pub parameters: Vec<FitParamSpec>,
    pub f_min: f64,
    pub f_max: f64,
}

/// An acceptance band: the largest |residual| allowed between two
/// frequencies (both included).
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub f_min: f64,
    pub f_max: f64,
    pub max_abs_db: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Acceptance {
    pub source: String,
    pub bands: Vec<Band>,
    /// Residuals above this frequency are reported without a bound.
    pub report_above: f64,
}

/// Fitting the driver to its own free-air impedance (not the spec's
/// criterion: a second, separately reported acceptance run that tells a
/// driver sample off its datasheet from a model error).
#[derive(Debug, Clone, PartialEq)]
pub struct DriverAnchor {
    pub measurement: String,
    pub parameters: Vec<FitParamSpec>,
}

/// One measured quantity of a configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// Names the files: `<id>_s<k>.<ext>` for seating k.
    pub id: String,
    pub probe: String,
    /// `drp`, `terminals`, ...
    pub reference_point: String,
}

/// A state of the device on a fixture.
#[derive(Debug, Clone, PartialEq)]
pub struct Configuration {
    pub id: String,
    pub label: String,
    /// Netlist overrides of this configuration (on top of the protocol's).
    pub overrides: Overrides,
    /// Sidecar fields every measured file must state (fixture, ear
    /// simulator, pinna).
    pub sidecar: Map<String, Value>,
    pub seatings: u32,
    /// A state of the reference headphone as the spec defines it: its
    /// acceptance decides the verdict.
    pub reference: bool,
    /// The acceptance test applies to its pressure measurements.
    pub acceptance: bool,
    /// Band of the prediction and of every comparison.
    pub band: (f64, f64),
    pub instructions: Option<String>,
    pub measurements: Vec<Measurement>,
}

/// A validation protocol (module documentation).
#[derive(Debug, Clone, PartialEq)]
pub struct Protocol {
    pub title: String,
    pub description: Option<String>,
    pub licence: Option<String>,
    /// Netlist path relative to the protocol file.
    pub netlist: String,
    pub grid: Grid,
    /// Overrides of every configuration (drive, source impedance, level).
    pub overrides: Overrides,
    pub mc_runs: usize,
    pub mc_seed: u64,
    pub source_impedance_max_ohm: f64,
    pub fit: FitSettings,
    pub acceptance: Acceptance,
    pub driver_anchor: Option<DriverAnchor>,
    pub configurations: Vec<Configuration>,
}

fn fit_params(v: Option<&Value>, ctx: &str) -> VResult<Vec<FitParamSpec>> {
    let a = v
        .and_then(Value::as_array)
        .ok_or_else(|| ValidationError(format!("{ctx}: 'parameters' must be an array")))?;
    a.iter()
        .map(|p| match p {
            Value::String(s) => Ok(FitParamSpec {
                name: s.clone(),
                min: None,
                max: None,
            }),
            _ => {
                let o = obj(p, ctx)?;
                check_keys(o, &["name", "min", "max"], ctx)?;
                Ok(FitParamSpec {
                    name: text(o, "name", ctx)?,
                    min: o.get("min").map(|_| num(o, "min", ctx)).transpose()?,
                    max: o.get("max").map(|_| num(o, "max", ctx)).transpose()?,
                })
            }
        })
        .collect()
}

impl Protocol {
    /// Reads a protocol. Unknown keys are errors.
    pub fn parse(text_: &str) -> VResult<Protocol> {
        let v: Value =
            serde_json::from_str(text_).map_err(|e| ValidationError(format!("protocol: {e}")))?;
        let o = obj(&v, "protocol")?;
        check_keys(
            o,
            &[
                "schema",
                "title",
                "description",
                "licence",
                "netlist",
                "grid",
                "overrides",
                "monte_carlo",
                "source_impedance_max_ohm",
                "fit",
                "acceptance",
                "driver_anchor",
                "configurations",
            ],
            "protocol",
        )?;
        if o.get("schema").and_then(Value::as_str) != Some(PROTOCOL_SCHEMA) {
            return err(format!("protocol: 'schema' must be '{PROTOCOL_SCHEMA}'"));
        }
        let g = obj(o.get("grid").unwrap_or(&Value::Null), "protocol grid")?;
        check_keys(
            g,
            &["f_min_Hz", "f_max_Hz", "points_per_octave"],
            "protocol grid",
        )?;
        let grid = Grid {
            f_min: num(g, "f_min_Hz", "protocol grid")?,
            f_max: num(g, "f_max_Hz", "protocol grid")?,
            points_per_octave: num(g, "points_per_octave", "protocol grid")?,
        };
        if grid.freqs().len() < 2 {
            return err("protocol grid: fewer than two frequencies");
        }
        let mc = obj(
            o.get("monte_carlo").unwrap_or(&Value::Null),
            "protocol monte_carlo",
        )?;
        check_keys(mc, &["n", "seed"], "protocol monte_carlo")?;
        let mc_runs = mc
            .get("n")
            .and_then(Value::as_u64)
            .filter(|n| (1..=crate::analysis::mc::MAX_RUNS as u64).contains(n))
            .ok_or_else(|| {
                ValidationError("protocol monte_carlo: 'n' must be 1 to 100000".into())
            })? as usize;
        let mc_seed = mc.get("seed").and_then(Value::as_u64).ok_or_else(|| {
            ValidationError("protocol monte_carlo: 'seed' must be an unsigned integer".into())
        })?;
        let f = obj(o.get("fit").unwrap_or(&Value::Null), "protocol fit")?;
        check_keys(f, &["parameters", "f_min_Hz", "f_max_Hz"], "protocol fit")?;
        let fit = FitSettings {
            parameters: fit_params(f.get("parameters"), "protocol fit")?,
            f_min: num(f, "f_min_Hz", "protocol fit")?,
            f_max: num(f, "f_max_Hz", "protocol fit")?,
        };
        let a = obj(
            o.get("acceptance").unwrap_or(&Value::Null),
            "protocol acceptance",
        )?;
        check_keys(
            a,
            &["source", "bands", "report_above_Hz"],
            "protocol acceptance",
        )?;
        let bands = a
            .get("bands")
            .and_then(Value::as_array)
            .ok_or_else(|| ValidationError("protocol acceptance: 'bands' must be an array".into()))?
            .iter()
            .map(|b| {
                let bo = obj(b, "acceptance band")?;
                check_keys(
                    bo,
                    &["f_min_Hz", "f_max_Hz", "max_abs_dB"],
                    "acceptance band",
                )?;
                Ok(Band {
                    f_min: num(bo, "f_min_Hz", "acceptance band")?,
                    f_max: num(bo, "f_max_Hz", "acceptance band")?,
                    max_abs_db: num(bo, "max_abs_dB", "acceptance band")?,
                })
            })
            .collect::<VResult<Vec<_>>>()?;
        let acceptance = Acceptance {
            source: text(a, "source", "protocol acceptance")?,
            bands,
            report_above: num(a, "report_above_Hz", "protocol acceptance")?,
        };
        let driver_anchor = match o.get("driver_anchor") {
            None | Some(Value::Null) => None,
            Some(d) => {
                let d = obj(d, "protocol driver_anchor")?;
                check_keys(d, &["measurement", "parameters"], "protocol driver_anchor")?;
                Some(DriverAnchor {
                    measurement: text(d, "measurement", "protocol driver_anchor")?,
                    parameters: fit_params(d.get("parameters"), "protocol driver_anchor")?,
                })
            }
        };
        let configurations = o
            .get("configurations")
            .and_then(Value::as_array)
            .ok_or_else(|| ValidationError("protocol: 'configurations' must be an array".into()))?
            .iter()
            .map(|c| parse_configuration(c, &grid))
            .collect::<VResult<Vec<_>>>()?;
        let p = Protocol {
            title: text(o, "title", "protocol")?,
            description: opt_text(o, "description", "protocol")?,
            licence: opt_text(o, "licence", "protocol")?,
            netlist: text(o, "netlist", "protocol")?,
            grid,
            overrides: overrides(o.get("overrides"), "protocol overrides")?,
            mc_runs,
            mc_seed,
            source_impedance_max_ohm: num(o, "source_impedance_max_ohm", "protocol")?,
            fit,
            acceptance,
            driver_anchor,
            configurations,
        };
        p.check_ids()?;
        Ok(p)
    }

    fn check_ids(&self) -> VResult<()> {
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.configurations {
            if !seen.insert(format!("c:{}", c.id)) {
                return err(format!(
                    "protocol: configuration '{}' is listed twice",
                    c.id
                ));
            }
            for m in &c.measurements {
                if !seen.insert(format!("m:{}", m.id)) {
                    return err(format!("protocol: measurement '{}' is listed twice", m.id));
                }
            }
        }
        if let Some(a) = &self.driver_anchor {
            if self.measurement(&a.measurement).is_none() {
                return err(format!(
                    "protocol driver_anchor: no measurement '{}'",
                    a.measurement
                ));
            }
        }
        Ok(())
    }

    /// Every measurement with its configuration, in protocol order.
    pub fn measurements(&self) -> impl Iterator<Item = (&Configuration, &Measurement)> {
        self.configurations
            .iter()
            .flat_map(|c| c.measurements.iter().map(move |m| (c, m)))
    }

    pub fn measurement(&self, id: &str) -> Option<(&Configuration, &Measurement)> {
        self.measurements().find(|(_, m)| m.id == id)
    }

    /// The protocol's overrides with the configuration's on top.
    pub fn overrides_for(&self, c: &Configuration) -> Overrides {
        let mut o = self.overrides.clone();
        o.extend(c.overrides.clone());
        o
    }

    /// Checks the protocol against its netlist: every configuration's
    /// overrides are valid and every measured probe exists there.
    pub fn check_against(&self, p: &Parametric) -> VResult<()> {
        for c in &self.configurations {
            let circuit = crate::Circuit::from_parametric(p, &self.overrides_for(c))
                .map_err(|e| ValidationError(format!("configuration '{}': {e}", c.id)))?;
            for m in &c.measurements {
                if !circuit.probes.iter().any(|q| q.id == m.probe) {
                    return err(format!(
                        "configuration '{}': measurement '{}' needs probe '{}', which the netlist does not have in this configuration",
                        c.id, m.id, m.probe
                    ));
                }
            }
        }
        for f in &self.fit.parameters {
            if p.def(&f.name).is_none() {
                return err(format!("protocol fit: no parameter '{}'", f.name));
            }
        }
        if let Some(a) = &self.driver_anchor {
            for f in &a.parameters {
                if p.def(&f.name).is_none() {
                    return err(format!("protocol driver_anchor: no parameter '{}'", f.name));
                }
            }
        }
        Ok(())
    }
}

fn parse_configuration(v: &Value, grid: &Grid) -> VResult<Configuration> {
    let o = obj(v, "configuration")?;
    let id = text(o, "id", "configuration")?;
    let ctx = format!("configuration '{id}'");
    check_id(&id, &ctx)?;
    check_keys(
        o,
        &[
            "id",
            "label",
            "overrides",
            "sidecar",
            "seatings",
            "reference",
            "acceptance",
            "band_Hz",
            "instructions",
            "measurements",
        ],
        &ctx,
    )?;
    let sidecar = match o.get("sidecar") {
        None => Map::new(),
        Some(s) => {
            let s = obj(s, &ctx)?;
            check_keys(s, &["fixture", "ear_simulator", "pinna"], &ctx)?;
            if s.values().any(|x| !x.is_string()) {
                return err(format!("{ctx}: sidecar fields must be strings"));
            }
            s.clone()
        }
    };
    let flag = |k: &str| -> VResult<bool> {
        match o.get(k) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => err(format!("{ctx}: '{k}' must be true or false")),
        }
    };
    let band = match o.get("band_Hz") {
        None => (grid.f_min, grid.f_max),
        Some(b) => {
            let a = b.as_array().filter(|a| a.len() == 2).ok_or_else(|| {
                ValidationError(format!("{ctx}: 'band_Hz' must be [f_min, f_max]"))
            })?;
            let lo = a[0].as_f64().unwrap_or(f64::NAN);
            let hi = a[1].as_f64().unwrap_or(f64::NAN);
            if !(lo > 0.0 && hi > lo) {
                return err(format!("{ctx}: 'band_Hz' needs 0 < f_min < f_max"));
            }
            (lo, hi)
        }
    };
    let seatings = o
        .get("seatings")
        .and_then(Value::as_u64)
        .filter(|n| (1..=100).contains(n))
        .ok_or_else(|| ValidationError(format!("{ctx}: 'seatings' must be 1 to 100")))?
        as u32;
    let measurements = o
        .get("measurements")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| ValidationError(format!("{ctx}: 'measurements' must be a non-empty array")))?
        .iter()
        .map(|m| {
            let mo = obj(m, &ctx)?;
            check_keys(mo, &["id", "probe", "reference_point"], &ctx)?;
            let id = text(mo, "id", &ctx)?;
            check_id(&id, &ctx)?;
            Ok(Measurement {
                id,
                probe: text(mo, "probe", &ctx)?,
                reference_point: text(mo, "reference_point", &ctx)?,
            })
        })
        .collect::<VResult<Vec<_>>>()?;
    Ok(Configuration {
        label: text(o, "label", &ctx)?,
        overrides: overrides(o.get("overrides"), &ctx)?,
        sidecar,
        seatings,
        reference: flag("reference")?,
        acceptance: flag("acceptance")?,
        band,
        instructions: opt_text(o, "instructions", &ctx)?,
        measurements,
        id,
    })
}

/// The netlist with its sweep replaced by `freqs`, parsed. The netlist
/// document is otherwise unchanged.
pub fn netlist_on_grid(netlist_text: &str, freqs: &[f64]) -> VResult<Parametric> {
    let mut v: Value =
        serde_json::from_str(netlist_text).map_err(|e| ValidationError(format!("netlist: {e}")))?;
    let o = v
        .as_object_mut()
        .ok_or_else(|| ValidationError("netlist: not a JSON object".into()))?;
    o.insert("sweep".into(), json!({ "frequencies_Hz": freqs }));
    Ok(Parametric::parse(&v.to_string())?)
}

/// Lower-case hex SHA-256 of bytes.
pub fn sha256_hex(data: &[u8]) -> String {
    crate::analysis::canonical::hex(&crate::analysis::canonical::sha256(data))
}

/// Shading band (0 none, 1 light, 2 dark) as a CSV number.
fn shade(s: u8) -> &'static str {
    match s {
        0 => "0",
        1 => "1",
        _ => "2",
    }
}

/// Where a measured file of `measurement` for seating `k` is expected:
/// `<id>_s<k>` plus one of the curve extensions.
pub fn seating_stem(measurement: &str, k: u32) -> String {
    format!("{measurement}_s{k}")
}

/// Curve file extensions a session may use.
pub const CURVE_EXTENSIONS: [&str; 4] = ["frd", "zma", "txt", "csv"];

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
      "schema": "acoustilab-validation-protocol/0.1", "title": "t", "netlist": "n.json",
      "grid": {"f_min_Hz": 100, "f_max_Hz": 1000, "points_per_octave": 3},
      "monte_carlo": {"n": 10, "seed": 1}, "source_impedance_max_ohm": 1,
      "fit": {"parameters": ["a"], "f_min_Hz": 20, "f_max_Hz": 4000},
      "acceptance": {"source": "s", "bands": [{"f_min_Hz": 20, "f_max_Hz": 1000, "max_abs_dB": 2}], "report_above_Hz": 4000},
      "configurations": [{"id": "c1", "label": "C", "seatings": 5,
        "measurements": [{"id": "c1_p", "probe": "p", "reference_point": "drp"}]}]}"#;

    #[test]
    fn protocol_parses_and_rejects_unknown_keys_and_bad_ids() {
        let p = Protocol::parse(MINIMAL).unwrap();
        assert_eq!(p.configurations[0].band, (100.0, 1000.0));
        assert_eq!(p.grid.freqs().len(), 10);
        assert_eq!(p.measurements().count(), 1);
        let bad = MINIMAL.replace("\"title\": \"t\"", "\"title\": \"t\", \"extra\": 1");
        assert!(Protocol::parse(&bad)
            .unwrap_err()
            .0
            .contains("unknown key 'extra'"));
        let bad = MINIMAL.replace("\"id\": \"c1_p\"", "\"id\": \"C1 p\"");
        assert!(Protocol::parse(&bad).unwrap_err().0.contains("names files"));
        let bad = MINIMAL.replace("\"id\": \"c1_p\"", "\"id\": \"c1\"");
        assert!(
            Protocol::parse(&bad).is_ok(),
            "a measurement may share a configuration's id"
        );
        let twice = MINIMAL.replace(
            "\"measurements\": [{\"id\": \"c1_p\", \"probe\": \"p\", \"reference_point\": \"drp\"}]",
            "\"measurements\": [{\"id\": \"c1_p\", \"probe\": \"p\", \"reference_point\": \"drp\"}, {\"id\": \"c1_p\", \"probe\": \"q\", \"reference_point\": \"drp\"}]",
        );
        assert!(Protocol::parse(&twice)
            .unwrap_err()
            .0
            .contains("listed twice"));
    }
}
