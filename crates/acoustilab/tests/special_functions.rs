//! Special functions and radiation impedances against mpmath references
//! (tools/refgen/special_refs.py and tools/refgen/radiation_refs.py).
//!
//! Tolerance: relative error ≤ 1e-12 for every function, with one stated
//! exception. J1 has zeros, and its relative condition number
//! κ = |x·J1'(x)/J1(x)| is unbounded there: any evaluation that forms
//! x·sin θ or x − 3π/4 in double precision perturbs x by about one ulp and
//! so moves J1 by u·|x·J1'| (u = 2^-53), which is κ·u relative. J1 is
//! therefore checked at 1e-12 relative wherever κ ≤ 1e3, and everywhere
//! against the backward-error bound |ΔJ1| ≤ 1e-12·|J1| + 8u·|x·J1'(x)|,
//! including at points within 1e-9 of every zero below 500 (κ ≈ 1e9). Above
//! x = 25 the phase is carried in double-double arithmetic and the error
//! stays below 5 % of that bound. The engine only uses J1 through
//! R1 = 1 − 2J1(x)/x ≥ 0.42 (x ≥ 2), which is checked at 1e-12 relative
//! everywhere.

use acoustilab::elements::radiation::{radiation_impedance, Baffle};
use acoustilab::special::*;
use acoustilab::{AirState, C64};
use std::f64::consts::PI;

struct Table {
    cols: Vec<String>,
    rows: Vec<Vec<f64>>,
}

impl Table {
    /// Loads a fixture whose numbers are decimal strings (exact doubles).
    fn load(name: &str) -> Table {
        let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let strs = |x: &serde_json::Value| -> Vec<String> {
            x.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        let cols = strs(&v["columns"]);
        let rows: Vec<Vec<f64>> = v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| strs(r).iter().map(|s| s.parse::<f64>().unwrap()).collect())
            .collect();
        assert!(!rows.is_empty() && rows.iter().all(|r| r.len() == cols.len()));
        Table { cols, rows }
    }

    fn col(&self, name: &str) -> usize {
        self.cols
            .iter()
            .position(|c| c == name)
            .unwrap_or_else(|| panic!("no column {name}"))
    }

    fn c64(&self, row: &[f64], name: &str) -> C64 {
        C64::new(
            row[self.col(&format!("{name}_re"))],
            row[self.col(&format!("{name}_im"))],
        )
    }
}

fn rel(a: C64, b: C64) -> f64 {
    (a - b).norm() / b.norm()
}

/// Tracks the worst relative error of one quantity.
struct Worst {
    name: &'static str,
    err: f64,
    at: String,
}

impl Worst {
    fn new(name: &'static str) -> Self {
        Worst {
            name,
            err: 0.0,
            at: String::new(),
        }
    }
    fn add(&mut self, err: f64, at: impl FnOnce() -> String) {
        assert!(
            err.is_finite(),
            "{}: non-finite error at {}",
            self.name,
            at()
        );
        if err > self.err {
            self.err = err;
            self.at = at();
        }
    }
    fn check(&self, tol: f64) {
        println!("{}: worst {:.2e} at {}", self.name, self.err, self.at);
        assert!(
            self.err <= tol,
            "{}: {:.3e} > {tol:e} at {}",
            self.name,
            self.err,
            self.at
        );
    }
}

#[test]
fn bessel_i_ratio_matches_mpmath() {
    let t = Table::load("special_bessel_ratio.json");
    let mut w = [Worst::new("I1/I0"), Worst::new("I2/I1")];
    for r in &t.rows {
        let nu = r[t.col("nu")] as u32;
        let z = C64::new(r[t.col("z_re")], r[t.col("z_im")]);
        let e = rel(bessel_i_ratio(nu, z), t.c64(r, "ratio"));
        w[nu as usize].add(e, || format!("|z| = {}, arg = {}", z.norm(), z.arg()));
    }
    for x in &w {
        x.check(1e-12);
    }
}

#[test]
fn shape_functions_match_mpmath() {
    let t = Table::load("special_shape.json");
    let mut w = [
        Worst::new("slit F"),
        Worst::new("slit 1-F"),
        Worst::new("circle F"),
        Worst::new("circle 1-F"),
    ];
    for r in &t.rows {
        let z = C64::new(r[t.col("z_re")], r[t.col("z_im")]);
        let (sl, ci) = (shape_slit(z), shape_circle(z));
        let got = [sl.f, sl.one_minus, ci.f, ci.one_minus];
        let want = [
            t.c64(r, "slit_f"),
            t.c64(r, "slit_om"),
            t.c64(r, "circle_f"),
            t.c64(r, "circle_om"),
        ];
        for i in 0..4 {
            w[i].add(rel(got[i], want[i]), || format!("x = {}", r[0]));
        }
    }
    for x in &w {
        x.check(1e-12);
    }
}

#[test]
fn piston_functions_match_mpmath() {
    let t = Table::load("special_piston.json");
    let u = f64::EPSILON / 2.0;
    let mut w_j1 = Worst::new("J1 (relative, condition number <= 1e3)");
    let mut w_j1_small = Worst::new("J1 (bound units, x < 25)");
    let mut w_j1_large = Worst::new("J1 (bound units, x >= 25)");
    let mut w = [Worst::new("H1"), Worst::new("R1"), Worst::new("X1")];
    for r in &t.rows {
        let x = r[t.col("x")];
        let at = || format!("x = {x}");
        if x == 0.0 {
            assert_eq!(bessel_j1(0.0), 0.0);
            assert_eq!(struve_h1(0.0), 0.0);
            assert_eq!(piston_r1(0.0), 0.0);
            assert_eq!(piston_x1(0.0), 0.0);
            continue;
        }
        let j1 = r[t.col("j1")];
        let x_dj1 = r[t.col("x_dj1")].abs();
        let got = bessel_j1(x);
        if x_dj1 <= 1e3 * j1.abs() {
            w_j1.add(((got - j1) / j1).abs(), at);
        }
        // Error in units of the backward-error bound: must stay ≤ 1.
        let units = (got - j1).abs() / (1e-12 * j1.abs() + 8.0 * u * x_dj1);
        if x < 25.0 {
            w_j1_small.add(units, at);
        } else {
            w_j1_large.add(units, at);
        }
        let got = [struve_h1(x), piston_r1(x), piston_x1(x)];
        let want = [r[t.col("h1")], r[t.col("r1")], r[t.col("x1")]];
        for i in 0..3 {
            w[i].add(((got[i] - want[i]) / want[i]).abs(), at);
        }
    }
    w_j1.check(1e-12);
    w_j1_small.check(1.0);
    // The phase-form branch is far inside the bound, even 1e-9 from zeros.
    w_j1_large.check(0.05);
    for x in &w {
        x.check(1e-12);
    }
}

#[test]
fn unflanged_pipe_matches_levine_schwinger() {
    let t = Table::load("special_unflanged.json");
    let mut w = [Worst::new("-ln|R|"), Worst::new("L/a")];
    for r in &t.rows {
        let ka = r[t.col("ka")];
        let (a, l) = unflanged_pipe(ka);
        let (a_ref, l_ref) = (r[t.col("attenuation")], r[t.col("end_correction")]);
        if ka == 0.0 {
            assert_eq!(a, 0.0);
        } else {
            w[0].add(((a - a_ref) / a_ref).abs(), || format!("ka = {ka}"));
        }
        w[1].add(((l - l_ref) / l_ref).abs(), || format!("ka = {ka}"));
    }
    for x in &w {
        x.check(1e-12);
    }
}

/// Normalised radiation impedance Z·S/(ρc) of a circular aperture.
fn z_norm(baffle: Baffle, ka: f64) -> C64 {
    let air = AirState::spec_reference();
    let a = 1e-3;
    let omega = ka * air.c / a;
    radiation_impedance(baffle, a, &air, omega) * (PI * a * a / air.rho_c())
}

#[test]
fn radiation_low_ka_limits() {
    // Baffled piston: R = (ka)²/2, end correction 8a/(3π).
    // Unflanged pipe (Levine & Schwinger): R = (ka)²/4, end correction
    // 0.6127a (their printed 0.6133a is within 0.1 %). The next terms are
    // O((ka)² ln ka) relative, 1e-7 at ka = 1e-4.
    for ka in [1e-5, 1e-4] {
        let z = z_norm(Baffle::Infinite, ka);
        assert!((z.re / (ka * ka / 2.0) - 1.0).abs() < 1e-6, "{z}");
        assert!((z.im / ka / (8.0 / (3.0 * PI)) - 1.0).abs() < 1e-6, "{z}");
        let z = z_norm(Baffle::Free, ka);
        assert!((z.re / (ka * ka / 4.0) - 1.0).abs() < 1e-6, "{z}");
        let delta = z.im / ka;
        assert!((delta - 0.612_701_035_93).abs() < 1e-6, "{delta}");
        assert!((delta / 0.6133 - 1.0).abs() < 1e-3, "{delta}");
    }
}

#[test]
fn unflanged_radiation_is_passive_smooth_and_below_the_piston() {
    let mut prev = z_norm(Baffle::Free, 0.01);
    let mut ka = 0.01;
    while ka < 8.0 {
        let z = z_norm(Baffle::Free, ka);
        assert!(z.re > 0.0 && z.im > 0.0, "ka = {ka}: {z}");
        // |R| ≤ 1 and decreasing, end correction decreasing.
        let (a, l) = unflanged_pipe(ka);
        assert!(a > 0.0 && l > 0.0 && l < 0.62);
        let step = (z - prev).norm();
        assert!(step < 0.02, "jump of {step} at ka = {ka}");
        prev = z;
        ka += 0.005;
    }
    // Continuity where the Levine–Schwinger evaluation hands over to the
    // continuation (ka = 3.8).
    let e = LEVINE_SCHWINGER_KA_MAX;
    let (a0, l0) = unflanged_pipe(e);
    let (a1, l1) = unflanged_pipe(e * (1.0 + 1e-12));
    assert!((a0 - a1).abs() < 1e-10 && (l0 - l1).abs() < 1e-10);
    // An unflanged opening radiates less than a baffled piston at low ka
    // (half the resistance) and has the shorter end correction.
    for ka in [0.05, 0.2, 0.5] {
        let (zf, zi) = (z_norm(Baffle::Free, ka), z_norm(Baffle::Infinite, ka));
        assert!(zf.re < zi.re && zf.im < zi.im);
    }
}

#[test]
fn unflanged_pipe_agrees_with_silva_et_al_fits() {
    // Silva et al., JSV 322 (2009), Eqs. (21)-(22) and Table 1, state errors
    // below 2 % for ka < 3 against Levine–Schwinger. Their fit has
    // L/a(0) = 0.6133; the largest deviation we find is 2.9 % in L at
    // ka ≈ 2.6 and 0.9 % in |R| (tools/refgen/radiation_refs.py prints the
    // table), so the check uses 3 %.
    let silva = |ka: f64| {
        let x2 = ka * ka;
        let r = (1.0 + 0.8 * x2) / (1.0 + 1.3 * x2 + 0.266 * x2 * x2 + 0.0263 * x2 * x2 * x2);
        let l = 0.6133 * (1.0 + 0.0599 * x2)
            / (1.0 + 0.238 * x2 - 0.0153 * x2 * x2 + 0.0015 * x2 * x2 * x2);
        (r, l)
    };
    let mut ka = 0.05;
    while ka <= 3.0 {
        let (a, l) = unflanged_pipe(ka);
        let (rs, ls) = silva(ka);
        assert!(((-a).exp() / rs - 1.0).abs() < 0.01, "|R| at ka = {ka}");
        assert!((l / ls - 1.0).abs() < 0.03, "L at ka = {ka}");
        ka += 0.05;
    }
}

/// Cost per call in release mode:
/// `cargo test --release --test special_functions -- --ignored --nocapture`
#[test]
#[ignore]
fn timing_shape_functions() {
    use std::hint::black_box;
    use std::time::Instant;
    fn ns(f: impl Fn(C64) -> C64, z: C64) -> f64 {
        let n = 200_000;
        let mut acc = C64::new(0.0, 0.0);
        for _ in 0..1000 {
            acc += f(black_box(z));
        }
        let t = Instant::now();
        for _ in 0..n {
            acc += f(black_box(z));
        }
        black_box(acc);
        t.elapsed().as_nanos() as f64 / n as f64
    }
    println!(
        "{:>8} {:>10} {:>10} {:>12}",
        "|z|", "circle ns", "slit ns", "rect 1x3 ns"
    );
    for r in [
        1e-4, 1e-2, 0.1, 1.0, 3.0, 10.0, 17.9, 18.0, 30.0, 60.0, 100.0, 300.0, 1000.0, 2000.0, 1e4,
    ] {
        let z = C64::from_polar(r, std::f64::consts::FRAC_PI_4);
        println!(
            "{:>8} {:>10.0} {:>10.0} {:>12.0}",
            r,
            ns(|z| shape_circle(z).one_minus, z),
            ns(|z| shape_slit(z).one_minus, z),
            ns(|k| shape_rect(k, 1.0, 3.0).one_minus, z),
        );
    }
    for x in [1.0, 10.0, 30.0, 500.0] {
        let z = C64::new(x, 0.0);
        println!(
            "x = {x}: J1 {:.0} ns, H1 {:.0} ns, unflanged(x/10) {:.0} ns",
            ns(|z| C64::new(bessel_j1(z.re), 0.0), z),
            ns(|z| C64::new(struve_h1(z.re), 0.0), z),
            ns(|z| C64::new(unflanged_pipe(z.re / 10.0).1, 0.0), z),
        );
    }
}
