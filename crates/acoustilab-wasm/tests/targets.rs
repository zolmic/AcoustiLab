//! Tests of the targets exports of the wasm wrapper, run natively.
//!
//! The wrapper must pass the engine's numbers through unchanged (compared
//! with the engine called directly), read every documented input form, and
//! turn bad input into `{"error", "kind"}` objects.

use acoustilab::targets::curve::Curve;
use acoustilab::targets::metrics::{evaluate, Normalisation, Options, Response};
use acoustilab::targets::smoothing::smooth_power;
use acoustilab::targets::target::{self, Shelves};
use acoustilab::Circuit;
use acoustilab_wasm::{api, targets as t};
use serde_json::{json, Value};

fn design() -> String {
    let path = format!(
        "{}/../../examples/design_over_ear.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

/// Compares JSON values, numbers to a relative 1e-12: the inputs went
/// through JSON text, and serde_json's default float parser may differ from
/// the exact value in the last bit.
fn assert_close(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            assert!(
                (x - y).abs() <= 1e-12 * x.abs().max(y.abs()) + 1e-12,
                "{path}: {x} vs {y}"
            );
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{path}: length");
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                assert_close(p, q, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            assert_eq!(
                x.keys().collect::<Vec<_>>(),
                y.keys().collect::<Vec<_>>(),
                "{path}: keys"
            );
            for (k, p) in x {
                assert_close(p, &y[k], &format!("{path}.{k}"));
            }
        }
        _ => assert_eq!(a, b, "{path}"),
    }
}

fn kind(v: &Value) -> &str {
    v["kind"]
        .as_str()
        .unwrap_or_else(|| panic!("not an error: {v}"))
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

#[test]
fn list_names_targets_fixtures_and_models() {
    let v = t::targets_list_value();
    let targets = v["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 33);
    assert_eq!(targets[0]["name"], "ravizza2023_5128");
    assert_eq!(targets[0]["licence"], "CC-BY-4.0");
    assert_eq!(targets[0]["fixture"], "bk5128");
    assert!(targets[0]["fixture_label"]
        .as_str()
        .unwrap()
        .contains("5128"));
    assert!(v["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["id"] == "gras45ca_harman"));
    let models: Vec<&str> = v["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        models,
        ["harman_oe_2018", "harman_ie_patent", "harman_ie_listen"]
    );
    assert_eq!(v["smoothing_fractions"], json!([1, 2, 3, 6, 12, 24, 48]));
}

#[test]
fn target_by_name_or_object() {
    let a = t::target_value("ravizza2023_5128");
    assert_eq!(a, target::find("ravizza2023_5128").unwrap().to_json());
    assert_eq!(t::target_value("\"ravizza2023_5128\""), a);
    assert_eq!(t::target_value(&a.to_string()), a);
    assert_eq!(kind(&t::target_value("nope")), "target");
    assert_eq!(kind(&t::target_value("{\"name\": 1")), "input");
}

#[test]
fn metrics_of_a_solve_result_equal_the_engine() {
    let net = design();
    let result = api::solve_value(&net);
    let fx = t::probe_fixture_value(&net, "", "p_drp");
    assert_eq!(fx["fixture"], "iec60318_4");
    let opts = json!({"probe": "p_drp", "fixture": fx["fixture"]}).to_string();
    let got = t::target_metrics_value(&result.to_string(), "ravizza2023_5128", &opts);
    assert!(got.get("error").is_none(), "{got}");

    let c = Circuit::from_json(&net).unwrap();
    let r = c.solve().unwrap();
    let resp = Response {
        left: Curve::new(r.freqs_hz.clone(), r.probe("p_drp").unwrap().spl_db()).unwrap(),
        right: None,
        fixture: Some("iec60318_4".into()),
        label: "p_drp".into(),
        drive: Some(r.meta.drive.label.clone()),
    };
    let direct = evaluate(
        &resp,
        target::find("ravizza2023_5128").unwrap(),
        &Options::default(),
    )
    .unwrap()
    .to_json();
    assert_close(&got, &direct, "report");
    assert_eq!(got["response"]["label"], "p_drp");
    assert!(got["scores"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["greyed"] == true));
}

#[test]
fn metrics_inputs_and_options() {
    let f: Vec<f64> = (0..500)
        .map(|k| 10.0 * 2f64.powf(k as f64 / 48.0))
        .collect();
    let l: Vec<f64> = f.iter().map(|x| 90.0 + (x / 300.0).ln().sin()).collect();
    let r: Vec<f64> = l.iter().map(|x| x + 0.3).collect();
    let curve = json!({"frequencies_Hz": f, "dB": l});
    let spl = json!({"frequencies_Hz": f, "spl_dB": l});
    let one = t::target_metrics_value(&curve.to_string(), "ravizza2023_5128", "");
    assert_eq!(
        t::target_metrics_value(&spl.to_string(), "ravizza2023_5128", ""),
        one
    );
    assert!(one["tracking"].is_null());
    let two = json!({"left": curve, "right": {"frequencies_Hz": f, "dB": r}});
    let opts = json!({
        "smoothing": "1/6",
        "tracking_smoothing": 3,
        "normalisation": {"band_mean_Hz": [200, 500]},
        "personalisation": {"bass_dB": 2},
        "band": {"above_2kHz_dB": 4, "above_8kHz_dB": 0},
        "measured": true,
        "fixture": "bk5128",
    });
    let v = t::target_metrics_value(&two.to_string(), "ravizza2023_5128", &opts.to_string());
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["options"]["smoothing"], "1/6 octave");
    assert_eq!(
        v["options"]["normalisation"],
        json!({"band_mean_Hz": [200.0, 500.0]})
    );
    assert_eq!(v["fixture_match"], "same");
    assert_eq!(v["response"]["measured"], true);
    let tr = &v["tracking"]["bands"][0];
    assert!((tr["max_abs_dB"].as_f64().unwrap() - 0.3).abs() < 1e-9);
    assert_eq!(tr["pass"], true);
    // The measured pair on the 5128 is not the Harman training pair.
    assert!(v["scores"][0]["flags"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["code"] != "simulated"));
    // A custom grid.
    let g = json!({"grid_Hz": [20, 50, 100, 500, 1000, 5000, 10000]}).to_string();
    let v = t::target_metrics_value(&curve.to_string(), "ravizza2023_5128", &g);
    assert_eq!(v["grid_Hz"].as_array().unwrap().len(), 7);
    assert_eq!(v["options"]["grid"], "custom");
}

#[test]
fn metrics_errors_name_the_problem() {
    let net = design();
    let result = api::solve_value(&net).to_string();
    let e = t::target_metrics_value(&result, "ravizza2023_5128", "");
    assert_eq!(kind(&e), "input");
    assert!(e["error"].as_str().unwrap().contains("p_drp"), "{e}");
    let e = t::target_metrics_value(&result, "ravizza2023_5128", "{\"probe\": \"x\"}");
    assert_eq!(kind(&e), "input");
    let e = t::target_metrics_value(&result, "ravizza2023_5128", "{\"probe\": \"zin\"}");
    assert!(
        e["error"]
            .as_str()
            .unwrap()
            .contains("not an acoustic pressure"),
        "{e}"
    );
    let bad = [
        "{\"colour\": 1}",
        "{\"smoothing\": 5}",
        "{\"smoothing\": \"1/5\"}",
        "{\"normalisation\": {\"at_Hz\": -1}}",
        "{\"normalisation\": {\"band_mean_Hz\": [500, 200]}}",
        "{\"grid_Hz\": [100, 50]}",
        "{\"personalisation\": {\"mid_dB\": 1}}",
        "{\"band\": {\"above_1kHz_dB\": 1}}",
        "[]",
    ];
    for o in bad {
        let e = t::target_metrics_value(&result, "ravizza2023_5128", o);
        assert_eq!(kind(&e), "options", "{o}: {e}");
    }
    let curve = json!({"frequencies_Hz": [1000, 2000], "dB": [0, 0]});
    let e = t::target_metrics_value(&json!({"left": curve}).to_string(), "ravizza2023_5128", "");
    assert_eq!(kind(&e), "input");
    let e = t::target_metrics_value(&curve.to_string(), "ravizza2023_5128", "");
    assert_eq!(kind(&e), "options", "normalisation outside the data: {e}");
    let e = t::target_metrics_value(&curve.to_string(), "missing", "");
    assert_eq!(kind(&e), "target");
}

#[test]
fn smoothing_export() {
    let f: Vec<f64> = (0..200)
        .map(|k| 20.0 * 2f64.powf(k as f64 / 20.0))
        .collect();
    let db: Vec<f64> = f.iter().map(|x| (x / 90.0).ln().cos() * 3.0).collect();
    let c = json!({"frequencies_Hz": f, "dB": db}).to_string();
    let v = t::smooth_value(&c, "1/3");
    assert_eq!(t::smooth_value(&c, "3"), v);
    let direct = smooth_power(&Curve::new(f.clone(), db.clone()).unwrap(), 3).unwrap();
    assert_close(&v["dB"], &json!(direct.db()), "smoothed");
    assert_eq!(v["smoothing"], "1/3 octave");
    assert_close(&t::smooth_value(&c, "none")["dB"], &json!(db), "none");
    assert_eq!(kind(&t::smooth_value(&c, "1/5")), "options");
    assert_eq!(kind(&t::smooth_value(&c, "third")), "options");
    let z = json!({"frequencies_Hz": f, "re": db, "im": db}).to_string();
    let v = t::smooth_value(&z, "12");
    assert_eq!(v["method"], "complex");
    assert_eq!(v["re"], v["im"]);
    assert_eq!(kind(&t::smooth_value("{}", "3")), "input");
}

#[test]
fn csv_import_export() {
    let v = t::import_target_csv_value("20,1\n100,2\n20000,0\n", "bk5128", "mine");
    assert_eq!(v["name"], "mine");
    assert_eq!(v["fixture"], "bk5128");
    assert_eq!(v["provenance"]["class"], "user");
    // The imported object is a valid target for the other exports.
    let m = t::target_metrics_value(
        &json!({"frequencies_Hz": [20, 20000], "dB": [0, 0]}).to_string(),
        &v.to_string(),
        "",
    );
    assert!(m.get("error").is_none(), "{m}");
    let e = t::import_target_csv_value("20,1\n100,2\n", "", "");
    assert_eq!(kind(&e), "csv");
    let e = t::import_target_csv_value("# fixture: a\n20,1\n10,2\n", "", "");
    assert_eq!(e["line"], 3);
}

#[test]
fn probe_fixture_follows_the_ear_parameter() {
    let net = design();
    let v = t::probe_fixture_value(&net, "{\"ear\": \"type43\"}", "p_drp");
    assert_eq!(v["fixture"], "p57_type4_3");
    let v = t::probe_fixture_value(&net, "", "p_front");
    assert!(v["fixture"].is_null());
    assert_eq!(kind(&t::probe_fixture_value(&net, "", "nope")), "probe");
    assert_eq!(
        kind(&t::probe_fixture_value(&net, "{\"ear\": \"x\"}", "p_drp")),
        "parameter"
    );
}

#[test]
fn reconstruction_export() {
    let base = t::import_target_csv_value(
        "# fixture: gras45ca_harman\n20,0\n1000,0\n20000,0\n",
        "",
        "flat baseline",
    );
    let v =
        t::reconstruct_target_value(&base.to_string(), "{\"preset\": \"olive_welti_2015_mean\"}");
    assert_eq!(v["family"], "harman_style_reconstruction");
    assert!(v["flags"]
        .as_array()
        .unwrap()
        .contains(&json!("approximation")));
    // A flat baseline gives the shelves themselves.
    let s = Shelves::gains(6.44, -1.41);
    for (f, db) in f64s(&v["frequencies_Hz"]).iter().zip(f64s(&v["dB"])) {
        assert!((db - s.db_at(*f)).abs() < 1e-12);
    }
    let v = t::reconstruct_target_value(&base.to_string(), "{\"bass_dB\": 3, \"name\": \"x\"}");
    assert_eq!(v["name"], "x");
    let e = t::reconstruct_target_value(&base.to_string(), "{\"preset\": \"p\", \"bass_dB\": 1}");
    assert_eq!(kind(&e), "options");
    let e = t::reconstruct_target_value(&base.to_string(), "{\"preset\": \"nope\"}");
    assert_eq!(kind(&e), "options");
}

#[test]
fn target_curve_export() {
    let v = t::target_curve_value(
        "ravizza2023_5128",
        "{\"personalisation\": {\"bass_dB\": 4}}",
    );
    let g = f64s(&v["grid_Hz"]);
    assert_eq!(g.len(), 121);
    let s = Shelves::gains(4.0, 0.0);
    let (r, p) = (&v["reference_dB"], &v["target_dB"]);
    // Both normalised at 500 Hz (read between grid points): the difference
    // is the shelf minus its value there.
    let sv: Vec<Option<f64>> = g.iter().map(|&f| Some(s.db_at(f))).collect();
    let off = Normalisation::At(500.0).offset(&g, &sv).unwrap();
    for k in [10, 40, 60, 100, 118] {
        if let (Some(a), Some(b)) = (r[k].as_f64(), p[k].as_f64()) {
            assert!((b - a - (s.db_at(g[k]) - off)).abs() < 1e-9, "{}", g[k]);
        }
    }
    let (lo, up) = (&v["band_lower_dB"], &v["band_upper_dB"]);
    for k in 0..g.len() {
        if let (Some(l), Some(x), Some(u)) = (lo[k].as_f64(), r[k].as_f64(), up[k].as_f64()) {
            assert!(l <= x + 1e-12 && x <= u + 1e-12);
        }
    }
    assert_eq!(v["band"]["classes"].as_array().unwrap().len(), 3);
}
