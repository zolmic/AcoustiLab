//! Ear loads (spec Section 7): the conical canal chain, drum terminations and
//! the ear-simulator macros, checked against closed forms, an independent
//! numerical integration of the lossy horn equations, independent Python
//! implementations (tools/ear/gen_fixtures.py) and the headline values the
//! spec and the standards state.

use acoustilab::elements::ear::{
    chain_abcd, cone_abcd, AreaFunction, Cone, HuddeEngel, Iec711, Type43Drum,
};
use acoustilab::mna::abcd_mul;
use acoustilab::thermoviscous::{self, Section};
use acoustilab::{AirState, Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::PI;
use std::path::Path;

fn reference() -> Value {
    let text = include_str!("data/ear_reference.json");
    serde_json::from_str(text).unwrap()
}

fn c(v: &Value) -> C64 {
    C64::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
}

fn mat(v: &Value) -> [C64; 4] {
    [c(&v[0]), c(&v[1]), c(&v[2]), c(&v[3])]
}

fn rel(a: C64, b: C64) -> f64 {
    (a - b).norm() / b.norm()
}

fn air() -> AirState {
    AirState::standard_23c()
}

fn w(f: f64) -> f64 {
    2.0 * PI * f
}

/// Circuit with a unit volume-velocity source into `node` and the given
/// elements; probes `p_drp` (pressure at `drp`, if given) and `zin`.
fn driven(elements: Value, extra_nodes: &[&str], drp: Option<&str>) -> Circuit {
    let mut nodes = vec![json!({"id": "a", "domain": "acoustic"})];
    for n in extra_nodes {
        nodes.push(json!({"id": n, "domain": "acoustic"}));
    }
    let mut els = vec![json!({"id": "src", "type": "flow_source", "node": "a", "U_m3_per_s": 1.0})];
    els.extend(elements.as_array().unwrap().iter().cloned());
    let mut probes = vec![json!({"id": "zin", "quantity": "impedance", "element": "src"})];
    if let Some(d) = drp {
        probes.push(json!({"id": "p_drp", "quantity": "pressure", "node": d}));
    }
    let doc = json!({
        "schema": "acoustilab-netlist/0.1",
        "air": {"preset": "standard_23C"},
        "sweep": {"frequencies_Hz": [1000.0]},
        "nodes": nodes,
        "elements": els,
        "probes": probes
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

fn probe(c: &Circuit, id: &str, f: f64) -> C64 {
    let x = c.solve_at(f).unwrap();
    let p = c.probes.iter().find(|p| p.id == id).unwrap();
    c.probe_value(p, f, &x).unwrap()
}

// ----- Conical segments -----------------------------------------------------

#[test]
fn det_t_is_one_for_every_cone() {
    // Reciprocity: det T = 1 exactly for any passive reciprocal two-port.
    let a = air();
    for (r1, r2, l) in [
        (2e-3, 4e-3, 10e-3),
        (4e-3, 1e-3, 3e-3),
        (3.7e-3, 3.7e-3, 12e-3),
        (0.3e-3, 0.31e-3, 0.5e-3),
    ] {
        for f in [5.0, 100.0, 3000.0, 20_000.0, 40_000.0] {
            for lossy in [false, true] {
                let [ta, tb, tc, td] = cone_abcd(Cone { length: l, r1, r2 }, &a, w(f), lossy);
                let det = ta * td - tb * tc;
                assert!(
                    (det - 1.0).norm() < 1e-12,
                    "det {det} for {r1} {r2} {l} at {f} Hz"
                );
            }
        }
    }
}

#[test]
fn cascade_of_identical_cylinders_equals_one_uniform_tube() {
    // Exact multiplicativity of the uniform-line matrix: N segments of L/N
    // reproduce the thermoviscous tube of length L to rounding.
    let a = air();
    let (r, l) = (3.75e-3, 25e-3);
    for n in [1usize, 7, 40, 200] {
        let cones = AreaFunction::uniform(l, PI * r * r).unwrap().segments(n);
        assert_eq!(cones.len(), n);
        for f in [20.0, 1000.0, 9000.0, 20_000.0] {
            let t = chain_abcd(&cones, &a, w(f), true);
            let tube = thermoviscous::abcd(&Section::Circle { radius: r }, &a, w(f), l);
            let zc = a.rho_c() / (PI * r * r);
            let scaled = |m: [C64; 4]| [m[0], m[1] / zc, m[2] * zc, m[3]];
            let (s1, s2) = (scaled(t), scaled(tube));
            for k in 0..4 {
                assert!(
                    (s1[k] - s2[k]).norm() < 1e-12 * n as f64,
                    "n={n} f={f} entry {k}: {} vs {}",
                    s1[k],
                    s2[k]
                );
            }
        }
    }
}

#[test]
fn lossless_cone_matches_textbook_apex_form() {
    let a = air();
    for e in reference()["cone_lossless"].as_array().unwrap() {
        let cone = Cone {
            length: e["length_mm"].as_f64().unwrap() * 1e-3,
            r1: e["r1_mm"].as_f64().unwrap() * 1e-3,
            r2: e["r2_mm"].as_f64().unwrap() * 1e-3,
        };
        let f = e["f_Hz"].as_f64().unwrap();
        let t = cone_abcd(cone, &a, w(f), false);
        let r = mat(&e["T"]);
        for k in 0..4 {
            assert!(
                (t[k] - r[k]).norm() <= 1e-11 * r[k].norm().max(1e-30) + 1e-20,
                "{cone:?} {f} Hz entry {k}: {} vs {}",
                t[k],
                r[k]
            );
        }
    }
}

#[test]
fn lossless_cone_subdivision_is_exact() {
    // A cone cut into N shorter cones is the same cone: without losses the
    // chain equals the single-segment matrix to rounding.
    let a = air();
    let f = AreaFunction::new(vec![0.0, 18e-3], vec![PI * 4e-6, PI * 16e-6]).unwrap();
    for freq in [50.0, 5000.0, 18_000.0] {
        let one = chain_abcd(&f.segments(1), &a, w(freq), false);
        let many = chain_abcd(&f.segments(64), &a, w(freq), false);
        for k in 0..4 {
            assert!(
                (one[k] - many[k]).norm() < 1e-10 * one[k].norm().max(1e-9),
                "{freq} Hz entry {k}: {} vs {}",
                one[k],
                many[k]
            );
        }
    }
}

/// Normalised matrix distance between a chain and a reference.
fn matrix_error(t: [C64; 4], r: [C64; 4], z0: f64) -> f64 {
    let s = |m: [C64; 4]| [m[0], m[1] / z0, m[2] * z0, m[3]];
    let (a, b) = (s(t), s(r));
    let scale = b.iter().map(|v| v.norm()).fold(0.0, f64::max);
    (0..4).map(|k| (a[k] - b[k]).norm()).fold(0.0, f64::max) / scale
}

#[test]
fn conical_chain_converges_to_the_lossy_horn_equation() {
    // Reference: the lossy horn equations integrated with the medium at the
    // local radius (scipy DOP853, rtol 1e-11). The chain's only error is the
    // loss variation inside a segment, so it falls as N^-2. Observed
    // (normalised entries, docs/ear-loads.md): 1.0e-6 at N = 40 for the
    // 2 -> 4 mm cone at 10 kHz and 5e-7 for the P.57 canal; the tolerance
    // is 5e-6.
    let a = air();
    for h in reference()["webster"].as_array().unwrap() {
        let xs: Vec<f64> = h["positions_mm"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() * 1e-3)
            .collect();
        let areas: Vec<f64> = h["radii_mm"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| PI * (v.as_f64().unwrap() * 1e-3).powi(2))
            .collect();
        let prof = AreaFunction::new(xs, areas.clone()).unwrap();
        let z0 = a.rho_c() / areas[0];
        for e in h["reference"].as_array().unwrap() {
            let f = e["f_Hz"].as_f64().unwrap();
            let r = mat(&e["T"]);
            let err: Vec<f64> = [10usize, 20, 40, 80]
                .iter()
                .map(|&n| matrix_error(chain_abcd(&prof.segments(n), &a, w(f), true), r, z0))
                .collect();
            let name = h["name"].as_str().unwrap();
            assert!(err[2] < 5e-6, "{name} {f} Hz: N=40 error {:e}", err[2]);
            // Second-order convergence while the discretisation error
            // dominates the reference's own error (~1e-9).
            if err[1] > 1e-7 {
                let order = (err[1] / err[3]).log2() / 2.0;
                assert!(
                    (1.6..=2.4).contains(&order),
                    "{name} {f} Hz: order {order} from {err:?}"
                );
            }
        }
    }
}

/// Lossless exponential horn S = S0·e^{2mx} (Webster), closed form:
/// p = e^{−mx}(a cos βx + b sin βx) with β² = k² − m² gives the forward
/// matrix M, [p(L); U(L)] = M·[p(0); U(0)], and T = M⁻¹ (det M = 1).
fn exponential_horn(s0: f64, s1: f64, len: f64, air: &AirState, omega: f64) -> [C64; 4] {
    let m = (s1 / s0).ln() / (2.0 * len);
    let k = omega / air.c;
    let beta = C64::new(k * k - m * m, 0.0).sqrt();
    let (c, s) = ((beta * len).cos(), (beta * len).sin());
    let z = C64::new(0.0, omega * air.rho / s0);
    let (e_minus, e_plus) = ((-m * len).exp(), (m * len).exp());
    let m11 = (c + s * m / beta) * e_minus;
    let m12 = -(z * s / beta) * e_minus;
    let m21 = (s * k * k / (z * beta)) * e_plus;
    let m22 = (c - s * m / beta) * e_plus;
    [m22, -m12, -m21, m11]
}

#[test]
fn exponential_profile_converges_to_the_exact_horn() {
    // A canal given as an exponential profile is approximated by cones; the
    // lossless chain converges to the closed-form Webster horn as N^-2.
    // Observed: normalised error 1.3e-5 at N = 40 above cut-off (757 Hz) and
    // 8e-7 at 100 Hz; tolerance 5e-5.
    let a = air();
    let (s0, s1, len) = (30e-6, 60e-6, 25e-3);
    let z0 = a.rho_c() / s0;
    for f in [100.0, 2000.0, 10_000.0] {
        let exact = exponential_horn(s0, s1, len, &a, w(f));
        let err: Vec<f64> = [10usize, 20, 40, 80]
            .iter()
            .map(|&n| {
                let x: Vec<f64> = (0..=n).map(|i| len * i as f64 / n as f64).collect();
                let s: Vec<f64> = (0..=n)
                    .map(|i| s0 * (s1 / s0).powf(i as f64 / n as f64))
                    .collect();
                let prof = AreaFunction::new(x, s).unwrap();
                matrix_error(chain_abcd(&prof.segments(n), &a, w(f), false), exact, z0)
            })
            .collect();
        assert!(err[2] < 5e-5, "{f} Hz: {err:?}");
        let order = (err[0] / err[3]).log2() / 3.0;
        assert!(
            (1.9..=2.1).contains(&order),
            "{f} Hz: order {order}, {err:?}"
        );
    }
    // The netlist form builds the same chain.
    let ckt = driven(
        json!([{"id": "c", "type": "canal", "nodes": ["a", "d"], "length_mm": 25,
                "area_entrance_mm2": 30, "area_end_mm2": 60, "profile": "exponential",
                "segments": 80, "wall_loss": false}]),
        &["d"],
        Some("d"),
    );
    let exact = exponential_horn(s0, s1, len, &a, w(2000.0));
    // Rigid far end: transfer impedance p2/U1 = 1/C.
    let p = probe(&ckt, "p_drp", 2000.0);
    assert!(rel(p, exact[2].inv()) < 1e-4, "{p} vs {}", exact[2].inv());
}

#[test]
fn canal_element_orientation_and_profile_forms() {
    // Reversing the positions reverses the two-port (A and D swap).
    let a = air();
    let fwd = AreaFunction::new(vec![0.0, 5e-3, 20e-3], vec![40e-6, 50e-6, 30e-6]).unwrap();
    let rev = AreaFunction::new(vec![20e-3, 5e-3, 0.0], vec![30e-6, 50e-6, 40e-6]).unwrap();
    let tf = chain_abcd(&fwd.segments(30), &a, w(4000.0), true);
    let tr = chain_abcd(&rev.segments(30), &a, w(4000.0), true);
    assert!(rel(tr[0], tf[3]) < 1e-12 && rel(tr[3], tf[0]) < 1e-12);
    assert!(rel(tr[1], tf[1]) < 1e-12 && rel(tr[2], tf[2]) < 1e-12);

    // Mid-area + taper and entrance/end areas describe the same cone.
    let zin = |params: Value| {
        let mut el = json!({"id": "c", "type": "canal", "nodes": ["a", "d"], "segments": 20});
        for (k, v) in params.as_object().unwrap() {
            el[k] = v.clone();
        }
        let ckt = driven(json!([el]), &["d"], Some("d"));
        probe(&ckt, "p_drp", 3000.0)
    };
    let r0 = (40e-6 / PI).sqrt();
    let r1 = (10e-6 / PI).sqrt();
    let mid = PI * (0.5 * (r0 + r1)).powi(2) * 1e6;
    let p1 = zin(json!({"length_mm": 20, "area_entrance_mm2": 40, "area_end_mm2": 10}));
    let p2 = zin(json!({"length_mm": 20, "area_mm2": mid, "taper": 0.25}));
    let p3 = zin(json!({"positions_mm": [0, 20], "areas_mm2": [40, 10]}));
    assert!(rel(p2, p1) < 1e-12 && rel(p3, p1) < 1e-12, "{p1} {p2} {p3}");
}

#[test]
fn canal_reports_transverse_and_stinson_limits() {
    let doc = json!({
        "nodes": [{"id": "a", "domain": "acoustic"}, {"id": "d", "domain": "acoustic"}],
        "elements": [{"id": "c", "type": "canal", "nodes": ["a", "d"],
                      "length_mm": 25, "area_entrance_mm2": 60, "area_end_mm2": 30}]
    });
    let ckt = Circuit::from_json(&doc.to_string()).unwrap();
    let v = ckt.validity();
    let r = (60e-6 / PI).sqrt();
    let stinson = v.iter().find(|l| l.criterion.contains("Stinson")).unwrap();
    assert!(
        (stinson.deep_hz.unwrap() / acoustilab::validity::stinson_bound(r) - 1.0).abs() < 1e-12
    );
    let mode = v
        .iter()
        .find(|l| l.criterion.contains("transverse"))
        .unwrap();
    let cut = acoustilab::validity::circular_cut_on(r, ckt.air.c);
    assert!((mode.deep_hz.unwrap() / cut - 1.0).abs() < 1e-12);
}

#[test]
fn lumped_canal_joins_the_line_at_low_frequency() {
    // L0 is the T-section of the same cones; below kL ~ 0.17 the drum
    // pressure agrees within 0.1 dB (docs/conventions.md continuity rule).
    let make = |level: u8| {
        let doc = json!({
            "level": level,
            "air": {"preset": "standard_23C"},
            "nodes": [{"id": "a", "domain": "acoustic"}, {"id": "d", "domain": "acoustic"}],
            "elements": [
                {"id": "src", "type": "flow_source", "node": "a", "U_m3_per_s": 1e-6},
                {"id": "c", "type": "canal", "nodes": ["a", "d"],
                 "length_mm": 25, "area_entrance_mm2": 50, "area_end_mm2": 35},
                {"id": "drum", "type": "eardrum", "node": "d"}
            ],
            "probes": [{"id": "p", "quantity": "pressure", "node": "d"}]
        });
        Circuit::from_json(&doc.to_string()).unwrap()
    };
    let (l0, l1) = (make(0), make(1));
    let c = l1.air.c;
    let f_max = 0.17 * c / (2.0 * PI * 0.025);
    for f in [20.0, 100.0, 200.0, f_max] {
        let p0 = probe(&l0, "p", f);
        let p1 = probe(&l1, "p", f);
        let d = 20.0 * (p0.norm() / p1.norm()).log10();
        assert!(d.abs() < 0.1, "{f} Hz: {d} dB");
    }
}

// ----- Drum terminations ----------------------------------------------------------

#[test]
fn hudde_engel_matches_independent_implementation() {
    let ckt = driven(
        json!([{"id": "td", "type": "eardrum", "node": "a"}]),
        &[],
        None,
    );
    for e in reference()["hudde_engel"].as_array().unwrap() {
        let f = e["f_Hz"].as_f64().unwrap();
        let z = probe(&ckt, "zin", f);
        assert!(rel(z, c(&e["Z"])) < 1e-9, "{f} Hz: {z} vs {}", c(&e["Z"]));
    }
}

#[test]
fn hudde_engel_is_passive_to_40_khz_and_scales_act_where_expected() {
    let a = air();
    let he = HuddeEngel::human();
    let g = a.bulk_modulus();
    let mut f = 1.0;
    while f <= 40_000.0 {
        assert!(he.impedance(w(f), g).re > 0.0, "Re Z < 0 at {f} Hz");
        f *= 1.05;
    }
    // Spot values against the COMSOL plot digitised in docs/ear-loads.md:
    // |Z| = 26.5 dB re 8e6 Pa s/m^3 at 165 Hz and a minimum of 12.2 dB near
    // 860 Hz (plot read to about +-0.3 dB).
    let db = |f: f64| 20.0 * (he.impedance(w(f), g).norm() / 8e6).log10();
    assert!((db(165.0) - 26.5).abs() < 0.3, "{}", db(165.0));
    assert!((db(865.0) - 12.2).abs() < 0.3, "{}", db(865.0));
    // Doubling the drum compliances lowers the low-frequency impedance.
    let soft = he.scaled(acoustilab::elements::ear::DrumScales {
        r: 1.0,
        m: 1.0,
        c: 2.0,
    });
    assert!(soft.impedance(w(100.0), g).norm() < he.impedance(w(100.0), g).norm());
    // The netlist keys reach the model.
    let base = driven(
        json!([{"id": "td", "type": "eardrum", "node": "a"}]),
        &[],
        None,
    );
    let scaled = driven(
        json!([{"id": "td", "type": "eardrum", "node": "a", "C_scale": 2.0,
                "middle_ear_volume_cm3": 1.0}]),
        &[],
        None,
    );
    let mut m = soft.clone();
    m.v_tcav = 1.0e-6;
    let z = probe(&scaled, "zin", 300.0);
    assert!(rel(z, m.impedance(w(300.0), g)) < 1e-12);
    assert!(rel(probe(&base, "zin", 300.0), he.impedance(w(300.0), g)) < 1e-12);
}

#[test]
fn type43_drum_and_rigid_termination() {
    let a = air();
    let r = reference();
    let ckt = driven(
        json!([{"id": "td", "type": "eardrum", "node": "a", "model": "type43"}]),
        &[],
        None,
    );
    for e in r["type43"].as_array().unwrap() {
        let f = e["f_Hz"].as_f64().unwrap();
        assert!(rel(probe(&ckt, "zin", f), c(&e["drum"])) < 1e-10);
        let d = Type43Drum::fitted().impedance(w(f), a.bulk_modulus());
        assert!(rel(d, c(&e["drum"])) < 1e-10);
    }
    // A rigid drum on a closed tube: input impedance Zc·coth(ΓL) of the tube.
    let rigid = driven(
        json!([
            {"id": "c", "type": "canal", "nodes": ["a", "d"], "length_mm": 20, "area_mm2": 45},
            {"id": "td", "type": "eardrum", "node": "d", "model": "rigid"}
        ]),
        &["d"],
        None,
    );
    let rad = (45e-6 / PI).sqrt();
    for f in [100.0, 2000.0] {
        let t = thermoviscous::abcd(&Section::Circle { radius: rad }, &a, w(f), 0.02);
        let z = probe(&rigid, "zin", f);
        assert!(rel(z, t[0] / t[2]) < 1e-12);
    }
}

// ----- IEC 60318-4 and Type 3.3 ------------------------------------------------------

fn transfer(ckt: &Circuit, f: f64) -> C64 {
    // Unit volume velocity: pressure at the DRP is the transfer impedance.
    probe(ckt, "p_drp", f)
}

fn effective_volume_cm3(z: C64, f: f64) -> f64 {
    air().bulk_modulus() / (w(f) * z.norm()) * 1e6
}

#[test]
fn iec60318_4_matches_independent_implementation() {
    let r = reference();
    let ckt = driven(
        json!([{"id": "e", "type": "iec60318_4", "node": "a"}]),
        &[],
        Some("e.drp"),
    );
    let rigid = driven(
        json!([{"id": "e", "type": "iec60318_4", "node": "a", "microphone": "rigid"}]),
        &[],
        Some("e.drp"),
    );
    for e in r["iec60318_4"].as_array().unwrap() {
        let f = e["f_Hz"].as_f64().unwrap();
        assert!(rel(transfer(&ckt, f), c(&e["transfer"])) < 1e-9, "{f} Hz");
        assert!(rel(probe(&ckt, "zin", f), c(&e["input"])) < 1e-9, "{f} Hz");
        assert!(
            rel(transfer(&rigid, f), c(&e["transfer_rigid_mic"])) < 1e-9,
            "{f} Hz"
        );
    }
    let t33 = driven(
        json!([{"id": "e", "type": "type33", "node": "a"}]),
        &[],
        Some("e.drp"),
    );
    for e in r["type33"].as_array().unwrap() {
        let f = e["f_Hz"].as_f64().unwrap();
        assert!(rel(transfer(&t33, f), c(&e["transfer"])) < 1e-9, "{f} Hz");
    }
}

#[test]
fn iec60318_4_headline_values() {
    // Effective volume 1260 mm^3 at 500 Hz (tolerance +-3 %, the fit target;
    // the fit itself reaches it to 1e-9) and the half-wave resonance at
    // 13.5 +- 1.5 kHz (checked, not fitted).
    let ckt = driven(
        json!([{"id": "e", "type": "iec60318_4", "node": "a"}]),
        &[],
        Some("e.drp"),
    );
    let v = effective_volume_cm3(transfer(&ckt, 500.0), 500.0);
    assert!((v / 1.260 - 1.0).abs() < 0.03, "effective volume {v} cm3");
    let mut best = (0.0, 0.0);
    let mut f = 10_000.0;
    while f <= 17_000.0 {
        let z = transfer(&ckt, f).norm();
        if z > best.1 {
            best = (f, z);
        }
        f += 10.0;
    }
    assert!(
        (best.0 - 13_500.0).abs() <= 1_500.0,
        "half-wave at {} Hz",
        best.0
    );
    // The literature model's unfitted geometry is ~15 % short of 1260 mm^3
    // (docs/ear-loads.md); the side-volume scale restores it.
    assert!(Iec711::fitted_side_volume_scale() > 1.0);
}

#[test]
fn type33_adds_the_extension_in_front_of_the_coupler() {
    // At low frequency the extension adds its volume (10 mm x 7.5 mm bore,
    // 441.8 mm^3) to the coupler's effective volume, enlarged by its wall
    // thermal layer: |gamma P0 / K_eff| = 1.028 at 100 Hz. Tolerance 0.5 %
    // covers the extension's own inertance and phase.
    let t33 = driven(
        json!([{"id": "e", "type": "type33", "node": "a"}]),
        &[],
        Some("e.drp"),
    );
    let e711 = driven(
        json!([{"id": "e", "type": "iec60318_4", "node": "a"}]),
        &[],
        Some("e.drp"),
    );
    let f = 100.0;
    let dv = effective_volume_cm3(probe(&t33, "zin", f), f)
        - effective_volume_cm3(probe(&e711, "zin", f), f);
    let a = air();
    let m = thermoviscous::medium(&Section::Circle { radius: 3.75e-3 }, &a, w(f));
    let ext = PI * 3.75e-3f64.powi(2) * 10e-3 * 1e6 * (a.bulk_modulus() / m.k_eff).norm();
    assert!((dv / ext - 1.0).abs() < 0.005, "{dv} vs {ext}");
}

// ----- Type 4.3 -----------------------------------------------------------------------

#[test]
fn type43_matches_independent_implementation() {
    let r = reference();
    let at_ref = driven(
        json!([{"id": "e", "type": "type43", "node": "a", "input": "ref"}]),
        &[],
        Some("e.drp"),
    );
    let at_eep = driven(
        json!([{"id": "e", "type": "type43", "node": "a"}]),
        &[],
        Some("e.drp"),
    );
    let he = driven(
        json!([{"id": "e", "type": "type43", "node": "a", "input": "ref", "drum": "hudde_engel"}]),
        &[],
        Some("e.drp"),
    );
    for e in r["type43"].as_array().unwrap() {
        let f = e["f_Hz"].as_f64().unwrap();
        assert!(
            rel(transfer(&at_ref, f), c(&e["transfer_ref"])) < 1e-9,
            "{f} Hz"
        );
        assert!(
            rel(probe(&at_ref, "zin", f), c(&e["input_ref"])) < 1e-9,
            "{f} Hz"
        );
        assert!(
            rel(transfer(&at_eep, f), c(&e["transfer_eep"])) < 1e-9,
            "{f} Hz"
        );
    }
    for e in r["type43_hudde_engel"].as_array().unwrap() {
        let f = e["f_Hz"].as_f64().unwrap();
        assert!(
            rel(transfer(&he, f), c(&e["transfer_ref"])) < 1e-9,
            "{f} Hz"
        );
    }
}

#[test]
fn type43_headline_values() {
    // ITU-T P.57 clause 6.4.3.3 NOTE 2: 27.7 MPa s/m^3 at 500 Hz, i.e. an
    // effective volume of 1.63 cm^3; tolerance +-5 % (the Recommendation
    // allows +-0.10 cm^3, 6 %).
    let ckt = driven(
        json!([{"id": "e", "type": "type43", "node": "a", "input": "ref"}]),
        &[],
        Some("e.drp"),
    );
    let z = transfer(&ckt, 500.0);
    assert!((z.norm() / 27.7e6 - 1.0).abs() < 0.05, "|Z| = {}", z.norm());
    let v = effective_volume_cm3(z, 500.0);
    assert!((v / 1.63 - 1.0).abs() < 0.05, "V = {v} cm3");
    // The model reports the thermoviscous line bound near 19 kHz at the
    // widest section between the reference plane and the drum.
    let lim = ckt.validity();
    let st = lim
        .iter()
        .find(|l| l.criterion.contains("Stinson"))
        .unwrap();
    let f = st.deep_hz.unwrap();
    assert!((18_000.0..20_500.0).contains(&f), "{f}");
}

/// ITU-T P.57 Table 5-c: runs only when the verbatim table is present in the
/// untracked private/ directory (tools/ear/p57_geometry.py writes it).
#[test]
fn type43_reproduces_p57_table_5c_when_available() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../private/p57_table5c.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("skipping: {} not present", path.display());
        return;
    };
    let table: Value = serde_json::from_str(&text).unwrap();
    let ckt = driven(
        json!([{"id": "e", "type": "type43", "node": "a", "input": "ref"}]),
        &[],
        Some("e.drp"),
    );
    let z500 = transfer(&ckt, 500.0).norm();
    let mut fails = Vec::new();
    let rows = table["rows"].as_array().unwrap();
    for r in rows {
        let f = r["f_Hz"].as_f64().unwrap();
        let level = 20.0 * (transfer(&ckt, f).norm() * f / (z500 * 500.0)).log10();
        let d = level - r["level_dB"].as_f64().unwrap();
        let (up, lo) = (
            r["tol_upper_dB"].as_f64().unwrap(),
            r["tol_lower_dB"].as_f64().unwrap(),
        );
        if d > up + 1e-6 || d < lo - 1e-6 {
            fails.push((f, d));
        }
    }
    assert_eq!(rows.len(), 121);
    assert!(fails.is_empty(), "outside Table 5-c tolerance: {fails:?}");
}

// ----- Power balance and netlist checks ------------------------------------------------

#[test]
fn power_balances_with_ear_loads_and_internal_node_connections() {
    // Tellegen: all absorbed powers (sources negative) sum to zero, including
    // a resistor hung on a macro's internal node.
    let doc = json!({
        "air": {"preset": "standard_23C"},
        "nodes": [
            {"id": "a", "domain": "acoustic"}, {"id": "b", "domain": "acoustic"},
            {"id": "c", "domain": "acoustic"}, {"id": "d", "domain": "acoustic"},
            {"id": "e", "domain": "acoustic"}, {"id": "g", "domain": "acoustic"}
        ],
        "elements": [
            {"id": "ps", "type": "pressure_source", "node": "a", "p_Pa": 1.0, "Zs_Pa_s_per_m3": 2e7},
            {"id": "canal", "type": "canal", "nodes": ["a", "b"], "length_mm": 22, "area_mm2": 45, "taper": 0.7},
            {"id": "drum", "type": "eardrum", "node": "b"},
            {"id": "fs", "type": "flow_source", "node": "c", "U_m3_per_s": 1e-7},
            {"id": "e711", "type": "iec60318_4", "node": "c"},
            {"id": "leak", "type": "acoustic_resistance", "nodes": ["e711.drp"], "R_Pa_s_per_m3": 5e8},
            {"id": "ps2", "type": "pressure_source", "node": "d", "p_Pa": 0.5, "Zs_Pa_s_per_m3": 1e7},
            {"id": "t43", "type": "type43", "node": "d"},
            {"id": "tie", "type": "acoustic_resistance", "nodes": ["t43.ref", "e"], "R_Pa_s_per_m3": 1e8},
            {"id": "vol", "type": "cavity", "node": "e", "volume_cm3": 0.5},
            {"id": "ps3", "type": "pressure_source", "node": "g", "p_Pa": 0.2, "Zs_Pa_s_per_m3": 1e7},
            {"id": "t33", "type": "type33", "node": "g"},
            {"id": "td43", "type": "eardrum", "node": "t33.ref", "model": "type43"}
        ]
    });
    let ckt = Circuit::from_json(&doc.to_string()).unwrap();
    for f in [20.0, 700.0, 3000.0, 10_600.0, 19_000.0] {
        let x = ckt.solve_at(f).unwrap();
        let p = ckt.power_absorbed(f, &x);
        assert!(p.iter().all(|(_, v)| v.is_some()), "{p:?}");
        let delivered: f64 = p
            .iter()
            .filter(|(id, _)| id.starts_with("ps") || id == "fs")
            .map(|(_, v)| -v.unwrap())
            .sum();
        let sum: f64 = p.iter().map(|(_, v)| v.unwrap()).sum();
        assert!(delivered > 0.0);
        assert!(sum.abs() < 1e-10 * delivered, "f={f}: sum {sum:e}, {p:?}");
        // Every passive ear part absorbs power.
        for id in ["canal", "drum", "e711", "t43", "t33", "td43"] {
            let v = p.iter().find(|(i, _)| i == id).unwrap().1.unwrap();
            assert!(v >= -1e-12 * delivered, "{id} absorbs {v:e} at {f} Hz");
        }
    }
}

#[test]
fn macros_expose_named_nodes() {
    let doc = json!({
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [
            {"id": "src", "type": "flow_source", "node": "a"},
            {"id": "ear", "type": "type43", "node": "a"}
        ],
        "probes": [
            {"id": "eep", "quantity": "pressure", "node": "ear.eep"},
            {"id": "ref", "quantity": "pressure", "node": "ear.ref"},
            {"id": "drp", "quantity": "pressure", "node": "ear.drp"}
        ]
    });
    let ckt = Circuit::from_json(&doc.to_string()).unwrap();
    let x = ckt.solve_at(50.0).unwrap();
    let eep = ckt.probe_value(&ckt.probes[0], 50.0, &x).unwrap();
    let drp = ckt.probe_value(&ckt.probes[2], 50.0, &x).unwrap();
    // At 50 Hz the canal inertance (about 800 kg/m^4) against the drum's
    // ~2 cm^3 leaves the pressure uniform to about 0.1 %.
    assert!(
        (drp.norm() / eep.norm() - 1.0).abs() < 0.01,
        "{drp} vs {eep}"
    );
    for ty in ["iec60318_4", "type33", "type43"] {
        let doc = json!({
            "nodes": [{"id": "a", "domain": "acoustic"}],
            "elements": [{"id": "src", "type": "flow_source", "node": "a"},
                         {"id": "ear", "type": ty, "node": "a"}],
            "probes": [{"id": "eep", "quantity": "pressure", "node": "ear.eep"},
                       {"id": "drp", "quantity": "pressure", "node": "ear.drp"},
                       {"id": "z_ear", "quantity": "impedance", "element": "ear"},
                       {"id": "z_src", "quantity": "impedance", "element": "src"}]
        });
        let ckt = Circuit::from_json(&doc.to_string()).unwrap();
        // Port 0 of the macro is its terminal: its impedance is the ear's
        // input impedance, which is what the source sees.
        for f in [100.0, 4000.0] {
            let x = ckt.solve_at(f).unwrap();
            let z_ear = ckt.probe_value(&ckt.probes[2], f, &x).unwrap();
            let z_src = ckt.probe_value(&ckt.probes[3], f, &x).unwrap();
            assert!(rel(z_ear, z_src) < 1e-12, "{ty} {f} Hz: {z_ear} vs {z_src}");
        }
    }
}

#[test]
fn ear_netlist_errors_are_reported() {
    let bad = |el: Value| {
        let doc = json!({
            "nodes": [{"id": "a", "domain": "acoustic"}, {"id": "d", "domain": "acoustic"}],
            "elements": [el]
        });
        Circuit::from_json(&doc.to_string()).is_err()
    };
    assert!(bad(
        json!({"id": "c", "type": "canal", "nodes": ["a", "d"]})
    ));
    assert!(bad(
        json!({"id": "c", "type": "canal", "nodes": ["a", "d"], "length_mm": 20,
                       "area_mm2": 40, "positions_mm": [0, 20], "areas_mm2": [40, 40]})
    ));
    assert!(bad(json!({"id": "c", "type": "canal", "nodes": ["a", "d"],
                       "positions_mm": [0, 10, 5], "areas_mm2": [40, 40, 40]})));
    assert!(bad(
        json!({"id": "c", "type": "canal", "nodes": ["a", "d"], "length_mm": 20,
                       "area_mm2": 40, "segmnts": 30})
    ));
    assert!(bad(
        json!({"id": "c", "type": "canal", "nodes": ["a"], "length_mm": 20, "area_mm2": 40})
    ));
    assert!(bad(
        json!({"id": "t", "type": "eardrum", "node": "a", "model": "zwislocki"})
    ));
    assert!(bad(
        json!({"id": "t", "type": "eardrum", "node": "a", "model": "rigid", "R_scale": 2})
    ));
    assert!(bad(
        json!({"id": "t", "type": "eardrum", "node": "a", "C_scale": -1})
    ));
    assert!(bad(
        json!({"id": "e", "type": "type43", "node": "a", "input": "concha"})
    ));
    assert!(bad(json!({"id": "e", "type": "iec60318_4", "node": "gnd"})));
    assert!(bad(
        json!({"id": "e", "type": "iec60318_4", "node": "a", "microphone": "bk4134"})
    ));
}

#[test]
fn cone_chain_is_consistent_with_area_function_volume() {
    // Low-frequency compliance of a closed canal equals V/(gamma P0).
    let a = air();
    let prof = AreaFunction::new(vec![0.0, 8e-3, 20e-3], vec![30e-6, 55e-6, 42e-6]).unwrap();
    let t = chain_abcd(&prof.segments(40), &a, w(1.0), false);
    let c_in = (t[2] / t[0]) / C64::new(0.0, w(1.0));
    let expect = prof.volume() / a.bulk_modulus();
    assert!((c_in.re / expect - 1.0).abs() < 1e-6, "{c_in} vs {expect}");
    let _ = abcd_mul;
}
