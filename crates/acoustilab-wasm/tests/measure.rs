//! Native tests of the `curve_uncertainty` export against the combined
//! uncertainty of docs/fitting.md: u = sqrt(coupler² + calibration² +
//! fixture² + numerical² + (repositioning² + noise²)/N), the phase term
//! phase_deg/√N, tables interpolated linearly in ln f and held beyond their
//! ends.

use acoustilab_wasm::measure::curve_uncertainty_value;
use serde_json::{json, Value};

fn curve(uncertainty: Value, seatings: Option<u32>) -> String {
    let mut sidecar = json!({"schema": "acoustilab-curve-sidecar/0.1", "quantity": "pressure"});
    if !uncertainty.is_null() {
        sidecar["uncertainty"] = uncertainty;
    }
    if let Some(n) = seatings {
        sidecar["seatings"] = json!(n);
    }
    json!({
        "schema": "acoustilab-curve/0.1",
        "quantity": "pressure",
        "frequencies_Hz": [50.0, 100.0, (100.0f64 * 1000.0).sqrt(), 1000.0, 10000.0],
        "level_dB": [90.0, 91.0, 92.0, 93.0, 94.0],
        "sidecar": sidecar,
    })
    .to_string()
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

#[test]
fn combined_level_and_phase_uncertainty() {
    let budget = json!({
        "coupler_dB": {"frequencies_Hz": [100, 1000], "values_dB": [0.1, 0.3]},
        "microphone_calibration_dB": 0.2,
        "repositioning_dB": 0.4,
        "noise_dB": 0.1,
        "phase_deg": 2.0,
    });
    let v = curve_uncertainty_value(&curve(budget, Some(4)));
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["seatings"], 4);
    let f = f64s(&v["frequencies_Hz"]);
    let u = f64s(&v["level_dB"]);
    // Coupler: 0.1 held below 100 Hz, 0.2 at the geometric mean 316.2 Hz,
    // 0.3 held above 1 kHz.
    let coupler = [0.1, 0.1, 0.2, 0.3, 0.3];
    for k in 0..f.len() {
        let expect =
            (coupler[k] * coupler[k] + 0.2f64.powi(2) + (0.4f64.powi(2) + 0.1f64.powi(2)) / 4.0)
                .sqrt();
        assert!(
            (u[k] - expect).abs() < 1e-12,
            "{} Hz: {} vs {expect}",
            f[k],
            u[k]
        );
    }
    for p in f64s(&v["phase_deg"]) {
        assert!((p - 1.0).abs() < 1e-12, "{p}");
    }
}

#[test]
fn one_seating_and_absent_terms() {
    let v = curve_uncertainty_value(&curve(json!({"noise_dB": 0.3}), None));
    assert_eq!(v["seatings"], 1);
    assert!(f64s(&v["level_dB"]).iter().all(|u| (u - 0.3).abs() < 1e-15));
    assert!(v["phase_deg"].is_null());
    // No budget: neither term.
    let v = curve_uncertainty_value(&curve(Value::Null, Some(3)));
    assert!(v["level_dB"].is_null() && v["phase_deg"].is_null(), "{v}");
    // A phase term alone: no level uncertainty.
    let v = curve_uncertainty_value(&curve(json!({"phase_deg": 3.0}), Some(9)));
    assert!(v["level_dB"].is_null());
    assert!(f64s(&v["phase_deg"])
        .iter()
        .all(|p| (p - 1.0).abs() < 1e-12));
}

#[test]
fn errors_are_objects() {
    let v = curve_uncertainty_value("{");
    assert_eq!(v["kind"], "json");
    let v =
        curve_uncertainty_value(r#"{"schema": "acoustilab-curve/0.1", "quantity": "pressure"}"#);
    assert_eq!(v["kind"], "curve", "{v}");
    let bad = curve(json!({"noise_dB": -1}), None);
    let v = curve_uncertainty_value(&bad);
    assert_eq!(v["kind"], "curve", "{v}");
}
