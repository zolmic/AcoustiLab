//! Tests of the curve and fitting exports (JSON API behind the wasm shims),
//! run natively: files and sidecars through `import_curve`/`export_curve`,
//! a virtual measurement fitted through `fit`, the resumable bounded fit a
//! worker uses, the compatibility check, and the error shapes.

use acoustilab_wasm::fit as api;
use serde_json::{json, Value};

fn bench() -> String {
    let path = format!(
        "{}/../../examples/driver_bench.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap()
}

const TRUTH: [(&str, f64); 5] = [
    ("Re_ohm", 31.5),
    ("Bl_Tm", 2.5),
    ("Mms_g", 0.33),
    ("Cms_mm_per_N", 11.0),
    ("Rms_Ns_per_m", 0.07),
];

fn measure(extra: Value, seed: u64) -> Value {
    let mut ov = json!({});
    for (k, v) in TRUTH {
        ov[k] = json!(v);
    }
    for (k, v) in extra.as_object().unwrap() {
        ov[k] = v.clone();
    }
    let spec = json!({
        "probe": "zin", "overrides": ov,
        "f_min_Hz": 10, "f_max_Hz": 20000, "points_per_octave": 12,
        "noise": {"seed": seed, "level_dB": 0.05, "phase_deg": 0.3},
        "sidecar": {"fixture": "free air", "device": "bench unit 1"}
    });
    let out = api::virtual_measure_value(&bench(), &spec.to_string());
    assert!(out.get("error").is_none(), "{out}");
    out
}

#[test]
fn files_and_sidecars_round_trip_through_the_api() {
    let m = measure(json!({}), 1);
    assert_eq!(m["format"], "zma");
    assert_eq!(m["extension"], "zma");
    let text = m["text"].as_str().unwrap();
    let sidecar: Value = serde_json::from_str(m["sidecar"].as_str().unwrap()).unwrap();
    assert_eq!(sidecar["provenance"]["origin"], "virtual_rig");
    // A real rig's two files, read back with the sidecar.
    let opts = json!({"format": "zma", "sidecar": sidecar}).to_string();
    let c = api::import_curve_value(text, &opts);
    assert!(c.get("error").is_none(), "{c}");
    assert_eq!(c["quantity"], "impedance");
    assert_eq!(c["sidecar"]["fixture"], "free air");
    let n = c["frequencies_Hz"].as_array().unwrap().len();
    assert_eq!(n, m["curve"]["frequencies_Hz"].as_array().unwrap().len());
    // Export every format and read each back.
    for (fmt, ext) in [("csv", "csv"), ("rew", "txt"), ("zma", "zma")] {
        let e = api::export_curve_value(&c.to_string(), fmt);
        assert_eq!(e["extension"], ext, "{e}");
        let back = api::import_curve_value(
            e["text"].as_str().unwrap(),
            &json!({"format": fmt, "sidecar": serde_json::from_str::<Value>(e["sidecar"].as_str().unwrap()).unwrap()}).to_string(),
        );
        assert_eq!(back["quantity"], "impedance", "{fmt}: {back}");
        assert_eq!(back["frequencies_Hz"].as_array().unwrap().len(), n);
    }
    let bad = api::export_curve_value(&c.to_string(), "frd");
    assert_eq!(bad["kind"], "curve");
}

#[test]
fn import_errors_carry_the_line() {
    let e = api::import_curve_value("100 1 0\n200 x\n", "zma");
    assert_eq!(e["kind"], "curve");
    assert_eq!(e["line"], 2);
    assert!(e["error"].as_str().unwrap().starts_with("line 2:"));
    assert_eq!(
        api::import_curve_value("100 1\n200 2\n", "wav")["kind"],
        "curve"
    );
    let e = api::import_curve_value(
        "100 1\n200 2\n",
        r#"{"format": "csv", "quantity": "voltage"}"#,
    );
    assert!(e["error"].as_str().unwrap().contains("unknown quantity"));
    let e = api::import_curve_value("100 1\n200 2\n", r#"{"fromat": "csv"}"#);
    assert!(e["error"]
        .as_str()
        .unwrap()
        .contains("unknown key 'fromat'"));
    let c = api::import_curve_value(
        "100 1\n200 2\n",
        r#"{"format": "csv", "quantity": "impedance"}"#,
    );
    assert_eq!(c["magnitude_ohm"], json!([1.0, 2.0]));
}

#[test]
fn a_virtual_measurement_is_fitted_through_the_api() {
    let mut curves = Vec::new();
    for (mass, seed) in [(0.0, 11), (150.0, 12)] {
        let m = measure(json!({"added_mass_mg": mass}), seed);
        curves.push(
            json!({"probe": "zin", "curve": m["curve"], "overrides": {"added_mass_mg": mass}}),
        );
    }
    let spec = json!({
        "schema": "acoustilab-fit/0.1",
        "parameters": TRUTH.iter().map(|(n, _)| json!(n)).collect::<Vec<_>>(),
        "curves": curves,
    });
    let r = api::fit_value(&bench(), &spec.to_string());
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["schema"], "acoustilab-fit-report/0.1");
    assert_eq!(r["converged"], true);
    for (name, truth) in TRUTH {
        let p = r["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap();
        assert_eq!(p["status"], "determined", "{p}");
        let ci = p["ci95"].as_array().unwrap();
        let (lo, hi) = (ci[0].as_f64().unwrap(), ci[1].as_f64().unwrap());
        // 99 %: the 95 % interval widened by 2.576/1.960.
        let v = p["value"].as_f64().unwrap();
        let k = 2.576 / 1.96;
        assert!(
            v * (lo / v).powf(k) <= truth && truth <= v * (hi / v).powf(k),
            "{name}: {truth} vs {p}"
        );
    }
    let codes: Vec<&str> = r["identifiability"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, vec!["scale_resolved", "added_mass_unreliable"]);
    assert!(r["fitted"]["Bl_Tm"].is_number());
    assert!(r["curves"][0]["residuals"]["level_dB"].is_array());
}

#[test]
fn a_bounded_fit_resumes_from_its_report() {
    // A worker runs a few iterations per call and restarts from the
    // report's fitted values, so it can report progress and cancel.
    let m = measure(json!({}), 21);
    let names = ["Re_ohm", "Bl_Tm"];
    let mut start = json!({"Re_ohm": 20.0, "Bl_Tm": 1.2});
    let mut last = Value::Null;
    for _ in 0..30 {
        let spec = json!({
            "parameters": names.iter().map(|n| json!({"name": n, "start": start[*n]})).collect::<Vec<_>>(),
            "overrides": {"Mms_g": 0.33, "Cms_mm_per_N": 11.0, "Rms_Ns_per_m": 0.07},
            "curves": [{"probe": "zin", "curve": m["curve"]}],
            "max_iterations": 1,
        });
        last = api::fit_value(&bench(), &spec.to_string());
        assert!(last.get("error").is_none(), "{last}");
        if last["converged"] == true {
            break;
        }
        assert_eq!(last["stop"], "max_iterations");
        start = last["fitted"].clone();
    }
    assert_eq!(last["converged"], true, "{last}");
    let bl = last["fitted"]["Bl_Tm"].as_f64().unwrap();
    assert!((bl / 2.5 - 1.0).abs() < 0.01, "{bl}");
}

#[test]
fn refusals_and_errors_have_kinds() {
    let m = measure(json!({"box_volume_cm3": 20.0}), 31);
    let spl = api::virtual_measure_value(
        &bench(),
        &json!({"probe": "p_box", "overrides": {"box_volume_cm3": 20.0},
                "f_min_Hz": 20, "f_max_Hz": 2000, "points_per_octave": 6,
                "format": "frd"})
        .to_string(),
    );
    assert_eq!(spl["format"], "frd");
    let spec = json!({
        "parameters": ["Bl_Tm"],
        "curves": [{"probe": "p_box", "curve": spl["curve"], "overrides": {"box_volume_cm3": 20.0}}],
    });
    let r = api::fit_value(&bench(), &spec.to_string());
    assert_eq!(r["kind"], "fit_refused", "{r}");
    assert!(r["error"].as_str().unwrap().contains("Section 12"));
    let r = api::fit_value(
        &bench(),
        r#"{"parameters": ["Re_ohm"], "curves": [], "extra": 1}"#,
    );
    assert_eq!(r["kind"], "fit_spec");
    let r = api::fit_value(&bench(), "{not json");
    assert_eq!(r["kind"], "json");
    let r = api::fit_value(r#"{"elements": 3}"#, &spec.to_string());
    assert!(r.get("error").is_some());
    let spec = json!({"parameters": ["Re_ohm"], "curves": [{"probe": "zin", "curve": {"quantity": "impedance"}}]});
    assert_eq!(api::fit_value(&bench(), &spec.to_string())["kind"], "curve");
    let r = api::virtual_measure_value(&bench(), r#"{"probe": "zin", "noise": {"sead": 1}}"#);
    assert_eq!(r["kind"], "fit_spec");
    assert!(m.get("error").is_none());
}

#[test]
fn probe_curve_exports_a_simulated_probe() {
    let c = api::probe_curve_value(&bench(), r#"{"box_volume_cm3": 20}"#, "p_box");
    assert!(c.get("error").is_none(), "{c}");
    assert_eq!(c["quantity"], "pressure");
    assert_eq!(c["sidecar"]["provenance"]["origin"], "simulated");
    let e = api::export_curve_value(&c.to_string(), "frd");
    assert!(e["text"].as_str().unwrap().starts_with("* FRD"));
    assert_eq!(
        api::probe_curve_value(&bench(), "", "nope")["kind"],
        "curve"
    );
    assert_eq!(
        api::probe_curve_value(&bench(), r#"{"nope": 1}"#, "zin")["kind"],
        "parameter"
    );
}

#[test]
fn compare_curves_names_the_differences() {
    let a = measure(json!({}), 41)["curve"].clone();
    let mut b = a.clone();
    assert_eq!(
        api::compare_curves_value(&a.to_string(), &b.to_string(), "")["ok"],
        true
    );
    b["sidecar"]["fixture"] = json!("IEC 60318-4 on a flat plate");
    b["sidecar"]["drive"] = json!({"power_mW": 1, "rated_ohm": 32});
    let r = api::compare_curves_value(&a.to_string(), &b.to_string(), "[]");
    assert_eq!(r["ok"], false);
    let fields: Vec<&str> = r["blocking"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, vec!["fixture", "drive"]);
    assert!(r["message"]
        .as_str()
        .unwrap()
        .contains("fixture differs (free air vs IEC 60318-4 on a flat plate)"));
    let r = api::compare_curves_value(&a.to_string(), &b.to_string(), r#"["fixture", "drive"]"#);
    assert_eq!(r["ok"], true);
    assert_eq!(
        api::compare_curves_value(&a.to_string(), &b.to_string(), "{")["kind"],
        "json"
    );
}
