//! The metadata sidecar of a curve (spec Section 14, "Metadata sidecar")
//! with its measurement uncertainty budget (spec Section 12,
//! "Measurement uncertainty budget"), and the compatibility check that
//! refuses to compare curves measured under different conditions.
//!
//! Schema `acoustilab-curve-sidecar/0.1`. Every key but `schema` is
//! optional, keys carry their unit, and unknown keys are rejected:
//!
//! ```json
//! {"schema": "acoustilab-curve-sidecar/0.1",
//!  "quantity": "pressure", "calibrated": true,
//!  "fixture": "GRAS 45CA", "ear_simulator": "iec60318_4", "pinna": "KB5000",
//!  "seatings": 5, "averaging": "complex", "smoothing": "1/12",
//!  "drive": {"power_mW": 1, "rated_ohm": 32}, "source_impedance_ohm": 0,
//!  "compensation": "none", "reference_point": "drp",
//!  "temperature_C": 23, "date": "2026-09-25", "device": "prototype A, left",
//!  "provenance": {"origin": "measured", "source": "bench session 12", "licence": "internal"},
//!  "uncertainty": {"coupler_dB": 0.3, "microphone_calibration_dB": 0.2,
//!                  "repositioning_dB": 0.8, "noise_dB": 0.05, "phase_deg": 1.0},
//!  "notes": "..."}
//! ```
//!
//! `drive` uses the netlist's `drive` keys (docs/netlist.md, "Drive").
//! Uncertainty terms are standard uncertainties (one standard deviation),
//! each a number or a table `{"frequencies_Hz": [..], "values_dB": [..]}`
//! (`values_deg` for the phase) interpolated linearly in ln f and held
//! constant beyond its ends. `noise_dB`, `phase_deg` and `repositioning_dB`
//! are per seating: a curve averaged over N seatings carries them divided
//! by √N. The others are systematic and do not average down. The combined
//! level uncertainty is their root sum of squares ([`Uncertainty::level_db`]).

use super::curve::Quantity;
use super::CurveError;
use crate::drive::DriveSpec;
use serde_json::{json, Map, Value};

/// Schema tag of sidecar documents.
pub const SIDECAR_SCHEMA: &str = "acoustilab-curve-sidecar/0.1";

/// How the seatings of a measurement were averaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Averaging {
    /// A single seating.
    None,
    /// Power (RMS) average of the magnitudes, sqrt(mean |H_k|²).
    Magnitude,
    /// Mean of the levels in dB.
    Db,
    /// Vector average of the complex responses; loses treble where the
    /// seatings differ in phase.
    Complex,
}

impl Averaging {
    pub fn name(self) -> &'static str {
        match self {
            Averaging::None => "none",
            Averaging::Magnitude => "magnitude",
            Averaging::Db => "db",
            Averaging::Complex => "complex",
        }
    }

    pub fn parse(s: &str) -> Option<Averaging> {
        [
            Averaging::None,
            Averaging::Magnitude,
            Averaging::Db,
            Averaging::Complex,
        ]
        .into_iter()
        .find(|a| a.name() == s)
    }
}

/// Fractional-octave smoothing applied to a curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Smoothing {
    None,
    /// 1/N octave.
    Octave(u32),
}

impl Smoothing {
    pub fn name(self) -> String {
        match self {
            Smoothing::None => "none".into(),
            Smoothing::Octave(n) => format!("1/{n}"),
        }
    }

    /// Reads `none` or `1/N` (also REW's "1/12 octave", "No smoothing").
    pub fn parse(s: &str) -> Option<Smoothing> {
        let t = s.trim().to_ascii_lowercase();
        let t = t.strip_suffix("octave").unwrap_or(&t).trim();
        if t == "none" || t == "no smoothing" || t == "no" {
            return Some(Smoothing::None);
        }
        let n = t.strip_prefix("1/")?.trim().parse::<u32>().ok()?;
        (n > 0).then_some(Smoothing::Octave(n))
    }
}

/// Who produced a curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Measured,
    Simulated,
    /// Synthetic measurement from the virtual rig ([`crate::fit::rig`]).
    VirtualRig,
    Published,
    Digitized,
    User,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Origin::Measured => "measured",
            Origin::Simulated => "simulated",
            Origin::VirtualRig => "virtual_rig",
            Origin::Published => "published",
            Origin::Digitized => "digitized",
            Origin::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<Origin> {
        [
            Origin::Measured,
            Origin::Simulated,
            Origin::VirtualRig,
            Origin::Published,
            Origin::Digitized,
            Origin::User,
        ]
        .into_iter()
        .find(|o| o.name() == s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Provenance {
    pub origin: Option<Origin>,
    pub source: Option<String>,
    pub url: Option<String>,
    pub licence: Option<String>,
    /// Program that produced the file (e.g. "REW V5.31").
    pub tool: Option<String>,
}

/// The drive of a curve: the JSON as given (so it round-trips) and its
/// parsed form (for comparisons).
#[derive(Debug, Clone, PartialEq)]
pub struct Drive {
    pub json: Value,
    pub spec: DriveSpec,
}

impl Drive {
    pub fn from_spec(spec: &DriveSpec) -> Drive {
        let json = match spec {
            DriveSpec::Voltage(v) => json!({"voltage_V": v}),
            DriveSpec::Power { watts, rated_ohm } => {
                json!({"power_mW": watts * 1e3, "rated_ohm": rated_ohm})
            }
            DriveSpec::Characteristic {
                probe,
                level_db,
                f_hz,
            } => json!({"characteristic": probe, "level_dB": level_db, "f_Hz": f_hz}),
            DriveSpec::Current(a) => json!({"current_mA": a * 1e3}),
        };
        Drive {
            json,
            spec: spec.clone(),
        }
    }

    /// Short description, e.g. "1 mW into 32 ohm".
    pub fn label(&self) -> String {
        match &self.spec {
            DriveSpec::Voltage(v) => format!("{v} V RMS"),
            DriveSpec::Power { watts, rated_ohm } => {
                format!("{} mW into {rated_ohm} ohm", watts * 1e3)
            }
            DriveSpec::Characteristic { level_db, f_hz, .. } => {
                format!("characteristic voltage ({level_db} dB SPL at {f_hz} Hz)")
            }
            DriveSpec::Current(a) => format!("{} mA RMS", a * 1e3),
        }
    }

    /// Same convention and level (a characteristic drive compares its level
    /// and frequency, not the probe name, which is the model's).
    pub fn same_as(&self, other: &Drive) -> bool {
        let eq = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs());
        match (&self.spec, &other.spec) {
            (DriveSpec::Voltage(a), DriveSpec::Voltage(b)) => eq(*a, *b),
            (DriveSpec::Current(a), DriveSpec::Current(b)) => eq(*a, *b),
            (
                DriveSpec::Power {
                    watts: a,
                    rated_ohm: ra,
                },
                DriveSpec::Power {
                    watts: b,
                    rated_ohm: rb,
                },
            ) => eq(*a, *b) && eq(*ra, *rb),
            (
                DriveSpec::Characteristic {
                    level_db: la,
                    f_hz: fa,
                    ..
                },
                DriveSpec::Characteristic {
                    level_db: lb,
                    f_hz: fb,
                    ..
                },
            ) => eq(*la, *lb) && eq(*fa, *fb),
            _ => false,
        }
    }
}

/// One uncertainty term: constant, or tabulated against frequency.
#[derive(Debug, Clone, PartialEq)]
pub enum Profile {
    Constant(f64),
    Table { freqs: Vec<f64>, values: Vec<f64> },
}

impl Profile {
    /// Value at f: the table interpolated linearly in ln f, constant beyond
    /// its ends.
    pub fn at(&self, f: f64) -> f64 {
        match self {
            Profile::Constant(v) => *v,
            Profile::Table { freqs, values } => {
                if f <= freqs[0] {
                    return values[0];
                }
                let n = freqs.len();
                if f >= freqs[n - 1] {
                    return values[n - 1];
                }
                let i = freqs.partition_point(|&x| x <= f) - 1;
                let t = (f.ln() - freqs[i].ln()) / (freqs[i + 1].ln() - freqs[i].ln());
                values[i] + t * (values[i + 1] - values[i])
            }
        }
    }

    fn to_json(&self, unit: &str) -> Value {
        match self {
            Profile::Constant(v) => json!(v),
            Profile::Table { freqs, values } => {
                json!({"frequencies_Hz": freqs, format!("values_{unit}"): values})
            }
        }
    }

    fn from_json(key: &str, unit: &str, v: &Value) -> Result<Profile, CurveError> {
        let bad = |m: &str| CurveError::new(format!("sidecar uncertainty '{key}': {m}"));
        match v {
            Value::Number(n) => {
                let x = n.as_f64().unwrap_or(f64::NAN);
                if x.is_finite() && x >= 0.0 {
                    Ok(Profile::Constant(x))
                } else {
                    Err(bad("must be a non-negative number"))
                }
            }
            Value::Object(o) => {
                let vkey = format!("values_{unit}");
                if let Some(k) = o.keys().find(|k| *k != "frequencies_Hz" && **k != vkey) {
                    return Err(bad(&format!(
                        "unknown key '{k}' (a table has 'frequencies_Hz' and '{vkey}')"
                    )));
                }
                let nums = |k: &str| -> Result<Vec<f64>, CurveError> {
                    o.get(k)
                        .and_then(Value::as_array)
                        .and_then(|a| a.iter().map(Value::as_f64).collect::<Option<Vec<_>>>())
                        .ok_or_else(|| bad(&format!("'{k}' must be an array of numbers")))
                };
                let freqs = nums("frequencies_Hz")?;
                let values = nums(&vkey)?;
                if freqs.is_empty() || freqs.len() != values.len() {
                    return Err(bad("the table needs equal, non-empty arrays"));
                }
                if freqs.iter().any(|f| !(f.is_finite() && *f > 0.0))
                    || freqs.windows(2).any(|w| w[1] <= w[0])
                {
                    return Err(bad("table frequencies must be positive and increasing"));
                }
                if values.iter().any(|x| !(x.is_finite() && *x >= 0.0)) {
                    return Err(bad("table values must be non-negative"));
                }
                Ok(Profile::Table { freqs, values })
            }
            _ => Err(bad("must be a number or a table")),
        }
    }
}

/// Measurement uncertainty budget (spec Section 12): standard
/// uncertainties of the level in dB, and of the phase in degrees.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Uncertainty {
    /// Coupler or ear-simulator tolerance (from the standard's table, which
    /// the user supplies; the engine bundles none).
    pub coupler_db: Option<Profile>,
    /// Microphone (or sensor) calibration.
    pub microphone_calibration_db: Option<Profile>,
    /// Spread between seatings (one standard deviation of one seating).
    pub repositioning_db: Option<Profile>,
    /// Translation from the fixture to a human ear; include it only when the
    /// model's load is not the measurement fixture.
    pub fixture_to_human_db: Option<Profile>,
    /// Numerical error of a simulated curve.
    pub numerical_db: Option<Profile>,
    /// Random measurement noise of one seating.
    pub noise_db: Option<Profile>,
    /// Phase uncertainty of one seating, degrees.
    pub phase_deg: Option<Profile>,
}

const LEVEL_TERMS: [&str; 6] = [
    "coupler_dB",
    "microphone_calibration_dB",
    "repositioning_dB",
    "fixture_to_human_dB",
    "numerical_dB",
    "noise_dB",
];

impl Uncertainty {
    fn level_terms(&self) -> [(&'static str, &Option<Profile>, bool); 6] {
        // (key, term, averages down over seatings)
        [
            ("coupler_dB", &self.coupler_db, false),
            (
                "microphone_calibration_dB",
                &self.microphone_calibration_db,
                false,
            ),
            ("repositioning_dB", &self.repositioning_db, true),
            ("fixture_to_human_dB", &self.fixture_to_human_db, false),
            ("numerical_dB", &self.numerical_db, false),
            ("noise_dB", &self.noise_db, true),
        ]
    }

    /// Combined standard uncertainty of the level at f in dB for a curve
    /// averaged over `seatings`: the root sum of squares of the terms, with
    /// the per-seating terms divided by √seatings. `None` if no level term
    /// is given.
    pub fn level_db(&self, f: f64, seatings: u32) -> Option<f64> {
        let n = f64::from(seatings.max(1));
        let mut any = false;
        let mut s2 = 0.0;
        for (_, t, per_seating) in self.level_terms() {
            if let Some(p) = t {
                any = true;
                let v = p.at(f);
                s2 += if per_seating { v * v / n } else { v * v };
            }
        }
        any.then(|| s2.sqrt())
    }

    /// Standard uncertainty of the phase at f in degrees, averaged over
    /// `seatings`; `None` if not given.
    pub fn phase_deg(&self, f: f64, seatings: u32) -> Option<f64> {
        let n = f64::from(seatings.max(1));
        self.phase_deg.as_ref().map(|p| p.at(f) / n.sqrt())
    }

    pub fn is_empty(&self) -> bool {
        self.level_terms().iter().all(|(_, t, _)| t.is_none()) && self.phase_deg.is_none()
    }

    fn to_json(&self) -> Value {
        let mut o = Map::new();
        for (k, t, _) in self.level_terms() {
            if let Some(p) = t {
                o.insert(k.into(), p.to_json("dB"));
            }
        }
        if let Some(p) = &self.phase_deg {
            o.insert("phase_deg".into(), p.to_json("deg"));
        }
        Value::Object(o)
    }

    fn from_json(v: &Value) -> Result<Uncertainty, CurveError> {
        let o = v
            .as_object()
            .ok_or_else(|| CurveError::new("sidecar 'uncertainty' must be an object"))?;
        let mut u = Uncertainty::default();
        for (k, x) in o {
            let slot = match k.as_str() {
                "coupler_dB" => &mut u.coupler_db,
                "microphone_calibration_dB" => &mut u.microphone_calibration_db,
                "repositioning_dB" => &mut u.repositioning_db,
                "fixture_to_human_dB" => &mut u.fixture_to_human_db,
                "numerical_dB" => &mut u.numerical_db,
                "noise_dB" => &mut u.noise_db,
                "phase_deg" => &mut u.phase_deg,
                _ => {
                    return Err(CurveError::new(format!(
                        "sidecar uncertainty: unknown key '{k}' (known: {}, phase_deg)",
                        LEVEL_TERMS.join(", ")
                    )))
                }
            };
            let unit = if k == "phase_deg" { "deg" } else { "dB" };
            *slot = Some(Profile::from_json(k, unit, x)?);
        }
        Ok(u)
    }
}

/// The metadata sidecar (module documentation).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sidecar {
    pub quantity: Option<Quantity>,
    /// Unit of a generic quantity.
    pub unit: Option<String>,
    /// Pressure levels are absolute (dB SPL at the stated drive), not
    /// relative to an arbitrary reference.
    pub calibrated: Option<bool>,
    pub fixture: Option<String>,
    pub ear_simulator: Option<String>,
    pub pinna: Option<String>,
    pub seatings: Option<u32>,
    pub averaging: Option<Averaging>,
    pub smoothing: Option<Smoothing>,
    pub drive: Option<Drive>,
    pub source_impedance_ohm: Option<f64>,
    /// Compensation state: "none", "diffuse_field", "free_field", or a
    /// description of the compensation applied.
    pub compensation: Option<String>,
    /// "drp", "eep", "erp", "coupler", "terminals", ...
    pub reference_point: Option<String>,
    pub temperature_c: Option<f64>,
    pub date: Option<String>,
    /// Device identifier.
    pub device: Option<String>,
    pub provenance: Option<Provenance>,
    pub uncertainty: Option<Uncertainty>,
    /// Settings of the virtual rig that generated the curve.
    pub virtual_rig: Option<Value>,
    pub notes: Option<String>,
}

fn opt_string(o: &Map<String, Value>, k: &str, ctx: &str) -> Result<Option<String>, CurveError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(CurveError::new(format!("{ctx} '{k}' must be a string"))),
    }
}

fn opt_number(o: &Map<String, Value>, k: &str) -> Result<Option<f64>, CurveError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map(Some)
            .ok_or_else(|| CurveError::new(format!("sidecar '{k}' must be a finite number"))),
    }
}

const SIDECAR_KEYS: &[&str] = &[
    "schema",
    "quantity",
    "unit",
    "calibrated",
    "fixture",
    "ear_simulator",
    "pinna",
    "seatings",
    "averaging",
    "smoothing",
    "drive",
    "source_impedance_ohm",
    "compensation",
    "reference_point",
    "temperature_C",
    "date",
    "device",
    "provenance",
    "uncertainty",
    "virtual_rig",
    "notes",
];

impl Sidecar {
    /// An empty sidecar for a curve of `quantity`.
    pub fn for_quantity(quantity: Quantity) -> Sidecar {
        Sidecar {
            quantity: Some(quantity),
            ..Sidecar::default()
        }
    }

    /// Number of seatings averaged (1 if not stated).
    pub fn seatings_or_one(&self) -> u32 {
        self.seatings.unwrap_or(1).max(1)
    }

    pub fn parse(text: &str) -> Result<Sidecar, CurveError> {
        let v: Value = serde_json::from_str(text).map_err(|e| CurveError {
            line: Some(e.line()),
            msg: format!("sidecar JSON: {e}"),
        })?;
        Sidecar::from_json(&v)
    }

    pub fn from_json(v: &Value) -> Result<Sidecar, CurveError> {
        let o = v
            .as_object()
            .ok_or_else(|| CurveError::new("a sidecar must be a JSON object"))?;
        if let Some(k) = o.keys().find(|k| !SIDECAR_KEYS.contains(&k.as_str())) {
            return Err(CurveError::new(format!(
                "sidecar: unknown key '{k}' (known: {})",
                SIDECAR_KEYS.join(", ")
            )));
        }
        match o.get("schema").and_then(Value::as_str) {
            Some(SIDECAR_SCHEMA) => {}
            Some(s) => {
                return Err(CurveError::new(format!(
                    "sidecar schema must be '{SIDECAR_SCHEMA}', got '{s}'"
                )))
            }
            None => {
                return Err(CurveError::new(format!(
                    "a sidecar needs \"schema\": \"{SIDECAR_SCHEMA}\""
                )))
            }
        }
        let ctx = "sidecar";
        let quantity = opt_string(o, "quantity", ctx)?
            .map(|s| {
                Quantity::parse(&s)
                    .ok_or_else(|| CurveError::new(format!("sidecar: unknown quantity '{s}'")))
            })
            .transpose()?;
        let calibrated = match o.get("calibrated") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => {
                return Err(CurveError::new(
                    "sidecar 'calibrated' must be true or false",
                ))
            }
        };
        let seatings = match o.get("seatings") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|n| *n >= 1 && *n <= 10_000)
                    .ok_or_else(|| {
                        CurveError::new("sidecar 'seatings' must be a positive integer")
                    })? as u32,
            ),
        };
        let averaging = opt_string(o, "averaging", ctx)?
            .map(|s| {
                Averaging::parse(&s).ok_or_else(|| {
                    CurveError::new(format!(
                        "sidecar: unknown averaging '{s}' (none, magnitude, db, complex)"
                    ))
                })
            })
            .transpose()?;
        let smoothing = opt_string(o, "smoothing", ctx)?
            .map(|s| {
                Smoothing::parse(&s).ok_or_else(|| {
                    CurveError::new(format!("sidecar: smoothing '{s}' must be 'none' or '1/N'"))
                })
            })
            .transpose()?;
        let drive = match o.get("drive") {
            None | Some(Value::Null) => None,
            Some(d) => Some(Drive {
                json: d.clone(),
                spec: DriveSpec::from_json(d)
                    .map_err(|e| CurveError::new(format!("sidecar {e}")))?,
            }),
        };
        let source_impedance_ohm = opt_number(o, "source_impedance_ohm")?;
        if source_impedance_ohm.is_some_and(|z| z < 0.0) {
            return Err(CurveError::new(
                "sidecar 'source_impedance_ohm' must be non-negative",
            ));
        }
        let temperature_c = opt_number(o, "temperature_C")?;
        if temperature_c.is_some_and(|t| t < -273.15) {
            return Err(CurveError::new(
                "sidecar 'temperature_C' is below absolute zero",
            ));
        }
        let provenance = match o.get("provenance") {
            None | Some(Value::Null) => None,
            Some(Value::Object(p)) => {
                const KEYS: [&str; 5] = ["origin", "source", "url", "licence", "tool"];
                if let Some(k) = p.keys().find(|k| !KEYS.contains(&k.as_str())) {
                    return Err(CurveError::new(format!(
                        "sidecar provenance: unknown key '{k}' (known: {})",
                        KEYS.join(", ")
                    )));
                }
                let pctx = "sidecar provenance";
                Some(Provenance {
                    origin: opt_string(p, "origin", pctx)?
                        .map(|s| {
                            Origin::parse(&s).ok_or_else(|| {
                                CurveError::new(format!(
                                    "sidecar provenance: unknown origin '{s}' (measured, simulated, virtual_rig, published, digitized, user)"
                                ))
                            })
                        })
                        .transpose()?,
                    source: opt_string(p, "source", pctx)?,
                    url: opt_string(p, "url", pctx)?,
                    licence: opt_string(p, "licence", pctx)?,
                    tool: opt_string(p, "tool", pctx)?,
                })
            }
            Some(_) => return Err(CurveError::new("sidecar 'provenance' must be an object")),
        };
        let uncertainty = o
            .get("uncertainty")
            .filter(|v| !v.is_null())
            .map(Uncertainty::from_json)
            .transpose()?;
        let virtual_rig = match o.get("virtual_rig") {
            None | Some(Value::Null) => None,
            Some(v @ Value::Object(_)) => Some(v.clone()),
            Some(_) => return Err(CurveError::new("sidecar 'virtual_rig' must be an object")),
        };
        Ok(Sidecar {
            quantity,
            unit: opt_string(o, "unit", ctx)?,
            calibrated,
            fixture: opt_string(o, "fixture", ctx)?,
            ear_simulator: opt_string(o, "ear_simulator", ctx)?,
            pinna: opt_string(o, "pinna", ctx)?,
            seatings,
            averaging,
            smoothing,
            drive,
            source_impedance_ohm,
            compensation: opt_string(o, "compensation", ctx)?,
            reference_point: opt_string(o, "reference_point", ctx)?,
            temperature_c,
            date: opt_string(o, "date", ctx)?,
            device: opt_string(o, "device", ctx)?,
            provenance,
            uncertainty,
            virtual_rig,
            notes: opt_string(o, "notes", ctx)?,
        })
    }

    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("schema".into(), json!(SIDECAR_SCHEMA));
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                o.insert(k.into(), v);
            }
        };
        put("quantity", self.quantity.map(|q| json!(q.name())));
        put("unit", self.unit.as_ref().map(|s| json!(s)));
        put("calibrated", self.calibrated.map(|b| json!(b)));
        put("fixture", self.fixture.as_ref().map(|s| json!(s)));
        put(
            "ear_simulator",
            self.ear_simulator.as_ref().map(|s| json!(s)),
        );
        put("pinna", self.pinna.as_ref().map(|s| json!(s)));
        put("seatings", self.seatings.map(|n| json!(n)));
        put("averaging", self.averaging.map(|a| json!(a.name())));
        put("smoothing", self.smoothing.map(|s| json!(s.name())));
        put("drive", self.drive.as_ref().map(|d| d.json.clone()));
        put(
            "source_impedance_ohm",
            self.source_impedance_ohm.map(|z| json!(z)),
        );
        put("compensation", self.compensation.as_ref().map(|s| json!(s)));
        put(
            "reference_point",
            self.reference_point.as_ref().map(|s| json!(s)),
        );
        put("temperature_C", self.temperature_c.map(|t| json!(t)));
        put("date", self.date.as_ref().map(|s| json!(s)));
        put("device", self.device.as_ref().map(|s| json!(s)));
        put(
            "provenance",
            self.provenance.as_ref().map(|p| {
                let mut m = Map::new();
                if let Some(x) = p.origin {
                    m.insert("origin".into(), json!(x.name()));
                }
                for (k, v) in [
                    ("source", &p.source),
                    ("url", &p.url),
                    ("licence", &p.licence),
                    ("tool", &p.tool),
                ] {
                    if let Some(v) = v {
                        m.insert(k.into(), json!(v));
                    }
                }
                Value::Object(m)
            }),
        );
        put(
            "uncertainty",
            self.uncertainty.as_ref().map(Uncertainty::to_json),
        );
        put("virtual_rig", self.virtual_rig.clone());
        put("notes", self.notes.as_ref().map(|s| json!(s)));
        Value::Object(o)
    }

    /// Pretty-printed JSON text, for a sidecar file.
    pub fn to_text(&self) -> String {
        format!("{:#}\n", self.to_json())
    }
}

/// A field in which two sidecars differ.
#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    pub field: &'static str,
    pub a: String,
    pub b: String,
}

/// Result of [`compare`]: differences that forbid comparing the curves
/// (quantity, fixture, ear simulator, pinna, compensation, drive, source
/// impedance, reference point), and differences worth a note (averaging,
/// smoothing, calibration).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    pub blocking: Vec<Difference>,
    pub notes: Vec<Difference>,
}

/// Fields [`compare`] refuses on.
pub const BLOCKING_FIELDS: [&str; 8] = [
    "quantity",
    "fixture",
    "ear_simulator",
    "pinna",
    "compensation",
    "drive",
    "source_impedance_ohm",
    "reference_point",
];

impl Comparison {
    /// Ok if every blocking difference is in `allow` ("all" allows every
    /// field); otherwise an error naming each difference.
    pub fn check(&self, allow: &[&str]) -> Result<(), CurveError> {
        let refused: Vec<&Difference> = self
            .blocking
            .iter()
            .filter(|d| !allow.contains(&"all") && !allow.contains(&d.field))
            .collect();
        if refused.is_empty() {
            return Ok(());
        }
        Err(CurveError::new(format!(
            "the curves were not measured under the same conditions: {}; compare them only with an explicit override naming {}",
            refused
                .iter()
                .map(|d| format!("{} differs ({} vs {})", d.field, d.a, d.b))
                .collect::<Vec<_>>()
                .join(", "),
            refused.iter().map(|d| d.field).collect::<Vec<_>>().join(", ")
        )))
    }
}

fn norm_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Compares the conditions of two curves (spec Section 14: fixture,
/// compensation and drive, plus the quantity, ear simulator, pinna, source
/// impedance and reference point). A field stated on one side only is a
/// blocking difference ("unstated"), since the match cannot be confirmed; a
/// field stated on neither side is a note.
pub fn compare(a: &Sidecar, b: &Sidecar) -> Comparison {
    let mut c = Comparison::default();
    let unstated = || "unstated".to_string();
    let mut text = |field: &'static str, x: &Option<String>, y: &Option<String>, blocking: bool| {
        match (x, y) {
            (Some(x), Some(y)) if norm_text(x) == norm_text(y) => {}
            (None, None) => {
                if blocking {
                    c.notes.push(Difference {
                        field,
                        a: unstated(),
                        b: unstated(),
                    })
                }
            }
            _ => {
                let d = Difference {
                    field,
                    a: x.clone().unwrap_or_else(unstated),
                    b: y.clone().unwrap_or_else(unstated),
                };
                if blocking {
                    c.blocking.push(d)
                } else {
                    c.notes.push(d)
                }
            }
        }
    };
    text(
        "quantity",
        &a.quantity.map(|q| q.name().to_string()),
        &b.quantity.map(|q| q.name().to_string()),
        true,
    );
    text("fixture", &a.fixture, &b.fixture, true);
    text("ear_simulator", &a.ear_simulator, &b.ear_simulator, true);
    text("pinna", &a.pinna, &b.pinna, true);
    text("compensation", &a.compensation, &b.compensation, true);
    text(
        "reference_point",
        &a.reference_point,
        &b.reference_point,
        true,
    );
    text(
        "averaging",
        &a.averaging.map(|x| x.name().to_string()),
        &b.averaging.map(|x| x.name().to_string()),
        false,
    );
    text(
        "smoothing",
        &a.smoothing.map(|x| x.name()),
        &b.smoothing.map(|x| x.name()),
        false,
    );
    text(
        "calibrated",
        &a.calibrated.map(|x| x.to_string()),
        &b.calibrated.map(|x| x.to_string()),
        false,
    );
    match (&a.drive, &b.drive) {
        (Some(x), Some(y)) if x.same_as(y) => {}
        (None, None) => c.notes.push(Difference {
            field: "drive",
            a: unstated(),
            b: unstated(),
        }),
        (x, y) => c.blocking.push(Difference {
            field: "drive",
            a: x.as_ref().map_or_else(unstated, Drive::label),
            b: y.as_ref().map_or_else(unstated, Drive::label),
        }),
    }
    let z = |v: Option<f64>| v.map(|z| format!("{z} ohm"));
    match (a.source_impedance_ohm, b.source_impedance_ohm) {
        (Some(x), Some(y)) if (x - y).abs() <= 1e-9 * x.abs().max(y.abs()) => {}
        (None, None) => {}
        (x, y) => c.blocking.push(Difference {
            field: "source_impedance_ohm",
            a: z(x).unwrap_or_else(unstated),
            b: z(y).unwrap_or_else(unstated),
        }),
    }
    c
}
