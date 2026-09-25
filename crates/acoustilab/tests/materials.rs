//! Materials: meshes, perforated plates, porous layers, fibrous fill and
//! membrane vents (spec Sections 6, 13, 17 and App. C6; erratum E11).
//!
//! Reference values come from closed forms, from the independent Python
//! implementation `tools/materials/reference.py` (mpmath Bessel functions)
//! stored in `tests/data/materials_reference.json`, and from the
//! variational end-correction solution in
//! `tests/data/materials_end_corrections.json`. Tolerances are stated at
//! each check with the reason for them.

use acoustilab::elements::ducts::{surface_resistance, EndResistance};
use acoustilab::elements::materials::{
    database, fibre_relaxation_time, fok, maa_impedance, material, FillModel, MembraneVentModel,
    Mesh, MeshModel, PerforateModel, PorousLayer, PorousModel, BADGE_BAND, PORE_VELOCITY_WARNING,
};
use acoustilab::{AirState, Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::PI;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn reference() -> Value {
    fixture("materials_reference.json")
}

fn air() -> AirState {
    AirState::spec_reference()
}

fn c(v: &Value) -> C64 {
    C64::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

/// Rust and mpmath implementations of the same formulas: 1e-9 relative
/// covers the double-precision Bessel ratios, nothing else.
const SAME_MODEL: f64 = 1e-9;

fn close(a: C64, b: C64, tol: f64) -> bool {
    (a - b).norm() <= tol * b.norm()
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

fn builds(element: Value) -> bool {
    let doc = json!({
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [element],
    });
    Circuit::from_json(&doc.to_string()).is_ok()
}

/// Input impedance p/U at node `a` driven by the default 1e-6 m³/s source.
fn input_impedance(c: &Circuit, f: f64) -> C64 {
    let x = c.solve_at(f).unwrap();
    c.node_value(&x, "a").unwrap() / 1e-6
}

// ----- Mesh ----------------------------------------------------------------------

fn mesh_160() -> MeshModel {
    MeshModel::calibrate(
        160.0,
        1e-4,
        Some(60e-6),
        Some(0.3),
        None,
        air().mu,
        EndResistance::Maa,
    )
    .unwrap()
}

fn mesh_003() -> MeshModel {
    MeshModel::calibrate(
        3.0,
        1e-4,
        None,
        Some(0.44),
        Some(285e-6),
        air().mu,
        EndResistance::Maa,
    )
    .unwrap()
}

#[test]
fn mesh_is_rs_over_area_at_dc() {
    let air = air();
    // Pore models: the thermoviscous pores tend to Poiseuille and the end
    // resistance, which grows as sqrt(f), vanishes. The deviation from R_s/A
    // is the end resistance R_s,Ingard/(φA) alone (the tube's own correction
    // is O(f²)), so it falls by 10 per factor 100 in frequency.
    for m in [mesh_160(), mesh_003()] {
        let phi = m.pores.unwrap().open_area;
        for f in [1e-8, 1e-4] {
            let w = 2.0 * PI * f;
            let z = m.impedance(&air, w);
            let end = surface_resistance(&air, w) / (phi * m.area);
            assert!(
                (z.re - m.dc_resistance() - end).abs() < 1e-9 * z.re,
                "{f}: {z}"
            );
            assert!(z.im.abs() < 1e-5 * z.re);
        }
        // At 1e-8 Hz what is left of the end resistance is below 1e-6 of
        // R_s/A even for the 3 rayl mesh.
        let z = m.impedance(&air, 2.0 * PI * 1e-8);
        assert!((z.re / m.dc_resistance() - 1.0).abs() < 1e-6, "{z}");
        assert!((m.dc_resistance() - m.r_s / 1e-4).abs() < 1e-9 * m.dc_resistance());
    }
    // Rayl value alone: a pure resistance.
    let pure =
        MeshModel::calibrate(70.0, 2e-4, None, None, None, air.mu, EndResistance::Maa).unwrap();
    assert_eq!(
        pure.impedance(&air, 2.0 * PI * 5000.0),
        C64::new(70.0 / 2e-4, 0.0)
    );
    // The same through a netlist.
    let circ = circuit(
        1,
        &["a"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["a"]},
            {"id": "m", "type": "mesh", "nodes": ["a"], "R_s_rayl": 160, "area_cm2": 1,
             "thickness_um": 60, "open_area": 0.3}
        ]),
    );
    let z = input_impedance(&circ, 1e-8);
    assert!((z.re / 1.6e6 - 1.0).abs() < 1e-7);
}

#[test]
fn mesh_pores_follow_poiseuille_calibration() {
    let air = air();
    let m = mesh_160();
    let p = m.pores.unwrap();
    // R_s = 8μt/(φa²) by construction.
    assert!(
        (8.0 * air.mu * p.thickness / (p.open_area * p.radius.powi(2)) / 160.0 - 1.0).abs() < 1e-12
    );
    // Any two geometry inputs give the same pores.
    let d = 2.0 * p.radius;
    let alt = MeshModel::calibrate(
        160.0,
        1e-4,
        Some(60e-6),
        None,
        Some(d),
        air.mu,
        EndResistance::Maa,
    )
    .unwrap();
    assert!((alt.pores.unwrap().open_area - 0.3).abs() < 1e-12);
    // Low-frequency mass: Poiseuille 4/3 factor on the pore length plus the
    // Fok-reduced piston end corrections, all pores in parallel.
    let n = m.pore_count().unwrap();
    let s = PI * p.radius * p.radius;
    let delta = 8.0 * p.radius / (3.0 * PI) * fok(0.3f64.sqrt());
    let m_expect = (4.0 / 3.0 * air.rho * p.thickness + 2.0 * air.rho * delta) / s / n;
    let w = 2.0 * PI * 1.0;
    let m_solved = m.impedance(&air, w).im / w;
    // O(shear number²) ≈ 1e-6 at 1 Hz.
    assert!(
        (m_solved / m_expect - 1.0).abs() < 1e-4,
        "{m_solved} vs {m_expect}"
    );
}

#[test]
fn mesh_matches_independent_reference() {
    let air = air();
    let r = reference();
    for (case, model) in r["mesh"]
        .as_array()
        .unwrap()
        .iter()
        .zip([mesh_160(), mesh_003()])
    {
        let pores = model.pores.unwrap();
        assert!((pores.radius / case["radius"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert!((pores.thickness / case["thickness"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        for (f, z) in f64s(&case["freqs"])
            .iter()
            .zip(case["z"].as_array().unwrap())
        {
            let zr = model.impedance(&air, 2.0 * PI * f);
            assert!(close(zr, c(z), SAME_MODEL), "{f}: {zr} vs {z}");
        }
    }
}

#[test]
fn mesh_badge_flat_for_fine_meshes_not_for_coarse() {
    // Spec Section 6: meshes of 30 rayl and above are resistive-flat; coarse
    // meshes under 10 rayl are not. With the spec's Saati end points:
    let air = air();
    let fine = |id: &str| {
        let doc = json!({"id": "m", "type": "mesh", "nodes": ["a"], "material": id, "area_cm2": 1});
        let circ = circuit(1, &["a"], json!([doc]));
        let i = circ.element_index("m").unwrap();
        circ.elements[i]
            .as_any()
            .downcast_ref::<Mesh>()
            .unwrap()
            .model
    };
    let m260 = fine("saati_acoustex_260");
    let m003 = fine("saati_acoustex_003");
    let b260 = m260.badge(&air, BADGE_BAND);
    let b003 = m003.badge(&air, BADGE_BAND);
    assert!(b260.flat, "260 rayl changes {} dB", b260.change_db);
    assert!(b260.change_db < 0.5);
    assert!(!b003.flat);
    assert!(b003.change_db > 6.0, "3 rayl changes {} dB", b003.change_db);
    // The rise is the thermoviscous √f law. For shear numbers s ≫ 1 the pore
    // resistance tends to (t/a)·sqrt(2μρω) + 4μt/a² (specific, per pore
    // area), plus Maa's end resistance ½·sqrt(2μρω); the remainder of the
    // asymptotic series is O(1/s), bounded here by 0.5/s.
    let p = m003.pores.unwrap();
    let (a, t) = (p.radius, p.thickness);
    let n = m003.pore_count().unwrap();
    for f in [20_000.0, 80_000.0] {
        let w = 2.0 * PI * f;
        let s = a * (w * air.rho / air.mu).sqrt();
        let asym = ((t / a + 0.5) * (2.0 * air.mu * air.rho * w).sqrt()
            + 4.0 * air.mu * t / (a * a))
            / (PI * a * a)
            / n;
        let r = m003.impedance(&air, w).re / asym - 1.0;
        assert!(r.abs() < 0.5 / s, "s = {s}: {r}");
    }
}

#[test]
fn mesh_pore_velocity_helper() {
    let circ = circuit(
        1,
        &["a"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["a"], "U_m3_per_s": 1e-5},
            {"id": "m", "type": "mesh", "nodes": ["a"], "material": "saati_acoustex_260",
             "area_cm2": 1}
        ]),
    );
    let x = circ.solve_at(1000.0).unwrap();
    let cx = circ.cx(1000.0);
    let mesh = circ.elements[1].as_any().downcast_ref::<Mesh>().unwrap();
    // All the source flow passes the mesh: |U|/(φA).
    let v = mesh.pore_velocity(&cx, &x).unwrap();
    assert!((v - 1e-5 / (0.13 * 1e-4)).abs() < 1e-9 * v);
    assert!(v < PORE_VELOCITY_WARNING);
    assert!(mesh.model.pore_velocity(C64::new(1e-4, 0.0)).unwrap() > PORE_VELOCITY_WARNING);
    // Without pore geometry the open area is unknown.
    let pure =
        MeshModel::calibrate(30.0, 1e-4, None, None, None, 1.81e-5, EndResistance::Maa).unwrap();
    assert!(pure.pore_velocity(C64::new(1.0, 0.0)).is_none());
}

#[test]
fn mesh_rejects_bad_input() {
    let base = json!({"id": "m", "type": "mesh", "nodes": ["a"], "R_s_rayl": 100, "area_cm2": 1});
    let with = |k: &str, v: Value| {
        let mut e = base.clone();
        e[k] = v;
        e
    };
    assert!(builds(base.clone()));
    let mut three = with("thickness_um", json!(50));
    three["open_area"] = json!(0.3);
    three["pore_diameter_um"] = json!(40);
    assert!(!builds(three));
    assert!(!builds(with("open_area", json!(1.2))));
    assert!(!builds(with("R_s", json!(100))));
    assert!(!builds(with("material", json!("no_such_mesh"))));
    assert!(!builds(with("material", json!("pu_foam_acoustic_grade"))));
    // Thickness and pore diameter implying an open area ≥ 1.
    let mut e = with("thickness_um", json!(500));
    e["pore_diameter_um"] = json!(10);
    assert!(!builds(e));
}

// ----- Perforated plate ------------------------------------------------------------

#[test]
fn fok_function_matches_mode_matching() {
    // Independent variational solution of a thin orifice in a duct: its
    // end correction per side over Rayleigh's π/4·a is the interaction
    // function. Fok's series agrees within 1e-3 up to ξ = 0.7 and within
    // 2e-3 up to 0.8, beyond which its truncation shows.
    let t = fixture("materials_end_corrections.json");
    for (a, d) in f64s(&t["alpha"])
        .iter()
        .zip(f64s(&t["delta_orifice_side_over_a"]))
    {
        let ratio = d / (PI / 4.0);
        if *a <= 0.7 {
            assert!(
                (fok(*a) - ratio).abs() < 1e-3,
                "{a}: {} vs {ratio}",
                fok(*a)
            );
        } else if *a <= 0.8 {
            assert!((fok(*a) - ratio).abs() < 2e-3, "{a}");
        }
    }
}

#[test]
fn perforate_matches_independent_reference() {
    let air = air();
    let r = reference();
    for case in r["perforate"]["cases"].as_array().unwrap() {
        let get = |k: &str| case[k].as_f64().unwrap();
        for interaction in [true, false] {
            let m = PerforateModel {
                hole_radius: 0.5 * get("d"),
                thickness: get("t"),
                porosity: get("porosity"),
                area: get("area"),
                end_resistance: EndResistance::Maa,
                interaction,
            };
            let key = if interaction {
                "exact"
            } else {
                "exact_no_interaction"
            };
            for (f, z) in f64s(&case["freqs"])
                .iter()
                .zip(case[key].as_array().unwrap())
            {
                assert!(close(m.impedance(&air, 2.0 * PI * f), c(z), SAME_MODEL));
            }
            for (f, z) in f64s(&case["freqs"])
                .iter()
                .zip(case["maa"].as_array().unwrap())
            {
                let zm = maa_impedance(
                    &air,
                    2.0 * PI * f,
                    get("d"),
                    get("t"),
                    get("porosity"),
                    get("area"),
                );
                assert!(close(zm, c(z), SAME_MODEL));
            }
        }
    }
}

#[test]
fn perforate_agrees_with_maa_1998_within_its_accuracy() {
    // Maa's formula approximates Crandall's function (and uses 0.85d for the
    // two end corrections, against 16a/3π = 0.8488d here). Over perforate
    // constants 0.1–30 the independent computation finds deviations up to
    // 7.0 % in resistance and 3.4 % in reactance; the bounds below add a
    // small margin. Interaction is off: Maa has none.
    let air = air();
    let r = reference();
    let dev_r = r["perforate"]["maa_max_rel_dev_r"].as_f64().unwrap();
    let dev_x = r["perforate"]["maa_max_rel_dev_x"].as_f64().unwrap();
    assert!(dev_r < 0.075 && dev_x < 0.04);
    let (mut max_r, mut max_x): (f64, f64) = (0.0, 0.0);
    for (d, t) in [
        (0.2e-3, 0.2e-3),
        (0.5e-3, 1e-3),
        (1e-3, 0.5e-3),
        (0.3e-3, 3e-3),
    ] {
        let m = PerforateModel {
            hole_radius: 0.5 * d,
            thickness: t,
            porosity: 0.01,
            area: 1.0,
            end_resistance: EndResistance::Maa,
            interaction: false,
        };
        for i in 0..60 {
            let k = 0.1 * 300f64.powf(i as f64 / 59.0);
            let w = (k / d).powi(2) * 4.0 * air.mu / air.rho;
            assert!((m.perforate_constant(&air, w) - k).abs() < 1e-9 * k);
            let ze = m.impedance(&air, w);
            let zm = maa_impedance(&air, w, d, t, 0.01, 1.0);
            max_r = max_r.max((zm.re / ze.re - 1.0).abs());
            max_x = max_x.max((zm.im / ze.im - 1.0).abs());
        }
    }
    assert!((max_r - dev_r).abs() < 1e-6 && (max_x - dev_x).abs() < 1e-6);
    assert!(max_r < 0.075 && max_x < 0.04);
    assert_eq!(PerforateModel::regime(0.5), "resistive");
    assert_eq!(PerforateModel::regime(5.0), "transitional");
    assert_eq!(PerforateModel::regime(20.0), "mass");
}

#[test]
fn perforate_interaction_reduces_end_mass() {
    let air = air();
    let base = PerforateModel {
        hole_radius: 0.5e-3,
        thickness: 1e-3,
        porosity: 0.2,
        area: 1e-3,
        end_resistance: EndResistance::None,
        interaction: true,
    };
    let free = PerforateModel {
        interaction: false,
        ..base
    };
    let psi = fok(0.2f64.sqrt());
    assert!((base.end_correction() / free.end_correction() - psi).abs() < 1e-12);
    // Mass difference at low frequency = 2ρ(δ_free − δ)/S per hole.
    let w = 2.0 * PI * 10.0;
    let dm = (free.impedance(&air, w).im - base.impedance(&air, w).im) / w;
    let expect = 2.0 * air.rho * (free.end_correction() - base.end_correction())
        / (PI * 0.25e-6)
        / base.hole_count();
    assert!((dm / expect - 1.0).abs() < 1e-9);
    // Hole count ↔ porosity through the netlist.
    let circ = circuit(
        1,
        &["a"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["a"]},
            {"id": "p", "type": "perforated_plate", "nodes": ["a"], "hole_diameter_mm": 1,
             "thickness_mm": 1, "holes": 20, "area_cm2": 1, "end_resistance": "none"}
        ]),
    );
    let sigma = 20.0 * PI * 0.25e-6 / 1e-4;
    let m = PerforateModel {
        porosity: sigma,
        area: 1e-4,
        ..base
    };
    let z = input_impedance(&circ, 500.0);
    assert!(close(z, m.impedance(&air, 2.0 * PI * 500.0), 1e-12));
}

// ----- Porous layer ------------------------------------------------------------------

fn textbook_foam() -> PorousModel {
    PorousModel::Jca {
        porosity: 0.85,
        sigma: 34_000.0,
        tortuosity: 1.18,
        viscous_length: 60e-6,
        thermal_length: 87e-6,
    }
}

#[test]
fn porous_models_match_independent_reference() {
    let air = air();
    let r = reference();
    let p = &r["porous"];
    let models = [
        ("jca_foam", textbook_foam()),
        (
            "jcal_foam",
            PorousModel::Jcal {
                porosity: 0.85,
                sigma: 34_000.0,
                tortuosity: 1.18,
                viscous_length: 60e-6,
                thermal_length: 87e-6,
                thermal_permeability: 2.0e-10,
            },
        ),
        ("db_20k", PorousModel::DelanyBazley { sigma: 20_000.0 }),
        ("miki_20k", PorousModel::Miki { sigma: 20_000.0 }),
    ];
    for (name, model) in models {
        let row = &p["models"][name];
        let layer = PorousLayer {
            model,
            thickness: p["thickness"].as_f64().unwrap(),
            area: p["area"].as_f64().unwrap(),
        };
        for (i, f) in f64s(&p["freqs"]).iter().enumerate() {
            let w = 2.0 * PI * f;
            let (rho, k) = model.equivalent_fluid(&air, w);
            assert!(
                close(rho, c(&row["rho_eq"][i]), SAME_MODEL),
                "{name} ρ at {f}"
            );
            assert!(close(k, c(&row["k_eq"][i]), SAME_MODEL), "{name} K at {f}");
            let zs = layer.surface_impedance(&air, w);
            assert!(close(zs, c(&row["zs"][i]), SAME_MODEL), "{name} Zs at {f}");
        }
    }
}

#[test]
fn porous_slab_dc_limit_is_sigma_t_over_a() {
    // Series slab of the textbook foam, 5 mm over 2 cm², to ambient. At
    // 0.01 Hz the pore-air compliance and the inertia are 1e-5 corrections.
    let (t, a, sigma) = (5e-3, 2e-4, 34_000.0);
    for level in [0u8, 1] {
        let circ = circuit(
            level,
            &["a"],
            json!([
                {"id": "src", "type": "flow_source", "nodes": ["a"]},
                {"id": "foam", "type": "porous_layer", "nodes": ["a"],
                 "material": "pu_foam_textbook_jca", "thickness_mm": 5, "area_cm2": 2}
            ]),
        );
        let z = input_impedance(&circ, 0.01);
        assert!((z.re / (sigma * t / a) - 1.0).abs() < 1e-4, "L{level}: {z}");
    }
    // JCAL and the lumped slab share the limit.
    let air = air();
    let layer = PorousLayer {
        model: PorousModel::Jcal {
            porosity: 0.97,
            sigma,
            tortuosity: 1.1,
            viscous_length: 80e-6,
            thermal_length: 200e-6,
            thermal_permeability: 3e-9,
        },
        thickness: t,
        area: a,
    };
    let z = layer.lumped_impedance(&air, 2.0 * PI * 1e-3);
    assert!((z.re / (sigma * t / a) - 1.0).abs() < 1e-6);
    // Rigid backing: at low frequency Z_c·coth(Γt)/A → K_eq/(jωtA) + σt/(3A).
    // The pore air is an isothermal compliance C = φAt/P0 whose thermal
    // relaxation, K ≈ P0·(1 + (γ−1)/γ·jωτ') with τ' = ρ0·Pr·k0'/(ηφ), adds
    // the frequency-independent resistance (γ−1)P0τ'/(γφtA) (Lafarge's
    // thermal time; it dominates the viscous third here).
    let w = 2.0 * PI * 1e-3;
    let zs = layer.surface_impedance(&air, w);
    let c_iso = 0.97 * a * t / air.p0;
    assert!((-1.0 / (w * zs.im) / c_iso - 1.0).abs() < 1e-4, "{zs}");
    let tau_t = air.rho * air.prandtl * 3e-9 / (air.mu * 0.97);
    let r_thermal = (air.gamma - 1.0) * air.p0 * tau_t / (air.gamma * 0.97 * t * a);
    let r_expect = sigma * t / (3.0 * a) + r_thermal;
    assert!((zs.re / r_expect - 1.0).abs() < 1e-3, "{zs} vs {r_expect}");
}

#[test]
fn jca_high_frequency_limits() {
    // ρ_eq·φ/(α∞ρ0) → 1 + (1 − j)·δv/Λ and K_eq·φ/(γP0) → 1/(1 + (γ−1)(1 − j)δt/Λ'),
    // with δv = sqrt(2η/ρ0ω), δt = δv/sqrt(Pr) (Johnson et al. 1987;
    // Champoux & Allard 1991). The residual of these first-order asymptotes
    // is of higher order: relative to the first-order term it must vanish,
    // asymptotically by sqrt(10) per decade (O(δ) relative). The density
    // residual falls by 10 per decade and the modulus residual by 2.9 between
    // 1 and 10 MHz (its O(δ³) part is still visible), hence the bound 2.5.
    let air = air();
    let m = textbook_foam();
    let (phi, ainf, lv, lt) = (0.85, 1.18, 60e-6, 87e-6);
    let resid = |f: f64| {
        let w = 2.0 * PI * f;
        let dv = air.viscous_layer(w);
        let dt = air.thermal_layer(w);
        let (rho, k) = m.equivalent_fluid(&air, w);
        let r = rho * phi / (ainf * air.rho) - (1.0 + C64::new(1.0, -1.0) * dv / lv);
        let kk = k * phi / air.bulk_modulus()
            - (1.0 + (air.gamma - 1.0) * C64::new(1.0, -1.0) * dt / lt).inv();
        (r.norm() / (dv / lv), kk.norm() / (dt / lt))
    };
    let (r1, k1) = resid(1e6);
    let (r2, k2) = resid(1e7);
    assert!(r1 < 0.1 && k1 < 0.1, "{r1} {k1}");
    let decade = 2.5;
    assert!(r2 < r1 / decade && k2 < k1 / decade, "{r1} {r2} {k1} {k2}");
    // Characteristic impedance → ρ0c0·sqrt(α∞)/φ.
    let (_, zc) = m.propagation(&air, 2.0 * PI * 1e9);
    let limit = air.rho_c() * ainf.sqrt() / phi;
    assert!((zc / limit - 1.0).norm() < 1e-2, "{zc} vs {limit}");
}

#[test]
fn jca_agrees_with_miki_in_miki_range() {
    // A fibrous material of σ = 20 kPa·s/m² in JCA form: φ = 0.98, α∞ = 1,
    // Λ = sqrt(8η/(σφ)) (Johnson's shape factor 1) and Λ' = 2Λ (Allard &
    // Champoux 1992, fibrous materials). Over 0.01 ≤ f/σ ≤ 1 it agrees with
    // Miki's regression within 8 % in Z_c and 12 % in the wavenumber (the
    // computed worst cases are 6.3 % and 10.9 %). The bound is empirical:
    // two different models of the same class of material; a convention or
    // unit error gives O(1) disagreement.
    let air = air();
    let sigma = 20_000.0;
    let phi = 0.98;
    let lv = (8.0 * air.mu / (sigma * phi)).sqrt();
    let jca = PorousModel::Jca {
        porosity: phi,
        sigma,
        tortuosity: 1.0,
        viscous_length: lv,
        thermal_length: 2.0 * lv,
    };
    let miki = PorousModel::Miki { sigma };
    let (lo, hi) = miki.window().unwrap();
    assert_eq!((lo, hi), (200.0, 20_000.0));
    let (mut dz, mut dk): (f64, f64) = (0.0, 0.0);
    for i in 0..=40 {
        let f = lo * (hi / lo).powf(i as f64 / 40.0);
        let w = 2.0 * PI * f;
        let (g1, z1) = jca.propagation(&air, w);
        let (g2, z2) = miki.propagation(&air, w);
        dz = dz.max((z1 / z2 - 1.0).norm());
        dk = dk.max((g1 / g2 - 1.0).norm());
    }
    assert!(dz < 0.08 && dk < 0.12, "Zc {dz}, k {dk}");
}

#[test]
fn one_parameter_laws_are_kept_passive_below_their_window() {
    // Written out here from the papers' coefficients (X = f/σ, σ in Pa·s/m²):
    // (ρ_eq, K_eq) of the raw power law, without the engine's guard.
    let air = air();
    let raw = |miki: bool, f: f64, sigma: f64| {
        let y = 1e3 * f / sigma;
        let (zc, k) = if miki {
            (
                C64::new(1.0 + 5.50 * y.powf(-0.632), -8.43 * y.powf(-0.632)),
                C64::new(1.0 + 7.81 * y.powf(-0.618), -11.41 * y.powf(-0.618)),
            )
        } else {
            (
                C64::new(1.0 + 9.08 * y.powf(-0.75), -11.9 * y.powf(-0.73)),
                C64::new(1.0 + 10.8 * y.powf(-0.70), -10.3 * y.powf(-0.59)),
            )
        };
        (air.rho * zc * k, air.rho * air.c * air.c * zc / k)
    };
    let model = |miki: bool, sigma: f64| {
        if miki {
            PorousModel::Miki { sigma }
        } else {
            PorousModel::DelanyBazley { sigma }
        }
    };
    // Where the raw law is passive the engine uses it unchanged: Miki over
    // its whole window, DB from f/σ = 0.011 up.
    for (miki, x0) in [(true, 0.01f64), (false, 0.011)] {
        for i in 0..=30 {
            let x = x0 * (3.0 / x0).powf(i as f64 / 30.0);
            let (f, sigma) = (x * 20_000.0, 20_000.0);
            let (r0, k0) = raw(miki, f, sigma);
            let (r1, k1) = model(miki, sigma).equivalent_fluid(&air, 2.0 * PI * f);
            assert!(k0.im > 0.0 && r0.im < 0.0, "raw law active at X = {x}");
            assert!(close(r1, r0, 1e-12) && close(k1, k0, 1e-12), "X = {x}");
        }
    }
    // Below f/σ ≈ 0.00105 (Miki) and 0.0106 (DB, just inside its window)
    // the raw Im K_eq turns negative — an active medium (Dragna et al.
    // 2015). The engine clips it at zero and keeps the real part.
    for (miki, x) in [(true, 5e-4), (false, 5e-3), (false, 0.01)] {
        let (_, k0) = raw(miki, x * 50_000.0, 50_000.0);
        assert!(k0.im < 0.0, "raw K at X = {x}: {k0}");
        let (_, k1) = model(miki, 50_000.0).equivalent_fluid(&air, 2.0 * PI * x * 50_000.0);
        assert_eq!(k1.im, 0.0);
        assert!((k1.re / k0.re - 1.0).abs() < 1e-12);
    }
    // Every slab and rigid-backed layer is then passive from 1 Hz to
    // 40 kHz: Re Z_s ≥ 0, Re Z ≥ 0 of the lumped slab, and the Hermitian
    // part of the slab's impedance matrix is positive semi-definite. For
    // the symmetric slab its eigenvalues are Re((A ± 1)/C).
    for miki in [true, false] {
        for sigma in [5_900.0, 20_000.0, 50_000.0, 100_000.0] {
            for t in [2e-3, 10e-3, 25e-3] {
                let layer = PorousLayer {
                    model: model(miki, sigma),
                    thickness: t,
                    area: 1e-3,
                };
                for i in 0..=120 {
                    let f = 40_000f64.powf(i as f64 / 120.0);
                    let w = 2.0 * PI * f;
                    let zs = layer.surface_impedance(&air, w);
                    assert!(zs.re >= 0.0, "Z_s {zs} at {f} Hz, σ {sigma}, t {t}");
                    assert!(layer.lumped_impedance(&air, w).re >= 0.0);
                    let [a, _, c, _] = layer.abcd(&air, w);
                    let one = C64::new(1.0, 0.0);
                    for e in [(a + one) / c, (a - one) / c] {
                        assert!(e.re >= -1e-9 * e.norm(), "slab {e} at {f} Hz");
                    }
                }
            }
        }
    }
    // Through a netlist: 10 mm of the 50 kPa·s/m² pad foam (Miki by
    // default) on a rigid backing absorbs power at 10-40 Hz; the raw law
    // gave it a negative surface resistance there.
    let circ = circuit(
        1,
        &["a"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["a"]},
            {"id": "pad", "type": "porous_layer", "nodes": ["a"], "backing": "rigid",
             "material": "pu_foam_acoustic_grade", "thickness_mm": 10, "area_cm2": 20}
        ]),
    );
    for f in [10.0, 20.0, 40.0] {
        assert!(input_impedance(&circ, f).re > 0.0, "{f} Hz");
        let x = circ.solve_at(f).unwrap();
        let p = circ.power_absorbed(f, &x);
        let pad = p.iter().find(|(id, _)| id == "pad").unwrap().1.unwrap();
        assert!(pad > 0.0, "{p:?}");
    }
    // The one-parameter laws flag their range; the physical models do not.
    assert_eq!(
        PorousModel::DelanyBazley { sigma: 20_000.0 }.window(),
        Some((200.0, 20_000.0))
    );
    assert!(textbook_foam().window().is_none());
    let circ = circuit(
        1,
        &["a"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["a"]},
            {"id": "wool", "type": "porous_layer", "nodes": ["a"], "backing": "rigid",
             "model": "delany_bazley", "sigma_kPa_s_per_m2": 20, "thickness_mm": 25, "area_cm2": 10}
        ]),
    );
    let v = circ.validity();
    assert!(v
        .iter()
        .any(|l| l.criterion.contains("one-parameter") && l.begin_hz == Some(20_000.0)));
}

#[test]
fn textbook_foam_entry_is_self_consistent() {
    // The spec's p. 47 set (attributed to Allard & Atalla; not verified
    // against the book): Johnson's shape factor M = 8α∞η/(σφΛ²) is 1.64
    // and Λ' > Λ, both physically admissible.
    let e = material("pu_foam_textbook_jca", &["porous"]).unwrap();
    assert_eq!(e.status, "unverified");
    let p = &e.params;
    let (phi, sigma, ainf) = (
        p["porosity"].as_f64().unwrap(),
        p["sigma_Pa_s_per_m2"].as_f64().unwrap(),
        p["tortuosity"].as_f64().unwrap(),
    );
    let lv = p["viscous_length_um"].as_f64().unwrap() * 1e-6;
    let lt = p["thermal_length_um"].as_f64().unwrap() * 1e-6;
    let m = 8.0 * ainf * 1.81e-5 / (sigma * phi * lv * lv);
    assert!((m - 1.64).abs() < 0.01, "{m}");
    assert!(lt > lv);
    // Every entry carries provenance.
    for e in database().unwrap() {
        assert!(!e.source.is_empty() && !e.method.is_empty() && !e.licence.is_empty());
    }
}

#[test]
fn porous_layer_rejects_bad_input() {
    let base = json!({"id": "p", "type": "porous_layer", "nodes": ["a"], "thickness_mm": 10,
        "area_cm2": 1, "sigma_Pa_s_per_m2": 20000});
    let with = |k: &str, v: Value| {
        let mut e = base.clone();
        e[k] = v;
        e
    };
    assert!(builds(base.clone()));
    assert!(!builds(with("model", json!("jca")))); // missing JCA parameters
    assert!(!builds(with("porosity", json!(0.9)))); // incomplete JCA set
    assert!(!builds(with("backing", json!("soft"))));
    let mut rigid = with("backing", json!("rigid"));
    rigid["nodes"] = json!(["a", "a"]);
    assert!(!builds(rigid));
}

// ----- Fill (App. C6, E11) ----------------------------------------------------------

/// Pressure in a lossless 30 cm³ cavity with optional fill, driven by a
/// prescribed volume velocity.
fn filled_cup_flow(fill: Option<Value>) -> Circuit {
    let mut elements = vec![
        json!({"id": "src", "type": "flow_source", "nodes": ["a"]}),
        json!({"id": "cup", "type": "cavity", "node": "a", "volume_cm3": 30, "wall_loss": false}),
    ];
    if let Some(mut f) = fill {
        f["id"] = json!("fill");
        f["type"] = json!("fill");
        f["node"] = json!("a");
        elements.push(f);
    }
    circuit(1, &["a"], Value::Array(elements))
}

fn spl(c: &Circuit, f: f64, node: &str) -> f64 {
    let x = c.solve_at(f).unwrap();
    20.0 * (c.node_value(&x, node).unwrap().norm() / 20e-6).log10()
}

#[test]
fn fill_lowers_spl_by_2_92_and_0_83_db_at_constant_volume_velocity() {
    // E11: under a prescribed volume velocity the low-frequency SPL falls
    // by 20·log10(1.4) = 2.92 dB at full fill and 20·log10(1.1) = 0.83 dB at
    // 25 %; τ = 1 ms (f_relax = 159 Hz) and f = 1 Hz, where the relaxation
    // term deviates from its limit by (ωτ)² ≈ 4e-5.
    let empty = filled_cup_flow(None);
    for (phi, drop) in [(1.0, 20.0 * 1.4f64.log10()), (0.25, 20.0 * 1.1f64.log10())] {
        let full = filled_cup_flow(Some(
            json!({"volume_cm3": 30, "fraction": phi, "tau_ms": 1}),
        ));
        let d = spl(&empty, 1.0, "a") - spl(&full, 1.0, "a");
        assert!((d - drop).abs() < 1e-3, "φ = {phi}: {d} dB");
        // Above the relaxation frequency the gain vanishes.
        let d_hi = spl(&empty, 15_900.0, "a") - spl(&full, 15_900.0, "a");
        assert!(d_hi.abs() < 0.01, "{d_hi} dB at 100 f_relax");
        // At the relaxation frequency itself, half-way in compliance.
        let fr = 1.0 / (2.0 * PI * 1e-3);
        let d_r = spl(&empty, fr, "a") - spl(&full, fr, "a");
        let expect = 20.0
            * (C64::new(1.0, 0.0) + 0.4 * phi / C64::new(1.0, 1.0))
                .norm()
                .log10();
        assert!((d_r - expect).abs() < 1e-9);
    }
    assert!((20.0 * 1.4f64.log10() - 2.92).abs() < 0.005);
    assert!((20.0 * 1.1f64.log10() - 0.83).abs() < 0.005);
}

/// Spec C2 example driver (0.1 g, 1 mm/N, 10 cm², 30 cm³) at constant
/// voltage, as in tests/core_network.rs.
fn c2_driver(fill: Option<f64>) -> Circuit {
    let mut elements = vec![
        json!({"id": "amp", "type": "vsource", "nodes": ["e1"], "V_V": 1.0}),
        json!({"id": "coil", "type": "coil", "nodes": ["e1", "e2"], "Re_ohm": 32.0}),
        json!({"id": "motor", "type": "motor", "nodes": ["e2", "gnd", "m", "gnd"], "Bl_Tm": 1.0}),
        json!({"id": "susp", "type": "suspension", "node": "m", "Mms_g": 0.1,
               "Cms_mm_per_N": 1.0, "Rms_Ns_per_m": 0.05}),
        json!({"id": "dia", "type": "piston", "nodes": ["m", "gnd", "af", "gnd"], "Sd_cm2": 10}),
        json!({"id": "front", "type": "cavity", "node": "af", "volume_cm3": 30, "wall_loss": false}),
    ];
    if let Some(phi) = fill {
        elements.push(
            json!({"id": "fill", "type": "fill", "node": "af", "volume_cm3": 30,
            "fraction": phi, "tau_ms": 0.01}),
        );
    }
    let doc = json!({
        "air": {"preset": "spec_reference"},
        "nodes": [
            {"id": "e1", "domain": "electrical"}, {"id": "e2", "domain": "electrical"},
            {"id": "m", "domain": "mechanical"}, {"id": "af", "domain": "acoustic"}
        ],
        "elements": elements,
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

#[test]
fn fill_drop_is_smaller_at_constant_voltage() {
    // E11: with the C2 example at constant voltage the stiffness-controlled
    // drop is only ≈0.59 dB (full fill) and ≈0.15 dB (25 %), because the
    // softer cavity lets the diaphragm move further. Closed form below the
    // coupled resonance: p ∝ 1/(K_total·C), K_total = 1/Cms + Sd²/C.
    let air = air();
    let c0 = 30e-6 / air.bulk_modulus();
    let empty = c2_driver(None);
    for (phi, expect) in [(1.0, 0.59), (0.25, 0.15)] {
        let full = c2_driver(Some(phi));
        let d = spl(&empty, 20.0, "af") - spl(&full, 20.0, "af");
        let c1 = c0 * (1.0 + 0.4 * phi);
        let k = |c: f64| 1e3 + 1e-6 / c;
        let closed = 20.0 * ((k(c1) * c1) / (k(c0) * c0)).log10();
        // 0.01 dB: the mass, Rms and back-EMF terms at 20 Hz.
        assert!((d - closed).abs() < 0.01, "φ = {phi}: {d} vs {closed}");
        assert!((d - expect).abs() < 0.01, "φ = {phi}: {d}");
    }
}

#[test]
fn fibre_relaxation_matches_exact_cell_model() {
    // Tarnow's cell (isothermal fibre in a coaxial air cell) solved exactly
    // with Bessel functions (tools/materials/reference.py) against the
    // single relaxation with τ = ⟨θ⟩/ν'. They agree to first order in ω by
    // construction; the single pole stays within 2 % in magnitude up to
    // three times the relaxation frequency, and both vanish above it.
    let air = air();
    for case in reference()["fill"].as_array().unwrap() {
        let get = |k: &str| case[k].as_f64().unwrap();
        let c_solid = get("bulk_density") / get("fibre_density");
        let tau = fibre_relaxation_time(&air, get("fibre_diameter"), c_solid);
        assert!((tau / get("tau") - 1.0).abs() < 1e-12);
        let model = FillModel {
            volume: 1.0,
            fraction: 1.0,
            tau,
        };
        for (x, e) in f64s(&case["ratios"])
            .iter()
            .zip(case["exact"].as_array().unwrap())
        {
            let w = 2.0 * PI * x * model.relaxation_frequency();
            let single = model.delta_compliance(&air, w) * air.bulk_modulus() / (air.gamma - 1.0);
            let exact = c(e);
            if *x <= 3.0 {
                assert!(
                    (single.norm() / exact.norm() - 1.0).abs() < 0.02,
                    "f/fr = {x}"
                );
            }
            if *x >= 10.0 {
                assert!(single.norm() < 0.1 && exact.norm() < 0.12, "f/fr = {x}");
            }
        }
    }
    // Glass wool 14 kg/m³ (spec p. 46), 6 µm fibres of 2500 kg/m³: ≈2.3 kHz.
    let fr = 1.0 / (2.0 * PI * fibre_relaxation_time(&air, 6e-6, 14.0 / 2500.0));
    assert!((fr - 2256.0).abs() < 5.0, "{fr}");
}

#[test]
fn fill_from_fibre_parameters_and_input_checks() {
    let air = air();
    let circ = filled_cup_flow(Some(json!({"volume_cm3": 30, "fibre_diameter_um": 20,
        "bulk_density_kg_per_m3": 10, "fibre_density_kg_per_m3": 1380})));
    let tau = fibre_relaxation_time(&air, 20e-6, 10.0 / 1380.0);
    let fr = 1.0 / (2.0 * PI * tau);
    assert!((fr - 281.7).abs() < 0.5, "{fr}");
    let empty = filled_cup_flow(None);
    let d = spl(&empty, 1.0, "a") - spl(&circ, 1.0, "a");
    assert!((d - 2.923).abs() < 0.01);
    let bad = |f: Value| {
        let mut e = f;
        e["id"] = json!("f");
        e["type"] = json!("fill");
        e["node"] = json!("a");
        !builds(e)
    };
    assert!(bad(json!({"volume_cm3": 30})));
    assert!(bad(
        json!({"volume_cm3": 30, "tau_ms": 1, "f_relax_Hz": 100})
    ));
    assert!(bad(json!({"volume_cm3": 30, "tau_ms": 1, "fraction": 1.5})));
    assert!(bad(
        json!({"volume_cm3": 30, "fibre_diameter_um": 20, "bulk_density_kg_per_m3": 900,
        "fibre_density_kg_per_m3": 1380})
    ));
}

// ----- Membrane vent --------------------------------------------------------------------

#[test]
fn membrane_vent_resistance_mass_and_compliance() {
    // Porous membrane: R ∥ (R_m + jωM + 1/(jωC)). DC → R_s/A; at the
    // membrane resonance the frame branch bypasses the pores.
    let air = air();
    let (r_s, radius, t, rho_m, tension) = (2000.0, 2e-3, 20e-6, 800.0, 20.0);
    let area = PI * radius * radius;
    let m = 4.0 / 3.0 * rho_m * t / area;
    let cm = PI * radius.powi(4) / (8.0 * tension);
    let model = MembraneVentModel {
        r_flow: r_s / area,
        mass: m,
        compliance: cm,
        r_membrane: 1e5,
    };
    let circ = circuit(
        1,
        &["a"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["a"]},
            {"id": "v", "type": "membrane_vent", "nodes": ["a"], "R_s_rayl": r_s,
             "radius_mm": 2, "thickness_um": 20, "density_kg_per_m3": 800,
             "tension_N_per_m": 20, "R_m_Pa_s_per_m3": 1e5}
        ]),
    );
    for f in [1e-3, 100.0, model.resonance(), 10_000.0] {
        let z = input_impedance(&circ, f);
        assert!(close(z, model.impedance(2.0 * PI * f), 1e-9), "{f}");
    }
    let zdc = model.impedance(2.0 * PI * 1e-3);
    assert!((zdc.re / (r_s / area) - 1.0).abs() < 1e-6);
    let zr = model.impedance(2.0 * PI * model.resonance());
    let par = 1.0 / (1.0 / (r_s / area) + 1.0 / 1e5);
    assert!((zr - C64::new(par, 0.0)).norm() < 1e-6 * par);
    // Raising the rayl value tenfold hardly changes the vent near its
    // resonance: the membrane branch carries the flow, so insertion loss is
    // not a monotonic function of the rayl value alone.
    let stiff = MembraneVentModel {
        r_flow: 10.0 * model.r_flow,
        ..model
    };
    let w = 2.0 * PI * model.resonance();
    assert!(stiff.impedance(0.0 + 1e-3).norm() > 9.9 * model.impedance(1e-3).norm());
    assert!((stiff.impedance(w).norm() / model.impedance(w).norm()) < 1.2);
    // Mass given as a total and compliance as a resonance give the same model.
    let circ2 = circuit(
        1,
        &["a"],
        json!([
            {"id": "src", "type": "flow_source", "nodes": ["a"]},
            {"id": "v", "type": "membrane_vent", "nodes": ["a"], "R_s_rayl": r_s,
             "radius_mm": 2, "mass_mg": rho_m * t * area * 1e6,
             "resonance_Hz": model.resonance(), "R_m_Pa_s_per_m3": 1e5}
        ]),
    );
    assert!(close(
        input_impedance(&circ2, 700.0),
        model.impedance(2.0 * PI * 700.0),
        1e-9
    ));
    // The membrane-branch loss is required, as R_m or as a Q.
    let vent = |extra: Value| {
        let mut e = json!({"id": "v", "type": "membrane_vent", "nodes": ["a"], "R_s_rayl": r_s,
            "radius_mm": 2, "thickness_um": 20, "density_kg_per_m3": 800, "tension_N_per_m": 20});
        for (k, v) in extra.as_object().unwrap() {
            e[k] = v.clone();
        }
        e
    };
    assert!(!builds(vent(json!({}))));
    assert!(!builds(vent(json!({"Q_m": 0}))));
    let q = (m / cm).sqrt() / 1e5;
    let circ3 = circuit(
        1,
        &["a"],
        json!([{"id": "src", "type": "flow_source", "nodes": ["a"]}, vent(json!({"Q_m": q}))]),
    );
    assert!(close(
        input_impedance(&circ3, 900.0),
        model.impedance(2.0 * PI * 900.0),
        1e-9
    ));
    let _ = air;
}

// ----- Power balance ---------------------------------------------------------------------

#[test]
fn power_balances_with_every_material() {
    for level in [0u8, 1] {
        let circ = circuit(
            level,
            &["a", "b", "c", "d"],
            json!([
                {"id": "src", "type": "pressure_source", "nodes": ["a"], "p_Pa": 1.0,
                 "Zs_Pa_s_per_m3": 1e5},
                {"id": "mesh", "type": "mesh", "nodes": ["a", "b"], "R_s_rayl": 70,
                 "area_cm2": 3, "thickness_um": 70, "open_area": 0.25},
                {"id": "cup", "type": "cavity", "node": "b", "volume_cm3": 25},
                {"id": "fill", "type": "fill", "node": "b", "volume_cm3": 25, "fraction": 0.5,
                 "tau_ms": 0.5},
                {"id": "plate", "type": "perforated_plate", "nodes": ["b", "c"],
                 "hole_diameter_mm": 0.8, "thickness_mm": 1, "porosity": 0.1, "area_cm2": 2},
                {"id": "foam", "type": "porous_layer", "nodes": ["c", "d"],
                 "material": "pu_foam_textbook_jca", "thickness_mm": 4, "area_cm2": 2},
                {"id": "wool", "type": "porous_layer", "nodes": ["d"], "backing": "rigid",
                 "material": "glass_wool_14kg_through", "thickness_mm": 20, "area_cm2": 5},
                {"id": "gore", "type": "membrane_vent", "nodes": ["d"], "R_s_rayl": 5000,
                 "area_mm2": 10, "mass_mg": 0.2, "resonance_Hz": 1500, "Q_m": 5},
                {"id": "rear", "type": "cavity", "node": "c", "volume_cm3": 10}
            ]),
        );
        for f in [20.0, 300.0, 1500.0, 9000.0] {
            let x = circ.solve_at(f).unwrap();
            let p = circ.power_absorbed(f, &x);
            assert!(p.iter().all(|(_, v)| v.is_some()), "{p:?}");
            let scale: f64 = p.iter().map(|(_, v)| v.unwrap().abs()).sum();
            let sum: f64 = p.iter().map(|(_, v)| v.unwrap()).sum();
            assert!(sum.abs() < 1e-10 * scale, "L{level} {f}: {p:?}");
            for (id, v) in &p {
                if id != "src" {
                    assert!(v.unwrap() >= -1e-12 * scale, "{id} active at {f} Hz");
                }
            }
        }
    }
}
