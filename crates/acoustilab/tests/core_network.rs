//! End-to-end checks of the MNA core, couplers and sign conventions against
//! closed-form solutions of a driver loading a sealed cavity.

use acoustilab::{AirState, Circuit, C64};
use serde_json::json;
use std::f64::consts::PI;

const RE: f64 = 32.0;
const BL: f64 = 1.0;
const MMS: f64 = 0.1e-3;
const CMS: f64 = 1e-3;
const RMS: f64 = 0.05;
const SD: f64 = 10e-4;
const V: f64 = 30e-6;

fn sealed_cup(wall_loss: bool, re: f64, bl: f64) -> Circuit {
    let doc = json!({
        "schema": "acoustilab-netlist/0.1",
        "air": {"preset": "spec_reference"},
        "sweep": {"f_min_Hz": 10, "f_max_Hz": 20000, "points_per_octave": 12},
        "nodes": [
            {"id": "e1", "domain": "electrical"},
            {"id": "e2", "domain": "electrical"},
            {"id": "m", "domain": "mechanical"},
            {"id": "af", "domain": "acoustic"}
        ],
        "elements": [
            {"id": "amp", "type": "vsource", "nodes": ["e1"], "V_V": 1.0},
            {"id": "coil", "type": "coil", "nodes": ["e1", "e2"], "Re_ohm": re},
            {"id": "motor", "type": "motor", "nodes": ["e2", "gnd", "m", "gnd"], "Bl_Tm": bl},
            {"id": "susp", "type": "suspension", "node": "m",
             "Mms_kg": MMS, "Cms_m_per_N": CMS, "Rms_Ns_per_m": RMS},
            {"id": "dia", "type": "piston", "nodes": ["m", "gnd", "af", "gnd"], "Sd_m2": SD},
            {"id": "front", "type": "cavity", "node": "af", "volume_m3": V, "wall_loss": wall_loss}
        ],
        "probes": [
            {"id": "p", "quantity": "pressure", "node": "af"},
            {"id": "zin", "quantity": "impedance", "element": "amp"},
            {"id": "x", "quantity": "displacement", "node": "m"},
            {"id": "i", "quantity": "current", "element": "amp"}
        ]
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

/// Closed-form pressure and input impedance (lossless cavity).
fn analytic(f: f64, re: f64, bl: f64) -> (C64, C64) {
    let air = AirState::spec_reference();
    let jw = C64::new(0.0, 2.0 * PI * f);
    let caf = V / air.bulk_modulus();
    let zm = jw * MMS + RMS + (jw * CMS).inv() + SD * SD / (jw * caf);
    let zin = re + bl * bl / zm;
    let i = 1.0 / zin;
    let v = bl * i / zm;
    let p = SD * v / (jw * caf);
    (p, zin)
}

#[test]
fn matches_closed_form_driver_in_sealed_cavity() {
    let c = sealed_cup(false, RE, BL);
    let r = c.solve().unwrap();
    let p = r.probe("p").unwrap();
    let z = r.probe("zin").unwrap();
    for (k, &f) in r.freqs_hz.iter().enumerate() {
        let (pa, za) = analytic(f, RE, BL);
        assert!(
            (p.values[k] - pa).norm() <= 1e-10 * pa.norm(),
            "p at {f} Hz"
        );
        assert!(
            (z.values[k] - za).norm() <= 1e-10 * za.norm(),
            "Zin at {f} Hz"
        );
    }
}

#[test]
fn sign_convention_positive_voltage_gives_positive_pressure() {
    // Spec Section 3, evaluated at a low non-zero frequency (DC is excluded).
    let c = sealed_cup(false, RE, BL);
    let x = c.solve_at(20.0).unwrap();
    let p = c.node_value(&x, "af").unwrap();
    let v = c.node_value(&x, "m").unwrap();
    assert!(p.re > 0.0 && p.arg().abs() < 0.05, "p = {p}");
    // Stiffness-controlled: velocity leads displacement by 90°.
    assert!(v.im > 0.0);
    let i = c.probe_value(&c.probes[3], 20.0, &x).unwrap();
    assert!(i.re > 0.0);
}

#[test]
fn pressure_chamber_level_matches_rho_c2_sd_x_over_v() {
    // Spec C1: p = ρc²·Sd·x/V for a lossless adiabatic cavity.
    let c = sealed_cup(false, RE, BL);
    let air = AirState::spec_reference();
    for f in [20.0, 50.0, 100.0] {
        let x = c.solve_at(f).unwrap();
        let p = c.probe_value(&c.probes[0], f, &x).unwrap();
        let d = c.probe_value(&c.probes[2], f, &x).unwrap();
        let expect = air.rho * air.c * air.c * SD * d / V;
        assert!((p - expect).norm() < 1e-10 * p.norm());
    }
    // Worked example: 10 cm², 30 cm³, 1 µm RMS → 4.72 Pa, 107.46 dB.
    let p = air.rho * air.c * air.c * SD * 1e-6 / V;
    assert!((p - 4.7216).abs() < 1e-3);
    assert!((20.0 * (p / 20e-6).log10() - 107.46).abs() < 0.01);
}

#[test]
fn coil_rewind_scaling_is_9_72_db_at_constant_voltage() {
    // Spec C7: Bl ∝ sqrt(Re) at constant mass; 32 → 300 Ω costs 9.72 dB.
    let a = sealed_cup(false, 32.0, 1.0);
    let b = sealed_cup(false, 300.0, (300.0f64 / 32.0).sqrt());
    for f in [30.0, 300.0, 3000.0] {
        let pa = a
            .probe_value(&a.probes[0], f, &a.solve_at(f).unwrap())
            .unwrap();
        let pb = b
            .probe_value(&b.probes[0], f, &b.solve_at(f).unwrap())
            .unwrap();
        let d = 20.0 * (pb.norm() / pa.norm()).log10();
        assert!((d + 9.72).abs() < 0.005, "{d} dB at {f} Hz");
    }
}

#[test]
fn wall_loss_makes_cavity_passive_and_slightly_more_compliant() {
    let lossy = sealed_cup(true, RE, BL);
    let ideal = sealed_cup(false, RE, BL);
    for f in [20.0, 200.0, 2000.0] {
        let zl = lossy
            .probe_value(&lossy.probes[1], f, &lossy.solve_at(f).unwrap())
            .unwrap();
        let zi = ideal
            .probe_value(&ideal.probes[1], f, &ideal.solve_at(f).unwrap())
            .unwrap();
        assert!(zl.re >= RE && zi.re >= RE);
        let pl = lossy
            .probe_value(&lossy.probes[0], f, &lossy.solve_at(f).unwrap())
            .unwrap();
        let pi = ideal
            .probe_value(&ideal.probes[0], f, &ideal.solve_at(f).unwrap())
            .unwrap();
        assert!(pl.norm() < pi.norm());
    }
}

#[test]
fn rejects_unit_less_and_misspelled_keys() {
    let bad = json!({
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [{"id": "c", "type": "cavity", "node": "a", "volume": 30}]
    });
    assert!(Circuit::from_json(&bad.to_string()).is_err());
    let bad = json!({
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [{"id": "c", "type": "cavity", "node": "a", "volume_cm3": 30, "wal_loss": false}]
    });
    assert!(Circuit::from_json(&bad.to_string()).is_err());
    let bad = json!({
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [{"id": "c", "type": "mass", "node": "a", "M_g": 1}]
    });
    assert!(
        Circuit::from_json(&bad.to_string()).is_err(),
        "domain mismatch"
    );
}
