//! Native tests of the `impulse`, `vector_fit` and `isolation` exports:
//! the wrapper passes the engine's reports through, applies its options,
//! enforces its runtime bounds and reports every failure as an object.

use acoustilab::isolation::{insertion_loss, IsolationOptions};
use acoustilab::params::{Overrides, Parametric};
use acoustilab::time::report::{impulses, ImpulseRequest};
use acoustilab::Circuit;
use acoustilab_wasm::time::{impulse_value, isolation_value, vector_fit_value};
use serde_json::Value;

fn design() -> String {
    let path = format!(
        "{}/../../examples/design_over_ear.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

#[test]
fn impulse_export_passes_the_report_through() {
    let text = design();
    let v = impulse_value(&text, "", r#"{"n": 4096, "ir_length_check": false}"#);
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["probe"], "p_drp");
    assert_eq!(v["n"], 4096);
    for key in ["ir", "ir_min_phase", "step", "etc_dB"] {
        assert_eq!(v[key].as_array().unwrap().len(), 4096, "{key}");
    }
    for key in ["spectrum", "excess_phase_deg", "group_delay_s", "trusted"] {
        assert!(v.get(key).is_some(), "{key}");
    }
    assert_eq!(v["frequencies_Hz"].as_array().unwrap().len(), 2049);
    assert_eq!(v["decision"]["mode"], "minimum");
    assert!(v["ir_length"].is_null());
    assert_eq!(v["parameters"]["ear"], "iec60318_4");
    // The same numbers as the engine's own report.
    let c = Circuit::from_json(&text).unwrap();
    let mut req = ImpulseRequest {
        length_check: false,
        ..Default::default()
    };
    req.uniform.n = 4096;
    let r = impulses(&c, "p_drp", &req).unwrap();
    assert_eq!(f64s(&v["ir"]), r.mixed.h);
    assert_eq!(f64s(&v["ir_min_phase"]), r.minimum.h);
}

#[test]
fn impulse_export_options() {
    let text = design();
    let v = impulse_value(
        &text,
        r#"{"ear": "type43"}"#,
        r#"{"probe": "p_front", "fs_Hz": 96000, "n": 2048, "spectrum": false, "order": 20}"#,
    );
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["probe"], "p_front");
    assert_eq!(v["fs_Hz"], 96000.0);
    assert!(v.get("spectrum").is_none());
    assert!(v["ir_length"]["recommended_n"].as_u64().unwrap() >= 4096);
    assert_eq!(v["parameters"]["ear"], "type43");
    for (opts, kind) in [
        (r#"{"n": 32768}"#, "options"),
        (r#"{"bogus": 1}"#, "options"),
        (r#"{"n": 1000}"#, "netlist"),
        (r#"{"probe": "nope"}"#, "probe"),
        (r#"[1]"#, "options"),
        (r#"{"order": 0}"#, "options"),
    ] {
        let e = impulse_value(&text, "", opts);
        assert_eq!(e["kind"], kind, "{opts}: {e}");
    }
    let e = impulse_value(&text, r#"{"nope": 1}"#, "");
    assert_eq!(e["kind"], "parameter");
}

#[test]
fn vector_fit_export() {
    let text = design();
    let v = vector_fit_value(
        &text,
        "",
        r#"{"order": 24, "attribute": true, "max_parameters": 4, "state_space": true}"#,
    );
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["order"], 24);
    assert!(v["fit"]["max_dB"].as_f64().unwrap() < 0.01);
    let poles = v["poles"].as_array().unwrap();
    assert!(poles.iter().any(|p| p["resonant"] == true));
    let a = &v["attribution"];
    assert_eq!(a["perturbed"].as_array().unwrap().len(), 4);
    let n = v["state_space"]["B"].as_array().unwrap().len();
    assert_eq!(n, 24);
    assert_eq!(
        v["group_delay_s"].as_array().unwrap().len(),
        v["frequencies_Hz"].as_array().unwrap().len()
    );
    for opts in [
        r#"{"order": 81}"#,
        r#"{"asymptote": "cubic"}"#,
        r#"{"weighting": "log"}"#,
        r#"{"max_parameters": 65}"#,
        r#"{"step": 0.9}"#,
    ] {
        assert_eq!(
            vector_fit_value(&text, "", opts)["kind"],
            "options",
            "{opts}"
        );
    }
}

#[test]
fn isolation_export() {
    let text = design();
    let v = isolation_value(&text, "", "");
    assert!(v.get("error").is_none(), "{v}");
    let il = f64s(&v["insertion_loss_dB"]);
    let p = Parametric::parse(&text).unwrap();
    let iso = insertion_loss(&p, &Overrides::new(), &IsolationOptions::default()).unwrap();
    assert_eq!(il, iso.insertion_loss_db);
    assert_eq!(v["ear"]["entrance_node"], "a_ear");
    assert_eq!(v["paths"].as_array().unwrap().len(), 2);
    assert!(v["third_octave_bands"].as_array().unwrap().len() > 20);
    assert_eq!(v["bleed"]["distances_m"], serde_json::json!([0.3, 1.0]));
    assert!(v["fixture_self_insertion_loss"]["url"]
        .as_str()
        .unwrap()
        .starts_with("https://"));
    let v = isolation_value(&text, "", r#"{"bleed": false, "paths": false}"#);
    assert!(v["bleed"].is_null());
    assert!(v["paths"].as_array().unwrap().is_empty());
    let e = isolation_value(&text, "", r#"{"entrance_node": "a_front"}"#);
    assert_eq!(e["kind"], "netlist", "{e}");
    let e = isolation_value(&text, "", r#"{"bleed_distances_m": [0]}"#);
    assert_eq!(e["kind"], "options");
}
