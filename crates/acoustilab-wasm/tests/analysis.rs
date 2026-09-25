//! Tests of the design-analysis JSON API behind the wasm exports, run
//! natively: each export returns what the engine's Rust API returns,
//! malformed options fail with kind `options`, engine errors keep their
//! kinds, and a Monte Carlo run split into chunks equals the run in one go.

use acoustilab::analysis::mc::{self, PlanSpec, RunOptions};
use acoustilab::analysis::{readouts, Design};
use acoustilab::params::Overrides;
use acoustilab_wasm::analysis as wa;
use serde_json::{json, Value};

fn template() -> String {
    let path = format!(
        "{}/../../examples/design_over_ear.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap()
}

fn kind(v: &Value) -> &str {
    v["kind"].as_str().unwrap_or("")
}

#[test]
fn sensitivity_export_shape_and_subsets() {
    let t = template();
    let v = wa::sensitivity_value(
        &t,
        r#"{"rear": "open"}"#,
        r#"{"parameters": ["driver_Mms_g", "vent_count", "grille_rayl"], "probes": ["p_drp", "zin"]}"#,
    );
    assert!(v.get("error").is_none(), "{v}");
    let nf = v["frequencies_Hz"].as_array().unwrap().len();
    assert_eq!(v["probes"], json!(["p_drp", "zin"]));
    let params = v["parameters"].as_array().unwrap();
    assert_eq!(params.len(), 2);
    assert_eq!(params[0]["name"], "driver_Mms_g");
    assert_eq!(params[0]["scheme"], "central");
    assert_eq!(params[0]["dB_per_pct"].as_array().unwrap().len(), 2);
    assert_eq!(params[0]["dB_per_pct"][0].as_array().unwrap().len(), nf);
    assert_eq!(params[0]["deg_per_pct"][1].as_array().unwrap().len(), nf);
    // The grille only exists with the open back (the override applied).
    assert_eq!(params[1]["name"], "grille_rayl");
    assert!(params[1]["dB_per_pct"][0]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x.as_f64().unwrap().abs() > 1e-6));
    assert_eq!(v["excluded"][0]["name"], "vent_count");
    assert!(v["method"]
        .as_str()
        .unwrap()
        .contains("central differences"));
    assert!(v["hash"].as_str().unwrap().len() == 64);
    // The forward-sensitivity method through the same export.
    let f = wa::sensitivity_value(
        &t,
        r#"{"rear": "open"}"#,
        r#"{"parameters": ["driver_Mms_g"], "probes": ["p_drp"], "method": "forward_sensitivity"}"#,
    );
    assert!(f["method"]
        .as_str()
        .unwrap()
        .starts_with("forward sensitivities"));
    let a = params[0]["dB_per_pct"][0].as_array().unwrap();
    let b = f["parameters"][0]["dB_per_pct"][0].as_array().unwrap();
    let scale = a
        .iter()
        .fold(0.0f64, |m, x| m.max(x.as_f64().unwrap().abs()));
    for (x, y) in a.iter().zip(b) {
        assert!((x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 1e-5 * scale);
    }
}

#[test]
fn options_and_engine_errors_are_reported_by_kind() {
    let t = template();
    for (v, k) in [
        (wa::sensitivity_value(&t, "", r#"{"step": 0.2}"#), "options"),
        (
            wa::sensitivity_value(&t, "", r#"{"method": "adjoint"}"#),
            "options",
        ),
        (
            wa::sensitivity_value(&t, "", r#"{"steps": 0.2}"#),
            "options",
        ),
        (wa::sensitivity_value(&t, "", "{"), "options"),
        (
            wa::sensitivity_value(&t, "", r#"{"parameters": ["nope"]}"#),
            "parameter",
        ),
        (
            wa::sensitivity_value(&t, "", r#"{"probes": ["nope"]}"#),
            "probe",
        ),
        (wa::sensitivity_value("{", "", ""), "json"),
        (
            wa::sensitivity_value(&t, r#"{"vent_count": 99}"#, ""),
            "parameter",
        ),
        (
            wa::tornado_value(&t, "", r#"{"metric": {"kind": "readout", "name": "x"}}"#),
            "options",
        ),
        (
            wa::tornado_value(&t, "", r#"{"metric": {"kind": "level", "probe": "p_drp"}}"#),
            "options",
        ),
        (wa::explain_value(&t, "", r#"{"step_pct": 0}"#), "options"),
        (wa::explain_value(&t, "", r#"{"probe": "zin"}"#), "probe"),
        (
            wa::readouts_value(&t, "", r#"{"rated_ohm": -1}"#),
            "options",
        ),
        (
            wa::readouts_value(&t, "", r#"{"driver": "amp"}"#),
            "element",
        ),
        (
            wa::mc_plan_value(&t, "", r#"{"method": "lhs", "n": 0}"#),
            "options",
        ),
        (
            wa::mc_plan_value(&t, "", r#"{"method": "sobol", "n": 3}"#),
            "options",
        ),
        (
            wa::mc_plan_value(
                &t,
                "",
                r#"{"method": "runs", "runs": [{"leak_gap_mm": 9}]}"#,
            ),
            "parameter",
        ),
        // Bounded work: huge plans and repeated names are refused before any
        // allocation or solve (a trillion-level factor used to abort).
        (
            wa::mc_plan_value(&t, "", r#"{"method": "lhs", "n": 100001}"#),
            "options",
        ),
        (
            wa::mc_plan_value(
                &t,
                "",
                r#"{"method": "factorial", "factors": {"leak_gap_mm": {"levels": 1000000000000, "from": 0.02, "to": 0.2}}}"#,
            ),
            "parameter",
        ),
        (
            wa::sensitivity_value(
                &t,
                "",
                r#"{"parameters": ["driver_Mms_g", "driver_Mms_g"]}"#,
            ),
            "parameter",
        ),
        (
            wa::explain_value(
                &t,
                "",
                r#"{"parameters": ["driver_Mms_g", "driver_Mms_g"]}"#,
            ),
            "parameter",
        ),
        (wa::mc_run_value(&t, "", "[{}]", ""), "options"),
        (wa::mc_envelope_value("[]"), "options"),
    ] {
        assert_eq!(kind(&v), k, "{v}");
        assert!(v["error"].as_str().is_some_and(|s| !s.is_empty()));
    }
}

#[test]
fn exports_equal_the_rust_api() {
    let t = template();
    let d = Design::parse(&t, &Overrides::new()).unwrap();
    let r = readouts::readouts(&d, &Default::default()).unwrap();
    assert_eq!(
        wa::readouts_value(&t, "", ""),
        serde_json::to_value(&r).unwrap()
    );
    let tor = wa::tornado_value(
        &t,
        "",
        r#"{"metric": {"kind": "band_mean", "probe": "p_drp", "f_min_Hz": 100, "f_max_Hz": 1000}}"#,
    );
    assert_eq!(tor["unit"], "dB SPL");
    let rows = tor["rows"].as_array().unwrap();
    assert!(rows
        .windows(2)
        .all(|w| w[0]["effect"].as_f64() >= w[1]["effect"].as_f64()));
    let ex = wa::explain_value(&t, "", r#"{"top": 2}"#);
    let s = ex["sentences"].as_array().unwrap();
    assert_eq!(s.len(), 2);
    assert!(s[0]["text"].as_str().unwrap().starts_with("Raising "));
    assert!(s[0]["bands"][0]["f_min_Hz"].as_f64().is_some());
    assert_eq!(ex["statistic"], "mean");
}

#[test]
fn monte_carlo_in_chunks_equals_one_run() {
    let t = template();
    let plan = wa::mc_plan_value(&t, "", r#"{"method": "lhs", "n": 5, "seed": 7}"#);
    let samples = plan["samples"].as_array().unwrap().clone();
    assert_eq!(samples.len(), 5);
    let opts = r#"{"probes": ["p_drp", "zin"]}"#;
    // Chunks by plan and range: the samples are re-planned in Rust, so no
    // sampled value passes through a JSON parser.
    let spec = json!({"method": "lhs", "n": 5, "seed": 7});
    let mut all: Vec<Value> = Vec::new();
    let mut freqs = Value::Null;
    for first in [0, 2, 4] {
        let which = json!({"plan": spec, "first": first, "count": 2});
        let c = wa::mc_run_value(&t, "", &which.to_string(), opts);
        assert!(c.get("error").is_none(), "{c}");
        freqs = c["frequencies_Hz"].clone();
        all.extend(c["samples"].as_array().unwrap().iter().cloned());
    }
    // The same runs through the Rust API in one go.
    let d = Design::parse(&t, &Overrides::new()).unwrap();
    let p = mc::plan(
        &d,
        &PlanSpec::Lhs {
            n: 5,
            seed: 7,
            parameters: None,
        },
    )
    .unwrap();
    let one = mc::run(
        &d,
        &p.samples,
        &RunOptions {
            probes: Some(vec!["p_drp".into(), "zin".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    let one_json = serde_json::to_value(&one).unwrap();
    assert_eq!(Value::Array(all.clone()), one_json["samples"]);
    // Samples passed as JSON may move by one ulp in serde_json's parser;
    // the results then agree to rounding and the hashes exactly.
    let c = wa::mc_run_value(&t, "", &Value::Array(samples.clone()).to_string(), opts);
    for (a, b) in c["samples"].as_array().unwrap().iter().zip(&all) {
        assert_eq!(a["hash"], b["hash"]);
        let (x, y) = (&a["curves"][0]["dB"], &b["curves"][0]["dB"]);
        for (u, v) in x.as_array().unwrap().iter().zip(y.as_array().unwrap()) {
            assert!((u.as_f64().unwrap() - v.as_f64().unwrap()).abs() < 1e-12);
        }
    }
    let bad = wa::mc_run_value(
        &t,
        "",
        r#"{"plan": {"method": "lhs", "n": 2}, "last": 1}"#,
        "",
    );
    assert_eq!(kind(&bad), "options");
    // Out-of-range and huge ranges are clamped, never a panic.
    for which in [
        json!({"plan": spec, "first": 1, "count": 4294967295u64}),
        json!({"plan": spec, "first": u64::MAX, "count": u64::MAX}),
        json!({"plan": spec, "first": 9, "count": 1}),
    ] {
        let c = wa::mc_run_value(&t, "", &which.to_string(), r#"{"metrics": false}"#);
        assert!(c["samples"].as_array().unwrap().len() <= 4, "{which}");
    }
    let chunk = json!({"engine": one.engine, "frequencies_Hz": freqs, "samples": all});
    let env = wa::mc_envelope_value(&chunk.to_string());
    assert_eq!(env["runs"], 5);
    let median = env["probes"][0]["dB"]["median"].as_array().unwrap();
    assert_eq!(median.len(), one.freqs_hz.len());
    let zphase = &env["probes"][1]["phase_deg"]["p95"];
    assert!(zphase.as_array().is_some());
    assert!(env["metrics"]["coupled_resonance_Hz"]["n"] == 5);
    let csv = wa::mc_csv_value(&chunk.to_string(), &plan["parameters"].to_string());
    let text = csv["csv"].as_str().unwrap();
    assert_eq!(text.lines().count(), 6);
    // The plan's parameters are the first columns (read from the plan, so
    // that editing the template does not break the test).
    let names: Vec<&str> = plan["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(text.starts_with(&format!("run,hash,engine,{},", names.join(","))));
    // Overrides are the sample's own; the base overrides apply underneath.
    let first = &plan["samples"][0]["overrides"];
    assert!(first[names[0]].as_f64().is_some());
    let open = wa::mc_run_value(
        &t,
        r#"{"rear": "open"}"#,
        &json!([plan["samples"][0]]).to_string(),
        r#"{"metrics": false}"#,
    );
    let r0 = &open["samples"][0];
    assert_eq!(r0["ok"], true);
    assert!(r0["metrics"].as_object().unwrap().is_empty());
    assert_ne!(r0["hash"], all_hash(&chunk, 0));
}

fn all_hash(chunk: &Value, i: usize) -> Value {
    chunk["samples"][i]["hash"].clone()
}
