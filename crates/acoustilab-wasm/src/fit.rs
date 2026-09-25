//! JSON-in, JSON-out API of the curve and fitting exports (docs/fitting.md).
//!
//! * [`import_curve_value`]`(text, options)`: reads FRD, ZMA, REW text or
//!   CSV into a curve document (`acoustilab-curve/0.1`). `options` is a
//!   format name (`auto`, `frd`, `zma`, `rew`, `csv`) or a JSON object
//!   `{"format", "quantity", "sidecar"}`, where `sidecar` is the curve's
//!   sidecar document.
//! * [`export_curve_value`]`(curve_json, format)`: `{"format", "extension",
//!   "text", "sidecar"}`, the file and its sidecar text.
//! * [`compare_curves_value`]`(a_json, b_json, allow_json)`: the sidecar
//!   compatibility check, `{"ok", "blocking": [{field, a, b}], "notes": [..],
//!   "message"}`; `allow_json` is `[]`, a list of field names, or `["all"]`.
//! * [`fit_value`]`(netlist, spec_json)`: the fit report
//!   (`acoustilab-fit-report/0.1`). Runtime is bounded by the spec's
//!   `max_iterations` and `max_evaluations`; a worker that wants progress
//!   and cancellation runs a few iterations per call and passes the
//!   report's `fitted` values back as the next call's `start` values.
//! * [`virtual_measure_value`]`(netlist, spec_json)`: a virtual-rig
//!   measurement, `{"curve", "format", "text", "sidecar"}`; the spec is the
//!   rig's JSON form plus an optional `"format"` for `text` (default: `zma`
//!   for impedance, `frd` otherwise).
//!
//! * [`probe_curve_value`]`(netlist, overrides_json, probe)`: probe `probe`
//!   of the solved netlist (its own sweep) as a curve document with a
//!   `simulated` sidecar, ready for [`export_curve_value`].
//!
//! Errors are `{"error", "kind", ...}`: `curve` (with `line` when the input
//! file has one), `fit_spec`, `fit_refused` (spec Section 12 refusals), or
//! the engine kinds of [`crate::api::error_value`].

use crate::api;
use acoustilab::fit::rig::{self, RigSpec};
use acoustilab::fit::{self, FitError, FitSpec};
use acoustilab::io::{sidecar, text, Curve, CurveError, Format, Quantity, Sidecar};
use acoustilab::params::Parametric;
use serde_json::{json, Value};

fn curve_error(e: &CurveError) -> Value {
    let mut v = json!({"error": e.to_string(), "kind": "curve"});
    if let Some(l) = e.line {
        v["line"] = json!(l);
    }
    v
}

fn fit_error(e: &FitError) -> Value {
    match e {
        FitError::Spec(_) => json!({"error": e.to_string(), "kind": "fit_spec"}),
        FitError::Refused(_) => json!({"error": e.to_string(), "kind": "fit_refused"}),
        FitError::Curve(c) => curve_error(c),
        FitError::Engine(err) => api::error_value(err),
    }
}

fn parse_json(text: &str, what: &str) -> Result<Value, Value> {
    serde_json::from_str(text).map_err(|e| {
        json!({"error": format!("{what} JSON: {e}"), "kind": "json",
               "line": e.line(), "column": e.column()})
    })
}

fn format_of(name: &str) -> Result<Format, Value> {
    Format::parse(name).ok_or_else(|| {
        json!({"error": format!("unknown format '{name}' (auto, frd, zma, rew, csv)"),
               "kind": "curve"})
    })
}

/// Reads a curve file (module documentation).
pub fn import_curve_value(text_in: &str, options: &str) -> Value {
    let (format, quantity, sidecar_doc) = if options.trim_start().starts_with('{') {
        let o = match parse_json(options, "options") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let Some(m) = o.as_object() else {
            return json!({"error": "options must be an object", "kind": "curve"});
        };
        if let Some(k) = m
            .keys()
            .find(|k| !["format", "quantity", "sidecar"].contains(&k.as_str()))
        {
            return json!({"error": format!("options: unknown key '{k}' (format, quantity, sidecar)"),
                          "kind": "curve"});
        }
        let format = match format_of(m.get("format").and_then(Value::as_str).unwrap_or("auto")) {
            Ok(f) => f,
            Err(e) => return e,
        };
        let quantity = match m.get("quantity").and_then(Value::as_str) {
            None => None,
            Some(q) => match Quantity::parse(q) {
                Some(q) => Some(q),
                None => return json!({"error": format!("unknown quantity '{q}'"), "kind": "curve"}),
            },
        };
        (format, quantity, m.get("sidecar").cloned())
    } else {
        match format_of(options) {
            Ok(f) => (f, None, None),
            Err(e) => return e,
        }
    };
    let sc = match sidecar_doc.map(|v| Sidecar::from_json(&v)).transpose() {
        Ok(s) => s,
        Err(e) => return curve_error(&e),
    };
    let quantity = quantity.or(sc.as_ref().and_then(|s| s.quantity));
    match text::import(text_in, format, quantity) {
        Ok(mut c) => {
            if let Some(s) = sc {
                c.sidecar = s;
                c.sidecar.quantity = Some(c.quantity);
            }
            c.to_json()
        }
        Err(e) => curve_error(&e),
    }
}

/// Writes a curve document as a file of `format` (module documentation).
pub fn export_curve_value(curve_json: &str, format: &str) -> Value {
    let v = match parse_json(curve_json, "curve") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let c = match Curve::from_json(&v) {
        Ok(c) => c,
        Err(e) => return curve_error(&e),
    };
    let f = match format_of(format) {
        Ok(f) => f,
        Err(e) => return e,
    };
    match text::export(&c, f) {
        Ok(t) => {
            let mut sc = c.sidecar.clone();
            sc.quantity = Some(c.quantity);
            json!({
                "format": f.name(),
                "extension": extension(f),
                "text": t,
                "sidecar": sc.to_text(),
            })
        }
        Err(e) => curve_error(&e),
    }
}

fn extension(f: Format) -> &'static str {
    match f {
        Format::Frd => "frd",
        Format::Zma => "zma",
        Format::Csv => "csv",
        Format::Rew | Format::Auto => "txt",
    }
}

/// The sidecar compatibility check of two curves (module documentation).
pub fn compare_curves_value(a_json: &str, b_json: &str, allow_json: &str) -> Value {
    let read = |t: &str, what: &str| -> Result<Curve, Value> {
        let v = parse_json(t, what)?;
        Curve::from_json(&v).map_err(|e| curve_error(&e))
    };
    let (a, b) = match (read(a_json, "curve a"), read(b_json, "curve b")) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let allow: Vec<String> = if allow_json.trim().is_empty() {
        Vec::new()
    } else {
        match parse_json(allow_json, "allow").map(|v| {
            v.as_array()
                .and_then(|a| a.iter().map(|x| x.as_str().map(String::from)).collect())
        }) {
            Ok(Some(list)) => list,
            Ok(None) => {
                return json!({"error": "allow must be an array of field names", "kind": "curve"})
            }
            Err(e) => return e,
        }
    };
    let (mut sa, mut sb) = (a.sidecar.clone(), b.sidecar.clone());
    sa.quantity = Some(a.quantity);
    sb.quantity = Some(b.quantity);
    let c = sidecar::compare(&sa, &sb);
    let allow_ref: Vec<&str> = allow.iter().map(String::as_str).collect();
    let diff = |d: &sidecar::Difference| json!({"field": d.field, "a": d.a, "b": d.b});
    let check = c.check(&allow_ref);
    json!({
        "ok": check.is_ok(),
        "blocking": c.blocking.iter().map(diff).collect::<Vec<_>>(),
        "notes": c.notes.iter().map(diff).collect::<Vec<_>>(),
        "message": check.err().map(|e| e.msg),
    })
}

/// Fits a netlist to curves (module documentation).
pub fn fit_value(netlist_json: &str, spec_json: &str) -> Value {
    let p = match Parametric::parse(netlist_json) {
        Ok(p) => p,
        Err(e) => return api::error_value(&e),
    };
    let v = match parse_json(spec_json, "fit specification") {
        Ok(v) => v,
        Err(e) => return e,
    };
    match FitSpec::from_json(&v).and_then(|s| fit::fit(&p, &s)) {
        Ok(r) => r.to_json(),
        Err(e) => fit_error(&e),
    }
}

/// A simulated probe as a curve document (module documentation).
pub fn probe_curve_value(netlist_json: &str, overrides_json: &str, probe: &str) -> Value {
    let overrides = match api::parse_overrides(overrides_json) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let c = match acoustilab::Circuit::from_json_with(netlist_json, &overrides) {
        Ok(c) => c,
        Err(e) => return api::error_value(&e),
    };
    let r = match c.solve() {
        Ok(r) => r,
        Err(e) => return api::error_value(&e),
    };
    match acoustilab::io::curve::from_solve(&c, &r, probe) {
        Ok(cv) => cv.to_json(),
        Err(e) => curve_error(&e),
    }
}

/// A virtual-rig measurement (module documentation).
pub fn virtual_measure_value(netlist_json: &str, spec_json: &str) -> Value {
    let p = match Parametric::parse(netlist_json) {
        Ok(p) => p,
        Err(e) => return api::error_value(&e),
    };
    let mut v = match parse_json(spec_json, "rig specification") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let format = match v.as_object_mut().and_then(|o| o.remove("format")) {
        None => None,
        Some(Value::String(s)) => match format_of(&s) {
            Ok(f) => Some(f),
            Err(e) => return e,
        },
        Some(_) => return json!({"error": "'format' must be a string", "kind": "fit_spec"}),
    };
    let spec = match RigSpec::from_json(&v) {
        Ok(s) => s,
        Err(e) => return fit_error(&e),
    };
    let c = match rig::measure(&p, &spec) {
        Ok(c) => c,
        Err(e) => return fit_error(&e),
    };
    let f = format.unwrap_or(if c.quantity == Quantity::Impedance {
        Format::Zma
    } else {
        Format::Frd
    });
    match text::export(&c, f) {
        Ok(t) => json!({
            "curve": c.to_json(),
            "format": f.name(),
            "extension": extension(f),
            "text": t,
            "sidecar": c.sidecar.to_text(),
        }),
        Err(e) => curve_error(&e),
    }
}
