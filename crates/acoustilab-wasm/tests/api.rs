//! Tests of the JSON API behind the wasm exports, run natively.
//!
//! The wrapper must pass the engine's numbers through untouched, label each
//! probe with the right domain and SI unit, and turn every engine error
//! into an object that names what failed. Unit labels are checked against
//! closed forms: a label is only right if the number under it has that
//! dimension.

use acoustilab::Circuit;
use acoustilab_wasm::api;
use serde_json::{json, Value};
use std::f64::consts::PI;

fn example(name: &str) -> String {
    let path = format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn probe<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("probe {id} missing"))
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

/// Every example netlist in the repository solves through the wrapper, and
/// the wrapper changes no number of the engine's result: it only adds
/// `domain` and fills empty `unit` fields.
#[test]
fn examples_pass_through_unchanged() {
    let dir = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let engine = Circuit::from_json(&text)
            .and_then(|c| c.solve())
            .map(|r| r.to_json());
        // Compared as values: serde_json's default float *parser* is only
        // best-effort (last-ulp differences without its float_roundtrip
        // feature), while its writer is shortest-round-trip exact, and the
        // browser parses with the correctly rounded JSON.parse.
        let wrapped = api::solve_value(&text);
        let s = api::to_string(&wrapped);
        assert_eq!(s, serde_json::to_string(&wrapped).unwrap());
        let Ok(engine) = engine else {
            // An example that fails in the engine must fail the same way.
            assert!(wrapped.get("error").is_some(), "{path:?}");
            continue;
        };
        n += 1;
        assert!(wrapped.get("error").is_none(), "{path:?}: {wrapped}");
        for key in ["meta", "frequencies_Hz", "validity", "shading"] {
            assert_eq!(wrapped[key], engine[key], "{path:?}: {key}");
        }
        let (we, ee) = (
            wrapped["probes"].as_array().unwrap(),
            engine["probes"].as_array().unwrap(),
        );
        assert_eq!(we.len(), ee.len());
        for (w, e) in we.iter().zip(ee) {
            let (mut w, e) = (w.as_object().unwrap().clone(), e.as_object().unwrap());
            assert!(w.remove("domain").is_some(), "{path:?}: domain missing");
            let unit = w.remove("unit").unwrap();
            let engine_unit = &e["unit"];
            if engine_unit != "" {
                assert_eq!(&unit, engine_unit, "{path:?}: node-probe unit changed");
            }
            let mut e = e.clone();
            e.remove("unit");
            assert_eq!(w, e, "{path:?}: probe values changed");
        }
    }
    assert!(n >= 1, "no example solved");
}

#[test]
fn sealed_cup_probe_labels() {
    let doc = api::solve_value(&example("sealed_cup.json"));
    let p = probe(&doc, "p_front");
    assert_eq!(
        (p["domain"].as_str(), p["unit"].as_str()),
        (Some("acoustic"), Some("Pa"))
    );
    assert!(p.get("spl_dB").is_some());
    let z = probe(&doc, "zin");
    assert_eq!(
        (z["domain"].as_str(), z["unit"].as_str()),
        (Some("electrical"), Some("ohm"))
    );
    let x = probe(&doc, "x");
    assert_eq!(
        (x["domain"].as_str(), x["unit"].as_str()),
        (Some("mechanical"), Some("m"))
    );
    let u = probe(&doc, "u_leak");
    assert_eq!(
        (u["domain"].as_str(), u["unit"].as_str()),
        (Some("acoustic"), Some("m^3/s"))
    );
}

/// Closed forms for the three `impedance` units. Each network is a source
/// driving one ideal element, so port potential / port flow at the source is
/// that element's own ratio:
/// * 8 Ω resistor: V/I = 8 Ω, phase 0.
/// * 1 cm³ lossless cavity with the spec_reference air (ρc² = 1.204·343² Pa):
///   p/U = 1/(jωC), C = V/(ρc²), in Pa·s/m³.
/// * 2 g mass: v/F = 1/(jωM), a mobility in m/(N·s).
///
/// The tolerance (1e-9 relative) only covers rounding; any wrong factor
/// (2π, √2, a unit prefix) fails it by orders of magnitude.
#[test]
fn impedance_units_match_closed_forms() {
    let freqs = [20.0, 1000.0, 10_000.0];
    let net = json!({
        "schema": "acoustilab-netlist/0.1",
        "air": {"preset": "spec_reference"},
        "sweep": {"frequencies_Hz": freqs},
        "level": 0,
        "nodes": [
            {"id": "e1", "domain": "electrical"},
            {"id": "a1", "domain": "acoustic"},
            {"id": "m1", "domain": "mechanical"}
        ],
        "elements": [
            {"id": "src_e", "type": "vsource", "node": "e1", "V_V": 1},
            {"id": "r", "type": "resistor", "node": "e1", "R_ohm": 8},
            {"id": "src_a", "type": "flow_source", "node": "a1", "U_m3_per_s": 1e-6},
            {"id": "cav", "type": "cavity", "node": "a1", "volume_cm3": 1, "wall_loss": false},
            {"id": "src_m", "type": "force_source", "node": "m1", "F_N": 1},
            {"id": "mass", "type": "mass", "node": "m1", "M_kg": 0.002}
        ],
        "probes": [
            {"id": "z_e", "quantity": "impedance", "element": "src_e"},
            {"id": "z_a", "quantity": "impedance", "element": "src_a"},
            {"id": "z_m", "quantity": "impedance", "element": "src_m"},
            {"id": "p", "quantity": "pressure", "node": "a1"}
        ]
    });
    let doc = api::solve_value(&net.to_string());
    assert!(doc.get("error").is_none(), "{doc}");

    let rho_c2 = 1.204 * 343.0 * 343.0;
    let c_ac = 1e-6 / rho_c2;
    let m = 0.002;

    let z_e = probe(&doc, "z_e");
    assert_eq!(
        (z_e["domain"].as_str(), z_e["unit"].as_str()),
        (Some("electrical"), Some("ohm"))
    );
    let z_a = probe(&doc, "z_a");
    assert_eq!(
        (z_a["domain"].as_str(), z_a["unit"].as_str()),
        (Some("acoustic"), Some("Pa*s/m^3"))
    );
    let z_m = probe(&doc, "z_m");
    assert_eq!(
        (z_m["domain"].as_str(), z_m["unit"].as_str()),
        (Some("mechanical"), Some("m/(N*s)"))
    );

    let rel = |a: f64, b: f64| ((a - b) / b).abs();
    for (k, &f) in freqs.iter().enumerate() {
        let w = 2.0 * PI * f;
        assert!(rel(f64s(&z_e["magnitude"])[k], 8.0) < 1e-9);
        assert!(f64s(&z_e["phase_deg"])[k].abs() < 1e-9);
        assert!(rel(f64s(&z_a["magnitude"])[k], 1.0 / (w * c_ac)) < 1e-9);
        assert!((f64s(&z_a["phase_deg"])[k] + 90.0).abs() < 1e-7);
        assert!(rel(f64s(&z_m["magnitude"])[k], 1.0 / (w * m)) < 1e-9);
        assert!((f64s(&z_m["phase_deg"])[k] + 90.0).abs() < 1e-7);
        // RMS phasors: dB SPL is 20·log10(|p|/20 µPa) with no √2.
        let p = 1e-6 / (w * c_ac);
        let spl = f64s(&probe(&doc, "p")["spl_dB"])[k];
        assert!((spl - 20.0 * (p / 20e-6).log10()).abs() < 1e-9);
    }
}

/// Flow and port-potential units on mixed-domain couplers: the motor's
/// port 0 is electrical and port 1 mechanical; the piston's port 0 is
/// mechanical and port 1 acoustic (couplers.rs).
#[test]
fn coupler_port_domains() {
    let mut net: Value = serde_json::from_str(&example("sealed_cup.json")).unwrap();
    net["probes"] = json!([
        {"id": "motor_i", "quantity": "current", "element": "motor", "port": 0},
        {"id": "motor_v", "quantity": "port_potential", "element": "motor", "port": 1},
        {"id": "motor_f", "quantity": "force", "element": "motor", "port": 1},
        {"id": "dia_f", "quantity": "force", "element": "dia", "port": 0},
        {"id": "dia_dp", "quantity": "port_potential", "element": "dia", "port": 1},
        {"id": "dia_u", "quantity": "volume_velocity", "element": "dia", "port": 1},
        {"id": "vent_z", "quantity": "impedance", "element": "rear_vent", "port": 1}
    ]);
    let doc = api::solve_value(&net.to_string());
    assert!(doc.get("error").is_none(), "{doc}");
    for (id, domain, unit) in [
        ("motor_i", "electrical", "A"),
        ("motor_v", "mechanical", "m/s"),
        ("motor_f", "mechanical", "N"),
        ("dia_f", "mechanical", "N"),
        ("dia_dp", "acoustic", "Pa"),
        ("dia_u", "acoustic", "m^3/s"),
        ("vent_z", "acoustic", "Pa*s/m^3"),
    ] {
        let p = probe(&doc, id);
        assert_eq!(p["domain"].as_str(), Some(domain), "{id}");
        assert_eq!(p["unit"].as_str(), Some(unit), "{id}");
    }
}

#[test]
fn errors_name_what_failed() {
    let base: Value = serde_json::from_str(&example("sealed_cup.json")).unwrap();

    // Unknown parameter on an element.
    let mut net = base.clone();
    net["elements"][5]["volume"] = json!(30);
    let e = api::solve_value(&net.to_string());
    assert_eq!(e["kind"], "element");
    assert_eq!(e["element"], "front");
    assert!(e["error"].as_str().unwrap().contains("front"));
    // check() reports the same error without solving.
    assert_eq!(api::check_value(&net.to_string()), e);

    // Unknown element type.
    let mut net = base.clone();
    net["elements"][1]["type"] = json!("coill");
    let e = api::solve_value(&net.to_string());
    assert_eq!(e["kind"], "unknown_type");
    assert_eq!(e["element"], "coil");
    assert_eq!(e["type"], "coill");

    // Terminal in the wrong domain is reported against the element.
    let mut net = base.clone();
    net["elements"][4]["nodes"][2] = json!("m_dia");
    let e = api::solve_value(&net.to_string());
    assert_eq!(e["kind"], "element");
    assert_eq!(e["element"], "dia");

    // Probe on a missing node.
    let mut net = base.clone();
    net["probes"][0]["node"] = json!("a_nowhere");
    let e = api::solve_value(&net.to_string());
    assert_eq!(e["kind"], "probe");
    assert_eq!(e["probe"], "p_front");

    // JSON syntax error: position is reported.
    let e = api::solve_value("{\n  \"nodes\": [\n    {\"id\": \"a\",, }\n]}");
    assert_eq!(e["kind"], "json");
    assert_eq!(e["line"], 3);
    assert!(e["column"].as_u64().unwrap() > 0);

    // Structural netlist error without an element.
    let e = api::solve_value("[]");
    assert_eq!(e["kind"], "netlist");
}

/// A probe on a port the element does not have is rejected when the netlist
/// is built. `check` and `solve` must report it with the same error object,
/// or the live check could call a netlist valid that every run rejects (and
/// the UI would clear the run's error box).
#[test]
fn check_rejects_probe_on_missing_port() {
    let mut net: Value = serde_json::from_str(&example("sealed_cup.json")).unwrap();
    net["probes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "z_bad", "quantity": "impedance", "element": "coil", "port": 3}));
    let text = net.to_string();
    // The engine rejects it at build time.
    assert!(Circuit::from_json(&text).is_err());
    let solved = api::solve_value(&text);
    assert_eq!(solved["kind"], "probe", "{solved}");
    assert_eq!(solved["probe"], "z_bad");
    assert_eq!(api::check_value(&text), solved);

    // Existing ports of every kind still pass the check: coupler ports 0
    // and 1, a two-port's port 1, and node probes.
    let mut ok = net.clone();
    ok["probes"] = json!([
        {"id": "a", "quantity": "current", "element": "motor", "port": 0},
        {"id": "b", "quantity": "force", "element": "motor", "port": 1},
        {"id": "c", "quantity": "volume_velocity", "element": "dia", "port": 1},
        {"id": "d", "quantity": "impedance", "element": "rear_vent", "port": 1},
        {"id": "e", "quantity": "pressure", "node": "a_front"}
    ]);
    assert_eq!(api::check_value(&ok.to_string())["ok"], true);
    let mut bad = ok.clone();
    bad["probes"][2]["port"] = json!(2);
    assert_eq!(api::check_value(&bad.to_string())["probe"], "c");
}

/// A floating node (an electrical node fed by a current source and nothing
/// else) makes the system singular; the error names the node.
#[test]
fn singular_network_names_the_unknown() {
    let net = json!({
        "sweep": {"frequencies_Hz": [100]},
        "nodes": [{"id": "e1", "domain": "electrical"}, {"id": "e2", "domain": "electrical"}],
        "elements": [
            {"id": "i1", "type": "isource", "nodes": ["e1", "e2"], "I_A": 1}
        ],
        "probes": [{"id": "v", "quantity": "voltage", "node": "e1"}]
    });
    let e = api::solve_value(&net.to_string());
    assert_eq!(e["kind"], "singular", "{e}");
    assert_eq!(e["f_Hz"], 100.0);
    assert!(e.get("node").is_some() || e.get("element").is_some(), "{e}");
    // The netlist itself is well-formed, so check() accepts it.
    assert_eq!(api::check_value(&net.to_string())["ok"], true);
}

#[test]
fn check_reports_counts() {
    let text = example("sealed_cup.json");
    let c = Circuit::from_json(&text).unwrap();
    let v = api::check_value(&text);
    assert_eq!(v["ok"], true);
    assert_eq!(v["nodes"], c.nodes.len());
    assert_eq!(v["elements"], c.elements.len());
    assert_eq!(v["unknowns"], c.dim);
    assert_eq!(v["probes"], c.probes.len());
    assert_eq!(v["frequencies"], c.freqs.len());
    assert_eq!(v["f_min_Hz"], 10.0);
    assert_eq!(v["level"], 1);
    assert_eq!(v["shading"], json!(c.shading()));
}

#[test]
fn element_types_and_version() {
    let v = api::element_types_value();
    let names: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(names, acoustilab::elements::known_types());
    assert!(names.contains(&"cavity") && names.contains(&"vsource"));
    assert!(acoustilab_wasm::engine_version().starts_with("acoustilab "));
    let parsed: Value = serde_json::from_str(&acoustilab_wasm::element_types()).unwrap();
    assert_eq!(parsed, v);
}

#[test]
fn exports_return_json_text() {
    let text = example("sealed_cup.json");
    let solved: Value = serde_json::from_str(&acoustilab_wasm::solve(&text)).unwrap();
    assert_eq!(solved["probes"].as_array().unwrap().len(), 4);
    let checked: Value = serde_json::from_str(&acoustilab_wasm::check(&text)).unwrap();
    assert_eq!(checked["ok"], true);
    let bad: Value = serde_json::from_str(&acoustilab_wasm::solve("{")).unwrap();
    assert_eq!(bad["kind"], "json");
}

#[test]
fn panic_message_is_taken_once() {
    acoustilab_wasm::record_panic("engine panic: test");
    assert_eq!(acoustilab_wasm::take_last_panic(), "engine panic: test");
    assert_eq!(acoustilab_wasm::take_last_panic(), "");
}

#[test]
fn parameters_are_described_and_overridable() {
    let text = example("design_over_ear.json");
    let d = api::parameters_value(&text, "");
    let params = d["parameters"].as_array().unwrap();
    let find = |n: &str| params.iter().find(|p| p["name"] == n).unwrap().clone();
    let vc = find("vent_count");
    assert_eq!(vc["kind"], "integer");
    assert_eq!(vc["value"], json!(1));
    assert_eq!(vc["group"], "Rear");
    assert_eq!(find("front_volume_cm3")["kind"], "derived");
    assert_eq!(find("ear")["choices"][1]["value"], "type43");
    assert_eq!(d["ui"]["primary_probe"], "p_drp");
    // Derived values follow overrides.
    let d = api::parameters_value(&text, r#"{"front_radius_mm": 20}"#);
    let fv = d["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "front_volume_cm3")
        .unwrap()["value"]
        .as_f64()
        .unwrap();
    assert!((fv - PI * 400.0 * 20.0 / 1000.0).abs() < 1e-9);

    let base = api::solve_with_value(&text, "");
    let open = api::solve_with_value(&text, r#"{"rear": "open", "vent_count": 0}"#);
    assert!(base["error"].is_null() && open["error"].is_null());
    assert_eq!(open["meta"]["parameters"]["rear"], "open");
    assert_ne!(
        probe(&base, "p_drp")["spl_dB"],
        probe(&open, "p_drp")["spl_dB"]
    );
    assert!(base["warnings"].as_array().is_some());
    assert_eq!(base["meta"]["drive"]["convention"], "power");

    // Bad overrides are parameter errors that name the parameter.
    for (ov, name) in [
        (r#"{"vent_count": 2.5}"#, "vent_count"),
        (r#"{"front_volume_cm3": 3}"#, "front_volume_cm3"),
        (r#"{"ear": "hats"}"#, "ear"),
        (r#"{"nope": 1}"#, "nope"),
        (r#"{"rear": [1]}"#, "rear"),
    ] {
        let e = api::solve_with_value(&text, ov);
        assert_eq!(e["kind"], "parameter", "{ov}: {e}");
        assert_eq!(e["parameter"], name, "{ov}: {e}");
    }
    assert_eq!(api::solve_with_value(&text, "[1]")["kind"], "parameter");
    let parsed: Value = serde_json::from_str(&acoustilab_wasm::parameters(&text, "")).unwrap();
    assert_eq!(parsed, api::parameters_value(&text, ""));
}
