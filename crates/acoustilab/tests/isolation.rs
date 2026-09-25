//! Passive isolation, the `shell` cup-wall element and bleed (spec
//! Section 10; docs/isolation.md).
//!
//! References: the leak-limited isolation of a sealed rigid cup computed
//! independently with mpmath (`tools/time/isolation_refs.py`,
//! `tests/data/isolation_reference.json`), and closed forms evaluated here:
//! the normal-incidence mass law, the panel impedance, the monopole bleed
//! of spec Appendix C9. Where the network is the closed form the tolerance
//! is 1e-9 relative (observed 3e-15 for the slit-leak cup).

use acoustilab::isolation::{
    bleed, find_ear, insertion_loss, occluded_circuit, outside_document, third_octave_bands,
    IsolationOptions,
};
use acoustilab::params::{Overrides, Parametric};
use acoustilab::{AirState, Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::PI;

fn fixture() -> Value {
    let path = format!(
        "{}/tests/data/isolation_reference.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn example(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../examples/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn opts(probe: &str, entrance: &str) -> IsolationOptions {
    IsolationOptions {
        drum_probe: Some(probe.into()),
        entrance_node: Some(entrance.into()),
        paths: true,
    }
}

fn parse(v: &Value) -> Parametric {
    Parametric::parse(&v.to_string()).unwrap()
}

/// A rigid cup (lossless compliance) with a slit leak to ambient.
fn cup_with_slit(level: u8, freqs: &[f64]) -> Value {
    let air = AirState::spec_reference();
    json!({
        "schema": "acoustilab-netlist/0.2",
        "air": {"preset": "spec_reference"},
        "sweep": {"frequencies_Hz": freqs},
        "level": level,
        "nodes": [{"id": "a_cup", "domain": "acoustic"}],
        "elements": [
            {"id": "cup", "type": "acoustic_compliance", "node": "a_cup",
             "C_m3_per_Pa": 25e-6 / air.bulk_modulus()},
            {"id": "leak", "type": "slit", "nodes": ["a_cup", "ambient"],
             "gap_mm": 0.1, "width_mm": 20, "length_mm": 5}
        ],
        "probes": [{"id": "p_cup", "quantity": "pressure", "node": "a_cup"}]
    })
}

/// Leak-limited isolation (spec Section 10: "low-frequency isolation is
/// leak-limited"): a sealed rigid cup with only a slit to the outside is a
/// first-order divider of the leak impedance and the cup compliance. The
/// cup node is also the "ear", so the open-ear pressure is the outside
/// pressure itself. Against mpmath at L0 and L1: pressures to 1e-9
/// relative (the engine's slit agrees with mpmath to ~1e-13).
#[test]
fn leak_limited_isolation_of_a_sealed_cup() {
    let refs = fixture();
    for case in refs["cases"].as_array().unwrap() {
        let level = case["level"].as_u64().unwrap() as u8;
        let rows = case["rows"].as_array().unwrap();
        let freqs: Vec<f64> = rows.iter().map(|r| r["f_Hz"].as_f64().unwrap()).collect();
        let p = parse(&cup_with_slit(level, &freqs));
        let iso = insertion_loss(&p, &Overrides::new(), &opts("p_cup", "a_cup")).unwrap();
        assert!(iso.ear.elements.is_empty());
        assert_eq!(iso.driven, vec!["leak".to_string()]);
        for (i, r) in rows.iter().enumerate() {
            let want = C64::new(r["re"].as_f64().unwrap(), r["im"].as_f64().unwrap());
            assert!(
                (iso.p_occluded[i] - want).norm() <= 1e-9 * want.norm(),
                "L{level} {} Hz: {} vs {want}",
                freqs[i],
                iso.p_occluded[i]
            );
            assert_eq!(iso.p_open[i], C64::new(1.0, 0.0));
            let il = r["insertion_loss_dB"].as_f64().unwrap();
            assert!((iso.insertion_loss_db[i] - il).abs() < 1e-8);
        }
    }
    // Leak-limited: well below the leak's RC corner f_c = 1/(2πR_pC)
    // (16.6 Hz) the divider is that of the Poiseuille leak R_p + jω·(6/5)M,
    // IL = 10·log10((1 − ω²C·6M/5)² + (f/f_c)²) (0.016 dB at 1 Hz; the
    // thermoviscous corrections at a shear number of 0.03 are ~1e-6 of it),
    // and +6 dB per octave above the corner while the leak is resistive.
    let fc = refs["corner_Hz"].as_f64().unwrap();
    let air = AirState::spec_reference();
    let c = 25e-6 / air.bulk_modulus();
    let m = 1.2 * air.rho * 5e-3 / (20e-3 * 0.1e-3);
    let freqs = [1.0, 200.0, 400.0];
    let p = parse(&cup_with_slit(0, &freqs));
    let iso = insertion_loss(&p, &Overrides::new(), &opts("p_cup", "a_cup")).unwrap();
    let w = 2.0 * PI;
    let divider = 10.0 * ((1.0 - w * w * c * m).powi(2) + (1.0 / fc).powi(2)).log10();
    assert!(
        (iso.insertion_loss_db[0] / divider - 1.0).abs() < 1e-4,
        "{} vs {divider}",
        iso.insertion_loss_db[0]
    );
    let slope = iso.insertion_loss_db[2] - iso.insertion_loss_db[1];
    assert!((slope - 6.0).abs() < 0.3, "{slope}");
}

/// The panel impedance Z = R + jωκm/A² + 1/(jωC), read through an
/// `impedance` probe, for each profile and each way of giving the mass and
/// stiffness (to 1e-12 relative: the element is the closed form).
#[test]
fn shell_impedance_is_the_closed_form() {
    let area = 12e-4; // 12 cm²
    let cases = [
        (
            json!({"surface_density_kg_per_m2": 2.5}),
            2.5 * area,
            1.0,
            "piston",
        ),
        (json!({"mass_g": 3.0}), 3.0e-3, 9.0 / 5.0, "plate"),
        (
            json!({"thickness_mm": 1.5, "density_kg_per_m3": 1200}),
            1.5e-3 * 1200.0 * area,
            4.0 / 3.0,
            "membrane",
        ),
    ];
    for (mass, m, kappa, profile) in cases {
        let mut el = json!({"id": "wall", "type": "shell", "nodes": ["a"], "area_cm2": 12,
                            "profile": profile, "resonance_Hz": 150, "Q": 4});
        for (k, v) in mass.as_object().unwrap() {
            el[k] = v.clone();
        }
        let doc = json!({
            "sweep": {"frequencies_Hz": [50.0, 150.0, 2000.0]},
            "nodes": [{"id": "a", "domain": "acoustic"}],
            "elements": [{"id": "u", "type": "flow_source", "node": "a"}, el],
            "probes": [{"id": "z", "quantity": "impedance", "element": "wall"}]
        });
        let r = Circuit::from_json(&doc.to_string())
            .unwrap()
            .solve()
            .unwrap();
        let ma = kappa * m / (area * area);
        let w0 = 2.0 * PI * 150.0;
        let c = 1.0 / (w0 * w0 * ma);
        let rr = (ma / c).sqrt() / 4.0;
        for (i, f) in [50.0, 150.0, 2000.0].iter().enumerate() {
            let jw = C64::new(0.0, 2.0 * PI * f);
            let want = rr + jw * ma + (jw * c).inv();
            let got = r.probe("z").unwrap().values[i];
            assert!(
                (got - want).norm() <= 1e-12 * want.norm(),
                "{profile} {f}: {got} vs {want}"
            );
        }
        // At resonance the reactances cancel: Z = R.
        assert!((r.probe("z").unwrap().values[1].im).abs() < 1e-9 * rr);
    }
}

/// The normal-incidence mass law, reproduced exactly: a limp panel of
/// surface density m_s between a blocked source 2p (source impedance ρc/A)
/// and a matched load ρc/A transmits p_t/p = 1/(1 + jωm_s/(2ρc)).
/// Above the mass-law corner the loss grows 6.02 dB per doubling of
/// frequency and of mass: the exact step 10·log10((1 + 4x²)/(1 + x²)) at
/// x = ωm_s/(2ρc) approaches 20·log10(2) as 3.26/x² dB (0.0016 dB at
/// x = 45, the case checked).
#[test]
fn shell_follows_the_mass_law() {
    let air = AirState::spec_reference();
    let area = 10e-4;
    let zc = air.rho_c() / area;
    let tl = |ms: f64, f: f64| -> f64 {
        let doc = json!({
            "air": {"preset": "spec_reference"},
            "sweep": {"frequencies_Hz": [f]},
            "nodes": [{"id": "s", "domain": "acoustic"}, {"id": "t", "domain": "acoustic"}],
            "elements": [
                {"id": "src", "type": "pressure_source", "node": "s", "p_Pa": 2.0, "Zs_Pa_s_per_m3": zc},
                {"id": "panel", "type": "shell", "nodes": ["s", "t"], "area_cm2": 10,
                 "surface_density_kg_per_m2": ms},
                {"id": "load", "type": "acoustic_resistance", "node": "t", "R_Pa_s_per_m3": zc}
            ],
            "probes": [{"id": "pt", "quantity": "pressure", "node": "t"}]
        });
        let r = Circuit::from_json(&doc.to_string())
            .unwrap()
            .solve()
            .unwrap();
        let pt = r.probe("pt").unwrap().values[0];
        let x = 2.0 * PI * f * ms / (2.0 * air.rho_c());
        let want = C64::new(1.0, x).inv();
        assert!((pt - want).norm() <= 1e-12 * want.norm(), "{ms} {f}");
        -20.0 * pt.norm().log10()
    };
    let (ms, f) = (2.0, 3000.0);
    let x = 2.0 * PI * f * ms / (2.0 * air.rho_c());
    let step = 10.0 * ((1.0 + 4.0 * x * x) / (1.0 + x * x)).log10();
    let per_octave = tl(ms, 2.0 * f) - tl(ms, f);
    let per_mass = tl(2.0 * ms, f) - tl(ms, f);
    for d in [per_octave, per_mass] {
        assert!((d - step).abs() < 1e-9, "{d} vs {step}");
        assert!((d - 20.0 * 2f64.log10()).abs() < 0.002, "{d}");
    }
}

#[test]
fn shell_rejects_incomplete_or_ambiguous_input() {
    let bad = [
        json!({"area_cm2": 10}),
        json!({"area_cm2": 10, "mass_g": 2, "surface_density_kg_per_m2": 1}),
        json!({"area_cm2": 10, "mass_g": 2, "resonance_Hz": 100}),
        json!({"area_cm2": 10, "mass_g": 2, "resonance_Hz": 100, "Q": 3, "R_Pa_s_per_m3": 1}),
        json!({"area_cm2": 10, "mass_g": 2, "Q": 3}),
        json!({"area_cm2": 10, "mass_g": 2, "profile": "dome"}),
        json!({"mass_g": 2}),
        json!({"area_cm2": 10, "radius_mm": 5, "mass_g": 2}),
        json!({"area_cm2": 10, "thickness_mm": 1, "mass_g": 2}),
    ];
    for params in bad {
        let mut el = params.clone();
        el["id"] = json!("w");
        el["type"] = json!("shell");
        el["nodes"] = json!(["a"]);
        let doc = json!({
            "nodes": [{"id": "a", "domain": "acoustic"}],
            "elements": [{"id": "u", "type": "flow_source", "node": "a"}, el],
        });
        assert!(Circuit::from_json(&doc.to_string()).is_err(), "{params}");
    }
}

/// A sealed cup whose wall is a `shell` (the cup on its cushion: 20 g on
/// a 25 cm² face, resonance 80 Hz, Q 2) and a lumped (L0) slit leak, a
/// series impedance: the occluded pressure is the closed form of the two
/// paths in parallel into the cup compliance, p = Y/(Y + jωC) with Y the
/// sum of the paths' admittances, to 1e-9 (the slit's impedance is read
/// from the engine, whose model the first test checks against mpmath).
#[test]
fn shell_and_leak_paths_add_in_parallel() {
    let air = AirState::spec_reference();
    let freqs = [20.0, 80.0, 300.0, 1000.0, 3000.0];
    let mut doc = cup_with_slit(0, &freqs);
    doc["elements"].as_array_mut().unwrap().push(
        json!({"id": "cup_wall", "type": "shell", "nodes": ["a_cup", "ambient"],
                     "area_cm2": 25, "mass_g": 20, "resonance_Hz": 80, "Q": 2}),
    );
    doc["probes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "z_leak", "quantity": "impedance", "element": "leak"}));
    let p = parse(&doc);
    let iso = insertion_loss(&p, &Overrides::new(), &opts("p_cup", "a_cup")).unwrap();
    assert_eq!(iso.driven, vec!["leak".to_string(), "cup_wall".to_string()]);
    // Leak impedance with its far end grounded (a two-port: B/D).
    let mut solo = cup_with_slit(0, &freqs);
    solo["elements"][0] = json!({"id": "u", "type": "flow_source", "node": "a_cup"});
    solo["elements"][1]["nodes"] = json!(["a_cup", "gnd"]);
    solo["probes"] = json!([{"id": "z", "quantity": "impedance", "element": "u"}]);
    let zl = Circuit::from_json(&solo.to_string())
        .unwrap()
        .solve()
        .unwrap();
    let ma = 20e-3 / (25e-4 * 25e-4);
    let w0 = 2.0 * PI * 80.0;
    let cw = 1.0 / (w0 * w0 * ma);
    let rw = (ma / cw).sqrt() / 2.0;
    let c = 25e-6 / air.bulk_modulus();
    for (i, f) in freqs.iter().enumerate() {
        let jw = C64::new(0.0, 2.0 * PI * f);
        let yl = zl.probe("z").unwrap().values[i].inv();
        let yw = (rw + jw * ma + (jw * cw).inv()).inv();
        let want = (yl + yw) / (yl + yw + jw * c);
        let got = iso.p_occluded[i];
        assert!(
            (got - want).norm() <= 1e-9 * want.norm(),
            "{f}: {got} vs {want}"
        );
    }
    // Superposition: the two paths driven alone sum to the whole.
    for i in 0..freqs.len() {
        let sum: C64 = iso.paths.iter().map(|pr| pr.p[i]).sum();
        assert!((sum - iso.p_occluded[i]).norm() <= 1e-12 * iso.p_occluded[i].norm());
    }
}

/// The design template: the ear load is the macro owning the drum probe's
/// node (`ear` for `ear.drp`, entered at `a_ear`); the leak and the vent
/// are the driven paths; the ear's own transfer cancels in the insertion
/// loss, IL = −20·log10|p_entrance| exactly (the ear is a one-port at its
/// entrance), for both ear loads; the paths sum to the occluded pressure.
#[test]
fn design_template_isolation() {
    let text = example("design_over_ear.json");
    let p = Parametric::parse(&text).unwrap();
    for ear in ["iec60318_4", "type43"] {
        let mut ov = Overrides::new();
        ov.insert("ear".into(), acoustilab::expr::PValue::Str(ear.into()));
        let iso = insertion_loss(&p, &ov, &IsolationOptions::default()).unwrap();
        assert_eq!(iso.ear.drum_probe, "p_drp");
        assert_eq!(iso.ear.entrance_node, "a_ear");
        assert_eq!(iso.ear.elements, vec!["ear".to_string()]);
        assert_eq!(iso.driven, vec!["leak".to_string(), "vent".to_string()]);
        assert!(iso.warnings.is_empty(), "{:?}", iso.warnings);
        // Entrance pressure of the occluded ear, from the same transform.
        let r = p.resolve(&ov).unwrap();
        let (mut doc, _, _) = outside_document(&r.doc, None, None, 1.0, true).unwrap();
        doc["probes"] = json!([{"id": "p_ent", "quantity": "pressure", "node": "a_ear"}]);
        let c = Circuit::from_json(&doc.to_string()).unwrap();
        let res = c.solve().unwrap();
        let pe = &res.probe("p_ent").unwrap().values;
        for (i, il) in iso.insertion_loss_db.iter().enumerate() {
            let want = -20.0 * pe[i].norm().log10();
            assert!(
                (il - want).abs() < 1e-9,
                "{ear} {} Hz: {il} vs {want}",
                iso.freqs_hz[i]
            );
            let sum: C64 = iso.paths.iter().map(|pr| pr.p[i]).sum();
            assert!((sum - iso.p_occluded[i]).norm() <= 1e-9 * iso.p_occluded[i].norm());
        }
        // Leak-limited at low frequency, tens of dB in the midrange.
        assert!(iso.insertion_loss_db[0] < 2.0);
        let s = iso.summary.as_ref().unwrap();
        assert!(s.max_db > 30.0);
    }
}

/// Energy balance of the occluded circuit: the power the outside source
/// delivers is absorbed by the network (Tellegen's theorem over every
/// element port), to 1e-9 of the source power.
#[test]
fn occluded_energy_balance() {
    let p = Parametric::parse(&example("design_over_ear.json")).unwrap();
    let c = occluded_circuit(&p, &Overrides::new(), &IsolationOptions::default()).unwrap();
    for &f in c.freqs.iter().step_by(17) {
        let x = c.solve_at(f).unwrap();
        let pw = c.power_absorbed(f, &x);
        let src = pw
            .iter()
            .find(|(id, _)| id == "iso_ambient")
            .and_then(|(_, v)| *v)
            .unwrap();
        let total: f64 = pw.iter().filter_map(|(_, v)| *v).sum();
        assert!(total.abs() <= 1e-9 * src.abs(), "{f} Hz: {total} of {src}");
        assert!(src < 0.0, "the source delivers power");
    }
}

/// The transformation's conventions: sources zeroed with their impedance
/// kept, the drive key removed, only the drum probe kept, and warnings for
/// paths that end at the reference.
#[test]
fn transformation_and_warnings() {
    let text = example("design_over_ear.json");
    let p = Parametric::parse(&text).unwrap();
    let r = p.resolve(&Overrides::new()).unwrap();
    let (doc, node, src) = outside_document(&r.doc, Some("p_drp"), None, 1.0, true).unwrap();
    assert!(doc.get("drive").is_none());
    let els = doc["elements"].as_array().unwrap();
    let amp = els.iter().find(|e| e["id"] == "amp").unwrap();
    assert_eq!(amp["V_V"], json!(0.0));
    assert!(amp.get("Zs_ohm").is_some());
    let leak = els.iter().find(|e| e["id"] == "leak").unwrap();
    assert_eq!(leak["nodes"][1], json!(node));
    assert!(els
        .iter()
        .any(|e| e["id"] == json!(src) && e["type"] == "pressure_source"));
    assert_eq!(doc["probes"].as_array().unwrap().len(), 1);
    // A vent that ends at 'gnd' is not driven: warned.
    let mut v = serde_json::from_str::<Value>(&text).unwrap();
    for e in v["elements"].as_array_mut().unwrap() {
        if e["id"] == "vent" {
            e["nodes"] = json!(["a_rear", "gnd"]);
        }
    }
    let iso = insertion_loss(&parse(&v), &Overrides::new(), &IsolationOptions::default()).unwrap();
    assert_eq!(iso.driven, vec!["leak".to_string()]);
    assert!(iso
        .warnings
        .iter()
        .any(|w| w.code == "undriven_path" && w.element.as_deref() == Some("vent")));
    // A separate piston whose rear face is 'gnd' is not driven either, and
    // an air compliance written "to ambient" (as conventions.md phrases a
    // cavity) would be driven with the outside pressure: both warned.
    let doc = json!({
        "sweep": {"frequencies_Hz": [100.0, 1000.0]},
        "ui": {"primary_probe": "p_cup"},
        "nodes": [{"id": "m", "domain": "mechanical"}, {"id": "a_cup", "domain": "acoustic"}],
        "elements": [
            {"id": "susp", "type": "suspension", "node": "m", "Mms_g": 0.3, "Cms_mm_per_N": 1, "Rms_Ns_per_m": 0.1},
            {"id": "dia", "type": "piston", "nodes": ["m", "gnd", "a_cup", "gnd"], "Sd_cm2": 10},
            {"id": "air", "type": "acoustic_compliance", "nodes": ["a_cup", "ambient"], "C_m3_per_Pa": 1e-10}
        ],
        "probes": [{"id": "p_cup", "quantity": "pressure", "node": "a_cup"}]
    });
    let w = acoustilab::isolation::undriven_paths(&doc);
    assert!(w
        .iter()
        .any(|w| w.code == "undriven_path" && w.element.as_deref() == Some("dia")));
    assert!(w
        .iter()
        .any(|w| w.code == "compliance_to_ambient" && w.element.as_deref() == Some("air")));
    let front = json!(["m", "gnd", "a_cup", "ambient"]);
    let mut open_rear = doc.clone();
    open_rear["elements"][1]["nodes"] = front;
    open_rear["elements"][2]["nodes"] = json!(["a_cup"]);
    assert!(acoustilab::isolation::undriven_paths(&open_rear).is_empty());
}

/// With two or more paths the paths are also added in power (independent
/// phases at the openings, the diffuse-field limit for openings far apart):
/// that IL can only be finite where the coherent sum cancels, and it is
/// the coherent IL itself when one path dominates.
#[test]
fn incoherent_insertion_loss() {
    let text = example("design_over_ear.json");
    let p = Parametric::parse(&text).unwrap();
    let iso = insertion_loss(&p, &Overrides::new(), &IsolationOptions::default()).unwrap();
    let inc = iso.insertion_loss_incoherent_db.as_ref().unwrap();
    for (i, f) in iso.freqs_hz.iter().enumerate() {
        let open = iso.p_open[i].norm_sqr();
        let power: f64 = iso.paths.iter().map(|p| p.p[i].norm_sqr()).sum();
        let want = 10.0 * (open / power).log10();
        assert!((inc[i] - want).abs() < 1e-9, "{f} Hz");
        // |Σp|² ≤ n·Σ|p|² (Cauchy–Schwarz): the coherent IL is at least
        // the incoherent one less 10·log10(n).
        let n = iso.paths.len() as f64;
        assert!(iso.insertion_loss_db[i] >= inc[i] - 10.0 * n.log10() - 1e-9);
    }
    // In the template the leak and vent contributions cancel near 1.16 kHz
    // when driven in phase: 40.5 dB coherent against 28.3 dB in power.
    let k = iso
        .freqs_hz
        .iter()
        .position(|f| (*f / 1156.0 - 1.0).abs() < 0.01)
        .unwrap();
    assert!(
        iso.insertion_loss_db[k] > inc[k] + 10.0,
        "{} vs {}",
        iso.insertion_loss_db[k],
        inc[k]
    );
}

/// Ear identification: a `canal` + `eardrum` ear on user nodes is entered
/// at the canal's first node; everything beyond a named entrance belongs
/// to the ear side (here the pad as well); an entrance that a path
/// bypasses does not separate the ear from the headphone and is refused.
#[test]
fn ear_identification() {
    let doc = json!({
        "sweep": {"frequencies_Hz": [100.0, 1000.0]},
        "ui": {"primary_probe": "p_drum"},
        "nodes": [{"id": "a_cup", "domain": "acoustic"}, {"id": "a_ent", "domain": "acoustic"},
                  {"id": "drum", "domain": "acoustic"}],
        "elements": [
            {"id": "src", "type": "flow_source", "node": "a_cup"},
            {"id": "cup", "type": "cavity", "node": "a_cup", "volume_cm3": 20},
            {"id": "pad", "type": "acoustic_resistance", "nodes": ["a_cup", "a_ent"], "R_Pa_s_per_m3": 1e3},
            {"id": "leak", "type": "slit", "nodes": ["a_cup", "ambient"], "gap_mm": 0.1, "width_mm": 20, "length_mm": 5},
            {"id": "canal", "type": "canal", "nodes": ["a_ent", "drum"], "length_mm": 25, "area_mm2": 44},
            {"id": "td", "type": "eardrum", "node": "drum"}
        ],
        "probes": [{"id": "p_drum", "quantity": "pressure", "node": "drum"}]
    });
    let ear = find_ear(&doc, None, None).unwrap();
    assert_eq!(ear.entrance_node, "a_ent");
    assert_eq!(ear.elements, vec!["canal".to_string(), "td".to_string()]);
    let iso = insertion_loss(
        &parse(&doc),
        &Overrides::new(),
        &IsolationOptions::default(),
    )
    .unwrap();
    // The flow source is zeroed: the ear is driven only through the leak.
    assert!(iso
        .insertion_loss_db
        .iter()
        .all(|x| x.is_finite() && *x > 0.0));
    let wide = find_ear(&doc, None, Some("a_cup")).unwrap();
    assert_eq!(
        wide.elements,
        vec!["canal".to_string(), "td".to_string(), "pad".to_string()]
    );
    let mut bypassed = doc.clone();
    bypassed["elements"].as_array_mut().unwrap().push(
        json!({"id": "bypass", "type": "acoustic_resistance", "nodes": ["a_cup", "drum"], "R_Pa_s_per_m3": 1e9}),
    );
    let err = find_ear(&bypassed, None, None).unwrap_err().to_string();
    assert!(err.contains("does not separate"), "{err}");
}

/// 1/3-octave bands (ETSI TS 103 640 clause 5.1.3): base-ten mid-band
/// frequencies with the nominal R10 labels, bands kept only inside the
/// sweep, and a constant pressure ratio giving exactly its loss.
#[test]
fn third_octave_band_loss() {
    let f = acoustilab::grid::log_grid(10.0, 20_000.0, 48.0);
    let open: Vec<C64> = f.iter().map(|&f| C64::new(1.0 + f / 1e4, 0.2)).collect();
    let occ: Vec<C64> = open.iter().map(|p| p * 0.01).collect();
    let bands = third_octave_bands(&f, &open, &occ);
    let nominal: Vec<f64> = bands.iter().map(|b| b.nominal_hz).collect();
    assert_eq!(nominal[0], 20.0);
    assert_eq!(nominal[5], 63.0);
    assert!(nominal.contains(&31.5) && nominal.contains(&3150.0));
    assert_eq!(*nominal.last().unwrap(), 16_000.0);
    for b in &bands {
        assert!((b.insertion_loss_db - 40.0).abs() < 1e-9);
        assert!((b.center_hz / b.nominal_hz - 1.0).abs() < 0.03);
    }
}

/// Bleed as the monopole of spec Appendix C9: a 30 cm³ sealed front at
/// 94 dB SPL moves U = ωC·p; radiated from the rear opening, it gives
/// ρf|U|/(2r) = 32.1 dB at 1 m at 1 kHz (C9 quotes 32 dB). The estimate
/// follows the netlist's drive: 4 mW is 6.02 dB above 1 mW.
#[test]
fn bleed_monopole() {
    let air = AirState::spec_reference();
    let p94 = 20e-6 * 10f64.powf(94.0 / 20.0);
    let u = 2.0 * PI * 1000.0 * 30e-6 / air.bulk_modulus() * p94;
    let doc = json!({
        "air": {"preset": "spec_reference"},
        "sweep": {"frequencies_Hz": [1000.0]},
        "nodes": [{"id": "a_rear", "domain": "acoustic"}],
        "elements": [
            {"id": "rear", "type": "flow_source", "node": "a_rear", "U_m3_per_s": u},
            {"id": "grille", "type": "radiation", "nodes": ["a_rear", "ambient"], "area_cm2": 10}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "a_rear"}]
    });
    let b = bleed(&parse(&doc), &Overrides::new(), &[1.0]).unwrap();
    assert!((b.u_out[0] - C64::new(u, 0.0)).norm() < 1e-12 * u);
    let want = 20.0 * (air.rho * 1000.0 * u / 2.0 / 20e-6).log10();
    assert!((b.spl_db[0][0] - want).abs() < 1e-9);
    assert!((b.spl_db[0][0] - 32.07).abs() < 0.01, "{}", b.spl_db[0][0]);
    let p = Parametric::parse(&example("design_over_ear.json")).unwrap();
    let b1 = bleed(&p, &Overrides::new(), &[0.3]).unwrap();
    let mut ov = Overrides::new();
    ov.insert("drive_mW".into(), acoustilab::expr::PValue::Num(4.0));
    let b4 = bleed(&p, &ov, &[0.3]).unwrap();
    for (x, y) in b1.spl_db[0].iter().zip(&b4.spl_db[0]) {
        assert!((y - x - 20.0 * 2f64.log10()).abs() < 1e-9);
    }
}
