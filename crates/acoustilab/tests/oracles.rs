//! Analytical oracles of spec Section 17 and Appendix C, in the corrected
//! form of docs/spec-errata.md (E3, E4, E8–E13, E22, E23, E45).
//!
//! Every check solves a netlist through the public engine API and compares
//! the result with a closed form evaluated here, or with the independent
//! mpmath computation in `tests/data/oracles_reference.json`
//! (generator: `tools/oracles/reference.py`). Air is the spec's
//! analytical-check set (`spec_reference`): ρ = 1.204 kg/m³, c = 343 m/s,
//! μ = 1.81e-5 Pa·s, γ = 1.4, Pr = 0.71.
//!
//! Tolerances. Where the network *is* the closed form (lumped elements, a
//! lossless cavity), results must agree to round-off; those checks use 1e-9
//! relative, which leaves six orders of margin over the observed 1e-15 and
//! still catches any modelling slip. Where the spec states a physical
//! tolerance (0.5 % resonance, 0.3 dB slope, 5 % Q, 0.1 % Poiseuille limit)
//! the spec's tolerance is asserted as well, together with the erratum that
//! shows why the literal spec wording would fail a correct solver.

use acoustilab::{AirState, Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::PI;

// The spec's analytical-check air, written out independently of `air.rs`.
const RHO: f64 = 1.204;
const C_AIR: f64 = 343.0;
const MU: f64 = 1.81e-5;
const GAMMA: f64 = 1.4;
const PR: f64 = 0.71;
/// ρc² = γP0 (Pa).
const K0: f64 = RHO * C_AIR * C_AIR;
const P_REF: f64 = 20e-6;

// ----- Helpers ---------------------------------------------------------------

fn build(level: u8, nodes: &[(&str, &str)], elements: Vec<Value>, probes: Vec<Value>) -> Circuit {
    let doc = json!({
        "schema": "acoustilab-netlist/0.1",
        "air": {"preset": "spec_reference"},
        "sweep": {"frequencies_Hz": [1000.0]},
        "level": level,
        "nodes": nodes.iter().map(|(id, d)| json!({"id": id, "domain": d})).collect::<Vec<_>>(),
        "elements": elements,
        "probes": probes,
    });
    Circuit::from_json(&doc.to_string()).unwrap_or_else(|e| panic!("{e}\n{doc:#}"))
}

/// Value of probe `id` in the solution `x` at `f`.
fn probe(c: &Circuit, x: &[C64], f: f64, id: &str) -> C64 {
    let p = c
        .probes
        .iter()
        .find(|p| p.id == id)
        .unwrap_or_else(|| panic!("no probe '{id}'"));
    c.probe_value(p, f, x).unwrap()
}

/// Solves at `f` and reads probe `id`.
fn at(c: &Circuit, f: f64, id: &str) -> C64 {
    probe(c, &c.solve_at(f).unwrap(), f, id)
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

fn spl(p: C64) -> f64 {
    db(p.norm() / P_REF)
}

fn rel(a: C64, b: C64) -> f64 {
    (a - b).norm() / b.norm()
}

fn log_freqs(f0: f64, f1: f64, per_octave: f64) -> Vec<f64> {
    acoustilab::grid::log_grid(f0, f1, per_octave)
}

/// Average slope of `g` (dB) in dB per octave between `f0` and `f1`.
fn slope(f0: f64, f1: f64, g: impl Fn(f64) -> f64) -> f64 {
    (g(f1) - g(f0)) / (f1 / f0).log2()
}

/// Root of `g` on [lo, hi] by bisection (g must change sign).
fn bisect(mut lo: f64, mut hi: f64, g: impl Fn(f64) -> f64) -> f64 {
    let g_lo = g(lo);
    assert!(g_lo * g(hi) < 0.0, "no sign change on [{lo}, {hi}]");
    while hi - lo > 1e-14 * hi {
        let mid = 0.5 * (lo + hi);
        if (g(mid) < 0.0) == (g_lo < 0.0) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Maximum of a unimodal `g` on [a, b] by golden-section search.
fn golden_max(mut a: f64, mut b: f64, g: impl Fn(f64) -> f64) -> f64 {
    let r = (5f64.sqrt() - 1.0) / 2.0;
    let (mut c, mut d) = (b - r * (b - a), a + r * (b - a));
    let (mut gc, mut gd) = (g(c), g(d));
    while b - a > 1e-11 * (a + b) {
        if gc > gd {
            (b, d, gd) = (d, c, gc);
            c = b - r * (b - a);
            gc = g(c);
        } else {
            (a, c, gc) = (c, d, gd);
            d = a + r * (b - a);
            gd = g(d);
        }
    }
    0.5 * (a + b)
}

fn reference() -> Value {
    let text = include_str!("data/oracles_reference.json");
    serde_json::from_str(text).unwrap()
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

/// Moving-coil driver in the across/through convention (docs/conventions.md).
#[derive(Debug, Clone, Copy)]
struct Driver {
    re: f64,
    bl: f64,
    mms: f64,
    cms: f64,
    rms: f64,
    sd: f64,
}

impl Driver {
    /// Tymphany HPD-40N16PET00-32 primary set (spec Section 5, with the
    /// primary fields of erratum E5): fs 81.8 Hz, Qms 2.71, Qes 1.01,
    /// Re 32.8 Ω, Mms 0.3 g, Sd 10 cm². Cms, Rms and Bl are derived.
    fn tymphany() -> Self {
        let (fs, qms, qes, re, mms, sd) = (81.8, 2.71, 1.01, 32.8, 0.3e-3, 10e-4);
        let ws = 2.0 * PI * fs;
        Driver {
            re,
            bl: (ws * mms * re / qes).sqrt(),
            mms,
            cms: 1.0 / (ws * ws * mms),
            rms: ws * mms / qms,
            sd,
        }
    }

    /// Spec App. C2 example: Mms (0.1 g or 0.3 g), 1 mm/N, 10 cm². Re, Bl
    /// and Rms are not given there; these are test values, and the checks
    /// that use this driver do not depend on them beyond stated margins.
    fn c2(mms: f64) -> Self {
        Driver {
            re: 32.0,
            bl: 1.0,
            mms,
            cms: 1e-3,
            rms: 0.05,
            sd: 10e-4,
        }
    }

    /// Coil, motor, suspension and (if `piston` is given as (front, rear))
    /// the diaphragm, from source node `e_in` to the acoustic nodes.
    fn elements(&self, piston: Option<(&str, &str)>) -> Vec<Value> {
        let mut v = vec![
            json!({"id": "coil", "type": "coil", "nodes": ["e_in", "e_coil"], "Re_ohm": self.re}),
            json!({"id": "motor", "type": "motor", "nodes": ["e_coil", "gnd", "m_dia", "gnd"],
                   "Bl_Tm": self.bl}),
            json!({"id": "susp", "type": "suspension", "node": "m_dia", "Mms_kg": self.mms,
                   "Cms_m_per_N": self.cms, "Rms_Ns_per_m": self.rms}),
        ];
        if let Some((front, rear)) = piston {
            v.push(
                json!({"id": "dia", "type": "piston", "nodes": ["m_dia", "gnd", front, rear],
                          "Sd_m2": self.sd}),
            );
        }
        v
    }

    /// Cavity stiffness Sd²·ρc²/V seen by the diaphragm.
    fn k_cavity(&self, v: f64) -> f64 {
        self.sd * self.sd * K0 / v
    }

    /// Coupled resonance (App. C2).
    fn fc(&self, v: f64) -> f64 {
        ((1.0 / self.cms + self.k_cavity(v)) / self.mms).sqrt() / (2.0 * PI)
    }

    fn fs(&self) -> f64 {
        1.0 / (2.0 * PI * (self.mms * self.cms).sqrt())
    }

    /// Closed-form front pressure (lossless cavity V) at voltage `volts`.
    fn pressure(&self, f: f64, v: f64, volts: f64) -> C64 {
        let jw = C64::new(0.0, 2.0 * PI * f);
        let caf = v / K0;
        let zm = jw * self.mms + self.rms + (jw * self.cms).inv() + self.sd * self.sd / (jw * caf);
        let i = volts / (self.re + self.bl * self.bl / zm);
        self.sd * (self.bl * i / zm) / (jw * caf)
    }
}

const DRIVER_NODES: [(&str, &str); 3] = [
    ("e_in", "electrical"),
    ("e_coil", "electrical"),
    ("m_dia", "mechanical"),
];

/// Driver at `volts` into a single front cavity; probes p, zin, x, i.
fn sealed_cup(d: Driver, cavity: Value, volts: f64) -> Circuit {
    let mut nodes = DRIVER_NODES.to_vec();
    nodes.push(("a_front", "acoustic"));
    let mut el = vec![json!({"id": "amp", "type": "vsource", "node": "e_in", "V_V": volts})];
    el.extend(d.elements(Some(("a_front", "gnd"))));
    let mut cav = cavity;
    cav["id"] = json!("front");
    cav["type"] = json!("cavity");
    cav["node"] = json!("a_front");
    el.push(cav);
    build(
        1,
        &nodes,
        el,
        vec![
            json!({"id": "p", "quantity": "pressure", "node": "a_front"}),
            json!({"id": "zin", "quantity": "impedance", "element": "amp"}),
            json!({"id": "x", "quantity": "displacement", "node": "m_dia"}),
            json!({"id": "i", "quantity": "current", "element": "amp"}),
        ],
    )
}

// ----- Pressure chamber (Section 17 row 1, App. C1, erratum E10) -------------

#[test]
fn air_constants_are_the_spec_reference_set() {
    let a = AirState::spec_reference();
    assert_eq!(
        (a.rho, a.c, a.mu, a.gamma, a.prandtl),
        (RHO, C_AIR, MU, GAMMA, PR)
    );
    assert!((a.bulk_modulus() / K0 - 1.0).abs() < 1e-15);
    // App. C: ρc² = 1.4165e5 Pa.
    assert!((K0 - 1.4165e5).abs() < 10.0);
}

#[test]
fn pressure_chamber_is_flat_and_at_rho_c2_sd_x_over_v_with_lossless_cavity() {
    // E10: the level check holds exactly for a lossless adiabatic cavity.
    let d = Driver::c2(0.1e-3);
    let v = 30e-6;
    let c = sealed_cup(d, json!({"volume_m3": v, "wall_loss": false}), 1.0);
    let fc = d.fc(v);
    // Flatness: within 0.1 dB from 20 Hz to fc/10 (spec). The ideal
    // second-order rise alone is −20·log10(1 − 0.01) = 0.087 dB at fc/10.
    let ref20 = spl(at(&c, 20.0, "p"));
    let worst = log_freqs(20.0, fc / 10.0, 48.0)
        .iter()
        .map(|&f| (spl(at(&c, f, "p")) - ref20).abs())
        .fold(0.0, f64::max);
    assert!(worst < 0.1, "flatness {worst} dB");
    assert!(
        worst > 0.08,
        "flatness {worst} dB: expected the 0.087 dB rise"
    );
    // Level: p = ρc²·Sd·x/V with the solved x (exact for a lossless cavity).
    for f in [20.0, 60.0, fc / 10.0] {
        let x = c.solve_at(f).unwrap();
        let (p, disp) = (probe(&c, &x, f, "p"), probe(&c, &x, f, "x"));
        assert!(rel(p, disp * (K0 * d.sd / v)) < 1e-9, "f = {f}");
    }
    // And against the static excursion of App. C1, x = Bl·V/(Re·K_total):
    // within the spec's 0.05 dB at 20 Hz (the dynamic terms are 0.003 dB).
    let x_static = d.bl * 1.0 / (d.re * (1.0 / d.cms + d.k_cavity(v)));
    let p_static = K0 * d.sd * x_static / v;
    let err = (spl(at(&c, 20.0, "p")) - db(p_static / P_REF)).abs();
    assert!(err < 0.05, "level error {err} dB");
    // Worked example (C1, E40): 10 cm², 30 cm³, 1 µm RMS → 4.72 Pa, 107.46 dB.
    let p = K0 * 10e-4 * 1e-6 / 30e-6;
    assert!((p - 4.7216).abs() < 1e-3 && (db(p / P_REF) - 107.46).abs() < 0.01);
}

#[test]
fn pressure_chamber_with_wall_loss_matches_eps_corrected_level() {
    // E10: with the L0 thermal wall layer the cavity compliance is
    // C = V/(ρc²)·[1 + ε(1 − j)], ε = (γ−1)·δt·A_wall/(2V), so
    // |p| = ρc²·Sd·|x| / (V·|1 + ε(1 − j)|).
    let d = Driver::c2(0.1e-3);
    let (v, wall) = (30e-6, 60e-4);
    let c = sealed_cup(
        d,
        json!({"volume_m3": v, "wall_area_m2": wall, "wall_loss": true}),
        1.0,
    );
    let eps = |f: f64| {
        let delta_t = (2.0 * MU / (RHO * 2.0 * PI * f)).sqrt() / PR.sqrt();
        (GAMMA - 1.0) * delta_t * wall / (2.0 * v)
    };
    // App. C6: about 2.3 % at 20 Hz for 30 cm³ with 60 cm² of wall.
    assert!((eps(20.0) - 0.0232).abs() < 0.0005, "ε = {}", eps(20.0));
    for f in [20.0, 50.0, 120.0] {
        let x = c.solve_at(f).unwrap();
        let (p, disp) = (probe(&c, &x, f, "p"), probe(&c, &x, f, "x"));
        let factor = C64::new(1.0 + eps(f), -eps(f));
        let expect = disp * (K0 * d.sd / v) / factor;
        assert!(rel(p, expect) < 1e-9, "f = {f}: {p} vs {expect}");
        assert!((p.norm() - K0 * d.sd * disp.norm() / (v * factor.norm())).abs() < 1e-9 * p.norm());
    }
    // The literal spec check (lossless formula, 0.05 dB) fails by ≈ 0.2 dB.
    let x = c.solve_at(20.0).unwrap();
    let lossless = K0 * d.sd * probe(&c, &x, 20.0, "x").norm() / v;
    let gap = db(lossless / probe(&c, &x, 20.0, "p").norm());
    assert!(gap > 0.15 && gap < 0.25, "gap {gap} dB");
}

// ----- Coupled resonance and high-frequency fall (App. C2, erratum E8) --------

#[test]
fn coupled_resonance_and_high_frequency_fall() {
    let v = 30e-6;
    for (mms, f_worked) in [(0.1e-3, 1203.9), (0.3e-3, 695.1)] {
        let d = Driver::c2(mms);
        let fc = d.fc(v);
        // Worked values (App. C2; recomputed in the spec review): 1204 / 695 Hz.
        assert!((fc - f_worked).abs() < 0.5, "fc = {fc}");
        let c = sealed_cup(d, json!({"volume_m3": v, "wall_loss": false}), 1.0);
        // Resonance where the input impedance is real (series mechanical
        // resonance): exact, so far inside the spec's 0.5 %.
        let f_res = bisect(0.5 * fc, 2.0 * fc, |f| at(&c, f, "zin").im);
        assert!((f_res / fc - 1.0).abs() < 1e-9, "{f_res} vs {fc}");
        // Exact lumped response to 16 kHz.
        for f in log_freqs(10.0, 16_000.0, 24.0) {
            let p = at(&c, f, "p");
            assert!(rel(p, d.pressure(f, v, 1.0)) < 1e-9, "f = {f}");
        }
        // −12 dB/oct within 0.3 dB measured over f ≥ 8·fc (E8).
        let s = slope(8.0 * fc, 16_000.0, |f| spl(at(&c, f, "p")));
        assert!((s + 12.0).abs() < 0.3, "slope {s} dB/oct above 8 fc");
        if mms == 0.1e-3 {
            // E8: the literal 4–16 kHz band is −12.4…−12.7 dB/oct for this
            // driver, so it cannot pass ±0.3 dB with a correct solver.
            let literal = slope(4000.0, 16_000.0, |f| spl(at(&c, f, "p")));
            assert!(literal < -12.3, "4–16 kHz slope {literal}");
        }
    }
}

// ----- Leak slopes (App. C3, erratum E9) -----------------------------------

/// Flow source into a lossless 30 cm³ cavity at "a" with `leak` from "a" to
/// ambient, and an identical sealed cavity at "b". H = p_a / p_b.
fn leak_pair(level: u8, leak: Value) -> Circuit {
    let mut leak = leak;
    leak["id"] = json!("leak");
    leak["nodes"] = json!(["a", "ambient"]);
    build(
        level,
        &[("a", "acoustic"), ("b", "acoustic")],
        vec![
            json!({"id": "ua", "type": "flow_source", "node": "a", "U_m3_per_s": 1e-6}),
            json!({"id": "ca", "type": "cavity", "node": "a", "volume_cm3": 30, "wall_loss": false}),
            leak,
            json!({"id": "ub", "type": "flow_source", "node": "b", "U_m3_per_s": 1e-6}),
            json!({"id": "cb", "type": "cavity", "node": "b", "volume_cm3": 30, "wall_loss": false}),
        ],
        vec![
            json!({"id": "pa", "quantity": "pressure", "node": "a"}),
            json!({"id": "pb", "quantity": "pressure", "node": "b"}),
        ],
    )
}

fn leak_h(c: &Circuit, f: f64) -> C64 {
    let x = c.solve_at(f).unwrap();
    probe(c, &x, f, "pa") / probe(c, &x, f, "pb")
}

/// |H| in dB as a function of frequency.
fn h_db(c: &Circuit) -> impl Fn(f64) -> f64 + '_ {
    move |f| db(leak_h(c, f).norm())
}

/// |p/U| (probe "z") in dB as a function of frequency.
fn z_db(c: &Circuit) -> impl Fn(f64) -> f64 + '_ {
    move |f| db(at(c, f, "z").norm())
}

/// H = Z_L / (Z_L + 1/(jωC)) (App. C3).
fn h_exact(z_leak: C64, f: f64, cap: f64) -> C64 {
    let jw = C64::new(0.0, 2.0 * PI * f);
    z_leak / (z_leak + (jw * cap).inv())
}

#[test]
fn leak_slopes_and_exact_transfer() {
    let cap = 30e-6 / K0;
    let f_rc = 200.0; // resistive corner
    let f0 = 500.0; // inertive (Helmholtz) corner
    let r = 1.0 / (2.0 * PI * f_rc * cap);
    let m = 1.0 / ((2.0 * PI * f0).powi(2) * cap);
    let resistive = leak_pair(
        0,
        json!({"type": "acoustic_resistance", "R_Pa_s_per_m3": r}),
    );
    let inertive = leak_pair(0, json!({"type": "acoustic_inertance", "M_kg_per_m4": m}));

    // Exact H over the band (the lossless inertive case is singular at f0,
    // which the 1/12-octave grid below does not hit).
    for f in log_freqs(10.0, 10_000.0, 12.0) {
        let jw = C64::new(0.0, 2.0 * PI * f);
        assert!(rel(leak_h(&resistive, f), h_exact(C64::new(r, 0.0), f, cap)) < 1e-9);
        assert!(rel(leak_h(&inertive, f), h_exact(jw * m, f, cap)) < 1e-9);
    }
    // +6 and +12 dB/oct within 0.3 dB over two octaves ending at fc/8 (E9).
    let s1 = slope(f_rc / 32.0, f_rc / 8.0, h_db(&resistive));
    let s2 = slope(f0 / 32.0, f0 / 8.0, h_db(&inertive));
    assert!((s1 - 6.02).abs() < 0.3, "resistive {s1}");
    assert!((s2 - 12.04).abs() < 0.3, "inertive {s2}");
    // E9: the literal band (two octaves below the corner) misses by > 1 dB.
    let literal = slope(f_rc / 4.0, f_rc, h_db(&resistive));
    assert!((literal - 6.02).abs() > 1.0, "literal {literal}");
}

#[test]
fn mixed_leak_transition_and_helmholtz_q() {
    let cap = 30e-6 / K0;
    let f0 = 500.0;
    let m = 1.0 / ((2.0 * PI * f0).powi(2) * cap);
    for q in [20.0, 100.0] {
        let r = (m / cap).sqrt() / q;
        let c = leak_pair(
            0,
            json!({"type": "acoustic_impedance", "R_Pa_s_per_m3": r, "M_kg_per_m4": m}),
        );
        for f in log_freqs(0.1, 10_000.0, 6.0) {
            let jw = C64::new(0.0, 2.0 * PI * f);
            assert!(
                rel(leak_h(&c, f), h_exact(jw * m + r, f, cap)) < 1e-9,
                "f = {f}"
            );
        }
        // Helmholtz Q from the half-power bandwidth of p at constant volume
        // velocity, against sqrt(M/C)/R (spec: within 5 %).
        let mag = |f: f64| at(&c, f, "pa").norm();
        let fp = golden_max(0.8 * f0, 1.2 * f0, mag);
        let half = mag(fp) / 2f64.sqrt();
        let f1 = bisect(0.5 * f0, fp, |f| mag(f) - half);
        let f2 = bisect(fp, 2.0 * f0, |f| mag(f) - half);
        let q_meas = fp / (f2 - f1);
        assert!((q_meas / q - 1.0).abs() < 0.05, "Q {q_meas} vs {q}");
        // Its resistive-to-inertive corner R/(2πM) = f0/Q lies at least a
        // decade below the test band [f1, f2].
        let f_rm = r / (2.0 * PI * m);
        assert!(f_rm <= f1 / 10.0, "corner {f_rm} vs band from {f1}");
        if q == 100.0 {
            // Transition: +6 dB/oct well below the corner, +12 above it.
            let g = |f: f64| db(leak_h(&c, f).norm());
            let low = slope(f_rm / 32.0, f_rm / 8.0, g);
            let high = slope(8.0 * f_rm, f0 / 8.0, g);
            assert!((low - 6.02).abs() < 0.3, "below corner {low}");
            assert!((high - 12.04).abs() < 0.3, "above corner {high}");
        }
    }
}

// ----- Slit leak worked examples (Section 17, App. C3, erratum E45) ---------

#[test]
fn slit_leak_worked_examples_match_independent_reference() {
    let reference = reference();
    let cap = 30e-6 / K0;
    for rec in reference["slit_examples"].as_array().unwrap() {
        let gap = num(&rec["gap_mm"]);
        let slit = json!({"type": "slit", "gap_mm": gap, "width_mm": 30, "length_mm": 10});
        // Corner definitions of E45 from the Poiseuille values.
        let r_pois = 12.0 * MU * 10e-3 / (30e-3 * (gap * 1e-3).powi(3));
        assert!((r_pois / num(&rec["R_pois"]) - 1.0).abs() < 1e-12);
        let rc_corner = 1.0 / (2.0 * PI * r_pois * cap);
        for lv in rec["levels"].as_array().unwrap() {
            let level = num(&lv["level"]) as u8;
            let c = leak_pair(level, slit.clone());
            for row in lv["H"].as_array().unwrap() {
                let (f, h) = (num(&row[0]), C64::new(num(&row[1]), num(&row[2])));
                assert!(rel(leak_h(&c, f), h) < 1e-9, "gap {gap} L{level} f {f}");
            }
            let mag = |f: f64| leak_h(&c, f).norm();
            if gap == 1.0 {
                let fp = golden_max(400.0, 650.0, mag);
                assert!((fp / num(&lv["peak_Hz"]) - 1.0).abs() < 1e-6, "peak {fp}");
                assert!((db(mag(fp)) - num(&lv["peak_dB"])).abs() < 1e-6);
                let half = mag(fp) / 2f64.sqrt();
                let f1 = bisect(300.0, fp, |f| mag(f) - half);
                let f2 = bisect(fp, 900.0, |f| mag(f) - half);
                let q = fp / (f2 - f1);
                assert!((q / num(&lv["Q_half_power"]) - 1.0).abs() < 1e-5, "Q {q}");
                // E45: ≈523 Hz ("near 550" in the spec), with a Q far below
                // the Poiseuille estimate (19–21) because R grows as √f.
                assert!((fp - 523.0).abs() < 2.0, "peak {fp}");
                assert!(q < 0.5 * num(&rec["Q_poiseuille_estimate"]), "Q {q}");
            } else {
                // E45: −3 dB at 73 Hz against the 83 Hz RC corner, and a
                // 0.8 dB bump but no resonance (Poiseuille Q 0.34).
                assert!((rc_corner - 83.0).abs() < 0.1, "RC corner {rc_corner}");
                assert!((rc_corner / num(&rec["rc_corner_Hz"]) - 1.0).abs() < 1e-12);
                let f3 = bisect(30.0, 120.0, |f| mag(f) - 0.5f64.sqrt());
                assert!((f3 / num(&lv["minus3dB_Hz"]) - 1.0).abs() < 1e-6, "{f3}");
                assert!((f3 - 73.0).abs() < 0.5, "−3 dB at {f3}");
                let fp = golden_max(150.0, 1000.0, mag);
                let bump = db(mag(fp));
                assert!((bump - num(&lv["peak_dB"])).abs() < 1e-6);
                assert!((bump - 0.78).abs() < 0.02, "bump {bump} dB");
                assert!(num(&rec["Q_poiseuille_estimate"]) < 0.5);
            }
        }
    }
}

#[test]
fn wide_slit_is_inertive_above_a_few_tens_of_hertz() {
    // Spec C3: "inertive above a few tens of hertz". The exact crossover
    // Im Z = Re Z is 24.25 Hz (reference); the Poiseuille estimate
    // R/(2πM) = 12μ/(2π·(6/5)·ρh²) is 23.9 Hz.
    let reference = reference();
    let rec = &reference["slit_examples"][0];
    assert_eq!(num(&rec["gap_mm"]), 1.0);
    let f_pois = num(&rec["R_pois"]) / (2.0 * PI * num(&rec["M_pois"]));
    assert!((f_pois - 23.9).abs() < 0.05, "{f_pois}");
    for level in [0, 1] {
        let c = build(
            level,
            &[("a", "acoustic")],
            vec![
                json!({"id": "u", "type": "flow_source", "node": "a"}),
                json!({"id": "slit", "type": "slit", "nodes": ["a", "ambient"],
                       "gap_mm": 1, "width_mm": 30, "length_mm": 10}),
            ],
            vec![json!({"id": "z", "quantity": "impedance", "element": "u"})],
        );
        let f_x = bisect(5.0, 200.0, |f| {
            let z = at(&c, f, "z");
            z.im - z.re
        });
        let exact = num(&rec["levels"][level as usize]["inertive_above_Hz"]);
        assert!(
            (f_x / exact - 1.0).abs() < 1e-7,
            "crossover {f_x} vs {exact} Hz"
        );
        assert!((f_x - f_pois).abs() < 1.0);
    }
}

// ----- Helmholtz vent (App. C4, errata E22 and E23) -------------------------

/// Flow source into a 100 cm³ lossless cavity vented to ambient by `vent`.
fn vent_cup(level: u8, vent: Value) -> Circuit {
    let mut vent = vent;
    vent["id"] = json!("vent");
    vent["nodes"] = json!(["a", "ambient"]);
    build(
        level,
        &[("a", "acoustic")],
        vec![
            json!({"id": "u", "type": "flow_source", "node": "a"}),
            json!({"id": "cup", "type": "cavity", "node": "a", "volume_cm3": 100, "wall_loss": false}),
            vent,
        ],
        vec![json!({"id": "z", "quantity": "impedance", "element": "u"})],
    )
}

/// Frequency where the cup's input impedance is real (Im Y = 0).
fn vent_resonance(c: &Circuit, lo: f64, hi: f64) -> f64 {
    bisect(lo, hi, |f| at(c, f, "z").im)
}

#[test]
fn helmholtz_vent_with_piston_end_corrections() {
    let reference = reference();
    let rv = &reference["helmholtz_vent"];
    let (a, wall) = (1.5e-3, 2e-3);
    let s = PI * a * a;
    // Low-frequency baffled-piston end correction, one per end.
    let end = 8.0 * a / (3.0 * PI);
    // Lossless: an ideal inertance ρ(L + 2·8a/3π)/S (App. C4).
    let m_vent = RHO * (wall + 2.0 * end) / s;
    let lossless = vent_cup(
        0,
        json!({"type": "acoustic_inertance", "M_kg_per_m4": m_vent}),
    );
    let f_h = vent_resonance(&lossless, 150.0, 300.0);
    let formula = C_AIR / (2.0 * PI) * (s / (100e-6 * (wall + 2.0 * end))).sqrt();
    assert!((f_h / formula - 1.0).abs() < 1e-9);
    assert!((f_h / num(&rv["lossless_piston_ends_Hz"]) - 1.0).abs() < 1e-9);
    assert!((f_h - 215.0).abs() < 0.5, "{f_h}");
    // Deliberate no-correction case: must miss 215 Hz by more than 30 %.
    let bare = vent_cup(
        0,
        json!({"type": "acoustic_inertance", "M_kg_per_m4": RHO * wall / s}),
    );
    let f_bare = vent_resonance(&bare, 200.0, 450.0);
    assert!((f_bare / num(&rv["lossless_no_ends_Hz"]) - 1.0).abs() < 1e-9);
    assert!(f_bare / 215.0 - 1.0 > 0.3, "no-correction {f_bare}");

    // The tube element's "piston" ends add exactly 2·ρ·(8a/3π)/S at L0.
    let tube = |ends: &str| json!({"type": "tube", "radius_mm": 1.5, "length_mm": 2, "inlet": ends, "outlet": ends});
    let with = vent_cup(0, tube("piston"));
    let without = vent_cup(0, tube("none"));
    for f in [50.0, 215.0, 1000.0] {
        // The cavity is in parallel: compare vent impedances via admittances.
        let y_cav = C64::new(0.0, 2.0 * PI * f * 100e-6 / K0);
        let zv = |c: &Circuit| (at(c, f, "z").inv() - y_cav).inv();
        let dz = zv(&with) - zv(&without);
        let expect = C64::new(0.0, 2.0 * PI * f * RHO * 2.0 * end / s);
        assert!(rel(dz, expect) < 1e-6, "f = {f}: {dz} vs {expect}");
    }
    // Thermoviscous neck (E22): the boundary layer adds mass, tuning the
    // same vent to ≈210 Hz; independent reference at both levels.
    for rec in rv["thermoviscous"].as_array().unwrap() {
        let ends = rec["ends"].as_str().unwrap();
        let level = num(&rec["level"]) as u8;
        let c = vent_cup(level, tube(ends));
        let (lo, hi) = if ends == "piston" {
            (150.0, 260.0)
        } else {
            (250.0, 400.0)
        };
        let f_r = vent_resonance(&c, lo, hi);
        assert!(
            (f_r / num(&rec["resonance_Hz"]) - 1.0).abs() < 1e-7,
            "{ends} L{level}: {f_r}"
        );
        if ends == "piston" {
            assert!(f_r < f_h && f_r > 0.96 * f_h, "{f_r}");
        } else {
            assert!(f_r / 215.0 - 1.0 > 0.3, "no-correction thermoviscous {f_r}");
        }
    }
}

// ----- Electrical impedance (App. C5) ----------------------------------------

/// Driver at 1 V with an optional front load: `None` is free air;
/// `Some(r_series)` puts a lossless 30 cm³ cavity behind a series acoustic
/// resistance (0 for none).
fn impedance_rig(d: Driver, load: Option<f64>) -> Circuit {
    let mut nodes = DRIVER_NODES.to_vec();
    let mut el = vec![json!({"id": "amp", "type": "vsource", "node": "e_in"})];
    match load {
        None => el.extend(d.elements(None)),
        Some(r) => {
            nodes.push(("a_front", "acoustic"));
            el.extend(d.elements(Some(("a_front", "gnd"))));
            if r > 0.0 {
                nodes.push(("a_cav", "acoustic"));
                el.push(json!({"id": "ra", "type": "acoustic_resistance",
                               "nodes": ["a_front", "a_cav"], "R_Pa_s_per_m3": r}));
                el.push(json!({"id": "front", "type": "cavity", "node": "a_cav",
                               "volume_cm3": 30, "wall_loss": false}));
            } else {
                el.push(json!({"id": "front", "type": "cavity", "node": "a_front",
                               "volume_cm3": 30, "wall_loss": false}));
            }
        }
    }
    build(
        1,
        &nodes,
        el,
        vec![json!({"id": "zin", "quantity": "impedance", "element": "amp"})],
    )
}

#[test]
fn impedance_peak_cavity_shift_and_acoustic_resistance() {
    let d = Driver::tymphany();
    // E5: the primary set derives Bl = 2.24 T·m.
    assert!((d.bl - 2.24).abs() < 0.005, "Bl {}", d.bl);
    let resonance = |c: &Circuit, f: f64| {
        let fr = bisect(0.5 * f, 2.0 * f, |g| at(c, g, "zin").im);
        (fr, at(c, fr, "zin"))
    };
    // Free air: peak Re + Bl²/Rms at fs (spec: within 0.1 %).
    let (fs, zs) = resonance(&impedance_rig(d, None), d.fs());
    let peak = d.re + d.bl * d.bl / d.rms;
    assert!((fs / d.fs() - 1.0).abs() < 1e-9);
    assert!((zs.norm() / peak - 1.0).abs() < 1e-9, "{zs} vs {peak}");
    // Sealed lossless cavity: fc² − fs² = Sd²/(4π²·Mms·Caf) (spec: 0.5 %),
    // same peak height.
    let caf = 30e-6 / K0;
    let (fc, zc) = resonance(&impedance_rig(d, Some(0.0)), d.fc(30e-6));
    let shift = d.sd * d.sd / (4.0 * PI * PI * d.mms * caf);
    assert!(
        ((fc * fc - fs * fs) / shift - 1.0).abs() < 1e-8,
        "{fc} {fs}"
    );
    assert!((zc.norm() / peak - 1.0).abs() < 1e-9);
    // Series acoustic resistance adds Sd²·Ra to the mechanical loss: the
    // peak drops to Re + Bl²/(Rms + Sd²·Ra) at the same frequency.
    let ra = 5e4;
    let (fr, zr) = resonance(&impedance_rig(d, Some(ra)), fc);
    let lowered = d.re + d.bl * d.bl / (d.rms + d.sd * d.sd * ra);
    assert!((fr / fc - 1.0).abs() < 1e-9);
    assert!(
        (zr.norm() / lowered - 1.0).abs() < 1e-9,
        "{zr} vs {lowered}"
    );
    assert!(zr.norm() < 0.8 * peak);
}

// ----- Sensitivity scaling (App. C7) -----------------------------------------

#[test]
fn sensitivity_scaling_under_three_drives() {
    // Coil rewound 32 → 300 Ω at constant mass: Bl ∝ √Re.
    let v = 30e-6;
    let base = Driver::c2(0.1e-3);
    let rewound = Driver {
        re: 300.0,
        bl: base.bl * (300.0f64 / 32.0).sqrt(),
        ..base
    };
    let exact = 10.0 * (300.0f64 / 32.0).log10();
    assert!((exact - 9.72).abs() < 0.005);
    let rig = |d: Driver, drive: &str| {
        let src = match drive {
            "voltage" => json!({"id": "amp", "type": "vsource", "node": "e_in", "V_V": 1.0}),
            // Constant power: 1 mW into the nominal (DC) resistance.
            "power" => json!({"id": "amp", "type": "vsource", "node": "e_in",
                              "V_V": (1e-3 * d.re).sqrt()}),
            _ => json!({"id": "amp", "type": "isource", "node": "e_in", "I_A": 1.0}),
        };
        let mut el = vec![src];
        el.extend(d.elements(Some(("a", "gnd"))));
        el.push(
            json!({"id": "front", "type": "cavity", "node": "a", "volume_m3": v,
                       "wall_loss": false}),
        );
        let mut nodes = DRIVER_NODES.to_vec();
        nodes.push(("a", "acoustic"));
        build(
            1,
            &nodes,
            el,
            vec![json!({"id": "p", "quantity": "pressure", "node": "a"})],
        )
    };
    for (drive, expect) in [("voltage", -exact), ("power", 0.0), ("current", exact)] {
        let (a, b) = (rig(base, drive), rig(rewound, drive));
        for f in [30.0, 300.0, 1200.0, 3000.0] {
            let d = db(at(&b, f, "p").norm() / at(&a, f, "p").norm());
            assert!((d - expect).abs() < 1e-9, "{drive} at {f} Hz: {d} dB");
        }
    }
}

// ----- Lumped validity (erratum E3) -------------------------------------------

/// Flow source into the driver face of a two-node cylindrical cavity
/// (radius 10 mm, depth `depth_mm`).
fn depth_cavity(level: u8, depth_mm: f64, wall_loss: bool) -> Circuit {
    build(
        level,
        &[("a", "acoustic"), ("far", "acoustic")],
        vec![
            json!({"id": "u", "type": "flow_source", "node": "a"}),
            json!({"id": "cav", "type": "cavity", "nodes": ["a", "far"], "radius_mm": 10,
                   "depth_mm": depth_mm, "wall_loss": wall_loss}),
        ],
        vec![
            json!({"id": "p", "quantity": "pressure", "node": "a"}),
            json!({"id": "pfar", "quantity": "pressure", "node": "far"}),
        ],
    )
}

#[test]
fn lumped_cavity_against_depth_line() {
    let depth = 0.060;
    let kl_to_f = |kl: f64| kl * C_AIR / (2.0 * PI * depth);
    let (l0, l1) = (depth_cavity(0, 60.0, false), depth_cavity(1, 60.0, false));
    let diff = |f: f64| db(at(&l1, f, "p").norm() / at(&l0, f, "p").norm());
    // The input compliance error is exactly kL·cot(kL) for a lossless line.
    for f in log_freqs(10.0, kl_to_f(0.3), 12.0) {
        let kl = 2.0 * PI * f * depth / C_AIR;
        let d = diff(f);
        assert!((d - db(kl / kl.tan())).abs() < 1e-9, "f = {f}");
        assert!(d.abs() < 0.3, "{d} dB at kL {kl}");
    }
    // E3: 0.27 dB at kL 0.3 and 3.85 dB at kL 1.
    assert!((diff(kl_to_f(0.3)) + 0.27).abs() < 0.005);
    assert!(
        (diff(kl_to_f(1.0)) + 3.85).abs() < 0.005,
        "{}",
        diff(kl_to_f(1.0))
    );
    // The far-wall ratio 1/cos(kL) that C8 quotes is a different metric:
    // 5.35 dB at kL 1 (E3).
    let f1 = kl_to_f(1.0);
    let far = db(at(&l1, f1, "pfar").norm() / at(&l1, f1, "p").norm());
    assert!((far - 5.35).abs() < 0.005, "far wall {far} dB");
    // With wall loss the two levels still agree within 0.3 dB below kL 0.3.
    let (w0, w1) = (depth_cavity(0, 60.0, true), depth_cavity(1, 60.0, true));
    for f in log_freqs(10.0, kl_to_f(0.3), 12.0) {
        let d = db(at(&w1, f, "p").norm() / at(&w0, f, "p").norm());
        assert!(d.abs() < 0.3, "{d} dB at {f} Hz");
    }
    // The lumped cavity's validity shading begins at the 10 % frequency
    // (kL ≈ 0.54, 493 Hz for 60 mm) and deepens at 36 % (≈ 910 Hz).
    let s = l0.shading();
    assert!((s.begin_hz.unwrap() - 493.0).abs() < 3.0, "{s:?}");
    assert!((s.deep_hz.unwrap() - 910.0).abs() < 5.0, "{s:?}");
}

// ----- Thermoviscous limits (Section 17, App. D, errata E1/E2) ---------------

fn duct_json(rec: &Value) -> Value {
    let mut e = json!({"id": "duct", "type": rec["type"]});
    for k in ["gap_mm", "width_mm", "length_mm", "radius_mm"] {
        if let Some(v) = rec.get(k) {
            e[k] = v.clone();
        }
    }
    e
}

/// ABCD matrix of a duct at L1, measured through the solver with its far
/// end shorted to ambient (B, D) and open (A, C).
fn measured_abcd(duct: &Value, f: f64) -> [C64; 4] {
    let probes = vec![
        json!({"id": "v1", "quantity": "port_potential", "element": "duct", "port": 0}),
        json!({"id": "i1", "quantity": "flow", "element": "duct", "port": 0}),
        json!({"id": "v2", "quantity": "port_potential", "element": "duct", "port": 1}),
        json!({"id": "i2in", "quantity": "flow", "element": "duct", "port": 1}),
    ];
    let rig = |far: &str| {
        let mut d = duct.clone();
        d["nodes"] = json!(["a", far]);
        let nodes: Vec<(&str, &str)> = if far == "b" {
            vec![("a", "acoustic"), ("b", "acoustic")]
        } else {
            vec![("a", "acoustic")]
        };
        let src = json!({"id": "u", "type": "flow_source", "node": "a"});
        build(1, &nodes, vec![src, d], probes.clone())
    };
    let (short, open) = (rig("ambient"), rig("b"));
    let xs = short.solve_at(f).unwrap();
    let xo = open.solve_at(f).unwrap();
    let i2 = -probe(&short, &xs, f, "i2in");
    let v2 = probe(&open, &xo, f, "v2");
    assert!(probe(&open, &xo, f, "i2in").norm() < 1e-12 * probe(&open, &xo, f, "i1").norm());
    [
        probe(&open, &xo, f, "v1") / v2,
        probe(&short, &xs, f, "v1") / i2,
        probe(&open, &xo, f, "i1") / v2,
        probe(&short, &xs, f, "i1") / i2,
    ]
}

#[test]
fn thermoviscous_low_frequency_limits() {
    let reference = reference();
    for rec in reference["duct_limits"].as_array().unwrap() {
        let f = num(&rec["f_Hz"]);
        let w = 2.0 * PI * f;
        let (r0, m0) = (num(&rec["R_poiseuille"]), num(&rec["M_poiseuille"]));
        let (r_ex, m_ex) = (num(&rec["R_exact"]), num(&rec["M_exact"]));
        // Poiseuille formulas written out here: 12μl/(wh³), 6ρl/(5wh);
        // 8μL/(πa⁴), 4ρL/(3πa²).
        let l = num(&rec["length_mm"]) * 1e-3;
        let (r_f, m_f) = if rec["type"] == "slit" {
            let (h, wd) = (num(&rec["gap_mm"]) * 1e-3, num(&rec["width_mm"]) * 1e-3);
            (
                12.0 * MU * l / (wd * h.powi(3)),
                6.0 * RHO * l / (5.0 * wd * h),
            )
        } else {
            let a = num(&rec["radius_mm"]) * 1e-3;
            (
                8.0 * MU * l / (PI * a.powi(4)),
                4.0 * RHO * l / (3.0 * PI * a * a),
            )
        };
        assert!((r_f / r0 - 1.0).abs() < 1e-12 && (m_f / m0 - 1.0).abs() < 1e-12);
        // L0: the element is the series impedance itself.
        let mut d = duct_json(rec);
        d["nodes"] = json!(["a", "ambient"]);
        let c = build(
            0,
            &[("a", "acoustic")],
            vec![json!({"id": "u", "type": "flow_source", "node": "a"}), d],
            vec![json!({"id": "z", "quantity": "impedance", "element": "u"})],
        );
        let z = at(&c, f, "z");
        let (r, m) = (z.re, z.im / w);
        assert!(
            (r / r_ex - 1.0).abs() < 1e-9 && (m / m_ex - 1.0).abs() < 1e-9,
            "{rec}"
        );
        assert!(
            (r / r0 - 1.0).abs() < 1e-3 && (m / m0 - 1.0).abs() < 1e-3,
            "{rec}"
        );
        // L1: the line's series impedance Γ·Zc·l, from its measured ABCD:
        // Zc = sqrt(B/C), Γl = asinh(B/Zc).
        let [a, b, cc, dd] = measured_abcd(&duct_json(rec), f);
        assert!((a * dd - b * cc - 1.0).norm() < 1e-9, "det of {rec}");
        let zc = (b / cc).sqrt();
        let z1 = zc * (b / zc).asinh();
        let (r1, m1) = (z1.re, z1.im / w);
        assert!(
            (r1 / r_ex - 1.0).abs() < 1e-6 && (m1 / m_ex - 1.0).abs() < 1e-6,
            "{rec}"
        );
        assert!(
            (r1 / r0 - 1.0).abs() < 1e-3 && (m1 / m0 - 1.0).abs() < 1e-3,
            "{rec}"
        );
    }
}

#[test]
fn thermoviscous_resistance_grows_as_sqrt_f() {
    let reference = reference();
    let hf = &reference["hf_resistance"];
    let c = build(
        0,
        &[("a", "acoustic")],
        vec![
            json!({"id": "u", "type": "flow_source", "node": "a"}),
            json!({"id": "t", "type": "tube", "nodes": ["a", "ambient"], "radius_mm": 2,
                   "length_mm": 10}),
        ],
        vec![json!({"id": "z", "quantity": "impedance", "element": "u"})],
    );
    let (r10, r40) = (at(&c, 10_000.0, "z").re, at(&c, 40_000.0, "z").re);
    assert!((r10 / num(&hf["R_10kHz"]) - 1.0).abs() < 1e-9);
    assert!((r40 / num(&hf["R_40kHz"]) - 1.0).abs() < 1e-9);
    // Thin-boundary-layer limit R ∝ √f: the ratio over two octaves is 2
    // up to O(δ/a) ≈ 1 % terms.
    assert!((r40 / r10 - 2.0).abs() < 0.04, "ratio {}", r40 / r10);
}

// ----- Baffle-vent sign (Section 17) -------------------------------------------

#[test]
fn front_to_rear_duct_lowers_low_frequency_front_pressure() {
    // A duct from the front to the rear cavity (sealed rear) short-circuits
    // the piston's pressure difference at low frequency. Under voltage drive
    // the front pressure is set by force balance, Sd·Δp ≈ Bl·i − K_s·x, so the
    // duct lowers it once its impedance falls below the suspension stiffness
    // reflected through Sd, and more so as its area grows. A sign error in
    // the piston's rear coupling would make the duct raise the front level
    // (the smaller rear cavity would then be the one at higher pressure).
    //
    // "Low frequency" is bounded by the resonance of the diaphragm with the
    // duct's air mass reflected through Sd² (≈34 Hz for the 1 mm duct here,
    // estimated with the geometric mass and flanged end corrections). Just
    // above it a damped peak legitimately lifts the level (+1.5 dB at 40 Hz
    // for 1 mm), so the band ends at half the lowest such resonance.
    let d = Driver::tymphany();
    let length = 2e-3;
    let rig = |radius_mm: Option<f64>| {
        let mut nodes = DRIVER_NODES.to_vec();
        nodes.extend([("a_front", "acoustic"), ("a_rear", "acoustic")]);
        let mut el = vec![json!({"id": "amp", "type": "vsource", "node": "e_in"})];
        el.extend(d.elements(Some(("a_front", "a_rear"))));
        el.push(json!({"id": "front", "type": "cavity", "node": "a_front", "volume_cm3": 30}));
        el.push(json!({"id": "rear", "type": "cavity", "node": "a_rear", "volume_cm3": 20}));
        if let Some(r) = radius_mm {
            el.push(
                json!({"id": "baffle_vent", "type": "tube", "nodes": ["a_front", "a_rear"],
                           "radius_mm": r, "length_m": length, "inlet": "flanged",
                           "outlet": "flanged"}),
            );
        }
        build(
            1,
            &nodes,
            el,
            vec![json!({"id": "p", "quantity": "pressure", "node": "a_front"})],
        )
    };
    let radii = [0.1, 0.25, 0.5, 1.0, 2.0];
    // (frequency, Q) of that resonance; damping from Rms, the electrical
    // damping Bl²/Re and the duct's Poiseuille resistance. Only resonances
    // that are not overdamped (Q > 0.5) can lift the level.
    let mass_resonance = |a_mm: f64| {
        let a = a_mm * 1e-3;
        let m_duct = RHO * (length + 2.0 * 0.8216 * a) / (PI * a * a);
        let r_duct = 8.0 * MU * length / (PI * a.powi(4));
        let m = d.mms + d.sd * d.sd * m_duct;
        let r = d.rms + d.bl * d.bl / d.re + d.sd * d.sd * r_duct;
        (
            1.0 / (2.0 * PI * (m * d.cms).sqrt()),
            (m / d.cms).sqrt() / r,
        )
    };
    let f_hi = 0.5
        * radii
            .iter()
            .map(|&a| mass_resonance(a))
            .filter(|&(_, q)| q > 0.5)
            .map(|(f, _)| f)
            .fold(f64::MAX, f64::min);
    assert!(f_hi > 15.0 && f_hi < 20.0, "band edge {f_hi}");
    let mut rigs = vec![rig(None)];
    rigs.extend(radii.iter().map(|&r| rig(Some(r))));
    for f in log_freqs(1.0, f_hi, 3.0) {
        let levels: Vec<f64> = rigs.iter().map(|c| spl(at(c, f, "p"))).collect();
        for k in 1..levels.len() {
            assert!(
                levels[k] < levels[k - 1],
                "f = {f}: larger vent raised SPL: {levels:?}"
            );
        }
    }
    // Not a marginal effect: at 1 Hz the 2 mm duct costs more than 40 dB.
    let drop = spl(at(&rigs[0], 1.0, "p")) - spl(at(&rigs[radii.len()], 1.0, "p"));
    assert!(drop > 40.0, "drop {drop} dB");
}

// ----- Asymptotes (erratum E13) ----------------------------------------------

#[test]
fn low_frequency_asymptotes_of_p_over_u() {
    let cap = 30e-6 / K0;
    let rig = |leak: Option<Value>, wall_loss: bool| {
        let mut el = vec![
            json!({"id": "u", "type": "flow_source", "node": "a"}),
            json!({"id": "cav", "type": "cavity", "node": "a", "volume_cm3": 30,
                   "wall_loss": wall_loss}),
        ];
        if let Some(mut l) = leak {
            l["id"] = json!("leak");
            l["nodes"] = json!(["a", "ambient"]);
            el.push(l);
        }
        build(
            1,
            &[("a", "acoustic")],
            el,
            vec![json!({"id": "z", "quantity": "impedance", "element": "u"})],
        )
    };
    // Sealed: p/U = 1/(jωC) falls 6 dB/oct and diverges toward DC.
    let sealed = rig(None, false);
    for (f0, f1) in [(1.0, 2.0), (10.0, 20.0), (1000.0, 2000.0)] {
        let s = slope(f0, f1, z_db(&sealed));
        assert!((s + 20.0 * 2f64.log10()).abs() < 1e-9, "sealed {s}");
    }
    // With the thermal wall layer ε ∝ f^−1/2 flattens it slightly.
    let s = slope(10.0, 20.0, z_db(&rig(None, true)));
    assert!((s + 6.02).abs() < 0.15, "sealed with wall loss {s}");
    // Resistive leak: p/U → R_leak. Deviation ≈ ωRC, 1e-4 here.
    let r = 1.0 / (2.0 * PI * 200.0 * cap);
    let res = rig(
        Some(json!({"type": "acoustic_resistance", "R_Pa_s_per_m3": r})),
        false,
    );
    for f in [0.1, 0.01] {
        let dev = (at(&res, f, "z") / r - 1.0).norm();
        assert!(dev < 1.1 * 2.0 * PI * f * r * cap, "f = {f}: {dev}");
    }
    // Inertive leak: p/U → jωM → 0 (+6 dB/oct toward DC).
    let m = 1.0 / ((2.0 * PI * 500.0).powi(2) * cap);
    let ine = rig(
        Some(json!({"type": "acoustic_inertance", "M_kg_per_m4": m})),
        false,
    );
    for f in [0.01, 0.1, 1.0] {
        let z = at(&ine, f, "z");
        assert!(rel(z, C64::new(0.0, 2.0 * PI * f * m)) < 1e-5, "f = {f}");
    }
    assert!((slope(0.01, 0.02, z_db(&ine)) - 6.02).abs() < 1e-3);
    // Real slit leak: p/U → the Poiseuille resistance 12μl/(wh³).
    let slit = rig(
        Some(json!({"type": "slit", "gap_mm": 0.2, "width_mm": 30, "length_mm": 10})),
        true,
    );
    let r_pois = 12.0 * MU * 10e-3 / (30e-3 * (0.2e-3f64).powi(3));
    let z = at(&slit, 0.01, "z");
    assert!((z / r_pois - 1.0).norm() < 1e-3, "{z} vs {r_pois}");
}

// ----- Sign convention (Section 3, erratum E12) --------------------------------

#[test]
fn sign_convention_at_20_hz_on_a_sealed_netlist() {
    let d = Driver::tymphany();
    let rig = |reversed: bool| {
        let mut nodes = DRIVER_NODES.to_vec();
        nodes.extend([("a_front", "acoustic"), ("a_drum", "acoustic")]);
        let mut el = vec![json!({"id": "amp", "type": "vsource", "node": "e_in", "V_V": 1})];
        el.extend(d.elements(Some(("a_front", "gnd"))));
        if reversed {
            el[2]["nodes"] = json!(["gnd", "e_coil", "m_dia", "gnd"]);
        }
        el.extend([
            json!({"id": "front", "type": "cavity", "node": "a_front", "volume_cm3": 30}),
            json!({"id": "canal", "type": "tube", "nodes": ["a_front", "a_drum"],
                   "diameter_mm": 7.5, "length_mm": 25}),
            json!({"id": "drum", "type": "cavity", "node": "a_drum", "volume_cm3": 0.5}),
        ]);
        build(
            1,
            &nodes,
            el,
            vec![
                json!({"id": "i", "quantity": "current", "element": "amp"}),
                json!({"id": "f_in", "quantity": "force", "element": "motor", "port": 1}),
                json!({"id": "x", "quantity": "displacement", "node": "m_dia"}),
                json!({"id": "p", "quantity": "pressure", "node": "a_front"}),
                json!({"id": "p_drum", "quantity": "pressure", "node": "a_drum"}),
            ],
        )
    };
    let f = 20.0;
    let c = rig(false);
    let x = c.solve_at(f).unwrap();
    let get = |id: &str| probe(&c, &x, f, id);
    assert!(get("i").re > 0.0, "current {}", get("i"));
    // The motor port flow is the force *entering* it; it delivers the negative.
    let force = -get("f_in");
    assert!(force.re > 0.0, "force toward the ear {force}");
    for id in ["x", "p", "p_drum"] {
        let v = get(id);
        assert!(v.re > 0.0 && v.arg().abs() < 0.1, "{id} = {v}");
    }
    // Reversing the coil polarity flips the drum pressure.
    let r = rig(true);
    assert!(at(&r, f, "p_drum").re < 0.0);
    // E12: DC is excluded from every grid, so the test cannot run at 0 Hz.
    let dc = json!({"sweep": {"frequencies_Hz": [0.0]}, "nodes": [], "elements": []});
    assert!(Circuit::from_json(&dc.to_string()).is_err());
}

// ----- Bleed estimate (App. C9) -------------------------------------------------

#[test]
fn bleed_monopole_estimate_of_appendix_c9() {
    // 94 dB in a sealed 30 cm³ front cavity: the volume velocity follows
    // from U = jωC·p, and the monopole at 1 m is ρ·f·|U|/(2r).
    let p94 = P_REF * 10f64.powf(94.0 / 20.0);
    let c = build(
        1,
        &[("a", "acoustic")],
        vec![
            json!({"id": "src", "type": "pressure_source", "node": "a", "p_Pa": p94}),
            json!({"id": "cav", "type": "cavity", "node": "a", "volume_cm3": 30, "wall_loss": false}),
        ],
        vec![json!({"id": "u", "quantity": "volume_velocity", "element": "src"})],
    );
    for (f, published) in [(1000.0, 32.1), (5000.0, 60.0)] {
        let u = at(&c, f, "u").norm();
        assert!((u / (2.0 * PI * f * 30e-6 / K0 * p94) - 1.0).abs() < 1e-9);
        let bleed = spl(C64::new(RHO * f * u / (2.0 * 1.0), 0.0));
        // Spec: 32 dB at 1 kHz and 60 dB at 5 kHz (review: 32.1 / 60.0).
        assert!((bleed - published).abs() < 0.1, "{bleed} dB at {f} Hz");
    }
}

#[test]
fn monopole_field_carries_the_power_the_radiation_element_absorbs() {
    // Integrating the C9 monopole intensity |ρ·f·U/(2r)|²/(ρc) over a sphere
    // gives W = π·ρ·f²·|U|²/c; a baffled source radiates into half space with
    // doubled pressure, W = 2π·ρ·f²·|U|²/c. At ka ≤ 0.02 the radiation
    // element's resistance must reproduce both to O((ka)²) ≈ 1e-4.
    let u = 1e-6;
    for (baffle, factor) in [("free", 1.0), ("infinite", 2.0)] {
        let c = build(
            1,
            &[("a", "acoustic")],
            vec![
                json!({"id": "src", "type": "flow_source", "node": "a", "U_m3_per_s": u}),
                json!({"id": "rad", "type": "radiation", "node": "a", "radius_mm": 1,
                       "baffle": baffle}),
            ],
            vec![],
        );
        for f in [200.0, 1000.0] {
            let x = c.solve_at(f).unwrap();
            let w = c
                .power_absorbed(f, &x)
                .into_iter()
                .find(|(id, _)| id == "rad")
                .unwrap()
                .1
                .unwrap();
            let expect = factor * PI * RHO * f * f * u * u / C_AIR;
            assert!(
                (w / expect - 1.0).abs() < 1e-3,
                "{baffle} at {f}: {w} vs {expect}"
            );
        }
    }
}
