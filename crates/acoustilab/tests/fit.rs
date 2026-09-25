//! Parameter identification, identifiability and the virtual rig
//! (docs/fitting.md; spec Section 12 and case study 2 of Section 18).
//!
//! Every fit here runs on synthetic measurements from the virtual rig with
//! known true parameters. "Recovered" means the truth lies within the
//! reported interval: tests of single fits use 2.576 standard deviations
//! (99 %) so that a fixed seed is not a coin toss, and
//! `confidence_intervals_are_calibrated` checks the 95 % coverage and the
//! size of the reported standard deviations over 30 seeds.
//!
//! The optimiser and the covariance are checked against scipy
//! (`tools/fit/reference.py`, fixture `tests/data/fit/reference.json`).

use acoustilab::expr::PValue;
use acoustilab::fit::identify::{self, Level};
use acoustilab::fit::lm::{self, Bound, FnProblem, LmOptions};
use acoustilab::fit::rig::{self, Noise, RigSpec};
use acoustilab::fit::{self, CurveSpec, FitError, FitParam, FitReport, FitSpec, Offset, Status};
use acoustilab::io::curve::exchange_grid;
use acoustilab::io::sidecar::{Averaging, Origin};
use acoustilab::io::{text, Curve, Format, Quantity};
use acoustilab::params::{Overrides, Parametric};
use acoustilab::Circuit;
use serde_json::{json, Value};

/// A driver in free air parameterised by its primary set.
const PRIMARY: &str = r#"{
 "schema": "acoustilab-netlist/0.2",
 "title": "Driver in free air, primary set",
 "parameters": {
   "fs_Hz": {"value": 81.8, "min": 20, "max": 400},
   "Qms": {"value": 2.71, "min": 0.3, "max": 15},
   "Qes": {"value": 1.01, "min": 0.1, "max": 10},
   "Re_ohm": {"value": 32.8, "min": 4, "max": 600},
   "Mms_g": {"value": 0.3, "min": 0.05, "max": 3},
   "Sd_cm2": {"value": 10, "min": 2, "max": 25},
   "creep_lambda": {"value": 0, "min": 0, "max": 0.3},
   "Le_uH": {"value": 0, "min": 0, "max": 500}
 },
 "level": 0,
 "drive": {"voltage_V": 1},
 "nodes": [{"id": "e_in", "domain": "electrical"}],
 "elements": [
   {"id": "amp", "type": "vsource", "node": "e_in", "V_V": 1},
   {"id": "drv", "type": "driver", "nodes": ["e_in", "gnd", "ambient", "ambient"], "model": "D1",
    "fs_Hz": "=fs_Hz", "Qms": "=Qms", "Qes": "=Qes", "Re_ohm": "=Re_ohm", "Mms_g": "=Mms_g",
    "Sd_cm2": "=Sd_cm2", "creep_lambda": "=creep_lambda", "Le_uH": "=Le_uH"}
 ],
 "probes": [{"id": "zin", "quantity": "impedance", "element": "amp"}]
}"#;

const BENCH_TRUTH: [(&str, f64); 5] = [
    ("Re_ohm", 31.5),
    ("Bl_Tm", 2.5),
    ("Mms_g", 0.33),
    ("Cms_mm_per_N", 11.0),
    ("Rms_Ns_per_m", 0.07),
];

fn example(name: &str) -> Parametric {
    let path = format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"));
    Parametric::parse(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn ov(pairs: &[(&str, f64)]) -> Overrides {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), PValue::Num(*v)))
        .collect()
}

fn impedance_noise(seed: u64) -> Noise {
    Noise {
        seed,
        level_db: 0.05,
        phase_deg: 0.3,
        ..Noise::default()
    }
}

/// A virtual measurement of `probe` under the true values plus the
/// condition `extra`.
fn measure(
    p: &Parametric,
    probe: &str,
    truth: &[(&str, f64)],
    extra: &[(&str, f64)],
    grid: &[f64],
    noise: Noise,
) -> Curve {
    let mut s = RigSpec::new(probe);
    s.overrides = ov(truth);
    s.overrides.extend(ov(extra));
    s.freqs = grid.to_vec();
    s.noise = noise;
    rig::measure(p, &s).unwrap()
}

fn params(names: &[(&str, f64)]) -> Vec<FitParam> {
    names.iter().map(|(n, _)| FitParam::new(n)).collect()
}

fn param<'a>(rep: &'a FitReport, name: &str) -> &'a fit::ParamReport {
    rep.parameter(name)
        .unwrap_or_else(|| panic!("no parameter {name}"))
}

/// The truth lies within 2.576 standard deviations (99 %) of the fitted
/// log value, and the parameter is reported as determined.
fn recovered(rep: &FitReport, name: &str, truth: f64) {
    let p = param(rep, name);
    assert!(
        p.status.is_determined(),
        "{name}: {:?} {:?}",
        p.status,
        p.notes
    );
    let sd = p.sd.unwrap();
    let z = (p.value / truth).ln() / sd;
    assert!(
        z.abs() <= 2.576,
        "{name}: fitted {} vs truth {truth}, sd {sd}, z {z}",
        p.value
    );
    let [lo, hi] = p.ci95.unwrap();
    assert!(lo < p.value && p.value < hi);
}

// ----- Optimiser and covariance against scipy -------------------------------

fn fixture() -> Value {
    let path = format!(
        "{}/tests/data/fit/reference.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn nums(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().parse::<f64>().unwrap())
        .collect()
}

#[test]
fn lm_and_covariance_match_scipy() {
    // Weighted fit of a resonance level in ln-parameter space; scipy's
    // curve_fit (analytic Jacobian, absolute_sigma = False) is the reference.
    let fx = fixture();
    let ls = &fx["least_squares"];
    let f = nums(&ls["frequencies_Hz"]);
    let y = nums(&ls["level_dB"]);
    let sigma = ls["sigma_dB"].as_str().unwrap().parse::<f64>().unwrap();
    let level = |f: f64, u: &[f64]| {
        let (a, f0, q) = (u[0].exp(), u[1].exp(), u[2].exp());
        let x = f / f0;
        20.0 * (a / ((1.0 - x * x).powi(2) + (x / q).powi(2)).sqrt()).log10()
    };
    let resid = |u: &[f64]| -> Result<Vec<f64>, String> {
        Ok(f.iter()
            .zip(&y)
            .map(|(f, y)| (level(*f, u) - y) / sigma)
            .collect())
    };
    let u0: Vec<f64> = nums(&ls["start"]).iter().map(|x| x.ln()).collect();
    let mut prob = FnProblem {
        f: resid,
        steps: vec![1e-5; 3],
    };
    let o = LmOptions {
        ftol: 1e-15,
        xtol: 1e-14,
        ..LmOptions::default()
    };
    let r = lm::minimize(&mut prob, &u0, &[Bound::Free; 3], &o).unwrap();
    assert!(r.stop.converged(), "{r:?}");
    let popt = nums(&ls["popt"]);
    for (u, p) in r.u.iter().zip(&popt) {
        assert!((u.exp() / p - 1.0).abs() < 1e-9, "{} vs {p}", u.exp());
    }
    let chi2 = ls["chi2"].as_str().unwrap().parse::<f64>().unwrap();
    assert!((r.cost / chi2 - 1.0).abs() < 1e-12);
    // Covariance of the ln parameters: s²·(JᵀJ)⁻¹ with s² = χ²/(m − n).
    let (jac, _) = acoustilab::fit::jacobian::central_differences(
        &mut |u: &[f64]| resid(u),
        &r.u,
        &r.residuals,
        &[1e-5; 3],
        &|_, _| true,
    )
    .unwrap();
    let dof = ls["dof"].as_f64().unwrap();
    let names: Vec<String> = ["A", "f0", "Q"].iter().map(|s| s.to_string()).collect();
    let an = identify::analyse(&names, &jac, r.cost / dof, 1e-9);
    let cov: Vec<Vec<f64>> = ls["cov_ln"].as_array().unwrap().iter().map(nums).collect();
    for (i, row) in cov.iter().enumerate() {
        for (j, c) in row.iter().enumerate() {
            let tol = 1e-6 * (cov[i][i] * cov[j][j]).sqrt();
            assert!(
                (an.covariance.get(i, j) - c).abs() < tol,
                "cov[{i}][{j}]: {} vs {c}",
                an.covariance.get(i, j)
            );
        }
    }
}

#[test]
fn jacobian_of_full_solves_matches_the_closed_form() {
    // d(20·log10|Z|)/d(ln fs) and d(arg Z)/d(ln fs) of a D0 driver in free
    // air, by the fit's central differences of network solves (step 1e-4 in
    // ln fs), against the derivative of Z = Re + (Re/Qes)/(jw + 1/Qms +
    // 1/(jw)), w = f/fs: dZ/d(ln fs) = −w·dZ/dw.
    let p = Parametric::parse(PRIMARY).unwrap();
    let freqs = [30.0, 73.6, 81.8, 100.0, 300.0, 3000.0];
    let (fs, qms, qes, re) = (81.8f64, 2.71, 1.01, 32.8);
    let mut f = |u: &[f64]| -> Result<Vec<f64>, String> {
        let mut c = Circuit::from_parametric(&p, &ov(&[("fs_Hz", u[0].exp())]))
            .map_err(|e| e.to_string())?;
        c.freqs = freqs.to_vec();
        let r = c.solve().map_err(|e| e.to_string())?;
        let z = &r.probe("zin").unwrap().values;
        Ok(z.iter()
            .map(|z| 20.0 * z.norm().log10())
            .chain(z.iter().map(|z| z.arg().to_degrees()))
            .collect())
    };
    let u = [fs.ln()];
    let r0 = f(&u).unwrap();
    let (jac, _) =
        acoustilab::fit::jacobian::central_differences(&mut f, &u, &r0, &[1e-4], &|_, _| true)
            .unwrap();
    let j = acoustilab::C64::new(0.0, 1.0);
    for (i, &fr) in freqs.iter().enumerate() {
        let w = fr / fs;
        let d = j * w + 1.0 / qms + 1.0 / (j * w);
        let z = re + (re / qes) / d;
        let dz_dw = -(re / qes) * (j - 1.0 / (j * w * w)) / (d * d);
        let g = -w * dz_dw / z; // d(ln Z)/d(ln fs)
        let (dl, dp) = (20.0 / std::f64::consts::LN_10 * g.re, g.im.to_degrees());
        let n = freqs.len();
        // Truncation (h²/6 times the third derivative) dominates near the
        // resonance: 1.6e-7 relative there, 1e-8 or less elsewhere.
        let tol = |x: f64| 1e-6 * x.abs().max(1.0);
        assert!(
            (jac.get(i, 0) - dl).abs() < tol(dl),
            "{fr} Hz level: {} vs {dl}",
            jac.get(i, 0)
        );
        assert!(
            (jac.get(n + i, 0) - dp).abs() < tol(dp),
            "{fr} Hz phase: {} vs {dp}",
            jac.get(n + i, 0)
        );
    }
}

#[test]
fn svd_matches_numpy() {
    let fx = fixture();
    let sv = &fx["svd"];
    let a: Vec<Vec<f64>> = sv["matrix"].as_array().unwrap().iter().map(nums).collect();
    let d = acoustilab::fit::dense::svd(&acoustilab::fit::dense::Mat::from_rows(&a));
    let s = nums(&sv["singular_values"]);
    let v: Vec<Vec<f64>> = sv["v"].as_array().unwrap().iter().map(nums).collect();
    for (i, want) in s.iter().enumerate() {
        // The smallest singular value (8e-8) is still accurate relative to
        // the largest; one-sided Jacobi gets it to far better than that.
        assert!(
            (d.s[i] - want).abs() < 1e-14 * s[0],
            "s[{i}]: {} vs {want}",
            d.s[i]
        );
        let col = d.direction(i);
        let sign = (col[0] * v[0][i]).signum();
        for (k, row) in v.iter().enumerate() {
            let tol = if i == s.len() - 1 { 1e-7 } else { 1e-12 };
            assert!((sign * col[k] - row[i]).abs() < tol, "v[{k}][{i}]");
        }
    }
}

// ----- The impedance-only rule and its resolutions (spec Section 12) --------

#[test]
fn impedance_only_fit_flags_the_bl_scale_ambiguity() {
    let p = example("driver_bench.json");
    let grid = exchange_grid(10.0, 20_000.0, 12.0);
    let z = measure(&p, "zin", &BENCH_TRUTH, &[], &grid, impedance_noise(7));
    let rep = fit::fit(
        &p,
        &FitSpec::new(params(&BENCH_TRUTH), vec![CurveSpec::new("zin", z)]),
    )
    .unwrap();
    assert!(rep.converged, "{:?}", rep.stop);
    recovered(&rep, "Re_ohm", 31.5);
    for name in ["Bl_Tm", "Mms_g", "Cms_mm_per_N", "Rms_Ns_per_m"] {
        let pr = param(&rep, name);
        assert_eq!(pr.status, Status::ScaleAmbiguous, "{name}");
        assert!(pr.ci95.is_none(), "{name} must not carry an interval");
    }
    // The data do determine Bl²/Mms, Bl²·Cms and Bl²/Rms.
    let v = |n: &str| param(&rep, n).value;
    let t = |n: &str| BENCH_TRUTH.iter().find(|x| x.0 == n).unwrap().1;
    for combo in [
        |g: &dyn Fn(&str) -> f64| g("Bl_Tm").powi(2) / g("Mms_g"),
        |g: &dyn Fn(&str) -> f64| g("Bl_Tm").powi(2) * g("Cms_mm_per_N"),
        |g: &dyn Fn(&str) -> f64| g("Bl_Tm").powi(2) / g("Rms_Ns_per_m"),
    ] {
        let (fitted, truth) = (combo(&v), combo(&t));
        assert!((fitted / truth - 1.0).abs() < 0.01, "{fitted} vs {truth}");
    }
    // Exactly one null direction: Bl, Mms, Cms and Rms in ln ratio
    // 1 : 2 : −2 : 2.
    let null: Vec<&identify::Direction> = rep
        .identifiability
        .directions
        .iter()
        .filter(|d| d.status == Level::Unidentifiable)
        .collect();
    assert_eq!(null.len(), 1, "{:#?}", rep.identifiability.directions);
    let want = [
        ("Bl_Tm", 1.0),
        ("Mms_g", 2.0),
        ("Cms_mm_per_N", -2.0),
        ("Rms_Ns_per_m", 2.0),
    ];
    let norm = 13f64.sqrt();
    assert_eq!(null[0].components.len(), 4);
    for (c, (n, w)) in null[0].components.iter().zip(want) {
        assert_eq!(c.name, n);
        assert!((c.weight - w / norm).abs() < 1e-4, "{c:?}");
    }
    assert!(
        null[0].text.contains("ratio +1 : +2 : -2 : +2"),
        "{}",
        null[0].text
    );
    let rule = &rep.identifiability.rules[0];
    assert_eq!(rule.code, "scale_ambiguous");
    assert_eq!(rule.parameters.len(), 4);
    assert_eq!(rule.resolve_with.len(), 4);
    assert!(rule.message.contains("Bl²/Mms, Bl²·Cms and Bl²/Rms"));
    // Fixing Bl removes the ambiguity: nothing else carries the scale.
    let rest: Vec<(&str, f64)> = BENCH_TRUTH
        .iter()
        .copied()
        .filter(|(n, _)| *n != "Bl_Tm")
        .collect();
    let z = measure(&p, "zin", &BENCH_TRUTH, &[], &grid, impedance_noise(8));
    let mut spec = FitSpec::new(params(&rest), vec![CurveSpec::new("zin", z)]);
    spec.overrides = ov(&[("Bl_Tm", 2.5)]);
    let rep = fit::fit(&p, &spec).unwrap();
    assert!(rep.identifiability.rules.is_empty());
    for (n, t) in rest {
        recovered(&rep, n, t);
    }
}

#[test]
fn added_mass_resolves_the_scale() {
    let p = example("driver_bench.json");
    let grid = exchange_grid(10.0, 20_000.0, 12.0);
    let mut curves = Vec::new();
    for (seed, mass) in [(7, 0.0), (8, 150.0)] {
        let z = measure(
            &p,
            "zin",
            &BENCH_TRUTH,
            &[("added_mass_mg", mass)],
            &grid,
            impedance_noise(seed),
        );
        let mut cs = CurveSpec::new("zin", z);
        cs.overrides = ov(&[("added_mass_mg", mass)]);
        curves.push(cs);
    }
    let rep = fit::fit(&p, &FitSpec::new(params(&BENCH_TRUTH), curves)).unwrap();
    for (n, t) in BENCH_TRUTH {
        recovered(&rep, n, t);
    }
    let codes: Vec<&str> = rep.identifiability.rules.iter().map(|f| f.code).collect();
    assert_eq!(codes, vec!["scale_resolved", "added_mass_unreliable"]);
    let msg = &rep.identifiability.rules[1].message;
    // 0.33 g: a 10 mg dot is 3 % of it; 10 mg of the 150 mg test mass
    // moves Mms by 6.7 %.
    assert!(msg.contains("0.3287") || msg.contains("0.33"), "{msg}");
    assert!(msg.contains("6.667 %"), "{msg}");
    assert!(rep
        .identifiability
        .directions
        .iter()
        .all(|d| d.status == Level::Determined));

    // A 12 g driver with a 6 g test mass: no reliability flag.
    let woofer = [
        ("Re_ohm", 6.5),
        ("Bl_Tm", 7.0),
        ("Mms_g", 12.0),
        ("Cms_mm_per_N", 0.8),
        ("Rms_Ns_per_m", 1.2),
    ];
    let mut curves = Vec::new();
    for (seed, mass) in [(17, 0.0), (18, 6000.0)] {
        let z = measure(
            &p,
            "zin",
            &woofer,
            &[("added_mass_mg", mass)],
            &grid,
            impedance_noise(seed),
        );
        let mut cs = CurveSpec::new("zin", z);
        cs.overrides = ov(&[("added_mass_mg", mass)]);
        curves.push(cs);
    }
    let mut spec = FitSpec::new(params(&woofer), curves);
    // Start from the nominal 40 mm driver: far from the woofer.
    spec.starts = 1;
    let rep = fit::fit(&p, &spec).unwrap();
    for (n, t) in woofer {
        recovered(&rep, n, t);
    }
    let codes: Vec<&str> = rep.identifiability.rules.iter().map(|f| f.code).collect();
    assert_eq!(codes, vec!["scale_resolved"]);
}

#[test]
fn known_volume_resolves_the_scale_when_sd_is_known() {
    let p = example("driver_bench.json");
    let grid = exchange_grid(10.0, 20_000.0, 12.0);
    let mut curves = Vec::new();
    for (seed, v) in [(21, 0.0), (22, 20.0)] {
        let z = measure(
            &p,
            "zin",
            &BENCH_TRUTH,
            &[("box_volume_cm3", v)],
            &grid,
            impedance_noise(seed),
        );
        let mut cs = CurveSpec::new("zin", z);
        cs.overrides = ov(&[("box_volume_cm3", v)]);
        curves.push(cs);
    }
    let rep = fit::fit(&p, &FitSpec::new(params(&BENCH_TRUTH), curves.clone())).unwrap();
    for (n, t) in BENCH_TRUTH {
        recovered(&rep, n, t);
    }
    assert_eq!(rep.identifiability.rules[0].code, "scale_resolved");
    assert!(rep.identifiability.rules[0]
        .message
        .contains("known_volume"));
    // The lumped box leaves its validity range near 2 kHz: the fit says so.
    assert!(
        rep.warnings
            .iter()
            .any(|w| w.contains("curve 1 (zin)") && w.contains("outside its validity")),
        "{:?}",
        rep.warnings
    );
    // With Sd free the box stiffness Sd²·ρc²/V is unknown too: the scale
    // is ambiguous again, and the numerics agree (one null direction with
    // Sd in it).
    let mut with_sd = params(&BENCH_TRUTH);
    with_sd.push(FitParam::new("Sd_cm2"));
    let rep = fit::fit(&p, &FitSpec::new(with_sd, curves)).unwrap();
    assert_eq!(rep.identifiability.rules[0].code, "scale_ambiguous");
    assert!(rep.identifiability.rules[0]
        .parameters
        .contains(&"Sd_cm2".to_string()));
    assert_eq!(param(&rep, "Sd_cm2").status, Status::ScaleAmbiguous);
    let null: Vec<_> = rep
        .identifiability
        .directions
        .iter()
        .filter(|d| d.status == Level::Unidentifiable)
        .collect();
    assert_eq!(null.len(), 1);
    assert!(null[0].components.iter().any(|c| c.name == "Sd_cm2"));
    recovered(&rep, "Re_ohm", 31.5);
}

#[test]
fn laser_displacement_resolves_the_scale() {
    let p = example("driver_bench.json");
    let grid = exchange_grid(10.0, 20_000.0, 12.0);
    let z = measure(&p, "zin", &BENCH_TRUTH, &[], &grid, impedance_noise(31));
    let x = measure(
        &p,
        "x",
        &BENCH_TRUTH,
        &[],
        &exchange_grid(10.0, 2000.0, 12.0),
        Noise {
            seed: 32,
            level_db: 0.1,
            phase_deg: 0.5,
            ..Noise::default()
        },
    );
    assert_eq!(x.quantity, Quantity::Displacement);
    assert!(x.sidecar.drive.is_some());
    let rep = fit::fit(
        &p,
        &FitSpec::new(
            params(&BENCH_TRUTH),
            vec![CurveSpec::new("zin", z), CurveSpec::new("x", x)],
        ),
    )
    .unwrap();
    for (n, t) in BENCH_TRUTH {
        recovered(&rep, n, t);
    }
    assert!(rep.identifiability.rules[0]
        .message
        .contains("displacement"));
}

#[test]
fn calibrated_spl_in_a_known_load_resolves_the_scale() {
    // Free-air impedance plus the calibrated pressure in a sealed box of
    // known volume, Sd known: p ∝ Sd·Bl/Zm fixes what impedance leaves open.
    let p = example("driver_bench.json");
    let z = measure(
        &p,
        "zin",
        &BENCH_TRUTH,
        &[],
        &exchange_grid(10.0, 20_000.0, 12.0),
        impedance_noise(91),
    );
    let pb = measure(
        &p,
        "p_box",
        &BENCH_TRUTH,
        &[("box_volume_cm3", 20.0)],
        &exchange_grid(20.0, 1000.0, 12.0),
        Noise {
            seed: 92,
            level_db: 0.1,
            ..Noise::default()
        },
    );
    let mut cs = CurveSpec::new("p_box", pb);
    cs.overrides = ov(&[("box_volume_cm3", 20.0)]);
    let rep = fit::fit(
        &p,
        &FitSpec::new(params(&BENCH_TRUTH), vec![CurveSpec::new("zin", z), cs]),
    )
    .unwrap();
    assert_eq!(rep.identifiability.rules[0].code, "scale_resolved");
    assert!(rep.identifiability.rules[0]
        .message
        .contains("spl_known_load"));
    for (n, t) in BENCH_TRUTH {
        recovered(&rep, n, t);
    }
}

#[test]
fn spl_only_driver_fits_are_refused() {
    let p = example("driver_bench.json");
    let grid = exchange_grid(20.0, 2000.0, 12.0);
    let pb = measure(
        &p,
        "p_box",
        &BENCH_TRUTH,
        &[("box_volume_cm3", 20.0)],
        &grid,
        Noise {
            seed: 41,
            level_db: 0.1,
            ..Noise::default()
        },
    );
    let mut cs = CurveSpec::new("p_box", pb);
    cs.overrides = ov(&[("box_volume_cm3", 20.0)]);
    let mut spec = FitSpec::new(
        vec![FitParam::new("Bl_Tm"), FitParam::new("Re_ohm")],
        vec![cs.clone()],
    );
    match fit::fit(&p, &spec) {
        Err(FitError::Refused(m)) => {
            assert!(m.contains("Bl_Tm, Re_ohm"), "{m}");
            assert!(m.contains("spec Section 12"), "{m}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    spec.allow_spl_only = true;
    let rep = fit::fit(&p, &spec).unwrap();
    assert_eq!(rep.identifiability.rules[0].code, "spl_only_override");
    // Fitting a parameter of the load, not of the driver, to SPL is fine:
    // here the box volume, from its pressure-chamber level.
    let mut spec = FitSpec::new(
        vec![FitParam {
            start: Some(25.0),
            ..FitParam::new("box_volume_cm3")
        }],
        vec![CurveSpec::new("p_box", cs.curve.clone())],
    );
    spec.overrides = ov(&BENCH_TRUTH);
    let rep = fit::fit(&p, &spec).unwrap();
    recovered(&rep, "box_volume_cm3", 20.0);
}

// ----- Thiele–Small in free air, calibration of the intervals ----------------

#[test]
fn primary_set_fit_recovers_thiele_small_and_flags_mms() {
    let p = Parametric::parse(PRIMARY).unwrap();
    let truth = [
        ("fs_Hz", 90.0),
        ("Qms", 3.2),
        ("Qes", 0.9),
        ("Re_ohm", 31.0),
    ];
    let grid = exchange_grid(10.0, 20_000.0, 6.0);
    let z = measure(&p, "zin", &truth, &[], &grid, impedance_noise(51));
    let rep = fit::fit(
        &p,
        &FitSpec::new(params(&truth), vec![CurveSpec::new("zin", z.clone())]),
    )
    .unwrap();
    for (n, t) in truth {
        recovered(&rep, n, t);
    }
    assert!(rep.identifiability.rules.is_empty());
    assert!(!rep.curves[0].structured);
    // Mms has no effect on a free-air impedance without load: it is the
    // scale, and the rule marks it.
    let mut with_mms = params(&truth);
    with_mms.push(FitParam::new("Mms_g"));
    let rep = fit::fit(&p, &FitSpec::new(with_mms, vec![CurveSpec::new("zin", z)])).unwrap();
    let mms = param(&rep, "Mms_g");
    assert_eq!(mms.status, Status::ScaleAmbiguous);
    assert!(
        (mms.value / 0.3 - 1.0).abs() < 1e-12,
        "an unidentifiable parameter stays at its start: {}",
        mms.value
    );
    let null = rep
        .identifiability
        .directions
        .iter()
        .find(|d| d.status == Level::Unidentifiable)
        .unwrap();
    assert_eq!(null.components.len(), 1);
    assert_eq!(null.components[0].name, "Mms_g");
    for (n, t) in truth {
        recovered(&rep, n, t);
    }
}

#[test]
fn confidence_intervals_are_calibrated() {
    // 30 seeds × 4 parameters: the 95 % intervals should cover the truth
    // about 95 % of the time. The engine's intervals are conservative by
    // construction (s² ≥ 1), so the check is one-sided at the low end
    // (binomial: 114 expected, sd 2.4; below 102 is a failure), and the
    // reported standard deviations must match the scatter of the fits
    // (ratio of the empirical to the mean reported sd between 0.7 and 1.3;
    // with 30 samples the empirical sd is itself uncertain by ±13 %).
    let p = Parametric::parse(PRIMARY).unwrap();
    let truth = [
        ("fs_Hz", 90.0),
        ("Qms", 3.2),
        ("Qes", 0.9),
        ("Re_ohm", 31.0),
    ];
    let grid = exchange_grid(10.0, 20_000.0, 6.0);
    let n = 30;
    let mut covered = 0;
    let mut logs = vec![Vec::new(); 4];
    let mut sds = [0.0; 4];
    for seed in 0..n {
        let z = measure(&p, "zin", &truth, &[], &grid, impedance_noise(1000 + seed));
        let rep = fit::fit(
            &p,
            &FitSpec::new(params(&truth), vec![CurveSpec::new("zin", z)]),
        )
        .unwrap();
        for (k, (name, t)) in truth.iter().enumerate() {
            let pr = param(&rep, name);
            let [lo, hi] = pr.ci95.unwrap();
            if lo <= *t && *t <= hi {
                covered += 1;
            }
            logs[k].push((pr.value / t).ln());
            sds[k] += pr.sd.unwrap() / n as f64;
        }
    }
    assert!(covered >= 102, "coverage {covered}/120");
    for k in 0..4 {
        let m = logs[k].iter().sum::<f64>() / n as f64;
        let var = logs[k].iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n as f64 - 1.0);
        let ratio = var.sqrt() / sds[k];
        assert!(
            (0.7..=1.3).contains(&ratio),
            "{}: empirical sd {} vs reported {}",
            truth[k].0,
            var.sqrt(),
            sds[k]
        );
    }
}

#[test]
fn model_form_error_shows_structured_residuals() {
    // Data from a driver with creep and coil inductance (D1), fitted with
    // both fixed at zero (the D0 model).
    let p = Parametric::parse(PRIMARY).unwrap();
    let truth = [
        ("fs_Hz", 90.0),
        ("Qms", 3.2),
        ("Qes", 0.9),
        ("Re_ohm", 31.0),
    ];
    let grid = exchange_grid(10.0, 20_000.0, 6.0);
    let d1 = [("creep_lambda", 0.12), ("Le_uH", 60.0)];
    let z = measure(&p, "zin", &truth, &d1, &grid, impedance_noise(61));
    let wrong = fit::fit(
        &p,
        &FitSpec::new(params(&truth), vec![CurveSpec::new("zin", z.clone())]),
    )
    .unwrap();
    let c = &wrong.curves[0];
    assert!(c.structured, "{c:?}");
    assert!(c.lag1_autocorrelation > 0.5 && c.runs.z < -3.0, "{c:?}");
    assert!(c.inflation > 3.0);
    assert!(wrong.reduced_chi2.unwrap() > 10.0);
    assert!(wrong.warnings.iter().any(|w| w.contains("structured")));
    assert!(wrong
        .warnings
        .iter()
        .any(|w| w.contains("reduced chi-square")));
    // The same data fitted with the right model: unstructured, recovered.
    let mut right = params(&truth);
    right.extend(params(&d1));
    let good = fit::fit(&p, &FitSpec::new(right, vec![CurveSpec::new("zin", z)])).unwrap();
    assert!(!good.curves[0].structured, "{:?}", good.curves[0]);
    for (n, t) in truth.iter().chain(&d1) {
        recovered(&good, n, *t);
    }
    // No false precision: the wrong model's intervals are wider than the
    // right model's by the misfit (s² and the inflation), here 7 to 30×,
    // and in this case they still cover the truth. That is not guaranteed
    // in general: a biased model can be precisely wrong, which is why the
    // structure of the residuals is reported.
    for (n, t) in truth {
        let (w, g) = (param(&wrong, n).sd.unwrap(), param(&good, n).sd.unwrap());
        assert!(w > 5.0 * g, "{n}: {w} vs {g}");
        let [lo, hi] = param(&wrong, n).ci95.unwrap();
        assert!(lo <= t && t <= hi, "{n}: {t} outside [{lo}, {hi}]");
    }
}

// ----- Case study 2 (spec Section 18): impedance and drum response -----------

#[test]
fn design_template_case_study() {
    // The over-ear template in its sealed configuration (no baffle vents,
    // docs/over-ear-template.md): fit the pad leak, the front depth and the
    // driver's electrical parameters to a measured impedance and drum
    // response (IEC 60318-4), with a realistic pressure budget: 5 seatings,
    // repositioning, microphone calibration and coupler errors.
    let p = example("design_over_ear.json");
    let sealed = [("baffle_vent_count", 0.0)];
    let truth = [
        ("leak_gap_mm", 0.12),
        ("front_depth_mm", 13.0),
        ("driver_fs_Hz", 90.0),
        ("driver_Qms", 3.0),
        ("driver_Qes", 0.95),
        ("driver_Re_ohm", 31.8),
    ];
    let grid = exchange_grid(20.0, 10_000.0, 8.0);
    let z = measure(&p, "zin", &truth, &sealed, &grid, impedance_noise(71));
    let pd = measure(
        &p,
        "p_drp",
        &truth,
        &sealed,
        &grid,
        Noise {
            seed: 72,
            level_db: 0.1,
            seatings: 5,
            averaging: Averaging::Complex,
            repositioning_db: 0.2,
            microphone_offset_db: 0.2,
            coupler_db: 0.1,
            ..Noise::default()
        },
    );
    let mut spec = FitSpec::new(
        params(&truth),
        vec![CurveSpec::new("zin", z), CurveSpec::new("p_drp", pd)],
    );
    spec.overrides = ov(&sealed);
    let rep = fit::fit(&p, &spec).unwrap();
    assert!(rep.converged, "{:?}", rep.stop);
    for (n, t) in &truth[..2] {
        recovered(&rep, n, *t);
    }
    recovered(&rep, "driver_Re_ohm", 31.8);
    // In the sealed cup the air spring is about 100 times stiffer than the
    // suspension, so the free-air fs, Qms and Qes are not separately
    // identifiable: raising all three together changes only Cms. The report
    // names that direction; Qes/fs (∝ Mms·Re/Bl²) and Qms/fs (∝ Mms/Rms)
    // are what the data fix.
    let d = rep
        .identifiability
        .directions
        .iter()
        .find(|d| d.components.len() == 3 && d.status != Level::Determined)
        .expect("the fs-Qms-Qes direction");
    let names: Vec<&str> = d.components.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["driver_fs_Hz", "driver_Qms", "driver_Qes"]);
    // Nearly 1 : 1 : 1; not exactly, since the air spring is finite.
    for c in &d.components {
        assert!((c.weight - 1.0 / 3f64.sqrt()).abs() < 0.1, "{d:?}");
    }
    for n in ["driver_fs_Hz", "driver_Qms", "driver_Qes"] {
        assert!(!param(&rep, n).status.is_determined(), "{n}");
    }
    // Qes/fs and Qms/fs, with the standard deviations of their logarithms
    // from the reported sds and correlation: the truth within 2.576 of
    // them (99 %). Over five seeds the sd is about 0.8 % for Qes/fs and 5 %
    // for Qms/fs; a fixed tolerance would hold for some seeds only.
    let v = |n: &str| param(&rep, n).value;
    let at = |n: &str| rep.correlation.names.iter().position(|x| x == n).unwrap();
    let ratio_sd = |a: &str, b: &str| {
        let (sa, sb) = (param(&rep, a).sd.unwrap(), param(&rep, b).sd.unwrap());
        let rho = rep.correlation.matrix[at(a)][at(b)].unwrap();
        (sa * sa + sb * sb - 2.0 * rho * sa * sb).sqrt()
    };
    for (q, truth, sd_max) in [("driver_Qes", 0.95, 0.015), ("driver_Qms", 3.0, 0.1)] {
        let sd = ratio_sd(q, "driver_fs_Hz");
        let z = (v(q) / v("driver_fs_Hz") / (truth / 90.0)).ln() / sd;
        assert!(sd < sd_max && z.abs() < 2.576, "{q}/fs: sd {sd}, z {z}");
    }
    // The pressure residuals carry the smooth coupler and repositioning
    // errors: flagged as structured, with inflated intervals.
    assert!(rep.curves[1].structured);
    assert!(rep.curves[1].inflation > 1.0);
    let fitted = rep.fitted.as_object().unwrap();
    assert_eq!(fitted.len(), 6);
}

// ----- Robustness -------------------------------------------------------------

#[test]
fn failing_trial_points_are_rejected_not_fatal() {
    // The series resistor is sqrt(x − 30) ohm: the network cannot be built
    // for x < 30, and the path from the start crosses that region.
    let text = r#"{
      "parameters": {"x": {"value": 40, "min": 1, "max": 100}},
      "sweep": {"frequencies_Hz": [100, 1000]},
      "nodes": [{"id": "a", "domain": "electrical"}],
      "elements": [
        {"id": "src", "type": "vsource", "node": "a"},
        {"id": "r", "type": "resistor", "node": "a", "R_ohm": "=sqrt(x - 30) + 0.2"}
      ],
      "probes": [{"id": "z", "quantity": "impedance", "element": "src"}]
    }"#;
    let p = Parametric::parse(text).unwrap();
    // Truth x = 30.05: R = 0.4236 ohm.
    let z = Curve::new(
        Quantity::Impedance,
        &[100.0, 1000.0],
        &[0.05f64.sqrt() + 0.2; 2],
        Some(&[0.0, 0.0]),
    )
    .unwrap();
    let rep = fit::fit(
        &p,
        &FitSpec::new(vec![FitParam::new("x")], vec![CurveSpec::new("z", z)]),
    )
    .unwrap();
    assert!(
        rep.failed_evaluations > 0,
        "the path should have met the failing region"
    );
    assert!(rep.converged);
    assert!((param(&rep, "x").value - 30.05).abs() < 1e-6);
}

#[test]
fn multi_start_escapes_a_local_minimum() {
    let p = Parametric::parse(PRIMARY).unwrap();
    let truth = [
        ("fs_Hz", 60.0),
        ("Qms", 2.0),
        ("Qes", 1.5),
        ("Re_ohm", 30.0),
    ];
    let grid = exchange_grid(10.0, 20_000.0, 6.0);
    let z = measure(&p, "zin", &truth, &[], &grid, impedance_noise(3));
    // From a resonance at 300 Hz with a hump too small to see (Qes at 9,
    // near its maximum of 10), the magnitude alone leads the optimiser to
    // flatten the hump further, into the bounds of Qms and Qes.
    let start = vec![
        FitParam {
            start: Some(300.0),
            ..FitParam::new("fs_Hz")
        },
        FitParam {
            start: Some(14.0),
            ..FitParam::new("Qms")
        },
        FitParam {
            start: Some(9.0),
            ..FitParam::new("Qes")
        },
        FitParam::new("Re_ohm"),
    ];
    let mut cs = CurveSpec::new("zin", z);
    cs.use_phase = Some(false);
    let mut spec = FitSpec::new(start, vec![cs]);
    spec.max_iterations = 30;
    let one = fit::fit(&p, &spec).unwrap();
    assert!(
        one.reduced_chi2.unwrap() > 100.0,
        "the nominal start should be trapped"
    );
    assert!(one.parameters.iter().any(|p| p.status == Status::AtBound));
    spec.starts = 6;
    spec.seed = 5;
    let many = fit::fit(&p, &spec).unwrap();
    assert_eq!(many.starts.len(), 6);
    for (n, t) in truth {
        recovered(&many, n, t);
    }
    assert!(many.cost < 0.01 * one.cost);
}

#[test]
fn a_weakly_sensitive_start_far_from_the_optimum_still_converges() {
    // Over-ear template, drum response only: the pad leak starts nearly
    // closed (0.005 mm against 0.12 mm), where the response hardly depends
    // on it (singular value 0.02, below MIN_SIGMA). Frozen there, the fit
    // stopped at once and reported convergence at a reduced chi-square of
    // 350; above GROSS_CHI2 it steps along the flat direction too.
    let p = example("design_over_ear.json");
    let grid = exchange_grid(20.0, 10_000.0, 8.0);
    let pd = measure(
        &p,
        "p_drp",
        &[("leak_gap_mm", 0.12)],
        &[],
        &grid,
        Noise {
            seed: 5,
            level_db: 0.1,
            ..Noise::default()
        },
    );
    let spec = FitSpec::new(
        vec![FitParam {
            start: Some(0.005),
            ..FitParam::new("leak_gap_mm")
        }],
        vec![CurveSpec::new("p_drp", pd)],
    );
    let rep = fit::fit(&p, &spec).unwrap();
    assert!(rep.converged, "{:?}", rep.stop);
    assert!(rep.reduced_chi2.unwrap() < 2.0, "{:?}", rep.reduced_chi2);
    recovered(&rep, "leak_gap_mm", 0.12);
}

#[test]
fn a_sensor_calibration_error_is_one_offset_not_point_noise() {
    // A laser displacement curve whose budget states a 0.5 dB sensor
    // calibration uncertainty. Its absolute level is what fixes the scale
    // (Bl), so Bl can be known no better than 0.5 dB allows: sd of ln Bl
    // at least 0.5·ln 10/20 = 0.058. Weighted as independent noise at every
    // point, the calibration averaged down over the points: Bl was reported
    // within ±1.2 % while off by up to 8 %, covering the truth in 6 of 30
    // seeds.
    let p = example("driver_bench.json");
    let n = 12;
    let mut covered = 0;
    for seed in 0..n {
        let z = measure(
            &p,
            "zin",
            &BENCH_TRUTH,
            &[],
            &exchange_grid(10.0, 20_000.0, 12.0),
            impedance_noise(100 + seed),
        );
        let x = measure(
            &p,
            "x",
            &BENCH_TRUTH,
            &[],
            &exchange_grid(10.0, 2000.0, 12.0),
            Noise {
                seed: 300 + seed,
                level_db: 0.1,
                phase_deg: 0.5,
                microphone_offset_db: 0.5,
                ..Noise::default()
            },
        );
        let rep = fit::fit(
            &p,
            &FitSpec::new(
                params(&BENCH_TRUTH),
                vec![CurveSpec::new("zin", z), CurveSpec::new("x", x)],
            ),
        )
        .unwrap();
        assert_eq!(rep.offsets.len(), 1);
        assert_eq!(rep.offsets[0].prior_db, Some(0.5));
        let bl = param(&rep, "Bl_Tm");
        let sd = bl.sd.unwrap();
        assert!(sd > 0.9 * 0.5 * std::f64::consts::LN_10 / 20.0, "sd {sd}");
        let [lo, hi] = bl.ci95.unwrap();
        if lo <= 2.5 && 2.5 <= hi {
            covered += 1;
        }
    }
    assert!(covered >= 10, "Bl covered in {covered} of {n} fits");
}

#[test]
fn a_signed_parameter_near_zero_is_judged_on_its_own_scale() {
    // R = 10 + x ohm with x in [−5, 5] and the truth x = 0: relative to |x|
    // every interval is infinitely wide (and a value of exactly 0 made the
    // direction look null); relative to 1 % of the range the fit determines
    // x to a few milliohm.
    let text = r#"{
      "parameters": {"x": {"value": 1, "min": -5, "max": 5},
                     "L_mH": {"value": 1, "min": 0.1, "max": 10}},
      "sweep": {"frequencies_Hz": [100, 1000]},
      "nodes": [{"id": "a", "domain": "electrical"}, {"id": "b", "domain": "electrical"}],
      "elements": [
        {"id": "src", "type": "vsource", "node": "a"},
        {"id": "r", "type": "resistor", "nodes": ["a", "b"], "R_ohm": "=10 + x"},
        {"id": "l", "type": "inductor", "node": "b", "L_mH": "=L_mH"}
      ],
      "probes": [{"id": "z", "quantity": "impedance", "element": "src"}]
    }"#;
    let p = Parametric::parse(text).unwrap();
    let z = measure(
        &p,
        "z",
        &[("x", 0.0), ("L_mH", 2.0)],
        &[],
        &exchange_grid(100.0, 10_000.0, 12.0),
        Noise {
            seed: 1,
            level_db: 0.01,
            phase_deg: 0.05,
            ..Noise::default()
        },
    );
    let rep = fit::fit(
        &p,
        &FitSpec::new(
            vec![FitParam::new("x"), FitParam::new("L_mH")],
            vec![CurveSpec::new("z", z)],
        ),
    )
    .unwrap();
    let x = param(&rep, "x");
    assert_eq!(x.scale, "linear");
    assert_eq!(x.status, Status::Determined, "{x:?}");
    let [lo, hi] = x.ci95.unwrap();
    assert!(lo <= 0.0 && 0.0 <= hi && hi - lo < 0.02, "[{lo}, {hi}]");
    recovered(&rep, "L_mH", 2.0);
}

// ----- Specification and conditions ------------------------------------------

#[test]
fn fit_spec_json_is_validated() {
    let p = example("driver_bench.json");
    let z = measure(
        &p,
        "zin",
        &BENCH_TRUTH,
        &[],
        &exchange_grid(10.0, 20_000.0, 6.0),
        impedance_noise(1),
    );
    let good = json!({
        "schema": "acoustilab-fit/0.1",
        "parameters": ["Re_ohm", {"name": "Bl_Tm", "start": 2.4, "min": 1, "max": 5, "scale": "log"}],
        "overrides": {"Mms_g": 0.33, "Cms_mm_per_N": 11, "Rms_Ns_per_m": 0.07},
        "curves": [{"probe": "zin", "curve": z.to_json(), "weight": 1, "use_phase": true,
                    "f_min_Hz": 10, "f_max_Hz": 20000}],
        "f_min_Hz": 10, "f_max_Hz": 20000, "max_iterations": 50, "max_evaluations": 2000,
        "starts": 1, "seed": 3
    });
    let spec = FitSpec::from_json(&good).unwrap();
    let rep = fit::fit(&p, &spec).unwrap();
    recovered(&rep, "Re_ohm", 31.5);
    recovered(&rep, "Bl_Tm", 2.5);
    let err = |edit: &dyn Fn(&mut Value)| -> String {
        let mut v = good.clone();
        edit(&mut v);
        match FitSpec::from_json(&v).and_then(|s| fit::fit(&p, &s)) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("accepted {v}"),
        }
    };
    assert!(err(&|v| v["tolerance"] = json!(1)).contains("unknown key 'tolerance'"));
    assert!(err(&|v| v["parameters"][0] = json!("Rx_ohm")).contains("not declared"));
    assert!(
        err(&|v| v["parameters"][0] = json!({"name": "box_volume_cm3", "scale": "log"}))
            .contains("a log-scale parameter needs a positive start")
    );
    assert!(
        err(&|v| v["parameters"][0] = json!({"name": "Re_ohm", "min": 0.5}))
            .contains("below the declared minimum")
    );
    assert!(
        err(&|v| v["parameters"][0] = json!({"name": "Re_ohm", "max": 20}))
            .contains("outside its bounds")
    );
    assert!(
        err(&|v| v["parameters"][0] = json!({"name": "Re_ohm", "scale": "exp"})).contains("scale")
    );
    assert!(err(&|v| v["overrides"]["Re_ohm"] = json!(30)).contains("both fitted and fixed"));
    assert!(err(&|v| v["curves"][0]["probe"] = json!("x"))
        .contains("the curve is impedance but the probe reads displacement"));
    assert!(err(&|v| v["curves"][0]["probe"] = json!("nope")).contains("no such probe"));
    assert!(err(&|v| v["curves"][0]["offset"] = json!("maybe")).contains("offset"));
    assert!(err(&|v| v["curves"][0]["weight"] = json!(0)).contains("weight"));
    assert!(err(&|v| v["curves"][0]["f_max_Hz"] = json!(10.5)).contains("fewer than 2 points"));
    assert!(err(&|v| v["parameters"] = json!([])).contains("no parameters"));
    // The Latin hypercube is drawn before any start runs: its size is
    // bounded (a wasm worker would abort on the allocation).
    assert!(err(&|v| v["starts"] = json!(1_000_000_000_000u64)).contains("at most 1000"));
    // A compensated curve is not the model's probe, unless allowed.
    let compensated =
        |v: &mut Value| v["curves"][0]["curve"]["sidecar"]["compensation"] = json!("diffuse_field");
    assert!(err(&compensated).contains("compensated ('diffuse_field')"));
    let mut v = good.clone();
    compensated(&mut v);
    v["curves"][0]["allow"] = json!(["compensation"]);
    assert!(fit::fit(&p, &FitSpec::from_json(&v).unwrap()).is_ok());
    assert!(err(&|v| v["curves"][0]["allow"] = json!(["fixture"])).contains("only 'compensation'"));
    // Integers, choices and derived parameters cannot be fitted.
    let t = example("design_over_ear.json");
    for name in ["vent_count", "ear", "front_volume_cm3"] {
        let spec = FitSpec::new(
            vec![FitParam::new(name)],
            vec![CurveSpec::new("zin", z.clone())],
        );
        let e = fit::fit(&t, &spec).unwrap_err().to_string();
        assert!(e.contains("not a continuous number"), "{name}: {e}");
    }
}

#[test]
fn pressure_curves_without_a_drive_or_calibration_get_a_level_offset() {
    let p = example("driver_bench.json");
    let grid = exchange_grid(20.0, 2000.0, 12.0);
    let mut pb = measure(
        &p,
        "p_box",
        &BENCH_TRUTH,
        &[("box_volume_cm3", 20.0)],
        &grid,
        Noise {
            seed: 81,
            level_db: 0.05,
            ..Noise::default()
        },
    );
    // Shift the level by 3 dB and forget the drive: only the shape counts.
    for m in &mut pb.magnitude {
        *m *= 10f64.powf(3.0 / 20.0);
    }
    pb.sidecar.drive = None;
    let mut cs = CurveSpec::new("p_box", pb.clone());
    cs.overrides = ov(&[("box_volume_cm3", 20.0)]);
    let mut spec = FitSpec::new(vec![FitParam::new("Rms_Ns_per_m")], vec![cs.clone()]);
    spec.overrides = ov(&[
        ("Re_ohm", 31.5),
        ("Bl_Tm", 2.5),
        ("Mms_g", 0.33),
        ("Cms_mm_per_N", 11.0),
    ]);
    spec.allow_spl_only = true;
    let rep = fit::fit(&p, &spec).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("states no drive")));
    assert_eq!(rep.offsets.len(), 1);
    let o = &rep.offsets[0];
    let [lo, hi] = o.ci95_db.unwrap();
    assert!(lo <= 3.0 && 3.0 <= hi, "{o:?}");
    recovered(&rep, "Rms_Ns_per_m", 0.07);
    // With a prior on the offset (a microphone calibration uncertainty) the
    // prior residual is part of the fit.
    cs.offset = Some(Offset::Prior(5.0));
    spec.curves = vec![cs];
    let rep = fit::fit(&p, &spec).unwrap();
    assert_eq!(rep.offsets[0].prior_db, Some(5.0));
}

// ----- The virtual rig ------------------------------------------------------------

#[test]
fn virtual_rig_is_deterministic_and_says_what_it_is() {
    let p = example("driver_bench.json");
    let grid = exchange_grid(10.0, 20_000.0, 12.0);
    let noise = Noise {
        seed: 5,
        level_db: 0.1,
        phase_deg: 0.5,
        seatings: 3,
        averaging: Averaging::Magnitude,
        ..Noise::default()
    };
    let a = measure(&p, "zin", &BENCH_TRUTH, &[], &grid, noise.clone());
    let b = measure(&p, "zin", &BENCH_TRUTH, &[], &grid, noise.clone());
    assert_eq!(a, b);
    let c = measure(
        &p,
        "zin",
        &BENCH_TRUTH,
        &[],
        &grid,
        Noise { seed: 6, ..noise },
    );
    assert_ne!(a.magnitude, c.magnitude);
    let sc = &a.sidecar;
    assert_eq!(
        sc.provenance.as_ref().unwrap().origin,
        Some(Origin::VirtualRig)
    );
    assert_eq!(sc.seatings, Some(3));
    assert_eq!(sc.averaging, Some(Averaging::Magnitude));
    let rig = sc.virtual_rig.as_ref().unwrap();
    assert_eq!(rig["true_parameters"]["Bl_Tm"], json!(2.5));
    assert_eq!(rig["noise"]["seed"], json!(5));
    let u = sc.uncertainty.as_ref().unwrap();
    assert!((u.level_db(100.0, 3).unwrap() - 0.1 / 3f64.sqrt()).abs() < 1e-15);
    // Without noise the rig reproduces the solver, and states no budget.
    let exact = measure(&p, "zin", &BENCH_TRUTH, &[], &grid, Noise::default());
    assert!(exact.sidecar.uncertainty.is_none());
    let mut c = Circuit::from_parametric(&p, &ov(&BENCH_TRUTH)).unwrap();
    c.freqs = grid.clone();
    let r = c.solve().unwrap();
    for (z, m) in r.probe("zin").unwrap().values.iter().zip(&exact.magnitude) {
        assert!((z.norm() / m - 1.0).abs() < 1e-14);
    }
    // Written as a real rig would: ZMA plus a sidecar file, read back.
    let text = text::export(&a, Format::Zma).unwrap();
    let mut back = text::import(&text, Format::Zma, None).unwrap();
    back.sidecar = acoustilab::io::Sidecar::parse(&a.sidecar.to_text()).unwrap();
    assert_eq!(back.sidecar, a.sidecar);
    for (x, y) in back.magnitude.iter().zip(&a.magnitude) {
        assert!((x - y).abs() <= 5.1e-7);
    }
}

#[test]
fn complex_averaging_loses_treble_by_the_delay_spread() {
    // Seatings that differ only in a delay τ (standard deviation σ): the
    // vector mean of e^(−jωτ) tends to exp(−(ωσ)²/2), the power mean of the
    // magnitudes stays |H|.
    let p = example("driver_bench.json");
    let grid = exchange_grid(100.0, 8000.0, 3.0);
    let sigma_us = 20.0;
    let base = Noise {
        seed: 9,
        seatings: 4000,
        repositioning_delay_us: sigma_us,
        ..Noise::default()
    };
    let truth = measure(
        &p,
        "p_box",
        &BENCH_TRUTH,
        &[("box_volume_cm3", 20.0)],
        &grid,
        Noise::default(),
    );
    let cplx = measure(
        &p,
        "p_box",
        &BENCH_TRUTH,
        &[("box_volume_cm3", 20.0)],
        &grid,
        Noise {
            averaging: Averaging::Complex,
            ..base.clone()
        },
    );
    let mag = measure(
        &p,
        "p_box",
        &BENCH_TRUTH,
        &[("box_volume_cm3", 20.0)],
        &grid,
        Noise {
            averaging: Averaging::Magnitude,
            ..base
        },
    );
    for (i, f) in grid.iter().enumerate() {
        let w = 2.0 * std::f64::consts::PI * f * sigma_us * 1e-6;
        let expect = (-w * w / 2.0).exp();
        let got = cplx.magnitude[i] / truth.magnitude[i];
        // Standard error of the mean of 4000 unit phasors: below 0.012.
        assert!((got - expect).abs() < 0.04, "{f} Hz: {got} vs {expect}");
        assert!((mag.magnitude[i] / truth.magnitude[i] - 1.0).abs() < 1e-12);
    }
    // At 8 kHz the expected loss is 1.1 dB.
    let last = grid.len() - 1;
    assert!(cplx.magnitude[last] / truth.magnitude[last] < 0.9);
}

#[test]
fn rig_json_spec() {
    let p = example("driver_bench.json");
    let spec = RigSpec::from_json(&json!({
        "probe": "zin", "overrides": {"Bl_Tm": 2.5},
        "f_min_Hz": 20, "f_max_Hz": 2000, "points_per_octave": 6,
        "noise": {"seed": 4, "level_dB": 0.1, "phase_deg": 1, "seatings": 2, "averaging": "db"},
        "sidecar": {"fixture": "free air", "device": "unit 7"}
    }))
    .unwrap();
    let c = rig::measure(&p, &spec).unwrap();
    assert_eq!(c.freqs_hz, exchange_grid(20.0, 2000.0, 6.0));
    assert_eq!(c.sidecar.fixture.as_deref(), Some("free air"));
    assert_eq!(c.sidecar.averaging, Some(Averaging::Db));
    for bad in [
        json!({"probe": "zin", "nosie": {}}),
        json!({"probe": "zin", "noise": {"level_dB": -1}}),
        json!({"probe": "zin", "noise": {"seatings": 3, "averaging": "none"}}),
        json!({"probe": "zin", "frequencies_Hz": [10, 20], "f_min_Hz": 5}),
        json!({"overrides": {}}),
    ] {
        assert!(RigSpec::from_json(&bad).is_err(), "{bad}");
    }
    assert!(rig::measure(&p, &RigSpec::new("nope")).is_err());
    // An empty or one-point grid is an error, not a panic (the CLI's
    // `measure --ppo 0` gave one).
    for freqs in [vec![], vec![100.0, 100.0], vec![-1.0, 100.0]] {
        let mut s = RigSpec::new("zin");
        s.freqs = freqs;
        assert!(rig::measure(&p, &s).is_err());
    }
    // Seatings times points bound one call's run time.
    let mut s = RigSpec::new("zin");
    s.freqs = exchange_grid(10.0, 20_000.0, 2000.0);
    s.noise.seatings = 10_000;
    s.noise.averaging = Averaging::Complex;
    let e = rig::measure(&p, &s).unwrap_err().to_string();
    assert!(e.contains("the limit of one call"), "{e}");
}

/// Cost of the case-study fit on three grid densities; run with
/// `cargo test --release -p acoustilab --test fit -- --ignored --nocapture`.
#[test]
#[ignore]
fn fit_cost() {
    let p = example("design_over_ear.json");
    let truth = [
        ("leak_gap_mm", 0.12),
        ("front_depth_mm", 13.0),
        ("driver_fs_Hz", 90.0),
        ("driver_Qms", 3.0),
        ("driver_Qes", 0.95),
        ("driver_Re_ohm", 31.8),
    ];
    for ppo in [8.0, 24.0, 48.0] {
        let grid = exchange_grid(20.0, 10_000.0, ppo);
        let z = measure(&p, "zin", &truth, &[], &grid, impedance_noise(71));
        let pd = measure(&p, "p_drp", &truth, &[], &grid, impedance_noise(72));
        let spec = FitSpec::new(
            params(&truth),
            vec![CurveSpec::new("zin", z), CurveSpec::new("p_drp", pd)],
        );
        let t = std::time::Instant::now();
        let rep = fit::fit(&p, &spec).unwrap();
        let dt = t.elapsed().as_secs_f64();
        println!(
            "over-ear template, {} points per curve, 6 parameters: {:.3} s, {} evaluations ({:.2} ms each), {} iterations",
            grid.len(),
            dt,
            rep.evaluations,
            dt / rep.evaluations as f64 * 1e3,
            rep.iterations
        );
    }
}
