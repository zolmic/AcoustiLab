//! Driver macro (D0–D2, creep), driver records and governance, and
//! Thiele–Small identification (spec Sections 5 and 17; errata E5, E16,
//! E17, E18).
//!
//! Oracles: closed forms (Appendix C2, C5), an independent mpmath solution
//! of the same driver models written as Newton's law per mass
//! (tools/driver/reference.py → tests/data/driver_reference.json), the
//! Tymphany HPD-40N16PET00-32 datasheet numbers, and a numerical
//! Kramers–Kronig transform of the solver's own output.

use acoustilab::elements::driver::{
    self, governance, parse_record, CheckStatus, Driver, DriverLevel,
};
use acoustilab::ts::{self, Creep, FitOptions, ImpedanceModel};
use acoustilab::{AirState, Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::{LN_10, PI};

const TYMPHANY: &str = "tymphany_hpd_40n16pet00_32";

fn reference() -> Value {
    serde_json::from_str(include_str!("data/driver_reference.json")).unwrap()
}

fn circuit(elements: Value, probes: Value) -> Circuit {
    // Declare the acoustic nodes only where elements use them (a floating
    // node would make the system singular), and drop probes on absent nodes.
    let text = elements.to_string();
    let mut nodes = vec![json!({"id": "e1", "domain": "electrical"})];
    let mut present = vec!["e1", "drv.m", "drv.m2", "drv.e"];
    for n in ["af", "ar"] {
        if text.contains(&format!("\"{n}\"")) {
            nodes.push(json!({"id": n, "domain": "acoustic"}));
            present.push(n);
        }
    }
    let probes: Vec<Value> = probes
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["node"].as_str().is_none_or(|n| present.contains(&n)))
        .cloned()
        .collect();
    let doc = json!({
        "schema": "acoustilab-netlist/0.1",
        "air": {"preset": "spec_reference"},
        "sweep": {"f_min_Hz": 10, "f_max_Hz": 20000, "points_per_octave": 48},
        "nodes": nodes,
        "elements": elements,
        "probes": probes,
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

/// Driver element JSON with the given parameters, wired e1/gnd/af/ar.
fn drv(params: Value) -> Value {
    let mut e = json!({"id": "drv", "type": "driver", "nodes": ["e1", "gnd", "af", "ar"]});
    for (k, v) in params.as_object().unwrap() {
        e[k] = v.clone();
    }
    e
}

fn probe(c: &Circuit, id: &str, f: f64) -> C64 {
    let x = c.solve_at(f).unwrap();
    let p = c.probes.iter().find(|p| p.id == id).unwrap();
    c.probe_value(p, f, &x).unwrap()
}

/// Golden-section maximum of `g` on [a, b] (unimodal).
fn golden_max(g: impl Fn(f64) -> f64, mut a: f64, mut b: f64) -> (f64, f64) {
    let r = (5f64.sqrt() - 1.0) / 2.0;
    let mut c = b - r * (b - a);
    let mut d = a + r * (b - a);
    let (mut gc, mut gd) = (g(c), g(d));
    for _ in 0..200 {
        if gc > gd {
            b = d;
            d = c;
            gd = gc;
            c = b - r * (b - a);
            gc = g(c);
        } else {
            a = c;
            c = d;
            gc = gd;
            d = a + r * (b - a);
            gd = g(d);
        }
        if (b - a) < 1e-12 * b {
            break;
        }
    }
    let x = 0.5 * (a + b);
    (x, g(x))
}

fn physical(re: f64, bl: f64) -> Value {
    json!({"model": "D0", "Re_ohm": re, "Bl_Tm": bl, "Mms_g": 0.3,
           "Cms_mm_per_N": 1.0, "Rms_Ns_per_m": 0.05, "Sd_cm2": 10})
}

fn source(zs: f64) -> Value {
    json!({"id": "amp", "type": "vsource", "node": "e1", "V_V": 1.0, "Zs_ohm": zs})
}

fn zin_probe() -> Value {
    json!([
        {"id": "zin", "quantity": "impedance", "element": "drv", "port": 0},
        {"id": "i", "quantity": "current", "element": "amp"},
        {"id": "v", "quantity": "velocity", "node": "drv.m"},
        {"id": "p", "quantity": "pressure", "node": "af"}
    ])
}

// ----- Records and governance ------------------------------------------------

#[test]
fn every_embedded_record_parses_with_provenance_and_builds() {
    for name in driver::record_names() {
        let rec = driver::record(name).unwrap();
        assert_eq!(rec.name, name);
        let p = &rec.provenance;
        for s in [&p.source, &p.date, &p.licence, &p.condition, &p.air_load] {
            assert!(!s.trim().is_empty());
        }
        // Every record must build at D0 and D1 (its model keys are valid).
        for model in ["D0", "D1"] {
            let c = free_air(json!({"record": name, "model": model}), 0.0);
            assert!(probe(&c, "zin", 100.0).re >= rec.primary.re);
        }
    }
    assert!(driver::record("no_such_driver").is_err());
}

#[test]
fn tymphany_record_holds_the_datasheet_values() {
    // Tymphany HPD-40N16PET00-32 datasheet, Rev 1 (2018-07-11), as quoted in
    // spec p. 16.
    let rec = driver::record(TYMPHANY).unwrap();
    let p = rec.primary;
    assert_eq!(
        (p.fs, p.qms, p.qes, p.re),
        (81.8, 2.71, 1.01, 32.8),
        "primary set"
    );
    assert!((p.mms - 0.3e-3).abs() < 1e-15 && (p.sd - 10e-4).abs() < 1e-15);
    let d = &rec.datasheet;
    assert_eq!(d.bl, Some(2.42));
    assert!((d.cms.unwrap() - 11.7e-6).abs() < 1e-18, "printed um/N");
    assert!((d.vas.unwrap() - 1.64e-3).abs() < 1e-15);
    assert_eq!(d.le, Some(0.0));
    assert_eq!(d.qts, Some(0.74));
    assert_eq!(d.rated_impedance, Some(32.0));
    assert!((d.rated_power.unwrap() - 0.01).abs() < 1e-15);
    assert!((d.xmax.unwrap() - 0.8e-3).abs() < 1e-15);
    assert_eq!(d.sensitivity.len(), 2);
    assert!(rec
        .provenance
        .url
        .as_deref()
        .unwrap()
        .starts_with("https://"));
}

#[test]
fn tymphany_governance_flags_qes_and_the_cms_unit() {
    // Erratum E5: Bl, Re, Mms and fs give Qes = 0.864, not 1.01; Cms is only
    // consistent in mm/N; the primary set derives Bl = 2.24 T·m.
    let refs = &reference()["tymphany_identities"];
    let r = |k: &str| refs[k].as_f64().unwrap();
    let rep = governance(&driver::record(TYMPHANY).unwrap());

    let q = rep.check("electrical_q").unwrap();
    assert_eq!(q.status, CheckStatus::Fail);
    assert!((q.value.unwrap() / r("qes_from_bl") - 1.0).abs() < 1e-12);
    assert!((q.value.unwrap() - 0.864).abs() < 5e-4);
    assert_eq!(q.reference, Some(1.01));
    assert!(q.repair.is_none(), "no unit slip explains the Qes mismatch");

    let fsc = rep.check("resonance").unwrap();
    assert_eq!(fsc.status, CheckStatus::Fail, "as printed (um/N)");
    assert!((fsc.value.unwrap() / r("fs_from_printed_cms_Hz") - 1.0).abs() < 1e-12);
    let fix = fsc.repair.as_ref().unwrap();
    assert!(fix.pass);
    assert!((fix.value / r("fs_from_cms_mm_per_N_Hz") - 1.0).abs() < 1e-12);
    assert!((fix.value - 84.95).abs() < 0.01);

    let vas = rep.check("equivalent_volume").unwrap();
    assert_eq!(vas.status, CheckStatus::Fail);
    assert!((vas.value.unwrap() / r("vas_from_printed_cms_m3") - 1.0).abs() < 1e-12);
    assert!(vas.repair.as_ref().unwrap().pass);

    assert_eq!(rep.anomalies.len(), 1, "{:?}", rep.anomalies);
    let a = &rep.anomalies[0];
    assert_eq!(a.field, "Cms");
    assert_eq!(a.printed, "Cms_um_per_N");
    assert_eq!(a.likely, "Cms_mm_per_N");
    assert_eq!(a.factor, 1e3);
    assert_eq!(a.repairs, vec!["resonance", "equivalent_volume"]);
    assert!(!a.primary && rep.primary_anomaly().is_none());

    let qts = rep.check("total_q").unwrap();
    assert_eq!(qts.status, CheckStatus::Pass);
    assert!((qts.value.unwrap() / r("qts_primary") - 1.0).abs() < 1e-12);
    assert!(qts.description.contains("0.6549"), "{}", qts.description);

    assert_eq!(
        rep.check("identifiability").unwrap().status,
        CheckStatus::Pass
    );
    assert_eq!(
        rep.check("rated_impedance").unwrap().status,
        CheckStatus::Pass
    );
    let sens = rep.check("sensitivity_conversion").unwrap();
    assert!((sens.value.unwrap() / r("sensitivity_implied_ohm") - 1.0).abs() < 1e-12);

    let d = rep.derived;
    assert!((d.bl / r("bl_derived_Tm") - 1.0).abs() < 1e-12);
    assert!((d.bl - 2.24).abs() < 0.005, "E5: Bl = {}", d.bl);
    assert!((d.cms / r("cms_derived_m_per_N") - 1.0).abs() < 1e-12);
    assert!((d.rms / r("rms_derived_Ns_per_m") - 1.0).abs() < 1e-12);
    // The report serialises for the UI.
    let j = rep.to_json();
    assert_eq!(j["anomalies"][0]["likely"], "Cms_mm_per_N");
}

fn synthetic_record(primary: Value, datasheet: Value) -> String {
    json!({
        "schema": "acoustilab-driver/0.1",
        "name": "synthetic",
        "title": "synthetic test record",
        "provenance": {"origin": "user", "source": "test", "date": "2026-09-24",
                       "licence": "test", "condition": "free air", "air_load": "free air"},
        "primary": primary,
        "datasheet": datasheet
    })
    .to_string()
}

#[test]
fn governance_detects_a_milligram_for_gram_moving_mass() {
    // A consistent driver (Mms 0.3 g, Cms 12.618 mm/N, Bl 2.2377 T·m) whose
    // mass is printed as 0.3 mg: resonance and electrical-Q identities both
    // fail, and multiplying Mms by 1000 repairs both. (The spec's µg-for-mg
    // slip is the same factor, but `Dim::Mass` has no `ug` suffix, so a
    // record cannot carry a microgram value as printed.)
    let rec = parse_record(&synthetic_record(
        json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_mg": 0.3, "Sd_cm2": 10}),
        json!({"Bl_Tm": 2.2377, "Cms_mm_per_N": 12.6186}),
    ))
    .unwrap();
    let rep = governance(&rec);
    assert_eq!(rep.anomalies.len(), 1, "{:?}", rep.anomalies);
    let a = &rep.anomalies[0];
    assert_eq!(
        (a.field, a.printed.as_str(), a.likely.as_str()),
        ("Mms", "Mms_mg", "Mms_g")
    );
    assert_eq!(a.repairs, vec!["resonance", "electrical_q"]);
    assert!(a.primary && rep.primary_anomaly().is_some());
}

#[test]
fn governance_reports_every_field_when_the_slip_is_ambiguous() {
    // Cms printed in um/N with neither Bl nor Vas: only the resonance
    // identity can fail, and scaling Mms or Cms by 1000 repairs it equally,
    // so both candidates are reported (the element then refuses the record
    // and names both).
    let rec = parse_record(&synthetic_record(
        json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_g": 0.3, "Sd_cm2": 10}),
        json!({"Cms_um_per_N": 12.6186}),
    ))
    .unwrap();
    let rep = governance(&rec);
    assert_eq!(rep.check("resonance").unwrap().status, CheckStatus::Fail);
    let mut fields: Vec<&str> = rep.anomalies.iter().map(|a| a.field).collect();
    fields.sort_unstable();
    assert_eq!(fields, vec!["Cms", "Mms"], "{:?}", rep.anomalies);
    assert!(rep
        .anomalies
        .iter()
        .all(|a| a.repairs == vec!["resonance"] && a.factor == 1e3));
}

#[test]
fn record_model_block_cannot_carry_the_d0_set() {
    // Bl, Cms and Rms are derived from the primary set; a `model` key naming
    // any D0 quantity would be overwritten or clash at build time.
    for key in [
        "fs_Hz",
        "Mms_g",
        "Bl_Tm",
        "Cms_mm_per_N",
        "Rms_Ns_per_m",
        "model",
    ] {
        let mut v: Value = serde_json::from_str(&synthetic_record(
            json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_g": 0.3, "Sd_cm2": 10}),
            json!({}),
        ))
        .unwrap();
        v["model"] = json!({ key: 1 });
        let err = parse_record(&v.to_string()).unwrap_err();
        assert!(err.contains(key), "{key}: {err}");
    }
    // Element keys beyond the D0 set are fine, including ones that merely
    // start with the same letters.
    let mut v: Value = serde_json::from_str(&synthetic_record(
        json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_g": 0.3, "Sd_cm2": 10}),
        json!({}),
    ))
    .unwrap();
    v["model"] = json!({"Le_uH": 20, "R2_ohm": 15, "L2_uH": 60, "Rbend_Ns_per_m": 0.01,
                        "Ssur_cm2": 3, "Msur_mg": 20, "Kbend_N_per_m": 2000, "creep_lambda": 0.05});
    assert!(parse_record(&v.to_string()).is_ok());
}

#[test]
fn governance_warns_when_the_hump_is_unidentifiable() {
    let rec = parse_record(&synthetic_record(
        json!({"fs_Hz": 200, "Qms": 0.5, "Qes": 10, "Re_ohm": 32, "Mms_g": 0.2, "Sd_cm2": 10}),
        json!({}),
    ))
    .unwrap();
    let rep = governance(&rec);
    let c = rep.check("identifiability").unwrap();
    assert_eq!(c.status, CheckStatus::Warning);
    assert!((c.value.unwrap() - 0.05).abs() < 1e-15);
    for id in ["resonance", "electrical_q", "equivalent_volume", "total_q"] {
        assert_eq!(
            rep.check(id).unwrap().status,
            CheckStatus::NotAvailable,
            "{id}"
        );
    }
}

#[test]
fn records_are_validated() {
    let bad_units = synthetic_record(
        json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re": 32.8, "Mms_g": 0.3, "Sd_cm2": 10}),
        json!({}),
    );
    assert!(parse_record(&bad_units).is_err(), "unit-less key");
    let unknown = synthetic_record(
        json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_g": 0.3, "Sd_cm2": 10}),
        json!({"Bl_Tm": 2.4, "Blx_Tm": 1}),
    );
    assert!(parse_record(&unknown).is_err(), "unknown datasheet key");
    let mut v: Value = serde_json::from_str(&synthetic_record(
        json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_g": 0.3, "Sd_cm2": 10}),
        json!({}),
    ))
    .unwrap();
    v["provenance"]["licence"] = json!(" ");
    assert!(parse_record(&v.to_string()).is_err(), "empty licence");
}

// ----- Element against the independent reference ---------------------------

fn case_element(case: &Value, front: bool, rear: bool) -> Vec<Value> {
    let p = &case["params"];
    let pr = &p["primary"];
    let mut d = json!({
        "model": p["model"], "fs_Hz": pr["fs_Hz"], "Qms": pr["Qms"], "Qes": pr["Qes"],
        "Re_ohm": pr["Re_ohm"], "Mms_kg": pr["Mms_kg"], "Sd_m2": pr["Sd_m2"]
    });
    for k in ["Le_H", "L2_H", "R2_ohm", "creep_lambda"] {
        if !p[k].is_null() {
            d[k] = p[k].clone();
        }
    }
    if let Some(d2) = p["d2"].as_object() {
        for (k, v) in d2 {
            d[k] = v.clone();
        }
    }
    let k0 = 1.204 * 343.0 * 343.0;
    let mut els = vec![
        json!({"id": "amp", "type": "vsource", "node": "e1", "V_V": p["V_V"], "Zs_ohm": p["Zs_ohm"]}),
    ];
    let mut e = drv(d);
    e["nodes"] = json!([
        "e1",
        "gnd",
        if front { "af" } else { "ambient" },
        if rear { "ar" } else { "ambient" }
    ]);
    els.push(e);
    for (node, spec) in [("af", &p["front"]), ("ar", &p["rear"])] {
        if spec.is_null() {
            continue;
        }
        if let Some(v) = spec["volume_m3"].as_f64() {
            els.push(json!({"id": format!("c_{node}"), "type": "acoustic_compliance", "node": node, "C_m3_per_Pa": v / k0}));
        }
        if let Some(r) = spec["R_Pa_s_per_m3"].as_f64() {
            els.push(json!({"id": format!("r_{node}"), "type": "acoustic_resistance", "node": node, "R_Pa_s_per_m3": r}));
        }
    }
    els
}

fn cval(v: &Value) -> C64 {
    C64::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
}

#[test]
fn driver_matches_independent_newton_law_solution() {
    // Tolerance 1e-9 relative: both sides are exact solutions of the same
    // model, one in 40-digit arithmetic, one by equilibrated double LU.
    let refs = reference();
    for (name, front, rear) in [
        ("d1_sealed", true, true),
        ("d2_sealed", true, true),
        ("d2_free", false, false),
    ] {
        let case = &refs["cases"][name];
        let els = case_element(case, front, rear);
        let mut probes = vec![
            json!({"id": "zin", "quantity": "impedance", "element": "drv", "port": 0}),
            json!({"id": "v1", "quantity": "velocity", "node": "drv.m"}),
            json!({"id": "v2", "quantity": "velocity", "node": "drv.m2"}),
            json!({"id": "u", "quantity": "volume_velocity", "element": "drv", "port": 1}),
        ];
        if front {
            probes.push(json!({"id": "pf", "quantity": "pressure", "node": "af"}));
            probes.push(json!({"id": "pr", "quantity": "pressure", "node": "ar"}));
        }
        let c = circuit(json!(els), json!(probes));
        let freqs = case["frequencies_Hz"].as_array().unwrap();
        for (k, f) in freqs.iter().enumerate() {
            let f = f.as_f64().unwrap();
            let x = c.solve_at(f).unwrap();
            let get = |id: &str| {
                let p = c.probes.iter().find(|p| p.id == id).unwrap();
                c.probe_value(p, f, &x).unwrap()
            };
            let close = |a: C64, b: C64, what: &str| {
                assert!(
                    (a - b).norm() <= 1e-9 * b.norm(),
                    "{name} {what} at {f} Hz: {a} vs {b}"
                );
            };
            close(get("zin"), cval(&case["zin"][k]), "Zin");
            close(get("v1"), cval(&case["v_dome"][k]), "v_dome");
            close(get("v2"), cval(&case["v_surround"][k]), "v_surround");
            let seff = -get("u") / get("v1");
            close(seff, cval(&case["s_eff"][k]), "S_eff");
            if front {
                close(get("pf"), cval(&case["p_front"][k]), "p_front");
                close(get("pr"), cval(&case["p_rear"][k]), "p_rear");
            }
        }
    }
}

#[test]
fn d1_coil_is_re_plus_le_plus_lr2() {
    // Unloaded D1 impedance equals Re + jωLe + R2∥jωL2 + Bl²/Zm exactly.
    let c = free_air(
        json!({"model": "D1", "Re_ohm": 32, "Bl_Tm": 1.5, "Mms_g": 0.3,
            "Cms_mm_per_N": 1.0, "Rms_Ns_per_m": 0.05, "Sd_cm2": 10,
            "Le_uH": 50, "L2_uH": 80, "R2_ohm": 12}),
        0.0,
    );
    let el = c.elements[c.element_index("drv").unwrap()]
        .as_any()
        .downcast_ref::<Driver>()
        .unwrap();
    assert_eq!(el.level, DriverLevel::D1);
    for f in [20.0, 290.0, 3000.0, 20000.0] {
        let w = 2.0 * PI * f;
        let jw = C64::new(0.0, w);
        let zm = jw * 0.3e-3 + 0.05 + (jw * 1e-3).inv();
        let zl = jw * 80e-6 * 12.0 / (jw * 80e-6 + 12.0);
        let expect = 32.0 + jw * 50e-6 + zl + 1.5 * 1.5 / zm;
        assert!((probe(&c, "zin", f) - expect).norm() < 1e-12 * expect.norm());
        assert!((el.unloaded_impedance(w) - expect).norm() < 1e-12 * expect.norm());
    }
}

// ----- Spec Section 17 impedance-peak checks --------------------------------

fn free_air(params: Value, zs: f64) -> Circuit {
    let mut d = drv(params);
    d["nodes"] = json!(["e1", "gnd", "ambient", "ambient"]);
    circuit(json!([source(zs), d]), zin_probe())
}

#[test]
fn free_air_peak_equals_re_plus_bl2_over_rms() {
    let (re, bl, rms) = (32.0, 1.5, 0.05);
    let c = free_air(physical(re, bl), 0.0);
    let fs = 1.0 / (2.0 * PI * (0.3e-3f64 * 1e-3).sqrt());
    let (fpk, zpk) = golden_max(|f| probe(&c, "zin", f).norm(), fs / 2.0, 2.0 * fs);
    let expect = re + bl * bl / rms;
    // Spec: within 0.1 %; the lumped model makes it exact.
    assert!((zpk / expect - 1.0).abs() < 1e-3);
    assert!((zpk / expect - 1.0).abs() < 1e-9, "{zpk} vs {expect}");
    assert!((fpk / fs - 1.0).abs() < 1e-6, "{fpk} vs {fs}");
}

#[test]
fn sealed_cavity_moves_the_peak_by_the_c2_identity() {
    // Appendix C2: fc² − fs² = Sd²/(4π²·Mms·Caf); a lossless cavity keeps the
    // peak height (C5).
    let air = AirState::spec_reference();
    let v = 30e-6;
    let caf = v / air.bulk_modulus();
    let (mms, cms, sd): (f64, f64, f64) = (0.3e-3, 1e-3, 10e-4);
    let fs = 1.0 / (2.0 * PI * (mms * cms).sqrt());
    let mut d = drv(physical(32.0, 1.5));
    d["nodes"] = json!(["e1", "gnd", "af", "ambient"]);
    let c = circuit(
        json!([source(0.0), d,
            {"id": "front", "type": "cavity", "node": "af", "volume_m3": v, "wall_loss": false}]),
        zin_probe(),
    );
    let (fc, zc) = golden_max(|f| probe(&c, "zin", f).norm(), fs, 10.0 * fs);
    let lhs = fc * fc - fs * fs;
    let rhs = sd * sd / (4.0 * PI * PI * mms * caf);
    assert!((lhs / rhs - 1.0).abs() < 5e-3, "spec tolerance");
    assert!((lhs / rhs - 1.0).abs() < 1e-7, "{lhs} vs {rhs}");
    assert!(
        (zc / (32.0 + 1.5 * 1.5 / 0.05) - 1.0).abs() < 1e-9,
        "height kept"
    );
}

#[test]
fn acoustic_resistance_lowers_the_peak_as_predicted() {
    // A rear resistance Ra adds Sd²·Ra to Rms without reactance, so the peak
    // stays at fs with height Re + Bl²/(Rms + Sd²·Ra).
    let (re, bl, rms, sd) = (32.0, 1.5, 0.05, 10e-4);
    let fs = 1.0 / (2.0 * PI * (0.3e-3f64 * 1e-3).sqrt());
    for ra in [1e4, 5e4, 2e5] {
        let mut d = drv(physical(re, bl));
        d["nodes"] = json!(["e1", "gnd", "ambient", "ar"]);
        let c = circuit(
            json!([source(0.0), d,
                {"id": "damp", "type": "acoustic_resistance", "node": "ar", "R_Pa_s_per_m3": ra}]),
            zin_probe(),
        );
        let (fpk, zpk) = golden_max(|f| probe(&c, "zin", f).norm(), fs / 2.0, 2.0 * fs);
        let expect = re + bl * bl / (rms + sd * sd * ra);
        assert!(zpk < re + bl * bl / rms);
        assert!((zpk / expect - 1.0).abs() < 1e-3, "spec tolerance");
        assert!((zpk / expect - 1.0).abs() < 1e-9, "{zpk} vs {expect}");
        assert!((fpk / fs - 1.0).abs() < 1e-6);
    }
    // A leak across a sealed cavity: the peak falls to the closed-form
    // maximum of Re + Bl²/(Zm + Sd²·Za), Za = 1/(jωCaf + 1/R).
    let air = AirState::spec_reference();
    let caf = 30e-6 / air.bulk_modulus();
    let r_leak = 3e6;
    let mut d = drv(physical(re, bl));
    d["nodes"] = json!(["e1", "gnd", "af", "ambient"]);
    let c = circuit(
        json!([source(0.0), d,
            {"id": "front", "type": "acoustic_compliance", "node": "af", "C_m3_per_Pa": caf},
            {"id": "leak", "type": "acoustic_resistance", "node": "af", "R_Pa_s_per_m3": r_leak}]),
        zin_probe(),
    );
    let closed = |f: f64| {
        let jw = C64::new(0.0, 2.0 * PI * f);
        let zm = jw * 0.3e-3 + rms + (jw * 1e-3).inv();
        let za = (jw * caf + 1.0 / r_leak).inv();
        (re + bl * bl / (zm + sd * sd * za)).norm()
    };
    let (_, z_sim) = golden_max(|f| probe(&c, "zin", f).norm(), fs, 10.0 * fs);
    let (_, z_cf) = golden_max(closed, fs, 10.0 * fs);
    assert!(z_sim < re + bl * bl / rms);
    assert!((z_sim / z_cf - 1.0).abs() < 1e-9, "{z_sim} vs {z_cf}");
}

// ----- D2 ----------------------------------------------------------------------

fn d2_params(r_bend: f64, creep: Option<f64>) -> Value {
    let mut p = json!({"model": "D2", "Re_ohm": 32.8, "Bl_Tm": 2.24, "Mms_g": 0.3,
        "Cms_mm_per_N": 12.6, "Rms_Ns_per_m": 0.057, "Sd_cm2": 10,
        "Msur_mg": 20, "Ssur_cm2": 3, "Kbend_N_per_m": 2000, "Rbend_Ns_per_m": r_bend});
    if let Some(l) = creep {
        p["creep_lambda"] = json!(l);
    }
    p
}

fn d2_free(r_bend: f64) -> (Circuit, driver::TwoDof) {
    let mut d = drv(d2_params(r_bend, None));
    d["nodes"] = json!(["e1", "gnd", "ambient", "ambient"]);
    let c = circuit(
        json!([source(0.0), d]),
        json!([
            {"id": "u", "quantity": "volume_velocity", "element": "drv", "port": 1},
            {"id": "v1", "quantity": "velocity", "node": "drv.m"}
        ]),
    );
    let t = c.elements[c.element_index("drv").unwrap()]
        .as_any()
        .downcast_ref::<Driver>()
        .unwrap()
        .two_dof
        .unwrap();
    (c, t)
}

fn s_eff(c: &Circuit, f: f64) -> C64 {
    -probe(c, "u", f) / probe(c, "v1", f)
}

#[test]
fn d2_effective_area_is_complex_and_dips_where_the_surround_is_in_antiphase() {
    // Lossless bending coupling: S_eff = S1 + S2·K_b/(K_b + K_o − ω²M2)
    // vanishes at ω² = (K_b + K_o + K_b·S2/S1)/M2.
    let (c, t) = d2_free(0.0);
    let fz = t.area_zero_hz();
    assert!(fz > 1000.0 && fz < 6000.0, "spec: 1 to 6 kHz region, {fz}");
    assert!(s_eff(&c, fz).norm() < 1e-9 * 10e-4, "{}", s_eff(&c, fz));
    // Surround in antiphase just above the edge resonance.
    let fr = t.surround_resonance_hz();
    assert!(fr < fz);
    let v2 = t.velocity_ratio(2.0 * PI * fz, C64::new(1.0, 0.0));
    assert!(v2.re < 0.0 && v2.im.abs() < 1e-12, "{v2}");
    // Low-frequency limit is the D0 area Sd; high-frequency limit is S1.
    assert!((s_eff(&c, 10.0).norm() / 10e-4 - 1.0).abs() < 1e-4);
    assert!((s_eff(&c, 40000.0).norm() / t.s_dome - 1.0).abs() < 0.01);

    // With loss the area is complex and the dip is finite.
    let (c, t) = d2_free(0.005);
    let sz = s_eff(&c, t.area_zero_hz());
    assert!(sz.im.abs() > 0.1 * sz.norm(), "complex: {sz}");
    let (_, neg_min) = golden_max(|f| -s_eff(&c, f).norm(), fr * 1.05, 2.0 * fz);
    let dip = -neg_min;
    assert!(dip > 0.0 && dip < 0.2 * 10e-4, "dip {dip}");
}

#[test]
fn d2_reproduces_the_d0_free_air_parameters_at_low_frequency() {
    // The split keeps the motor-driven Mms, Cms, Rms and Sd, so unloaded D2
    // departs from D0 through the next term of the reduction: an extra mass
    // δM = r²·M2·(f/f_sur)² (f_sur the blocked-dome surround resonance), i.e.
    // ΔZ ≈ −Bl²·jω·δM/Zm². Checked to 5 % of ΔZ (the next order is
    // (f/f_sur)² relative), with |ΔZ|/|Z| < 1e-3 at 100 Hz.
    let mk = |model: &str| {
        let mut p = d2_params(0.0, None);
        p["model"] = json!(model);
        free_air(p, 0.0)
    };
    let (c0, c2) = (mk("D0"), mk("D2"));
    let el = c2.elements[c2.element_index("drv").unwrap()]
        .as_any()
        .downcast_ref::<Driver>()
        .unwrap();
    let t = el.two_dof.unwrap();
    let p = el.params;
    let w2 = 2.0 * PI * t.surround_resonance_hz();
    for f in [12.5, 25.0, 50.0, 100.0] {
        let w = 2.0 * PI * f;
        let jw = C64::new(0.0, w);
        let dz = probe(&c2, "zin", f) - probe(&c0, "zin", f);
        let zm = jw * p.mms + p.rms + (jw * p.cms).inv();
        let dm = t.coupling * t.coupling * t.m_surround * (w / w2).powi(2);
        let predicted = -p.bl * p.bl * jw * dm / (zm * zm);
        assert!(
            (dz - predicted).norm() < 0.05 * predicted.norm(),
            "{f} Hz: {dz} vs {predicted}"
        );
        if f == 100.0 {
            assert!(dz.norm() < 1e-3 * probe(&c0, "zin", f).norm());
        }
    }
}

#[test]
fn d2_adds_the_surround_pressure_compliance_in_a_sealed_cup() {
    // Under pressure the surround also bulges relative to the dome. Statics
    // of the two-mass system give a volume compliance to pressure of
    // Sd²·Cms + r·(1 − r)·S2²·Cms instead of D0's Sd²·Cms (while the
    // motor-driven compliance Sd·Cms is unchanged), so in a sealed cavity of
    // compliance Caf the low-frequency pressure ratio is
    // (1 + Sd²·Cms/Caf)/(1 + (Sd²·Cms + r(1 − r)·S2²·Cms)/Caf).
    let air = AirState::spec_reference();
    let caf = 30e-6 / air.bulk_modulus();
    let mk = |model: &str| {
        let mut p = d2_params(0.0, None);
        p["model"] = json!(model);
        let mut d = drv(p);
        d["nodes"] = json!(["e1", "gnd", "af", "ambient"]);
        circuit(
            json!([source(0.0), d,
                {"id": "front", "type": "acoustic_compliance", "node": "af", "C_m3_per_Pa": caf}]),
            zin_probe(),
        )
    };
    let (c0, c2) = (mk("D0"), mk("D2"));
    let t = c2.elements[c2.element_index("drv").unwrap()]
        .as_any()
        .downcast_ref::<Driver>()
        .unwrap()
        .two_dof
        .unwrap();
    let (cms, sd, s2, r) = (12.6e-3, 10e-4, t.s_surround, t.coupling);
    let b0 = sd * sd * cms;
    let b2 = b0 + r * (1.0 - r) * s2 * s2 * cms;
    let expect = (1.0 + b0 / caf) / (1.0 + b2 / caf);
    let got = (probe(&c2, "p", 10.0) / probe(&c0, "p", 10.0)).norm();
    // Residual dynamics at 10 Hz are O((10 Hz/fc)²) ~ 2e-4.
    assert!((got / expect - 1.0).abs() < 5e-4, "{got} vs {expect}");
    assert!(expect < 0.999, "the effect is resolvable: {expect}");
}

#[test]
fn switching_level_never_changes_the_netlist() {
    // One netlist with D1 and D2 keys and a probe on the surround node runs
    // at every level; at D0/D1 the surround node moves with the dome.
    for model in ["D0", "D1", "D2"] {
        let mut p = d2_params(0.005, Some(0.05));
        p["model"] = json!(model);
        p["Le_uH"] = json!(20);
        p["L2_uH"] = json!(60);
        p["R2_ohm"] = json!(15);
        let mut d = drv(p);
        d["nodes"] = json!(["e1", "gnd", "af", "ambient"]);
        let c = circuit(
            json!([source(0.0), d,
                {"id": "front", "type": "cavity", "node": "af", "volume_cm3": 30}]),
            json!([
                {"id": "v1", "quantity": "velocity", "node": "drv.m"},
                {"id": "v2", "quantity": "velocity", "node": "drv.m2"},
                {"id": "zin", "quantity": "impedance", "element": "drv"}
            ]),
        );
        let (v1, v2) = (probe(&c, "v1", 3000.0), probe(&c, "v2", 3000.0));
        if model == "D2" {
            assert!((v2 - v1).norm() > 0.1 * v1.norm());
        } else {
            assert!((v2 - v1).norm() < 1e-12 * v1.norm());
        }
        // Inductance only from D1 up.
        let z = probe(&c, "zin", 20000.0);
        if model == "D0" {
            assert!(z.im < 0.0, "mass-controlled motional reactance: {z}");
        } else {
            assert!(z.im > 1.0, "{z}");
        }
    }
}

// ----- Power balance and passivity ------------------------------------------

fn cup(model: &str, zs: f64) -> Circuit {
    let mut p = d2_params(0.005, Some(0.08));
    p["model"] = json!(model);
    p["Le_uH"] = json!(20);
    p["L2_uH"] = json!(60);
    p["R2_ohm"] = json!(15);
    circuit(
        json!([source(zs), drv(p),
            {"id": "front", "type": "cavity", "node": "af", "volume_cm3": 30},
            {"id": "leak", "type": "slit", "nodes": ["af", "ambient"], "gap_mm": 0.1, "width_mm": 20, "length_mm": 8},
            {"id": "rear", "type": "cavity", "node": "ar", "volume_cm3": 20},
            {"id": "vent", "type": "acoustic_resistance", "node": "ar", "R_Pa_s_per_m3": 5e6}]),
        zin_probe(),
    )
}

#[test]
fn power_balances_over_the_driver_macro_ports() {
    // Tellegen: absorbed powers (sources negative) sum to zero; the driver
    // absorbs the coil, suspension, bending and creep losses (> 0).
    for model in ["D0", "D1", "D2"] {
        let c = cup(model, 10.0);
        for f in [15.0, 81.8, 400.0, 1624.0, 1925.0, 9000.0, 20000.0] {
            let x = c.solve_at(f).unwrap();
            let p = c.power_absorbed(f, &x);
            assert!(p.iter().all(|(_, v)| v.is_some()), "{p:?}");
            let delivered = -p.iter().find(|(id, _)| id == "amp").unwrap().1.unwrap();
            let drv = p.iter().find(|(id, _)| id == "drv").unwrap().1.unwrap();
            let sum: f64 = p.iter().map(|(_, v)| v.unwrap()).sum();
            assert!(delivered > 0.0 && drv > 0.0);
            assert!(sum.abs() < 1e-12 * delivered, "{model} f={f}: {p:?}");
            // Nothing outside the macro touches its internal nodes.
            let d = &c.elements[c.element_index("drv").unwrap()];
            let br = c.branches(c.element_index("drv").unwrap());
            let cx = c.cx(f);
            let force = d.port_flow(&cx, &x, &br, 3).unwrap();
            let f_motor = 2.24 * d.port_flow(&cx, &x, &br, 0).unwrap().norm();
            assert!(force.norm() < 1e-12 * f_motor, "{force}");
        }
    }
}

#[test]
fn added_mass_on_the_dome_node_shifts_fs_and_keeps_the_balance() {
    // Classical added-mass method: fs' = fs·sqrt(Mms/(Mms + Madd)). The mass
    // hangs on the internal node <id>.m, so power flows through the macro's
    // mechanical port.
    let madd: f64 = 0.1e-3;
    let mut d = drv(physical(32.0, 1.5));
    d["nodes"] = json!(["e1", "gnd", "ambient", "ambient"]);
    let c = circuit(
        json!([source(0.0), d, {"id": "added", "type": "mass", "node": "drv.m", "M_kg": madd}]),
        zin_probe(),
    );
    let fs = 1.0 / (2.0 * PI * (0.3e-3f64 * 1e-3).sqrt());
    let (fpk, _) = golden_max(|f| probe(&c, "zin", f).norm(), fs / 3.0, 2.0 * fs);
    let expect = fs * (0.3e-3 / (0.3e-3 + madd)).sqrt();
    assert!((fpk / expect - 1.0).abs() < 1e-6, "{fpk} vs {expect}");
    for f in [50.0, fpk, 1000.0] {
        let x = c.solve_at(f).unwrap();
        let p = c.power_absorbed(f, &x);
        let delivered = -p.iter().find(|(id, _)| id == "amp").unwrap().1.unwrap();
        let sum: f64 = p.iter().map(|(_, v)| v.unwrap()).sum();
        assert!(sum.abs() < 1e-12 * delivered, "{p:?}");
    }
}

#[test]
fn input_impedance_is_passive() {
    // E17: Re(Zin) ≥ Re for a single moving coil driven directly (creep at
    // its largest allowed λ included).
    let lmax = Creep::max_lambda(80.0);
    for model in ["D0", "D1", "D2"] {
        let mut p = d2_params(0.005, Some(0.99 * lmax));
        p["model"] = json!(model);
        p["creep_f0_Hz"] = json!(80.0);
        p["Le_uH"] = json!(20);
        p["L2_uH"] = json!(60);
        p["R2_ohm"] = json!(15);
        let c = circuit(
            json!([source(0.0), drv(p),
                {"id": "front", "type": "cavity", "node": "af", "volume_cm3": 30},
                {"id": "rear", "type": "cavity", "node": "ar", "volume_cm3": 20},
                {"id": "vent", "type": "acoustic_resistance", "node": "ar", "R_Pa_s_per_m3": 5e6}]),
            json!([
                {"id": "zin", "quantity": "impedance", "element": "drv", "port": 0}
            ]),
        );
        for &f in &c.freqs {
            let z = probe(&c, "zin", f);
            assert!(z.re >= 32.8 * (1.0 - 1e-12), "{model} {f} Hz: {z}");
        }
    }
}

#[test]
fn acoustic_port_of_the_driver_is_passive() {
    // E16: Re(Z) ≥ 0 at every port. The driver's acoustic port is driven by
    // a volume-velocity source with the electrical port shorted through a
    // 0 or 10 Ω source, or left open, so the only power entering is the
    // source's: Re(p/U) ≥ 0 must hold with creep at its largest allowed λ,
    // at every level, over 10 Hz–40 kHz. The motor, the D2 two-mass branch
    // and the creep spring are all inside the port.
    let lmax = Creep::max_lambda(80.0);
    for model in ["D0", "D1", "D2"] {
        for lambda in [0.0, 0.99 * lmax] {
            for termination in [Some(0.0), Some(10.0), None] {
                let mut p = d2_params(0.005, Some(lambda));
                p["model"] = json!(model);
                p["creep_f0_Hz"] = json!(80.0);
                p["Le_uH"] = json!(20);
                p["L2_uH"] = json!(60);
                p["R2_ohm"] = json!(15);
                let mut d = drv(p);
                d["nodes"] = json!(["e1", "gnd", "af", "ambient"]);
                let mut els = vec![
                    d,
                    json!({"id": "q", "type": "flow_source", "node": "af", "U_m3_per_s": 1e-6}),
                ];
                if let Some(zs) = termination {
                    let mut s = source(zs);
                    s["V_V"] = json!(0.0);
                    els.push(s);
                }
                let doc = json!({
                    "air": {"preset": "spec_reference"},
                    "sweep": {"f_min_Hz": 10, "f_max_Hz": 40000, "points_per_octave": 24},
                    "nodes": [{"id": "e1", "domain": "electrical"}, {"id": "af", "domain": "acoustic"}],
                    "elements": els,
                    "probes": [{"id": "p", "quantity": "pressure", "node": "af"}]
                });
                let c = Circuit::from_json(&doc.to_string()).unwrap();
                let r = c.solve().unwrap();
                for (f, pf) in r.freqs_hz.iter().zip(&r.probe("p").unwrap().values) {
                    let z = pf / 1e-6;
                    assert!(
                        z.re >= 0.0,
                        "{model} λ={lambda} Zs={termination:?} {f} Hz: {z}"
                    );
                }
            }
        }
    }
}

// ----- Creep ---------------------------------------------------------------------

#[test]
fn creep_is_passive_and_its_real_compliance_stays_positive_in_band() {
    // Re{1/(jωC0·c)} ≥ 0 for every allowed λ; Re c > 0 over 10 Hz–40 kHz.
    let f0 = 81.8;
    let lmax = Creep::max_lambda(f0);
    assert!((lmax - 1.0 / (40000.0f64 / f0).log10()).abs() < 1e-15);
    for k in 0..=10 {
        let c = Creep {
            lambda: 0.0999 * k as f64 * lmax,
            f0,
        };
        c.validate().unwrap();
        let mut f = Creep::BAND_HZ.0;
        while f <= Creep::BAND_HZ.1 {
            let w = 2.0 * PI * f;
            let fac = c.factor(w);
            assert!(fac.re > 0.0);
            let zm = (C64::new(0.0, w) * 1e-3 * fac).inv();
            assert!(zm.re >= 0.0, "λ = {}: Re Zm = {}", c.lambda, zm.re);
            f *= 1.1;
        }
    }
    // The element rejects λ < 0 (active) and λ ≥ λmax (Re C ≤ 0 in band).
    for bad in [-0.01, lmax, 1.5 * lmax] {
        let mut p = physical(32.0, 1.5);
        p["creep_lambda"] = json!(bad);
        p["creep_f0_Hz"] = json!(f0);
        let doc = json!({"nodes": [{"id": "e1", "domain": "electrical"},
                {"id": "af", "domain": "acoustic"}, {"id": "ar", "domain": "acoustic"}],
            "elements": [source(0.0), drv(p)]});
        assert!(Circuit::from_json(&doc.to_string()).is_err(), "λ = {bad}");
    }
}

#[test]
fn creep_raises_the_low_frequency_compliance() {
    // Stiffness-controlled displacement at 10 Hz scales with |C(jω)|.
    let mk = |lambda: Option<f64>| {
        let mut p = physical(32.0, 1.5);
        if let Some(l) = lambda {
            p["creep_lambda"] = json!(l);
        }
        free_air(p, 0.0)
    };
    let (c0, c1) = (mk(None), mk(Some(0.1)));
    let f = 10.0;
    let r = (probe(&c1, "v", f) / probe(&c0, "v", f)).norm();
    let fs = 1.0 / (2.0 * PI * (0.3e-3f64 * 1e-3).sqrt());
    let fac = Creep {
        lambda: 0.1,
        f0: fs,
    }
    .factor(2.0 * PI * f);
    // Below fs/29 the mass and loss terms change the ratio by < 0.5 %.
    assert!((r / fac.norm() - 1.0).abs() < 5e-3, "{r} vs {}", fac.norm());
    assert!(r > 1.08);
}

/// Subtracted Kramers–Kronig estimate of Re Z(ω1) − Re Z(ω2) from Im Z on
/// a log grid, for e^{+jωt} (analytic in the lower half ω-plane):
/// Re Z(ω1) − Re Z(ω2) = −(1/π)·PV∫ Im Z(e^u)·[coth(u − u1) − coth(u − u2)] du.
/// The poles are removed by subtracting Im Z(u_k)·coth(u − u_k) and adding
/// its exact principal value over [A, B].
fn kk_real_difference(im: &dyn Fn(f64) -> f64, w1: f64, w2: f64, a: f64, b: f64, n: usize) -> f64 {
    let (u1, u2) = (w1.ln(), w2.ln());
    let (i1, i2) = (im(w1), im(w2));
    let h = (b - a) / n as f64;
    let mut sum = 0.0;
    for k in 0..=n {
        let u = a + k as f64 * h;
        let iu = im(u.exp());
        let t = |uk: f64, ik: f64| {
            let d = u - uk;
            if d.abs() < 1e-9 {
                0.0
            } else {
                (iu - ik) / d.tanh()
            }
        };
        let g = t(u1, i1) - t(u2, i2);
        sum += if k == 0 || k == n { 0.5 * g } else { g };
    }
    let pv = |uk: f64| (b - uk).sinh().abs().ln() - (a - uk).sinh().abs().ln();
    let integral = h * sum + i1 * pv(u1) - i2 * pv(u2);
    -integral / PI
}

#[test]
fn creep_law_satisfies_kramers_kronig_and_the_real_only_law_does_not() {
    // The solver's input impedance of a D1 driver with creep and LR-2 must be
    // Hilbert-consistent (causal). Grid: 100 points per decade over
    // 1.07 mHz–10.7 GHz (offset so no pole abscissa falls on a node). The
    // trapezoidal rule on this smooth integrand is accurate to ~1e-12 of |Z|
    // (a Python prototype agreed at 50 and 200 per decade); the neglected
    // tail above the grid, ~2·Le·(ω1² − ω2²)/(π·ω_max), is below 1e-9·|Z|
    // for f1 ≤ 1 kHz.
    let fs = 81.8;
    let mut p = json!({"model": "D1", "fs_Hz": fs, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8,
        "Mms_g": 0.3, "Sd_cm2": 10, "creep_lambda": 0.1, "Le_uH": 20, "L2_uH": 60, "R2_ohm": 15});
    p["model"] = json!("D1");
    let c = free_air(p, 0.0);
    let (a, b) = ((2.0 * PI * 1.07e-3f64).ln(), (2.0 * PI * 1.07e10f64).ln());
    let n = 1300;
    let im_z = |w: f64| probe(&c, "zin", w / (2.0 * PI)).im;
    // Tabulate once (the integrand is evaluated on the grid only, plus the
    // two pole abscissae).
    let table: Vec<(f64, f64)> = (0..=n)
        .map(|k| {
            let u = a + k as f64 * (b - a) / n as f64;
            (u, im_z(u.exp()))
        })
        .collect();
    let lookup = |w: f64| {
        let u = w.ln();
        table
            .iter()
            .find(|(uu, _)| (uu - u).abs() < 1e-12)
            .map_or_else(|| im_z(w), |(_, v)| *v)
    };
    let w2 = 2.0 * PI * 20.0;
    let z2 = probe(&c, "zin", 20.0);
    for f1 in [5.0, 50.0, fs, 150.0, 1000.0] {
        let w1 = 2.0 * PI * f1;
        let predicted = kk_real_difference(&lookup, w1, w2, a, b, n);
        let actual = probe(&c, "zin", f1).re - z2.re;
        assert!(
            (predicted - actual).abs() < 1e-8 * z2.norm(),
            "{f1} Hz: KK {predicted} vs {actual}"
        );
    }

    // The real-only law C0·[1 − λ·log10(ω/ω0)] (closed form, unloaded) has
    // no loss term, so its Hilbert transform misses the log slope of Re C.
    let ts = driver::record(TYMPHANY).unwrap().primary.to_physical();
    let real_only = |w: f64| -> C64 {
        let jw = C64::new(0.0, w);
        let c = ts.cms * (1.0 - 0.1 * (w / (2.0 * PI * fs)).log10());
        // Suspension displacement compliance x/F = 1/(jω·Zm).
        (jw * (jw * ts.mms + ts.rms + (jw * c).inv())).inv()
    };
    let complex_law = |w: f64| -> C64 {
        let jw = C64::new(0.0, w);
        let c = ts.cms
            * Creep {
                lambda: 0.1,
                f0: fs,
            }
            .factor(w);
        (jw * (jw * ts.mms + ts.rms + (jw * c).inv())).inv()
    };
    let (a, b) = ((2.0 * PI * 1.07e-6f64).ln(), (2.0 * PI * 1.07e6f64).ln());
    for (law, consistent) in [
        (&complex_law as &dyn Fn(f64) -> C64, true),
        (&real_only as &dyn Fn(f64) -> C64, false),
    ] {
        let im = |w: f64| law(w).im;
        let mut worst: f64 = 0.0;
        for f1 in [5.0, 30.0, 150.0] {
            let w1 = 2.0 * PI * f1;
            let pred = kk_real_difference(&im, w1, w2, a, b, 1200);
            let act = law(w1).re - law(w2).re;
            worst = worst.max((pred - act).abs() / law(w2).norm());
        }
        if consistent {
            assert!(worst < 1e-9, "complex law: {worst}");
        } else {
            assert!(worst > 1e-3, "real-only law should fail: {worst}");
        }
    }
    // Sanity: the constant loss of the complex law is λ·π/(2 ln 10).
    let fac = Creep {
        lambda: 0.1,
        f0: fs,
    }
    .factor(1.0);
    assert!((fac.im + 0.1 * PI / (2.0 * LN_10)).abs() < 1e-15);
}

#[test]
fn creep_kramers_kronig_holds_in_band_over_the_allowed_lambda_range() {
    // The compliance C0·[1 − λ·log10(s/ω0)] is analytic for Re s > 0, but it
    // vanishes at the real s = ω0·10^(1/λ), so the stiffness 1/(sC) has a
    // pole there and the driver has one right-half-plane pole just above it
    // (42.6 kHz for this driver at 0.99·λmax; mpmath root search in review).
    // The λ limit keeps that point above 40 kHz. In band its share of the
    // impedance is tiny: the Hilbert transform of the solver's impedance
    // (D0, free air, same grid as above) agrees to quadrature accuracy up to
    // λmax/2 and to ~3e-8 of |Z| at 0.99·λmax, where the residual is
    // independent of the grid (it is the non-causal pole term, ≤ 4.4e-8
    // of |Z| in 10 Hz–40 kHz by the residue).
    let fs = 81.8;
    let lmax = Creep::max_lambda(fs);
    let (a, b) = ((2.0 * PI * 1.07e-3f64).ln(), (2.0 * PI * 1.07e10f64).ln());
    let n = 1300;
    for (frac, bound) in [(0.5, 1e-10), (0.99, 1e-6)] {
        let lambda = frac * lmax;
        let creep = Creep { lambda, f0: fs };
        assert!(creep.validate().is_ok());
        assert!(creep.zero_crossing_hz() > Creep::BAND_HZ.1);
        let c = free_air(
            json!({"model": "D0", "fs_Hz": fs, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8,
                   "Mms_g": 0.3, "Sd_cm2": 10, "creep_lambda": lambda}),
            0.0,
        );
        let im_z = |w: f64| probe(&c, "zin", w / (2.0 * PI)).im;
        let table: Vec<(f64, f64)> = (0..=n)
            .map(|k| {
                let u = a + k as f64 * (b - a) / n as f64;
                (u, im_z(u.exp()))
            })
            .collect();
        let lookup = |w: f64| {
            let u = w.ln();
            table
                .iter()
                .find(|(uu, _)| (uu - u).abs() < 1e-12)
                .map_or_else(|| im_z(w), |(_, v)| *v)
        };
        let w2 = 2.0 * PI * 20.0;
        let z2 = probe(&c, "zin", 20.0);
        let mut worst: f64 = 0.0;
        for f1 in [5.0, 50.0, fs, 150.0, 1000.0, 10000.0, 30000.0] {
            let w1 = 2.0 * PI * f1;
            let predicted = kk_real_difference(&lookup, w1, w2, a, b, n);
            let actual = probe(&c, "zin", f1).re - z2.re;
            worst = worst.max((predicted - actual).abs() / z2.norm());
        }
        assert!(worst < bound, "λ = {lambda}: KK residual {worst}");
        if frac > 0.9 {
            // The residual is real (the pole term), not quadrature noise.
            assert!(worst > 1e-9, "λ = {lambda}: KK residual {worst}");
        }
    }
}

// ----- Thiele–Small identification --------------------------------------------

fn sweep_impedance(c: &Circuit) -> (Vec<f64>, Vec<C64>) {
    let r = c.solve().unwrap();
    (r.freqs_hz.clone(), r.probe("zin").unwrap().values.clone())
}

#[test]
fn ts_round_trip_recovers_the_primary_set_within_1_percent() {
    // Spec Section 17: fit fs, Qms, Qes and Re from the simulated impedance
    // (48 points per octave, 10 Hz–20 kHz) and recover them within 1 %.
    for (fs, qms, qes, re, mms) in [
        (81.8, 2.71, 1.01, 32.8, 0.3e-3),
        (300.0, 8.0, 0.5, 32.0, 0.1e-3),
        (120.0, 1.5, 3.0, 300.0, 0.25e-3),
    ] {
        let c = free_air(
            json!({"model": "D0", "fs_Hz": fs, "Qms": qms, "Qes": qes, "Re_ohm": re,
                   "Mms_kg": mms, "Sd_cm2": 10}),
            0.0,
        );
        let (f, z) = sweep_impedance(&c);
        let est = ts::extract_ts(&f, &z, None).unwrap();
        for (got, want, what) in [
            (est.fs, fs, "fs"),
            (est.qms, qms, "Qms"),
            (est.qes, qes, "Qes"),
            (est.re, re, "Re"),
            (est.qts, ts::qts(qms, qes), "Qts"),
        ] {
            assert!((got / want - 1.0).abs() < 0.01, "{what}: {got} vs {want}");
            // Interpolation error at 48 per octave is below 0.1 %.
            assert!((got / want - 1.0).abs() < 1e-3, "{what}: {got} vs {want}");
        }
        assert!(est.warnings.is_empty(), "{:?}", est.warnings);
        let back = est.with_mass(mms, 10e-4);
        assert!((back.bl / ts::force_factor(qes, re, mms, fs) - 1.0).abs() < 2e-3);
    }
}

#[test]
fn extraction_warns_when_the_hump_is_too_small() {
    let c = free_air(
        json!({"model": "D0", "fs_Hz": 200, "Qms": 0.5, "Qes": 10, "Re_ohm": 32,
               "Mms_g": 0.2, "Sd_cm2": 10}),
        0.0,
    );
    let (f, z) = sweep_impedance(&c);
    let est = ts::extract_ts(&f, &z, None).unwrap();
    assert!(est.q_ratio() < ts::MIN_Q_RATIO);
    assert!(
        est.warnings.iter().any(|w| w.contains("too small")),
        "{:?}",
        est.warnings
    );
}

#[test]
fn synthetic_fit_recovers_creep_and_lossy_inductance_within_2_percent() {
    // Spec Section 17: a generated impedance with known creep and
    // lossy-inductance parameters is recovered within 2 %.
    let truth = ImpedanceModel {
        re: 32.8,
        fs: 81.8,
        qms: 2.71,
        qes: 1.01,
        creep_lambda: 0.08,
        le: 20e-6,
        l2: 60e-6,
        r2: 15.0,
    };
    let c = free_air(
        json!({"model": "D1", "fs_Hz": truth.fs, "Qms": truth.qms, "Qes": truth.qes,
               "Re_ohm": truth.re, "Mms_g": 0.3, "Sd_cm2": 10,
               "creep_lambda": truth.creep_lambda, "Le_H": truth.le,
               "L2_H": truth.l2, "R2_ohm": truth.r2}),
        0.0,
    );
    let (f, z) = sweep_impedance(&c);
    // The identification model is the element's own impedance.
    for (fk, zk) in f.iter().zip(&z) {
        assert!((truth.impedance(*fk) - zk).norm() < 1e-10 * zk.norm());
    }
    let opts = FitOptions::default();
    let init = ts::initial_model(&f, &z, &opts).unwrap();
    let fit = ts::fit_impedance(&f, &z, init, &opts).unwrap();
    assert!(fit.converged, "{fit:?}");
    let m = fit.model;
    for (got, want, what) in [
        (m.re, truth.re, "Re"),
        (m.fs, truth.fs, "fs"),
        (m.qms, truth.qms, "Qms"),
        (m.qes, truth.qes, "Qes"),
        (m.creep_lambda, truth.creep_lambda, "creep"),
        (m.le, truth.le, "Le"),
        (m.l2, truth.l2, "L2"),
        (m.r2, truth.r2, "R2"),
    ] {
        assert!((got / want - 1.0).abs() < 0.02, "{what}: {got} vs {want}");
    }
    assert!(fit.rms_relative_error < 1e-8, "{}", fit.rms_relative_error);
}

#[test]
fn tymphany_reproduces_its_datasheet_impedance_within_3_percent() {
    // First-release threshold (spec p. 59): resonance, minimum impedance and
    // total Q within 3 % of the datasheet. Zmin is 33.53 Ω on the sheet; the
    // record's Le = 0 puts the model minimum at Re = 32.8 Ω (−2.2 %).
    let rec = driver::record(TYMPHANY).unwrap();
    let c = free_air(json!({"record": TYMPHANY, "model": "D1"}), 0.0);
    let (f, z) = sweep_impedance(&c);
    let est = ts::extract_ts(&f, &z, None).unwrap();
    assert!((est.fs / rec.primary.fs - 1.0).abs() < 0.03);
    assert!(
        (est.qts / rec.datasheet.qts.unwrap() - 1.0).abs() < 0.03,
        "{}",
        est.qts
    );
    let zmin = f
        .iter()
        .zip(&z)
        .filter(|(fk, _)| **fk > est.fs)
        .map(|(_, zk)| zk.norm())
        .fold(f64::INFINITY, f64::min);
    assert!(
        (zmin / rec.datasheet.z_min.unwrap() - 1.0).abs() < 0.03,
        "{zmin}"
    );
}

// ----- Conventions and errors ------------------------------------------------------

#[test]
fn sign_convention_holds_through_the_macro() {
    // Spec Section 3 at 20 Hz (erratum E12): positive voltage drives positive
    // current, positive velocity toward the ear and positive front pressure.
    for model in ["D0", "D1", "D2"] {
        let mut p = d2_params(0.005, Some(0.05));
        p["model"] = json!(model);
        let mut d = drv(p);
        d["nodes"] = json!(["e1", "gnd", "af", "ambient"]);
        let c = circuit(
            json!([source(0.0), d, {"id": "front", "type": "cavity", "node": "af", "volume_cm3": 30}]),
            zin_probe(),
        );
        let pf = probe(&c, "p", 20.0);
        assert!(pf.re > 0.0 && pf.arg().abs() < 0.1, "{model}: {pf}");
        assert!(probe(&c, "i", 20.0).re > 0.0);
        assert!(probe(&c, "v", 20.0).im > 0.0, "stiffness-controlled");
    }
}

#[test]
fn rejects_inconsistent_parameters() {
    let build = |p: Value| {
        let doc = json!({"nodes": [{"id": "e1", "domain": "electrical"},
                {"id": "af", "domain": "acoustic"}, {"id": "ar", "domain": "acoustic"}],
            "elements": [source(0.0), drv(p)]});
        Circuit::from_json(&doc.to_string())
    };
    assert!(build(json!({"record": TYMPHANY})).is_ok());
    assert!(build(json!({"record": "nope"})).is_err(), "unknown record");
    assert!(
        build(json!({"record": TYMPHANY, "Re_ohm": 32})).is_err(),
        "record + inline duplicate"
    );
    assert!(
        build(json!({"record": TYMPHANY, "Bl_Tm": 2.42})).is_err(),
        "Bl is derived"
    );
    assert!(
        build(json!({"record": TYMPHANY, "model": "D3"})).is_err(),
        "unknown level"
    );
    assert!(
        build(json!({"record": TYMPHANY, "model": "D2"})).is_err(),
        "D2 needs surround keys"
    );
    assert!(
        build(json!({"record": TYMPHANY, "Msur_mg": 20})).is_err(),
        "partial surround"
    );
    assert!(
        build(json!({"record": TYMPHANY, "colour": "red"})).is_err(),
        "unknown key"
    );
    assert!(
        build(json!({"record": TYMPHANY, "creep_f0_Hz": 80})).is_err(),
        "f0 without λ"
    );
    let mut p = physical(32.0, 1.5);
    p["Kms_N_per_m"] = json!(1000);
    assert!(build(p).is_err(), "Cms and Kms");
    let mut p = physical(32.0, 1.5);
    p["fs_Hz"] = json!(290);
    assert!(build(p).is_err(), "mixed primary and physical sets");
    let mut p = d2_params(0.0, None);
    p["Kbend_N_per_m"] = json!(50);
    assert!(build(p.clone()).is_err(), "bending compliance above Cms");
    // The split is checked at every level, so a netlist that builds at D0
    // also builds at D2.
    for model in ["D0", "D1"] {
        p["model"] = json!(model);
        assert!(build(p.clone()).is_err(), "invalid surround at {model}");
    }
    let mut p = d2_params(0.0, None);
    p["Ssur_cm2"] = json!(20);
    assert!(build(p).is_err(), "surround area above Sd");
    let mut p = d2_params(0.0, None);
    p["Msur_g"] = json!(1);
    p.as_object_mut().unwrap().remove("Msur_mg");
    assert!(build(p).is_err(), "surround mass above Mms");
    let mut p = physical(32.0, 1.5);
    p["L2_uH"] = json!(10);
    assert!(build(p).is_err(), "L2 without R2");
    // A record whose primary mass has a unit slip is refused.
    let rep = governance(
        &parse_record(&synthetic_record(
            json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_mg": 0.3, "Sd_cm2": 10}),
            json!({"Bl_Tm": 2.2377, "Cms_mm_per_N": 12.6186}),
        ))
        .unwrap(),
    );
    assert!(rep.primary_anomaly().is_some());
}

#[test]
fn record_and_inline_primary_sets_are_the_same_driver() {
    let from_record = free_air(json!({"record": TYMPHANY}), 0.0);
    let inline = free_air(
        json!({"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_g": 0.3, "Sd_cm2": 10, "Le_mH": 0}),
        0.0,
    );
    let phys = driver::record(TYMPHANY).unwrap().primary.to_physical();
    let physical_set = free_air(
        json!({"Re_ohm": phys.re, "Bl_Tm": phys.bl, "Mms_kg": phys.mms, "Cms_m_per_N": phys.cms,
               "Rms_Ns_per_m": phys.rms, "Sd_m2": phys.sd}),
        0.0,
    );
    for f in [20.0, 81.8, 500.0, 5000.0] {
        let a = probe(&from_record, "zin", f);
        for other in [&inline, &physical_set] {
            let b = probe(other, "zin", f);
            assert!((a - b).norm() < 1e-12 * a.norm(), "{f}: {a} vs {b}");
        }
    }
    let el = from_record.elements[from_record.element_index("drv").unwrap()]
        .as_any()
        .downcast_ref::<Driver>()
        .unwrap();
    assert_eq!(el.record.as_deref(), Some(TYMPHANY));
    assert!((el.params.bl - 2.2377).abs() < 1e-4);
    // Piston validity: ka = 1 at 3.06 kHz for Sd = 10 cm² (erratum E28).
    let lim = from_record
        .validity()
        .into_iter()
        .find(|l| l.element == "drv")
        .unwrap();
    assert!((lim.begin_hz.unwrap() - 3060.0).abs() < 5.0, "{lim:?}");
}
