//! Drive conventions and sensitivity readouts (spec Section 4; erratum E32).
//!
//! The drive functions scale a linear solve, so most checks are exact
//! identities (tolerance 1e-9 dB, i.e. rounding); interpolation between grid
//! points is checked against exact re-solves at 0.01 dB.

use acoustilab::drive::{self, Drive, RewindBasis};
use acoustilab::{Circuit, SolveResult, C64};
use serde_json::{json, Value};

const TYMPHANY: &str = "tymphany_hpd_40n16pet00_32";

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

/// A driver in a sealed front cup with a pad leak, 1 V source.
fn cup(driver: Value, v: f64, zs: f64) -> Circuit {
    let mut d = json!({"id": "drv", "type": "driver", "nodes": ["e1", "gnd", "af", "ambient"]});
    for (k, x) in driver.as_object().unwrap() {
        d[k] = x.clone();
    }
    let doc = json!({
        "schema": "acoustilab-netlist/0.1",
        "air": {"preset": "spec_reference"},
        "sweep": {"f_min_Hz": 10, "f_max_Hz": 20000, "points_per_octave": 48},
        "nodes": [{"id": "e1", "domain": "electrical"}, {"id": "af", "domain": "acoustic"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e1", "V_V": v, "Zs_ohm": zs},
            d,
            {"id": "front", "type": "cavity", "node": "af", "volume_cm3": 30},
            {"id": "leak", "type": "slit", "nodes": ["af", "ambient"], "gap_mm": 0.1, "width_mm": 20, "length_mm": 8}
        ],
        "probes": [
            {"id": "p", "quantity": "pressure", "node": "af"},
            {"id": "i", "quantity": "current", "element": "amp"},
            {"id": "zin", "quantity": "impedance", "element": "drv", "port": 0},
            {"id": "v", "quantity": "velocity", "node": "drv.m"}
        ]
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

fn tymphany(v: f64, zs: f64) -> Circuit {
    cup(json!({"record": TYMPHANY}), v, zs)
}

fn level_at(c: &Circuit, probe: &str, f: f64) -> f64 {
    drive::spl_db(drive::probe_at(c, probe, f).unwrap())
}

#[test]
fn characteristic_voltage_gives_94_db_at_500_hz() {
    // IEC 60268-7 via linear scaling of a 1 V solve; re-solving at that
    // voltage must give 94 dB exactly.
    for zs in [0.0, 120.0] {
        let c = tymphany(1.0, zs);
        let v = drive::characteristic_voltage(&c, "p", 94.0, 500.0).unwrap();
        let again = tymphany(v, zs);
        assert!((level_at(&again, "p", 500.0) - 94.0).abs() < 1e-9);
        assert!(v > 0.001 && v < 1.0, "{v} V");
    }
    // Through the source impedance: the ratio is the divider |Z + Zs|/|Z|.
    let c0 = tymphany(1.0, 0.0);
    let c120 = tymphany(1.0, 120.0);
    let v0 = drive::characteristic_voltage(&c0, "p", 94.0, 500.0).unwrap();
    let v120 = drive::characteristic_voltage(&c120, "p", 94.0, 500.0).unwrap();
    let z = drive::probe_at(&c0, "zin", 500.0).unwrap();
    assert!((v120 / v0 - (z + 120.0).norm() / z.norm()).abs() < 1e-12);
    // From a swept result (500 Hz falls between grid points) the
    // interpolated value is within 0.01 dB of the exact one.
    let r = c0.solve().unwrap();
    assert!(!r.freqs_hz.contains(&500.0));
    let vi = drive::characteristic_voltage_from(&r, 1.0, "p", 94.0, 500.0).unwrap();
    assert!(db(vi / v0).abs() < 0.01, "{} dB", db(vi / v0));
    // Scaling the result to the characteristic drive puts 500 Hz at 94 dB.
    let scaled = drive::drive_result(&Drive::characteristic("p"), &r, 1.0).unwrap();
    let p500 = drive::interpolate(&scaled, "p", 500.0).unwrap();
    assert!((drive::spl_db(p500) - 94.0).abs() < 1e-9);
}

#[test]
fn one_milliwatt_into_the_rated_impedance() {
    // Rated 32 Ω (not Re = 32.8 Ω): V = sqrt(1 mW · 32 Ω) = 0.1789 V.
    let rated = driver_rated();
    let d = Drive::one_milliwatt(rated);
    assert!((drive::voltage_for_power(1e-3, rated) - 0.178_885_438_199_983_2).abs() < 1e-15);
    assert!(d.label().contains("1 mW"), "{}", d.label());
    let c = tymphany(1.0, 0.0);
    let r = c.solve().unwrap();
    let s = drive::drive_result(&d, &r, 1.0).unwrap();
    let (p1, pm) = (r.probe("p").unwrap(), s.probe("p").unwrap());
    let (z1, zm) = (r.probe("zin").unwrap(), s.probe("zin").unwrap());
    for k in 0..r.freqs_hz.len() {
        assert!(
            (pm.values[k] - p1.values[k] * 0.178_885_438_199_983_2).norm()
                < 1e-15 * p1.values[k].norm().max(1.0)
        );
        assert_eq!(zm.values[k], z1.values[k], "impedances do not scale");
    }
    // A netlist solved at 1 V versus one solved at the 1 mW voltage.
    let direct = tymphany(0.178_885_438_199_983_2, 0.0);
    let pm_direct = drive::probe_at(&direct, "p", 1000.0).unwrap();
    let pm_scaled = drive::probe_at(&c, "p", 1000.0).unwrap() * 0.178_885_438_199_983_2;
    assert!((pm_direct - pm_scaled).norm() < 1e-12 * pm_direct.norm());
}

fn driver_rated() -> f64 {
    acoustilab::elements::driver::record(TYMPHANY)
        .unwrap()
        .datasheet
        .rated_impedance
        .unwrap()
}

#[test]
fn sensitivity_converts_with_the_rated_impedance() {
    // Erratum E32: dB/V = dB/mW + 10·log10(1000/Z_rated), with the rated
    // 32 Ω (14.95 dB), not Re = 32.8 Ω (14.84 dB).
    let rated = driver_rated();
    let c = tymphany(1.0, 0.0);
    let s = drive::sensitivity(&c, "p", rated).unwrap();
    assert_eq!([s[0].f_hz, s[1].f_hz], drive::SENSITIVITY_FREQUENCIES_HZ);
    let mw = tymphany(drive::voltage_for_power(1e-3, rated), 0.0);
    for x in &s {
        assert!((x.db_per_v - level_at(&c, "p", x.f_hz)).abs() < 1e-9);
        assert!((x.db_per_mw - level_at(&mw, "p", x.f_hz)).abs() < 1e-9);
        assert!((x.conversion_db - 14.948_500_216_800_94).abs() < 1e-12);
        assert!((x.db_per_v - x.db_per_mw - x.conversion_db).abs() < 1e-12);
        // Using Re would be 0.107 dB off.
        assert!((drive::conversion_db(32.8) - x.conversion_db + 0.107).abs() < 1e-3);
    }
    // Readouts from a result at 1 V agree with the exact ones.
    let r = c.solve().unwrap();
    let si = drive::sensitivity_from(&r, 1.0, "p", rated).unwrap();
    for (a, b) in s.iter().zip(&si) {
        assert!((a.db_per_v - b.db_per_v).abs() < 0.01);
        assert!((a.db_per_mw - b.db_per_mw).abs() < 0.01);
    }
}

#[test]
fn constant_current_drive_holds_the_current() {
    let c = tymphany(1.0, 10.0);
    let r = c.solve().unwrap();
    let d = Drive::Current {
        amps: 0.01,
        probe: "i".into(),
    };
    let s = drive::drive_result(&d, &r, 1.0).unwrap();
    for v in &s.probe("i").unwrap().values {
        assert!((v - C64::new(0.01, 0.0)).norm() < 1e-15);
    }
}

/// D0 driver with Re = 32 Ω and a 52 Ω peak (Bl²/Rms = 20 Ω) in free air.
fn free_air(zs: f64) -> Circuit {
    let doc = json!({
        "air": {"preset": "spec_reference"},
        "nodes": [{"id": "e1", "domain": "electrical"}],
        "elements": [
            {"id": "amp", "type": "vsource", "node": "e1", "V_V": 1.0, "Zs_ohm": zs},
            {"id": "drv", "type": "driver", "nodes": ["e1", "gnd", "ambient", "ambient"], "model": "D0",
             "Re_ohm": 32, "Bl_Tm": 1.0, "Mms_g": 0.3, "Cms_mm_per_N": 1.0, "Rms_Ns_per_m": 0.05, "Sd_cm2": 10}
        ],
        "probes": [
            {"id": "zin", "quantity": "impedance", "element": "drv", "port": 0},
            {"id": "v", "quantity": "velocity", "node": "drv.m"}
        ]
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

#[test]
fn source_impedance_colours_the_response() {
    // Spec Section 4: 20·log10(|Z|/|Z + Zs|). A 120 Ω source on a 32 Ω
    // driver with a 52 Ω peak lifts the peak by about 3 dB relative to the
    // Re-dominated treble; 10 Ω by under 1 dB.
    let fs = 1.0 / (2.0 * std::f64::consts::PI * (0.3e-3f64 * 1e-3).sqrt());
    let c0 = free_air(0.0);
    for (zs, expect) in [(120.0, 3.143), (10.0, 0.835)] {
        let c = free_air(zs);
        let lift = |f: f64| {
            db(
                (drive::probe_at(&c, "v", f).unwrap() / drive::probe_at(&c0, "v", f).unwrap())
                    .norm(),
            )
        };
        for f in [20.0, fs, 1000.0, 20000.0] {
            let z = drive::probe_at(&c0, "zin", f).unwrap();
            let closed = drive::source_colouring_db(z, C64::new(zs, 0.0));
            assert!((lift(f) - closed).abs() < 1e-9, "{f} Hz");
        }
        let relative = lift(fs) - lift(20000.0);
        assert!((relative - expect).abs() < 0.01, "{zs} ohm: {relative} dB");
    }
    // The 52 Ω/32 Ω endpoints of the spec's example.
    let peak = drive::probe_at(&c0, "zin", fs).unwrap();
    assert!((peak.norm() - 52.0).abs() < 1e-9);
}

/// Rewound coil at constant moving mass: Re, Le, L2, R2 ∝ turns², Bl ∝ turns.
fn rewound(re: f64) -> Circuit {
    let n2 = re / 32.0;
    cup(
        json!({"model": "D1", "Re_ohm": re, "Bl_Tm": 1.0 * n2.sqrt(), "Mms_g": 0.3,
               "Cms_mm_per_N": 1.0, "Rms_Ns_per_m": 0.05, "Sd_cm2": 10,
               "Le_uH": 20.0 * n2, "L2_uH": 60.0 * n2, "R2_ohm": 15.0 * n2}),
        1.0,
        0.0,
    )
}

#[test]
fn rewinding_32_to_300_ohm_costs_9_72_db_at_constant_voltage_only() {
    // Spec p. 15 and Section 17: −9.72 dB at constant voltage, 0 at constant
    // power (into the rated impedance, which scales with Re), +9.72 dB at
    // constant current. Exact when every coil impedance scales with Re.
    let (a, b) = (rewound(32.0), rewound(300.0));
    let (ra, rb) = (a.solve().unwrap(), b.solve().unwrap());
    let level = |r: &SolveResult, d: &Drive| -> Vec<f64> {
        drive::drive_result(d, r, 1.0)
            .unwrap()
            .probe("p")
            .unwrap()
            .spl_db()
    };
    for (basis, da, dbb) in [
        (RewindBasis::Voltage, Drive::one_volt(), Drive::one_volt()),
        (
            RewindBasis::Power,
            Drive::one_milliwatt(32.0),
            Drive::one_milliwatt(300.0),
        ),
        (
            RewindBasis::Current,
            Drive::Current {
                amps: 0.01,
                probe: "i".into(),
            },
            Drive::Current {
                amps: 0.01,
                probe: "i".into(),
            },
        ),
    ] {
        let expect = drive::rewind_level_change_db(32.0, 300.0, basis);
        let (la, lb) = (level(&ra, &da), level(&rb, &dbb));
        for k in 0..la.len() {
            assert!(
                (lb[k] - la[k] - expect).abs() < 1e-9,
                "{basis:?} at {} Hz",
                ra.freqs_hz[k]
            );
        }
        let spec = match basis {
            RewindBasis::Voltage => -9.72,
            RewindBasis::Power => 0.0,
            RewindBasis::Current => 9.72,
        };
        assert!((expect - spec).abs() < 0.005, "{basis:?}: {expect}");
    }
}

#[test]
fn rated_impedance_check_flags_low_impedance() {
    let c = tymphany(1.0, 0.0);
    let r = c.solve().unwrap();
    let z = &r.probe("zin").unwrap().values;
    // Rated 32 Ω: the minimum is Re = 32.8 Ω > 25.6 Ω.
    assert!(drive::rated_impedance_violations(&r.freqs_hz, z, 32.0).is_empty());
    // A 50 Ω rating is violated wherever |Z| < 40 Ω.
    let v = drive::rated_impedance_violations(&r.freqs_hz, z, 50.0);
    assert!(!v.is_empty());
    for f in &v {
        let k = r.freqs_hz.iter().position(|x| x == f).unwrap();
        assert!(z[k].norm() < 40.0);
    }
}

#[test]
fn drive_scaling_needs_a_single_voltage_source() {
    let doc = json!({
        "nodes": [{"id": "e1", "domain": "electrical"}],
        "elements": [
            {"id": "a", "type": "vsource", "node": "e1", "V_V": 1.0, "Zs_ohm": 10},
            {"id": "b", "type": "isource", "node": "e1", "I_A": 0.01},
            {"id": "r", "type": "resistor", "node": "e1", "R_ohm": 32}
        ]
    });
    let c = Circuit::from_json(&doc.to_string()).unwrap();
    assert!(drive::source_voltage(&c).is_err());
    let c = tymphany(0.5, 0.0);
    assert_eq!(drive::source_voltage(&c).unwrap(), 0.5);
}

#[test]
fn interpolation_is_exact_on_the_grid() {
    let c = tymphany(1.0, 0.0);
    let r = c.solve().unwrap();
    let p = r.probe("p").unwrap();
    for k in [0, 100, r.freqs_hz.len() - 1] {
        assert_eq!(
            drive::interpolate(&r, "p", r.freqs_hz[k]).unwrap(),
            p.values[k]
        );
    }
    assert!(drive::interpolate(&r, "p", 5.0).is_err());
    assert!(drive::interpolate(&r, "nope", 100.0).is_err());
    // Between grid points: within 0.01 dB and 0.1° of an exact solve.
    for f in [33.3, 777.0, 4321.0] {
        let a = drive::interpolate(&r, "p", f).unwrap();
        let b = drive::probe_at(&c, "p", f).unwrap();
        assert!(db((a / b).norm()).abs() < 0.01, "{f} Hz");
        assert!((a / b).arg().to_degrees().abs() < 0.1, "{f} Hz");
    }
}
