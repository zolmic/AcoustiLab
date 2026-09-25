//! JSON API used by the web UI's measurement views (docs/web.md, "Fit").
//!
//! * [`curve_uncertainty_value`]`(curve_json)`: the combined standard
//!   uncertainty of a curve at each of its frequencies, from its sidecar's
//!   budget (`io::sidecar::Uncertainty::level_db` and `phase_deg`, averaged
//!   over the sidecar's seatings; docs/fitting.md, "Uncertainty budget"):
//!   `{"frequencies_Hz", "seatings", "level_dB", "phase_deg"}`, where
//!   `level_dB` (dB, one standard deviation) and `phase_deg` (degrees) are
//!   null when the budget has no such term. Errors are `{"error", "kind":
//!   "curve"}` or `{"error", "kind": "json", "line", "column"}`.

use acoustilab::io::{Curve, CurveError};
use serde_json::{json, Value};

fn curve_error(e: &CurveError) -> Value {
    let mut v = json!({"error": e.to_string(), "kind": "curve"});
    if let Some(l) = e.line {
        v["line"] = json!(l);
    }
    v
}

/// Combined level and phase uncertainty of a curve (module documentation).
pub fn curve_uncertainty_value(curve_json: &str) -> Value {
    let v: Value = match serde_json::from_str(curve_json) {
        Ok(v) => v,
        Err(e) => {
            return json!({"error": format!("curve JSON: {e}"), "kind": "json",
                          "line": e.line(), "column": e.column()})
        }
    };
    let c = match Curve::from_json(&v) {
        Ok(c) => c,
        Err(e) => return curve_error(&e),
    };
    let n = c.sidecar.seatings_or_one();
    let (level, phase) = match &c.sidecar.uncertainty {
        None => (Value::Null, Value::Null),
        Some(u) => {
            let level: Option<Vec<f64>> = c.freqs_hz.iter().map(|&f| u.level_db(f, n)).collect();
            let phase: Option<Vec<f64>> = c.freqs_hz.iter().map(|&f| u.phase_deg(f, n)).collect();
            (json!(level), json!(phase))
        }
    };
    json!({
        "frequencies_Hz": c.freqs_hz,
        "seatings": n,
        "level_dB": level,
        "phase_deg": phase,
    })
}
