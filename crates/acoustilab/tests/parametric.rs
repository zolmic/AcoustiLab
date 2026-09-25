//! Parametric netlists, the `drive` key, warnings and low-side validity
//! (docs/parameters.md, docs/netlist.md).

use acoustilab::expr::PValue;
use acoustilab::params::{Overrides, Parametric};
use acoustilab::{Circuit, Error, SolveResult};
use serde_json::{json, Value};

const TEMPLATE: &str = include_str!("../../../examples/design_over_ear.json");

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

fn solve(doc: &Value) -> SolveResult {
    Circuit::from_json(&doc.to_string())
        .unwrap()
        .solve()
        .unwrap()
}

fn ov(pairs: &[(&str, PValue)]) -> Overrides {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// A driver in a sealed cup with a slit leak and a rear vent, written with
/// parameters, and the same netlist written out literally.
fn cup_pair() -> (Value, Value) {
    let param = json!({
        "parameters": {
            "v_mm": {"value": 2.5, "min": 0.5, "max": 5},
            "n": {"value": 2, "integer": true, "min": 0, "max": 4},
            "gap_mm": 0.1,
            "front_cm3": {"expr": "pi * 25^2 * 15 / 1000"}
        },
        "sweep": {"f_min_Hz": 20, "f_max_Hz": 20000, "points_per_octave": 6},
        "nodes": [{"id": "e", "domain": "electrical"}, {"id": "af", "domain": "acoustic"},
                  {"id": "ar", "domain": "acoustic"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e"},
            {"id": "drv", "type": "driver", "nodes": ["e", "gnd", "af", "ar"],
             "record": "tymphany_hpd_40n16pet00_32"},
            {"id": "front", "type": "cavity", "node": "af", "volume_cm3": "=front_cm3"},
            {"id": "leak", "type": "slit", "nodes": ["af", "ambient"], "gap_mm": "=gap_mm",
             "width_mm": 20, "length_mm": 8},
            {"id": "rear", "type": "cavity", "node": "ar", "volume_cm3": 20},
            {"id": "vent", "type": "vent", "nodes": ["ar", "ambient"], "diameter_mm": "=v_mm",
             "length_mm": 2, "count": "=n", "enabled": "=n > 0"}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "af"},
                   {"id": "u", "quantity": "volume_velocity", "element": "vent",
                    "enabled": "=n > 0"}]
    });
    let mut literal = param.clone();
    literal.as_object_mut().unwrap().remove("parameters");
    literal["elements"][2]["volume_cm3"] = json!(std::f64::consts::PI * 625.0 * 15.0 / 1000.0);
    literal["elements"][3]["gap_mm"] = json!(0.1);
    literal["elements"][5] = json!({"id": "vent", "type": "vent", "nodes": ["ar", "ambient"],
        "diameter_mm": 2.5, "length_mm": 2, "count": 2});
    literal["probes"][1] = json!({"id": "u", "quantity": "volume_velocity", "element": "vent"});
    (param, literal)
}

#[test]
fn parametric_netlist_equals_its_literal_expansion() {
    let (param, literal) = cup_pair();
    let a = solve(&param);
    let b = solve(&literal);
    for (pa, pb) in a.probes.iter().zip(&b.probes) {
        assert_eq!(pa.values, pb.values, "probe {}", pa.id);
    }
    let params = &a.to_json()["meta"]["parameters"];
    assert_eq!(params["n"], json!(2));
    assert!((params["front_cm3"].as_f64().unwrap() - 29.452_431_127_404_31).abs() < 1e-12);
}

#[test]
fn overrides_change_the_network_and_enabled_removes_elements() {
    let (param, literal) = cup_pair();
    let text = param.to_string();
    // n = 0 removes the vent and its probe: the same as deleting them.
    let c = Circuit::from_json_with(&text, &ov(&[("n", PValue::Num(0.0))])).unwrap();
    assert!(c.element_index("vent").is_none());
    assert_eq!(c.probes.len(), 1);
    let mut sealed = literal.clone();
    sealed["elements"].as_array_mut().unwrap().remove(5);
    sealed["probes"].as_array_mut().unwrap().remove(1);
    assert_eq!(
        c.solve().unwrap().probes[0].values,
        solve(&sealed).probes[0].values
    );
    // A larger vent moves the response; the literal agrees again.
    let c = Circuit::from_json_with(&text, &ov(&[("v_mm", PValue::Num(4.0))])).unwrap();
    let mut big = literal.clone();
    big["elements"][5]["diameter_mm"] = json!(4.0);
    assert_eq!(
        c.solve().unwrap().probes[0].values,
        solve(&big).probes[0].values
    );
    // Out-of-range and unknown overrides fail before anything is built.
    for (k, v) in [("v_mm", 6.0), ("n", 0.5), ("front_cm3", 1.0), ("nope", 1.0)] {
        let e = Circuit::from_json_with(&text, &ov(&[(k, PValue::Num(v))]))
            .err()
            .expect("rejected");
        assert!(matches!(e, Error::Parameter { .. }), "{k}: {e}");
    }
}

#[test]
fn expression_errors_name_the_element_and_key() {
    let (mut param, _) = cup_pair();
    param["elements"][3]["gap_mm"] = json!("=gap_mm / (n - 2)");
    let e = Circuit::from_json(&param.to_string()).err().unwrap();
    match e {
        Error::Element { id, msg } => {
            assert_eq!(id, "leak");
            assert!(
                msg.contains("gap_mm") && msg.contains("division by zero"),
                "{msg}"
            );
        }
        other => panic!("{other}"),
    }
}

fn spl_at(r: &SolveResult, probe: &str, f: f64) -> f64 {
    let k = r
        .freqs_hz
        .iter()
        .position(|x| (x / f - 1.0).abs() < 1e-9)
        .expect("frequency on the grid");
    db(r.probe(probe).unwrap().values[k].norm() / 20e-6)
}

fn with_drive(drive: Value) -> Value {
    let (mut d, _) = cup_pair();
    d["sweep"] = json!({"frequencies_Hz": [50, 500, 1000, 5000]});
    d["elements"][0]["V_V"] = json!(0.37); // the solve scale must not matter
    d["drive"] = drive;
    d["probes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "i", "quantity": "current", "element": "amp"}));
    d
}

#[test]
fn drive_key_scales_to_the_stated_convention() {
    let one = solve(&with_drive(json!({"voltage_V": 1})));
    let two = solve(&with_drive(json!({"voltage_mV": 2000})));
    for f in [50.0, 500.0, 5000.0] {
        let d = spl_at(&two, "p", f) - spl_at(&one, "p", f);
        assert!((d - db(2.0)).abs() < 1e-9, "{d}");
    }
    let meta = &one.to_json()["meta"]["drive"];
    assert_eq!(meta["convention"], "voltage");
    assert_eq!(meta["source_voltage_V"], json!(1.0));

    // 1 mW into 32 ohm is sqrt(0.032) V.
    let mw = solve(&with_drive(json!({"power_mW": 1, "rated_ohm": 32})));
    let d = spl_at(&mw, "p", 500.0) - spl_at(&one, "p", 500.0);
    assert!((d - db(0.032f64.sqrt())).abs() < 1e-9);

    // Characteristic: exactly 94 dB at 500 Hz, and 90 dB at 1 kHz on request.
    let ch = solve(&with_drive(json!({"characteristic": "p"})));
    assert!((spl_at(&ch, "p", 500.0) - 94.0).abs() < 1e-9);
    let ch = solve(&with_drive(
        json!({"characteristic": "p", "level_dB": 90, "f_kHz": 1}),
    ));
    assert!((spl_at(&ch, "p", 1000.0) - 90.0).abs() < 1e-9);
    let v = ch.to_json()["meta"]["drive"]["source_voltage_V"]
        .as_f64()
        .unwrap();
    assert!(v > 0.0 && v < 1.0);

    // Constant current: the source current is 10 mA at every frequency and
    // p/I is the same as under any voltage drive.
    let cc = solve(&with_drive(json!({"current_mA": 10})));
    for (k, i) in cc.probe("i").unwrap().values.iter().enumerate() {
        assert!((i.re - 0.01).abs() < 1e-15 && i.im.abs() < 1e-15, "{i}");
        let ratio_cc = cc.probe("p").unwrap().values[k] / i;
        let ratio_v = one.probe("p").unwrap().values[k] / one.probe("i").unwrap().values[k];
        assert!((ratio_cc / ratio_v - 1.0).norm() < 1e-12);
    }
    assert_eq!(
        cc.to_json()["meta"]["drive"]["source_voltage_V"],
        Value::Null
    );
}

#[test]
fn drive_key_is_validated() {
    let bad = [
        json!({"voltage_V": 1, "power_mW": 1}),
        json!({"power_mW": 1}),
        json!({"rated_ohm": 32, "voltage_V": 1}),
        json!({"characteristic": "i"}),
        json!({"characteristic": "nope"}),
        json!({"voltage": 1}),
        json!({"voltage_V": -1}),
        json!({"level_dB": 94, "voltage_V": 1}),
        json!("1 V"),
    ];
    for d in bad {
        assert!(
            Circuit::from_json(&with_drive(d.clone()).to_string()).is_err(),
            "{d}"
        );
    }
    // Two sources cannot be scaled.
    let mut two = with_drive(json!({"voltage_V": 1}));
    two["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "q", "type": "flow_source", "node": "af"}));
    assert!(Circuit::from_json(&two.to_string()).is_err());
}

#[test]
fn warnings_report_operating_limits_at_the_stated_drive() {
    let d = with_drive(json!({"voltage_V": 1}));
    let low = solve(&with_drive(json!({"voltage_mV": 1})));
    let high = solve(&d);
    let codes = |r: &SolveResult| {
        r.warnings
            .iter()
            .map(|w| w.code.clone())
            .collect::<Vec<_>>()
    };
    // 1 V into this 32 ohm driver dissipates about 30 mW (rated 10 mW);
    // 1 mV is harmless.
    assert!(
        codes(&high).contains(&"coil_power".to_string()),
        "{:?}",
        codes(&high)
    );
    assert!(!codes(&low).contains(&"coil_power".to_string()));
    let w = high
        .warnings
        .iter()
        .find(|w| w.code == "coil_power")
        .unwrap();
    assert_eq!(w.element.as_deref(), Some("drv"));
    assert!(w.value.unwrap() > 25.0 && w.value.unwrap() < 35.0);
    assert_eq!(w.limit, Some(10.0));
    assert!(w.f_min_hz.unwrap() <= w.f_max_hz.unwrap());
    // The record's electrical-Q inconsistency (erratum E5) is a note.
    assert!(codes(&low).contains(&"record_consistency".to_string()));

    // Particle velocity: a narrow vent at 10 V exceeds 1 m/s; at 1 mV not.
    let mut loud = with_drive(json!({"voltage_V": 10}));
    loud["parameters"]["v_mm"]["value"] = json!(0.6);
    loud["elements"][1]["Xmax_mm"] = json!(0.05);
    let r = solve(&loud);
    let pv = r
        .warnings
        .iter()
        .find(|w| w.code == "particle_velocity" && w.element.as_deref() == Some("vent"))
        .expect("vent velocity warning");
    assert!(pv.message.contains("vent hole"), "{}", pv.message);
    assert!(r.warnings.iter().any(|w| w.code == "excursion"));
    let json = r.to_json();
    assert!(json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["code"] == "excursion"));
}

#[test]
fn template_solves_in_every_variant() {
    let p = Parametric::parse(TEMPLATE).unwrap();
    assert!(p.defs.len() > 20);
    let variants: Vec<Overrides> = vec![
        ov(&[]),
        ov(&[("rear", PValue::Str("open".into()))]),
        ov(&[("vent_count", PValue::Num(0.0))]),
        ov(&[
            ("vent_mesh_rayl", PValue::Num(0.0)),
            ("vent_count", PValue::Num(3.0)),
        ]),
        ov(&[("leak_gap_mm", PValue::Num(0.0))]),
        ov(&[("ear", PValue::Str("type43".into()))]),
        ov(&[("fidelity", PValue::Num(0.0))]),
    ];
    for o in variants {
        let c = Circuit::from_parametric(&p, &o).unwrap();
        let r = c.solve().unwrap();
        let p_drp = r.probe("p_drp").unwrap();
        assert!(
            p_drp
                .values
                .iter()
                .all(|v| v.norm().is_finite() && v.norm() > 0.0),
            "{o:?}"
        );
        assert_eq!(r.to_json()["meta"]["drive"]["convention"], "power");
    }
    // Parameters that do not reach the netlist are reported as unused:
    // with an open back the vent sizes and rear volume do nothing.
    let r = Circuit::from_parametric(&p, &ov(&[("rear", PValue::Str("open".into()))]))
        .unwrap()
        .solve()
        .unwrap();
    let json = r.to_json();
    let used: Vec<&str> = json["meta"]["parameters_used"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(used.contains(&"grille_rayl") && used.contains(&"rear"));
    for unused in [
        "vent_count",
        "vent_diameter_mm",
        "rear_volume_cm3",
        "vent_mesh_rayl",
    ] {
        assert!(!used.contains(&unused), "{unused}");
    }
    let types: Vec<(&str, &str)> = json["meta"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["id"].as_str().unwrap(), e["type"].as_str().unwrap()))
        .collect();
    assert!(types.contains(&("ear", "iec60318_4")) && types.contains(&("grille", "mesh")));
    assert!(!types.iter().any(|(id, _)| *id == "vent"));

    // The 60318-4 load is shaded below 100 Hz (coupler-extrapolated); Type
    // 4.3 is specified down to 20 Hz.
    let r = Circuit::from_parametric(&p, &ov(&[]))
        .unwrap()
        .solve()
        .unwrap();
    assert_eq!(r.shading.low_begin_hz, Some(100.0));
    assert!(r.warnings.iter().any(|w| w.code == "unverified_model"));
    let r = Circuit::from_parametric(&p, &ov(&[("ear", PValue::Str("type43".into()))]))
        .unwrap()
        .solve()
        .unwrap();
    assert_eq!(r.shading.low_begin_hz, None);
}

#[test]
fn porous_laws_shade_below_their_fitted_window() {
    let doc = json!({
        "sweep": {"frequencies_Hz": [100, 1000]},
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [
            {"id": "q", "type": "flow_source", "node": "a"},
            {"id": "foam", "type": "porous_layer", "nodes": ["a"], "backing": "rigid",
             "thickness_mm": 10, "area_cm2": 10, "sigma_kPa_s_per_m2": 50, "model": "miki"}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "a"}]
    });
    let r = solve(&doc);
    // f/σ = 0.01 at 500 Hz; Miki's Im K turns negative at f/σ ≈ 0.00105.
    assert!((r.shading.low_begin_hz.unwrap() - 500.0).abs() < 1e-9);
    assert!((r.shading.low_deep_hz.unwrap() - 250.0).abs() < 1e-9);
    assert_eq!(r.shading.band(100.0), 2);
    assert_eq!(r.shading.band(300.0), 1);
    assert_eq!(r.shading.band(1000.0), 0);
}
