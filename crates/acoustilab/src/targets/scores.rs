//! Harman preference models (spec Section 11 "Preference score", erratum
//! E7), read from `data/targets/preference_models.json`.
//!
//! `score = intercept − Σ weight·variable`, with the variables computed on
//! the error `e = response − target` on the evaluation grid, response (the
//! left-right average) and target both normalised at 500 Hz:
//!
//! * SD: sample standard deviation of e (ddof = 1) over the term's band;
//! * AS: |b|, b the least-squares slope of e against ln f (dB per neper of
//!   frequency; b·ln 2 is dB per octave);
//! * ME: mean of |e| over the term's band.
//!
//! Bands: over-ear SD and AS over 50 Hz–10 kHz (E7); in-ear SD and AS over
//! 20 Hz–10 kHz and ME over 40 Hz–10 kHz. The in-ear model has two
//! coefficient sets that differ by 10 to 30 points, each named by source.
//!
//! Greying (spec Section 11): a score is greyed when the response's fixture
//! or the target (fixture and family) differ from the model's training
//! pair, or when the data do not cover a term's band. Applied to a
//! simulated curve it carries the notice "simulated, not measured" (always,
//! in this tool today); on an IEC 60318-4 load the 20–100 Hz band is flagged
//! as coupler-extrapolated.

use super::fixture::{self, Match};
use super::metrics::{stats, Stats};
use super::target::Target;
use super::{embedded_json, Flag};
use serde_json::{json, Value};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variable {
    Sd,
    As,
    Me,
}

impl Variable {
    pub fn as_str(self) -> &'static str {
        match self {
            Variable::Sd => "SD",
            Variable::As => "AS",
            Variable::Me => "ME",
        }
    }

    fn of(self, s: &Stats) -> Option<f64> {
        match self {
            Variable::Sd => s.sd,
            Variable::As => s.slope.map(f64::abs),
            Variable::Me => Some(s.mae),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub variable: Variable,
    pub weight: f64,
    pub band_hz: (f64, f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub label: String,
    /// `over_ear` or `in_ear`.
    pub kind: String,
    pub intercept: f64,
    pub terms: Vec<Term>,
    pub training_fixture: String,
    pub training_family: String,
    pub source: String,
    pub fit: Value,
}

fn parse() -> Vec<Model> {
    let doc = embedded_json(
        "preference_models.json",
        include_str!("../../../../data/targets/preference_models.json"),
    );
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    doc["models"]
        .as_array()
        .expect("preference_models.json: models")
        .iter()
        .map(|m| Model {
            id: s(m, "id"),
            label: s(m, "label"),
            kind: s(m, "kind"),
            intercept: m["intercept"].as_f64().expect("intercept"),
            terms: m["terms"]
                .as_array()
                .expect("terms")
                .iter()
                .map(|t| Term {
                    variable: match t["variable"].as_str() {
                        Some("SD") => Variable::Sd,
                        Some("AS") => Variable::As,
                        Some("ME") => Variable::Me,
                        v => panic!("preference_models.json: variable {v:?}"),
                    },
                    weight: t["weight"].as_f64().expect("weight"),
                    band_hz: (
                        t["band_Hz"][0].as_f64().expect("band"),
                        t["band_Hz"][1].as_f64().expect("band"),
                    ),
                })
                .collect(),
            training_fixture: s(&m["training"], "fixture"),
            training_family: s(&m["training"], "target_family"),
            source: s(m, "source"),
            fit: m["fit"].clone(),
        })
        .collect()
}

pub fn models() -> &'static [Model] {
    static M: OnceLock<Vec<Model>> = OnceLock::new();
    M.get_or_init(parse)
}

pub fn model(id: &str) -> Option<&'static Model> {
    models().iter().find(|m| m.id == id)
}

/// A model evaluated on one error curve.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    pub model: &'static Model,
    /// `None` when a variable cannot be computed (fewer than two points).
    pub score: Option<f64>,
    /// Each term's variable value and the statistics it came from.
    pub variables: Vec<(Variable, Option<f64>, Option<Stats>)>,
    pub greyed: bool,
    pub flags: Vec<Flag>,
}

/// What a score is judged against for greying.
pub struct Context<'a> {
    /// The response's fixture, if known.
    pub fixture: Option<&'a str>,
    pub target: &'a Target,
    /// True for measured curves; false for simulated ones.
    pub measured: bool,
}

/// Evaluates `model` on the error `e` (normalised at 500 Hz) on `grid`.
pub fn evaluate(model: &'static Model, grid: &[f64], e: &[Option<f64>], cx: &Context) -> Score {
    let mut flags = Vec::new();
    let mut greyed = false;
    let mut variables = Vec::new();
    let mut score = Some(model.intercept);
    for t in &model.terms {
        let s = stats(grid, e, t.band_hz.0, t.band_hz.1);
        let v = s.as_ref().and_then(|s| t.variable.of(s));
        if s.as_ref().is_none_or(|s| s.partial) {
            greyed = true;
            let f = Flag::new(
                "partial_range",
                format!(
                    "{}-{} Hz is needed; the response and target do not cover it all",
                    t.band_hz.0, t.band_hz.1
                ),
            );
            if !flags.contains(&f) {
                flags.push(f);
            }
        }
        score = match (score, v) {
            (Some(x), Some(v)) => Some(x - t.weight * v),
            _ => None,
        };
        variables.push((t.variable, v, s));
    }
    match score {
        None => flags.push(Flag::new(
            "insufficient_data",
            "a variable needs at least two grid points in its band",
        )),
        Some(x) if !(0.0..=100.0).contains(&x) => flags.push(Flag::new(
            "outside_scale",
            "the linear model was fitted to ratings on a 0-100 scale; this value is an extrapolation",
        )),
        Some(_) => {}
    }
    let train = fixture::fixture(&model.training_fixture)
        .map_or(model.training_fixture.clone(), |f| f.label.clone());
    match cx.fixture {
        None => {
            greyed = true;
            flags.push(Flag::new(
                "training_fixture_mismatch",
                format!("the response's fixture is not stated; the model was trained on {train}"),
            ));
        }
        Some(f) if fixture::compare(f, &model.training_fixture) != Match::Same => {
            greyed = true;
            flags.push(Flag::new(
                "training_fixture_mismatch",
                format!("the response is on '{f}'; the model was trained on {train}"),
            ));
        }
        Some(_) => {}
    }
    if cx.target.family != model.training_family || cx.target.fixture != model.training_fixture {
        greyed = true;
        flags.push(Flag::new(
            "training_target_mismatch",
            format!(
                "the target '{}' (family '{}', fixture '{}') is not the model's training target (family '{}' on '{}')",
                cx.target.name,
                cx.target.family,
                cx.target.fixture,
                model.training_family,
                model.training_fixture
            ),
        ));
    }
    if !cx.measured {
        flags.push(Flag::new(
            "simulated",
            "simulated, not measured: no preference model is validated on simulated curves",
        ));
    }
    let lowest = model
        .terms
        .iter()
        .map(|t| t.band_hz.0)
        .fold(f64::INFINITY, f64::min);
    if lowest < 100.0
        && cx
            .fixture
            .and_then(fixture::fixture)
            .is_some_and(|f| f.is_iec60318_4())
    {
        flags.push(Flag::new(
            "coupler_extrapolated",
            format!(
                "{lowest}-100 Hz is coupler-extrapolated: IEC 60318-4 is not validated below 100 Hz"
            ),
        ));
    }
    Score {
        model,
        score,
        variables,
        greyed,
        flags,
    }
}

impl Score {
    pub fn to_json(&self) -> Value {
        let m = self.model;
        json!({
            "model": m.id,
            "label": m.label,
            "kind": m.kind,
            "score": self.score,
            "greyed": self.greyed,
            "variables": self.variables.iter().zip(&m.terms).map(|((v, x, s), t)| json!({
                "variable": v.as_str(),
                "value": x,
                "weight": t.weight,
                "band_Hz": [t.band_hz.0, t.band_hz.1],
                "used_Hz": s.as_ref().and_then(|s| s.used_hz).map(|(a, b)| [a, b]),
                "n": s.as_ref().map_or(0, |s| s.n),
            })).collect::<Vec<_>>(),
            "formula": format!(
                "{} - {}",
                m.intercept,
                m.terms.iter().map(|t| format!("{}*{}", t.weight, t.variable.as_str())).collect::<Vec<_>>().join(" - ")
            ),
            "training": {"fixture": m.training_fixture, "target_family": m.training_family},
            "fit": m.fit,
            "source": m.source,
            "flags": self.flags,
        })
    }
}
