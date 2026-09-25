//! Rigid-wall eigenmodes (`modes`) and the `modal_cavity` element: mode lists
//! (erratum E25), truncation counts (E27), reciprocity, passivity, the
//! lumped limit (E3, E4), resonances, symmetry, the depth line, wall-loss Q
//! (E29), power balance and input validation.
//!
//! Independent references come from tools/cavity/generate.py
//! (tests/data/cavity_reference.json): mpmath/scipy Bessel roots, brute-force
//! quadrature of footprint means and wall integrals, and lossless impedance
//! matrices from the mixed (waveguide) representation with 10–20× the
//! element's transverse cutoff.

use acoustilab::elements::cavity::ModalCavity;
use acoustilab::modes::{self, Face, Footprint, Mode, ModeIndex, Patch, Shape};
use acoustilab::{AirState, Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::PI;

fn reference() -> Value {
    serde_json::from_str(include_str!("data/cavity_reference.json")).unwrap()
}

fn air() -> AirState {
    AirState::spec_reference()
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

/// Netlist port from a fixture record [face, u, v, [kind, dims...]] (SI).
fn port_json(node: &str, rec: &Value) -> Value {
    let face = rec[0].as_str().unwrap();
    let (u, v) = (rec[1].as_f64().unwrap(), rec[2].as_f64().unwrap());
    let fp = &rec[3];
    let (ukey, vkey, ea, eb) = match face {
        "x0" | "x1" => ("y_mm", "z_mm", "ly_mm", "lz_mm"),
        "y0" | "y1" => ("x_mm", "z_mm", "lx_mm", "lz_mm"),
        "side" => ("angle_deg", "z_mm", "arc_mm", "lz_mm"),
        _ => ("x_mm", "y_mm", "lx_mm", "ly_mm"),
    };
    let mut p = json!({"node": node, "face": face});
    p[ukey] = json!(if face == "side" { u } else { u * 1e3 });
    p[vkey] = json!(v * 1e3);
    if fp[0] == "disk" {
        p["radius_mm"] = json!(fp[1].as_f64().unwrap() * 1e3);
    } else {
        p[ea] = json!(fp[1].as_f64().unwrap() * 1e3);
        p[eb] = json!(fp[2].as_f64().unwrap() * 1e3);
    }
    p
}

/// Builds a circuit: acoustic nodes, a modal cavity `cav`, extra elements.
fn circuit(
    level: u8,
    cavity: Value,
    nodes: &[String],
    extra: Vec<Value>,
    freqs: &[f64],
) -> Circuit {
    let mut elements = vec![cavity];
    elements.extend(extra);
    let doc = json!({
        "air": {"preset": "spec_reference"},
        "sweep": {"frequencies_Hz": freqs},
        "level": level,
        "nodes": nodes.iter().map(|n| json!({"id": n, "domain": "acoustic"})).collect::<Vec<_>>(),
        "elements": elements,
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

fn modal<'a>(c: &'a Circuit, id: &str) -> &'a ModalCavity {
    c.elements[c.element_index(id).unwrap()]
        .as_any()
        .downcast_ref::<ModalCavity>()
        .unwrap()
}

fn box_cavity(ports: Vec<Value>, extra: Value) -> Value {
    let mut v = json!({"id": "cav", "type": "modal_cavity",
        "lx_mm": 60, "ly_mm": 45, "lz_mm": 20, "ports": ports});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

fn node_names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("p{i}")).collect()
}

fn find_mode(shape: &Shape, index: ModeIndex) -> Mode {
    shape
        .modes_below(3000.0)
        .into_iter()
        .find(|m| m.index == index)
        .unwrap()
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

// ----- Mode lists (E25, E27) ----------------------------------------------------

#[test]
fn box_mode_list_matches_corrected_spec_list() {
    // Erratum E25: 60 × 45 × 20 mm, c = 343 m/s.
    let shape = Shape::Box {
        lx: 0.060,
        ly: 0.045,
        lz: 0.020,
    };
    let c = 343.0;
    let modes = shape.modes_below(2.0 * PI * 10_000.0 / c);
    let distinct = modes::distinct_frequencies(&modes, c, 1e-9);
    let expect = [2.86, 3.81, 4.76, 5.72, 6.87, 7.62, 8.14, 8.57];
    for (f, e) in distinct[1..9].iter().zip(expect) {
        // The list is rounded to 10 Hz (8575 Hz prints as 8.57 or 8.58).
        assert!((f / 1e3 - e).abs() <= 0.0051, "{f} Hz vs {e} kHz");
    }
    // The full list below 10 kHz against the reference: indices and
    // frequencies (closed form, so 1e-12).
    let r = reference();
    let list = r["modes"]["box"]["modes_below_10k"].as_array().unwrap();
    assert_eq!(list.len(), modes.len());
    for (m, rec) in modes.iter().zip(list) {
        let n = [0, 1, 2].map(|i| rec[i].as_u64().unwrap() as u32);
        let f = rec[3].as_f64().unwrap();
        assert!(
            (m.frequency(c) - f).abs() <= 1e-12 * f.max(1.0),
            "{:?}",
            m.index
        );
        // Degenerate pairs may swap; compare frequencies of the listed index.
        let other = find_mode(&shape, ModeIndex::Box { n });
        assert!((other.frequency(c) - f).abs() <= 1e-12 * f.max(1.0));
    }
}

#[test]
fn cylinder_mode_list_matches_corrected_spec_list() {
    // Erratum E25: radius 25 mm, depth 25 mm, c = 343 m/s.
    let shape = Shape::Cylinder {
        radius: 0.025,
        depth: 0.025,
    };
    let c = 343.0;
    let f = |m: u32, q: u32, l: u32| {
        find_mode(
            &shape,
            ModeIndex::Cylinder {
                m,
                q,
                l,
                sine: false,
            },
        )
        .frequency(c)
            / 1e3
    };
    for (got, want) in [
        (f(1, 0, 0), 4.02), // first azimuthal
        (f(2, 0, 0), 6.67), // m = 2
        (f(0, 0, 1), 6.86), // first axial
        (f(1, 0, 1), 7.95), // (1,0,1)
        (f(0, 1, 0), 8.37), // first radial
    ] {
        assert!((got - want).abs() <= 0.005, "{got} vs {want}");
    }
    let modes = shape.modes_below(2.0 * PI * 10_000.0 / c);
    let distinct = modes::distinct_frequencies(&modes, c, 1e-9);
    let expect = [4.02, 6.67, 6.86, 7.95, 8.37];
    for (g, e) in distinct[1..6].iter().zip(expect) {
        assert!((g / 1e3 - e).abs() <= 0.005, "{g} vs {e}");
    }
    let r = reference();
    let list = r["modes"]["cylinder"]["modes_below_10k"]
        .as_array()
        .unwrap();
    assert_eq!(list.len(), modes.len());
    for (m, rec) in modes.iter().zip(list) {
        let want = rec[4].as_f64().unwrap();
        assert!((m.frequency(c) - want).abs() <= 1e-11 * want.max(1.0));
        let idx = ModeIndex::Cylinder {
            m: rec[0].as_u64().unwrap() as u32,
            q: rec[1].as_u64().unwrap() as u32,
            l: rec[2].as_u64().unwrap() as u32,
            sine: rec[3].as_bool().unwrap(),
        };
        assert!((find_mode(&shape, idx).frequency(c) - want).abs() <= 1e-11 * want.max(1.0));
    }
}

#[test]
fn truncation_keeps_the_e27_mode_counts() {
    // Erratum E27: 3× the highest analysed wavenumber keeps about 1.4k modes
    // up to 20 kHz and about 10k up to 40 kHz (60 × 45 × 20 mm, 343 m/s).
    let r = reference();
    let counts = &r["modes"]["box"]["counts_3x"];
    let shape = Shape::Box {
        lx: 0.060,
        ly: 0.045,
        lz: 0.020,
    };
    for (fmax, approx, tol) in [(20_000.0, 1400.0, 0.05), (40_000.0, 10_000.0, 0.10)] {
        let k = 3.0 * 2.0 * PI * fmax / 343.0;
        let n = shape.modes_below(k).len();
        let key = format!("{}", fmax as u64);
        assert_eq!(n as u64, counts[&key].as_u64().unwrap(), "{fmax}");
        // The fixture reports how close the nearest eigenwavenumber is to the
        // cutoff, so an exact count is meaningful.
        assert!(counts[format!("gap_{key}")].as_f64().unwrap() > 1e-9);
        assert!(
            ((n as f64) / approx - 1.0).abs() < tol,
            "{n} modes at {fmax} Hz"
        );
    }
    let cyl = Shape::Cylinder {
        radius: 0.025,
        depth: 0.025,
    };
    let n = cyl.modes_below(3.0 * 2.0 * PI * 20_000.0 / 343.0).len();
    assert_eq!(
        n as u64,
        r["modes"]["cylinder"]["counts_3x"]["20000"]
            .as_u64()
            .unwrap()
    );
}

#[test]
fn bessel_values_and_derivative_roots() {
    let r = reference();
    for rec in r["bessel"]["j"].as_array().unwrap() {
        let m = rec[0].as_u64().unwrap() as usize;
        let x = rec[1].as_f64().unwrap();
        let want = rec[2].as_f64().unwrap();
        let got = modes::bessel_jn(m, x);
        // Miller's recurrence: absolute error ~1e-15 of the O(1) envelope.
        assert!(
            (got - want).abs() <= 1e-14 + 1e-12 * want.abs(),
            "J_{m}({x}) = {got}, want {want}"
        );
        if m == 1 {
            assert!((modes::bessel_j1(x) - want).abs() <= 1e-14 + 1e-12 * want.abs());
        }
    }
    let roots = modes::bessel_jp_zeros_all(60.0);
    for rec in r["bessel"]["jp_roots"].as_array().unwrap() {
        let m = rec[0].as_u64().unwrap() as usize;
        for (got, want) in roots[m].iter().zip(f64s(&rec[1])) {
            assert!((got - want).abs() < 1e-12 * want, "j'_{m}: {got} vs {want}");
        }
    }
}

#[test]
fn footprint_means_and_wall_integrals_match_quadrature() {
    let r = reference();
    let p = &r["patches"];
    let to_patch = |rec: &Value, cyl_radius: Option<f64>| -> Patch {
        let face = match rec[0].as_str().unwrap() {
            "x0" => Face::Box {
                axis: 0,
                far: false,
            },
            "x1" => Face::Box { axis: 0, far: true },
            "y0" => Face::Box {
                axis: 1,
                far: false,
            },
            "y1" => Face::Box { axis: 1, far: true },
            "z0" if cyl_radius.is_none() => Face::Box {
                axis: 2,
                far: false,
            },
            "z1" if cyl_radius.is_none() => Face::Box { axis: 2, far: true },
            "z0" => Face::End { far: false },
            "z1" => Face::End { far: true },
            _ => Face::Side,
        };
        let u = rec[1].as_f64().unwrap();
        let u = match face {
            Face::Side => cyl_radius.unwrap() * u.to_radians(),
            _ => u,
        };
        let fp = &rec[3];
        let footprint = if fp[0] == "disk" {
            Footprint::Disk {
                radius: fp[1].as_f64().unwrap(),
            }
        } else {
            Footprint::Rect {
                du: fp[1].as_f64().unwrap(),
                dv: fp[2].as_f64().unwrap(),
            }
        };
        Patch {
            face,
            u,
            v: rec[2].as_f64().unwrap(),
            footprint,
        }
    };
    // Box.
    let d = f64s(&p["box"]["dims"]);
    let shape = Shape::Box {
        lx: d[0],
        ly: d[1],
        lz: d[2],
    };
    let patches: Vec<Patch> = p["box"]["patches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| to_patch(x, None))
        .collect();
    for rec in p["box"]["modes"].as_array().unwrap() {
        let n = f64s(&rec["n"]);
        let md = find_mode(
            &shape,
            ModeIndex::Box {
                n: [n[0] as u32, n[1] as u32, n[2] as u32],
            },
        );
        for (patch, want) in patches.iter().zip(f64s(&rec["means"])) {
            patch.check(&shape).unwrap();
            let got = md.patch_average(&shape, patch);
            // 2-D Gauss–Legendre reference, accurate to ~1e-13.
            assert!(
                (got - want).abs() < 1e-11,
                "{:?} {patch:?}: {got} vs {want}",
                md.index
            );
        }
        let w = f64s(&rec["wall"]);
        let (i, j) = md.wall_integrals(&shape);
        assert!((i - w[0]).abs() < 1e-11 * w[0]);
        assert!((j - w[1]).abs() < 1e-10 * w[1].max(1.0), "{j} vs {}", w[1]);
    }
    // Cylinder.
    let (a, dep) = (
        p["cylinder"]["radius"].as_f64().unwrap(),
        p["cylinder"]["depth"].as_f64().unwrap(),
    );
    let shape = Shape::Cylinder {
        radius: a,
        depth: dep,
    };
    let patches: Vec<Patch> = p["cylinder"]["patches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| to_patch(x, Some(a)))
        .collect();
    let all = shape.modes_below(2500.0);
    for rec in p["cylinder"]["modes"].as_array().unwrap() {
        let ix = &rec["index"];
        let idx = ModeIndex::Cylinder {
            m: ix[0].as_u64().unwrap() as u32,
            q: ix[1].as_u64().unwrap() as u32,
            l: ix[2].as_u64().unwrap() as u32,
            sine: ix[3].as_bool().unwrap(),
        };
        let md = *all.iter().find(|m| m.index == idx).unwrap();
        // Normalisation: amplitude² = ε_m ε_l / ⟨J_m²⟩ with ⟨J_m²⟩ by quadrature.
        let (m, l) = (ix[0].as_u64().unwrap(), ix[2].as_u64().unwrap());
        let e = if m > 0 { 2.0 } else { 1.0 } * if l > 0 { 2.0 } else { 1.0 };
        let mean = rec["disk_mean_j2"].as_f64().unwrap();
        assert!((md.amplitude.powi(2) - e / mean).abs() < 1e-11 * e / mean);
        let averages = modes::patch_averages(&shape, &[md], &patches);
        for ((patch, want), cached) in patches.iter().zip(f64s(&rec["means"])).zip(&averages) {
            patch.check(&shape).unwrap();
            let got = md.patch_average(&shape, patch);
            assert!(
                (got - want).abs() < 1e-11,
                "{:?} {patch:?}: {got} vs {want}",
                md.index
            );
            assert!((cached[0] - got).abs() < 1e-15);
        }
        let w = f64s(&rec["wall"]);
        let (i, j) = md.wall_integrals(&shape);
        assert!((i - w[0]).abs() < 1e-10 * w[0]);
        assert!((j - w[1]).abs() < 1e-9 * w[1].max(1.0), "{j} vs {}", w[1]);
    }
}

// ----- The element: lumped limit ------------------------------------------------

fn four_port_box(level: u8, extra: Value, freqs: &[f64]) -> Circuit {
    let ports = vec![
        json!({"node": "p0", "face": "z0", "x_mm": 5, "y_mm": 3, "radius_mm": 15}),
        json!({"node": "p1", "face": "z1", "x_mm": 18, "y_mm": 10, "radius_mm": 4}),
        json!({"node": "p2", "face": "z1", "x_mm": -27, "y_mm": -19.5, "radius_mm": 2}),
        json!({"node": "p3", "face": "x1", "y_mm": 5, "z_mm": 10, "radius_mm": 3}),
    ];
    circuit(
        level,
        box_cavity(ports, extra),
        &node_names(4),
        vec![json!({"id": "src", "type": "flow_source", "node": "p0", "U_m3_per_s": 1e-6})],
        freqs,
    )
}

#[test]
fn zeroth_mode_is_exactly_the_lumped_compliance() {
    let c = four_port_box(1, json!({"f_max_kHz": 20}), &[100.0]);
    let m = modal(&c, "cav");
    let air = air();
    let uniform = m.shape.modes_below(1.0)[0];
    assert_eq!(uniform.k, 0.0);
    for f in [10.0, 100.0, 1000.0, 5000.0, 20_000.0] {
        let w = 2.0 * PI * f;
        let lumped = (C64::new(0.0, w) * m.lumped_compliance(&air, w)).inv();
        // The general modal term of the uniform mode, Morse–Ingard loss included.
        let term = m.prefactor(&air, w) / m.denominator(&uniform, &air, w);
        assert!(
            (term - lumped).norm() < 1e-13 * lumped.norm(),
            "{f}: {term} vs {lumped}"
        );
        // Same compliance as the `cavity` element (shared function, same area).
        let cav = acoustilab::elements::cavity::wall_layer_compliance(
            &air,
            w,
            0.06 * 0.045 * 0.02,
            2.0 * (0.06 * 0.045 + 0.045 * 0.02 + 0.06 * 0.02),
            true,
        );
        assert_eq!(cav, m.lumped_compliance(&air, w));
        // Level 0: every entry is that compliance's impedance.
        for z in m.impedance_matrix(&air, w, 0) {
            assert_eq!(z, lumped);
        }
    }
}

#[test]
fn level_zero_equals_the_lumped_cavity_element() {
    // Switching levels never changes the netlist: at L0 the modal cavity is
    // one lumped compliance with its ports joined, identical to `cavity`.
    let freqs = [20.0, 200.0, 2000.0, 9000.0];
    let c = four_port_box(0, json!({}), &freqs);
    let doc = json!({
        "air": {"preset": "spec_reference"},
        "sweep": {"frequencies_Hz": freqs},
        "level": 0,
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [
            {"id": "cav", "type": "cavity", "node": "a", "lx_mm": 60, "ly_mm": 45, "lz_mm": 20},
            {"id": "src", "type": "flow_source", "node": "a", "U_m3_per_s": 1e-6}
        ]
    });
    let lumped = Circuit::from_json(&doc.to_string()).unwrap();
    for f in freqs {
        let x = c.solve_at(f).unwrap();
        let y = lumped.solve_at(f).unwrap();
        let pl = lumped.node_value(&y, "a").unwrap();
        for i in 0..4 {
            let p = c.node_value(&x, &format!("p{i}")).unwrap();
            assert!((p - pl).norm() < 1e-12 * pl.norm(), "{f} Hz port {i}");
        }
    }
}

#[test]
fn low_frequency_agrees_with_lumped_level_e4() {
    // Continuity rule (erratum E4): below the lumped 3 % frequency (kL = 0.3)
    // the levels agree within 0.3 dB, below kL ≈ 0.17 (1 %) within 0.1 dB.
    // L is the element's validity length, the largest dimension. A flow
    // source drives port 0 (the driver); every port pressure is compared.
    let cylinder_ports = vec![
        json!({"node": "p0", "face": "z0", "x_mm": 4, "y_mm": -3, "radius_mm": 15}),
        json!({"node": "p1", "face": "z1", "x_mm": -10, "y_mm": 8, "radius_mm": 4}),
        json!({"node": "p2", "face": "side", "angle_deg": 120, "z_mm": 20, "radius_mm": 2}),
        json!({"node": "p3", "face": "side", "angle_deg": 250, "z_mm": 3, "arc_mm": 8, "lz_mm": 1}),
    ];
    let src = json!({"id": "src", "type": "flow_source", "node": "p0", "U_m3_per_s": 1e-6});
    let build = |level: u8, cylinder: bool, freqs: &[f64]| {
        if cylinder {
            let cav = json!({"id": "cav", "type": "modal_cavity", "radius_mm": 25,
                "depth_mm": 25, "f_max_kHz": 20, "ports": cylinder_ports.clone()});
            circuit(level, cav, &node_names(4), vec![src.clone()], freqs)
        } else {
            four_port_box(level, json!({"f_max_kHz": 20}), freqs)
        }
    };
    let c = 343.0;
    for (cylinder, l) in [(false, 0.060), (true, 0.050)] {
        let f3 = 0.3 * c / (2.0 * PI * l);
        let f1 = 0.17 * c / (2.0 * PI * l);
        let freqs: Vec<f64> = (0..40)
            .map(|i| 10.0 * (f3 / 10.0).powf(i as f64 / 39.0))
            .collect();
        let l1 = build(1, cylinder, &freqs);
        let l0 = build(0, cylinder, &freqs);
        let mut worst1: f64 = 0.0;
        let mut worst3: f64 = 0.0;
        for &f in &freqs {
            let (x1, x0) = (l1.solve_at(f).unwrap(), l0.solve_at(f).unwrap());
            for i in 0..4 {
                let n = format!("p{i}");
                let p1 = l1.node_value(&x1, &n).unwrap().norm();
                let p0 = l0.node_value(&x0, &n).unwrap().norm();
                let d = db(p1 / p0).abs();
                worst3 = worst3.max(d);
                if f <= f1 {
                    worst1 = worst1.max(d);
                }
            }
        }
        println!("cylinder {cylinder}: {worst1:.3} dB below kL 0.17, {worst3:.3} dB below kL 0.3");
        assert!(worst1 < 0.1, "{worst1} dB below kL 0.17");
        assert!(worst3 < 0.3, "{worst3} dB below kL 0.3");
    }
}

#[test]
fn lumped_versus_line_follows_the_e3_metric() {
    // Erratum E3: the lumped input compliance of a closed duct is off by
    // −20·log10(kL·cot kL): 0.27 dB at kL = 0.3 and 3.85 dB at kL = 1.0.
    // Depth L = 20 mm; lossless so the closed forms are exact.
    let depth = 0.020;
    let c = 343.0;
    for (kl, want) in [(0.3, 0.2662), (0.5, 0.7693), (1.0, 3.8480)] {
        let f = kl * c / (2.0 * PI * depth);
        let closed = -db(kl / f64::tan(kl));
        assert!((closed - want).abs() < 1e-4);
        // `cavity` with two nodes: L1 depth line vs L0 compliance.
        let z = |level: u8| {
            let doc = json!({
                "air": {"preset": "spec_reference"},
                "sweep": {"frequencies_Hz": [f]},
                "level": level,
                "nodes": [{"id": "a", "domain": "acoustic"}, {"id": "b", "domain": "acoustic"}],
                "elements": [
                    {"id": "cav", "type": "cavity", "nodes": ["a", "b"], "lx_mm": 60, "ly_mm": 45,
                     "lz_mm": 20, "wall_loss": false},
                    {"id": "src", "type": "flow_source", "node": "a", "U_m3_per_s": 1.0}
                ]
            });
            let cc = Circuit::from_json(&doc.to_string()).unwrap();
            cc.node_value(&cc.solve_at(f).unwrap(), "a").unwrap().norm()
        };
        let got = db(z(0) / z(1));
        assert!((got - want).abs() < 1e-3, "cavity kL={kl}: {got} dB");
        // The modal cavity with full-face ports on both ends: same metric.
        let ports = vec![
            json!({"node": "p0", "face": "z0", "x_mm": 0, "y_mm": 0, "lx_mm": 60, "ly_mm": 45}),
            json!({"node": "p1", "face": "z1", "x_mm": 0, "y_mm": 0, "lx_mm": 60, "ly_mm": 45}),
        ];
        let zm = |level: u8| {
            let cc = circuit(
                level,
                box_cavity(ports.clone(), json!({"wall_loss": false, "f_max_kHz": 20})),
                &node_names(2),
                vec![json!({"id": "src", "type": "flow_source", "node": "p0", "U_m3_per_s": 1.0})],
                &[f],
            );
            cc.node_value(&cc.solve_at(f).unwrap(), "p0")
                .unwrap()
                .norm()
        };
        let got = db(zm(0) / zm(1));
        assert!((got - want).abs() < 2e-3, "modal kL={kl}: {got} dB");
    }
}

// ----- Reciprocity and passivity -------------------------------------------------

/// Eigenvalues of a real symmetric matrix (cyclic Jacobi).
fn sym_eigenvalues(mut a: Vec<f64>, n: usize) -> Vec<f64> {
    let norm: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    for _ in 0..100 {
        let off: f64 = (0..n)
            .flat_map(|p| ((p + 1)..n).map(move |q| (p, q)))
            .map(|(p, q)| a[p * n + q].powi(2))
            .sum();
        if off.sqrt() <= 1e-15 * norm {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let apq = a[p * n + q];
                if apq == 0.0 {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (akp, akq) = (a[k * n + p], a[k * n + q]);
                    a[k * n + p] = c * akp - s * akq;
                    a[k * n + q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let (apk, aqk) = (a[p * n + k], a[q * n + k]);
                    a[p * n + k] = c * apk - s * aqk;
                    a[q * n + k] = s * apk + c * aqk;
                }
            }
        }
    }
    (0..n).map(|i| a[i * n + i]).collect()
}

/// Eigenvalues of the Hermitian part (Z + Z^H)/2 (each appears twice).
fn hermitian_part_eigenvalues(z: &[C64], n: usize) -> Vec<f64> {
    let m = 2 * n;
    let mut a = vec![0.0; m * m];
    for i in 0..n {
        for j in 0..n {
            let h = 0.5 * (z[i * n + j] + z[j * n + i].conj());
            a[i * m + j] = h.re;
            a[(i + n) * m + (j + n)] = h.re;
            a[i * m + (j + n)] = -h.im;
            a[(i + n) * m + j] = h.im;
        }
    }
    sym_eigenvalues(a, m)
}

/// Ports on four different walls, disks and a slit-like rectangle.
fn mixed_ports() -> Vec<Value> {
    vec![
        json!({"node": "p0", "face": "z0", "x_mm": 5, "y_mm": 3, "radius_mm": 15}),
        json!({"node": "p1", "face": "z1", "x_mm": 18, "y_mm": 10, "radius_mm": 4}),
        json!({"node": "p2", "face": "y1", "x_mm": -10, "z_mm": 12, "lx_mm": 10, "lz_mm": 0.5}),
        json!({"node": "p3", "face": "x0", "y_mm": -5, "z_mm": 8, "radius_mm": 2}),
    ]
}

#[test]
fn impedance_matrix_is_reciprocal_and_passive() {
    let air = air();
    let c = circuit(
        1,
        box_cavity(mixed_ports(), json!({"f_max_kHz": 20})),
        &node_names(4),
        vec![],
        &[100.0],
    );
    let m = modal(&c, "cav");
    let mut f = 20.0;
    while f < 20_000.0 {
        for level in [0, 1] {
            let w = 2.0 * PI * f;
            let z = m.impedance_matrix(&air, w, level);
            let scale = z.iter().map(|x| x.norm()).fold(0.0, f64::max);
            for i in 0..4 {
                for j in 0..4 {
                    assert!((z[i * 4 + j] - z[j * 4 + i]).norm() <= 1e-10 * scale);
                }
            }
            let ev = hermitian_part_eigenvalues(&z, 4);
            let emax = ev.iter().cloned().fold(0.0, f64::max);
            assert!(
                ev.iter().all(|&e| e >= -1e-12 * scale),
                "f={f} level={level}: {ev:?} (scale {scale})"
            );
            assert!(emax > 0.0, "wall loss must dissipate");
        }
        f *= 1.13;
    }
    // Lossless: the Hermitian part vanishes (the residual is reactive).
    let c = circuit(
        1,
        box_cavity(mixed_ports(), json!({"f_max_kHz": 20, "wall_loss": false})),
        &node_names(4),
        vec![],
        &[100.0],
    );
    let m = modal(&c, "cav");
    for f in [100.0, 3100.0, 11_000.0] {
        let z = m.impedance_matrix(&air, 2.0 * PI * f, 1);
        let scale = z.iter().map(|x| x.norm()).fold(0.0, f64::max);
        for e in hermitian_part_eigenvalues(&z, 4) {
            assert!(e.abs() < 1e-12 * scale);
        }
    }
}

#[test]
fn network_transfer_is_reciprocal() {
    // Drive port a, read port b, with the other ports loaded by a resistance
    // and a tube; then swap. Reciprocity of the whole network requires the
    // stamped N-port to be reciprocal.
    for f in [150.0, 2900.0, 7300.0, 15_000.0] {
        let run = |from: &str, to: &str| {
            let extra = vec![
                json!({"id": "r1", "type": "acoustic_resistance", "node": "p1", "R_Pa_s_per_m3": 3e6}),
                json!({"id": "t2", "type": "tube", "node": "p2", "radius_mm": 1, "length_mm": 4}),
                json!({"id": "src", "type": "flow_source", "node": from, "U_m3_per_s": 1e-6}),
            ];
            let c = circuit(
                1,
                box_cavity(mixed_ports(), json!({"f_max_kHz": 20})),
                &node_names(4),
                extra,
                &[f],
            );
            c.node_value(&c.solve_at(f).unwrap(), to).unwrap()
        };
        let ab = run("p0", "p3");
        let ba = run("p3", "p0");
        assert!((ab - ba).norm() < 1e-10 * ab.norm(), "{f}: {ab} vs {ba}");
    }
}

// ----- Against the independent mixed-representation reference ------------------

/// Errors of Z against the reference, per pair, normalised by
/// |Z_ref,ij| + 0.01·sqrt(s_i·s_j), s_i = ρc/A_i: a relative error, except
/// that near a zero of Z_ij it is measured against 1 % of the ports' own
/// characteristic impedance.
fn compare_to_reference(
    shape_json: Value,
    ports: &[Value],
    zref: &Value,
    residual: bool,
) -> Vec<(f64, Vec<Vec<f64>>)> {
    let n = ports.len();
    let recs: Vec<Value> = ports
        .iter()
        .enumerate()
        .map(|(i, p)| port_json(&format!("p{i}"), p))
        .collect();
    let mut cav = shape_json;
    cav["id"] = json!("cav");
    cav["type"] = json!("modal_cavity");
    cav["ports"] = json!(recs);
    cav["wall_loss"] = json!(false);
    cav["f_max_kHz"] = json!(20);
    cav["residual"] = json!(residual);
    let c = circuit(1, cav, &node_names(n), vec![], &[100.0]);
    let m = modal(&c, "cav");
    let air = air();
    let area: Vec<f64> = m.ports.iter().map(|p| p.patch.footprint.area()).collect();
    let mut out = Vec::new();
    for rec in zref.as_array().unwrap() {
        let f = rec["f"].as_f64().unwrap();
        let z = m.impedance_matrix(&air, 2.0 * PI * f, 1);
        let im = rec["im_Z"].as_array().unwrap();
        let mut e = vec![vec![0.0; n]; n];
        for i in 0..n {
            let row = f64s(&im[i]);
            for j in 0..n {
                let want = C64::new(0.0, row[j]);
                let s = air.rho_c() / (area[i] * area[j]).sqrt();
                e[i][j] = (z[i * n + j] - want).norm() / (want.norm() + 0.01 * s);
            }
        }
        out.push((f, e));
    }
    out
}

/// Tolerance against the mixed-representation reference (f_max = 20 kHz).
/// The residual makes the error of the omitted modes O((k/k_cut)⁴) of their
/// share, k_cut = 3·k_max: negligible in the lower half of the band and
/// growing to about (1/3)⁴ ≈ 1 % of that share at f_max. The references
/// themselves converge to better than 1e-6 (tools/cavity/generate.py).
fn reference_tolerance(f: f64) -> f64 {
    let x = f / 20_000.0;
    if x < 0.25 {
        1e-4
    } else if x < 0.5 {
        3e-4
    } else if x < 0.75 {
        1e-3
    } else {
        1e-2
    }
}

fn print_errors(label: &str, errs: &[(f64, Vec<Vec<f64>>)]) {
    for (f, e) in errs {
        let rows: Vec<String> = e
            .iter()
            .map(|r| {
                r.iter()
                    .map(|x| format!("{x:.1e}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        println!("{label} f={f}: [{}]", rows.join(" | "));
    }
}

#[test]
fn box_impedance_matches_mixed_representation() {
    let r = reference();
    let zr = &r["z"]["box"];
    let ports = zr["ports"].as_array().unwrap().clone();
    let shape = json!({"lx_mm": 60, "ly_mm": 45, "lz_mm": 20});
    let errs = compare_to_reference(shape.clone(), &ports, &zr["Z"], true);
    print_errors("box", &errs);
    for (f, e) in &errs {
        let tol = reference_tolerance(*f);
        for (i, row) in e.iter().enumerate() {
            for (j, &x) in row.iter().enumerate() {
                assert!(x < tol, "f={f} ({i},{j}): {x:.2e}");
            }
        }
    }
    // Without the residual the small ports' driving-point impedances are far
    // off even at 2 kHz: plain truncation is not enough.
    let errs = compare_to_reference(shape, &ports, &zr["Z"], false);
    print_errors("box, no residual", &errs);
    let e = &errs.iter().find(|(f, _)| *f > 2000.0).unwrap().1;
    assert!(e[0][0] > 0.1 && e[3][3] > 0.1, "{e:?}");
}

#[test]
fn cylinder_impedance_matches_mixed_representation() {
    let r = reference();
    let zr = &r["z"]["cylinder"];
    let ports = zr["ports"].as_array().unwrap().clone();
    let shape = json!({"radius_mm": 25, "depth_mm": 20});
    let errs = compare_to_reference(shape, &ports, &zr["Z"], true);
    print_errors("cylinder", &errs);
    for (f, e) in &errs {
        let tol = reference_tolerance(*f);
        for (i, row) in e.iter().enumerate() {
            for (j, &x) in row.iter().enumerate() {
                assert!(x < tol, "f={f} ({i},{j}): {x:.2e}");
            }
        }
    }
}

// ----- Resonances and symmetry --------------------------------------------------

/// Frequency of the largest |Z_ij| within ±`span` of `f0` (grid search and a
/// parabolic refinement on the log magnitude).
fn peak_near(m: &ModalCavity, i: usize, j: usize, f0: f64, span: f64) -> f64 {
    let air = air();
    let n = m.ports.len();
    let mag = |f: f64| {
        m.impedance_matrix(&air, 2.0 * PI * f, 1)[i * n + j]
            .norm()
            .ln()
    };
    let pts = 600;
    let grid: Vec<f64> = (0..=pts)
        .map(|k| f0 * (1.0 - span + 2.0 * span * k as f64 / pts as f64))
        .collect();
    let vals: Vec<f64> = grid.iter().map(|&f| mag(f)).collect();
    let k = (0..vals.len())
        .max_by(|&a, &b| vals[a].total_cmp(&vals[b]))
        .unwrap();
    assert!(k > 0 && k < pts, "no interior peak near {f0} Hz");
    let (a, b, c) = (vals[k - 1], vals[k], vals[k + 1]);
    let h = grid[1] - grid[0];
    grid[k] + 0.5 * h * (a - c) / (a - 2.0 * b + c)
}

#[test]
fn closed_box_resonances_land_on_the_eigenfrequencies() {
    // Spec Section 9 gate: rigid-cavity eigenfrequencies within 0.5 %. A
    // small off-centre piston drives the box and a small port on the far
    // face listens; wall loss on (the loss shifts a peak by about 1/(2Q)).
    let ports = vec![
        json!({"node": "p0", "face": "z0", "x_mm": -20, "y_mm": -14, "radius_mm": 3}),
        json!({"node": "p1", "face": "z1", "x_mm": 18, "y_mm": 10, "radius_mm": 3}),
    ];
    let c = circuit(
        1,
        box_cavity(ports, json!({"f_max_kHz": 20})),
        &node_names(2),
        vec![],
        &[100.0],
    );
    let m = modal(&c, "cav");
    let air = air();
    let distinct =
        modes::distinct_frequencies(&m.shape.modes_below(2.0 * PI * 9000.0 / air.c), air.c, 1e-9);
    for &fn_ in &distinct[1..9] {
        let fp = peak_near(m, 1, 0, fn_, 0.02);
        println!(
            "mode {fn_:.1} Hz: peak {fp:.1} Hz ({:+.3} %)",
            100.0 * (fp / fn_ - 1.0)
        );
        assert!(
            (fp / fn_ - 1.0).abs() < 0.005,
            "peak {fp} Hz vs mode {fn_} Hz"
        );
    }
    // Cylinder, radius 25 mm, depth 25 mm: the first five distinct modes.
    let ports = vec![
        json!({"node": "p0", "face": "z0", "x_mm": 8, "y_mm": -5, "radius_mm": 3}),
        json!({"node": "p1", "face": "z1", "x_mm": -12, "y_mm": 6, "radius_mm": 3}),
    ];
    let cav = json!({"id": "cav", "type": "modal_cavity", "radius_mm": 25, "depth_mm": 25,
        "f_max_kHz": 20, "ports": ports});
    let c = circuit(1, cav, &node_names(2), vec![], &[100.0]);
    let m = modal(&c, "cav");
    let distinct =
        modes::distinct_frequencies(&m.shape.modes_below(2.0 * PI * 9000.0 / air.c), air.c, 1e-9);
    for &fn_ in &distinct[1..6] {
        let fp = peak_near(m, 1, 0, fn_, 0.012);
        println!(
            "mode {fn_:.1} Hz: peak {fp:.1} Hz ({:+.3} %)",
            100.0 * (fp / fn_ - 1.0)
        );
        assert!(
            (fp / fn_ - 1.0).abs() < 0.005,
            "peak {fp} Hz vs mode {fn_} Hz"
        );
    }
}

#[test]
fn centred_piston_does_not_excite_antisymmetric_modes() {
    let air = air();
    let shape = Shape::Box {
        lx: 0.060,
        ly: 0.045,
        lz: 0.020,
    };
    let centred = Patch {
        face: Face::Box {
            axis: 2,
            far: false,
        },
        u: 0.0,
        v: 0.0,
        footprint: Footprint::Disk { radius: 0.015 },
    };
    let all = shape.modes_below(3.0 * 2.0 * PI * 20_000.0 / air.c);
    for md in &all {
        let ModeIndex::Box { n } = md.index else {
            unreachable!()
        };
        let c = md.patch_average(&shape, &centred);
        if n[0] % 2 == 1 || n[1] % 2 == 1 {
            assert!(c.abs() < 1e-12, "{n:?}: {c}");
        }
    }
    // The element keeps only symmetric modes when every port is centred.
    let ports = vec![
        json!({"node": "p0", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 15}),
        json!({"node": "p1", "face": "z1", "x_mm": 0, "y_mm": 0, "radius_mm": 4}),
    ];
    let c = circuit(
        1,
        box_cavity(ports, json!({"f_max_kHz": 20})),
        &node_names(2),
        vec![],
        &[100.0],
    );
    let d = modal(&c, "cav").data();
    assert!(d.modes.len() < d.modes_below_cutoff / 3);
    for md in &d.modes {
        let ModeIndex::Box { n } = md.index else {
            unreachable!()
        };
        assert!(n[0] % 2 == 0 && n[1] % 2 == 0, "{n:?}");
    }
    // With an off-centre listener the (1,0,0) mode exists in the cavity but
    // the centred piston does not drive it: no peak at 2858 Hz, while an
    // off-centre piston produces one.
    let f100 = 343.0 / (2.0 * 0.060);
    let ratio = |x_mm: f64| {
        let ports = vec![
            json!({"node": "p0", "face": "z0", "x_mm": x_mm, "y_mm": 0, "radius_mm": 15}),
            json!({"node": "p1", "face": "z1", "x_mm": 20, "y_mm": 12, "radius_mm": 4}),
        ];
        let c = circuit(
            1,
            box_cavity(ports, json!({"f_max_kHz": 20})),
            &node_names(2),
            vec![],
            &[100.0],
        );
        let m = modal(&c, "cav");
        let z = |f: f64| m.impedance_matrix(&air, 2.0 * PI * f, 1)[2].norm();
        z(f100) / (0.5 * (z(0.97 * f100) + z(1.03 * f100)))
    };
    let centred_ratio = ratio(0.0);
    let offset_ratio = ratio(-12.0);
    assert!(
        (centred_ratio - 1.0).abs() < 0.05,
        "centred: {centred_ratio}"
    );
    assert!(offset_ratio > 5.0, "offset: {offset_ratio}");
    // Cylinder: a centred disk couples to m = 0 only.
    let cyl = Shape::Cylinder {
        radius: 0.025,
        depth: 0.025,
    };
    let centred = Patch {
        face: Face::End { far: false },
        u: 0.0,
        v: 0.0,
        footprint: Footprint::Disk { radius: 0.01 },
    };
    for md in cyl.modes_below(3.0 * 2.0 * PI * 20_000.0 / air.c) {
        let ModeIndex::Cylinder { m, .. } = md.index else {
            unreachable!()
        };
        if m > 0 {
            assert!(md.patch_average(&cyl, &centred).abs() < 1e-12);
        }
    }
}

// ----- Depth line versus axial modes -------------------------------------------

/// p0/U and p1/U of the two-node `cavity` depth line (L1), and of a modal
/// cavity with full-face ports on both ends, driven at the first face.
fn line_and_modal(
    shape: Value,
    full_ports: Vec<Value>,
    wall_loss: bool,
    f: f64,
) -> ([C64; 2], [C64; 2]) {
    let mut line = shape.clone();
    line["id"] = json!("cav");
    line["type"] = json!("cavity");
    line["nodes"] = json!(["p0", "p1"]);
    line["wall_loss"] = json!(wall_loss);
    let src = json!({"id": "src", "type": "flow_source", "node": "p0", "U_m3_per_s": 1.0});
    let doc = json!({
        "air": {"preset": "spec_reference"}, "sweep": {"frequencies_Hz": [f]}, "level": 1,
        "nodes": [{"id": "p0", "domain": "acoustic"}, {"id": "p1", "domain": "acoustic"}],
        "elements": [line, src.clone()]
    });
    let cl = Circuit::from_json(&doc.to_string()).unwrap();
    let xl = cl.solve_at(f).unwrap();
    let mut modal_cav = shape;
    modal_cav["id"] = json!("cav");
    modal_cav["type"] = json!("modal_cavity");
    modal_cav["ports"] = json!(full_ports);
    modal_cav["wall_loss"] = json!(wall_loss);
    modal_cav["f_max_kHz"] = json!(20);
    let cm = circuit(1, modal_cav, &node_names(2), vec![src], &[f]);
    let xm = cm.solve_at(f).unwrap();
    let get = |c: &Circuit, x: &[C64]| {
        [
            c.node_value(x, "p0").unwrap(),
            c.node_value(x, "p1").unwrap(),
        ]
    };
    (get(&cl, &xl), get(&cm, &xm))
}

#[test]
fn depth_line_agrees_with_axial_modes() {
    // Full-face pistons on both ends excite only the (0,0,n) modes, so the
    // modal cavity must reproduce the plane-wave depth line of `cavity`.
    // Errors are relative to max(|Z|, ρc/S), so that the zeros of Z_11 (the
    // quarter-wave depth resonances) do not turn a small absolute error into
    // an unbounded relative one.
    let cases = [
        (
            json!({"lx_mm": 60, "ly_mm": 45, "lz_mm": 20}),
            0.060 * 0.045,
            vec![
                json!({"node": "p0", "face": "z0", "x_mm": 0, "y_mm": 0, "lx_mm": 60, "ly_mm": 45}),
                json!({"node": "p1", "face": "z1", "x_mm": 0, "y_mm": 0, "lx_mm": 60, "ly_mm": 45}),
            ],
        ),
        (
            json!({"radius_mm": 25, "depth_mm": 25}),
            PI * 0.025 * 0.025,
            vec![
                json!({"node": "p0", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 25}),
                json!({"node": "p1", "face": "z1", "x_mm": 0, "y_mm": 0, "radius_mm": 25}),
            ],
        ),
    ];
    let zc = |s: f64| air().rho_c() / s;
    for (shape, area, ports) in cases {
        let mut f = 20.0;
        let (mut lossless_lo, mut lossless_hi, mut lossy) = (0.0f64, 0.0f64, 0.0f64);
        while f < 20_000.0 {
            // Lossless: the line is exactly −j(ρc/S)·cot(kL) and −j(ρc/S)/sin(kL);
            // the modal sum with its residual differs by the O((k/k_cut)⁴)
            // dynamic tail of the omitted axial modes.
            let (l, m) = line_and_modal(shape.clone(), ports.clone(), false, f);
            for i in 0..2 {
                let e = (m[i] - l[i]).norm() / l[i].norm().max(zc(area));
                if f < 10_000.0 {
                    lossless_lo = lossless_lo.max(e);
                } else {
                    lossless_hi = lossless_hi.max(e);
                }
            }
            // Lossy: Morse–Ingard per mode against the thermoviscous line.
            // Relative error, or error against ρc/S where |Z| < ρc/S: the
            // depth of the notches of Z_11 is set by the loss alone, and the
            // diagonal perturbation apportions the end-face thermal loss of
            // superposed modes without their cross terms. The line itself
            // uses the exact circular shape function at the equivalent radius,
            // which differs from the thin-layer limit by O((δ/r)²), 1e-4 at 20 Hz.
            let (l, m) = line_and_modal(shape.clone(), ports.clone(), true, f);
            for i in 0..2 {
                let e = (m[i] - l[i]).norm() / l[i].norm().max(zc(area));
                lossy = lossy.max(e);
            }
            f *= 1.037;
        }
        println!("{shape}: lossless {lossless_lo:.1e} (<10 kHz), {lossless_hi:.1e} (10-20 kHz); lossy {lossy:.1e}");
        assert!(lossless_lo < 1e-4, "{shape}: {lossless_lo}");
        // Top octave: the O((k/k_cut)⁴) tail grows to (1/3)⁴ ≈ 1 % of the
        // omitted modes' share at f_max.
        assert!(lossless_hi < 3e-3, "{shape}: {lossless_hi}");
        assert!(lossy < 2e-3, "{shape}: {lossy}");
    }
}

// ----- Wall-loss Q (E29) ------------------------------------------------------

#[test]
fn wall_loss_q_of_modes() {
    let r = reference();
    let air = air();
    let cases = [
        ("box", json!({"lx_mm": 60, "ly_mm": 45, "lz_mm": 20})),
        ("cylinder", json!({"radius_mm": 25, "depth_mm": 25})),
    ];
    for (name, shape) in cases {
        let mut cav = shape;
        cav["id"] = json!("cav");
        cav["type"] = json!("modal_cavity");
        cav["ports"] = json!([{"node": "p0", "face": "z0", "x_mm": 3, "y_mm": 2, "radius_mm": 5}]);
        let c = circuit(1, cav, &node_names(1), vec![], &[100.0]);
        let m = modal(&c, "cav");
        let all = m.shape.modes_below(2.0 * PI * 9000.0 / air.c);
        for rec in r["q"][name].as_array().unwrap() {
            let f = rec["f"].as_f64().unwrap();
            let md = all
                .iter()
                .find(|md| {
                    (md.frequency(air.c) / f - 1.0).abs() < 1e-12 && {
                        match md.index {
                            ModeIndex::Box { n } => json!(n) == rec["n"],
                            ModeIndex::Cylinder { m, q, l, sine } => {
                                json!([m, q, l, sine]) == rec["index"]
                            }
                        }
                    }
                })
                .unwrap();
            let q = m.quality_factor(md, &air);
            let w = md.k * air.c;
            let (t, _) = m.loss_parts(md, &air, w);
            let q_thermal = md.k * md.k / t;
            assert!(
                (q / rec["Q"].as_f64().unwrap() - 1.0).abs() < 1e-8,
                "{name} {f}: Q {q}"
            );
            assert!((q_thermal / rec["Q_thermal"].as_f64().unwrap() - 1.0).abs() < 1e-8);
            // Erratum E29: "at mode frequencies it is 500–1000". That is the
            // thermal (lumped ε) figure, 1/ε at the mode frequency; the
            // viscous layer, which the Morse–Ingard perturbation includes,
            // roughly halves it for modes with tangential motion.
            let eps =
                (air.gamma - 1.0) * air.thermal_layer(w) * m.wall_area / (2.0 * m.shape.volume());
            if name == "box" && f < 8200.0 {
                // The 60 mm box's first seven distinct modes (2.86–8.14 kHz).
                assert!(
                    (500.0..=1000.0).contains(&(1.0 / eps)),
                    "1/eps = {}",
                    1.0 / eps
                );
            }
            assert!((150.0..600.0).contains(&q), "{name} {f} Hz: Q = {q}");
            println!(
                "{name} {f:.0} Hz: Q = {q:.0}, thermal only {q_thermal:.0}, 1/eps {:.0}",
                1.0 / eps
            );
        }
    }
}

#[test]
fn resonance_bandwidth_gives_the_modal_q() {
    // A centred piston and a centred listener in a cylinder see only m = 0
    // modes; the axial (0,0,1) mode at 6.86 kHz is isolated (the nearest
    // m = 0 mode, (0,1,0), is 22 % higher). Its −3 dB bandwidth in p1/U must
    // give the element's Q within 2 % (the non-resonant background of the
    // other modes skews the half-power points slightly).
    let ports = vec![
        json!({"node": "p0", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 10}),
        json!({"node": "p1", "face": "z1", "x_mm": 0, "y_mm": 0, "radius_mm": 3}),
    ];
    let cav = json!({"id": "cav", "type": "modal_cavity", "radius_mm": 25, "depth_mm": 25,
        "f_max_kHz": 20, "ports": ports});
    let c = circuit(1, cav, &node_names(2), vec![], &[100.0]);
    let m = modal(&c, "cav");
    let air = air();
    let md = *m
        .shape
        .modes_below(2000.0)
        .iter()
        .find(|x| {
            x.index
                == ModeIndex::Cylinder {
                    m: 0,
                    q: 0,
                    l: 1,
                    sine: false,
                }
        })
        .unwrap();
    let q = m.quality_factor(&md, &air);
    let fn_ = md.frequency(air.c);
    let mag = |f: f64| m.impedance_matrix(&air, 2.0 * PI * f, 1)[2].norm();
    let fp = peak_near(m, 1, 0, fn_, 0.02);
    let half = mag(fp) / 2f64.sqrt();
    let edge = |dir: f64| {
        let (mut a, mut b) = (fp, fp * (1.0 + dir * 0.02));
        for _ in 0..80 {
            let mid = 0.5 * (a + b);
            if mag(mid) > half {
                a = mid;
            } else {
                b = mid;
            }
        }
        0.5 * (a + b)
    };
    let q_meas = fp / (edge(1.0) - edge(-1.0));
    println!("(0,0,1): Q {q:.1}, from bandwidth {q_meas:.1}");
    assert!((q_meas / q - 1.0).abs() < 0.02, "{q_meas} vs {q}");
}

// ----- Power balance -----------------------------------------------------------

#[test]
fn power_balances_with_a_modal_cavity() {
    // Tellegen: a driver into a four-port front cavity with an ear
    // resistance, a leak slit and a radiating vent.
    let doc = |level: u8| {
        json!({
            "air": {"preset": "spec_reference"},
            "sweep": {"frequencies_Hz": [100.0]},
            "level": level,
            "nodes": [
                {"id": "e1", "domain": "electrical"}, {"id": "e2", "domain": "electrical"},
                {"id": "m", "domain": "mechanical"},
                {"id": "drv", "domain": "acoustic"}, {"id": "ear", "domain": "acoustic"},
                {"id": "leak", "domain": "acoustic"}, {"id": "vent", "domain": "acoustic"},
                {"id": "vent_out", "domain": "acoustic"}
            ],
            "elements": [
                {"id": "amp", "type": "vsource", "nodes": ["e1"], "V_V": 1.0},
                {"id": "coil", "type": "coil", "nodes": ["e1", "e2"], "Re_ohm": 32},
                {"id": "motor", "type": "motor", "nodes": ["e2", "gnd", "m", "gnd"], "Bl_Tm": 1.0},
                {"id": "susp", "type": "suspension", "node": "m",
                 "Mms_kg": 1e-4, "Cms_m_per_N": 1e-3, "Rms_Ns_per_m": 0.05},
                {"id": "dia", "type": "piston", "nodes": ["m", "gnd", "drv", "gnd"], "Sd_cm2": 10},
                {"id": "cav", "type": "modal_cavity", "lx_mm": 60, "ly_mm": 45, "lz_mm": 20,
                 "f_max_kHz": 20, "ports": [
                    {"node": "drv", "face": "z0", "x_mm": 5, "y_mm": 3, "radius_mm": 17.8},
                    {"node": "ear", "face": "z1", "x_mm": 18, "y_mm": 10, "radius_mm": 4},
                    {"node": "leak", "face": "y1", "x_mm": -10, "z_mm": 12, "lx_mm": 10, "lz_mm": 0.5},
                    {"node": "vent", "face": "x0", "y_mm": -5, "z_mm": 8, "radius_mm": 2}
                 ]},
                {"id": "ear_r", "type": "acoustic_resistance", "node": "ear", "R_Pa_s_per_m3": 2e7},
                {"id": "slit", "type": "slit", "node": "leak", "gap_mm": 0.3, "width_mm": 10, "length_mm": 5},
                {"id": "vent_t", "type": "tube", "nodes": ["vent", "vent_out"], "radius_mm": 2, "length_mm": 2},
                {"id": "rad", "type": "radiation", "node": "vent_out", "radius_mm": 2}
            ]
        })
    };
    for level in [0, 1] {
        let c = Circuit::from_json(&doc(level).to_string()).unwrap();
        for f in [20.0, 700.0, 2858.0, 6500.0, 15_000.0] {
            let x = c.solve_at(f).unwrap();
            let p = c.power_absorbed(f, &x);
            assert!(p.iter().all(|(_, v)| v.is_some()), "{p:?}");
            let delivered = -p.iter().find(|(id, _)| id == "amp").unwrap().1.unwrap();
            let cav = p.iter().find(|(id, _)| id == "cav").unwrap().1.unwrap();
            let sum: f64 = p.iter().map(|(_, v)| v.unwrap()).sum();
            assert!(delivered > 0.0);
            assert!(cav >= -1e-12 * delivered, "cavity absorbs {cav}");
            assert!(sum.abs() < 1e-10 * delivered, "level {level}, f={f}: {p:?}");
        }
    }
}

// ----- Validity and input checking ----------------------------------------------

#[test]
fn validity_limits_follow_the_level() {
    let c = four_port_box(1, json!({"f_max_kHz": 16}), &[100.0]);
    let v = c.validity();
    let lim = v.iter().find(|l| l.element == "cav").unwrap();
    assert_eq!(lim.begin_hz, Some(16_000.0));
    assert_eq!(lim.deep_hz, Some(48_000.0));
    let c = four_port_box(0, json!({}), &[100.0]);
    let lim = c
        .validity()
        .into_iter()
        .find(|l| l.element == "cav")
        .unwrap();
    // Lumped criterion on the largest dimension (60 mm): 10 % at 493 Hz.
    assert!((lim.begin_hz.unwrap() - 493.0).abs() < 3.0);
}

#[test]
fn rejects_malformed_modal_cavities() {
    let good_port = json!({"node": "a", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 5});
    let base = || {
        json!({"id": "cav", "type": "modal_cavity", "lx_mm": 60, "ly_mm": 45, "lz_mm": 20,
               "ports": [good_port.clone()]})
    };
    let try_build = |cav: Value| {
        let doc = json!({
            "nodes": [{"id": "a", "domain": "acoustic"}, {"id": "b", "domain": "acoustic"},
                      {"id": "m", "domain": "mechanical"}],
            "elements": [cav]
        });
        Circuit::from_json(&doc.to_string())
            .err()
            .map(|e| e.to_string())
    };
    assert!(try_build(base()).is_none(), "the base case must build");
    let with = |f: &dyn Fn(&mut Value)| {
        let mut v = base();
        f(&mut v);
        v
    };
    let port = |p: Value| move |v: &mut Value| v["ports"] = json!([p.clone()]);
    let cases: Vec<(Value, &str)> = vec![
        (
            with(&|v| {
                v.as_object_mut().unwrap().remove("ports");
            }),
            "missing 'ports'",
        ),
        (with(&|v| v["ports"] = json!([])), "non-empty"),
        (with(&|v| v["node"] = json!("a")), "from 'ports'"),
        (
            with(&|v| {
                let o = v.as_object_mut().unwrap();
                o.remove("lx_mm");
                o.remove("ly_mm");
                o.remove("lz_mm");
                o.insert("volume_cm3".into(), json!(30));
            }),
            "no modes",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z2", "x_mm": 0, "y_mm": 0, "radius_mm": 5}),
            )),
            "unknown face",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "side", "angle_deg": 0, "z_mm": 5, "radius_mm": 2}),
            )),
            "unknown face",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 5, "lx_mm": 3, "ly_mm": 3}),
            )),
            "one footprint",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z0", "x_mm": 0, "y_mm": 0, "lx_mm": 3}),
            )),
            "one footprint",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z0", "x_mm": 0, "y_mm": 0, "lx_mm": 3, "lz_mm": 3}),
            )),
            "one footprint",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z0", "x_mm": 28, "y_mm": 0, "radius_mm": 5}),
            )),
            "beyond",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "x1", "y_mm": 0, "z_mm": 19, "radius_mm": 2}),
            )),
            "beyond",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z0", "y_mm": 0, "radius_mm": 5}),
            )),
            "missing 'x'",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z0", "x": 0, "y_mm": 0, "radius_mm": 5}),
            )),
            "unit suffix",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z0", "x_mm": 0, "y_mm": 0, "raduis_mm": 5}),
            )),
            "one footprint",
        ),
        (
            with(&port(
                json!({"node": "a", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 5, "z_mm": 1}),
            )),
            "misspelled",
        ),
        (
            with(&port(
                json!({"face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 5}),
            )),
            "missing 'node'",
        ),
        (
            with(&port(
                json!({"node": "m", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 5}),
            )),
            "expected Acoustic",
        ),
        (
            with(&port(
                json!({"node": "zz", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 5}),
            )),
            "unknown node",
        ),
        (with(&port(json!("a"))), "must be an object"),
        (
            with(&|v| v["ports"] = json!([good_port.clone(), good_port.clone()])),
            "share node",
        ),
        (
            with(&|v| {
                v["ports"] = json!([good_port.clone(),
            {"node": "b", "face": "z0", "x_mm": 8, "y_mm": 0, "radius_mm": 5}])
            }),
            "overlap",
        ),
        (with(&|v| v["surface_factor"] = json!(20)), "surface_factor"),
        (with(&|v| v["f_max"] = json!(20000)), "unit suffix"),
    ];
    for (cav, needle) in cases {
        let msg = try_build(cav.clone()).unwrap_or_else(|| panic!("accepted: {cav}"));
        assert!(msg.contains(needle), "'{msg}' lacks '{needle}' for {cav}");
    }
    // Touching footprints and a full-face piston are fine; so is a port to
    // ambient (a pressure-release aperture).
    let ok = with(&|v| {
        v["ports"] = json!([
            {"node": "a", "face": "z0", "x_mm": 0, "y_mm": 0, "lx_mm": 60, "ly_mm": 45},
            {"node": "b", "face": "z1", "x_mm": -5, "y_mm": 0, "radius_mm": 5},
            {"node": "gnd", "face": "z1", "x_mm": 5, "y_mm": 0, "radius_mm": 5}
        ])
    });
    assert!(try_build(ok).is_none());
    let cyl_ok = json!({"id": "cav", "type": "modal_cavity", "radius_mm": 25, "depth_mm": 20,
        "ports": [{"node": "a", "face": "side", "angle_deg": 90, "z_mm": 10, "arc_mm": 6, "lz_mm": 2},
                  {"node": "b", "face": "z0", "x_mm": 0, "y_mm": 0, "radius_mm": 25}]});
    assert!(try_build(cyl_ok).is_none());
}

// ----- Cost ----------------------------------------------------------------------

/// Per-frequency cost of a four-port 60 × 45 × 20 mm modal cavity. Run with
/// `cargo test --release -p acoustilab --test cavity -- --ignored --nocapture`.
#[test]
#[ignore]
fn per_frequency_cost() {
    let air = air();
    for f_max in [20.0, 40.0] {
        let c = circuit(
            1,
            box_cavity(mixed_ports(), json!({"f_max_kHz": f_max})),
            &node_names(4),
            vec![],
            &[100.0],
        );
        let m = modal(&c, "cav");
        let patches: Vec<Patch> = m.ports.iter().map(|p| p.patch).collect();
        let ts = std::time::Instant::now();
        let st = modes::static_sums(&m.shape, &patches, m.k_cut);
        println!(
            "  static sums {:.1} ms (transverse cutoffs {:?} rad/m)",
            ts.elapsed().as_secs_f64() * 1e3,
            st.k_transverse
        );
        let t0 = std::time::Instant::now();
        let d = m.data();
        let build = t0.elapsed();
        let freqs: Vec<f64> = (0..576)
            .map(|i| 10.0 * 4000f64.powf(i as f64 / 575.0))
            .collect();
        let t1 = std::time::Instant::now();
        let mut acc = C64::new(0.0, 0.0);
        for &f in &freqs {
            acc += m.impedance_matrix(&air, 2.0 * PI * f, 1)[0];
        }
        let per = t1.elapsed().as_secs_f64() / freqs.len() as f64;
        let t2 = std::time::Instant::now();
        for &f in &freqs {
            let _ = c.solve_at(f).unwrap();
        }
        let solve = t2.elapsed().as_secs_f64() / freqs.len() as f64;
        println!(
            "f_max {f_max} kHz: {} modes below cutoff, {} retained, build {:.1} ms, \
             Z {:.1} µs/frequency, full solve {:.1} µs/frequency ({acc})",
            d.modes_below_cutoff,
            d.modes.len(),
            build.as_secs_f64() * 1e3,
            per * 1e6,
            solve * 1e6
        );
    }
}
