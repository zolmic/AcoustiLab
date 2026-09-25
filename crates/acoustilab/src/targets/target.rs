//! Typed targets (spec Section 11, "Target object"), the bundled set,
//! personalisation shelves, the Harman-style reconstruction and preference
//! bands.
//!
//! A target names its fixture, reference point, baseline, normalisation
//! frequency (500 Hz), valid range, provenance class, licence and source,
//! and carries flags such as `approximation`. Only data whose licence allows
//! redistribution is bundled: the Ravizza et al. 2023 curve set on the
//! B&K 5128 (CC-BY-4.0, `data/targets/ravizza2023_5128.json`). Harman targets
//! are not redistributable; [`reconstruct_harman_style`] rebuilds a
//! Harman-style target from a user-supplied fixture baseline and published
//! shelf settings (docs/targets.md).

use super::curve::{Curve, END_SLACK};
use super::fixture;
use super::{embedded_json, shelf, Result, TargetError};
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

/// Where a target comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceClass {
    /// Peer-reviewed as a complete manuscript.
    PeerReviewed,
    /// Published research reviewed on a summary only (e.g. AES Express
    /// Papers).
    Research,
    Manufacturer,
    /// Community curves (not peer-reviewed).
    Community,
    /// Supplied by the user of this tool.
    User,
}

impl ProvenanceClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ProvenanceClass::PeerReviewed => "peer_reviewed",
            ProvenanceClass::Research => "research",
            ProvenanceClass::Manufacturer => "manufacturer",
            ProvenanceClass::Community => "community",
            ProvenanceClass::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<ProvenanceClass> {
        Some(match s {
            "peer_reviewed" => ProvenanceClass::PeerReviewed,
            "research" => ProvenanceClass::Research,
            "manufacturer" => ProvenanceClass::Manufacturer,
            "community" => ProvenanceClass::Community,
            "user" => ProvenanceClass::User,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Provenance {
    pub class: ProvenanceClass,
    pub source: String,
    pub doi: Option<String>,
    pub url: Option<String>,
    /// SPDX identifier where one applies, otherwise a sentence.
    pub licence: String,
    pub retrieved: Option<String>,
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub name: String,
    pub label: String,
    pub version: String,
    /// Lineage used to check a preference model's training pair, e.g.
    /// `harman_ae_oe_2018` or `ravizza2023`.
    pub family: String,
    /// Display group of related targets, if any.
    pub group: Option<String>,
    /// The recommended member of its group.
    pub primary: bool,
    pub fixture: String,
    pub reference_point: String,
    pub baseline: String,
    pub normalisation_hz: f64,
    pub valid_range_hz: (f64, f64),
    pub provenance: Provenance,
    pub flags: Vec<String>,
    pub notes: Vec<String>,
    pub curve: Curve,
    /// Dataset-specific extras (e.g. preference ratings); `Null` if none.
    pub extra: Value,
}

fn opt_str(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

impl Target {
    /// The target's JSON form, which [`Target::from_json`] reads back.
    pub fn to_json(&self) -> Value {
        let p = &self.provenance;
        let mut o = json!({
            "name": self.name,
            "label": self.label,
            "version": self.version,
            "family": self.family,
            "group": self.group,
            "primary": self.primary,
            "fixture": self.fixture,
            "fixture_label": fixture::fixture(&self.fixture).map(|f| f.label.clone()),
            "reference_point": self.reference_point,
            "baseline": self.baseline,
            "normalisation_Hz": self.normalisation_hz,
            "valid_range_Hz": [self.valid_range_hz.0, self.valid_range_hz.1],
            "provenance": {
                "class": p.class.as_str(),
                "source": p.source,
                "doi": p.doi,
                "url": p.url,
                "licence": p.licence,
                "retrieved": p.retrieved,
                "attribution": p.attribution,
            },
            "flags": self.flags,
            "notes": self.notes,
            "frequencies_Hz": self.curve.freqs(),
            "dB": self.curve.db(),
        });
        if !self.extra.is_null() {
            o["extra"] = self.extra.clone();
        }
        o
    }

    /// Reads a target object (the form [`Target::to_json`] writes, or a
    /// user's own). Required: `name`, `fixture`, `frequencies_Hz`, `dB`.
    /// Unknown keys are rejected. A target without a `provenance` block is a
    /// user target with an unstated licence.
    pub fn from_json(v: &Value) -> Result<Target> {
        let name = opt_str(v, "name").unwrap_or_default();
        let err = |msg: String| TargetError::Target {
            name: if name.is_empty() {
                "(unnamed)".into()
            } else {
                name.clone()
            },
            msg,
        };
        let obj = v
            .as_object()
            .ok_or_else(|| err("a target must be a JSON object or a bundled target name".into()))?;
        const KEYS: [&str; 19] = [
            "name",
            "label",
            "version",
            "family",
            "group",
            "primary",
            "fixture",
            "fixture_label",
            "reference_point",
            "baseline",
            "normalisation_Hz",
            "valid_range_Hz",
            "provenance",
            "flags",
            "notes",
            "frequencies_Hz",
            "dB",
            "extra",
            "schema",
        ];
        if let Some(k) = obj.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(err(format!("unknown key '{k}'")));
        }
        if name.trim().is_empty() {
            return Err(err("needs a non-empty 'name'".into()));
        }
        let fixture = opt_str(v, "fixture")
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                err(
                    "needs a 'fixture': a target is never used without the fixture it belongs to"
                        .into(),
                )
            })?;
        let nums = |k: &str| -> Result<Vec<f64>> {
            v.get(k)
                .and_then(Value::as_array)
                .ok_or_else(|| err(format!("needs a '{k}' array")))?
                .iter()
                .map(|x| {
                    x.as_f64()
                        .ok_or_else(|| err(format!("'{k}' must hold numbers")))
                })
                .collect()
        };
        let curve =
            Curve::new(nums("frequencies_Hz")?, nums("dB")?).map_err(|e| err(e.to_string()))?;
        let (lo, hi) = curve.range();
        let valid_range_hz = match v.get("valid_range_Hz") {
            None | Some(Value::Null) => (lo, hi),
            Some(r) => {
                let a = r
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .ok_or_else(|| err("'valid_range_Hz' must be [low, high]".into()))?;
                let (a0, a1) = (a[0].as_f64(), a[1].as_f64());
                match (a0, a1) {
                    (Some(x), Some(y)) if x > 0.0 && y > x => (x, y),
                    _ => {
                        return Err(err(
                            "'valid_range_Hz' must be two increasing frequencies".into()
                        ))
                    }
                }
            }
        };
        let normalisation_hz = match v.get("normalisation_Hz") {
            None | Some(Value::Null) => 500.0,
            Some(x) => x
                .as_f64()
                .filter(|f| *f > 0.0)
                .ok_or_else(|| err("'normalisation_Hz' must be a positive number".into()))?,
        };
        let strings = |k: &str| -> Result<Vec<String>> {
            match v.get(k) {
                None | Some(Value::Null) => Ok(Vec::new()),
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|s| {
                        s.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| err(format!("'{k}' must hold strings")))
                    })
                    .collect(),
                _ => Err(err(format!("'{k}' must be an array of strings"))),
            }
        };
        let provenance = match v.get("provenance") {
            None | Some(Value::Null) => Provenance {
                class: ProvenanceClass::User,
                source: "user-supplied".into(),
                doi: None,
                url: None,
                licence: "not stated (user-supplied; not redistributed by AcoustiLab)".into(),
                retrieved: None,
                attribution: None,
            },
            Some(p) => {
                let po = p
                    .as_object()
                    .ok_or_else(|| err("'provenance' must be an object".into()))?;
                const PK: [&str; 7] = [
                    "class",
                    "source",
                    "doi",
                    "url",
                    "licence",
                    "retrieved",
                    "attribution",
                ];
                if let Some(k) = po.keys().find(|k| !PK.contains(&k.as_str())) {
                    return Err(err(format!("unknown provenance key '{k}'")));
                }
                let class = match opt_str(p, "class") {
                    None => ProvenanceClass::User,
                    Some(c) => ProvenanceClass::parse(&c).ok_or_else(|| {
                        err(format!(
                            "provenance class '{c}' is not one of peer_reviewed, research, manufacturer, community, user"
                        ))
                    })?,
                };
                Provenance {
                    class,
                    source: opt_str(p, "source").unwrap_or_else(|| "user-supplied".into()),
                    doi: opt_str(p, "doi"),
                    url: opt_str(p, "url"),
                    licence: opt_str(p, "licence").unwrap_or_else(|| {
                        "not stated (user-supplied; not redistributed by AcoustiLab)".into()
                    }),
                    retrieved: opt_str(p, "retrieved"),
                    attribution: opt_str(p, "attribution"),
                }
            }
        };
        Ok(Target {
            label: opt_str(v, "label").unwrap_or_else(|| name.clone()),
            version: opt_str(v, "version").unwrap_or_default(),
            family: opt_str(v, "family").unwrap_or_else(|| "user".into()),
            group: opt_str(v, "group"),
            primary: v.get("primary").and_then(Value::as_bool).unwrap_or(false),
            reference_point: opt_str(v, "reference_point").unwrap_or_else(|| "drp".into()),
            baseline: opt_str(v, "baseline").unwrap_or_else(|| "not stated".into()),
            normalisation_hz,
            valid_range_hz,
            provenance,
            flags: strings("flags")?,
            notes: strings("notes")?,
            extra: v.get("extra").cloned().unwrap_or(Value::Null),
            name,
            fixture,
            curve,
        })
    }

    /// The target's level on `grid`, with optional personalisation shelves,
    /// `None` outside its data or its valid range (both read with the
    /// curves' end slack, [`END_SLACK`]).
    pub fn on_grid(&self, grid: &[f64], shelves: Option<&Shelves>) -> Vec<Option<f64>> {
        let (lo, hi) = self.valid_range_hz;
        grid.iter()
            .map(|&f| {
                if f < lo * (1.0 - END_SLACK) || f > hi * (1.0 + END_SLACK) {
                    return None;
                }
                let t = self.curve.at(f)?;
                Some(t + shelves.map_or(0.0, |s| s.db_at(f)))
            })
            .collect()
    }

    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

// ---------------------------------------------------------------- bundled

fn ravizza_label(id: &str) -> String {
    let adapted = |what: &str| format!("{what}, adapted to the 5128 by Ravizza et al.");
    match id {
        "APHarm2015" => adapted("Harman 2015 over-ear target"),
        "APHarm2015v2" => adapted("Harman 2015 over-ear target, variant 2"),
        "APHarm2018" => adapted("Harman 2018 over-ear target"),
        "APHarm2018v2" => adapted("Harman 2018 over-ear target, variant 2"),
        "AVGAllMeas" => "Average of the eight measured headphones".into(),
        "DF5128" => "Diffuse-field response of the 5128".into(),
        "FF5128" => "Free-field response of the 5128".into(),
        "Soundguys" => "SoundGuys preference curve, adapted".into(),
        _ => match id.strip_prefix("HP") {
            Some(rest) => match rest.split_once("Mod") {
                Some((n, m)) => format!("Headphone {n}, modification {m}"),
                None => format!("Measured headphone {rest}"),
            },
            None => id.to_string(),
        },
    }
}

fn ravizza() -> Vec<Target> {
    let doc = embedded_json(
        "ravizza2023_5128.json",
        include_str!("../../../../data/targets/ravizza2023_5128.json"),
    );
    let p = &doc["provenance"];
    let provenance = Provenance {
        class: ProvenanceClass::parse(p["class"].as_str().unwrap_or_default())
            .expect("ravizza2023_5128.json: provenance class"),
        source: p["source"].as_str().unwrap_or_default().to_string(),
        doi: opt_str(p, "doi"),
        url: opt_str(p, "url"),
        licence: p["licence"].as_str().unwrap_or_default().to_string(),
        retrieved: opt_str(p, "retrieved"),
        attribution: opt_str(p, "attribution"),
    };
    let freqs: Vec<f64> = doc["band_centres_Hz"]
        .as_array()
        .expect("band_centres_Hz")
        .iter()
        .map(|x| x.as_f64().expect("band centre"))
        .collect();
    let n_curves = doc["curves"].as_array().map_or(0, Vec::len);
    let primary = doc["primary"]["curve"].as_str().unwrap_or_default();
    let baseline = doc["baseline"].as_str().unwrap_or_default();
    let representation = doc["representation"].as_str().unwrap_or_default();
    let group = "Ravizza et al. 2023 curve set (B&K 5128)";
    let mut out = Vec::new();
    for c in doc["curves"].as_array().expect("curves") {
        let id = c["id"].as_str().expect("curve id");
        let gains: Vec<f64> = c["gain_dB"]
            .as_array()
            .expect("gain_dB")
            .iter()
            .map(|x| x.as_f64().expect("gain"))
            .collect();
        let curve = Curve::new(freqs.clone(), gains).expect("ravizza curve");
        let r = &c["ratings"];
        let mut flags = vec!["third_octave".to_string()];
        let mut notes = vec![representation.to_string()];
        if id.starts_with("APHarm") || matches!(id, "DF5128" | "FF5128" | "Soundguys") {
            flags.push("adaptation".into());
            notes.push(
                "The depositors' adaptation of a third-party curve, published by them under CC-BY-4.0; not the originator's data."
                    .into(),
            );
        }
        if id.starts_with("HP6") {
            flags.push("data_anomaly".into());
            notes.push("+17.3 dB in the 20 kHz band between -17.3 dB neighbours: a sign error in the record is likely (kept as published).".into());
        }
        if id == "Soundguys" {
            flags.push("data_anomaly".into());
            notes.push("0.0 dB in the 25 kHz band after -18.1 dB at 20 kHz: probably a missing value (kept as published).".into());
        }
        let rating = format!(
            "mean preference {:.1} of 100 (rank {} of {n_curves})",
            r["mean"].as_f64().unwrap_or(f64::NAN),
            r["rank"].as_u64().unwrap_or(0)
        );
        let base = Target {
            name: format!("ravizza2023:{id}"),
            label: format!("{} [{id}; {rating}]", ravizza_label(id)),
            version: "Zenodo record v1.0 (2023-09-29)".into(),
            family: "ravizza2023".into(),
            group: Some(group.into()),
            primary: false,
            fixture: doc["fixture"].as_str().unwrap_or_default().to_string(),
            reference_point: doc["reference_point"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            baseline: baseline.to_string(),
            normalisation_hz: 500.0,
            valid_range_hz: (freqs[0], 20000.0),
            provenance: provenance.clone(),
            flags,
            notes,
            curve,
            extra: json!({"dataset_curve": id, "ratings": r}),
        };
        if id == primary {
            let mut t = base.clone();
            t.name = "ravizza2023_5128".into();
            t.label = format!(
                "Ravizza et al. 2023 over-ear target for the B&K 5128: top-rated curve ({id}; {rating})"
            );
            t.primary = true;
            t.flags.push("selection".into());
            t.notes.push(
                doc["primary"]["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
            out.insert(0, t);
        }
        out.push(base);
    }
    out
}

/// Every bundled target: `ravizza2023_5128` first, then the curve set as
/// `ravizza2023:<curve id>`.
pub fn bundled() -> &'static [Target] {
    static T: OnceLock<Vec<Target>> = OnceLock::new();
    T.get_or_init(ravizza)
}

/// A bundled target by name.
pub fn find(name: &str) -> Option<&'static Target> {
    bundled().iter().find(|t| t.name == name)
}

/// Resolves a target given as a bundled name or as a target object.
pub fn resolve(spec: &Value) -> Result<Target> {
    match spec {
        Value::String(name) => find(name).cloned().ok_or_else(|| TargetError::Target {
            name: name.clone(),
            msg: "no bundled target of that name (see targets_list)".into(),
        }),
        _ => Target::from_json(spec),
    }
}

// ------------------------------------------------------- personalisation

fn personalisation_doc() -> &'static Value {
    static D: OnceLock<Value> = OnceLock::new();
    D.get_or_init(|| {
        embedded_json(
            "personalisation.json",
            include_str!("../../../../data/targets/personalisation.json"),
        )
    })
}

/// Bass and treble shelves, stored separately from the reference target
/// (spec Section 11). Defaults: low shelf at 105 Hz and high shelf at
/// 2.5 kHz, Q = 1/√2, 0 dB (`data/targets/personalisation.json`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shelves {
    pub bass_db: f64,
    pub treble_db: f64,
    pub bass_fc_hz: f64,
    pub treble_fc_hz: f64,
    pub bass_q: f64,
    pub treble_q: f64,
}

impl Default for Shelves {
    fn default() -> Shelves {
        let s = &personalisation_doc()["shelves"];
        let num = |a: &str, b: &str| s[a][b].as_f64().expect("personalisation.json shelves");
        Shelves {
            bass_db: 0.0,
            treble_db: 0.0,
            bass_fc_hz: num("bass", "fc_Hz"),
            treble_fc_hz: num("treble", "fc_Hz"),
            bass_q: num("bass", "Q"),
            treble_q: num("treble", "Q"),
        }
    }
}

impl Shelves {
    /// Shelf gains in dB with the default corners and Q.
    pub fn gains(bass_db: f64, treble_db: f64) -> Shelves {
        Shelves {
            bass_db,
            treble_db,
            ..Shelves::default()
        }
    }

    /// Combined magnitude of both shelves at `f`, dB.
    pub fn db_at(&self, f: f64) -> f64 {
        shelf::low_shelf_db(f, self.bass_fc_hz, self.bass_db, self.bass_q)
            + shelf::high_shelf_db(f, self.treble_fc_hz, self.treble_db, self.treble_q)
    }

    pub fn is_flat(&self) -> bool {
        self.bass_db == 0.0 && self.treble_db == 0.0
    }

    pub fn to_json(&self) -> Value {
        json!({
            "bass_dB": self.bass_db, "treble_dB": self.treble_db,
            "bass_fc_Hz": self.bass_fc_hz, "treble_fc_Hz": self.treble_fc_hz,
            "bass_Q": self.bass_q, "treble_Q": self.treble_q,
        })
    }

    /// Reads `{"bass_dB", "treble_dB", "bass_fc_Hz", "treble_fc_Hz",
    /// "bass_Q", "treble_Q"}` (all optional) over the defaults. Accepted
    /// ranges: gains ±40 dB, corners 1 Hz–100 kHz, Q 0.1–10 (input limits
    /// that keep the filter arithmetic finite, not preference data).
    pub fn from_json(v: &Value) -> Result<Shelves> {
        let o = v
            .as_object()
            .ok_or_else(|| TargetError::Options("personalisation must be an object".into()))?;
        let mut s = Shelves::default();
        for (k, x) in o {
            let x = x.as_f64().filter(|x| x.is_finite()).ok_or_else(|| {
                TargetError::Options(format!("personalisation '{k}' must be a number"))
            })?;
            let within = |lo: f64, hi: f64| {
                if (lo..=hi).contains(&x) {
                    Ok(x)
                } else {
                    Err(TargetError::Options(format!(
                        "personalisation '{k}' = {x} is outside {lo} to {hi}"
                    )))
                }
            };
            match k.as_str() {
                "bass_dB" => s.bass_db = within(-40.0, 40.0)?,
                "treble_dB" => s.treble_db = within(-40.0, 40.0)?,
                "bass_fc_Hz" => s.bass_fc_hz = within(1.0, 1e5)?,
                "treble_fc_Hz" => s.treble_fc_hz = within(1.0, 1e5)?,
                "bass_Q" => s.bass_q = within(0.1, 10.0)?,
                "treble_Q" => s.treble_q = within(0.1, 10.0)?,
                _ => {
                    return Err(TargetError::Options(format!(
                        "unknown personalisation key '{k}'"
                    )))
                }
            }
        }
        Ok(s)
    }
}

/// The shelf settings of a published preset of the Harman-style recipe,
/// e.g. `olive_welti_2015_mean` (+6.44 dB bass, −1.41 dB treble).
pub fn recipe_preset(id: &str) -> Option<Shelves> {
    personalisation_doc()["recipe"]["presets"]
        .as_array()?
        .iter()
        .find(|p| p["id"] == id)
        .map(|p| {
            Shelves::gains(
                p["bass_dB"].as_f64().unwrap_or(0.0),
                p["treble_dB"].as_f64().unwrap_or(0.0),
            )
        })
}

/// Harman-style reconstruction: `baseline + bass shelf + treble shelf`.
///
/// The baseline must be the fixture's response to an accurate loudspeaker
/// equalised flat in a reference listening room, at the drum reference
/// point of the fixture the target is for (docs/targets.md). It is not
/// published as data and is never bundled; the user supplies it. The result
/// keeps the baseline's fixture, is flagged `approximation` and
/// `shelf_q_assumed`, and has family `harman_style_reconstruction`, so it is
/// never mistaken for a preference model's training target.
///
/// The curve is evaluated at the baseline's frequencies and at every
/// 1/48-octave grid point (base 10) inside its range, so the shelves are
/// resolved even on a sparse baseline.
pub fn reconstruct_harman_style(
    baseline: &Target,
    shelves: &Shelves,
    name: &str,
) -> Result<Target> {
    let (lo, hi) = baseline.curve.range();
    let mut f: Vec<f64> = baseline.curve.freqs().to_vec();
    let (k0, k1) = (
        (lo.log10() * 160.0).ceil() as i64,
        (hi.log10() * 160.0).floor() as i64,
    );
    f.extend((k0..=k1).map(|k| 10f64.powf(k as f64 / 160.0)));
    f.sort_by(f64::total_cmp);
    f.dedup_by(|a, b| (*a / *b - 1.0).abs() < 1e-9);
    f.retain(|&x| x >= lo && x <= hi);
    let db = f
        .iter()
        .map(|&x| {
            baseline
                .curve
                .at(x)
                .map(|b| b + shelves.db_at(x))
                .ok_or_else(|| TargetError::Curve(format!("baseline undefined at {x} Hz")))
        })
        .collect::<Result<Vec<f64>>>()?;
    let mut flags = vec!["approximation".to_string(), "shelf_q_assumed".to_string()];
    flags.extend(baseline.flags.iter().cloned());
    Ok(Target {
        name: name.to_string(),
        label: format!(
            "Harman-style reconstruction on '{}' ({:+.2} dB bass shelf at {} Hz, {:+.2} dB treble shelf at {} Hz)",
            baseline.name, shelves.bass_db, shelves.bass_fc_hz, shelves.treble_db, shelves.treble_fc_hz
        ),
        version: String::new(),
        family: "harman_style_reconstruction".into(),
        group: None,
        primary: false,
        fixture: baseline.fixture.clone(),
        reference_point: baseline.reference_point.clone(),
        baseline: format!("user-supplied baseline '{}': {}", baseline.name, baseline.baseline),
        normalisation_hz: baseline.normalisation_hz,
        valid_range_hz: baseline.valid_range_hz,
        provenance: Provenance {
            class: ProvenanceClass::User,
            source: format!(
                "Recipe of data/targets/personalisation.json (shelf frequencies after Olive, Welti and McMullin 2013 and Olive and Welti 2015) on the user's baseline; baseline source: {}",
                baseline.provenance.source
            ),
            doi: None,
            url: None,
            licence: baseline.provenance.licence.clone(),
            retrieved: None,
            attribution: baseline.provenance.attribution.clone(),
        },
        flags,
        notes: vec![
            "Parametric approximation, not Harman's published target: the shelf Q is not published (Q = 1/sqrt(2) assumed), and Harman's 2018 target also changed the region near 3 kHz, which no shelf reproduces.".into(),
        ],
        curve: Curve::new(f, db)?,
        extra: json!({"recipe": "harman_style", "shelves": shelves.to_json()}),
    })
}

// ------------------------------------------------------ preference bands

/// Parameters of a preference band: the bass-shelf gains of the listener
/// classes and the widening above 2 and 8 kHz
/// (`data/targets/personalisation.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct BandParams {
    /// Class id, population share, and the class's bass gain range (dB).
    pub classes: Vec<(String, f64, [f64; 2])>,
    /// (start Hz, full Hz, half-width dB) of each widening ramp.
    pub widening: Vec<(f64, f64, f64)>,
}

impl Default for BandParams {
    fn default() -> BandParams {
        let d = personalisation_doc();
        let classes = d["classes"]["items"]
            .as_array()
            .expect("personalisation.json classes")
            .iter()
            .map(|c| {
                let g = &c["bass_dB"];
                (
                    c["id"].as_str().unwrap_or_default().to_string(),
                    c["share"].as_f64().unwrap_or(0.0),
                    [g[0].as_f64().unwrap_or(0.0), g[1].as_f64().unwrap_or(0.0)],
                )
            })
            .collect();
        let w = &d["widening"];
        let ramp = |k: &str| {
            let r = &w[k];
            (
                r["start_Hz"].as_f64().expect("widening start"),
                r["full_Hz"].as_f64().expect("widening full"),
                r["half_width_dB"].as_f64().expect("widening half width"),
            )
        };
        BandParams {
            classes,
            widening: vec![ramp("above_2kHz"), ramp("above_8kHz")],
        }
    }
}

impl BandParams {
    /// Total widening at `f`: each ramp rises linearly on log frequency from
    /// 0 dB at its start to its half-width at its full frequency.
    pub fn widening_db(&self, f: f64) -> f64 {
        self.widening
            .iter()
            .map(|&(a, b, w)| w * ((f / a).ln() / (b / a).ln()).clamp(0.0, 1.0))
            .sum()
    }

    /// Every bass gain the classes span (both ends of each range).
    fn gains(&self) -> Vec<f64> {
        let mut g: Vec<f64> = self.classes.iter().flat_map(|c| c.2).collect();
        g.sort_by(f64::total_cmp);
        g.dedup();
        g
    }
}

/// A preference band relative to the reference target on a grid: the lower
/// and upper edges of `response − target` that the listener classes would
/// accept, each class variant normalised like the metrics, then widened.
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub lower: Vec<f64>,
    pub upper: Vec<f64>,
}

/// Builds the band on `grid`. A class variant is the target plus a bass
/// shelf of the class gain (default corner and Q); `offset(values)` is the
/// normalisation the metrics apply (so a +6 dB bass variant is compared
/// after the same 500 Hz alignment as the response).
pub fn preference_band(
    grid: &[f64],
    params: &BandParams,
    offset: impl Fn(&[Option<f64>]) -> Option<f64>,
) -> Band {
    let base = Shelves::default();
    let mut lower = vec![f64::INFINITY; grid.len()];
    let mut upper = vec![f64::NEG_INFINITY; grid.len()];
    for g in params.gains() {
        let s = Shelves { bass_db: g, ..base };
        let v: Vec<Option<f64>> = grid.iter().map(|&f| Some(s.db_at(f))).collect();
        let o = offset(&v).unwrap_or(0.0);
        for (k, x) in v.iter().enumerate() {
            let x = x.unwrap_or(0.0) - o;
            lower[k] = lower[k].min(x);
            upper[k] = upper[k].max(x);
        }
    }
    for (k, &f) in grid.iter().enumerate() {
        let w = params.widening_db(f);
        lower[k] -= w;
        upper[k] += w;
    }
    Band { lower, upper }
}

/// The listener classes as JSON (id, share, bass gain range, source).
pub fn classes_json(params: &BandParams) -> Value {
    let d = personalisation_doc();
    let labels: Map<String, Value> = d["classes"]["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| {
                    (
                        c["id"].as_str().unwrap_or_default().to_string(),
                        c["label"].clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "classes": params.classes.iter().map(|(id, share, g)| json!({
            "id": id, "label": labels.get(id), "share": share, "bass_dB": g,
        })).collect::<Vec<_>>(),
        "widening": params.widening.iter().map(|(a, b, w)| json!({
            "start_Hz": a, "full_Hz": b, "half_width_dB": w,
        })).collect::<Vec<_>>(),
        "source": d["classes"]["source"],
        "widening_basis": [d["widening"]["above_2kHz"]["basis"], d["widening"]["above_8kHz"]["basis"]],
    })
}
