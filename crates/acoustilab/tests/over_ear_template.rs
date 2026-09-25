//! The over-ear design template against the closed forms and the published
//! behaviour of closed over-ear headphones (docs/over-ear-template.md).
//!
//! * Sealed configuration (`baffle_vent_count = 0`): the air springs of the
//!   front and rear volumes raise the driver's resonance to the coupled
//!   resonance of spec Appendix C2, the "peak between 500 Hz and 1 KHz" of
//!   the sealed headphone in US 4,239,945.
//! * Vented default: the fixture resonance stays in the band published for
//!   closed over-ears (25 to 95 Hz, peak 1.08 to 1.84 times the mid-band
//!   impedance), the diaphragm is resistance-controlled in the bass, and the
//!   drum response is flat to the baffle vents' Helmholtz frequency.

use acoustilab::expr::PValue;
use acoustilab::params::{Overrides, Parametric};
use acoustilab::{Circuit, SolveResult, C64};
use std::f64::consts::PI;

const TEMPLATE: &str = include_str!("../../../examples/design_over_ear.json");

/// IEC 60318-4 nominal effective volume at 500 Hz, cm³ (docs/ear-loads.md).
const V_EAR_CM3: f64 = 1.26;

fn circuit(pairs: &[(&str, f64)]) -> Circuit {
    let p = Parametric::parse(TEMPLATE).unwrap();
    let ov: Overrides = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), PValue::Num(*v)))
        .collect();
    Circuit::from_parametric(&p, &ov).unwrap()
}

fn sealed() -> Circuit {
    circuit(&[("baffle_vent_count", 0.0)])
}

/// A probe's value at one frequency.
fn at(c: &Circuit, probe: &str, f: f64) -> C64 {
    let p = c.probes.iter().find(|p| p.id == probe).unwrap();
    let x = c.solve_at(f).unwrap();
    c.probe_value(p, f, &x).unwrap()
}

/// `n` log-spaced frequencies from `f1` to `f2`.
fn logspace(f1: f64, f2: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|k| f1 * (f2 / f1).powf(k as f64 / (n - 1) as f64))
        .collect()
}

/// Golden-section search for the maximum of |zin| in [a, b].
fn impedance_peak(c: &Circuit, mut a: f64, mut b: f64) -> (f64, f64) {
    let g = (5f64.sqrt() - 1.0) / 2.0;
    let z = |f: f64| at(c, "zin", f).norm();
    while b - a > 1e-4 * b {
        let (x1, x2) = (b - g * (b - a), a + g * (b - a));
        if z(x1) > z(x2) {
            b = x2;
        } else {
            a = x1;
        }
    }
    let f = 0.5 * (a + b);
    (f, z(f))
}

fn param(r: &SolveResult, name: &str) -> f64 {
    r.meta.parameters[name].as_f64().unwrap()
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

/// Drum pressure relative to its value at 500 Hz, dB.
fn drum_re_500(c: &Circuit, f: f64) -> f64 {
    db(at(c, "p_drp", f).norm() / at(c, "p_drp", 500.0).norm())
}

/// With no baffle vents the netlist is the sealed cup: the impedance peak
/// sits at f_c = (1/2π)·sqrt((1/Cms + γP0·Sd²·(1/V_f + 1/V_r))/Mms), with
/// V_f the front cavity plus the ear simulator's effective volume
/// (Appendix C2): 936 Hz by hand, 930 Hz solved. The 2 % tolerance covers
/// the ear's effective volume, which falls from 1.26 to about 1.1 cm³ by
/// 1 kHz (0.4 %), wall loss and the resistive leaks. The drum response
/// peaks there, well above its 500 Hz level.
#[test]
fn sealed_cup_resonates_where_the_air_springs_put_it() {
    let c = sealed();
    let r = c.solve().unwrap();
    assert!(r.probe("u_baffle").is_none());
    assert!(!r
        .meta
        .elements
        .iter()
        .any(|e| e["id"].as_str() == Some("baffle_vent")));

    let air = &r.meta.air;
    let gp0 = air.gamma * air.p0;
    let mms = param(&r, "driver_Mms_g") * 1e-3;
    let sd = param(&r, "driver_Sd_cm2") * 1e-4;
    let ws = 2.0 * PI * param(&r, "driver_fs_Hz");
    let v_f = (param(&r, "front_volume_cm3") + V_EAR_CM3) * 1e-6;
    let v_r = param(&r, "rear_volume_cm3") * 1e-6;
    let k = ws * ws * mms + gp0 * sd * sd * (1.0 / v_f + 1.0 / v_r);
    let f_c = (k / mms).sqrt() / (2.0 * PI);
    assert!((f_c - 936.0).abs() < 2.0, "{f_c}");

    let (f_pk, z_pk) = impedance_peak(&c, 500.0, 2000.0);
    assert!((f_pk / f_c - 1.0).abs() < 0.02, "{f_pk} vs {f_c}");
    let re = param(&r, "driver_Re_ohm");
    assert!(z_pk > 1.4 * re, "{z_pk}");

    // The sealed headphone's peak between 500 Hz and 1 kHz, then the fall.
    let (f_drum, level) = logspace(300.0, 3000.0, 241)
        .into_iter()
        .map(|f| (f, drum_re_500(&c, f)))
        .fold((0.0, f64::MIN), |m, x| if x.1 > m.1 { x } else { m });
    assert!((500.0..1000.0).contains(&f_drum), "{f_drum}");
    assert!(level > 10.0, "{level}");
    assert!(drum_re_500(&c, 4000.0) < -20.0);
}

/// The vented default on the fixture: the impedance maximum stays in the
/// published band of closed over-ears (25 to 95 Hz) at the heavily damped
/// end of the published heights (1.08 to 1.84 times the mid-band
/// impedance), and nothing resonates between 300 Hz and 3 kHz: the
/// impedance only falls there. The motional impedance Zin − Re is nearly
/// real from 50 to 300 Hz: the diaphragm is resistance-controlled by the
/// baffle vents' mesh.
#[test]
fn vented_default_keeps_the_fixture_resonance_of_closed_over_ears() {
    let c = circuit(&[]);
    let r = c.solve().unwrap();
    assert!(r.probe("u_baffle").is_some());
    let re = param(&r, "driver_Re_ohm");

    let z: Vec<(f64, f64)> = logspace(20.0, 3000.0, 289)
        .into_iter()
        .map(|f| (f, at(&c, "zin", f).norm()))
        .collect();
    let (f_max, z_max) = z
        .iter()
        .copied()
        .fold((0.0, 0.0), |m, x| if x.1 > m.1 { x } else { m });
    assert!((25.0..95.0).contains(&f_max), "{f_max}");
    let z_mid = at(&c, "zin", 1000.0).norm();
    assert!(
        (1.03..1.84).contains(&(z_max / z_mid)) && z_max < 1.2 * re,
        "{z_max} {z_mid}"
    );
    let above: Vec<&(f64, f64)> = z.iter().filter(|(f, _)| *f >= 300.0).collect();
    for w in above.windows(2) {
        assert!(w[1].1 <= w[0].1 + 1e-9, "rises at {} Hz", w[1].0);
    }

    for f in [50.0, 100.0, 200.0, 300.0] {
        let zm = at(&c, "zin", f) - C64::new(re, 0.0);
        let phase = zm.im.atan2(zm.re).to_degrees();
        assert!(phase.abs() < 25.0, "{f} Hz: {phase}°");
    }
}

/// Resistive coupling (Görike, US 4,389,542; Shaw and Thiessen 1962): with
/// the cup closed to the outside, the diaphragm drives air round the loop
/// front cavity → baffle mesh → rear cavity, the total volume does not
/// change, and the front takes the share C_r/(C_f + C_r) of the pressure
/// drop R_b·U across the mesh. With U = Sd·F/(Sd²·R_b + Rms) the front
/// pressure is p = (F/Sd)·(C_r/(C_f + C_r))·Sd²R_b/(Sd²R_b + Rms), with no
/// frequency in it: 107.4 dB against 107.7 solved at 100 Hz (the
/// suspension, the cavities' reactance and the leaks make the rest). The
/// flat band ends at the Helmholtz frequency of the vents' air mass with
/// the two volumes in series (about 1.44 kHz): the drum response stays
/// within 1.5 dB of its 500 Hz level from 50 Hz to 1.5 kHz. Above it the
/// fall is the mass-controlled one of any lumped cup, but it starts later
/// than in the sealed cup, so the 2 to 8 kHz band gains more than 5 dB
/// relative to 500 Hz.
#[test]
fn vented_default_is_flat_to_the_baffle_helmholtz_frequency() {
    let c = circuit(&[]);
    let r = c.solve().unwrap();
    let air = &r.meta.air;
    let gp0 = air.gamma * air.p0;
    let re = param(&r, "driver_Re_ohm");
    let mms = param(&r, "driver_Mms_g") * 1e-3;
    let sd = param(&r, "driver_Sd_cm2") * 1e-4;
    let ws = 2.0 * PI * param(&r, "driver_fs_Hz");
    let bl = (ws * mms * re / param(&r, "driver_Qes")).sqrt();
    let rms = ws * mms / param(&r, "driver_Qms");
    let c_f = (param(&r, "front_volume_cm3") + V_EAR_CM3) * 1e-6 / gp0;
    let c_r = param(&r, "rear_volume_cm3") * 1e-6 / gp0;
    let a = param(&r, "baffle_vent_count")
        * PI
        * (0.5e-3 * param(&r, "baffle_vent_diameter_mm")).powi(2);
    let x = sd * sd * param(&r, "baffle_vent_mesh_rayl") / a;

    // `solve_at` solves the netlist's own 1 V source; the drive key only
    // rescales, so the force is Bl times 1 V over |Zin|.
    let f = 100.0;
    let force = bl * 1.0 / at(&c, "zin", f).norm();
    let p = force / sd * c_r / (c_f + c_r) * x / (x + rms);
    let solved = at(&c, "p_front", f).norm();
    assert!(db(solved / p).abs() < 0.5, "{} dB", db(solved / p));

    for f in logspace(50.0, 1500.0, 121) {
        let d = drum_re_500(&c, f);
        assert!(d.abs() < 1.5, "{f} Hz: {d} dB");
    }

    let band = logspace(2000.0, 8000.0, 49);
    let mean = |c: &Circuit| band.iter().map(|&f| drum_re_500(c, f)).sum::<f64>() / 49.0;
    let (vented, sealed) = (mean(&c), mean(&sealed()));
    assert!(vented > sealed + 5.0, "{vented} vs {sealed}");
}
