//! Design analyses (docs/analysis.md): sensitivities, tornado charts,
//! explain sentences, readouts, Monte Carlo and design of experiments.
//!
//! Sensitivities and readouts are checked against closed forms of small
//! networks (series RC, a sealed cavity, a D0 driver in a lossless
//! pressure chamber and in free air, a first-order acoustic high-pass).
//! Sampling, hashing and percentiles are checked against an independent
//! Python implementation (`tools/analysis/reference.py`, fixture
//! `tests/data/analysis_reference.json`). Explain sentences are checked
//! against re-solves made here, independently of the explain code.
//!
//! Tolerances: finite-difference sensitivities 1e-7 relative plus 1e-10 dB
//! (or degrees) per percent absolute, above the truncation bound
//! (2Q·h)²/6 (below 1e-8 here with the default h = 1e-5) and the rounding
//! bound ε/h (see `analysis::sensitivity`); refined readouts 1e-7 relative (Brent's method
//! locates a maximum to about √ε in ln f); identities and exact re-solves
//! 1e-9 to 1e-12.

use acoustilab::analysis::canonical;
use acoustilab::analysis::detmath;
use acoustilab::analysis::explain::{self, ExplainOptions};
use acoustilab::analysis::mc::{self, PlanSpec, RunOptions, Sample};
use acoustilab::analysis::readouts::{self, ReadoutOptions};
use acoustilab::analysis::rng::{SplitMix64, Xoshiro256};
use acoustilab::analysis::sensitivity::{self, Method, Scheme, SensitivityOptions};
use acoustilab::analysis::tornado::{self, Metric, TornadoOptions};
use acoustilab::analysis::Design;
use acoustilab::expr::PValue;
use acoustilab::params::Overrides;
use acoustilab::{Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::{LN_10, PI};

const TEMPLATE: &str = include_str!("../../../examples/design_over_ear.json");
const FIXTURE: &str = include_str!("data/analysis_reference.json");

/// dB per percent of a unit change of ln p: 20/ln 10/100.
const DB_PER_PCT: f64 = 20.0 / LN_10 / 100.0;
/// ρc² of the spec_reference air.
const RHO_C2: f64 = 1.204 * 343.0 * 343.0;

fn design(doc: &Value) -> Design {
    Design::parse(&doc.to_string(), &Overrides::new()).unwrap()
}

fn design_with(doc: &Value, ov: &[(&str, PValue)]) -> Design {
    let o: Overrides = ov.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    Design::parse(&doc.to_string(), &o).unwrap()
}

fn num(x: f64) -> PValue {
    PValue::Num(x)
}

fn close(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    (a - b).abs() <= rel * b.abs() + abs
}

fn parse(s: &Value) -> f64 {
    s.as_str().unwrap().parse().unwrap()
}

fn log_grid(f0: f64, f1: f64, ppo: f64, extra: &[f64]) -> Vec<f64> {
    let n = ((f1 / f0).log2() * ppo).ceil() as usize;
    let mut v: Vec<f64> = (0..=n)
        .map(|i| f0 * (f1 / f0).powf(i as f64 / n as f64))
        .collect();
    v.extend_from_slice(extra);
    v.sort_by(f64::total_cmp);
    v.dedup();
    v
}

// ----- Networks with closed forms --------------------------------------------

/// Series R into a shunt C, 1 V; corner 1 kHz at the defaults.
fn rc(freqs: &[f64], r_bounds: (f64, f64)) -> Value {
    json!({
        "parameters": {
            "R_ohm": {"value": 1000, "min": r_bounds.0, "max": r_bounds.1,
                      "tolerance": {"rel": 0.05}, "label": "Series resistance"},
            "C_uF": {"value": 0.159_154_943_091_895_35, "min": 1e-4, "max": 100}
        },
        "sweep": {"frequencies_Hz": freqs},
        "nodes": [{"id": "in", "domain": "electrical"}, {"id": "c", "domain": "electrical"}],
        "elements": [
            {"id": "src", "type": "vsource", "node": "in", "V_V": 1},
            {"id": "r", "type": "resistor", "nodes": ["in", "c"], "R_ohm": "=R_ohm"},
            {"id": "cap", "type": "capacitor", "node": "c", "C_F": "=C_uF * 1e-6"}
        ],
        "probes": [
            {"id": "vc", "quantity": "voltage", "node": "c"},
            {"id": "zin", "quantity": "impedance", "element": "src"}
        ]
    })
}

/// A D0 driver (physical parameter set) on a lossless lumped cavity, rear
/// at ambient, 1 V; spec_reference air.
fn chamber(freqs: &[f64], drive: Option<Value>) -> Value {
    let mut d = json!({
        "parameters": {
            "Bl_Tm": {"value": 2.0, "min": 0.1, "max": 20},
            "Sd_cm2": {"value": 10, "min": 1, "max": 50},
            "Mms_g": {"value": 0.3, "min": 0.01, "max": 10, "tolerance": {"rel": 0.05}},
            "Kms_N_per_m": {"value": 1000, "min": 1, "max": 1e6},
            "Rms_Ns_per_m": {"value": 0.05, "min": 1e-4, "max": 10},
            "Re_ohm": {"value": 32, "min": 1, "max": 1000},
            "V_cm3": {"value": 30, "min": 1, "max": 1000, "label": "Front volume"},
            "P_mW": {"value": 1, "min": 0.001, "max": 1000}
        },
        "air": {"preset": "spec_reference"},
        "level": 0,
        "sweep": {"frequencies_Hz": freqs},
        "nodes": [{"id": "e", "domain": "electrical"}, {"id": "af", "domain": "acoustic"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e", "V_V": 1},
            {"id": "drv", "type": "driver", "nodes": ["e", "gnd", "af"], "model": "D0",
             "Re_ohm": "=Re_ohm", "Bl_Tm": "=Bl_Tm", "Mms_g": "=Mms_g",
             "Kms_N_per_m": "=Kms_N_per_m", "Rms_Ns_per_m": "=Rms_Ns_per_m", "Sd_cm2": "=Sd_cm2"},
            {"id": "front", "type": "cavity", "node": "af", "volume_cm3": "=V_cm3", "wall_loss": false}
        ],
        "probes": [
            {"id": "p", "quantity": "pressure", "node": "af"},
            {"id": "zin", "quantity": "impedance", "element": "amp"}
        ]
    });
    if let Some(dr) = drive {
        d["drive"] = dr;
    }
    d
}

struct ChamberForm {
    zm: C64,
    ze: C64,
    zb: C64,
    ztot: C64,
    jw: C64,
}

/// A closed-form log-derivative of the chamber's pressure.
type LogDerivative = fn(&ChamberForm) -> C64;

/// Mechanical impedances of [`chamber`] at its defaults and frequency f.
fn chamber_form(f: f64, mms: f64, kms: f64, sd: f64, v: f64) -> ChamberForm {
    let (bl, rms, re) = (2.0, 0.05, 32.0);
    let jw = C64::new(0.0, 2.0 * PI * f);
    let zm = jw * mms + rms + kms / jw;
    let ze = C64::new(bl * bl / re, 0.0);
    let zb = sd * sd * RHO_C2 / (jw * v);
    ChamberForm {
        zm,
        ze,
        zb,
        ztot: zm + ze + zb,
        jw,
    }
}

/// Both ways of evaluating the stepped designs; every closed-form test
/// runs with each.
const METHODS: [Method; 2] = [Method::CompleteSolves, Method::ForwardSensitivity];

fn with_method(method: Method) -> SensitivityOptions {
    SensitivityOptions {
        method: Some(method),
        ..Default::default()
    }
}

fn check_sens(got_db: &[f64], got_deg: &[f64], dlnp: &[C64], what: &str) {
    for (k, d) in dlnp.iter().enumerate() {
        let db = DB_PER_PCT * d.re;
        let deg = d.im.to_degrees() / 100.0;
        assert!(
            close(got_db[k], db, 1e-7, 1e-10),
            "{what} dB[{k}]: {} vs {db}",
            got_db[k]
        );
        assert!(
            close(got_deg[k], deg, 1e-7, 1e-10),
            "{what} deg[{k}]: {} vs {deg}",
            got_deg[k]
        );
    }
}

// ----- Sensitivities --------------------------------------------------------

#[test]
fn sensitivity_of_a_series_rc_matches_the_closed_form() {
    let freqs = log_grid(10.0, 100_000.0, 6.0, &[500.0, 1000.0]);
    for method in METHODS {
        rc_case(&freqs, method);
    }
}

fn rc_case(freqs: &[f64], method: Method) {
    let j = sensitivity::jacobian(&design(&rc(freqs, (1.0, 1e6))), &with_method(method)).unwrap();
    assert_eq!(j.probes, ["vc", "zin"]);
    for name in ["R_ohm", "C_uF"] {
        let p = j.parameter(name).unwrap();
        assert_eq!(p.scheme, Scheme::Central);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        // H = 1/(1 + jx), x = ωRC: d ln H/d ln R = d ln H/d ln C = −jx/(1 + jx).
        let dh: Vec<C64> = freqs
            .iter()
            .map(|f| {
                let x = C64::new(0.0, f / 1000.0);
                -x / (1.0 + x)
            })
            .collect();
        check_sens(&p.db_per_pct[0], &p.deg_per_pct[0], &dh, name);
        // Z = R + 1/(jωC): d ln Z/d ln R = R/Z, d ln Z/d ln C = −(1/jωC)/Z.
        let dz: Vec<C64> = freqs
            .iter()
            .map(|f| {
                let zc = 1.0 / C64::new(0.0, f / 1000.0 / 1000.0);
                let z = 1000.0 + zc;
                if name == "R_ohm" {
                    1000.0 / z
                } else {
                    -zc / z
                }
            })
            .collect();
        check_sens(&p.db_per_pct[1], &p.deg_per_pct[1], &dz, name);
    }
    // H depends on the product RC only: the two rows agree (symmetry), to
    // rounding with complete solves, and to the O(h²) terms of the update,
    // which differ between a resistor and a capacitor, with forward
    // sensitivities.
    let (r, c) = (j.db("R_ohm", "vc").unwrap(), j.db("C_uF", "vc").unwrap());
    let tol = match method {
        Method::CompleteSolves => 1e-11,
        Method::ForwardSensitivity => 1e-9,
    };
    for (a, b) in r.iter().zip(c) {
        assert!((a - b).abs() < tol, "{a} vs {b}");
    }
    let hm = j.heat_map("vc").unwrap();
    assert_eq!(hm.parameters, ["R_ohm", "C_uF"]);
    assert_eq!(hm.db_per_pct[0], r);
    assert!(j.heat_map("nope").is_err());
}

#[test]
fn central_difference_error_is_the_predicted_truncation() {
    // At x = ωRC = 0.5 the level g(u) = −10·log10(1 + x²), u = ln R, has
    // g' = −(20/ln 10)·s and g''' = −(20/ln 10)·4s(1 − s)(1 − 2s), s = x²/(1 + x²).
    // The central difference errs by h²/6·g''' (+ O(h⁴)) plus rounding ε/h.
    let freqs = [100.0, 500.0, 2000.0];
    let d = design(&rc(&freqs, (1.0, 1e6)));
    let s: f64 = 0.2;
    let g1 = -20.0 / LN_10 * s;
    let g3 = -20.0 / LN_10 * 4.0 * s * (1.0 - s) * (1.0 - 2.0 * s);
    for h in [1e-3, 1e-4] {
        let opts = SensitivityOptions {
            parameters: Some(vec!["R_ohm".into()]),
            probes: Some(vec!["vc".into()]),
            step: Some(h),
            ..Default::default()
        };
        let j = sensitivity::jacobian(&d, &opts).unwrap();
        let err = j.db("R_ohm", "vc").unwrap()[1] * 100.0 - g1;
        let predicted = h * h / 6.0 * g3;
        assert!(
            (err - predicted).abs() < 0.05 * predicted.abs(),
            "h = {h}: error {err:e}, predicted {predicted:e}"
        );
        // Relative error far below the 1e-6 target.
        assert!((err / g1).abs() < 5e-7 * (h / 1e-4).powi(2));
    }
}

#[test]
fn sealed_cavity_pressure_falls_exactly_one_per_cent_per_per_cent_of_volume() {
    // Constant volume velocity into a lossless lumped cavity: p = U/(jωV/ρc²),
    // so d(dB)/d(ln V) = −20/ln 10 at every frequency and the phase is fixed.
    let doc = json!({
        "parameters": {"V_cm3": {"value": 30, "min": 1, "max": 1000}},
        "air": {"preset": "spec_reference"}, "level": 0,
        "sweep": {"f_min_Hz": 10, "f_max_Hz": 10000, "points_per_octave": 3},
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [
            {"id": "q", "type": "flow_source", "node": "a", "U_m3_per_s": 1e-6},
            {"id": "v", "type": "cavity", "node": "a", "volume_cm3": "=V_cm3", "wall_loss": false}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "a"}]
    });
    let j = sensitivity::jacobian(&design(&doc), &Default::default()).unwrap();
    let p = j.parameter("V_cm3").unwrap();
    for (db, deg) in p.db_per_pct[0].iter().zip(&p.deg_per_pct[0]) {
        assert!((db + DB_PER_PCT).abs() < 1e-10, "{db}");
        assert!(deg.abs() < 1e-10, "{deg}");
    }
    assert!((DB_PER_PCT - 0.086_858_896_380_650_36).abs() < 1e-15);
}

#[test]
fn pressure_chamber_sensitivities_match_the_lumped_closed_form() {
    // p = (Sd·ρc²/(jωV))·(Bl·E/Re)/Z, Z = Zm + Bl²/Re + Sd²ρc²/(jωV),
    // Zm = jωMms + Rms + Kms/(jω) (spec Appendix C1 and C2 without the
    // low-frequency approximation). Log-derivatives:
    //   Bl: 1 − 2Ze/Z   Sd: 1 − 2Zb/Z   Mms: −jωMms/Z   V: −1 + Zb/Z
    //   Re: −1 + Ze/Z   Kms: −(Kms/jω)/Z   Rms: −Rms/Z
    // and for Zin = Re + Bl²/(Zm + Zb): Bl: 2(Zin − Re)/Zin, Re: Re/Zin.
    let freqs = log_grid(20.0, 5000.0, 6.0, &[]);
    for method in METHODS {
        chamber_case(&freqs, method);
    }
}

fn chamber_case(freqs: &[f64], method: Method) {
    let j = sensitivity::jacobian(&design(&chamber(freqs, None)), &with_method(method)).unwrap();
    let forms: Vec<ChamberForm> = freqs
        .iter()
        .map(|&f| chamber_form(f, 3e-4, 1000.0, 1e-3, 30e-6))
        .collect();
    let cases: [(&str, LogDerivative); 7] = [
        ("Bl_Tm", |c| 1.0 - 2.0 * c.ze / c.ztot),
        ("Sd_cm2", |c| 1.0 - 2.0 * c.zb / c.ztot),
        ("Mms_g", |c| -(c.jw * 3e-4) / c.ztot),
        ("V_cm3", |c| -1.0 + c.zb / c.ztot),
        ("Re_ohm", |c| -1.0 + c.ze / c.ztot),
        ("Kms_N_per_m", |c| -(1000.0 / c.jw) / c.ztot),
        ("Rms_Ns_per_m", |c| -0.05 / c.ztot),
    ];
    for (name, f) in cases {
        let p = j.parameter(name).unwrap();
        let d: Vec<C64> = forms.iter().map(f).collect();
        check_sens(&p.db_per_pct[0], &p.deg_per_pct[0], &d, name);
    }
    let zin = |c: &ChamberForm| 32.0 + 4.0 / (c.zm + c.zb);
    let dbl: Vec<C64> = forms
        .iter()
        .map(|c| 2.0 * (zin(c) - 32.0) / zin(c))
        .collect();
    let p = j.parameter("Bl_Tm").unwrap();
    check_sens(&p.db_per_pct[1], &p.deg_per_pct[1], &dbl, "zin/Bl");
    let dre: Vec<C64> = forms.iter().map(|c| 32.0 / zin(c)).collect();
    let p = j.parameter("Re_ohm").unwrap();
    check_sens(&p.db_per_pct[1], &p.deg_per_pct[1], &dre, "zin/Re");
    // The stiffness-controlled limit (spec C1): far below the coupled
    // resonance, with the box much stiffer than the suspension, p ∝ 1/Sd.
    let k20 = forms[0].zb / forms[0].ztot;
    assert!(
        (k20.re - 1.0).abs() < 0.2,
        "the box dominates at 20 Hz: {k20}"
    );
}

#[test]
fn characteristic_drive_sensitivity_is_renormalised_at_500_hz() {
    // Under a characteristic drive the level at 500 Hz at the reference
    // probe is held at 94 dB by a real factor, so every level derivative is
    // the voltage-drive derivative minus the reference probe's at 500 Hz;
    // phases and impedances are unchanged. Under a power drive, P_mW scales the voltage by
    // √P: +10/ln 10 per unit of ln P for every level.
    for method in METHODS {
        drive_case(method);
    }
}

fn drive_case(method: Method) {
    let freqs = [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0];
    let volt =
        sensitivity::jacobian(&design(&chamber(&freqs, None)), &with_method(method)).unwrap();
    let chr = sensitivity::jacobian(
        &design(&chamber(&freqs, Some(json!({"characteristic": "p"})))),
        &with_method(method),
    )
    .unwrap();
    assert_eq!(chr.drive.convention, "characteristic");
    let k500 = 4;
    for pv in &volt.parameters {
        let pc = chr.parameter(&pv.name).unwrap();
        let ref_db = pv.db_per_pct[0][k500];
        for k in 0..freqs.len() {
            let a = pc.db_per_pct[0][k];
            assert!(
                (a - (pv.db_per_pct[0][k] - ref_db)).abs() < 1e-9,
                "{} {k}",
                pv.name
            );
            // The characteristic voltage is a real factor: phases do not move.
            let b = pc.deg_per_pct[0][k];
            assert!((b - pv.deg_per_pct[0][k]).abs() < 1e-9);
            assert!((pc.db_per_pct[1][k] - pv.db_per_pct[1][k]).abs() < 1e-10);
        }
        assert!(pc.db_per_pct[0][k500].abs() < 1e-9);
    }
    // P_mW has no effect under a voltage drive (it is unused).
    assert!(volt.parameter("P_mW").unwrap().db_per_pct[0]
        .iter()
        .all(|x| *x == 0.0));
    let pow = sensitivity::jacobian(
        &design(&chamber(
            &freqs,
            Some(json!({"power_mW": "=P_mW", "rated_ohm": 32})),
        )),
        &SensitivityOptions {
            parameters: Some(vec!["P_mW".into()]),
            ..with_method(method)
        },
    )
    .unwrap();
    let p = &pow.parameters[0];
    for k in 0..freqs.len() {
        assert!((p.db_per_pct[0][k] - DB_PER_PCT / 2.0).abs() < 1e-10);
        assert!(p.deg_per_pct[0][k].abs() < 1e-10);
        assert!(p.db_per_pct[1][k].abs() < 1e-10, "impedances are ratios");
    }
}

#[test]
fn forward_sensitivities_agree_with_complete_solves() {
    // The template under every drive convention (the forward method applies
    // the drive factor itself), at L1 and L0. The two methods have
    // different O(h²) error terms, so they agree to about their truncation
    // error: 1e-6 of each parameter's largest sensitivity in the credible
    // band, 1e-5 next to the lightly damped depth resonance in the shaded
    // band (measured: 3e-6; docs/analysis.md).
    let base: Value = serde_json::from_str(TEMPLATE).unwrap();
    let drives = [
        Some(json!({"power_mW": "=drive_mW", "rated_ohm": "=rated_impedance_ohm"})),
        Some(json!({"voltage_V": 0.5})),
        Some(json!({"characteristic": "p_drp"})),
        Some(json!({"current_mA": 5})),
        None,
    ];
    let params: Vec<String> = [
        "driver_Mms_g",
        "driver_Sd_cm2",
        "front_depth_mm",
        "leak_gap_mm",
        "rear_volume_cm3",
        "vent_mesh_rayl",
        "drive_mW",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (k, drive) in drives.iter().enumerate() {
        let mut doc = base.clone();
        match drive {
            Some(d) => doc["drive"] = d.clone(),
            None => {
                doc.as_object_mut().unwrap().remove("drive");
            }
        }
        let fidelity = if k == 0 { vec![1.0, 0.0] } else { vec![1.0] };
        for level in fidelity {
            // A coarser grid (8 per octave) keeps the test quick in debug builds.
            let d = design_with(
                &doc,
                &[("fidelity", num(level)), ("points_per_octave", num(8.0))],
            );
            let opts = |m: Method| SensitivityOptions {
                parameters: Some(params.clone()),
                ..with_method(m)
            };
            let a = sensitivity::jacobian(&d, &opts(Method::CompleteSolves)).unwrap();
            let b = sensitivity::jacobian(&d, &opts(Method::ForwardSensitivity)).unwrap();
            assert_eq!(a.parameters.len(), b.parameters.len());
            for (pa, pb) in a.parameters.iter().zip(&b.parameters) {
                for (x, y) in [
                    (&pa.db_per_pct, &pb.db_per_pct),
                    (&pa.deg_per_pct, &pb.deg_per_pct),
                ] {
                    let scale = x
                        .iter()
                        .flatten()
                        .filter(|v| v.is_finite())
                        .fold(0.0f64, |m, v| m.max(v.abs()));
                    for (row_u, row_v) in x.iter().zip(y) {
                        for (k, (u, v)) in row_u.iter().zip(row_v).enumerate() {
                            let credible = a.shading.band(a.freqs_hz[k]) == 0;
                            let rel = if credible { 1e-6 } else { 1e-5 };
                            assert!(
                                // 1e-8 dB or degree per % is rounding noise where a
                                // derivative vanishes (the phase of a pure level change).
                                (u - v).abs() <= rel * scale + 1e-8,
                                "{} under {drive:?}, L{level}, {} Hz: {u} vs {v}",
                                pa.name,
                                a.freqs_hz[k]
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_changed_number_of_unknowns_is_not_differentiated() {
    // The leak's segment count comes from round(s): at s = 4.5 the step
    // down gives 4 segments instead of 5, so at level 1 the matrix loses two
    // branch unknowns while the structure (numbers aside) stays the same.
    let doc = json!({
        "parameters": {"s": {"value": 4.5, "min": 1, "max": 10}, "V_cm3": {"value": 30, "min": 1, "max": 100}},
        "air": {"preset": "spec_reference"}, "level": 1,
        "sweep": {"f_min_Hz": 20, "f_max_Hz": 2000, "points_per_octave": 1},
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [
            {"id": "q", "type": "flow_source", "node": "a", "U_m3_per_s": 1e-6},
            {"id": "v", "type": "cavity", "node": "a", "volume_cm3": "=V_cm3"},
            {"id": "leak", "type": "leak", "nodes": ["a", "ambient"], "perimeter_mm": 100,
             "depth_mm": 10, "gap_mm": 0.1, "segments": "=round(s)"}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "a"}]
    });
    let j = sensitivity::jacobian(&design(&doc), &Default::default()).unwrap();
    let e = &j.excluded[0];
    assert_eq!(e.name, "s");
    assert!(e.reason.contains("number of unknowns"), "{}", e.reason);
    assert!(j.parameter("V_cm3").is_some());
}

#[test]
fn one_sided_differences_at_bounds() {
    let freqs = log_grid(10.0, 100_000.0, 3.0, &[]);
    let exact: Vec<f64> = freqs
        .iter()
        .map(|f| {
            let x2 = (f / 1000.0) * (f / 1000.0);
            -DB_PER_PCT * x2 / (1.0 + x2)
        })
        .collect();
    let cases = [
        ((1.0, 1000.0), Scheme::Backward),
        ((1000.0, 1e6), Scheme::Forward),
    ];
    for ((bounds, scheme), method) in cases.iter().flat_map(|c| METHODS.map(|m| (*c, m))) {
        let j = sensitivity::jacobian(
            &design(&rc(&freqs, bounds)),
            &SensitivityOptions {
                parameters: Some(vec!["R_ohm".into()]),
                ..with_method(method)
            },
        )
        .unwrap();
        let p = &j.parameters[0];
        assert_eq!(p.scheme, scheme);
        assert!(p.note.as_deref().unwrap().contains("one-sided"));
        for (a, b) in p.db_per_pct[0].iter().zip(&exact) {
            // Second-order one-sided: error h²/3·|g'''| ≤ 1e-8 relative here.
            assert!(close(*a, *b, 1e-7, 1e-10), "{a} vs {b}");
        }
    }
}

#[test]
fn topology_changes_and_non_continuous_parameters_are_excluded() {
    let mut doc = rc(&[100.0, 1000.0, 10_000.0], (1.0, 1e6));
    doc["parameters"]["x"] = json!({"value": 1.0, "min": 0.5, "max": 2});
    doc["parameters"]["n"] = json!({"value": 2, "min": 0, "max": 4, "integer": true});
    doc["parameters"]["sw"] = json!(true);
    doc["parameters"]["ch"] = json!({"value": "a", "choices": ["a", "b"]});
    doc["parameters"]["z_ohm"] = json!(0.0);
    doc["parameters"]["k"] = json!({"value": 2, "min": 1, "max": 3});
    doc["parameters"]["fmax_Hz"] = json!({"value": 20000, "min": 1000, "max": 40000});
    doc["parameters"]["area"] = json!({"expr": "x * 2"});
    doc["sweep"] = json!({"f_min_Hz": 100, "f_max_Hz": "=fmax_Hz", "points_per_octave": 1});
    // An element that exists only for x > 1, and a kink at k = 2.
    doc["elements"].as_array_mut().unwrap().push(
        json!({"id": "r2", "type": "resistor", "node": "c", "R_ohm": 1e5, "enabled": "=x > 1"}),
    );
    doc["elements"][1]["R_ohm"] = json!("=R_ohm * min(k, 2) / 2");
    let j = sensitivity::jacobian(&design(&doc), &Default::default()).unwrap();
    let reason = |n: &str| {
        j.excluded
            .iter()
            .find(|e| e.name == n)
            .unwrap_or_else(|| panic!("{n} not excluded"))
            .reason
            .clone()
    };
    assert!(reason("x").contains("not differentiable") && reason("x").contains("'r2'"));
    assert!(reason("n").contains("integer"));
    assert!(reason("sw").contains("boolean"));
    assert!(reason("ch").contains("choice"));
    assert!(reason("z_ohm").contains("value is 0"));
    assert!(reason("fmax_Hz").contains("frequency grid"));
    assert!(
        j.excluded.iter().all(|e| e.name != "area"),
        "derived are not listed by default"
    );
    let k = j
        .parameter("k")
        .expect("k is differentiable, with a warning");
    assert!(
        k.warnings.iter().any(|w| w.contains("kink or jump")),
        "{:?}",
        k.warnings
    );
    assert!(j.parameter("R_ohm").unwrap().warnings.is_empty());
    // Forward sensitivities update each side separately, so they see the
    // kink and the same exclusions.
    let jf =
        sensitivity::jacobian(&design(&doc), &with_method(Method::ForwardSensitivity)).unwrap();
    let names = |j: &sensitivity::Jacobian| -> Vec<String> {
        j.excluded.iter().map(|e| e.name.clone()).collect()
    };
    assert_eq!(names(&jf), names(&j));
    assert!(jf
        .parameter("k")
        .unwrap()
        .warnings
        .iter()
        .any(|w| w.contains("kink or jump")));
    // Naming a derived parameter explicitly reports why it is excluded.
    let j = sensitivity::jacobian(
        &design(&doc),
        &SensitivityOptions {
            parameters: Some(vec!["area".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(j.excluded[0].reason.contains("derived"));
    assert!(sensitivity::jacobian(
        &design(&doc),
        &SensitivityOptions {
            parameters: Some(vec!["nope".into()]),
            ..Default::default()
        }
    )
    .is_err());
    assert!(sensitivity::jacobian(
        &design(&doc),
        &SensitivityOptions {
            step: Some(0.5),
            ..Default::default()
        }
    )
    .is_err());
    // A name listed twice is an error, not twice the work.
    let twice = |p: &[&str], q: &[&str]| SensitivityOptions {
        parameters: Some(p.iter().map(|s| s.to_string()).collect()),
        probes: Some(q.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    let e = sensitivity::jacobian(&design(&doc), &twice(&["R_ohm", "R_ohm"], &["vc"]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("listed twice"), "{e}");
    let e = sensitivity::jacobian(&design(&doc), &twice(&["R_ohm"], &["vc", "zin", "vc"]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("listed twice"), "{e}");
}

// ----- Readouts -----------------------------------------------------------------

#[test]
fn free_air_driver_readouts_recover_the_primary_set() {
    // A D0 driver with both acoustic ports at ambient: its input impedance
    // is Re + Bl²/Zm, whose sqrt(r0) readouts are exact (ts::extract_ts).
    // The grid (12 per octave) is coarse on purpose: every value is refined
    // by exact solves.
    let doc = json!({
        "sweep": {"f_min_Hz": 1, "f_max_Hz": 20000, "points_per_octave": 12},
        "nodes": [{"id": "e", "domain": "electrical"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e"},
            {"id": "drv", "type": "driver", "nodes": ["e", "gnd", "ambient"], "model": "D0",
             "fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8, "Mms_g": 0.3, "Sd_cm2": 10}
        ],
        "probes": []
    });
    let d = design(&doc);
    for (re, tol) in [(Some(32.8), 1e-7), (None, 1e-6)] {
        let r = readouts::readouts(
            &d,
            &ReadoutOptions {
                re_ohm: re,
                ..Default::default()
            },
        )
        .unwrap();
        let z = r.impedance.as_ref().unwrap();
        assert!(close(z.re, 32.8, tol / 10.0, 0.0), "Re {}", z.re);
        let q = z.q.as_ref().unwrap();
        assert!(close(q.fres, 81.8, 1e-7, 0.0), "fs {}", q.fres);
        assert!(close(q.qms, 2.71, tol, 0.0), "Qms {}", q.qms);
        assert!(close(q.qes, 1.01, tol, 0.0), "Qes {}", q.qes);
        assert!(close(q.qts, 2.71 * 1.01 / 3.72, tol, 0.0));
        // f1·f2 = fs² exactly for a single lumped resonance.
        assert!(close((q.f1 * q.f2).sqrt(), 81.8, 1e-7, 0.0));
        assert!(q.notes.is_empty());
        assert_eq!(z.peaks.len(), 1);
        assert!(r.response.is_none(), "no pressure probe");
    }
    let r = readouts::readouts(&d, &Default::default()).unwrap();
    let drv = &r.drivers[0];
    assert!(close(drv.d0.fs, 81.8, 1e-12, 0.0) && close(drv.d0.qes, 1.01, 1e-12, 0.0));
    let fa = drv.free_air.as_ref().unwrap();
    assert!(close(fa.fres, 81.8, 1e-7, 0.0));
    assert!(close(fa.qms, 2.71, 1e-7, 0.0) && close(fa.qes, 1.01, 1e-7, 0.0));
    // |Z(1 kHz)| by an exact solve: Re + (Re/Qes)/(1/Qms + j(w − 1/w)).
    let w = 1000.0 / 81.8;
    let zk = 32.8 + (32.8 / 1.01) / C64::new(1.0 / 2.71, w - 1.0 / w);
    assert!(close(
        r.impedance.as_ref().unwrap().z_1khz.unwrap(),
        zk.norm(),
        1e-12,
        0.0
    ));
}

#[test]
fn q_readouts_of_a_sharp_peak_between_grid_points() {
    // Qms = 100, Qes = 5: r0 = 21 and |Z| exceeds Re·√r0 only over
    // fs·√r0/Qms = 3.7 Hz. With fs midway (geometrically) between two points
    // of a 12-per-octave grid, 2.9 % either side, no grid point lies above
    // that level: the crossings are bracketed from the refined peak.
    let grid = acoustilab::grid::log_grid(1.0, 20_000.0, 12.0);
    let k = grid.iter().position(|f| *f > 80.0).unwrap();
    let fs = (grid[k - 1] * grid[k]).sqrt();
    let doc = json!({
        "sweep": {"f_min_Hz": 1, "f_max_Hz": 20000, "points_per_octave": 12},
        "nodes": [{"id": "e", "domain": "electrical"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e"},
            {"id": "drv", "type": "driver", "nodes": ["e", "gnd", "ambient"], "model": "D0",
             "fs_Hz": fs, "Qms": 100, "Qes": 5, "Re_ohm": 6, "Mms_g": 0.3, "Sd_cm2": 10}
        ],
        "probes": []
    });
    assert_eq!(Circuit::from_json(&doc.to_string()).unwrap().freqs, grid);
    // |Z| = |Re + (Re/Qes)/(1/Qms + j(w − 1/w))|, w = f/fs.
    let z = |f: f64| {
        let w = f / fs;
        (6.0 + 1.2 / C64::new(0.01, w - 1.0 / w)).norm()
    };
    let level = 6.0 * 21f64.sqrt();
    assert!(grid.iter().all(|f| z(*f) < level));
    let r = readouts::readouts(
        &design(&doc),
        &ReadoutOptions {
            re_ohm: Some(6.0),
            ..Default::default()
        },
    )
    .unwrap();
    let q = r.impedance.unwrap().q.unwrap();
    assert!(close(q.fres, fs, 1e-7, 0.0) && close(q.r0, 21.0, 1e-9, 0.0));
    assert!(close(q.qms, 100.0, 1e-6, 0.0), "Qms {}", q.qms);
    assert!(close(q.qes, 5.0, 1e-6, 0.0), "Qes {}", q.qes);
    assert!(q.f1 < q.fres && q.fres < q.f2);
}

#[test]
fn coupled_resonance_matches_appendix_c2() {
    // Spec C2: 10 cm², 30 cm³, 1 mm/N, 0.1 g: f_c = (1/2π)·√((1/Cms +
    // Sd²ρc²/V)/Mms) = 1204 Hz. |v/i| = Bl/|Zm + Zb| peaks exactly there,
    // and a lossless cavity moves the impedance peak to f_c without changing
    // its height Re + Bl²/Rms (spec C5).
    let freqs = log_grid(20.0, 20_000.0, 12.0, &[]);
    let d = design_with(&chamber(&freqs, None), &[("Mms_g", num(0.1))]);
    let r = readouts::readouts(&d, &Default::default()).unwrap();
    let kbox = 1e-6 * RHO_C2 / 30e-6;
    let fc = ((1000.0 + kbox) / 1e-4).sqrt() / (2.0 * PI);
    assert!((fc - 1204.0).abs() < 0.5, "{fc}");
    let c = r
        .response
        .as_ref()
        .unwrap()
        .coupled_resonance
        .as_ref()
        .unwrap();
    assert!(close(c.f_hz, fc, 1e-7, 0.0), "{} vs {fc}", c.f_hz);
    assert!(c.robust && c.prominence_db > 10.0);
    assert!(c.competing.is_none() && !c.ambiguous);
    let z = r.impedance.as_ref().unwrap();
    let res = z.resonance.unwrap();
    assert!(close(res.f_hz, fc, 1e-7, 0.0));
    assert!(close(res.z, 32.0 + 4.0 / 0.05, 1e-9, 0.0), "{}", res.z);
    // The response probe defaults to the first pressure probe.
    assert_eq!(
        r.response.as_ref().unwrap().probe_source,
        "first pressure probe"
    );
}

#[test]
fn no_q_estimate_across_an_overlapping_resonance() {
    // Re = 32 ohm in series with two parallel RLC tanks, Z = Re + Σ
    // Rk/(1 + jQk(f/fk − fk/f)): (4 ohm, 100 Hz, Q 2) and (40 ohm, 300 Hz,
    // Q 3). The first peak (r0 ≈ 1.15) is separated from the second by a
    // valley that stays above Re·√r0, so |Z| first falls to that level
    // beyond the second peak: a bandwidth there would span both
    // resonances. The sqrt(r0) method does not apply and no Q is given.
    let tank = |r: f64, f0: f64, q: f64| {
        let w0 = 2.0 * PI * f0;
        (r / (q * w0), q / (r * w0))
    };
    let ((l1, c1), (l2, c2)) = (tank(4.0, 100.0, 2.0), tank(40.0, 300.0, 3.0));
    let doc = json!({
        "sweep": {"f_min_Hz": 10, "f_max_Hz": 3000, "points_per_octave": 24},
        "nodes": [{"id": "e", "domain": "electrical"}, {"id": "n1", "domain": "electrical"},
                  {"id": "n2", "domain": "electrical"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e"},
            {"id": "re", "type": "resistor", "nodes": ["e", "n1"], "R_ohm": 32},
            {"id": "r1", "type": "resistor", "nodes": ["n1", "n2"], "R_ohm": 4},
            {"id": "l1", "type": "inductor", "nodes": ["n1", "n2"], "L_H": l1},
            {"id": "c1", "type": "capacitor", "nodes": ["n1", "n2"], "C_F": c1},
            {"id": "r2", "type": "resistor", "node": "n2", "R_ohm": 40},
            {"id": "l2", "type": "inductor", "node": "n2", "L_H": l2},
            {"id": "c2", "type": "capacitor", "node": "n2", "C_F": c2}
        ],
        "probes": []
    });
    let z = |f: f64| {
        let t = |r: f64, f0: f64, q: f64| r / C64::new(1.0, q * (f / f0 - f0 / f));
        (32.0 + t(4.0, 100.0, 2.0) + t(40.0, 300.0, 3.0)).norm()
    };
    let r = readouts::readouts(
        &design(&doc),
        &ReadoutOptions {
            re_ohm: Some(32.0),
            ..Default::default()
        },
    )
    .unwrap();
    let imp = r.impedance.unwrap();
    assert_eq!(imp.peaks.len(), 2, "{:?}", imp.peaks);
    let (p1, p2) = (imp.peaks[0], imp.peaks[1]);
    assert!(close(p1.z, z(p1.f_hz), 1e-12, 0.0));
    // The premise, from the closed form: the valley between the peaks
    // stays above Re·√r0 of the first.
    let level = (32.0 * p1.z).sqrt();
    let valley = (0..=10_000)
        .map(|k| z(p1.f_hz * (p2.f_hz / p1.f_hz).powf(k as f64 / 10_000.0)))
        .fold(f64::INFINITY, f64::min);
    assert!(valley > level * 1.02, "valley {valley}, level {level}");
    assert!(imp.q.is_none(), "{:?}", imp.q);
    assert!(
        imp.notes.iter().any(|n| n.contains("resonances overlap")),
        "{:?}",
        imp.notes
    );
    assert!(r.drivers.is_empty());
}

/// A D0 driver loaded at the front by a vented box (compliance Cb in
/// parallel with a port of inertance Mp and resistance Rp), rear at
/// ambient: two coupled resonances, below and above the port resonance.
fn vented_box(fb: f64, rp: f64) -> (Value, impl Fn(f64) -> f64) {
    let cb = 30e-6 / RHO_C2;
    let mp = 1.0 / ((2.0 * PI * fb).powi(2) * cb);
    let doc = json!({
        "sweep": {"f_min_Hz": 20, "f_max_Hz": 5000, "points_per_octave": 12},
        "nodes": [{"id": "e", "domain": "electrical"}, {"id": "af", "domain": "acoustic"},
                  {"id": "ap", "domain": "acoustic"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e", "V_V": 1},
            {"id": "drv", "type": "driver", "nodes": ["e", "gnd", "af"], "model": "D0",
             "Re_ohm": 32, "Bl_Tm": 2, "Mms_g": 0.3, "Kms_N_per_m": 1000,
             "Rms_Ns_per_m": 0.05, "Sd_cm2": 10},
            {"id": "box", "type": "acoustic_compliance", "node": "af", "C_m3_per_Pa": cb},
            {"id": "port", "type": "acoustic_inertance", "nodes": ["af", "ap"], "M_kg_per_m4": mp},
            {"id": "loss", "type": "acoustic_resistance", "node": "ap", "R_Pa_s_per_m3": rp}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "af"}]
    });
    // |v/i| = Bl/|Zm + Sd²·Za|, Za = Zc·Zp/(Zc + Zp).
    let vi = move |f: f64| {
        let jw = C64::new(0.0, 2.0 * PI * f);
        let (zc, zp) = (1.0 / (jw * cb), jw * mp + rp);
        let zm = jw * 3e-4 + 0.05 + 1000.0 / jw + 1e-6 * zc * zp / (zc + zp);
        2.0 / zm.norm()
    };
    (doc, vi)
}

/// Maximum of `g` on [a, b] by golden-section search (independent of the
/// engine's Brent search), to 1e-12 in ln f.
fn golden_max(g: &dyn Fn(f64) -> f64, a: f64, b: f64) -> (f64, f64) {
    let r = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (a.ln(), b.ln());
    while b - a > 1e-12 {
        let (x1, x2) = (b - r * (b - a), a + r * (b - a));
        if g(x1.exp()) > g(x2.exp()) {
            b = x2;
        } else {
            a = x1;
        }
    }
    let x = (0.5 * (a + b)).exp();
    (x, g(x))
}

#[test]
fn coupled_resonance_with_two_near_equal_peaks_is_ambiguous() {
    // Port resonance 700 Hz, Rp = 3e5: |v/i| peaks near 211 and 965 Hz,
    // 0.45 dB apart (closed form below). The maximum is reported with the
    // other peak, flagged ambiguous, not robust, and left out of the
    // scalars, so a tornado or Monte Carlo run never mixes the two.
    let (doc, vi) = vented_box(700.0, 3e5);
    let (fl, vl) = golden_max(&vi, 150.0, 400.0);
    let (fh, vh) = golden_max(&vi, 700.0, 1500.0);
    let margin = 20.0 * (vh / vl).log10();
    assert!(margin > 0.0 && margin < 1.0, "{margin}");
    let d = design(&doc);
    let r = readouts::readouts(&d, &Default::default()).unwrap();
    let resp = r.response.as_ref().unwrap();
    let c = resp.coupled_resonance.as_ref().unwrap();
    assert!(close(c.f_hz, fh, 1e-7, 0.0), "{} vs {fh}", c.f_hz);
    let comp = c.competing.unwrap();
    assert!(close(comp.f_hz, fl, 1e-7, 0.0), "{} vs {fl}", comp.f_hz);
    assert!((comp.margin_db - margin).abs() < 1e-9, "{}", comp.margin_db);
    assert!(c.ambiguous && !c.robust && c.prominence_db > readouts::RESONANCE_PROMINENCE_DB);
    assert!(resp.notes.iter().any(|n| n.contains("ambiguous")));
    assert_eq!(r.scalars()["coupled_resonance_Hz"], None);
    let e = tornado::tornado(
        &d,
        &TornadoOptions {
            metric: Some(Metric::Readout {
                name: "coupled_resonance_Hz".into(),
            }),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("undefined for the base design"), "{e}");
    // Port resonance 500 Hz, Rp = 1e5: the lower peak is 7 dB down, so the
    // resonance is robust and reported, with its competitor.
    let (doc, vi) = vented_box(500.0, 1e5);
    let (fl, vl) = golden_max(&vi, 100.0, 300.0);
    let (fh, vh) = golden_max(&vi, 600.0, 1500.0);
    let r = readouts::readouts(&design(&doc), &Default::default()).unwrap();
    let c = r
        .response
        .as_ref()
        .unwrap()
        .coupled_resonance
        .clone()
        .unwrap();
    assert!(close(c.f_hz, fh, 1e-7, 0.0) && c.robust && !c.ambiguous);
    let comp = c.competing.unwrap();
    assert!(close(comp.f_hz, fl, 1e-7, 0.0));
    assert!((comp.margin_db - 20.0 * (vh / vl).log10()).abs() < 1e-9);
    assert!(comp.margin_db > 6.0);
    assert_eq!(r.scalars()["coupled_resonance_Hz"], Some(c.f_hz));
}

#[test]
fn bass_extension_of_a_first_order_high_pass() {
    // p_a/p_s = jωRC/(1 + jωRC), f_c = 200 Hz. With x = f/f_c the level is
    // 10·log10(x²/(1 + x²)); the −3 dB point relative to 500 Hz solves
    // x_b²/(1 + x_b²) = a, a = |H(500)|²·10^(−0.3).
    let (c, fc) = (1e-10, 200.0);
    let r = 1.0 / (2.0 * PI * fc * c);
    let doc = json!({
        "air": {"preset": "spec_reference"}, "level": 0,
        "sweep": {"f_min_Hz": 10, "f_max_Hz": 20000, "points_per_octave": 6},
        "nodes": [{"id": "s", "domain": "acoustic"}, {"id": "a", "domain": "acoustic"}],
        "elements": [
            {"id": "ps", "type": "pressure_source", "node": "s", "p_Pa": 1},
            {"id": "c", "type": "acoustic_compliance", "nodes": ["s", "a"], "C_m3_per_Pa": c},
            {"id": "r", "type": "acoustic_resistance", "node": "a", "R_Pa_s_per_m3": r}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "a"}]
    });
    let out = readouts::readouts(&design(&doc), &Default::default()).unwrap();
    let resp = out.response.as_ref().unwrap();
    let h2 = |x: f64| x * x / (1.0 + x * x);
    let a = h2(500.0 / fc) * 10f64.powf(-0.3);
    let fb = fc * (a / (1.0 - a)).sqrt();
    let b = resp.bass_extension.unwrap();
    assert!(close(b.f_hz, fb, 1e-9, 0.0), "{} vs {fb}", b.f_hz);
    let l500 = 10.0 * h2(2.5).log10() - 20.0 * 20e-6f64.log10();
    assert!((resp.level_500 - l500).abs() < 1e-9);
    assert!((b.value - (l500 - 3.0)).abs() < 1e-9);
    // No vsource: no impedance or sensitivity readouts, said in notes.
    assert!(out.impedance.is_none() && resp.sensitivity.is_empty());
    assert!(out.notes.iter().any(|n| n.contains("no vsource")));
    assert!(resp.coupled_resonance.is_none());
}

#[test]
fn template_readouts_are_consistent_with_the_drive() {
    let d = Design::parse(TEMPLATE, &Overrides::new()).unwrap();
    let r = readouts::readouts(&d, &Default::default()).unwrap();
    let resp = r.response.as_ref().unwrap();
    assert_eq!(
        (resp.probe.as_str(), resp.probe_source),
        ("p_drp", "ui.primary_probe")
    );
    // 1 mW into the rated 32 ohm: the level at 500 Hz is the dB/mW
    // sensitivity, and dB/V − dB/mW = 10·log10(1000/32) (erratum E32).
    let s500 = &resp.sensitivity[0];
    assert!((resp.level_500 - s500.db_per_mw.unwrap()).abs() < 1e-9);
    assert!(
        (s500.db_per_v - s500.db_per_mw.unwrap() - 10.0 * (1000.0f64 / 32.0).log10()).abs() < 1e-12
    );
    assert!((resp.level_1k - resp.sensitivity[1].db_per_mw.unwrap()).abs() < 1e-9);
    // Exact re-solves: equal to a solve whose grid holds 500 Hz and 1 kHz.
    let mut doc: Value = serde_json::from_str(TEMPLATE).unwrap();
    doc["sweep"] = json!({"frequencies_Hz": [500, 1000]});
    let exact = Circuit::from_json(&doc.to_string())
        .unwrap()
        .solve()
        .unwrap();
    let spl = exact.probe("p_drp").unwrap().spl_db();
    assert!((resp.level_500 - spl[0]).abs() < 1e-12 && (resp.level_1k - spl[1]).abs() < 1e-12);
    // The in-situ impedance peak and the coupled resonance are close.
    let z = r.impedance.as_ref().unwrap();
    let c = resp.coupled_resonance.as_ref().unwrap();
    assert!((z.resonance.unwrap().f_hz / c.f_hz - 1.0).abs() < 0.01);
    assert!(z.rated_check.as_ref().unwrap().pass);
    // A rated impedance given as an option overrides the drive key's.
    let r50 = readouts::readouts(
        &d,
        &ReadoutOptions {
            rated_ohm: Some(50.0),
            ..Default::default()
        },
    )
    .unwrap();
    let chk = r50
        .impedance
        .as_ref()
        .unwrap()
        .rated_check
        .as_ref()
        .unwrap();
    assert!(!chk.pass && !chk.violations.is_empty() && chk.limit_ohm == 40.0);
    let sc = r50.scalars();
    assert!(sc["z_min_over_rated"].unwrap() < 0.8);
    assert_eq!(sc.len(), readouts::SCALARS.len());
    // A characteristic drive puts 94 dB at 500 Hz at its probe.
    let mut doc: Value = serde_json::from_str(TEMPLATE).unwrap();
    doc["drive"] = json!({"characteristic": "p_drp"});
    let r = readouts::readouts(&design(&doc), &Default::default()).unwrap();
    assert!((r.response.unwrap().level_500 - 94.0).abs() < 1e-9);
}

#[test]
fn rated_impedance_check_reports_the_violating_band() {
    // |Z| = |20 + jωL|, L = 1 mH: below 0.8·32 = 25.6 ohm up to
    // ω = √(25.6² − 20²)/L.
    let doc = json!({
        "sweep": {"f_min_Hz": 100, "f_max_Hz": 20000, "points_per_octave": 24},
        "drive": {"power_mW": 1, "rated_ohm": 32},
        "nodes": [{"id": "e", "domain": "electrical"}, {"id": "m", "domain": "electrical"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e"},
            {"id": "r", "type": "resistor", "nodes": ["e", "m"], "R_ohm": 20},
            {"id": "l", "type": "inductor", "node": "m", "L_H": 1e-3}
        ],
        "probes": [{"id": "i", "quantity": "current", "element": "amp"}]
    });
    let r = readouts::readouts(&design(&doc), &Default::default()).unwrap();
    let chk = r.impedance.as_ref().unwrap().rated_check.clone().unwrap();
    let f_cross = (25.6f64 * 25.6 - 400.0).sqrt() / 1e-3 / (2.0 * PI);
    assert!(!chk.pass);
    assert_eq!(chk.violations.len(), 1);
    assert_eq!(chk.violations[0].f_min, 100.0);
    let grid = Circuit::from_json(&doc.to_string()).unwrap().freqs;
    let last_below = grid
        .iter()
        .copied()
        .filter(|f| *f < f_cross)
        .fold(0.0, f64::max);
    assert_eq!(chk.violations[0].f_max, last_below);
    assert!((chk.z_min_ohm - (C64::new(20.0, 2.0 * PI * 100.0 * 1e-3)).norm()).abs() < 1e-12);
    assert!(r.impedance.as_ref().unwrap().peaks.is_empty());
}

// ----- Tornado ------------------------------------------------------------------

#[test]
fn tornado_uses_tolerance_ends_and_ranks_by_effect() {
    let freqs = [300.0, 1000.0, 3000.0];
    let mut doc = rc(&freqs, (800.0, 1e6));
    let d = design(&doc);
    let metric = Metric::Level {
        probe: "vc".into(),
        f_hz: 1000.0,
    };
    let t = tornado::tornado(
        &d,
        &TornadoOptions {
            metric: Some(metric.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    // |H| = 1/√(1 + x²), x = (R/1000)·(C/C0) at 1 kHz.
    let h = |x: f64| 1.0 / (1.0 + x * x).sqrt();
    assert!((t.base - h(1.0)).abs() < 1e-12);
    assert_eq!(t.rows[0].name, "C_uF", "±10 % assumed beats ±5 %");
    assert_eq!(t.rows[0].basis, "assumed");
    assert!((t.rows[0].delta_low.unwrap() - (h(0.9) - h(1.0))).abs() < 1e-12);
    assert!((t.rows[0].delta_high.unwrap() - (h(1.1) - h(1.0))).abs() < 1e-12);
    let r = &t.rows[1];
    assert_eq!((r.name.as_str(), r.basis), ("R_ohm", "tolerance"));
    assert_eq!((r.low_value, r.high_value), (950.0, 1050.0));
    assert!((r.delta_high.unwrap() - (h(1.05) - h(1.0))).abs() < 1e-12);
    assert!(t.rows[0].effect >= t.rows[1].effect);
    assert_eq!(t.unit, "V");
    // Uniform and lognormal ends, and clipping at a bound.
    doc["parameters"]["R_ohm"]["tolerance"] = json!({"rel": 0.5, "dist": "uniform"});
    let t = tornado::tornado(
        &design(&doc),
        &TornadoOptions {
            metric: Some(metric.clone()),
            parameters: Some(vec!["R_ohm".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((t.rows[0].low_value, t.rows[0].high_value), (800.0, 1500.0));
    assert!(t.rows[0].clipped_low && !t.rows[0].clipped_high);
    doc["parameters"]["R_ohm"]["tolerance"] = json!({"rel": 0.25, "dist": "lognormal"});
    let t = tornado::tornado(
        &design(&doc),
        &TornadoOptions {
            metric: Some(metric),
            parameters: Some(vec!["R_ohm".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((t.rows[0].low_value, t.rows[0].high_value), (800.0, 1250.0));
    assert!(tornado::tolerance_ends(1000.0, None, 0.1) == (900.0, 1100.0));
}

#[test]
fn tornado_band_and_readout_metrics() {
    let freqs = log_grid(20.0, 20_000.0, 12.0, &[]);
    let d = design_with(&chamber(&freqs, None), &[("Mms_g", num(0.1))]);
    // A band mean is the mean of the grid dB SPL values in the band.
    let t = tornado::tornado(
        &d,
        &TornadoOptions {
            metric: Some(Metric::BandMean {
                probe: "p".into(),
                f_min_hz: 100.0,
                f_max_hz: 400.0,
            }),
            parameters: Some(vec!["V_cm3".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    let r = d.base_point().unwrap().solve().unwrap();
    let spl = r.probe("p").unwrap().spl_db();
    let band: Vec<f64> = freqs
        .iter()
        .zip(&spl)
        .filter(|(f, _)| **f >= 100.0 && **f <= 400.0)
        .map(|(_, l)| *l)
        .collect();
    assert!((t.base - band.iter().sum::<f64>() / band.len() as f64).abs() < 1e-12);
    assert_eq!(t.unit, "dB SPL");
    // The coupled resonance moves as 1/√Mms: Mms ± 5 % (the tolerance).
    let t = tornado::tornado(
        &d,
        &TornadoOptions {
            metric: Some(Metric::Readout {
                name: "coupled_resonance_Hz".into(),
            }),
            parameters: Some(vec!["Mms_g".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    let fc = |m: f64| ((1000.0 + 1e-6 * RHO_C2 / 30e-6) / m).sqrt() / (2.0 * PI);
    let row = &t.rows[0];
    assert!(close(t.base, fc(1e-4), 1e-7, 0.0));
    assert!(close(row.metric_low.unwrap(), fc(0.95e-4), 1e-7, 0.0));
    assert!(close(row.metric_high.unwrap(), fc(1.05e-4), 1e-7, 0.0));
    assert_eq!(t.unit, "Hz");
    let bad = TornadoOptions {
        metric: Some(Metric::Readout {
            name: "nope".into(),
        }),
        ..Default::default()
    };
    assert!(tornado::tornado(&d, &bad).is_err());
    assert_eq!(
        tornado::readout_unit("sensitivity_1kHz_dB_per_V"),
        "dB SPL per V"
    );
    assert_eq!(
        tornado::readout_unit("sensitivity_500Hz_dB_per_mW"),
        "dB SPL per mW"
    );
    assert_eq!(tornado::readout_unit("level_500Hz_dB"), "dB SPL");
    assert_eq!(tornado::readout_unit("z_min_ohm"), "ohm");
    assert_eq!(tornado::readout_unit("Qts"), "");
    // A readout that the base design does not have is an error, not an
    // empty chart. Below the 1204 Hz coupled resonance this sealed chamber
    // is stiffness-controlled: its level at 500 Hz is only about
    // 20·log10(1/(1 − (500/1204)²)) = 1.6 dB above the flat bass, so it
    // has no bass extension (−3 dB re 500 Hz).
    let p = |f: f64| {
        let c = chamber_form(f, 1e-4, 1000.0, 1e-3, 30e-6);
        1.0 / (c.jw * c.ztot).norm()
    };
    let rise = 20.0 * (p(500.0) / p(freqs[0])).log10();
    assert!(rise > 0.0 && rise < 2.0, "{rise}");
    let e = tornado::tornado(
        &d,
        &TornadoOptions {
            metric: Some(Metric::Readout {
                name: "bass_extension_Hz".into(),
            }),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("undefined for the base design"), "{e}");
    // The options parse from JSON with unit-suffixed keys.
    let o: TornadoOptions = serde_json::from_value(
        json!({"metric": {"kind": "level", "probe": "p", "f_Hz": 500}, "default_rel": 0.2}),
    )
    .unwrap();
    assert_eq!(
        o.metric,
        Some(Metric::Level {
            probe: "p".into(),
            f_hz: 500.0
        })
    );
    assert!(serde_json::from_value::<TornadoOptions>(
        json!({"metric": {"kind": "level", "probe": "p", "f_Hz": 500, "q": 1}})
    )
    .is_err());
}

// ----- Explain ------------------------------------------------------------------

/// Re-solves the design with one parameter at `to`, independently of the
/// explain code, and returns ΔdB and the credible mask.
fn independent_delta(text: &str, name: &str, to: f64, probe: &str) -> (Vec<f64>, Vec<bool>) {
    let base = Circuit::from_json(text).unwrap().solve().unwrap();
    let mut ov = Overrides::new();
    ov.insert(name.to_string(), PValue::Num(to));
    let new = Circuit::from_json_with(text, &ov).unwrap().solve().unwrap();
    let a = base.probe(probe).unwrap().spl_db();
    let b = new.probe(probe).unwrap().spl_db();
    let credible = base
        .freqs_hz
        .iter()
        .map(|f| base.shading.band(*f) == 0 && new.shading.band(*f) == 0)
        .collect();
    (b.iter().zip(&a).map(|(x, y)| x - y).collect(), credible)
}

fn check_explanation(text: &str, e: &explain::Explanation) {
    for s in &e.sentences {
        let (delta, credible) = independent_delta(text, &s.parameter, s.to_value, &e.probe);
        let effect = delta
            .iter()
            .zip(&credible)
            .filter(|(_, c)| **c)
            .fold(0.0f64, |m, (d, _)| m.max(d.abs()));
        assert!((s.effect_db - effect).abs() < 1e-12);
        assert!(s.effect_db >= e.threshold_db);
        for (k, b) in s.bands.iter().enumerate() {
            let sign = if b.effect == "raises" { 1.0 } else { -1.0 };
            for i in b.first..=b.last {
                assert!(
                    credible[i] && sign * delta[i] > e.threshold_db,
                    "{} band {k} point {i}",
                    s.text
                );
            }
            // Maximal: the neighbours are outside the band.
            for i in [b.first.wrapping_sub(1), b.last + 1] {
                if i < delta.len() {
                    assert!(!(credible[i] && sign * delta[i] > e.threshold_db));
                }
            }
            let mean = delta[b.first..=b.last].iter().sum::<f64>() / (b.last - b.first + 1) as f64;
            assert!((b.mean_db - mean).abs() < 1e-12);
            assert_eq!(
                (b.f_min, b.f_max),
                (e.freqs_hz[b.first], e.freqs_hz[b.last])
            );
        }
        // The text states the bands it says it states, with their numbers.
        assert!(!s.stated.is_empty() && s.stated.len() <= 2);
        for &k in &s.stated {
            let b = &s.bands[k];
            let stated = format!("{} ", b.effect);
            assert!(s.text.contains(&stated), "{}", s.text);
            let v = explain::fmt_db(b.mean_db);
            assert!(
                s.text.contains(&format!("by {v} dB on average")),
                "{}",
                s.text
            );
            let decimals = v.split('.').nth(1).map_or(0, str::len) as i32;
            assert!(
                (v.parse::<f64>().unwrap() - b.mean_db.abs()).abs()
                    <= 0.5 * 10f64.powi(-decimals) + 1e-12
            );
            let span = if b.first == b.last {
                acoustilab::analysis::fmt_hz(b.f_min)
            } else {
                acoustilab::analysis::fmt_hz_range(b.f_min, b.f_max)
            };
            assert!(s.text.contains(&span), "{} lacks {span}", s.text);
        }
        let verb = if s.direction == "raise" {
            "Raising"
        } else {
            "Lowering"
        };
        assert!(s.text.starts_with(verb));
        match &s.resonance {
            Some(r) => {
                assert!(s.text.contains("moves the coupled resonance from"));
                let span = acoustilab::analysis::fmt_hz_range(r.from_hz, r.to_hz);
                assert!(s.text.contains(&span));
                // Independently: the readouts of both designs.
                let mut ov = Overrides::new();
                ov.insert(s.parameter.clone(), PValue::Num(s.to_value));
                let base = readouts::readouts(
                    &Design::parse(text, &Overrides::new()).unwrap(),
                    &Default::default(),
                )
                .unwrap();
                let new =
                    readouts::readouts(&Design::parse(text, &ov).unwrap(), &Default::default())
                        .unwrap();
                let fa = base.response.unwrap().coupled_resonance.unwrap();
                let fb = new.response.unwrap().coupled_resonance.unwrap();
                assert!(fa.robust && fb.robust);
                assert!((fa.f_hz - r.from_hz).abs() < 1e-9 * fa.f_hz);
                assert!((fb.f_hz - r.to_hz).abs() < 1e-9 * fb.f_hz);
            }
            None => assert!(!s.text.contains("resonance")),
        }
    }
}

#[test]
fn explain_sentences_are_supported_by_independent_re_solves() {
    for ov in [vec![], vec![("rear", PValue::Str("open".into()))]] {
        let o: Overrides = ov.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let d = Design::parse(TEMPLATE, &o).unwrap();
        let e = explain::explain(&d, &ExplainOptions::default()).unwrap();
        assert_eq!(e.probe, "p_drp");
        assert_eq!(e.sentences.len(), 5);
        // Ranked by effect.
        for w in e.sentences.windows(2) {
            assert!(w[0].effect_db >= w[1].effect_db);
        }
        let mut doc: Value = serde_json::from_str(TEMPLATE).unwrap();
        for (k, v) in &o {
            doc["parameters"][k]["value"] = v.to_json();
        }
        check_explanation(&doc.to_string(), &e);
        // Nothing is said about frequencies outside the credible band.
        for s in &e.sentences {
            for b in &s.bands {
                assert!(e.credible[b.first] && e.credible[b.last]);
            }
        }
    }
}

#[test]
fn explain_respects_erratum_e24_and_its_options() {
    // Erratum E24: +10 % front volume cannot lower a pressure chamber by
    // more than 20·log10(1.1) = 0.83 dB; at constant voltage the spec's C2
    // chamber (here with 0.3 g) drops only 0.15 dB, since p ∝ 1/(Kms·V +
    // ρc²Sd²). Below the 0.3 dB threshold nothing is said about the bass;
    // the sentence is about the rise below the (lower) resonance.
    let freqs = log_grid(20.0, 5000.0, 12.0, &[]);
    let doc = chamber(&freqs, None);
    let d = design(&doc);
    let only_v = |threshold: f64| ExplainOptions {
        parameters: Some(vec!["V_cm3".into()]),
        threshold_db: Some(threshold),
        ..Default::default()
    };
    let e = explain::explain(&d, &only_v(0.3)).unwrap();
    let s = &e.sentences[0];
    assert!(
        s.text.starts_with("Raising front volume 10 % raises p by"),
        "{}",
        s.text
    );
    assert!(s
        .bands
        .iter()
        .all(|b| b.effect == "raises" && b.f_min > 300.0));
    // The coupled resonance (695 Hz) lies in the shaded band of this
    // cavity, so it is not mentioned.
    let base_res = e.base_resonance.as_ref().unwrap();
    assert!(base_res.robust && base_res.shading > 0 && s.resonance.is_none());
    check_explanation(&doc.to_string(), &e);
    // With a 0.1 dB threshold the bass drop appears, as the closed form says.
    let e = explain::explain(&d, &only_v(0.1)).unwrap();
    let s = &e.sentences[0];
    let low = &s.bands[0];
    assert_eq!((low.effect, low.first), ("lowers", 0));
    assert!(low.max_abs_db <= 20.0 * 1.1f64.log10());
    let p = |f: f64, v: f64| {
        let c = chamber_form(f, 3e-4, 1000.0, 1e-3, v);
        1.0 / (v * c.ztot)
    };
    let want = 20.0 * (p(20.0, 33e-6) / p(20.0, 30e-6)).norm().log10();
    assert!(
        (s.delta_db[0] - want).abs() < 1e-9,
        "{} vs {want}",
        s.delta_db[0]
    );
    assert!((want + 0.15).abs() < 0.01);
    check_explanation(&doc.to_string(), &e);
    // At its maximum a parameter is lowered instead.
    let d = design_with(&doc, &[("V_cm3", num(1000.0))]);
    let e = explain::explain(
        &d,
        &ExplainOptions {
            parameters: Some(vec!["V_cm3".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(e.sentences[0].direction, "lower");
    assert!(e.sentences[0]
        .text
        .starts_with("Lowering front volume 10 % raises p"));
    // A high threshold leaves nothing to say.
    let e = explain::explain(
        &design(&doc),
        &ExplainOptions {
            threshold_db: Some(50.0),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(e.sentences.is_empty() && !e.quiet.is_empty());
    assert!(explain::explain(
        &design(&doc),
        &ExplainOptions {
            probe: Some("zin".into()),
            ..Default::default()
        }
    )
    .is_err());
}

// ----- Sampling, hashing, envelopes: the Python reference ---------------------

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

#[test]
fn generators_match_the_reference_streams() {
    let f = fixture();
    for (seed, st) in f["streams"].as_object().unwrap() {
        let seed: u64 = seed.parse().unwrap();
        let mut sm = SplitMix64::new(seed);
        for v in st["splitmix64"].as_array().unwrap() {
            assert_eq!(sm.next_u64(), v.as_str().unwrap().parse::<u64>().unwrap());
        }
        let mut x = Xoshiro256::seed_from(seed);
        for v in st["xoshiro256"].as_array().unwrap() {
            assert_eq!(x.next_u64(), v.as_str().unwrap().parse::<u64>().unwrap());
        }
    }
    let mut x = Xoshiro256::seed_from(f["below_seed"].as_u64().unwrap());
    for case in f["below"].as_array().unwrap() {
        let n = case[0].as_u64().unwrap();
        for v in case[1].as_array().unwrap() {
            assert_eq!(x.below(n), v.as_str().unwrap().parse::<u64>().unwrap());
        }
    }
    let mut x = Xoshiro256::seed_from(f["shuffle_seed"].as_u64().unwrap());
    let mut v: Vec<u64> = (0..20).collect();
    x.shuffle(&mut v);
    let want: Vec<u64> = f["shuffle"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap())
        .collect();
    assert_eq!(v, want);
}

#[test]
fn normal_quantile_matches_mpmath() {
    // AS 241 is accurate to about 1e-16 relative; with the deterministic ln
    // (≤ 2 ulp) the observed error must stay below 1e-15.
    for row in fixture()["quantiles"].as_array().unwrap() {
        let (p, z) = (parse(&row[0]), parse(&row[1]));
        let got = detmath::norm_quantile(p);
        assert!(close(got, z, 1e-15, 1e-300), "p = {p:e}: {got} vs {z}");
    }
}

#[test]
fn latin_hypercube_matches_the_reference_plan() {
    let f = fixture();
    let l = &f["lhs"];
    let n = l["n"].as_u64().unwrap() as usize;
    let seed = l["seed"].as_u64().unwrap();
    let unit = mc::lhs_unit(n, 3, seed);
    for (row, want) in unit.iter().zip(l["unit"].as_array().unwrap()) {
        for (a, b) in row.iter().zip(want.as_array().unwrap()) {
            assert_eq!(*a, parse(b), "unit cube points are bit-identical");
        }
    }
    let doc = json!({
        "parameters": {
            "a": {"value": 10, "min": 9.25, "tolerance": {"abs": 1}},
            "b": {"value": 2, "tolerance": {"abs": 0.3, "dist": "uniform"}},
            "c": {"value": 0.08, "min": 0.05, "tolerance": {"rel": 0.5, "dist": "lognormal"}}
        }
    });
    let spec: PlanSpec =
        serde_json::from_value(json!({"method": "lhs", "n": n, "seed": seed})).unwrap();
    let plan = mc::plan(&design(&doc), &spec).unwrap();
    for (s, want) in plan.samples.iter().zip(l["values"].as_array().unwrap()) {
        for (name, w) in ["a", "b", "c"].iter().zip(want.as_array().unwrap()) {
            let got = s.overrides[*name].as_num().unwrap();
            assert!(close(got, parse(w), 1e-14, 0.0), "{name}: {got} vs {w}");
        }
    }
    let clipped = plan.distributions.iter().map(|d| d.clipped).sum::<usize>();
    assert_eq!(clipped, plan.clipped);
    // The lowest stratum of `a` lies below its minimum: Φ⁻¹(1/16)·0.5 + 10 < 9.25.
    assert!(plan.distributions[0].clipped >= 1);
    assert_eq!(
        plan.samples.iter().map(|s| s.clipped.len()).sum::<usize>(),
        plan.clipped
    );
}

#[test]
fn canonical_text_and_hash_match_python() {
    for c in fixture()["canonical"].as_array().unwrap() {
        let doc: Value = serde_json::from_str(c["doc"].as_str().unwrap()).unwrap();
        let text = if c["netlist"].as_bool().unwrap() {
            canonical::netlist_text(&doc)
        } else {
            canonical::canonical(&doc)
        };
        assert_eq!(text, c["text"].as_str().unwrap());
        assert_eq!(
            canonical::hex(&canonical::sha256(text.as_bytes())),
            c["sha256"].as_str().unwrap()
        );
    }
}

#[test]
fn percentiles_match_numpy() {
    let p = &fixture()["percentiles"];
    let data: Vec<Vec<f64>> = p["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_array().unwrap().iter().map(parse).collect())
        .collect();
    let rows: Vec<&[f64]> = data.iter().map(Vec::as_slice).collect();
    let st = mc::curve_stats(&rows, 6);
    let pick = |q: &str| -> &Vec<f64> {
        match q {
            "0.05" => &st.p5,
            "0.1" => &st.p10,
            "0.5" => &st.median,
            "0.9" => &st.p90,
            _ => &st.p95,
        }
    };
    for (q, want) in p["expected"].as_object().unwrap() {
        for (a, b) in pick(q).iter().zip(want.as_array().unwrap()) {
            assert!((a - parse(b)).abs() < 1e-12, "q {q}: {a} vs {b}");
        }
    }
    for (a, b) in st.min.iter().zip(p["min"].as_array().unwrap()) {
        assert_eq!(*a, parse(b));
    }
    assert!(st.n.iter().all(|n| *n == 37));
}

// ----- Monte Carlo and design of experiments -----------------------------------

#[test]
fn sample_moments_match_the_declared_distributions() {
    // Normal: tolerance = 2σ. Uniform: ± tolerance. Lognormal: ln x normal
    // with median μ and σ_ln = ln(1 + rel)/2 (docs/parameters.md).
    let doc = json!({
        "parameters": {
            "a": {"value": 10, "tolerance": {"rel": 0.1}},
            "b": {"value": 2, "tolerance": {"abs": 0.3, "dist": "uniform"}},
            "c": {"value": 0.08, "tolerance": {"rel": 0.5, "dist": "lognormal"}}
        }
    });
    let n = 20_000;
    let plan = mc::plan(
        &design(&doc),
        &PlanSpec::Lhs {
            n,
            seed: 3,
            parameters: None,
        },
    )
    .unwrap();
    let col = |k: &str| -> Vec<f64> {
        plan.samples
            .iter()
            .map(|s| s.overrides[k].as_num().unwrap())
            .collect()
    };
    let stats = |v: &[f64]| {
        let m = v.iter().sum::<f64>() / v.len() as f64;
        let var = v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (v.len() - 1) as f64;
        (m, var.sqrt())
    };
    // Stratified sampling: the mean of n points is exact to ~σ/n and the
    // spread to a fraction of a percent.
    let (m, s) = stats(&col("a"));
    assert!(
        (m - 10.0).abs() < 1e-3 && (s / 0.5 - 1.0).abs() < 2e-3,
        "{m} {s}"
    );
    let (m, s) = stats(&col("b"));
    assert!(
        (m - 2.0).abs() < 1e-4 && (s / (0.3 / 3f64.sqrt()) - 1.0).abs() < 1e-3,
        "{m} {s}"
    );
    let b = col("b");
    assert!(b.iter().all(|x| (x - 2.0).abs() <= 0.3));
    let ln: Vec<f64> = col("c").iter().map(|x| x.ln()).collect();
    let (m, s) = stats(&ln);
    let sigma_ln = 1.5f64.ln() / 2.0;
    assert!(
        (m - 0.08f64.ln()).abs() < 1e-3 && (s / sigma_ln - 1.0).abs() < 2e-3,
        "{m} {s}"
    );
    // 2.3 % (the one-sided 2σ tail, 2.275 %) of the samples lie above
    // μ(1 + rel), and as many below μ/(1 + rel).
    let above = col("c").iter().filter(|x| **x > 0.12).count() as f64 / n as f64;
    let below = col("c").iter().filter(|x| **x < 0.08 / 1.5).count() as f64 / n as f64;
    assert!(
        (above - 0.02275).abs() < 0.002 && (below - 0.02275).abs() < 0.002,
        "{above} {below}"
    );
    // Deterministic: the same seed gives the same plan.
    let again = mc::plan(
        &design(&doc),
        &PlanSpec::Lhs {
            n,
            seed: 3,
            parameters: None,
        },
    )
    .unwrap();
    assert_eq!(plan, again);
}

#[test]
fn monte_carlo_runs_are_deterministic_chunkable_and_hashed() {
    let d = Design::parse(TEMPLATE, &Overrides::new()).unwrap();
    let plan = mc::plan(
        &d,
        &PlanSpec::Lhs {
            n: 6,
            seed: 9,
            parameters: None,
        },
    )
    .unwrap();
    // Every toleranced continuous parameter, in declaration order (counted
    // from the declarations, so that editing the template does not break
    // the test).
    let toleranced: Vec<String> = d
        .parametric
        .defs
        .iter()
        .filter(|p| {
            p.tolerance.is_some()
                && matches!(
                    p.kind,
                    acoustilab::params::ParamKind::Number { integer: false, .. }
                )
        })
        .map(|p| p.name.clone())
        .collect();
    assert!(toleranced.len() >= 2);
    assert_eq!(plan.parameters, toleranced);
    let opts = RunOptions {
        probes: Some(vec!["p_drp".into(), "zin".into(), "x".into()]),
        ..Default::default()
    };
    let all = mc::run(&d, &plan.samples, &opts).unwrap();
    let a = mc::run(&d, &plan.samples[..2], &opts).unwrap();
    let b = mc::run(&d, &plan.samples[2..], &opts).unwrap();
    let mut joined = a.samples.clone();
    joined.extend(b.samples);
    assert_eq!(all.samples, joined, "chunking changes nothing");
    assert_eq!(
        all,
        mc::run(&d, &plan.samples, &opts).unwrap(),
        "runs are deterministic"
    );
    for (s, r) in plan.samples.iter().zip(&all.samples) {
        assert!(r.ok, "{:?}", r.error);
        assert_eq!(r.curves.len(), 3);
        assert!(r.curves[0].db.is_some() && r.curves[0].magnitude.is_none());
        assert!(r.curves[1].magnitude.is_some() && r.curves[1].phase_deg.is_some());
        assert!(r.curves[2].magnitude.is_some() && r.curves[2].phase_deg.is_none());
        // The curves are the solve's, and the hash is that of the expanded
        // netlist of the same overrides.
        let c = Circuit::from_json_with(TEMPLATE, &s.overrides).unwrap();
        let res = c.solve().unwrap();
        assert_eq!(
            r.curves[0].db.as_ref().unwrap(),
            &res.probe("p_drp").unwrap().spl_db()
        );
        let p = acoustilab::params::Parametric::parse(TEMPLATE).unwrap();
        let expanded = p.resolve(&s.overrides).unwrap().doc;
        assert_eq!(
            r.hash.as_deref(),
            Some(canonical::netlist_hash(&expanded).as_str())
        );
        assert_eq!(r.metrics.len(), readouts::SCALARS.len());
    }
    let hashes: std::collections::BTreeSet<_> =
        all.samples.iter().map(|r| r.hash.clone()).collect();
    assert_eq!(hashes.len(), 6);
    // Envelopes bracket every run.
    let env = mc::envelope(&all.freqs_hz, &all.samples);
    assert_eq!((env.runs, env.failed), (6, 0));
    let e = env.probes[0].db.as_ref().unwrap();
    for r in &all.samples {
        for (k, v) in r.curves[0].db.as_ref().unwrap().iter().enumerate() {
            assert!(e.min[k] <= *v && *v <= e.max[k]);
            assert!(e.p5[k] <= e.median[k] && e.median[k] <= e.p95[k]);
        }
    }
    // Round trip through JSON, as the wasm worker does. serde_json writes
    // shortest round-trip numbers but parses them best-effort (a last-ulp
    // difference is possible without its float_roundtrip feature), so the
    // numbers are compared to 1e-15 and everything else exactly.
    let text = serde_json::to_string(&all).unwrap();
    let back: mc::RunChunk = serde_json::from_str(&text).unwrap();
    assert_eq!(back.samples.len(), all.samples.len());
    for (b, a) in back.samples.iter().zip(&all.samples) {
        assert_eq!((b.index, &b.hash, b.ok), (a.index, &a.hash, a.ok));
        for (cb, ca) in b.curves.iter().zip(&a.curves) {
            assert_eq!(cb.id, ca.id);
            for (x, y) in [
                (&cb.db, &ca.db),
                (&cb.magnitude, &ca.magnitude),
                (&cb.phase_deg, &ca.phase_deg),
            ] {
                assert_eq!(x.is_some(), y.is_some());
                for (u, v) in x.iter().flatten().zip(y.iter().flatten()) {
                    assert!((u - v).abs() <= 1e-15 * v.abs());
                }
            }
        }
        for (k, v) in &a.metrics {
            let u = b.metrics[k];
            assert_eq!(u.is_some(), v.is_some());
            assert!((u.unwrap_or(0.0) - v.unwrap_or(0.0)).abs() <= 1e-15 * v.unwrap_or(0.0).abs());
        }
    }
    let csv = mc::to_csv(&plan.parameters, &all.samples, &all.engine);
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 7);
    assert!(lines[0].starts_with("run,hash,engine,driver_fs_Hz,"));
    assert!(lines[0].ends_with(",z_min_over_rated,error"));
    assert!(lines[1].starts_with(&format!(
        "0,{},acoustilab ",
        all.samples[0].hash.as_ref().unwrap()
    )));
}

#[test]
fn netlist_hash_depends_on_what_the_engine_reads() {
    let p = acoustilab::params::Parametric::parse(TEMPLATE).unwrap();
    let base = p.resolve(&Overrides::new()).unwrap().doc;
    let h = canonical::netlist_hash(&base);
    let mut other = base.clone();
    other["title"] = json!("renamed");
    other["ui"] = json!({"primary_probe": "p_front"});
    assert_eq!(
        canonical::netlist_hash(&other),
        h,
        "presentation keys are ignored"
    );
    // A parameter that only feeds a disabled element leaves the netlist,
    // and therefore the hash, unchanged; one that feeds the network does not.
    let mut ov = Overrides::new();
    ov.insert("grille_rayl".into(), num(99.0));
    assert_eq!(canonical::netlist_hash(&p.resolve(&ov).unwrap().doc), h);
    ov.insert("leak_gap_mm".into(), num(0.09));
    assert_ne!(canonical::netlist_hash(&p.resolve(&ov).unwrap().doc), h);
    // A literal netlist equal to the expansion hashes the same.
    let literal: Value = serde_json::from_str(&base.to_string()).unwrap();
    assert_eq!(canonical::netlist_hash(&literal), h);
}

#[test]
fn netlist_hash_of_a_fixed_parametric_netlist_is_pinned() {
    // Regression pin of the expansion and the canonical form on a netlist
    // written here, so that editing the template never breaks it. The
    // expected text was written by hand from docs/analysis.md (sorted keys,
    // 12 significant digits, `enabled` and presentation keys dropped) and
    // its SHA-256 computed with Python's hashlib.
    let doc = json!({
        "schema": "acoustilab-netlist/0.2", "title": "hash pin", "ui": {"primary_probe": "p"},
        "parameters": {
            "r_mm": {"value": 1.5, "min": 0.1, "max": 5, "tolerance": {"rel": 0.05}},
            "len_mm": {"expr": "r_mm * pi"},
            "open": true
        },
        "air": {"preset": "spec_reference"}, "level": 1,
        "sweep": {"f_min_Hz": 100, "f_max_Hz": 1000, "points_per_octave": 3},
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [
            {"id": "q", "type": "flow_source", "node": "a", "U_m3_per_s": 1e-6},
            {"id": "v", "type": "cavity", "node": "a", "volume_cm3": 2},
            {"id": "t", "type": "tube", "nodes": ["a", "ambient"], "radius_mm": "=r_mm",
             "length_mm": "=len_mm", "enabled": "=open"}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "a"}]
    });
    let point = design(&doc).base_point().unwrap();
    assert_eq!(
        canonical::netlist_text(&point.doc),
        concat!(
            r#"{"air":{"preset":"spec_reference"},"elements":[{"U_m3_per_s":1e-6,"id":"q","node":"a","type":"flow_source"},"#,
            r#"{"id":"v","node":"a","type":"cavity","volume_cm3":2e0},"#,
            r#"{"id":"t","length_mm":4.71238898038e0,"nodes":["a","ambient"],"radius_mm":1.5e0,"type":"tube"}],"#,
            r#""level":1e0,"nodes":[{"domain":"acoustic","id":"a"}],"probes":[{"id":"p","node":"a","quantity":"pressure"}],"#,
            r#""sweep":{"f_max_Hz":1e3,"f_min_Hz":1e2,"points_per_octave":3e0}}"#
        )
    );
    assert_eq!(
        point.hash(),
        "5cf4594e9a48f64a618f73f7529be5f23eedad737a81c2a628302b3791db3162"
    );
}

#[test]
fn factorial_and_explicit_runs() {
    let d = Design::parse(TEMPLATE, &Overrides::new()).unwrap();
    let spec: PlanSpec = serde_json::from_value(json!({
        "method": "factorial",
        "factors": {
            "ear": ["iec60318_4", "type43"],
            "vent_count": {"levels": 3, "from": 0, "to": 4},
            "leak_gap_mm": {"levels": 2, "from": 0.02, "to": 0.2, "log": true}
        }
    }))
    .unwrap();
    let plan = mc::plan(&d, &spec).unwrap();
    assert_eq!(plan.samples.len(), 12);
    assert_eq!(plan.parameters, ["ear", "vent_count", "leak_gap_mm"]);
    // First factor slowest, last fastest.
    let s = &plan.samples;
    assert_eq!(s[0].overrides["ear"], PValue::Str("iec60318_4".into()));
    assert_eq!(s[6].overrides["ear"], PValue::Str("type43".into()));
    assert_eq!(s[0].overrides["leak_gap_mm"], num(0.02));
    assert_eq!(s[1].overrides["leak_gap_mm"], num(0.2));
    assert_eq!(s[2].overrides["vent_count"], num(2.0));
    assert_eq!(s[4].overrides["vent_count"], num(4.0));
    // Bad levels are rejected at plan time.
    for bad in [
        json!({"method": "factorial", "factors": {"vent_count": [1.5]}}),
        json!({"method": "factorial", "factors": {"ear": ["nope"]}}),
        json!({"method": "factorial", "factors": {}}),
        json!({"method": "runs", "runs": [{"leak_gap_mm": 5}]}),
        json!({"method": "lhs", "n": 0}),
        json!({"method": "lhs", "n": 3, "parameters": ["driver_Sd_cm2"]}),
        json!({"method": "lhs", "n": 3, "parameters": ["driver_fs_Hz", "driver_fs_Hz"]}),
        json!({"method": "lhs", "n": mc::MAX_RUNS + 1}),
        // Rejected before any level is made (it would need terabytes).
        json!({"method": "factorial",
               "factors": {"leak_gap_mm": {"levels": 1_000_000_000_000u64, "from": 0.02, "to": 0.2}}}),
        json!({"method": "factorial", "factors": {"leak_gap_mm": {"levels": 1, "from": 0.02, "to": 0.2}}}),
    ] {
        let spec: Result<PlanSpec, _> = serde_json::from_value(bad.clone());
        let failed = match spec {
            Err(_) => true,
            Ok(s) => mc::plan(&d, &s).is_err(),
        };
        assert!(failed, "{bad}");
    }
    assert!(
        serde_json::from_value::<PlanSpec>(json!({"method": "lhs", "n": 3, "extra": 1})).is_err()
    );
    // Nothing to sample is an error, not n copies of the base design.
    let mut untoleranced = rc(&[100.0, 1000.0], (1.0, 1e6));
    untoleranced["parameters"]["R_ohm"]
        .as_object_mut()
        .unwrap()
        .remove("tolerance");
    let e = mc::plan(
        &design(&untoleranced),
        &PlanSpec::Lhs {
            n: 3,
            seed: 1,
            parameters: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        e.contains("no continuous parameter with a tolerance"),
        "{e}"
    );
    let spec: PlanSpec = serde_json::from_value(json!({
        "method": "runs", "runs": [{"rear": "open"}, {"rear": "closed", "vent_count": 2}]
    }))
    .unwrap();
    let plan = mc::plan(&d, &spec).unwrap();
    assert_eq!(plan.parameters, ["rear", "vent_count"]);
    let chunk = mc::run(
        &d,
        &plan.samples,
        &RunOptions {
            metrics: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(chunk.samples.iter().all(|r| r.ok && r.metrics.is_empty()));
    // Structure differs between runs: the vent probe exists only when closed.
    assert!(chunk.samples[0].curves.iter().all(|c| c.id != "u_vent"));
    assert!(chunk.samples[1].curves.iter().any(|c| c.id == "u_vent"));
    let csv = mc::to_csv(&plan.parameters, &chunk.samples, &chunk.engine);
    assert!(
        csv.lines().next().unwrap() == "run,hash,engine,rear,vent_count,error",
        "{csv}"
    );
    assert!(
        csv.contains(",open,,") && csv.contains(",closed,2.0,"),
        "{csv}"
    );
}

#[test]
fn a_failing_run_is_reported_and_the_others_go_on() {
    let mut doc = rc(&[100.0, 1000.0], (0.0, 1e6));
    doc["parameters"]["R_ohm"]["min"] = json!(0);
    let d = design(&doc);
    let samples = vec![
        Sample {
            index: 0,
            overrides: [("R_ohm".to_string(), num(0.0))].into(),
            clipped: vec![],
        },
        Sample {
            index: 1,
            overrides: [("R_ohm".to_string(), num(500.0))].into(),
            clipped: vec![],
        },
    ];
    let chunk = mc::run(&d, &samples, &Default::default()).unwrap();
    assert!(!chunk.samples[0].ok);
    let err = chunk.samples[0].error.as_deref().unwrap();
    assert!(err.contains("element 'r'"), "{err}");
    assert!(chunk.samples[1].ok);
    let env = mc::envelope(&chunk.freqs_hz, &chunk.samples);
    assert_eq!((env.runs, env.failed), (1, 1));
    let csv = mc::to_csv(&[], &chunk.samples, &chunk.engine);
    let row0 = csv.lines().nth(1).unwrap();
    assert!(
        row0.starts_with("0,,acoustilab") && row0.ends_with(err),
        "{row0}"
    );
    // RFC 4180 quoting of a field with a comma or a quote.
    let mut quoted = chunk.samples[0].clone();
    quoted.error = Some("a, \"b\"".into());
    let csv = mc::to_csv(&[], &[quoted], "e");
    assert!(
        csv.lines().nth(1).unwrap().ends_with(r#","a, ""b""""#),
        "{csv}"
    );
}
