//! Native tests of the `audition_filter` export: the wrapper returns the
//! engine's report, its design cache changes nothing, and every bad request
//! comes back as an error object naming its kind (and the design at fault).

use acoustilab::audition::report::{parse_options, to_json};
use acoustilab::audition::{audition, Baseline, Design};
use acoustilab::params::Overrides;
use acoustilab_wasm::audition::{audition_filter_value, clear_cache};
use serde_json::{json, Value};

fn template() -> String {
    std::fs::read_to_string(format!(
        "{}/../../examples/design_over_ear.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn call(c: &Value, b: &Value, o: &str) -> Value {
    audition_filter_value(&c.to_string(), &b.to_string(), o)
}

#[test]
fn export_returns_the_engine_report() {
    clear_cache();
    let text = template();
    let cand = json!({"netlist": text, "overrides": {"vent_count": 3}});
    let base = json!({"kind": "netlist", "netlist": text});
    let v = call(&cand, &base, "");
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["schema"], "acoustilab-audition-filter/0.1");
    assert_eq!(v["taps"].as_array().unwrap().len(), 8192);
    assert_eq!(v["phase"]["used"], "minimum");
    assert_eq!(v["check"]["met"], true);
    // The engine's own report, taps included.
    let mut ov = Overrides::new();
    ov.insert(
        "vent_count".into(),
        acoustilab::expr::PValue::from_json(&json!(3)).unwrap(),
    );
    let mut c = Design::new(&text, &ov, json!({"vent_count": 3}), None).unwrap();
    let mut b = Design::new(&text, &Overrides::new(), json!({}), None).unwrap();
    let a = audition(
        &mut c,
        Baseline::Design(&mut b),
        &parse_options(&Value::Null).unwrap().0,
    )
    .unwrap();
    assert_eq!(v, to_json(&a, true));
}

/// A second call reuses both prepared designs; one with a new candidate
/// reuses the baseline. Every result equals a call on an empty cache.
#[test]
fn cache_changes_nothing() {
    let text = template();
    let base = json!({"kind": "netlist", "netlist": text});
    let c1 = json!({"netlist": text, "overrides": {"front_depth_mm": 14}});
    let c2 = json!({"netlist": text, "overrides": {"front_depth_mm": 16}});
    let o = r#"{"taps": true}"#;
    clear_cache();
    let fresh1 = call(&c1, &base, o);
    clear_cache();
    let fresh2 = call(&c2, &base, o);
    clear_cache();
    assert_eq!(call(&c1, &base, o), fresh1);
    assert_eq!(call(&c1, &base, o), fresh1);
    assert_eq!(call(&c2, &base, o), fresh2);
    assert_eq!(call(&c1, &base, o), fresh1);
    // Overrides differing only in key order name the same design.
    let c3 = json!({"netlist": text, "overrides": {"front_depth_mm": 14, "vent_count": 1}});
    let c4 = json!({"netlist": text, "overrides": {"vent_count": 1, "front_depth_mm": 14}});
    let a = call(&c3, &base, o);
    let b = call(&c4, &base, o);
    assert_eq!(a["taps"], b["taps"]);
    // Candidate and baseline the same design: a flat filter.
    let flat = call(&base_as_candidate(&text), &base, r#"{"taps": false}"#);
    assert!(
        flat["max_boost_dB"]["dB"].as_f64().unwrap().abs() < 1e-9,
        "{flat}"
    );
    assert!(flat.get("taps").is_none());
}

fn base_as_candidate(text: &str) -> Value {
    json!({"netlist": text})
}

#[test]
fn targets_curves_and_absolute_mode() {
    clear_cache();
    let text = template();
    let cand = json!({"netlist": text});
    let t = call(
        &cand,
        &json!({"kind": "target", "target": "ravizza2023_5128"}),
        "",
    );
    assert!(t.get("error").is_none(), "{t}");
    assert_eq!(t["baseline"]["kind"], "target");
    assert_eq!(t["inversion"]["boost_cap_dB"], 12.0);
    assert!(t["flags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["code"] == "fixture_mismatch"));
    // A measured curve (the template's own response as a curve document)
    // against the template. Where the curve stays near its 500 Hz–2 kHz
    // level (below 700 Hz: within 3 dB) the inversion undoes it to 1 dB
    // (sixth-octave smoothing and regularisation are what remain). Above
    // 3 kHz the template lies more than 20 dB below that level; the 12 dB
    // cap leaves the rest uninverted, so the filter keeps a deep cut there.
    let r = acoustilab::Circuit::from_json(&text)
        .unwrap()
        .solve()
        .unwrap();
    let p = r.probe("p_drp").unwrap();
    let curve = json!({
        "schema": "acoustilab-curve/0.1",
        "quantity": "pressure",
        "frequencies_Hz": r.freqs_hz,
        "level_dB": p.values.iter().map(|v| 20.0 * (v.norm() / 20e-6).log10()).collect::<Vec<_>>(),
        "sidecar": {"schema": "acoustilab-curve-sidecar/0.1", "fixture": "iec60318_4"},
    });
    let m = call(
        &cand,
        &json!({"kind": "curve", "curve": curve, "label": "self"}),
        "",
    );
    assert!(m.get("error").is_none(), "{m}");
    assert_eq!(m["baseline"]["kind"], "curve");
    assert_eq!(m["baseline"]["label"], "self");
    let f: Vec<f64> = serde_json::from_value(m["frequencies_Hz"].clone()).unwrap();
    let d: Vec<f64> = serde_json::from_value(m["design_dB"].clone()).unwrap();
    let worst = |lo: f64, hi: f64| {
        f.iter()
            .zip(&d)
            .filter(|(f, _)| **f >= lo && **f <= hi)
            .map(|(_, d)| *d)
            .fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)))
    };
    let (lo, hi) = worst(20.0, 700.0);
    assert!(lo > -1.0 && hi < 1.0, "{lo} {hi}");
    let (lo, _) = worst(3000.0, 6000.0);
    assert!(lo < -6.0, "{lo}");
    assert!(m["check"]["met"].as_bool().unwrap());
    let a = call(&cand, &Value::Null, r#"{"mode": "absolute"}"#);
    assert_eq!(a["mode"], "absolute");
    assert_eq!(a["flags"][0]["code"], "absolute_diagnostic");
    let d = call(&cand, &json!({"kind": "none"}), r#"{"mode": "difference"}"#);
    assert!(
        d["error"].as_str().unwrap().contains("needs a baseline"),
        "{d}"
    );
}

#[test]
fn errors_name_their_kind() {
    let text = template();
    let cand = json!({"netlist": text});
    let base = json!({"kind": "netlist", "netlist": text});
    for (c, b, o, kind, needle) in [
        (
            cand.clone(),
            base.clone(),
            r#"{"phase": "x"}"#,
            "options",
            "phase",
        ),
        (
            cand.clone(),
            base.clone(),
            r#"{"extra": 1}"#,
            "options",
            "unknown key",
        ),
        (cand.clone(), base.clone(), "{", "options", "options JSON"),
        (
            cand.clone(),
            json!({"kind": "loudspeaker"}),
            "",
            "options",
            "unknown kind",
        ),
        (
            cand.clone(),
            json!({"kind": "target", "target": "no_such_target"}),
            "",
            "baseline",
            "no bundled target",
        ),
        (
            json!({"overrides": {}}),
            base.clone(),
            "",
            "options",
            "'netlist'",
        ),
        (
            json!({"netlist": text, "overrides": {"vent_count": "three"}}),
            base.clone(),
            "",
            "parameter",
            "vent_count",
        ),
        (
            json!({"netlist": text, "probe": "nope"}),
            base.clone(),
            "",
            "probe",
            "nope",
        ),
    ] {
        let v = audition_filter_value(&c.to_string(), &b.to_string(), o);
        assert_eq!(v["kind"], kind, "{v}");
        assert!(v["error"].as_str().unwrap().contains(needle), "{v}");
    }
    let v = audition_filter_value(&json!({"netlist": "{"}).to_string(), &base.to_string(), "");
    assert_eq!(
        (v["kind"].as_str(), v["design"].as_str()),
        (Some("json"), Some("candidate"))
    );
    let v = audition_filter_value(
        &cand.to_string(),
        &json!({"kind": "netlist", "netlist": "{"}).to_string(),
        "",
    );
    assert_eq!(
        (v["kind"].as_str(), v["design"].as_str()),
        (Some("json"), Some("baseline"))
    );
}
