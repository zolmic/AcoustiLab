//! Vents, leaks, area steps and tube end resistance (spec Sections 6, 8, 17
//! and App. C3–C4, with errata E9, E22, E23 and E45).
//!
//! Reference values come from closed forms or from the independent Python
//! implementation `tools/materials/reference.py` (mpmath Bessel functions),
//! stored in `tests/data/materials_reference.json`. All checks use the
//! spec's reference air (ρ 1.204 kg/m³, c 343 m/s, μ 1.81e-5 Pa·s, γ 1.4,
//! Pr 0.71).

use acoustilab::elements::ducts::{
    area_step_inertance, slit_end_correction, step_end_correction, surface_resistance,
};
use acoustilab::elements::materials::{Mesh, PORE_VELOCITY_WARNING};
use acoustilab::elements::Composite;
use acoustilab::{AirState, Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::PI;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn air() -> AirState {
    AirState::spec_reference()
}

fn circuit(level: u8, nodes: &[&str], elements: Value) -> Circuit {
    let nodes: Vec<Value> = nodes
        .iter()
        .map(|n| json!({"id": n, "domain": "acoustic"}))
        .collect();
    let doc = json!({
        "air": {"preset": "spec_reference"},
        "sweep": {"frequencies_Hz": [100.0]},
        "level": level,
        "nodes": nodes,
        "elements": elements,
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

fn node(c: &Circuit, f: f64, name: &str) -> C64 {
    let x = c.solve_at(f).unwrap();
    c.node_value(&x, name).unwrap()
}

/// Maximum of |h(f)| by golden-section search inside [lo, hi].
fn peak(h: &dyn Fn(f64) -> f64, mut lo: f64, mut hi: f64) -> (f64, f64) {
    let g = 0.5 * (5f64.sqrt() - 1.0);
    let mut a = hi - g * (hi - lo);
    let mut b = lo + g * (hi - lo);
    let (mut fa, mut fb) = (h(a), h(b));
    for _ in 0..100 {
        if fa > fb {
            hi = b;
            b = a;
            fb = fa;
            a = hi - g * (hi - lo);
            fa = h(a);
        } else {
            lo = a;
            a = b;
            fa = fb;
            b = lo + g * (hi - lo);
            fb = h(b);
        }
        if hi - lo < 1e-9 * hi {
            break;
        }
    }
    let f = 0.5 * (lo + hi);
    (f, h(f))
}

/// Frequency in [lo, hi] where the monotonic g crosses `target` (bisection).
fn crossing(g: &dyn Fn(f64) -> f64, target: f64, mut lo: f64, mut hi: f64) -> f64 {
    let rising = g(hi) > g(lo);
    for _ in 0..200 {
        let mid = (lo * hi).sqrt();
        if (g(mid) < target) == rising {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo * hi).sqrt()
}

/// Resonance and −3 dB-bandwidth Q of |h|.
fn resonance(h: &dyn Fn(f64) -> f64, lo: f64, hi: f64) -> (f64, f64, f64) {
    let (f0, hm) = peak(h, lo, hi);
    let target = hm / 2f64.sqrt();
    let f1 = crossing(h, target, 0.2 * f0, f0);
    let f2 = crossing(h, target, f0, 5.0 * f0);
    (f0, f0 / (f2 - f1), hm)
}

fn assert_power_balance(c: &Circuit, freqs: &[f64]) {
    for &f in freqs {
        let x = c.solve_at(f).unwrap();
        let p = c.power_absorbed(f, &x);
        assert!(p.iter().all(|(_, v)| v.is_some()), "{p:?}");
        let scale: f64 = p.iter().map(|(_, v)| v.unwrap().abs()).sum();
        let sum: f64 = p.iter().map(|(_, v)| v.unwrap()).sum();
        assert!(scale > 0.0);
        assert!(sum.abs() < 1e-10 * scale, "f = {f}: {p:?}");
        for (id, v) in &p {
            if id != "src" {
                assert!(
                    v.unwrap() >= -1e-12 * scale,
                    "{id} generates power at {f} Hz"
                );
            }
        }
    }
}

// ----- Helmholtz vents (App. C4, E22, E23) ------------------------------------

/// Lossless Helmholtz frequency c/2π·sqrt(S/(V·L_eff)).
fn helmholtz(a: f64, l_eff: f64, v: f64) -> f64 {
    let c = air().c;
    c / (2.0 * PI) * (PI * a * a / (v * l_eff)).sqrt()
}

/// |p_cav/p_ext| for an external pressure acting on the vent's outer end
/// (through its radiation impedance), cavity given by volume only.
fn vent_response(vent: Value, volume_cm3: f64) -> impl Fn(f64) -> f64 {
    let mut v = vent;
    v["id"] = json!("vent");
    v["type"] = json!("vent");
    v["nodes"] = json!(["cav", "ext"]);
    let c = circuit(
        1,
        &["cav", "ext"],
        json!([
            {"id": "src", "type": "pressure_source", "nodes": ["ext"], "p_Pa": 1.0},
            v,
            {"id": "cup", "type": "cavity", "node": "cav", "volume_cm3": volume_cm3}
        ]),
    );
    move |f| node(&c, f, "cav").norm()
}

fn vent_fixture(label: &str) -> Value {
    fixture("materials_reference.json")["vent"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["label"] == label)
        .unwrap()
        .clone()
}

#[test]
fn helmholtz_3mm_hole_2mm_wall_100cc() {
    // App. C4: 3 mm hole, 2 mm wall, 100 cm³, piston end corrections 8a/3π
    // at both ends → 215 Hz; without corrections 325 Hz (> 30 % off).
    let a = 1.5e-3;
    let piston = 8.0 * a / (3.0 * PI);
    let f_corr = helmholtz(a, 2e-3 + 2.0 * piston, 100e-6);
    let f_none = helmholtz(a, 2e-3, 100e-6);
    assert!((f_corr - 215.25).abs() < 0.05, "{f_corr}");
    assert!((f_none - 324.54).abs() < 0.05, "{f_none}");
    assert!(f_none / f_corr - 1.0 > 0.30);

    // Solved: thermoviscous neck, inner piston correction, exact baffled
    // radiation at the outer end, cavity with its thermal wall loss. The
    // viscous neck mass lowers the resonance to ≈210 Hz (−2.4 %).
    let h = vent_response(
        json!({"diameter_mm": 3, "length_mm": 2, "inlet": "piston", "end_resistance": "none"}),
        100.0,
    );
    let (f0, q, _) = resonance(&h, 150.0, 300.0);
    let r = vent_fixture("3mm_2mm_100cc_piston");
    // Same model in both: 0.05 % covers the peak search, not the physics.
    assert!((f0 / r["f0"].as_f64().unwrap() - 1.0).abs() < 5e-4, "{f0}");
    assert!((q / r["Q"].as_f64().unwrap() - 1.0).abs() < 5e-3, "{q}");
    assert!((f0 - 210.0).abs() < 1.0, "solved {f0} Hz");
    assert!(f0 < f_corr && f0 > 0.97 * f_corr);
}

#[test]
fn worked_vent_2mm_hole_1_5mm_wall_60cc() {
    // E22: both ends flanged, lossless ≈221 Hz (not 230); thermoviscous
    // ≈213 Hz; with end resistance Q ≈ 8–9 (not 13).
    let a = 1e-3;
    let l = 1.5e-3;
    let v = 60e-6;
    let lossless_piston = helmholtz(a, l + 2.0 * 8.0 * a / (3.0 * PI), v);
    let lossless_flanged = helmholtz(a, l + 2.0 * 0.8216 * a, v);
    assert!((lossless_piston - 220.9).abs() < 0.1);
    assert!((lossless_flanged - 222.8).abs() < 0.1);

    // E23: the corrections double the mass and lower the resonance by ≈30 %.
    let mass_ratio = (l + 2.0 * 0.85 * a) / l;
    assert!((mass_ratio - 2.13).abs() < 0.01);
    let drop = 1.0 - (l / (l + 2.0 * 0.85 * a)).sqrt();
    assert!((drop - 0.315).abs() < 0.005);

    for (label, end) in [
        ("2mm_1.5mm_60cc_flanged", "none"),
        ("2mm_1.5mm_60cc_flanged_maa", "maa"),
        ("2mm_1.5mm_60cc_flanged_ingard", "ingard"),
    ] {
        let h = vent_response(
            json!({"diameter_mm": 2, "length_mm": 1.5, "inlet": "flanged", "end_resistance": end}),
            60.0,
        );
        let (f0, q, _) = resonance(&h, 150.0, 300.0);
        let r = vent_fixture(label);
        assert!(
            (f0 / r["f0"].as_f64().unwrap() - 1.0).abs() < 5e-4,
            "{label}: {f0}"
        );
        assert!(
            (q / r["Q"].as_f64().unwrap() - 1.0).abs() < 5e-3,
            "{label}: {q}"
        );
        assert!((f0 - 213.0).abs() < 1.5, "{label}: {f0} Hz");
        match end {
            // Neck and wall loss only.
            "none" => assert!((q - 11.1).abs() < 0.2, "{q}"),
            // E22's "Q ≈ 8–9": Maa's end resistance (R_s in total).
            "maa" => assert!(q > 8.0 && q < 9.0, "{q}"),
            // Ingard's sharp-edge value (R_s per end) is a little lower.
            _ => assert!((q - 7.4).abs() < 0.2, "{q}"),
        }
    }
}

#[test]
fn mesh_160_rayl_turns_the_vent_into_a_resistive_leak() {
    // Spec p. 23: a 160 rayl mesh raises the resistance about thirty-fold
    // above the mass reactance. Mesh resistance over the hole area against
    // the vent's own reactance at its (unmeshed) resonance.
    let doc = |mesh: bool| {
        let mut v = json!({"id": "vent", "type": "vent", "nodes": ["cav"],
            "diameter_mm": 2, "length_mm": 1.5, "inlet": "flanged"});
        if mesh {
            v["mesh"] = json!({"R_s_rayl": 160});
        }
        // Level 0: the neck is lumped, so series terms add exactly.
        circuit(
            0,
            &["cav"],
            json!([{"id": "src", "type": "flow_source", "nodes": ["cav"]}, v]),
        )
    };
    let (open, meshed) = (doc(false), doc(true));
    // The flow source delivers 1e-6 m³/s into the vent alone.
    let z = |c: &Circuit, f: f64| node(c, f, "cav") / 1e-6;
    let s = PI * 1e-6;
    for f in [213.0, 221.0, 230.0] {
        let zo = z(&open, f);
        let zm = z(&meshed, f);
        // The mesh adds exactly 160 rayl over the hole area.
        assert!(((zm - zo).re / (160.0 / s) - 1.0).abs() < 1e-9);
        assert!((zm - zo).im.abs() < 1e-9 * zm.norm());
        let ratio = (160.0 / s) / zo.im;
        assert!(ratio > 27.0 && ratio < 31.0, "R/ωM = {ratio} at {f} Hz");
    }
    // Resistive leak: with a 60 cm³ cup the meshed vent shows no peak.
    let h = vent_response(
        json!({"diameter_mm": 2, "length_mm": 1.5, "inlet": "flanged", "mesh": {"R_s_rayl": 160}}),
        60.0,
    );
    for i in 0..60 {
        let f = 20.0 * 1.1f64.powi(i);
        assert!(h(f) <= 1.0 + 1e-9, "{f} Hz: {}", h(f));
    }
}

#[test]
fn vent_mesh_pore_velocity_is_readable() {
    // Spec: particle velocity above about 1 m/s in a vent flags the laminar
    // assumption. The vent's mesh is a `Mesh` part, so the helper applies.
    // L0: the neck carries no compressibility, so the mesh passes exactly
    // the source flow.
    let c = circuit(
        0,
        &["cav"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["cav"], "U_m3_per_s": 2e-6},
            {"id": "v", "type": "vent", "nodes": ["cav"], "diameter_mm": 2, "length_mm": 1.5,
             "count": 2, "mesh": {"R_s_rayl": 160, "thickness_um": 60, "open_area": 0.3}}
        ]),
    );
    let f = 200.0;
    let x = c.solve_at(f).unwrap();
    let cx = c.cx(f);
    let vent = c.elements[c.element_index("v").unwrap()]
        .as_any()
        .downcast_ref::<Composite>()
        .unwrap();
    let mesh = vent
        .parts
        .iter()
        .find_map(|p| p.as_any().downcast_ref::<Mesh>())
        .unwrap();
    assert_eq!(mesh.id, "v.mesh");
    // The mesh covers both holes by default: |U|/(φ·2πa²).
    let expect = 2e-6 / (0.3 * 2.0 * PI * 1e-6);
    let v = mesh.pore_velocity(&cx, &x).unwrap();
    assert!((v / expect - 1.0).abs() < 1e-9, "{v} vs {expect}");
    // 2 cm³/s through 30 % of two 2 mm holes is 1.06 m/s: flagged.
    assert!(v > PORE_VELOCITY_WARNING);
    assert_power_balance(&c, &[50.0, 500.0, 5000.0]);
}

#[test]
fn vent_macro_equals_tube_plus_radiation() {
    // The macro is a tube with the inner correction, the outer end
    // resistance and the baffled radiation impedance in series.
    let elements = |macro_: bool, end: &str| {
        if macro_ {
            json!([
                {"id": "src", "type": "flow_source", "nodes": ["cav"]},
                {"id": "v", "type": "vent", "nodes": ["cav"], "radius_mm": 1.2,
                 "length_mm": 3, "count": 3, "inlet": "unflanged", "end_resistance": end}
            ])
        } else {
            json!([
                {"id": "src", "type": "flow_source", "nodes": ["cav"]},
                {"id": "t", "type": "tube", "nodes": ["cav", "m"], "radius_mm": 1.2,
                 "length_mm": 3, "count": 3, "inlet": "unflanged", "end_resistance": end},
                {"id": "r", "type": "radiation", "nodes": ["m"], "radius_mm": 1.2, "count": 3}
            ])
        }
    };
    let air = air();
    // L1 without end resistance: identical networks. L0 with Ingard's end
    // resistance: the macro adds R_s/S per hole at the outer end as well,
    // which at L0 adds directly to the input impedance.
    for (level, end) in [(1u8, "none"), (0, "ingard")] {
        let a = circuit(level, &["cav"], elements(true, end));
        let b = circuit(level, &["cav", "m"], elements(false, end));
        for f in [50.0, 500.0, 5000.0] {
            let pa = node(&a, f, "cav");
            let pb = node(&b, f, "cav");
            let r_out = if end == "none" {
                0.0
            } else {
                surface_resistance(&air, 2.0 * PI * f) / (PI * 1.2e-3f64.powi(2)) / 3.0
            };
            let expect = pb + 1e-6 * r_out;
            assert!(
                (pa - expect).norm() < 1e-9 * pa.norm(),
                "L{level} {f}: {pa} vs {expect}"
            );
        }
    }
}

// ----- Slit worked examples (App. C3, E45) --------------------------------------

/// C3 transfer H = p_leaky/p_sealed for a flow source into a lossless 30 cm³
/// cavity shunted by an element.
fn leak_transfer(level: u8, leak: Value) -> impl Fn(f64) -> C64 {
    let mut l = leak;
    l["id"] = json!("leak");
    l["nodes"] = json!(["cav"]);
    let c = circuit(
        level,
        &["cav"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["cav"], "U_m3_per_s": 1e-6},
            {"id": "cup", "type": "cavity", "node": "cav", "volume_cm3": 30, "wall_loss": false},
            l
        ]),
    );
    let cap = 30e-6 / air().bulk_modulus();
    move |f| {
        let p = node(&c, f, "cav");
        p * C64::new(0.0, 2.0 * PI * f) * cap / 1e-6
    }
}

#[test]
fn slit_1mm_resonates_near_523_hz_with_low_q() {
    // E45: the exact slit puts the peak at ≈523 Hz ("near 550"), with a Q
    // far below the Poiseuille estimate because R grows as sqrt(f).
    let r = &fixture("materials_reference.json")["slit_examples"][0];
    let slit = json!({"type": "slit", "gap_mm": 1, "width_mm": 30, "length_mm": 10});
    let h0 = leak_transfer(0, slit.clone());
    let (f0, q, _) = resonance(&|f| h0(f).norm(), 300.0, 900.0);
    assert!((f0 / r["f0"].as_f64().unwrap() - 1.0).abs() < 5e-4, "{f0}");
    assert!((q / r["Q"].as_f64().unwrap() - 1.0).abs() < 5e-3, "{q}");
    assert!((f0 / 523.0 - 1.0).abs() < 0.05, "{f0}");
    let q_pois = r["Q_poiseuille"].as_f64().unwrap();
    assert!((q_pois - 20.8).abs() < 0.1);
    assert!(q < 0.5 * q_pois, "Q {q} vs Poiseuille {q_pois}");
    // The distributed slit adds the compressibility of its own air (1 % of
    // the cup): same peak within 1 %.
    let h1 = leak_transfer(1, slit);
    let (f1, _, _) = resonance(&|f| h1(f).norm(), 300.0, 900.0);
    assert!((f1 / f0 - 1.0).abs() < 0.01, "{f1} vs {f0}");
    // Inertive above a few tens of hertz: R = ωM near 24 Hz (review value).
    let air = air();
    let z = |f: f64| {
        acoustilab::thermoviscous::lumped_series_impedance(
            &acoustilab::thermoviscous::Section::Slit {
                gap: 1e-3,
                width: 30e-3,
            },
            &air,
            2.0 * PI * f,
            10e-3,
        )
    };
    let cross = crossing(&|f| z(f).im / z(f).re, 1.0, 5.0, 200.0);
    assert!((cross - 24.0).abs() < 1.0, "R = ωM at {cross} Hz");
}

#[test]
fn slit_0_2mm_is_a_first_order_leak_with_83_hz_rc_corner() {
    // E45: corner defined as 1/(2π·R_pois·C) = 83 Hz; the solved −3 dB point
    // is 73 Hz and the response overshoots by 0.78 dB, no resonance above
    // +1 dB.
    let r = &fixture("materials_reference.json")["slit_examples"][1];
    let air = air();
    let r_pois = 12.0 * air.mu * 10e-3 / (30e-3 * (0.2e-3f64).powi(3));
    let c = 30e-6 / air.bulk_modulus();
    let f_rc = 1.0 / (2.0 * PI * r_pois * c);
    assert!((f_rc - 83.0).abs() < 0.1, "{f_rc}");
    let h = leak_transfer(
        0,
        json!({"type": "slit", "gap_mm": 0.2, "width_mm": 30, "length_mm": 10}),
    );
    let f3 = crossing(&|f| h(f).norm(), 0.5f64.sqrt(), 5.0, 500.0);
    assert!(
        (f3 / r["f_minus3db"].as_f64().unwrap() - 1.0).abs() < 1e-3,
        "{f3}"
    );
    assert!((f3 - 73.0).abs() < 0.5);
    let mut max_db: f64 = -100.0;
    for i in 0..400 {
        let f = 5.0 * 1000f64.powf(i as f64 / 399.0);
        max_db = max_db.max(20.0 * h(f).norm().log10());
    }
    assert!(max_db < 1.0, "overshoot {max_db} dB");
    assert!((max_db - r["max_gain_db"].as_f64().unwrap()).abs() < 0.01);
    // Six dB per octave well below the corner.
    let slope = 20.0 * (h(f_rc / 8.0).norm() / h(f_rc / 16.0).norm()).log10();
    assert!((slope - 6.02).abs() < 0.3, "{slope}");
}

// ----- Leak slopes (App. C3, E9) ----------------------------------------------------

#[test]
fn leak_slopes_match_first_and_second_order_high_passes() {
    // E9: the asymptotic slopes hold within 0.3 dB/oct only well below the
    // corner, over a band ending at fc/8; elsewhere compare with the exact H.
    let air = air();
    let c = 30e-6 / air.bulk_modulus();
    let r = 5e6;
    let m = 400.0;
    let fc_r = 1.0 / (2.0 * PI * r * c);
    let fc_m = 1.0 / (2.0 * PI * (m * c).sqrt());
    let slope = |h: &dyn Fn(f64) -> C64, f_end: f64| {
        // Mean slope over the two octaves ending at f_end.
        20.0 * (h(f_end).norm() / h(f_end / 4.0).norm()).log10() / 2.0
    };
    let hr = leak_transfer(
        0,
        json!({"type": "acoustic_resistance", "R_Pa_s_per_m3": r}),
    );
    let hm = leak_transfer(0, json!({"type": "acoustic_inertance", "M_kg_per_m4": m}));
    let hrm = leak_transfer(
        0,
        json!({"type": "acoustic_impedance", "R_Pa_s_per_m3": 2e4, "M_kg_per_m4": m}),
    );
    let s_r = slope(&hr, fc_r / 8.0);
    let s_m = slope(&hm, fc_m / 8.0);
    assert!((s_r - 6.02).abs() < 0.3, "{s_r}");
    assert!((s_m - 12.04).abs() < 0.3, "{s_m}");
    // The spec's "two octaves below the corner" band fails for exact
    // high-passes (E9): the first-order slope there is below 6 − 0.3 dB/oct.
    assert!((slope(&hr, fc_r) - 6.02).abs() > 0.3);
    // Exact comparison with H = Z_L/(Z_L + 1/(jωC)) everywhere.
    for i in 0..50 {
        let f = 10.0 * 1.15f64.powi(i);
        let jw = C64::new(0.0, 2.0 * PI * f);
        let zc = (jw * c).inv();
        for (h, zl) in [
            (&hr as &dyn Fn(f64) -> C64, C64::new(r, 0.0)),
            (&hm, jw * m),
            (&hrm, jw * m + 2e4),
        ] {
            let exact = zl / (zl + zc);
            assert!((h(f) - exact).norm() < 1e-9 * exact.norm(), "{f}");
        }
    }
    // Mixed leak: resistive-to-inertive corner R/(2πM) = 8 Hz, a decade
    // below the peak; −3 dB-bandwidth Q within 5 % of ω0·M/R.
    let (f0, q, _) = resonance(&|f| hrm(f).norm(), 0.5 * fc_m, 2.0 * fc_m);
    let q_formula = 2.0 * PI * fc_m * m / 2e4;
    assert!(2e4 / (2.0 * PI * m) < f0 / 10.0);
    assert!((q / q_formula - 1.0).abs() < 0.05, "{q} vs {q_formula}");
}

// ----- Leak macro ----------------------------------------------------------------------

#[test]
fn leak_macro_uniform_segments_equal_one_wide_slit() {
    for level in [0u8, 1] {
        let one = leak_transfer(
            level,
            json!({"type": "slit", "gap_mm": 0.3, "width_mm": 200, "length_mm": 8}),
        );
        let eight = leak_transfer(
            level,
            json!({"type": "leak", "gap_mm": 0.3, "perimeter_mm": 200, "depth_mm": 8, "ends": "none"}),
        );
        let listed = leak_transfer(
            level,
            json!({"type": "leak", "gaps_mm": [0.3, 0.3, 0.3, 0.3], "perimeter_mm": 200,
                   "depth_mm": 8, "ends": "none"}),
        );
        for f in [20.0, 200.0, 2000.0, 15000.0] {
            let (a, b, c) = (one(f), eight(f), listed(f));
            assert!((a - b).norm() < 1e-9 * a.norm(), "L{level} {f}: {a} {b}");
            assert!((a - c).norm() < 1e-9 * a.norm(), "L{level} {f}: {a} {c}");
        }
    }
}

#[test]
fn leak_macro_sealed_segments_and_end_corrections() {
    // Two of four segments sealed: same as two open segments of the same
    // breadth (a 100 mm slit made of two 50 mm segments).
    let partial = leak_transfer(
        1,
        json!({"type": "leak", "gaps_mm": [0.5, 0, 0.5, 0], "perimeter_mm": 200,
               "depth_mm": 8, "ends": "none"}),
    );
    let half = leak_transfer(
        1,
        json!({"type": "slit", "gap_mm": 0.5, "width_mm": 50, "length_mm": 8, "count": 2}),
    );
    for f in [30.0, 300.0, 3000.0] {
        assert!((partial(f) - half(f)).norm() < 1e-9 * half(f).norm());
    }
    // Fully sealed: the cavity behaves as sealed (H = 1).
    let sealed = leak_transfer(
        1,
        json!({"type": "leak", "gaps_mm": [0, 0, 0, 0], "perimeter_mm": 200, "depth_mm": 8}),
    );
    assert!((sealed(100.0) - C64::new(1.0, 0.0)).norm() < 1e-12);
    // Flanged ends add the rectangular-piston mass at each end and lower the
    // leak's Helmholtz frequency: 8 × (1 mm × 25 mm) segments, depth 8 mm.
    let bare = leak_transfer(
        0,
        json!({"type": "leak", "gap_mm": 1, "perimeter_mm": 200, "depth_mm": 8, "ends": "none"}),
    );
    let ends = leak_transfer(
        0,
        json!({"type": "leak", "gap_mm": 1, "perimeter_mm": 200, "depth_mm": 8}),
    );
    let (fb, _, _) = resonance(&|f| bare(f).norm(), 200.0, 3000.0);
    let (fe, _, _) = resonance(&|f| ends(f).norm(), 200.0, 3000.0);
    let delta = slit_end_correction(1e-3, 25e-3);
    // Lossless-mass estimate of the shift, sqrt(l/(l + 2δ)); thermoviscous
    // mass shifts both peaks alike, so the ratio agrees within 2 %.
    let expect = (8e-3 / (8e-3 + 2.0 * delta)).sqrt();
    assert!(
        (fe / fb / expect - 1.0).abs() < 0.02,
        "{fe}/{fb} vs {expect}"
    );
}

#[test]
fn leak_macro_rejects_bad_input() {
    let bad = |leak: Value| {
        let mut l = leak;
        l["id"] = json!("leak");
        l["type"] = json!("leak");
        l["nodes"] = json!(["cav"]);
        let doc = json!({"nodes": [{"id": "cav", "domain": "acoustic"}], "elements": [l]});
        Circuit::from_json(&doc.to_string()).is_err()
    };
    // Segment breadth 2.5 mm < 5 × 1 mm gap.
    assert!(bad(
        json!({"gap_mm": 1, "perimeter_mm": 20, "depth_mm": 8, "segments": 8})
    ));
    assert!(bad(
        json!({"gap_mm": 1, "gaps_mm": [1, 1], "perimeter_mm": 200, "depth_mm": 8})
    ));
    assert!(bad(
        json!({"gaps_mm": [1, 1], "segments": 3, "perimeter_mm": 200, "depth_mm": 8})
    ));
    assert!(bad(
        json!({"gaps_mm": [1, -1], "perimeter_mm": 200, "depth_mm": 8})
    ));
    assert!(bad(json!({"gap": 1, "perimeter_mm": 200, "depth_mm": 8})));
}

#[test]
fn micron_leak_gaps_solve_at_level_1() {
    // A pad leak of a few µm is a very lossy line: Re Γl ∝ sqrt(f)/gap
    // reaches 33 inside the audio band (1 µm at 1.6 kHz, 2 µm at 6.4 kHz,
    // 3 µm at 14.5 kHz), where the transmission-form rows lost the input
    // relation and the solve was reported singular. It is stamped in
    // admittance form there.
    let doc = |gap_um: f64, level: u8| {
        json!({"schema": "acoustilab-netlist/0.2",
               "sweep": {"f_min_Hz": 10, "f_max_Hz": 20000, "points_per_octave": 96},
               "level": level,
               "nodes": [{"id": "a", "domain": "acoustic"}],
               "elements": [
                   {"id": "src", "type": "flow_source", "node": "a", "U_m3_per_s": 1e-6},
                   {"id": "c", "type": "cavity", "node": "a", "volume_cm3": 80},
                   {"id": "leak", "type": "leak", "nodes": ["a", "ambient"], "perimeter_mm": 245,
                    "depth_mm": 10, "gap_mm": gap_um * 1e-3, "segments": 8}],
               "probes": [{"id": "p", "quantity": "pressure", "node": "a"},
                          {"id": "u", "quantity": "flow", "element": "leak", "port": 0}]})
    };
    for gap_um in [1.0, 2.0, 3.0] {
        let c0 = Circuit::from_json(&doc(gap_um, 0).to_string()).unwrap();
        let c1 = Circuit::from_json(&doc(gap_um, 1).to_string()).unwrap();
        let r0 = c0.solve().unwrap();
        let r1 = c1
            .solve()
            .unwrap_or_else(|e| panic!("{gap_um} µm at L1: {e}"));
        let at = |r: &acoustilab::SolveResult, id: &str, i: usize| r.probe(id).unwrap().values[i];
        // The cavity sets the pressure at 10 Hz: the levels agree there to
        // within the leak's share of the flow (1e-5 to 1e-4; the leak is
        // already past its lumped limit, so its two models differ).
        let (p0, p1) = (at(&r0, "p", 0), at(&r1, "p", 0));
        assert!((p1 / p0 - 1.0).norm() < 1e-4, "{gap_um} µm: {p1} vs {p0}");
        // The leak itself agrees below its lumped-validity frequency: the
        // inertance error |tan x/x − 1| ≈ |x|²/3 is 10 % there and 1 % at a
        // tenth of it (x² ∝ f).
        let begin = c0
            .validity()
            .into_iter()
            .find(|l| l.element == "leak")
            .and_then(|l| l.begin_hz)
            .unwrap();
        let y = |c: &Circuit, f: f64| {
            let x = c.solve_at(f).unwrap();
            let cx = c.cx(f);
            let leak = c.element_index("leak").unwrap();
            let u = c.elements[leak]
                .port_flow(&cx, &x, &c.branches(leak), 0)
                .unwrap();
            u / c.node_value(&x, "a").unwrap()
        };
        let f = begin / 10.0;
        let (y0, y1) = (y(&c0, f), y(&c1, f));
        assert!(
            (y1 / y0 - 1.0).norm() < 0.012,
            "{gap_um} µm at {f} Hz: {y1} vs {y0}"
        );
        // At 20 kHz each segment's line is matched: its input impedance is
        // Z_c plus the inner end's mass, whatever terminates it.
        let n = r1.freqs_hz.len() - 1;
        let f = r1.freqs_hz[n];
        let omega = 2.0 * PI * f;
        let (g, w) = (gap_um * 1e-6, 245e-3 / 8.0);
        let (gamma, zc) = acoustilab::thermoviscous::propagation(
            &acoustilab::thermoviscous::Section::Slit { gap: g, width: w },
            &c1.air,
            omega,
        );
        assert!(gamma.re * 10e-3 > 30.0, "{gap_um} µm: Γl {}", gamma * 10e-3);
        let z_end = C64::new(
            0.0,
            omega * c1.air.rho * slit_end_correction(g, w) / (g * w),
        );
        let matched = 8.0 / (zc + z_end);
        let y1 = at(&r1, "u", n) / at(&r1, "p", n);
        assert!(
            (y1 / matched - 1.0).norm() < 1e-12,
            "{gap_um} µm: {y1} vs {matched}"
        );
    }
}

#[test]
fn slit_end_correction_closed_form() {
    // Square piston: ∬∬ dS dS'/|r − r'| = 2.97321 for a unit square
    // (checked by direct quadrature in tools), so δ = 2.97321/(2π).
    let d = slit_end_correction(1.0, 1.0);
    assert!((d - 2.973_209_598 / (2.0 * PI)).abs() < 1e-9, "{d}");
    // Narrow slit: (h/π)·[ln(2w/h) + 1/2] for w ≫ h.
    let (h, w) = (1e-4, 0.1);
    let asym = h / PI * ((2.0 * w / h).ln() + 0.5);
    assert!((slit_end_correction(h, w) / asym - 1.0).abs() < 1e-3);
    // 1 mm × 30 mm: 1.466 mm per end.
    assert!((slit_end_correction(1e-3, 30e-3) - 1.4659e-3).abs() < 1e-7);
}

// ----- Tube end resistance ---------------------------------------------------------

#[test]
fn tube_end_resistance_adds_rs_per_corrected_end() {
    let air = air();
    let z_of = |end: &str| {
        let c = circuit(
            0,
            &["a"],
            json!([
                {"id": "src", "type": "flow_source", "nodes": ["a"]},
                {"id": "t", "type": "tube", "nodes": ["a"], "radius_mm": 1, "length_mm": 5,
                 "inlet": "flanged", "outlet": "flanged", "end_resistance": end}
            ]),
        );
        move |f: f64| node(&c, f, "a") / 1e-6
    };
    let (none, maa, ingard) = (z_of("none"), z_of("maa"), z_of("ingard"));
    let s = PI * 1e-6;
    for f in [100.0, 1000.0, 10000.0] {
        let rs = surface_resistance(&air, 2.0 * PI * f) / s;
        assert!(((maa(f) - none(f)).re / rs - 1.0).abs() < 1e-9);
        assert!(((ingard(f) - none(f)).re / (2.0 * rs) - 1.0).abs() < 1e-9);
        assert!((ingard(f) - none(f)).im.abs() < 1e-9 * none(f).norm());
    }
    // R_s = ½·sqrt(2μρω).
    let w = 2.0 * PI * 1000.0;
    assert!(
        (surface_resistance(&air, w) - 0.5 * (2.0 * air.mu * air.rho * w).sqrt()).abs() < 1e-15
    );
    // No corrected end → nothing to attach it to.
    let doc = json!({"nodes": [{"id": "a", "domain": "acoustic"}], "elements": [
        {"id": "t", "type": "tube", "nodes": ["a"], "radius_mm": 1, "length_mm": 5,
         "end_resistance": "ingard"}]});
    assert!(Circuit::from_json(&doc.to_string()).is_err());
}

// ----- Area step ---------------------------------------------------------------------

#[test]
fn area_step_matches_mode_matching_table() {
    // Independent variational mode-matching solution
    // (tools/materials/end_corrections.py): the fit stays within 3e-4·a.
    let t = fixture("materials_end_corrections.json");
    let alphas = t["alpha"].as_array().unwrap();
    let step = t["delta_step_over_a"].as_array().unwrap();
    for (a, d) in alphas.iter().zip(step) {
        let (a, d) = (a.as_f64().unwrap(), d.as_f64().unwrap());
        let fit = step_end_correction(a);
        assert!((fit - d).abs() < 3e-4, "alpha {a}: {fit} vs {d}");
    }
    // Small-α limit: the flanged-pipe end correction 0.8216a (Norris & Sheng),
    // below Karal's piston value 8a/3π.
    assert!((step_end_correction(0.0) - 0.82159).abs() < 1e-12);
    assert!(step_end_correction(0.0) < 8.0 / (3.0 * PI));
    assert_eq!(step_end_correction(1.0), 0.0);
}

#[test]
fn area_step_is_a_series_inertance() {
    let air = air();
    let c = circuit(
        1,
        &["a", "b"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["a"]},
            {"id": "step", "type": "area_step", "nodes": ["a", "b"], "diameter1_mm": 2, "radius2_mm": 5},
            {"id": "r", "type": "acoustic_resistance", "nodes": ["b"], "R_Pa_s_per_m3": 1e6}
        ]),
    );
    let m = area_step_inertance(air.rho, 1e-3, 5e-3);
    assert_eq!(m, area_step_inertance(air.rho, 5e-3, 1e-3));
    let expect_m = air.rho * step_end_correction(0.2) * 1e-3 / (PI * 1e-6);
    assert!((m / expect_m - 1.0).abs() < 1e-12);
    for f in [100.0, 1000.0] {
        let x = c.solve_at(f).unwrap();
        let dp = c.node_value(&x, "a").unwrap() - c.node_value(&x, "b").unwrap();
        let z = dp / 1e-6;
        assert!((z - C64::new(0.0, 2.0 * PI * f * m)).norm() < 1e-9 * z.norm());
    }
    assert_power_balance(&c, &[100.0, 3000.0]);
}

// ----- Power balance -----------------------------------------------------------------

#[test]
fn power_balances_with_vents_leaks_and_steps() {
    for level in [0u8, 1] {
        let c = circuit(
            level,
            &["front", "rear", "ext", "m"],
            json!([
                {"id": "src", "type": "flow_source", "nodes": ["front"], "U_m3_per_s": 1e-5},
                {"id": "fc", "type": "cavity", "node": "front", "volume_cm3": 20},
                {"id": "rc", "type": "cavity", "node": "rear", "volume_cm3": 40},
                {"id": "pad", "type": "leak", "nodes": ["front", "ext"], "perimeter_mm": 180,
                 "depth_mm": 10, "gaps_mm": [0.2, 0.1, 0, 0.4, 0.3, 0.1, 0, 0.6]},
                {"id": "baffle", "type": "vent", "nodes": ["front", "rear"], "diameter_mm": 1,
                 "length_mm": 1, "count": 4, "mesh": {"R_s_rayl": 260, "thickness_um": 60, "open_area": 0.13}},
                {"id": "port", "type": "tube", "nodes": ["rear", "m"], "radius_mm": 1.5,
                 "length_mm": 6, "inlet": "flanged", "end_resistance": "ingard"},
                {"id": "step", "type": "area_step", "nodes": ["m", "ext"], "radius1_mm": 1.5, "radius2_mm": 4},
                {"id": "rad", "type": "radiation", "nodes": ["ext"], "radius_mm": 4}
            ]),
        );
        assert_power_balance(&c, &[20.0, 250.0, 2500.0, 12000.0]);
    }
}
