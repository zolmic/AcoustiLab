//! Moving-coil driver macro (D0–D2), driver records and record governance
//! (spec Section 5; errata E5, E16, E17, E18).
//!
//! # The `driver` element
//!
//! Nodes `[e+, e−, a_front, a_rear]` (a_rear defaults to ambient). The macro
//! is a [`Composite`] of existing parts plus internal nodes that probes can
//! read:
//!
//! * `<id>.e`, electrical: between the coil impedance and the motor;
//! * `<id>.m`, mechanical: dome (and coil) velocity;
//! * `<id>.m2`, mechanical: surround velocity. At D0 and D1 it is joined to
//!   `<id>.m` by an ideal short (the diaphragm is rigid), so a netlist is
//!   valid at every level.
//!
//! Levels (key `model`, default `"D1"`):
//!
//! * **D0**: Re, motor Bl (transformer), Mms, Cms, Rms to the frame, piston Sd
//!   (gyrator). Inductance keys are accepted and ignored.
//! * **D1**: adds the coil's Le and LR-2 eddy branch (the `coil` model of
//!   the electrical family). With no inductance keys D1 equals D0.
//! * **D2**: adds the dome/surround two-degree-of-freedom branch, see
//!   [`TwoDof`]. Each mass drives its own piston area, so the effective area
//!   U/v_dome is complex and dips where the surround moves in antiphase.
//!
//! Creep (erratum E18) is a suspension option at every level:
//! C(jω) = C0·[1 − λ·log10(jω/ω0)] ([`Creep`]); in D2 it multiplies both
//! springs, which keeps their ratio, and hence the low-frequency split, fixed.
//!
//! Parameters come inline or from an embedded record (`"record": "<name>"`),
//! as either the primary set (`fs_Hz`, `Qms`, `Qes`, `Re_ohm`, `Mms_g`,
//! `Sd_cm2`) or the physical set (`Re_ohm`, `Bl_Tm`, `Mms_g`,
//! `Cms_mm_per_N` | `Kms_N_per_m`, `Rms_Ns_per_m`, `Sd_cm2`). See
//! docs/netlist.md for all keys.
//!
//! Ports (flows positive entering the element, so `Circuit::power_absorbed`
//! balances). Each port's flow is a signed sum of the parts' port flows:
//!
//! * port 0, electrical (e+, e−): coil current; an `impedance` probe on
//!   port 0 reads the driver's electrical input impedance;
//! * port 1, acoustic (a_front, a_rear): the volume velocity entering at the
//!   front, i.e. minus the diaphragm output Σ S_k·v_k;
//! * ports 2 and 3, mechanical (`<id>.m`, frame) and (`<id>.m2`, frame): the
//!   net force that elements outside the macro apply at the internal nodes
//!   (zero unless something, e.g. an added test mass, is connected there).
//!
//! # Records
//!
//! One JSON file per driver in `data/drivers/`, embedded with `include_str!`
//! so the wasm build carries them. Each stores provenance (source, date,
//! licence, measurement condition and air load), the primary set, the
//! datasheet values kept only for the consistency report, element
//! parameters beyond the primary set (`model`), and which of those are
//! estimates. [`governance`] runs the identity checks of spec Section 5 and
//! flags unit anomalies.

use super::couplers::{Motor, Piston};
use super::electrical::CoilModel;
use super::{Build, Composite, Constructor, Element, FreqCx, OnePort};
use crate::air::AirState;
use crate::error::Result;
use crate::mna::{potential, Mna, Unknown};
use crate::netlist::Domain;
use crate::ts::{self, Creep, PhysicalParams, TsParams};
use crate::units::{Dim, Params};
use crate::validity::ValidityLimit;
use crate::C64;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::any::Any;
use std::collections::BTreeMap;
use std::f64::consts::PI;

pub const TYPES: &[&str] = &["driver"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    match ty {
        "driver" => Some(driver),
        _ => None,
    }
}

// ===== Records ===============================================================

/// Schema tag of driver record files.
pub const RECORD_SCHEMA: &str = "acoustilab-driver/0.1";

/// Embedded records: (name, JSON text).
const RECORDS: &[(&str, &str)] = &[(
    "tymphany_hpd_40n16pet00_32",
    include_str!("../../../../data/drivers/tymphany_hpd_40n16pet00_32.json"),
)];

/// Names of the embedded driver records.
pub fn record_names() -> Vec<&'static str> {
    RECORDS.iter().map(|(n, _)| *n).collect()
}

/// Parses the embedded record `name`.
pub fn record(name: &str) -> std::result::Result<DriverRecord, String> {
    let (_, text) = RECORDS.iter().find(|(n, _)| *n == name).ok_or_else(|| {
        format!(
            "unknown driver record '{name}' (known: {})",
            record_names().join(", ")
        )
    })?;
    let rec = parse_record(text)?;
    if rec.name != name {
        return Err(format!(
            "record file for '{name}' declares name '{}'",
            rec.name
        ));
    }
    Ok(rec)
}

/// Who produced a record's numbers (spec Section 5: user-contributed
/// records are marked as such).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Datasheet,
    Measured,
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provenance {
    pub origin: Origin,
    pub source: String,
    pub url: Option<String>,
    /// Date of the source document or measurement.
    pub date: String,
    /// Date the source was consulted.
    pub retrieved: Option<String>,
    pub licence: String,
    /// Measurement condition (free air, coupler, vacuum, ...).
    pub condition: String,
    /// Which air load the moving mass, compliance and loss include.
    pub air_load: String,
}

/// Drive of a published sensitivity figure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RatingDrive {
    /// Volts RMS.
    Voltage(f64),
    /// Watts.
    Power(f64),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SensitivityRating {
    pub level_db: f64,
    pub drive: RatingDrive,
    /// Measurement distance, m (loudspeaker-style ratings).
    pub distance: Option<f64>,
    /// Reference frequency, Hz, when stated.
    pub frequency: Option<f64>,
    pub condition: String,
}

/// Datasheet values kept for the consistency report (SI units). None of
/// them feeds the network: Bl, Cms and Rms come from the primary set.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Datasheet {
    pub bl: Option<f64>,
    pub cms: Option<f64>,
    pub vas: Option<f64>,
    pub le: Option<f64>,
    pub qts: Option<f64>,
    pub z_min: Option<f64>,
    pub xmax: Option<f64>,
    pub rated_impedance: Option<f64>,
    pub rated_power: Option<f64>,
    pub sensitivity: Vec<SensitivityRating>,
}

/// A driver record (see the module documentation).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriverRecord {
    pub name: String,
    pub title: String,
    pub provenance: Provenance,
    pub primary: TsParams,
    pub datasheet: Datasheet,
    /// Element parameters beyond the primary set, as unit-suffixed netlist
    /// keys (e.g. `Le_mH`, `creep_lambda`, the D2 surround keys).
    pub model: Map<String, Value>,
    /// Keys of `model` that are estimates rather than datasheet or measured
    /// values (spec Section 18).
    pub estimated: Vec<String>,
    /// Stated relative tolerances by parameter name.
    pub tolerances: BTreeMap<String, f64>,
    pub notes: Vec<String>,
    /// Keys exactly as printed for the dimensional fields, e.g.
    /// `"Cms" → "Cms_um_per_N"`, used to name unit anomalies.
    pub printed_keys: BTreeMap<String, String>,
}

fn obj<'a>(v: &'a Value, key: &str) -> std::result::Result<&'a Map<String, Value>, String> {
    v.get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("record: missing object '{key}'"))
}

fn string(m: &Map<String, Value>, key: &str) -> std::result::Result<String, String> {
    m.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("record: missing string '{key}'"))
}

fn opt_string(m: &Map<String, Value>, key: &str) -> Option<String> {
    m.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The key present in `m` for quantity `base` of dimension `dim`.
fn printed_key(m: &Map<String, Value>, base: &str, dim: Dim) -> Option<String> {
    dim.suffixes()
        .iter()
        .map(|(s, _)| format!("{base}_{s}"))
        .find(|k| m.contains_key(k))
}

/// Parses a driver record document.
pub fn parse_record(text: &str) -> std::result::Result<DriverRecord, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("record JSON: {e}"))?;
    let top = v.as_object().ok_or("record must be a JSON object")?;
    let allowed = [
        "schema",
        "name",
        "title",
        "provenance",
        "primary",
        "datasheet",
        "tolerances",
        "model",
        "estimated",
        "notes",
    ];
    if let Some(k) = top.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("record: unknown key '{k}'"));
    }
    let schema = string(top, "schema")?;
    if schema != RECORD_SCHEMA {
        return Err(format!(
            "record: schema '{schema}', expected '{RECORD_SCHEMA}'"
        ));
    }
    let name = string(top, "name")?;
    let title = string(top, "title")?;
    let e = |x: crate::Error| format!("record '{name}': {x}");

    let pv = obj(&v, "provenance")?;
    let origin = match string(pv, "origin")?.as_str() {
        "datasheet" => Origin::Datasheet,
        "measured" => Origin::Measured,
        "user" => Origin::User,
        o => return Err(format!("record '{name}': unknown origin '{o}'")),
    };
    let provenance = Provenance {
        origin,
        source: string(pv, "source")?,
        url: opt_string(pv, "url"),
        date: string(pv, "date")?,
        retrieved: opt_string(pv, "retrieved"),
        licence: string(pv, "licence")?,
        condition: string(pv, "condition")?,
        air_load: string(pv, "air_load")?,
    };
    for (k, s) in [
        ("source", &provenance.source),
        ("date", &provenance.date),
        ("licence", &provenance.licence),
        ("condition", &provenance.condition),
        ("air_load", &provenance.air_load),
    ] {
        if s.trim().is_empty() {
            return Err(format!("record '{name}': provenance '{k}' is empty"));
        }
    }

    let mut printed_keys = BTreeMap::new();
    let pm = obj(&v, "primary")?.clone();
    for (base, dim) in [
        ("fs", Dim::Frequency),
        ("Re", Dim::ElecResistance),
        ("Mms", Dim::Mass),
        ("Sd", Dim::Area),
    ] {
        if let Some(k) = printed_key(&pm, base, dim) {
            printed_keys.insert(base.to_string(), k);
        }
    }
    let mut p = Params::new(format!("record {name}: primary"), pm);
    let primary = TsParams {
        fs: p.positive("fs", Dim::Frequency).map_err(e)?,
        qms: p.number("Qms").map_err(e)?,
        qes: p.number("Qes").map_err(e)?,
        re: p.positive("Re", Dim::ElecResistance).map_err(e)?,
        mms: p.positive("Mms", Dim::Mass).map_err(e)?,
        sd: p.positive("Sd", Dim::Area).map_err(e)?,
    };
    p.finish().map_err(e)?;
    primary
        .validate()
        .map_err(|m| format!("record '{name}': {m}"))?;

    let dm = obj(&v, "datasheet")?.clone();
    for (base, dim) in [
        ("Bl", Dim::ForceFactor),
        ("Cms", Dim::MechCompliance),
        ("Vas", Dim::Volume),
        ("Le", Dim::Inductance),
    ] {
        if let Some(k) = printed_key(&dm, base, dim) {
            printed_keys.insert(base.to_string(), k);
        }
    }
    let mut d = Params::new(format!("record {name}: datasheet"), dm);
    let mut datasheet = Datasheet {
        bl: d.positive_opt("Bl", Dim::ForceFactor).map_err(e)?,
        cms: d.positive_opt("Cms", Dim::MechCompliance).map_err(e)?,
        vas: d.positive_opt("Vas", Dim::Volume).map_err(e)?,
        le: d.quantity_opt("Le", Dim::Inductance).map_err(e)?,
        qts: d.number_opt("Qts").map_err(e)?,
        z_min: d.positive_opt("Zmin", Dim::ElecResistance).map_err(e)?,
        xmax: d.positive_opt("Xmax", Dim::Length).map_err(e)?,
        rated_impedance: d
            .positive_opt("rated_impedance", Dim::ElecResistance)
            .map_err(e)?,
        rated_power: d.positive_opt("rated_power", Dim::Power).map_err(e)?,
        sensitivity: Vec::new(),
    };
    if let Some(list) = d.value_opt("sensitivity") {
        let arr = list
            .as_array()
            .ok_or_else(|| format!("record '{name}': 'sensitivity' must be an array"))?;
        for s in arr {
            let m = s
                .as_object()
                .ok_or_else(|| format!("record '{name}': sensitivity entries are objects"))?
                .clone();
            let mut sp = Params::new(format!("record {name}: sensitivity"), m);
            let level_db = sp.number("level_dB").map_err(e)?;
            let drive = match (
                sp.positive_opt("drive", Dim::Voltage).map_err(e)?,
                sp.positive_opt("drive", Dim::Power).map_err(e)?,
            ) {
                (Some(v), None) => RatingDrive::Voltage(v),
                (None, Some(w)) => RatingDrive::Power(w),
                _ => {
                    return Err(format!(
                        "record '{name}': a sensitivity needs exactly one of drive_V or drive_W"
                    ))
                }
            };
            let rating = SensitivityRating {
                level_db,
                drive,
                distance: sp.positive_opt("distance", Dim::Length).map_err(e)?,
                frequency: sp.positive_opt("f", Dim::Frequency).map_err(e)?,
                condition: sp.string_opt("condition").map_err(e)?.unwrap_or_default(),
            };
            sp.finish().map_err(e)?;
            datasheet.sensitivity.push(rating);
        }
    }
    d.finish().map_err(e)?;

    let mut tolerances = BTreeMap::new();
    if let Some(t) = top.get("tolerances") {
        for (k, x) in t
            .as_object()
            .ok_or_else(|| format!("record '{name}': 'tolerances' must be an object"))?
        {
            let x = x
                .as_f64()
                .filter(|x| x.is_finite() && *x >= 0.0)
                .ok_or_else(|| format!("record '{name}': tolerance '{k}' must be >= 0"))?;
            tolerances.insert(k.clone(), x);
        }
    }
    let model = match top.get("model") {
        None => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(format!("record '{name}': 'model' must be an object")),
    };
    let strings = |key: &str| -> std::result::Result<Vec<String>, String> {
        match top.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| x.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("record '{name}': '{key}' must hold strings")),
            Some(_) => Err(format!("record '{name}': '{key}' must be an array")),
        }
    };
    let estimated = strings("estimated")?;
    if let Some(k) = estimated.iter().find(|k| !model.contains_key(*k)) {
        return Err(format!(
            "record '{name}': estimated key '{k}' is not in 'model'"
        ));
    }
    let notes = strings("notes")?;
    Ok(DriverRecord {
        name,
        title,
        provenance,
        primary,
        datasheet,
        model,
        estimated,
        tolerances,
        notes,
        printed_keys,
    })
}

impl DriverRecord {
    /// Element parameters for the `driver` element: the primary set (in SI
    /// keys) plus the `model` block.
    pub fn element_params(&self) -> Map<String, Value> {
        let p = &self.primary;
        let mut m = self.model.clone();
        for (k, v) in [
            ("fs_Hz", json!(p.fs)),
            ("Qms", json!(p.qms)),
            ("Qes", json!(p.qes)),
            ("Re_ohm", json!(p.re)),
            ("Mms_kg", json!(p.mms)),
            ("Sd_m2", json!(p.sd)),
        ] {
            m.insert(k.to_string(), v);
        }
        m
    }
}

// ===== Governance ============================================================

/// Relative tolerance of the identity checks (spec Section 5, "Record
/// governance": resonance identity within 5 percent; the same bound is used
/// for electrical Q, Vas and total Q).
pub const IDENTITY_TOLERANCE: f64 = 0.05;

/// Fraction of the rated impedance the minimum impedance may not fall below
/// (spec Section 4, rated-impedance check).
pub const RATED_IMPEDANCE_FRACTION: f64 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    /// Passes the arithmetic but limits what the data can support.
    Warning,
    /// Reported for context; no pass/fail criterion.
    Info,
    /// A field the check needs is missing.
    NotAvailable,
}

/// The same identity evaluated after undoing a unit anomaly.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Repair {
    pub anomaly: String,
    pub value: f64,
    pub deviation: f64,
    pub pass: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub description: String,
    /// The quantity recomputed from other fields.
    pub value: Option<f64>,
    /// The printed or primary quantity it should equal.
    pub reference: Option<f64>,
    /// value/reference − 1.
    pub deviation: Option<f64>,
    pub tolerance: Option<f64>,
    pub status: CheckStatus,
    pub repair: Option<Repair>,
}

/// A field whose printed unit is probably wrong by a decimal factor:
/// scaling it repairs at least one failing identity and worsens none.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnitAnomaly {
    pub field: &'static str,
    /// Key as printed, e.g. `Cms_um_per_N`.
    pub printed: String,
    /// Factor the printed value must be multiplied by (in SI).
    pub factor: f64,
    /// The key the value is consistent with, e.g. `Cms_mm_per_N`.
    pub likely: String,
    /// Checks the rescaled value repairs.
    pub repairs: Vec<&'static str>,
    /// True for a field of the primary set, which the network uses.
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GovernanceReport {
    pub record: String,
    /// Bl, Cms and Rms derived from the primary set.
    pub derived: PhysicalParams,
    pub checks: Vec<Check>,
    pub anomalies: Vec<UnitAnomaly>,
}

impl GovernanceReport {
    pub fn check(&self, id: &str) -> Option<&Check> {
        self.checks.iter().find(|c| c.id == id)
    }

    /// Checks that failed as printed.
    pub fn failures(&self) -> Vec<&Check> {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .collect()
    }

    /// A unit anomaly in a primary field makes the record unusable.
    pub fn primary_anomaly(&self) -> Option<&UnitAnomaly> {
        self.anomalies.iter().find(|a| a.primary)
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("report serialises")
    }
}

/// Values the identities read, with datasheet fields optional.
#[derive(Debug, Clone, Copy)]
struct Values {
    fs: f64,
    qes: f64,
    re: f64,
    mms: f64,
    sd: f64,
    bl: Option<f64>,
    cms: Option<f64>,
    vas: Option<f64>,
    qts: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Mms,
    Sd,
    Cms,
    Vas,
}

impl Field {
    fn name(self) -> &'static str {
        match self {
            Field::Mms => "Mms",
            Field::Sd => "Sd",
            Field::Cms => "Cms",
            Field::Vas => "Vas",
        }
    }
    fn dim(self) -> Dim {
        match self {
            Field::Mms => Dim::Mass,
            Field::Sd => Dim::Area,
            Field::Cms => Dim::MechCompliance,
            Field::Vas => Dim::Volume,
        }
    }
    fn primary(self) -> bool {
        matches!(self, Field::Mms | Field::Sd)
    }
    /// Decimal slips considered: prefix mix-ups (µ/m/k: 10³) for masses,
    /// compliances and volumes, cm²/mm² (10²) for areas.
    fn factors(self) -> [f64; 2] {
        match self {
            Field::Sd => [1e2, 1e-2],
            _ => [1e3, 1e-3],
        }
    }
    fn scale(self, v: &mut Values, k: f64) {
        match self {
            Field::Mms => v.mms *= k,
            Field::Sd => v.sd *= k,
            Field::Cms => v.cms = v.cms.map(|x| x * k),
            Field::Vas => v.vas = v.vas.map(|x| x * k),
        }
    }
}

/// Identities that can fail: (id, fields involved).
const IDENTITIES: &[(&str, &[Field])] = &[
    ("resonance", &[Field::Mms, Field::Cms]),
    ("electrical_q", &[Field::Mms]),
    ("equivalent_volume", &[Field::Sd, Field::Cms, Field::Vas]),
];

/// (value recomputed, reference) of an identity, if its fields are present.
fn identity(id: &str, v: &Values, air: &AirState) -> Option<(f64, f64)> {
    match id {
        "resonance" => Some((ts::resonance_hz(v.mms, v.cms?), v.fs)),
        "electrical_q" => Some((ts::electrical_q(v.bl?, v.re, v.mms, v.fs), v.qes)),
        "equivalent_volume" => Some((ts::vas(air, v.sd, v.cms?), v.vas?)),
        _ => None,
    }
}

fn deviation((value, reference): (f64, f64)) -> f64 {
    value / reference - 1.0
}

fn passes(d: f64) -> bool {
    d.abs() <= IDENTITY_TOLERANCE
}

/// Runs the governance checks of spec Section 5 on a record:
///
/// * `resonance`: fs against 1/(2π·sqrt(Mms·Cms)) with the printed Cms;
/// * `electrical_q`: Qes against 2π·fs·Mms·Re/Bl² with the printed Bl;
/// * `equivalent_volume`: printed Vas against ρc²·Sd²·Cms (spec reference
///   air, ρc² = 1.4165e5 Pa; datasheet air conditions are rarely stated and
///   the 5 % tolerance covers the usual 1.18–1.21 kg/m³, 340–346 m/s);
/// * `total_q`: printed Qts against Qms·Qes/(Qms + Qes);
/// * `identifiability`: warns when Qms/Qes < 0.1;
/// * `rated_impedance`: minimum impedance (printed Zmin, else Re) at least
///   80 % of the rated impedance;
/// * `sensitivity_conversion` (info): the impedance implied by a pair of
///   voltage- and power-referenced sensitivities (erratum E32).
///
/// Unit anomalies: for each field of a failing identity, the decimal
/// factors of [`Field::factors`] are tried; a factor is reported when it
/// repairs at least one identity, moves no identity involving that field
/// further from agreement, and is not dominated by another factor that
/// repairs a superset of identities.
pub fn governance(rec: &DriverRecord) -> GovernanceReport {
    let air = AirState::spec_reference();
    let p = &rec.primary;
    let ds = &rec.datasheet;
    let v = Values {
        fs: p.fs,
        qes: p.qes,
        re: p.re,
        mms: p.mms,
        sd: p.sd,
        bl: ds.bl,
        cms: ds.cms,
        vas: ds.vas,
        qts: ds.qts,
    };

    // Unit anomaly search.
    let printed: Vec<(&str, Option<(f64, f64)>)> = IDENTITIES
        .iter()
        .map(|(id, _)| (*id, identity(id, &v, &air)))
        .collect();
    let failing = |id: &str| {
        printed
            .iter()
            .any(|(i, r)| *i == id && r.is_some_and(|r| !passes(deviation(r))))
    };
    let mut candidates: Vec<(Field, f64, Vec<&'static str>)> = Vec::new();
    for field in [Field::Mms, Field::Sd, Field::Cms, Field::Vas] {
        let involved: Vec<&'static str> = IDENTITIES
            .iter()
            .filter(|(_, fs)| fs.contains(&field))
            .map(|(id, _)| *id)
            .collect();
        if !involved.iter().any(|id| failing(id)) {
            continue;
        }
        for k in field.factors() {
            let mut s = v;
            field.scale(&mut s, k);
            let mut repairs = Vec::new();
            let mut worsens = false;
            for id in &involved {
                let (Some(before), Some(after)) = (identity(id, &v, &air), identity(id, &s, &air))
                else {
                    continue;
                };
                let (db, da) = (deviation(before), deviation(after));
                if !passes(db) && passes(da) {
                    repairs.push(*id);
                }
                if (1.0 + da).ln().abs() > (1.0 + db).ln().abs() + 1e-12 {
                    worsens = true;
                }
            }
            if !repairs.is_empty() && !worsens {
                candidates.push((field, k, repairs));
            }
        }
    }
    let dominated = |i: usize| {
        candidates.iter().enumerate().any(|(j, (_, _, rj))| {
            j != i
                && rj.len() > candidates[i].2.len()
                && candidates[i].2.iter().all(|x| rj.contains(x))
        })
    };
    let anomalies: Vec<UnitAnomaly> = (0..candidates.len())
        .filter(|&i| !dominated(i))
        .map(|i| {
            let (field, k, ref repairs) = candidates[i];
            let printed = rec
                .printed_keys
                .get(field.name())
                .cloned()
                .unwrap_or_else(|| field.name().to_string());
            let likely = likely_key(field, &printed, k);
            UnitAnomaly {
                field: field.name(),
                printed,
                factor: k,
                likely,
                repairs: repairs.clone(),
                primary: field.primary(),
            }
        })
        .collect();

    let repair_for = |id: &str| -> Option<Repair> {
        let (a, field, k) = anomalies.iter().find_map(|a| {
            a.repairs.contains(&id).then(|| {
                let field = [Field::Mms, Field::Sd, Field::Cms, Field::Vas]
                    .into_iter()
                    .find(|f| f.name() == a.field)
                    .expect("known field");
                (a, field, a.factor)
            })
        })?;
        let mut s = v;
        field.scale(&mut s, k);
        let r = identity(id, &s, &air)?;
        let d = deviation(r);
        Some(Repair {
            anomaly: format!("{} read as {}", a.printed, a.likely),
            value: r.0,
            deviation: d,
            pass: passes(d),
        })
    };

    let identity_check = |id: &'static str, description: String| -> Check {
        match identity(id, &v, &air) {
            Some(r) => {
                let d = deviation(r);
                let pass = passes(d);
                Check {
                    id,
                    description,
                    value: Some(r.0),
                    reference: Some(r.1),
                    deviation: Some(d),
                    tolerance: Some(IDENTITY_TOLERANCE),
                    status: if pass {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    repair: if pass { None } else { repair_for(id) },
                }
            }
            None => Check {
                id,
                description,
                value: None,
                reference: None,
                deviation: None,
                tolerance: Some(IDENTITY_TOLERANCE),
                status: CheckStatus::NotAvailable,
                repair: None,
            },
        }
    };

    let mut checks = vec![
        identity_check(
            "resonance",
            "fs from the printed Mms and Cms, 1/(2 pi sqrt(Mms Cms)), against the primary fs"
                .into(),
        ),
        identity_check(
            "electrical_q",
            "Qes from the printed Bl, 2 pi fs Mms Re / Bl^2, against the primary Qes".into(),
        ),
        identity_check(
            "equivalent_volume",
            "rho c^2 Sd^2 Cms from the printed Cms against the printed Vas".into(),
        ),
    ];

    // Total Q.
    let qts_primary = ts::qts(p.qms, p.qes);
    let mut desc = "Qms Qes/(Qms + Qes) from the primary set against the printed Qts".to_string();
    if let Some(bl) = ds.bl {
        let qes_bl = ts::electrical_q(bl, p.re, p.mms, p.fs);
        desc.push_str(&format!(
            "; with Qes from the printed Bl it would be {:.4}",
            ts::qts(p.qms, qes_bl)
        ));
    }
    checks.push(match v.qts {
        Some(q) => {
            let d = qts_primary / q - 1.0;
            Check {
                id: "total_q",
                description: desc,
                value: Some(qts_primary),
                reference: Some(q),
                deviation: Some(d),
                tolerance: Some(IDENTITY_TOLERANCE),
                status: if passes(d) {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                repair: None,
            }
        }
        None => Check {
            id: "total_q",
            description: desc,
            value: Some(qts_primary),
            reference: None,
            deviation: None,
            tolerance: Some(IDENTITY_TOLERANCE),
            status: CheckStatus::NotAvailable,
            repair: None,
        },
    });

    // Identifiability.
    let ratio = p.qms / p.qes;
    checks.push(Check {
        id: "identifiability",
        description: format!(
            "Qms/Qes = {ratio:.4}; below {} the impedance hump is too small to identify parameters electrically (spec Section 5)",
            ts::MIN_Q_RATIO
        ),
        value: Some(ratio),
        reference: Some(ts::MIN_Q_RATIO),
        deviation: None,
        tolerance: None,
        status: if ratio >= ts::MIN_Q_RATIO {
            CheckStatus::Pass
        } else {
            CheckStatus::Warning
        },
        repair: None,
    });

    // Rated impedance.
    let z_min = ds.z_min.unwrap_or(p.re);
    checks.push(match ds.rated_impedance {
        Some(zr) => Check {
            id: "rated_impedance",
            description: format!(
                "minimum impedance ({}) at least {:.0} % of the rated impedance (spec Section 4)",
                if ds.z_min.is_some() {
                    "printed Zmin"
                } else {
                    "Re"
                },
                100.0 * RATED_IMPEDANCE_FRACTION
            ),
            value: Some(z_min),
            reference: Some(RATED_IMPEDANCE_FRACTION * zr),
            deviation: Some(z_min / (RATED_IMPEDANCE_FRACTION * zr) - 1.0),
            tolerance: None,
            status: if z_min >= RATED_IMPEDANCE_FRACTION * zr {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            repair: None,
        },
        None => Check {
            id: "rated_impedance",
            description: "no rated impedance in the record".into(),
            value: Some(z_min),
            reference: None,
            deviation: None,
            tolerance: None,
            status: CheckStatus::NotAvailable,
            repair: None,
        },
    });

    // Impedance implied by a voltage/power sensitivity pair.
    let volt = ds.sensitivity.iter().find_map(|s| match s.drive {
        RatingDrive::Voltage(u) => Some((s, u)),
        _ => None,
    });
    let watt = ds.sensitivity.iter().find_map(|s| match s.drive {
        RatingDrive::Power(w) => Some((s, w)),
        _ => None,
    });
    if let (Some((sv, u)), Some((sw, w))) = (volt, watt) {
        if sv.distance == sw.distance && sv.frequency == sw.frequency {
            let z = u * u / w * 10f64.powf((sw.level_db - sv.level_db) / 10.0);
            checks.push(Check {
                id: "sensitivity_conversion",
                description: format!(
                    "the {u} V and {w} W sensitivities imply a {z:.2} ohm conversion impedance; IEC 60268-7 conversions use the rated impedance (erratum E32), Re = {:.2} ohm",
                    p.re
                ),
                value: Some(z),
                reference: ds.rated_impedance,
                deviation: ds.rated_impedance.map(|zr| z / zr - 1.0),
                tolerance: None,
                status: CheckStatus::Info,
                repair: None,
            });
        }
    }

    GovernanceReport {
        record: rec.name.clone(),
        derived: p.to_physical(),
        checks,
        anomalies,
    }
}

/// The suffixed key whose SI factor is `factor` times that of `printed`.
fn likely_key(field: Field, printed: &str, factor: f64) -> String {
    let base = field.name();
    let suffixes = field.dim().suffixes();
    let from = printed
        .strip_prefix(base)
        .and_then(|s| s.strip_prefix('_'))
        .and_then(|s| suffixes.iter().find(|(x, _)| *x == s));
    if let Some((_, f0)) = from {
        let target = f0 * factor;
        if let Some((s, _)) = suffixes
            .iter()
            .find(|(_, f)| ((f / target) - 1.0).abs() < 1e-9)
        {
            return format!("{base}_{s}");
        }
    }
    format!("{printed} x {factor:e}")
}

// ===== Element ===============================================================

/// Driver sub-ladder level (spec Section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum DriverLevel {
    D0,
    D1,
    D2,
}

impl DriverLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "D0" => Some(DriverLevel::D0),
            "D1" => Some(DriverLevel::D1),
            "D2" => Some(DriverLevel::D2),
            _ => None,
        }
    }
}

/// D2 surround parameters as given by the user.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Surround {
    /// Effective (modal) mass of the surround, kg.
    pub mass: f64,
    /// Effective piston area of the surround, m².
    pub area: f64,
    /// Bending stiffness coupling the dome edge to the surround mass, N/m.
    pub k_bend: f64,
    /// Loss in that coupling, N·s/m.
    pub r_bend: f64,
}

/// Two-degree-of-freedom dome/surround model derived so that it reproduces
/// the D0 parameters at low frequency.
///
/// Topology (velocity across): the dome-plus-coil mass M1 at `<id>.m` is
/// driven by the motor and damped to the frame by R_dome; the bending
/// coupling (C_bend with loss R_bend) joins it to the surround mass M2 at
/// `<id>.m2`, which is sprung to the frame by C_outer. Pistons of area S1
/// (on M1) and S2 (on M2) feed the same acoustic nodes.
///
/// Below the surround mode the surround follows the dome with the static
/// ratio r = C_outer/(C_bend + C_outer); a Rayleigh reduction to the dome
/// velocity then gives mass M1 + r²·M2, compliance C_bend + C_outer, loss
/// R_dome + (1 − r)²·R_bend and area S1 + r·S2. Setting those equal to Mms,
/// Cms, Rms and Sd fixes
///
/// ```text
/// C_outer = Cms − 1/K_bend,  r = C_outer/Cms,  M1 = Mms − r²·M2,
/// S1 = Sd − r·S2,            R_dome = Rms − (1 − r)²·R_bend,
/// ```
///
/// so the motor-driven (free-air) behaviour of D2 matches D0 to
/// O((f/f_surround)²): the next term is an extra mass r²·M2·(f/f_surround)².
/// Under acoustic load D2 legitimately differs even at low frequency: the
/// pressure also acts on the surround directly, and the statics of the two
/// masses give a volume compliance to pressure of Sd²·Cms + r·(1 − r)·S2²·Cms
/// against D0's Sd²·Cms (a two-mass compliance matrix is not rank one), while
/// the motor-driven volume compliance Sd·Cms is unchanged. Free-air
/// Thiele–Small data cannot see that term.
///
/// This is standard two-mass mechanics, a lumped stand-in for the first
/// non-piston mode (spec p. 17), not a published headphone-specific model.
/// Its surround parameters have no datasheet source and must be marked as
/// estimates in a record.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TwoDof {
    pub m_dome: f64,
    pub m_surround: f64,
    pub s_dome: f64,
    pub s_surround: f64,
    pub c_bend: f64,
    pub c_outer: f64,
    pub r_dome: f64,
    pub r_bend: f64,
    /// Static follow ratio r = C_outer/Cms.
    pub coupling: f64,
}

impl TwoDof {
    pub fn split(p: &PhysicalParams, s: &Surround) -> std::result::Result<Self, String> {
        let c_bend = 1.0 / s.k_bend;
        if c_bend >= p.cms {
            return Err(format!(
                "bending compliance 1/Kbend = {c_bend:.4e} m/N must be below Cms = {:.4e} m/N",
                p.cms
            ));
        }
        let c_outer = p.cms - c_bend;
        let r = c_outer / p.cms;
        let m_dome = p.mms - r * r * s.mass;
        if m_dome <= 0.0 {
            return Err(format!(
                "surround mass too large: Mms − r²·Msur = {m_dome:.4e} kg (r = {r:.4})"
            ));
        }
        let s_dome = p.sd - r * s.area;
        if s_dome <= 0.0 {
            return Err(format!(
                "surround area too large: Sd − r·Ssur = {s_dome:.4e} m² (r = {r:.4})"
            ));
        }
        let r_dome = p.rms - (1.0 - r) * (1.0 - r) * s.r_bend;
        if r_dome < 0.0 {
            return Err(format!(
                "Rbend too large: Rms − (1 − r)²·Rbend = {r_dome:.4e} N·s/m (r = {r:.4})"
            ));
        }
        Ok(TwoDof {
            m_dome,
            m_surround: s.mass,
            s_dome,
            s_surround: s.area,
            c_bend,
            c_outer,
            r_dome,
            r_bend: s.r_bend,
            coupling: r,
        })
    }

    /// Admittance (force/velocity) of the bending coupling.
    pub fn y_bend(&self, omega: f64, c: C64) -> C64 {
        self.r_bend + (C64::new(0.0, omega) * self.c_bend * c).inv()
    }

    /// Admittance of the surround branch to the frame (mass and outer spring).
    pub fn y_surround(&self, omega: f64, c: C64) -> C64 {
        let jw = C64::new(0.0, omega);
        jw * self.m_surround + (jw * self.c_outer * c).inv()
    }

    /// v_surround/v_dome with no acoustic load; `c` is the creep factor.
    pub fn velocity_ratio(&self, omega: f64, c: C64) -> C64 {
        let yb = self.y_bend(omega, c);
        yb / (yb + self.y_surround(omega, c))
    }

    /// Effective area U/v_dome with no acoustic load.
    pub fn effective_area(&self, omega: f64, c: C64) -> C64 {
        self.s_dome + self.s_surround * self.velocity_ratio(omega, c)
    }

    /// Surround resonance with the dome blocked, sqrt((K_bend + K_outer)/M2)/2π.
    pub fn surround_resonance_hz(&self) -> f64 {
        ((1.0 / self.c_bend + 1.0 / self.c_outer) / self.m_surround).sqrt() / (2.0 * PI)
    }

    /// Zero of the lossless effective area (no creep, R_bend = 0):
    /// ω² = (K_bend + K_outer + K_bend·S2/S1)/M2.
    pub fn area_zero_hz(&self) -> f64 {
        let kb = 1.0 / self.c_bend;
        ((kb + 1.0 / self.c_outer + kb * self.s_surround / self.s_dome) / self.m_surround).sqrt()
            / (2.0 * PI)
    }
}

/// A driver port whose flow is a signed sum of part-port flows.
struct TapPort {
    plus: Unknown,
    minus: Unknown,
    /// (part index, part port, sign).
    taps: Vec<(usize, usize, f64)>,
}

/// Ideal short between two nodes (one branch unknown); used to join
/// `<id>.m2` to `<id>.m` at D0 and D1.
struct Short {
    id: String,
    p: Unknown,
    n: Unknown,
}

impl Element for Short {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "driver.short"
    }
    fn branch_count(&self) -> usize {
        1
    }
    fn stamp(&self, _cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        mna.short(self.p, self.n, br[0]);
    }
    fn port_count(&self) -> usize {
        1
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        (port == 0).then(|| potential(x, self.p) - potential(x, self.n))
    }
    /// Flow entering at `p` (the branch unknown is the flow delivered out of `p`).
    fn port_flow(&self, _cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        (port == 0).then(|| -x[br[0]])
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The `driver` macro element.
pub struct Driver {
    pub id: String,
    pub level: DriverLevel,
    /// D0 parameters (derived from the primary set when given as such).
    pub params: PhysicalParams,
    /// Series coil inductance and LR-2 branch (used from D1 up).
    pub le: f64,
    pub lr2: Option<(f64, f64)>,
    pub creep: Option<Creep>,
    /// D2 split (present at D2 only).
    pub two_dof: Option<TwoDof>,
    /// Record the parameters came from, if any.
    pub record: Option<String>,
    parts: Composite,
    offsets: Vec<usize>,
    ports: Vec<TapPort>,
}

impl Driver {
    /// Coil impedance at this level: Re, plus jωLe and R2 ∥ jωL2 from D1 up.
    pub fn coil_impedance(&self, omega: f64) -> C64 {
        coil_model(self.level, &self.params, self.le, self.lr2).impedance(omega)
    }

    /// Creep factor C(jω)/C0 (1 without creep).
    pub fn creep_factor(&self, omega: f64) -> C64 {
        creep_factor(self.creep, omega)
    }

    /// Mechanical impedance at the dome with no acoustic load (force per
    /// dome velocity).
    pub fn mechanical_impedance(&self, omega: f64) -> C64 {
        let jw = C64::new(0.0, omega);
        let c = self.creep_factor(omega);
        match &self.two_dof {
            None => jw * self.params.mms + self.params.rms + (jw * self.params.cms * c).inv(),
            Some(t) => {
                let (yb, ys) = (t.y_bend(omega, c), t.y_surround(omega, c));
                jw * t.m_dome + t.r_dome + yb * ys / (yb + ys)
            }
        }
    }

    /// Electrical input impedance with no acoustic load (both acoustic
    /// nodes at ambient): Z_coil + Bl²/Zm.
    pub fn unloaded_impedance(&self, omega: f64) -> C64 {
        self.coil_impedance(omega)
            + self.params.bl * self.params.bl / self.mechanical_impedance(omega)
    }

    /// Effective area U/v_dome with no acoustic load (Sd below D2).
    pub fn effective_area(&self, omega: f64) -> C64 {
        match &self.two_dof {
            None => C64::new(self.params.sd, 0.0),
            Some(t) => t.effective_area(omega, self.creep_factor(omega)),
        }
    }

    fn part_offset(&self, part: usize) -> usize {
        self.offsets[part]
    }
}

fn creep_factor(creep: Option<Creep>, omega: f64) -> C64 {
    creep.map_or(C64::new(1.0, 0.0), |c| c.factor(omega))
}

fn coil_model(
    level: DriverLevel,
    p: &PhysicalParams,
    le: f64,
    lr2: Option<(f64, f64)>,
) -> CoilModel {
    if level >= DriverLevel::D1 {
        CoilModel { re: p.re, le, lr2 }
    } else {
        CoilModel {
            re: p.re,
            le: 0.0,
            lr2: None,
        }
    }
}

impl Element for Driver {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "driver"
    }
    fn branch_count(&self) -> usize {
        self.parts.branch_count()
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        self.parts.stamp(cx, mna, br);
    }
    fn port_count(&self) -> usize {
        self.ports.len()
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        self.ports
            .get(port)
            .map(|p| potential(x, p.plus) - potential(x, p.minus))
    }
    fn port_flow(&self, cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        let p = self.ports.get(port)?;
        let mut sum = C64::new(0.0, 0.0);
        for &(part, pp, sign) in &p.taps {
            let e = &self.parts.parts[part];
            let o = self.part_offset(part);
            sum += e.port_flow(cx, x, &br[o..o + e.branch_count()], pp)? * sign;
        }
        Some(sum)
    }
    /// The rigid-piston range ends near ka = 1 (spec p. 16; erratum E28:
    /// 3.06 kHz for Sd = 10 cm²), with a = sqrt(Sd/π).
    fn validity(&self, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        let mut v = self.parts.validity(air, level);
        let a = (self.params.sd / PI).sqrt();
        v.push(ValidityLimit {
            element: self.id.clone(),
            criterion: "rigid piston (ka < 1)",
            begin_hz: Some(air.c / (2.0 * PI * a)),
            deep_hz: None,
        });
        v
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Parameters read from the inline keys and, optionally, a record; a key
/// given in both is an error.
struct Sources {
    inline: Params,
    record: Option<Params>,
}

impl Sources {
    fn pick<T>(&self, base: &str, a: Option<T>, r: Option<T>) -> Result<Option<T>> {
        match (a, r) {
            (Some(_), Some(_)) => Err(crate::Error::element(
                self.inline.context(),
                format!("'{base}' is given inline and in the record"),
            )),
            (a, None) => Ok(a),
            (None, r) => Ok(r),
        }
    }

    fn quantity_opt(&mut self, base: &str, dim: Dim) -> Result<Option<f64>> {
        let a = self.inline.quantity_opt(base, dim)?;
        let r = match &mut self.record {
            Some(p) => p.quantity_opt(base, dim)?,
            None => None,
        };
        self.pick(base, a, r)
    }

    fn number_opt(&mut self, key: &str) -> Result<Option<f64>> {
        let a = self.inline.number_opt(key)?;
        let r = match &mut self.record {
            Some(p) => p.number_opt(key)?,
            None => None,
        };
        self.pick(key, a, r)
    }

    fn has(&self, base: &str) -> bool {
        self.inline.has(base) || self.record.as_ref().is_some_and(|p| p.has(base))
    }

    fn finish(self) -> Result<()> {
        self.inline.finish()?;
        if let Some(r) = self.record {
            r.finish()?;
        }
        Ok(())
    }
}

fn driver(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(3, 4)?;
    let e_p = b.terminal(0, Domain::Electrical)?;
    let e_n = b.terminal(1, Domain::Electrical)?;
    let front = b.terminal(2, Domain::Acoustic)?;
    let rear = b.terminal_or_ground(3, Domain::Acoustic)?;
    let e_int = b.internal_node("e", Domain::Electrical)?;
    let m1 = b.internal_node("m", Domain::Mechanical)?;
    let m2 = b.internal_node("m2", Domain::Mechanical)?;
    let id = b.id.clone();
    let err = |m: String| crate::Error::element(id.clone(), m);

    let mut inline = std::mem::replace(&mut b.params, Params::new(id.clone(), Map::new()));
    let level = match inline.string_opt("model")? {
        None => DriverLevel::D1,
        Some(s) => DriverLevel::parse(&s)
            .ok_or_else(|| err(format!("unknown model '{s}' (D0, D1, D2)")))?,
    };
    let record_name = inline.string_opt("record")?;
    let record = match &record_name {
        None => None,
        Some(name) => {
            let rec = record(name).map_err(&err)?;
            if let Some(a) = governance(&rec).primary_anomaly() {
                return Err(err(format!(
                    "record '{name}' fails governance: {} looks like {}",
                    a.printed, a.likely
                )));
            }
            Some(Params::new(id.clone(), rec.element_params()))
        }
    };
    let mut src = Sources { inline, record };

    // D0 parameter set.
    let ts_mode = src.has("fs") || src.has("Qms") || src.has("Qes");
    let params = if ts_mode {
        for k in ["Bl", "Cms", "Kms", "Rms"] {
            if src.has(k) {
                return Err(err(format!(
                    "'{k}' cannot be combined with the primary set (fs, Qms, Qes, Re, Mms, Sd); it is derived"
                )));
            }
        }
        let need = |x: Option<f64>, k: &str| x.ok_or_else(|| err(format!("missing '{k}'")));
        let tsp = TsParams {
            fs: need(src.quantity_opt("fs", Dim::Frequency)?, "fs")?,
            qms: need(src.number_opt("Qms")?, "Qms")?,
            qes: need(src.number_opt("Qes")?, "Qes")?,
            re: need(src.quantity_opt("Re", Dim::ElecResistance)?, "Re")?,
            mms: need(src.quantity_opt("Mms", Dim::Mass)?, "Mms")?,
            sd: need(src.quantity_opt("Sd", Dim::Area)?, "Sd")?,
        };
        tsp.validate().map_err(&err)?;
        tsp.to_physical()
    } else {
        let need = |x: Option<f64>, k: &str| x.ok_or_else(|| err(format!("missing '{k}'")));
        let cms = match (
            src.quantity_opt("Cms", Dim::MechCompliance)?,
            src.quantity_opt("Kms", Dim::MechStiffness)?,
        ) {
            (Some(c), None) => c,
            (None, Some(k)) => 1.0 / k,
            _ => {
                return Err(err(
                    "give exactly one of 'Cms' or 'Kms' (or the primary set fs, Qms, Qes)".into(),
                ))
            }
        };
        let p = PhysicalParams {
            re: need(src.quantity_opt("Re", Dim::ElecResistance)?, "Re")?,
            bl: need(src.quantity_opt("Bl", Dim::ForceFactor)?, "Bl")?,
            mms: need(src.quantity_opt("Mms", Dim::Mass)?, "Mms")?,
            cms,
            rms: src.quantity_opt("Rms", Dim::MechResistance)?.unwrap_or(0.0),
            sd: need(src.quantity_opt("Sd", Dim::Area)?, "Sd")?,
        };
        p.validate().map_err(&err)?;
        p
    };

    // D1 keys (read at every level so that switching level never changes the netlist).
    let le = src.quantity_opt("Le", Dim::Inductance)?.unwrap_or(0.0);
    if le.is_nan() || le < 0.0 {
        return Err(err("'Le' must be non-negative".into()));
    }
    let lr2 = match (
        src.quantity_opt("L2", Dim::Inductance)?,
        src.quantity_opt("R2", Dim::ElecResistance)?,
    ) {
        (Some(l), Some(r)) if l > 0.0 && r > 0.0 => Some((l, r)),
        (None, None) => None,
        _ => return Err(err("LR-2 needs positive 'L2' and 'R2'".into())),
    };

    // Creep.
    let creep = match (
        src.number_opt("creep_lambda")?,
        src.quantity_opt("creep_f0", Dim::Frequency)?,
    ) {
        (None, None) => None,
        (None, Some(_)) => return Err(err("'creep_f0' needs 'creep_lambda'".into())),
        (Some(lambda), f0) => {
            let c = Creep {
                lambda,
                f0: f0.unwrap_or_else(|| params.fs()),
            };
            c.validate().map_err(&err)?;
            Some(c)
        }
    };

    // D2 keys.
    let s_mass = src.quantity_opt("Msur", Dim::Mass)?;
    let s_area = src.quantity_opt("Ssur", Dim::Area)?;
    let k_bend = match (
        src.quantity_opt("Kbend", Dim::MechStiffness)?,
        src.quantity_opt("Cbend", Dim::MechCompliance)?,
    ) {
        (Some(k), None) => Some(k),
        (None, Some(c)) => Some(1.0 / c),
        (None, None) => None,
        _ => return Err(err("give at most one of 'Kbend' or 'Cbend'".into())),
    };
    let r_bend = src.quantity_opt("Rbend", Dim::MechResistance)?;
    let surround = match (s_mass, s_area, k_bend) {
        (Some(mass), Some(area), Some(k_bend)) => {
            let r_bend = r_bend.unwrap_or(0.0);
            if !(mass > 0.0 && area > 0.0 && k_bend > 0.0 && r_bend >= 0.0) {
                return Err(err(
                    "D2 needs positive 'Msur', 'Ssur', 'Kbend'/'Cbend' and non-negative 'Rbend'"
                        .into(),
                ));
            }
            Some(Surround {
                mass,
                area,
                k_bend,
                r_bend,
            })
        }
        (None, None, None) if r_bend.is_none() => None,
        _ => {
            return Err(err(
                "the D2 surround needs all of 'Msur', 'Ssur' and 'Kbend' (or 'Cbend')".into(),
            ))
        }
    };
    src.finish()?;

    let two_dof = match (level, &surround) {
        (DriverLevel::D2, Some(s)) => Some(TwoDof::split(&params, s).map_err(&err)?),
        (DriverLevel::D2, None) => {
            return Err(err(
                "model D2 needs the surround keys 'Msur', 'Ssur' and 'Kbend' (or 'Cbend')".into(),
            ))
        }
        _ => None,
    };

    // Parts.
    let mut parts: Vec<Box<dyn Element>> = Vec::new();
    let coil = coil_model(level, &params, le, lr2);
    let coil_i = parts.len();
    parts.push(Box::new(OnePort {
        id: format!("{id}.coil"),
        type_name: "driver.coil",
        n1: e_p,
        n2: e_int,
        y: Box::new(move |cx: &FreqCx| coil.impedance(cx.omega).inv()),
        limits: Vec::new(),
    }));
    let motor_i = parts.len();
    parts.push(Box::new(Motor {
        id: format!("{id}.motor"),
        e: (e_int, e_n),
        m: (m1, None),
        bl: params.bl,
    }));
    let dome_i = parts.len();
    let dome_y: Box<dyn Fn(&FreqCx) -> C64 + Send + Sync> = match two_dof {
        None => {
            let (m, r, c0) = (params.mms, params.rms, params.cms);
            Box::new(move |cx: &FreqCx| {
                cx.jw() * m + r + (cx.jw() * c0 * creep_factor(creep, cx.omega)).inv()
            })
        }
        Some(t) => Box::new(move |cx: &FreqCx| cx.jw() * t.m_dome + t.r_dome),
    };
    parts.push(Box::new(OnePort {
        id: format!("{id}.dome"),
        type_name: "driver.dome",
        n1: m1,
        n2: None,
        y: dome_y,
        limits: Vec::new(),
    }));
    let link_i = parts.len();
    let mut sur_i = None;
    match two_dof {
        None => parts.push(Box::new(Short {
            id: format!("{id}.rigid"),
            p: m1,
            n: m2,
        })),
        Some(t) => {
            parts.push(Box::new(OnePort {
                id: format!("{id}.bend"),
                type_name: "driver.bend",
                n1: m1,
                n2: m2,
                y: Box::new(move |cx: &FreqCx| t.y_bend(cx.omega, creep_factor(creep, cx.omega))),
                limits: Vec::new(),
            }));
            sur_i = Some(parts.len());
            parts.push(Box::new(OnePort {
                id: format!("{id}.surround"),
                type_name: "driver.surround",
                n1: m2,
                n2: None,
                y: Box::new(move |cx: &FreqCx| {
                    t.y_surround(cx.omega, creep_factor(creep, cx.omega))
                }),
                limits: Vec::new(),
            }));
        }
    }
    let piston1_i = parts.len();
    parts.push(Box::new(Piston {
        id: format!("{id}.piston"),
        m: (m1, None),
        front,
        rear,
        sd: two_dof.map_or(params.sd, |t| t.s_dome),
    }));
    let mut piston2_i = None;
    if let Some(t) = two_dof {
        piston2_i = Some(parts.len());
        parts.push(Box::new(Piston {
            id: format!("{id}.surround_piston"),
            m: (m2, None),
            front,
            rear,
            sd: t.s_surround,
        }));
    }

    // Ports.
    let mut acoustic = vec![(piston1_i, 1, 1.0)];
    let mech1 = vec![
        (motor_i, 1, 1.0),
        (dome_i, 0, 1.0),
        (link_i, 0, 1.0),
        (piston1_i, 0, 1.0),
    ];
    let mut mech2 = vec![(link_i, 0, -1.0)];
    if let (Some(s), Some(p2)) = (sur_i, piston2_i) {
        acoustic.push((p2, 1, 1.0));
        mech2.push((s, 0, 1.0));
        mech2.push((p2, 0, 1.0));
    }
    let ports = vec![
        TapPort {
            plus: e_p,
            minus: e_n,
            taps: vec![(coil_i, 0, 1.0)],
        },
        TapPort {
            plus: front,
            minus: rear,
            taps: acoustic,
        },
        TapPort {
            plus: m1,
            minus: None,
            taps: mech1,
        },
        TapPort {
            plus: m2,
            minus: None,
            taps: mech2,
        },
    ];
    let mut offsets = Vec::with_capacity(parts.len());
    let mut acc = 0;
    for p in &parts {
        offsets.push(acc);
        acc += p.branch_count();
    }
    Ok(Box::new(Driver {
        id: id.clone(),
        level,
        params,
        le,
        lr2,
        creep,
        two_dof,
        record: record_name,
        parts: Composite {
            id,
            type_name: "driver",
            parts,
            ports: Vec::new(),
        },
        offsets,
        ports,
    }))
}
