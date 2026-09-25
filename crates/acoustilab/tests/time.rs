//! Time-domain outputs (spec Sections 3, 10 and 16; erratum E46): the FFT,
//! the uniform re-solve and its impulse responses, minimum and excess
//! phase, group delay, vector fitting and the impulse-length check.
//!
//! References: numpy (`tools/time/numerics_refs.py`, `tools/time/ir_refs.py`,
//! fixtures in `tests/data/time_*.json`) and closed forms of ideal
//! electrical networks evaluated here, which the engine solves exactly.
//! Each tolerance is stated where it is used, with its reason.

use acoustilab::params::{Overrides, Parametric};
use acoustilab::time::dense::eigenvalues;
use acoustilab::time::fft::{hermitian_full, Fft};
use acoustilab::time::minphase::{min_phase_spectrum, MinPhaseOptions};
use acoustilab::time::poles::{
    attribute_poles, fit_probe, ir_length_check, PoleFitOptions, SensitivityOptions,
};
use acoustilab::time::report::{impulses, ImpulseRequest};
use acoustilab::time::vfit::{vector_fit, Asymptote, Pole, RationalModel, VfOptions};
use acoustilab::time::{
    excess_phase, group_delay_dense, impulse, min_phase, phase_decision, uniform_response,
    UniformOptions,
};
use acoustilab::{Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::PI;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

fn example(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../examples/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn jw(f: f64) -> C64 {
    C64::new(0.0, 2.0 * PI * f)
}

/// Wraps an angle difference into (−π, π].
fn wrap(x: f64) -> f64 {
    x - 2.0 * PI * (x / (2.0 * PI)).round()
}

// ----- Networks with closed forms --------------------------------------------

/// Electrical netlist driven by 1 V at `in`, swept to `f_max`.
fn netlist(nodes: &[&str], elements: Vec<Value>, probe: Value, f_max: f64) -> Circuit {
    let doc = json!({
        "schema": "acoustilab-netlist/0.2",
        "sweep": {"f_min_Hz": 10, "f_max_Hz": f_max, "points_per_octave": 24},
        "nodes": nodes.iter().map(|n| json!({"id": n, "domain": "electrical"})).collect::<Vec<_>>(),
        "elements": elements,
        "probes": [probe],
    });
    Circuit::from_json(&doc.to_string()).unwrap_or_else(|e| panic!("{e}"))
}

fn vin() -> Value {
    json!({"id": "v", "type": "vsource", "node": "in", "V_V": 1})
}

fn probe_out(node: &str) -> Value {
    json!({"id": "out", "quantity": "voltage", "node": node})
}

/// RC high-pass: C from `in` to `out`, R from `out` to ground.
struct Hp {
    r: f64,
    c: f64,
}

impl Hp {
    fn corner(fc: f64) -> Hp {
        Hp {
            r: 1000.0,
            c: 1.0 / (2.0 * PI * fc * 1000.0),
        }
    }
    fn h(&self, s: C64) -> C64 {
        let t = self.r * self.c;
        s * t / (1.0 + s * t)
    }
    fn circuit(&self, f_max: f64) -> Circuit {
        netlist(
            &["in", "out"],
            vec![
                vin(),
                json!({"id": "c", "type": "capacitor", "nodes": ["in", "out"], "C_F": self.c}),
                json!({"id": "r", "type": "resistor", "node": "out", "R_ohm": self.r}),
            ],
            probe_out("out"),
            f_max,
        )
    }
}

/// Series R-L, C to ground: V(C)/V = 1/(1 + sRC + s²LC).
struct Lp {
    r: f64,
    l: f64,
    c: f64,
}

impl Lp {
    fn new(f0: f64, q: f64, c: f64) -> Lp {
        let w0 = 2.0 * PI * f0;
        let l = 1.0 / (w0 * w0 * c);
        Lp {
            r: (l / c).sqrt() / q,
            l,
            c,
        }
    }
    fn h(&self, s: C64) -> C64 {
        (1.0 + s * self.r * self.c + s * s * self.l * self.c).inv()
    }
    /// −Re(H′/H), s.
    fn group_delay(&self, f: f64) -> f64 {
        let s = jw(f);
        ((self.r * self.c + 2.0 * s * self.l * self.c)
            / (1.0 + s * self.r * self.c + s * s * self.l * self.c))
            .re
    }
    fn elements(&self) -> Vec<Value> {
        vec![
            vin(),
            json!({"id": "r", "type": "resistor", "nodes": ["in", "a"], "R_ohm": self.r}),
            json!({"id": "l", "type": "inductor", "nodes": ["a", "out"], "L_H": self.l}),
            json!({"id": "c", "type": "capacitor", "node": "out", "C_F": self.c}),
        ]
    }
    fn circuit(&self, f_max: f64) -> Circuit {
        netlist(
            &["in", "a", "out"],
            self.elements(),
            probe_out("out"),
            f_max,
        )
    }
}

/// Notch divider: R1 from `in` to `out`, then R2 + L + C to ground:
/// H = (s²LC + sR2C + 1)/(s²LC + s(R1 + R2)C + 1), minimum phase.
struct Notch {
    r1: f64,
    r2: f64,
    l: f64,
    c: f64,
}

impl Notch {
    fn new(f0: f64, qz: f64, qp: f64) -> Notch {
        let c = 1e-5;
        let w0 = 2.0 * PI * f0;
        let l = 1.0 / (w0 * w0 * c);
        let z0 = (l / c).sqrt();
        Notch {
            r1: z0 / qp - z0 / qz,
            r2: z0 / qz,
            l,
            c,
        }
    }
    fn h(&self, s: C64) -> C64 {
        let (l, c) = (self.l, self.c);
        (s * s * l * c + s * self.r2 * c + 1.0)
            / (s * s * l * c + s * (self.r1 + self.r2) * c + 1.0)
    }
    fn circuit(&self, f_max: f64) -> Circuit {
        netlist(
            &["in", "out", "a", "b"],
            vec![
                vin(),
                json!({"id": "r1", "type": "resistor", "nodes": ["in", "out"], "R_ohm": self.r1}),
                json!({"id": "r2", "type": "resistor", "nodes": ["out", "a"], "R_ohm": self.r2}),
                json!({"id": "l", "type": "inductor", "nodes": ["a", "b"], "L_H": self.l}),
                json!({"id": "c", "type": "capacitor", "node": "b", "C_F": self.c}),
            ],
            probe_out("out"),
            f_max,
        )
    }
}

/// A leaky ladder: C1 then R1 to ground (0.2 Hz high-pass, a corner far
/// below the bin spacing), loaded by R3 + L then C3 to ground (1 kHz,
/// Q ≈ 2). A ladder of R, L and C is minimum phase.
struct Leaky {
    c1: f64,
    r1: f64,
    lp: Lp,
}

impl Leaky {
    fn new() -> Leaky {
        Leaky {
            c1: 1.0 / (2.0 * PI * 0.2 * 1000.0),
            r1: 1000.0,
            lp: Lp::new(1000.0, 2.0, 1e-6),
        }
    }
    fn h(&self, s: C64) -> C64 {
        let z2 = self.lp.r + s * self.lp.l + (s * self.lp.c).inv();
        let zload = (1.0 / self.r1 + z2.inv()).inv();
        let va = zload / ((s * self.c1).inv() + zload);
        va * (s * self.lp.c).inv() / z2
    }
    fn circuit(&self, f_max: f64) -> Circuit {
        netlist(
            &["in", "a", "b", "out"],
            vec![
                vin(),
                json!({"id": "c1", "type": "capacitor", "nodes": ["in", "a"], "C_F": self.c1}),
                json!({"id": "r1", "type": "resistor", "node": "a", "R_ohm": self.r1}),
                json!({"id": "r3", "type": "resistor", "nodes": ["a", "b"], "R_ohm": self.lp.r}),
                json!({"id": "l", "type": "inductor", "nodes": ["b", "out"], "L_H": self.lp.l}),
                json!({"id": "c3", "type": "capacitor", "node": "out", "C_F": self.lp.c}),
            ],
            probe_out("out"),
            f_max,
        )
    }
}

// ----- FFT and eigenvalues against numpy -----------------------------------------

/// Radix-2 FFT against numpy's pocketfft on the fixture's deterministic
/// sequence. Tolerance 1e-12 of the largest output: both transforms err by
/// about ε·log2 N (observed ~1e-15).
#[test]
fn fft_matches_numpy() {
    for case in fixture("time_numerics.json")["fft"].as_array().unwrap() {
        let n = case["n"].as_u64().unwrap() as usize;
        let x: Vec<C64> = (0..n)
            .map(|k| {
                let k = k as f64;
                C64::new(
                    (0.001 * k * k + 0.3 * k).cos(),
                    (0.7 * k).sin() * (-k / 3000.0).exp(),
                )
            })
            .collect();
        let mut y = x.clone();
        let fft = Fft::new(n);
        fft.forward(&mut y);
        let bins: Vec<usize> = case["bins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as usize)
            .collect();
        let (re, im) = (f64s(&case["re"]), f64s(&case["im"]));
        let scale = re
            .iter()
            .zip(&im)
            .map(|(a, b)| a.hypot(*b))
            .fold(0.0, f64::max);
        let err = bins
            .iter()
            .enumerate()
            .map(|(i, &b)| (y[b] - C64::new(re[i], im[i])).norm())
            .fold(0.0, f64::max);
        assert!(err <= 1e-12 * scale, "N = {n}: {err:e} of {scale}");
        fft.inverse(&mut y);
        let back = y
            .iter()
            .zip(&x)
            .map(|(a, b)| (a - b).norm())
            .fold(0.0, f64::max);
        assert!(back <= 1e-13, "N = {n}: round trip {back:e}");
    }
}

/// Eigenvalues (balancing, Hessenberg, Francis QR) against LAPACK's dgeev.
/// Tolerance 1e-9 of the spectral radius: the vector-fitting matrix is
/// non-normal, so eigenvalues are only as accurate as ε·‖A‖ times their
/// condition numbers (observed agreement ~1e-12).
#[test]
fn eigenvalues_match_numpy() {
    for case in fixture("time_numerics.json")["eig"].as_array().unwrap() {
        let n = case["n"].as_u64().unwrap() as usize;
        let mut got = eigenvalues(f64s(&case["a"]), n).unwrap();
        let want: Vec<C64> = f64s(&case["re"])
            .into_iter()
            .zip(f64s(&case["im"]))
            .map(|(r, i)| C64::new(r, i))
            .collect();
        let radius = want.iter().map(|z| z.norm()).fold(0.0, f64::max);
        // Match each reference eigenvalue to the nearest computed one.
        for w in &want {
            let (i, d) = got
                .iter()
                .enumerate()
                .map(|(i, z)| (i, (z - w).norm()))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert!(d <= 1e-9 * radius, "n = {n}: {w} off by {d:e}");
            got.remove(i);
        }
    }
}

// ----- Uniform re-solve and impulse responses -----------------------------------

fn options(n: usize) -> UniformOptions {
    UniformOptions {
        n,
        ..Default::default()
    }
}

/// The uniform grid is re-solved, not interpolated: every solved bin equals
/// a direct solve at that frequency, bit for bit.
#[test]
fn uniform_bins_are_exact_solves() {
    let c = Hp::corner(100.0).circuit(40_000.0);
    let u = uniform_response(&c, "out", &options(4096)).unwrap();
    assert_eq!(u.solved, (1, 2048));
    let p = &c.probes[0];
    for k in [1usize, 2, 7, 100, 1000, 2047] {
        let f = u.freq(k);
        let x = c.solve_at(f).unwrap();
        assert_eq!(u.values[k], c.probe_value(p, f, &x).unwrap(), "bin {k}");
    }
    assert_eq!(u.values[0].im, 0.0);
    assert_eq!(u.values[2048].im, 0.0);
}

/// The IR of the uniform re-solve against numpy's irfft of the analytic
/// spectrum, built by the same rules (solved to Nyquist; DC from the
/// low-frequency probe). This checks the solver, the DC rule, Hermitian
/// assembly and the transform together. Tolerance 1e-12 of max|h|: the
/// network solve is exact to ~1e-15, the transforms to ~1e-15.
#[test]
fn impulse_matches_band_limited_reference() {
    let refs = fixture("time_ir.json");
    for case in refs["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let p = &case["params"];
        let c = match name {
            "hp" => Hp {
                r: p["R_ohm"].as_f64().unwrap(),
                c: p["C_F"].as_f64().unwrap(),
            }
            .circuit(48_000.0),
            _ => Lp {
                r: p["R_ohm"].as_f64().unwrap(),
                l: p["L_H"].as_f64().unwrap(),
                c: p["C_F"].as_f64().unwrap(),
            }
            .circuit(48_000.0),
        };
        let n = case["n"].as_u64().unwrap() as usize;
        let u = uniform_response(&c, "out", &options(n)).unwrap();
        assert!(u.extrapolation.taper_hz.is_none());
        let dc = case["dc"].as_f64().unwrap();
        assert!(
            (u.values[0].re - dc).abs() <= 1e-12 * dc.abs().max(1.0),
            "{name} DC"
        );
        let h = u.impulse();
        let want = f64s(&case["h"]);
        let peak = want.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let idx = case["indices"].as_array().unwrap();
        let err = idx
            .iter()
            .zip(&want)
            .map(|(i, w)| (h[i.as_u64().unwrap() as usize] - w).abs())
            .fold(0.0, f64::max);
        assert!(err <= 1e-12 * peak, "{name}: {err:e} of {peak}");
        // Parseval: Σ h² = (1/N) Σ |H_k|² over the full Hermitian spectrum.
        let full = hermitian_full(&u.values);
        let e_freq = full.iter().map(|v| v.norm_sqr()).sum::<f64>() / n as f64;
        let e_time: f64 = h.iter().map(|v| v * v).sum();
        assert!((e_time - e_freq).abs() <= 1e-12 * e_freq, "{name} Parseval");
        assert!((e_time - case["energy"].as_f64().unwrap()).abs() <= 1e-10 * e_time);
    }
}

/// The resonator's IR against its continuous analytic response
/// h(t) = Σ rᵢ·e^{pᵢt}, time-aliased with period N/fs and sampled as
/// h(n/fs)/fs. The two differ only by the spectrum outside the band
/// (and the Nyquist and DC rules); the fixture's bound is the sum of those
/// terms, which the difference may not exceed (it reaches it at t = 0,
/// where every out-of-band term has the same sign).
#[test]
fn impulse_matches_continuous_response_within_band_limit() {
    let refs = fixture("time_ir.json");
    let case = &refs["cases"][1];
    let p = &case["params"];
    let lp = Lp {
        r: p["R_ohm"].as_f64().unwrap(),
        l: p["L_H"].as_f64().unwrap(),
        c: p["C_F"].as_f64().unwrap(),
    };
    let u = uniform_response(&lp.circuit(48_000.0), "out", &options(8192)).unwrap();
    let h = u.impulse();
    let bound = case["bound"].as_f64().unwrap();
    let hc = f64s(&case["h_continuous"]);
    let idx = case["indices"].as_array().unwrap();
    let peak = hc.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let mut worst = 0.0f64;
    for (i, w) in idx.iter().zip(&hc) {
        let d = (h[i.as_u64().unwrap() as usize] - w).abs();
        worst = worst.max(d);
    }
    assert!(
        worst <= bound * (1.0 + 1e-9) + 1e-15,
        "{worst:e} > {bound:e}"
    );
    // Away from t = 0 the agreement is much closer (the out-of-band terms
    // oscillate and cancel): 1e-4 of the peak beyond 1 ms.
    for (i, w) in idx.iter().zip(&hc) {
        let n = i.as_u64().unwrap() as usize;
        if n >= 48 {
            assert!((h[n] - w).abs() <= 1e-4 * peak, "sample {n}");
        }
    }
}

/// Step response and energy-time curve: the step settles at H(0) (1 for
/// the low-pass, 0 for the high-pass) and the ETC is 0 dB at its maximum,
/// near the IR peak.
#[test]
fn step_and_energy_time_curve() {
    let lp = Lp::new(1000.0, 5.0, 1e-6);
    let u = uniform_response(&lp.circuit(48_000.0), "out", &options(8192)).unwrap();
    let imp = impulse(&u.values, u.fs_hz, 256);
    assert_eq!(imp.t0_s, -256.0 / 48_000.0);
    let last = *imp.step.last().unwrap();
    assert!((last - 1.0).abs() < 1e-9, "{last}");
    let emax = imp.etc_db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    assert_eq!(emax, 0.0);
    // Continuous peak of (ω0²/ω_d)·e^{−σt}·sin(ω_d t) at tan(ω_d t) = ω_d/σ.
    let w0 = 2.0 * PI * 1000.0;
    let sig = w0 / 10.0;
    let wd = (w0 * w0 - sig * sig).sqrt();
    let t_peak = (wd / sig).atan() / wd;
    assert!(
        (imp.peak_s - t_peak).abs() <= 1.0 / 48_000.0,
        "{} vs {t_peak}",
        imp.peak_s
    );
    let hp = Hp::corner(100.0);
    let u = uniform_response(&hp.circuit(48_000.0), "out", &options(8192)).unwrap();
    let imp = impulse(&u.values, u.fs_hz, 256);
    assert!(imp.step.last().unwrap().abs() < 1e-12);
}

/// DC from the model's own low-frequency asymptote: a high-pass corner at
/// 0.2 Hz, far below the 5.86 Hz bins, still gives H(0) = 0 and order 1.
/// The probe's slope and the corner estimate f_a·|H(f₁)|/|H(f_a)| are
/// checked against the closed form to 1e-9, and the corner lies within
/// 0.1 % of 0.2 Hz (|H| departs from f/f_c by (f_a/f_c)²/2 ≈ 4e-4 at the
/// probe).
#[test]
fn dc_rule_finds_a_corner_below_the_bins() {
    let hp = Hp::corner(0.2);
    let c = hp.circuit(20_000.0);
    let u = uniform_response(&c, "out", &UniformOptions::default()).unwrap();
    let e = &u.extrapolation;
    assert_eq!(e.dc_rule, "zero");
    assert_eq!(e.dc_order, 1);
    let fa = e.dc_probe_hz.unwrap();
    assert_eq!(fa, 48_000.0 / 8192.0 / 1024.0);
    let slope = (hp.h(jw(2.0 * fa)).norm() / hp.h(jw(fa)).norm()).log2();
    assert!(
        (e.dc_slope - slope).abs() < 1e-9,
        "{} vs {slope}",
        e.dc_slope
    );
    let fc = e.dc_corner_hz.unwrap();
    let want = fa * hp.h(jw(u.freq(1))).norm() / hp.h(jw(fa)).norm();
    assert!((fc / want - 1.0).abs() < 1e-9, "{fc} vs {want}");
    assert!((fc / 0.2 - 1.0).abs() < 1e-3, "{fc}");
    assert_eq!(u.values[0], C64::new(0.0, 0.0));
    // A flat response keeps its DC value.
    let lp = Lp::new(1000.0, 5.0, 1e-6);
    let u = uniform_response(&lp.circuit(20_000.0), "out", &UniformOptions::default()).unwrap();
    assert_eq!(u.extrapolation.dc_rule, "extrapolated");
    assert!((u.values[0].re - 1.0).abs() < 1e-9);
}

/// Above the solved band: the power law of the top 1/6 octave times the
/// half-cosine taper, continuous at the band edge and zero at Nyquist.
#[test]
fn high_frequency_extrapolation_and_taper() {
    let lp = Lp::new(1000.0, 5.0, 1e-6);
    let u = uniform_response(&lp.circuit(20_000.0), "out", &UniformOptions::default()).unwrap();
    let (k_lo, k_hi) = u.solved;
    assert_eq!(k_lo, 1);
    assert_eq!(k_hi, 3413);
    let e = &u.extrapolation;
    assert!((e.hf_slope + 2.0).abs() < 0.02, "{}", e.hf_slope);
    assert_eq!(u.values[4096], C64::new(0.0, 0.0));
    let f_hi = u.freq(k_hi);
    for k in [k_hi + 1, k_hi + 200, 4000] {
        let f = u.freq(k);
        let w = 0.5 * (1.0 + (PI * (f - f_hi) / (24_000.0 - f_hi)).cos());
        let want = u.values[k_hi].norm() * (f / f_hi).powf(e.hf_slope) * w;
        assert!((u.values[k].norm() / want - 1.0).abs() < 1e-12, "bin {k}");
    }
    // Continuity at the edge: one bin further changes |H| by the slope only.
    let r = u.values[k_hi + 1].norm() / u.values[k_hi].norm();
    assert!((r - 1.0).abs() < 1e-3);
}

// ----- Causality and the impulse length (E46) -------------------------------

/// Erratum E46 on a 100 Hz, Q = 20 resonator. The late (negative-time)
/// energy agrees with numpy to 0.05 dB (both are exact computations of the
/// same quantity); 65536 points (N/(2·fs) = 0.68 s ≥ 2.93·Q/f = 0.586 s)
/// reach −80 dB, 16384 do not. The vector fit finds the pole and
/// recommends 65536, beyond the 16384-point audition limit.
#[test]
fn impulse_length_rule_e46() {
    let refs = fixture("time_ir.json");
    let e46 = &refs["e46"];
    let p = &e46["params"];
    let lp = Lp {
        r: p["R_ohm"].as_f64().unwrap(),
        l: p["L_H"].as_f64().unwrap(),
        c: p["C_F"].as_f64().unwrap(),
    };
    let c = lp.circuit(48_000.0);
    for row in e46["pow2"].as_array().unwrap() {
        let n = row["n"].as_u64().unwrap() as usize;
        if n < 16384 {
            continue;
        }
        let u = uniform_response(&c, "out", &options(n)).unwrap();
        let late = impulse(&u.values, u.fs_hz, 0).late_energy_db;
        let want = row["late_energy_dB"].as_f64().unwrap();
        assert!((late - want).abs() < 0.05, "N = {n}: {late} vs {want}");
        assert_eq!(late <= -80.0, n >= 65536, "N = {n}: {late}");
    }
    // At a causal half of exactly 2.9·Q/f the late energy is −79.1 dB: E46's
    // factor is the rounding of ln(10⁴)/π = 2.93 (80 dB envelope decay).
    let study = e46["study"].as_array().unwrap();
    let at = |label: &str| {
        study.iter().find(|r| r["label"] == label).unwrap()["late_energy_dB"]
            .as_f64()
            .unwrap()
    };
    assert!((at("E46") + 79.1).abs() < 0.1);
    assert!(at("T60") < -40.0 && at("T60") > -61.0);
    let fit = fit_probe(&c, "out", &PoleFitOptions::default()).unwrap();
    let chk = ir_length_check(&fit, 48_000.0, 8192);
    let b = chk.binding.as_ref().unwrap();
    assert!((b.f_hz / 100.0 - 1.0).abs() < 1e-6, "{}", b.f_hz);
    assert!((b.q / 20.0 - 1.0).abs() < 1e-6, "{}", b.q);
    assert!(!chk.covered);
    assert_eq!(chk.recommended_n, 65536);
    assert!(!chk.within_audition_range);
}

/// A minimum-phase system is causal (spec Section 17: energy before t = 0
/// below −80 dB). The mixed-phase IR of the uniform re-solve meets that
/// when the buffer covers E46 and the response is negligible in the taper
/// band: a 100 Hz, Q = 5 resonator (−92 dB at 20 kHz) at N = 16384
/// (N/(2·fs) = 0.17 s ≥ 2.93·Q/f = 0.147 s). A response still strong at the
/// band edge cannot meet it in mixed phase, because the zero-phase taper
/// band-limits it symmetrically: the high-pass, flat to 20 kHz, keeps a
/// band-limited impulse at t = 0 with energy at negative times near the
/// band-edge share of its spectrum. The minimum-phase filter is causal in
/// both cases (below −80 dB; `tools/time/minphase_study.py` gives
/// −89 dB for the high-pass).
#[test]
fn minimum_phase_system_is_causal() {
    let lp = Lp::new(100.0, 5.0, 1e-5);
    let req = ImpulseRequest {
        uniform: options(16384),
        ..Default::default()
    };
    let r = impulses(&lp.circuit(20_000.0), "out", &req).unwrap();
    assert!(r.ir_length.as_ref().unwrap().covered);
    assert!(r.mixed.late_energy_db < -80.0, "{}", r.mixed.late_energy_db);
    assert!(
        r.minimum.late_energy_db < -80.0,
        "{}",
        r.minimum.late_energy_db
    );
    let hp = Hp::corner(100.0);
    let r = impulses(&hp.circuit(20_000.0), "out", &ImpulseRequest::default()).unwrap();
    let edge = r.band_edge_energy_db();
    assert!(r.mixed.late_energy_db > -30.0, "{}", r.mixed.late_energy_db);
    assert!(
        r.mixed.late_energy_db < edge + 3.0,
        "{} vs {edge}",
        r.mixed.late_energy_db
    );
    assert!(
        r.minimum.late_energy_db < -80.0,
        "{}",
        r.minimum.late_energy_db
    );
}

// ----- Minimum phase and excess phase -------------------------------------------

/// Largest |arg H_min − arg H_analytic| over bins in [f0, f1], degrees.
fn phase_error(
    u: &acoustilab::time::UniformResponse,
    h_min: &[C64],
    h: impl Fn(C64) -> C64,
    f0: f64,
    f1: f64,
) -> f64 {
    (1..u.values.len())
        .filter(|&k| u.freq(k) >= f0 && u.freq(k) <= f1)
        .map(|k| {
            wrap(h_min[k].arg() - h(jw(u.freq(k))).arg())
                .to_degrees()
                .abs()
        })
        .fold(0.0, f64::max)
}

/// The cepstral minimum phase of known minimum-phase networks against
/// their analytic phase, over 20 Hz–20 kHz with the band solved to 20 kHz
/// and tapered above. Tolerances: the method's error, measured
/// independently in numpy on the same spectra (`tools/time/minphase_study.py`):
/// flat-topped responses (high-pass, notch) 0.002°, allowed 0.01°; a
/// response still falling 12 dB/octave at 20 kHz 0.83° at 19 kHz and
/// 0.042° at 1 kHz with the default 8× extension, allowed 1° and 0.05°.
#[test]
fn minimum_phase_matches_analytic_phase() {
    let opts = MinPhaseOptions::default();
    let hp = Hp::corner(100.0);
    let u = uniform_response(&hp.circuit(20_000.0), "out", &UniformOptions::default()).unwrap();
    let mp = min_phase(&u, &opts);
    assert_eq!((mp.dc_order, mp.polarity), (1, 1.0));
    let e = phase_error(&u, &mp.values, |s| hp.h(s), 20.0, 20_000.0);
    assert!(e < 0.01, "high-pass: {e}");

    let notch = Notch::new(200.0, 20.0, 2.0);
    let u = uniform_response(&notch.circuit(20_000.0), "out", &UniformOptions::default()).unwrap();
    let mp = min_phase(&u, &opts);
    let e = phase_error(&u, &mp.values, |s| notch.h(s), 20.0, 20_000.0);
    assert!(e < 0.01, "notch: {e}");

    let lp = Lp::new(1000.0, 5.0, 1e-6);
    let u = uniform_response(&lp.circuit(20_000.0), "out", &UniformOptions::default()).unwrap();
    let mp = min_phase(&u, &opts);
    let e = phase_error(&u, &mp.values, |s| lp.h(s), 20.0, 20_000.0);
    assert!(e < 1.0, "low-pass: {e}");
    let e = phase_error(&u, &mp.values, |s| lp.h(s), 20.0, 1_000.0);
    assert!(e < 0.05, "low-pass to 1 kHz: {e}");

    // The corner at 0.2 Hz is invisible on the bins; dividing out the
    // probed DC asymptote keeps the lowest bins right (numpy: 0.0008° at
    // 6 Hz). Above, the ladder falls 12 dB/octave at 20 kHz like the
    // low-pass and shares its extension error (0.05° at 1 kHz, 1° at
    // 19 kHz).
    let leaky = Leaky::new();
    let u = uniform_response(&leaky.circuit(20_000.0), "out", &UniformOptions::default()).unwrap();
    let mp = min_phase(&u, &opts);
    let e = phase_error(&u, &mp.values, |s| leaky.h(s), 5.0, 200.0);
    assert!(e < 0.01, "leaky ladder to 200 Hz: {e}");
    let e = phase_error(&u, &mp.values, |s| leaky.h(s), 5.0, 1_000.0);
    assert!(e < 0.06, "leaky ladder to 1 kHz: {e}");
    let e = phase_error(&u, &mp.values, |s| leaky.h(s), 20.0, 20_000.0);
    assert!(e < 1.2, "leaky ladder: {e}");
}

/// A non-minimum-phase response: the notch times the all-pass
/// A = (a − s)/(a + s), a = 2π·500 rad/s. Its excess phase must be the
/// all-pass phase −2·atan(ω/a) and its excess group delay 2a/(a² + ω²).
/// Tolerances: the notch's own minimum-phase error (0.002°, allowed 0.01°)
/// and the central-difference group delay (second-order error
/// (Δω)²·τ‴/6 ≈ 1e-4 relative at 20 Hz; allowed 1e-3).
#[test]
fn excess_phase_of_an_all_pass_factor() {
    let notch = Notch::new(200.0, 20.0, 2.0);
    let a = 2.0 * PI * 500.0;
    let h = |s: C64| notch.h(s) * (a - s) / (a + s);
    let n = 8192;
    let df = 48_000.0 / n as f64;
    let mut half: Vec<C64> = (0..=n / 2).map(|k| h(jw(k as f64 * df))).collect();
    half[0] = C64::new(half[0].re, 0.0);
    half[n / 2] = C64::new(half[n / 2].re, 0.0);
    let mp = min_phase_spectrum(&half, (1, n / 2), df, None, &MinPhaseOptions::default());
    assert_eq!(mp.polarity, 1.0);
    let ex = excess_phase(&half, &mp.values, df, 1);
    for k in 4..=3413 {
        let w = 2.0 * PI * k as f64 * df;
        let want = -2.0 * (w / a).atan();
        let err = (ex.phase_rad[k] - want).to_degrees().abs();
        assert!(err < 0.01, "bin {k}: {err}");
        let gd = 2.0 * a / (a * a + w * w);
        assert!((ex.group_delay_s[k] / gd - 1.0).abs() < 1e-3, "bin {k}");
    }
}

/// Excess group delay decides the phase mode (Section 16). An all-pass
/// lattice with RC = 0.5 ms (group delay 1 ms at DC, read differentially
/// across a zero-current source) fails the 0.5 ms test: mixed phase. The
/// resonator passes: minimum phase, with no pure delay.
#[test]
fn phase_decision_of_networks() {
    let (r, c) = (1000.0, 0.5e-6);
    let lattice = netlist(
        &["in", "a", "b"],
        vec![
            vin(),
            json!({"id": "ra", "type": "resistor", "nodes": ["in", "a"], "R_ohm": r}),
            json!({"id": "ca", "type": "capacitor", "node": "a", "C_F": c}),
            json!({"id": "cb", "type": "capacitor", "nodes": ["in", "b"], "C_F": c}),
            json!({"id": "rb", "type": "resistor", "node": "b", "R_ohm": r}),
            json!({"id": "diff", "type": "isource", "nodes": ["a", "b"], "I_A": 0}),
        ],
        json!({"id": "out", "quantity": "port_potential", "element": "diff"}),
        20_000.0,
    );
    let rep = impulses(&lattice, "out", &ImpulseRequest::default()).unwrap();
    let d = &rep.decision;
    assert_eq!(d.mode, "mixed");
    assert!(
        (d.max_excess_gd_s / 1e-3 - 1.0).abs() < 1e-3,
        "{}",
        d.max_excess_gd_s
    );
    assert!(d.pure_delay_s < 1e-6);
    let lp = Lp::new(1000.0, 5.0, 1e-6);
    let rep = impulses(&lp.circuit(20_000.0), "out", &ImpulseRequest::default()).unwrap();
    assert_eq!(rep.decision.mode, "minimum");
    assert!(
        rep.decision.max_excess_gd_s < 2e-6,
        "{}",
        rep.decision.max_excess_gd_s
    );
}

/// The design template: minimum phase passes the test in its trusted band
/// (105 Hz to 1.06 kHz, set by the IEC 60318-4 lower limit and the rear
/// cavity's lumped limit). The excess group delay is a near-constant
/// propagation delay of about 95 µs (front-cavity depth line and ear
/// simulator); its value depends on the magnitude assumed above 20 kHz by
/// tens of µs (docs/time-domain.md), hence the wide window.
#[test]
fn design_template_phase_decision() {
    let text = example("design_over_ear.json");
    let c = Circuit::from_json(&text).unwrap();
    let u = uniform_response(&c, "p_drp", &UniformOptions::default()).unwrap();
    let mp = min_phase(&u, &MinPhaseOptions::default());
    assert_eq!(mp.polarity, 1.0);
    let ex = excess_phase(&u.values, &mp.values, u.df(), u.solved.0);
    let d = phase_decision(&u.values, &ex, &u.trusted(), u.df(), mp.polarity, 0.5e-3);
    assert_eq!(d.mode, "minimum");
    let (lo, hi) = d.band_hz.unwrap();
    assert!(
        lo > 100.0 && lo < 110.0 && hi > 1000.0 && hi < 1100.0,
        "{lo} {hi}"
    );
    assert!(
        d.max_excess_gd_s > 50e-6 && d.max_excess_gd_s < 150e-6,
        "{}",
        d.max_excess_gd_s
    );
}

// ----- Group delay ----------------------------------------------------------------

/// Group delay three ways on the resonator: the rational fit (exact
/// derivative of the model), the dense uniform re-solve (exact derivative
/// of its trigonometric interpolant) and the closed form. The fit
/// recovers the second-order function, so it agrees to 1e-6 relative; the
/// dense estimate is limited by the band-limited, time-aliased IR (the
/// response is −55 dB at Nyquist) and agrees to 1e-3 of the peak delay
/// below 10 kHz.
#[test]
fn group_delay_rational_dense_and_closed_form_agree() {
    let lp = Lp::new(1000.0, 5.0, 1e-6);
    let c = lp.circuit(48_000.0);
    let opts = PoleFitOptions {
        vf: VfOptions {
            order: 2,
            asymptote: Asymptote::Zero,
            ..Default::default()
        },
        ..Default::default()
    };
    let fit = fit_probe(&c, "out", &opts).unwrap();
    let u = uniform_response(&c, "out", &options(8192)).unwrap();
    let dense = group_delay_dense(&u.values, u.fs_hz);
    let peak = lp.group_delay(1000.0);
    for k in (1..1707).step_by(7) {
        let f = u.freq(k);
        let exact = lp.group_delay(f);
        let rat = fit.group_delay(f);
        assert!(
            (rat - exact).abs() <= 1e-6 * exact.abs().max(1e-9),
            "rational at {f}: {rat} vs {exact}"
        );
        assert!(
            (dense[k] - exact).abs() <= 1e-3 * peak,
            "dense at {f}: {} vs {exact}",
            dense[k]
        );
    }
}

/// On the design template (irrational elements, IEC ear) the fit's group
/// delay agrees with a central difference of exact solves at f(1 ± 1e-5)
/// (truncation error ~1e-10 relative). Tolerance: 2 µs from 20 Hz to
/// 10 kHz, where the fit's phase error is below 0.001°
/// (0.001° / (360°·Δf) with Δf the local scale of phase change).
#[test]
fn design_group_delay_from_fit_matches_exact_derivative() {
    let text = example("design_over_ear.json");
    let c = Circuit::from_json(&text).unwrap();
    let fit = fit_probe(&c, "p_drp", &PoleFitOptions::default()).unwrap();
    let p = c.probes.iter().find(|p| p.id == "p_drp").unwrap();
    let at = |f: f64| {
        let x = c.solve_at(f).unwrap();
        c.probe_value(p, f, &x).unwrap()
    };
    for f in [
        20.0, 50.0, 100.0, 300.0, 935.0, 1000.0, 3000.0, 7000.0, 10_000.0,
    ] {
        let h = 1e-5 * f;
        let dphi = (at(f + h) / at(f - h)).arg();
        let exact = -dphi / (2.0 * PI * 2.0 * h);
        let rat = fit.group_delay(f);
        assert!((rat - exact).abs() < 2e-6, "{f} Hz: {rat} vs {exact}");
    }
}

// ----- Vector fitting ----------------------------------------------------------

fn known_model() -> RationalModel {
    let mut poles = Vec::new();
    let mut res = Vec::new();
    for i in 0..8 {
        let f = 30.0 * (500.0f64).powf(i as f64 / 7.0);
        let q = 0.8 + 2.5 * i as f64;
        let w = 2.0 * PI * f;
        let re = -w / (2.0 * q);
        poles.push(Pole::Pair(C64::new(re, (w * w - re * re).sqrt())));
        res.push(C64::new(
            w * (1.0 + 0.3 * i as f64),
            -w * (0.5 - 0.1 * i as f64),
        ));
    }
    for (i, f) in [15.0, 800.0, 12_000.0].iter().enumerate() {
        let w = 2.0 * PI * f;
        poles.push(Pole::Real(-w));
        res.push(C64::new(w * (0.7 - 0.4 * i as f64), 0.0));
    }
    RationalModel {
        poles,
        residues: vec![res],
        d: vec![0.25],
        e: vec![3e-6],
    }
}

/// Exact recovery of a known rational function of order 19 (8 pairs with
/// Q from 0.8 to 18, 3 real poles, d and s·e) sampled on a 24-per-octave
/// grid from 10 Hz to 20 kHz: poles and residues to 1e-9 relative, the
/// accuracy the brief asks for (observed ~1e-12).
#[test]
fn vector_fit_recovers_known_rational_function() {
    let truth = known_model();
    let f = acoustilab::grid::log_grid(10.0, 20_000.0, 24.0);
    let h: Vec<C64> = f.iter().map(|&f| truth.eval(0, jw(f))).collect();
    let (m, rep) = vector_fit(
        &f,
        &[h],
        &VfOptions {
            order: 19,
            iterations: 10,
            asymptote: Asymptote::Linear,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(rep.error.rms_relative < 1e-12, "{:?}", rep.error);
    for (p, c) in truth.poles.iter().zip(&truth.residues[0]) {
        let (i, d) = m
            .poles
            .iter()
            .enumerate()
            .map(|(i, q)| (i, (q.value() - p.value()).norm()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(d <= 1e-9 * p.value().norm(), "pole {:?} off by {d:e}", p);
        assert_eq!(p.width(), m.poles[i].width());
        let rc = m.residues[0][i];
        assert!((rc - c).norm() <= 1e-9 * c.norm(), "residue {c} vs {rc}");
    }
    assert!((m.d[0] - 0.25).abs() < 1e-9 * 0.25);
    assert!((m.e[0] - 3e-6).abs() < 1e-9 * 3e-6);
    // The state-space realisation evaluates to the same function.
    let ss = m.state_space(0);
    for f in [20.0, 1000.0, 15_000.0] {
        let s = jw(f);
        let n = ss.b.len();
        // Solve (sI − A)x = B by the block structure: evaluate per block.
        let mut y = C64::new(ss.d, 0.0) + s * ss.e;
        let mut i = 0;
        while i < n {
            if i + 1 < n && ss.a[i][i + 1] != 0.0 {
                let (ar, ai) = (ss.a[i][i], ss.a[i][i + 1]);
                let det = (s - ar) * (s - ar) + ai * ai;
                let x0 = ((s - ar) * ss.b[i] + ai * ss.b[i + 1]) / det;
                let x1 = (-ai * ss.b[i] + (s - ar) * ss.b[i + 1]) / det;
                y += ss.c[i] * x0 + ss.c[i + 1] * x1;
                i += 2;
            } else {
                y += ss.c[i] * ss.b[i] / (s - ss.a[i][i]);
                i += 1;
            }
        }
        assert!((y / m.eval(0, s) - 1.0).norm() < 1e-12);
    }
}

/// Zeros of a fitted model against the closed form: the resonator times
/// (s² + ω_z s/Q_z + ω_z²) has exactly those two zeros (D ≠ 0 path), and
/// the strictly proper high-pass s·ω/(s² + …) its zero at DC (D = 0 path).
#[test]
fn vector_fit_zeros() {
    let wz = 2.0 * PI * 3000.0;
    let lp = Lp::new(1000.0, 5.0, 1e-6);
    let f = acoustilab::grid::log_grid(10.0, 40_000.0, 24.0);
    let h: Vec<C64> = f
        .iter()
        .map(|&f| {
            let s = jw(f);
            lp.h(s) * (s * s + s * wz / 4.0 + wz * wz) / (wz * wz)
        })
        .collect();
    let (m, _) = vector_fit(
        &f,
        std::slice::from_ref(&h),
        &VfOptions {
            order: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let peak = h.iter().map(|v| v.norm()).fold(0.0, f64::max);
    let z = m.zeros(0, peak).unwrap();
    let want = C64::new(-wz / 8.0, wz * (1.0 - 1.0 / 64.0f64).sqrt());
    assert!(z.iter().any(|z| (z - want).norm() < 1e-8 * wz), "{z:?}");
    let w0 = 2.0 * PI * 500.0;
    let h: Vec<C64> = f
        .iter()
        .map(|&f| {
            let s = jw(f);
            s * w0 / (s * s + s * w0 / 3.0 + w0 * w0)
        })
        .collect();
    let (m, _) = vector_fit(
        &f,
        std::slice::from_ref(&h),
        &VfOptions {
            order: 2,
            asymptote: Asymptote::Zero,
            ..Default::default()
        },
    )
    .unwrap();
    let z = m.zeros(0, 1.0).unwrap();
    assert_eq!(z.len(), 1);
    assert!(z[0].norm() < 1e-6 * w0, "{z:?}");
}

/// A driver in a vented cup (`examples/closed_back_rear_vent.json`: the
/// rear Helmholtz vent with its mesh, thermoviscous ducts, a slit leak) and
/// the design template with the IEC 60318-4 ear, both at L1: the brief's
/// target is 0.1 dB and 1° from 20 Hz to 20 kHz. Order 30 reaches it with
/// two orders of magnitude to spare (docs/time-domain.md lists the error
/// against order); the thermoviscous elements are irrational but smooth.
#[test]
fn vector_fit_of_physical_networks() {
    for (file, probe) in [
        ("closed_back_rear_vent.json", "p_ear"),
        ("design_over_ear.json", "p_drp"),
        ("design_over_ear.json", "zin"),
    ] {
        let c = Circuit::from_json(&example(file)).unwrap();
        let opts = PoleFitOptions {
            f_min_hz: Some(20.0),
            f_max_hz: Some(20_000.0),
            ..Default::default()
        };
        let fit = fit_probe(&c, probe, &opts).unwrap();
        let e = &fit.report.error;
        assert!(e.max_db < 0.01 && e.max_deg < 0.1, "{file} {probe}: {e:?}");
    }
    // The Type 4.3 ear (a 48-segment canal) as well.
    let p = Parametric::parse(&example("design_over_ear.json")).unwrap();
    let mut ov = Overrides::new();
    ov.insert("ear".into(), acoustilab::expr::PValue::Str("type43".into()));
    let c = Circuit::from_parametric(&p, &ov).unwrap();
    let fit = fit_probe(&c, "p_drp", &PoleFitOptions::default()).unwrap();
    let e = &fit.report.error;
    assert!(e.max_db < 0.1 && e.max_deg < 1.0, "type43: {e:?}");
}

/// Attribution by re-fits on a parametric LC resonator, whose pole
/// frequency is (LC)^(−1/2)/(2π)·sqrt(1 − 1/(4Q²)) and Q = sqrt(L/C)/R:
/// d ln f/d ln L = d ln f/d ln C = −1/2 up to the damping term
/// (−1/2 ∓ 1/(2·(4Q² − 1)) at Q = 10: 0.0013), d ln f/d ln R ≈ 0, and
/// d ln Q/d ln R = −1. One-sided 1 % steps: tolerance 0.01.
#[test]
fn attribution_names_the_parameters_that_set_a_resonance() {
    let text = json!({
        "schema": "acoustilab-netlist/0.2",
        "parameters": {
            "L_mH": {"value": 25.33, "min": 1, "max": 100},
            "C_uF": {"value": 1.0, "min": 0.1, "max": 10},
            "R_ohm": {"value": 15.915, "min": 1, "max": 100}
        },
        "sweep": {"f_min_Hz": 20, "f_max_Hz": 20000, "points_per_octave": 24},
        "nodes": [{"id": "in", "domain": "electrical"}, {"id": "a", "domain": "electrical"},
                  {"id": "out", "domain": "electrical"}],
        "elements": [
            {"id": "v", "type": "vsource", "node": "in"},
            {"id": "r", "type": "resistor", "nodes": ["in", "a"], "R_ohm": "=R_ohm"},
            {"id": "l", "type": "inductor", "nodes": ["a", "out"], "L_mH": "=L_mH"},
            {"id": "c", "type": "capacitor", "node": "out", "C_uF": "=C_uF"}
        ],
        "probes": [{"id": "out", "quantity": "voltage", "node": "out"}]
    })
    .to_string();
    let p = Parametric::parse(&text).unwrap();
    let ov = Overrides::new();
    let c = Circuit::from_parametric(&p, &ov).unwrap();
    let opts = PoleFitOptions {
        vf: VfOptions {
            order: 2,
            asymptote: Asymptote::Zero,
            ..Default::default()
        },
        ..Default::default()
    };
    let fit = fit_probe(&c, "out", &opts).unwrap();
    assert_eq!(fit.resonant().len(), 1);
    let a = attribute_poles(&p, &ov, &fit, &opts, &SensitivityOptions::default()).unwrap();
    let pa = &a.poles[0];
    assert!(pa.fixed_elements.is_empty());
    let s = |name: &str| pa.parameters.iter().find(|q| q.parameter == name).unwrap();
    assert!((s("L_mH").dlnf_dlnp + 0.5).abs() < 0.01, "{:?}", s("L_mH"));
    assert!((s("C_uF").dlnf_dlnp + 0.5).abs() < 0.01, "{:?}", s("C_uF"));
    assert_eq!(s("L_mH").elements, vec!["l".to_string()]);
    assert!(
        (s("R_ohm").dlnq_dlnp + 1.0).abs() < 0.01,
        "{:?}",
        s("R_ohm")
    );
    assert!(s("R_ohm").dlnf_dlnp.abs() < 0.01);
}

/// The design template's resonances: the coupled resonance near 936 Hz is
/// set by the diaphragm area (d ln f/d ln Sd ≈ +1, the air springs scale
/// as Sd²), its mass (−1/2) and the cup radius (the front air spring, about
/// 45 % of the total stiffness with the rear cavity: −0.45); the 11.6 kHz
/// pole by the front depth (a depth half-wave: −1); a 7.1 kHz resonance
/// that no parameter moves belongs to the parameter-free ear simulator.
#[test]
fn design_template_attribution() {
    let text = example("design_over_ear.json");
    let p = Parametric::parse(&text).unwrap();
    let ov = Overrides::new();
    let c = Circuit::from_parametric(&p, &ov).unwrap();
    let opts = PoleFitOptions::default();
    let fit = fit_probe(&c, "p_drp", &opts).unwrap();
    let a = attribute_poles(&p, &ov, &fit, &opts, &SensitivityOptions::default()).unwrap();
    let find = |f: f64| {
        a.poles
            .iter()
            .find(|q| (q.f_hz / f - 1.0).abs() < 0.03)
            .unwrap_or_else(|| panic!("no resonant pole near {f}: {:?}", a.poles))
    };
    let coupled = find(936.0);
    assert_eq!(coupled.parameters[0].parameter, "driver_Sd_cm2");
    assert!((coupled.parameters[0].dlnf_dlnp - 1.0).abs() < 0.05);
    assert_eq!(coupled.parameters[1].parameter, "driver_Mms_g");
    assert!((coupled.parameters[1].dlnf_dlnp + 0.5).abs() < 0.02);
    let depth = find(11_570.0);
    assert_eq!(depth.parameters[0].parameter, "front_depth_mm");
    assert!((depth.parameters[0].dlnf_dlnp + 1.0).abs() < 0.05);
    let ear = find(7_100.0);
    assert!(ear.fixed_elements.contains(&"ear".to_string()), "{:?}", ear);
}

// ----- Pure delay, alignment and energy concentration -----------------------

/// A 100 mm tube of radius 10 mm between a matched source and a matched
/// termination (ρc/S): a travelling wave, H ≈ e^{−jωL/c} with small
/// thermoviscous dispersion.
fn matched_tube() -> Circuit {
    let air = acoustilab::AirState::spec_reference();
    let zc = air.rho_c() / (PI * 1e-4);
    let doc = json!({
        "air": {"preset": "spec_reference"},
        "sweep": {"f_min_Hz": 10, "f_max_Hz": 20000, "points_per_octave": 24},
        "nodes": [{"id": "s", "domain": "acoustic"}, {"id": "a", "domain": "acoustic"}],
        "elements": [
            {"id": "src", "type": "pressure_source", "node": "s", "p_Pa": 2.0, "Zs_Pa_s_per_m3": zc},
            {"id": "tube", "type": "tube", "nodes": ["s", "a"], "radius_mm": 10, "length_mm": 100},
            {"id": "end", "type": "acoustic_resistance", "node": "a", "R_Pa_s_per_m3": zc}
        ],
        "probes": [{"id": "p", "quantity": "pressure", "node": "a"}]
    });
    Circuit::from_json(&doc.to_string()).unwrap()
}

/// The matched tube's excess group delay is its propagation delay L/c =
/// 291.5 µs (to 2 %: the boundary layers slow the wave by
/// ~δ_v/a·(1 + (γ−1)/√Pr)/2, under 1 % above 100 Hz). The decision is
/// minimum phase (below 0.5 ms), the pure delay is found, and alignment
/// advances the mixed-phase IR's peak by that delay.
#[test]
fn pure_delay_of_a_matched_tube_and_alignment() {
    let c = matched_tube();
    let tau = 0.1 / 343.0;
    let plain = impulses(
        &c,
        "p",
        &ImpulseRequest {
            length_check: false,
            ..Default::default()
        },
    )
    .unwrap();
    let d = &plain.decision;
    assert!(
        (d.pure_delay_s / tau - 1.0).abs() < 0.02,
        "{} vs {tau}",
        d.pure_delay_s
    );
    assert!(
        (d.max_excess_gd_s / tau - 1.0).abs() < 0.02,
        "{}",
        d.max_excess_gd_s
    );
    assert_eq!(d.mode, "minimum");
    let aligned = impulses(
        &c,
        "p",
        &ImpulseRequest {
            length_check: false,
            align_delay: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(aligned.delay_removed_s, d.pure_delay_s);
    let shift = plain.mixed.peak_s - aligned.mixed.peak_s;
    assert!((shift - d.pure_delay_s).abs() <= 1.0 / 48_000.0, "{shift}");
}

/// Minimum-phase energy concentration (spec Section 17; Robinson's
/// energy-delay theorem): among causal sequences with the same magnitude
/// spectrum the minimum-phase one accumulates energy fastest, so its
/// partial energies dominate at every sample. The mixed-phase sequence
/// must itself be causal for the theorem to apply, so the response is
/// negligible at Nyquist: a 200 Hz, Q = 0.7 low-pass (−83 dB at 24 kHz)
/// times the all-pass (a − s)/(a + s), a = 2π·300 rad/s. Tolerance 1e-9
/// of the total energy (the totals are equal by Parseval).
#[test]
fn minimum_phase_energy_concentration() {
    let lp = Lp::new(200.0, 0.7, 1e-6);
    let a = 2.0 * PI * 300.0;
    let n = 16384;
    let df = 48_000.0 / n as f64;
    let mut half: Vec<C64> = (0..=n / 2)
        .map(|k| {
            let s = jw(k as f64 * df);
            lp.h(s) * (a - s) / (a + s)
        })
        .collect();
    half[n / 2] = C64::new(half[n / 2].re, 0.0);
    let fir =
        acoustilab::time::minphase::min_phase_fir_spectrum(&half, (1, n / 2), df, None, 4, 1.0);
    let mixed = impulse(&half, 48_000.0, 0).h;
    let minimum = impulse(&fir, 48_000.0, 0).h;
    let total: f64 = mixed.iter().map(|v| v * v).sum();
    let total_min: f64 = minimum.iter().map(|v| v * v).sum();
    assert!((total_min / total - 1.0).abs() < 1e-9);
    let (mut e_min, mut e_mix) = (0.0, 0.0);
    for (x, y) in minimum.iter().zip(&mixed) {
        e_min += x * x;
        e_mix += y * y;
        assert!(e_min >= e_mix - 1e-9 * total, "{e_min} < {e_mix}");
    }
    // The all-pass holds back a measurable share: after 1 ms the mixed
    // response has delivered less energy than the minimum-phase one.
    let m = 48;
    let head = |h: &[f64]| h[..m].iter().map(|v| v * v).sum::<f64>() / total;
    assert!(head(&minimum) > head(&mixed) + 0.01);
}
