//! JSON API of the design analyses (docs/analysis.md), behind the
//! `#[wasm_bindgen]` shims at the end of `lib.rs`.
//!
//! Every function takes the netlist text, parameter overrides
//! (`{"name": value}` or "" for none) and an options object ("" for the
//! defaults), and returns a JSON value: the result, or `{"error", "kind",
//! ...}`. Engine errors use [`crate::api::error_value`]; malformed or
//! invalid options have kind `options`.
//!
//! Each call is bounded so a worker can report progress and cancel between
//! calls: a Monte Carlo run is [`mc_plan_value`] once, then
//! [`mc_run_value`] on slices of the plan's samples, then
//! [`mc_envelope_value`] (and [`mc_csv_value`]) on the collected results.
//! A sensitivity map can be split by passing a few `parameters` per call.

use crate::api::{error_value, parse_overrides};
use acoustilab::analysis::explain::{self, ExplainOptions};
use acoustilab::analysis::mc::{self, PlanSpec, RunChunk, RunOptions, Sample};
use acoustilab::analysis::readouts::{self, ReadoutOptions};
use acoustilab::analysis::sensitivity::{self, SensitivityOptions};
use acoustilab::analysis::tornado::{self, TornadoOptions};
use acoustilab::analysis::{parse_options, Design};
use serde_json::{json, Value};

fn options_error(msg: impl std::fmt::Display) -> Value {
    json!({"error": msg.to_string(), "kind": "options"})
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
}

/// Parses an options JSON text ("" means the defaults) into the given
/// options type, or returns an `options` error object.
macro_rules! options {
    ($ty:ty, $what:expr, $text:expr) => {{
        let text: &str = $text;
        if text.trim().is_empty() {
            <$ty>::default()
        } else {
            let v: Value = tri!(serde_json::from_str(text)
                .map_err(|e| options_error(format!("{} options JSON: {e}", $what))));
            tri!(parse_options::<$ty>($what, Some(&v))
                .map_err(|e| options_error(e.to_string().trim_start_matches("netlist: "))))
        }
    }};
}

fn design(netlist: &str, overrides: &str) -> Result<Design, Value> {
    let o = parse_overrides(overrides)?;
    Design::parse(netlist, &o).map_err(|e| error_value(&e))
}

/// The result document, or the engine error as an error object.
macro_rules! to_value {
    ($r:expr) => {
        match $r {
            Ok(v) => serde_json::to_value(v)
                .unwrap_or_else(|e| json!({"error": e.to_string(), "kind": "other"})),
            Err(e) => error_value(&e),
        }
    };
}

/// Sensitivities: the [`sensitivity::Jacobian`] document.
pub fn sensitivity_value(netlist: &str, overrides: &str, opts: &str) -> Value {
    let o = options!(SensitivityOptions, "sensitivity", opts);
    tri!(o.validate().map_err(options_error));
    let d = tri!(design(netlist, overrides));
    to_value!(sensitivity::jacobian(&d, &o))
}

/// Tornado chart: the [`tornado::Tornado`] document.
pub fn tornado_value(netlist: &str, overrides: &str, opts: &str) -> Value {
    let o = options!(TornadoOptions, "tornado", opts);
    tri!(o.validate().map_err(options_error));
    let d = tri!(design(netlist, overrides));
    to_value!(tornado::tornado(&d, &o))
}

/// Explain sentences: the [`explain::Explanation`] document.
pub fn explain_value(netlist: &str, overrides: &str, opts: &str) -> Value {
    let o = options!(ExplainOptions, "explain", opts);
    tri!(o.validate().map_err(options_error));
    let d = tri!(design(netlist, overrides));
    to_value!(explain::explain(&d, &o))
}

/// Readouts: the [`readouts::Readouts`] document.
pub fn readouts_value(netlist: &str, overrides: &str, opts: &str) -> Value {
    let o = options!(ReadoutOptions, "readout", opts);
    tri!(o.validate().map_err(options_error));
    let d = tri!(design(netlist, overrides));
    to_value!(readouts::readouts(&d, &o))
}

/// A Monte Carlo or design-of-experiments plan ([`mc::Plan`]) from a plan
/// spec such as `{"method": "lhs", "n": 200, "seed": 1}`.
pub fn mc_plan_value(netlist: &str, overrides: &str, spec: &str) -> Value {
    let v: Value =
        tri!(serde_json::from_str(spec).map_err(|e| options_error(format!("plan JSON: {e}"))));
    let s: PlanSpec =
        tri!(serde_json::from_value(v).map_err(|e| options_error(format!("plan: {e}"))));
    tri!(s.validate().map_err(options_error));
    let d = tri!(design(netlist, overrides));
    to_value!(mc::plan(&d, &s))
}

/// Solves one chunk of runs and returns a [`mc::RunChunk`]. `which` is
/// either `{"plan": <plan spec>, "first": i, "count": k}`, which re-plans
/// (deterministically, in microseconds) and runs samples i..i+k, or a JSON
/// array of samples `{index, overrides}`. Prefer the first form: sampled
/// values then never pass through a JSON parser (serde_json's float parsing
/// is best effort and may move a value by one ulp).
pub fn mc_run_value(netlist: &str, overrides: &str, which: &str, opts: &str) -> Value {
    let o = options!(RunOptions, "run", opts);
    tri!(o.validate().map_err(options_error));
    let w: Value =
        tri!(serde_json::from_str(which).map_err(|e| options_error(format!("runs JSON: {e}"))));
    let d = tri!(design(netlist, overrides));
    let samples: Vec<Sample> = match &w {
        Value::Object(m) if m.contains_key("plan") => {
            if let Some(k) = m
                .keys()
                .find(|k| !["plan", "first", "count"].contains(&k.as_str()))
            {
                return options_error(format!("runs: unknown key '{k}'"));
            }
            let spec: PlanSpec = tri!(serde_json::from_value(m["plan"].clone())
                .map_err(|e| options_error(format!("plan: {e}"))));
            tri!(spec.validate().map_err(options_error));
            let plan = tri!(mc::plan(&d, &spec).map_err(|e| error_value(&e)));
            let n = plan.samples.len();
            let index = |key: &str, default: usize| -> Result<usize, Value> {
                match m.get(key) {
                    None => Ok(default),
                    Some(v) => v
                        .as_u64()
                        .map(|x| x as usize)
                        .ok_or_else(|| options_error(format!("runs: '{key}' must be an integer"))),
                }
            };
            let first = tri!(index("first", 0)).min(n);
            let count = tri!(index("count", n));
            plan.samples[first..(first + count).min(n)].to_vec()
        }
        _ => tri!(serde_json::from_value(w).map_err(|e| options_error(format!("samples: {e}")))),
    };
    to_value!(mc::run(&d, &samples, &o))
}

/// Envelopes of collected runs: the input is a [`mc::RunChunk`]
/// (`{"frequencies_Hz", "samples"}`, the samples of all chunks together).
pub fn mc_envelope_value(chunk: &str) -> Value {
    let c: RunChunk =
        tri!(serde_json::from_str(chunk).map_err(|e| options_error(format!("runs: {e}"))));
    to_value!(acoustilab::Result::Ok(mc::envelope(
        &c.freqs_hz,
        &c.samples
    )))
}

/// The design-of-experiments table of collected runs as CSV text,
/// `{"csv": ".."}`, with the plan's parameter columns first
/// (`parameters`: a JSON array of names, or "" for the runs' own).
pub fn mc_csv_value(chunk: &str, parameters: &str) -> Value {
    let c: RunChunk =
        tri!(serde_json::from_str(chunk).map_err(|e| options_error(format!("runs: {e}"))));
    let p: Vec<String> = if parameters.trim().is_empty() {
        Vec::new()
    } else {
        tri!(serde_json::from_str(parameters).map_err(|e| options_error(format!("parameters: {e}"))))
    };
    json!({"csv": mc::to_csv(&p, &c.samples, &c.engine)})
}
