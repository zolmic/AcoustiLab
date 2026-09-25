//! Targets, smoothing, metrics and preference scores (docs/targets.md).
//!
//! Reference values come from an independent Python implementation,
//! `tools/targets/reference.py` (numpy, scipy.integrate.quad for smoothing,
//! scipy.signal.freqs for the shelves, scipy.stats.linregress for slopes),
//! and from AutoEq itself (`tools/targets/autoeq_crosscheck.py`). Both
//! implementations work in double precision with different summation and
//! integration schemes, so values agree to 1e-9 dB (the quadrature is exact
//! to about 3e-14 dB on these curves); the tolerance is set well above that
//! and far below anything that matters.

use acoustilab::targets::curve::Curve;
use acoustilab::targets::fixture::{self, Match};
use acoustilab::targets::import::import_target_csv;
use acoustilab::targets::metrics::{self, bs708_limit, evaluate, Normalisation, Options, Response};
use acoustilab::targets::scores;
use acoustilab::targets::shelf::{high_shelf_db, low_shelf_db};
use acoustilab::targets::smoothing::{smooth_complex, smooth_power};
use acoustilab::targets::target::{
    self, preference_band, recipe_preset, reconstruct_harman_style, BandParams, Shelves, Target,
};
use acoustilab::targets::{grid, TargetError};
use acoustilab::{Circuit, C64};
use serde_json::{json, Value};

const TOL: f64 = 1e-9;

fn reference() -> Value {
    serde_json::from_str(include_str!("data/targets_reference.json")).unwrap()
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

fn opts(v: &Value) -> Vec<Option<f64>> {
    v.as_array().unwrap().iter().map(Value::as_f64).collect()
}

fn curve(v: &Value) -> Curve {
    Curve::new(f64s(&v["f"]), f64s(&v["dB"])).unwrap()
}

fn close(a: f64, b: f64, tol: f64, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tol {tol})");
}

fn close_opt(a: &[Option<f64>], b: &[Option<f64>], tol: f64, what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: length");
    for (k, (x, y)) in a.iter().zip(b).enumerate() {
        match (x, y) {
            (Some(x), Some(y)) => close(*x, *y, tol, &format!("{what}[{k}]")),
            (None, None) => {}
            _ => panic!("{what}[{k}]: {x:?} vs {y:?}"),
        }
    }
}

/// A target object around a curve, for tests.
fn user_target(name: &str, fixture: &str, family: &str, c: &Curve, valid: (f64, f64)) -> Target {
    Target::from_json(&json!({
        "name": name, "fixture": fixture, "family": family,
        "frequencies_Hz": c.freqs(), "dB": c.db(),
        "valid_range_Hz": [valid.0, valid.1],
    }))
    .unwrap()
}

// ------------------------------------------------------------------ grid

#[test]
fn grid_is_the_exact_twelfth_octave_series() {
    let g = grid::twelfth_octave();
    let r = reference();
    assert_eq!(g.len(), 121);
    for (a, b) in g.iter().zip(f64s(&r["grid"])) {
        close(*a / b, 1.0, 1e-15, "grid");
    }
    for w in g.windows(2) {
        close(w[1] / w[0], 10f64.powf(0.025), 1e-14, "grid ratio");
    }
    // 1/12 octave in base 10: the IEC 61260-1 octave ratio G = 10^0.3.
    close(10f64.powf(12.0 / 40.0), grid::G, 1e-15, "G");
    for f in [100.0, 1000.0, 10000.0] {
        let k = grid::nearest(&g, f);
        close(g[k] / f, 1.0, 1e-14, "decade points on the grid");
    }
    close(g[0], 19.952_623_149_688_8, 1e-9, "first point");
    close(g[120], 19_952.623_149_688_8, 1e-7, "last point");
}

#[test]
fn band_membership_within_half_a_step() {
    let g = grid::twelfth_octave();
    let r = reference();
    for (key, v) in r["band_indices"].as_object().unwrap() {
        let (lo, hi) = key.split_once('-').unwrap();
        let b = grid::band(&g, lo.parse().unwrap(), hi.parse().unwrap());
        assert_eq!(b.start as u64, v[0].as_u64().unwrap(), "{key} start");
        assert_eq!((b.end - 1) as u64, v[1].as_u64().unwrap(), "{key} end");
    }
    // Nominal-frequency membership: 20 Hz selects 19.95 Hz, 16 kHz 15.85 kHz,
    // and each edge's grid point is also the nearest one.
    let b = grid::band(&g, 20.0, 16000.0);
    close(g[b.start], 19.95, 0.01, "20 Hz edge");
    close(g[b.end - 1], 15848.9, 0.1, "16 kHz edge");
    for (lo, hi) in [
        (20.0, 10000.0),
        (40.0, 8000.0),
        (50.0, 16000.0),
        (200.0, 500.0),
    ] {
        let b = grid::band(&g, lo, hi);
        assert_eq!(b.start, grid::nearest(&g, lo));
        assert_eq!(b.end - 1, grid::nearest(&g, hi));
    }
    // A coarse grid never lends a band points far outside it.
    let coarse = [20.0, 50.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0];
    assert_eq!(grid::band(&coarse, 2000.0, 8000.0), 5..6);
    assert_eq!(grid::band(&coarse, 2000.0, 3000.0), 5..5);
    assert!(!grid::covers(5000.0, 5000.0, 2000.0, 8000.0));
}

// ---------------------------------------------------------------- curves

#[test]
fn interpolation_is_linear_in_db_on_log_frequency() {
    let r = reference();
    let g = grid::twelfth_octave();
    let a = curve(&r["curves"]["a"]);
    close_opt(
        &a.sample(&g),
        &opts(&r["interp_a_on_grid"]),
        1e-12,
        "a on grid",
    );
    let t = curve(&r["curves"]["target_third"]);
    close_opt(
        &t.sample(&g),
        &opts(&r["interp_third_on_grid"]),
        1e-12,
        "third on grid",
    );
    let p = &r["interp_third_probes"];
    close_opt(&t.sample(&f64s(&p["f"])), &opts(&p["dB"]), 1e-12, "probes");
    // Closed form: halfway on a log axis between two points is their mean.
    let c = Curve::new(vec![100.0, 400.0], vec![0.0, 6.0]).unwrap();
    close(c.at(200.0).unwrap(), 3.0, 1e-12, "log midpoint");
    // Up to 1.3 % beyond an end the curve reads its end value (the gap
    // between a nominal preferred frequency and the exact grid point it
    // names); further out it is undefined.
    assert_eq!(c.at(98.8), Some(0.0));
    assert_eq!(c.at(405.0), Some(6.0));
    assert_eq!(c.at(98.6), None);
    assert_eq!(c.at(405.4), None);
}

/// The grid point named 20 Hz is 19.95 Hz. A response and a target that
/// both run from 20 Hz to 20 kHz, the range the Harman models are defined
/// on, must cover the whole grid: no metric is partial and no score is
/// greyed for range.
#[test]
fn curves_from_20_hz_cover_the_grid() {
    let f: Vec<f64> = (0..=480)
        .map(|k| 20.0 * 1000f64.powf(k as f64 / 480.0))
        .collect();
    let t: Vec<f64> = f.iter().map(|x| 2.0 * (x / 700.0).ln().sin()).collect();
    let r: Vec<f64> = f
        .iter()
        .zip(&t)
        .map(|(x, y)| 95.0 + y + 0.3 * (x / 300.0).ln().cos())
        .collect();
    let target = user_target(
        "t",
        "iec60318_4_canal_extender",
        "harman_ie_2017",
        &Curve::new(f.clone(), t).unwrap(),
        (20.0, 20000.0),
    );
    let resp = Response {
        fixture: Some("iec60318_4_canal_extender".into()),
        ..response(&Curve::new(f, r).unwrap())
    };
    let rep = evaluate(
        &resp,
        &target,
        &Options {
            measured: true,
            ..Options::default()
        },
    )
    .unwrap();
    let g = grid::twelfth_octave();
    assert!(rep.error.iter().all(Option::is_some), "{:?}", rep.error);
    let m = rep.main.as_ref().unwrap();
    assert!(!m.partial);
    close(m.used_hz.unwrap().0, g[0], 1e-12, "first point used");
    assert!(rep.flags.iter().all(|f| f.code != "partial_range"));
    for id in ["harman_ie_patent", "harman_ie_listen"] {
        let s = rep.score(id).unwrap();
        assert!(!s.greyed, "{id}: {:?}", s.flags);
    }
}

#[test]
fn curves_reject_malformed_input() {
    let bad = [
        (vec![100.0], vec![1.0], "at least two"),
        (vec![100.0, 100.0], vec![1.0, 2.0], "increase strictly"),
        (vec![200.0, 100.0], vec![1.0, 2.0], "increase strictly"),
        (vec![0.0, 100.0], vec![1.0, 2.0], "positive"),
        (vec![10.0, 100.0], vec![1.0, f64::NAN], "not finite"),
        (vec![10.0, 100.0], vec![1.0], "levels"),
    ];
    for (f, db, msg) in bad {
        let e = Curve::new(f, db).unwrap_err();
        assert!(e.to_string().contains(msg), "{e}");
    }
}

// ------------------------------------------------------------- smoothing

#[test]
fn power_smoothing_matches_the_quadrature_reference() {
    let r = reference();
    let a = curve(&r["curves"]["a"]);
    for (n, v) in r["smoothing"].as_object().unwrap() {
        let s = smooth_power(&a, n.parse().unwrap()).unwrap();
        for (k, (x, y)) in s.db().iter().zip(f64s(v)).enumerate() {
            close(*x, y, TOL, &format!("1/{n} octave, point {k}"));
        }
    }
    let t = curve(&r["curves"]["target_third"]);
    let s = smooth_power(&t, 3).unwrap();
    for (x, y) in s.db().iter().zip(f64s(&r["smoothing_third_target_3"])) {
        close(*x, y, TOL, "sparse curve");
    }
}

/// A dense FFT-like curve (linear frequency spacing, so an octave window
/// holds thousands of points) with an 80 dB deep notch: the smoothed levels
/// equal a direct integration of the piecewise-linear power over each
/// window, piece by piece, to a relative 1e-12 of the power (4e-12 dB), also
/// in windows that contain little but the notch.
#[test]
fn power_smoothing_of_a_dense_curve_equals_direct_integration() {
    let n = 6000;
    let f: Vec<f64> = (1..=n).map(|k| k as f64 * 4.0).collect();
    let db: Vec<f64> = f
        .iter()
        .map(|&x| {
            let notch = (x / 3000.0).log2() / 0.02;
            90.0 + 6.0 * (x / 150.0).ln().sin() - 80.0 * (-0.5 * notch * notch).exp()
        })
        .collect();
    let c = Curve::new(f.clone(), db).unwrap();
    let x: Vec<f64> = f.iter().map(|v| v.log10()).collect();
    let p: Vec<f64> = c.db().iter().map(|l| 10f64.powf(l / 10.0)).collect();
    let direct = |i: usize, h: f64| -> f64 {
        let h = h.min(x[i] - x[0]).min(x[n - 1] - x[i]);
        if h <= 0.0 {
            return c.db()[i];
        }
        let (a, b) = (x[i] - h, x[i] + h);
        let at = |j: usize, u: f64| p[j] + (p[j + 1] - p[j]) * (u - x[j]) / (x[j + 1] - x[j]);
        let mut s = 0.0;
        for j in 0..n - 1 {
            let (u0, u1) = (a.max(x[j]), b.min(x[j + 1]));
            if u1 > u0 {
                s += (u1 - u0) * 0.5 * (at(j, u0) + at(j, u1));
            }
        }
        10.0 * (s / (b - a)).log10()
    };
    for frac in [1, 3, 48] {
        let s = smooth_power(&c, frac).unwrap();
        let h = 3.0 / (20.0 * frac as f64);
        for i in (0..n)
            .step_by(37)
            .chain([1, 2, n - 2, n - 1, 749, 750, 751])
        {
            close(
                s.db()[i],
                direct(i, h),
                4e-12,
                &format!("1/{frac}, {} Hz", f[i]),
            );
        }
    }
}

#[test]
fn power_smoothing_invariants() {
    let f: Vec<f64> = (0..300)
        .map(|k| 20.0 * 2f64.powf(k as f64 / 30.0))
        .collect();
    // A constant is unchanged.
    let c = Curve::new(f.clone(), vec![87.5; f.len()]).unwrap();
    for n in [1, 3, 48] {
        for v in smooth_power(&c, n).unwrap().db() {
            close(*v, 87.5, 1e-12, "constant");
        }
    }
    // Power linear in log f is unchanged by a symmetric window.
    let db: Vec<f64> = f
        .iter()
        .map(|x| 10.0 * (2.0 + 0.7 * x.log10()).log10())
        .collect();
    let l = Curve::new(f.clone(), db.clone()).unwrap();
    for n in [1, 6] {
        for (v, w) in smooth_power(&l, n).unwrap().db().iter().zip(&db) {
            close(*v, *w, 1e-12, "linear power");
        }
    }
    // End points are left as they are (the window shrinks to zero).
    let a = Curve::new(f.clone(), f.iter().map(|x| (x / 100.0).sin()).collect()).unwrap();
    let s = smooth_power(&a, 1).unwrap();
    assert_eq!(s.db()[0], a.db()[0]);
    assert_eq!(s.db()[f.len() - 1], a.db()[f.len() - 1]);
    // 1/5 octave is not offered.
    let e = smooth_power(&a, 5).unwrap_err();
    assert!(matches!(e, TargetError::Options(_)), "{e}");
}

#[test]
fn complex_smoothing_matches_reference_and_keeps_linear_functions() {
    let r = reference();
    let a = curve(&r["curves"]["a"]);
    let h: Vec<C64> = a
        .freqs()
        .iter()
        .zip(a.db())
        .map(|(f, l)| {
            C64::from_polar(
                10f64.powf(l / 20.0),
                -2.0 * std::f64::consts::PI * f * 1.5e-4,
            )
        })
        .collect();
    let scale = h.iter().map(|z| z.norm()).fold(0.0, f64::max);
    for (n, v) in r["complex_smoothing"].as_object().unwrap() {
        let s = smooth_complex(a.freqs(), &h, n.parse().unwrap()).unwrap();
        for (k, (z, (re, im))) in s
            .iter()
            .zip(f64s(&v["re"]).into_iter().zip(f64s(&v["im"])))
            .enumerate()
        {
            close(z.re, re, 1e-11 * scale, &format!("re, 1/{n}, {k}"));
            close(z.im, im, 1e-11 * scale, &format!("im, 1/{n}, {k}"));
        }
    }
    // A complex function linear in log f is unchanged.
    let f: Vec<f64> = (0..100)
        .map(|k| 50.0 * 2f64.powf(k as f64 / 12.0))
        .collect();
    let z: Vec<C64> = f
        .iter()
        .map(|x| C64::new(1.0 + x.log10(), -2.0 + 0.5 * x.log10()))
        .collect();
    for (a, b) in smooth_complex(&f, &z, 3).unwrap().iter().zip(&z) {
        close((a - b).norm(), 0.0, 1e-12, "linear complex");
    }
    // Where the phase turns within the window, the complex mean falls
    // below the power mean: a pure delay of 1 ms at 1/1 octave near 5 kHz.
    let f: Vec<f64> = (0..400)
        .map(|k| 1000.0 * 2f64.powf(k as f64 / 96.0))
        .collect();
    let z: Vec<C64> = f
        .iter()
        .map(|x| C64::from_polar(1.0, -2.0 * std::f64::consts::PI * x * 1e-3))
        .collect();
    let k = 200;
    assert!(smooth_complex(&f, &z, 1).unwrap()[k].norm() < 0.2);
}

// --------------------------------------------------------------- shelves

#[test]
fn shelves_meet_their_closed_forms() {
    let q = std::f64::consts::FRAC_1_SQRT_2;
    for g in [-6.0, -1.41, 2.0, 6.44, 12.0] {
        // Half the gain at the corner, for any Q.
        for qq in [0.5, q, 1.3] {
            close(
                low_shelf_db(105.0, 105.0, g, qq),
                g / 2.0,
                1e-12,
                "low at fc",
            );
            close(
                high_shelf_db(2500.0, 2500.0, g, qq),
                g / 2.0,
                1e-12,
                "high at fc",
            );
        }
        // Asymptotes: the full gain far on the shelf side, 0 dB far on the other.
        close(low_shelf_db(105e-4, 105.0, g, q), g, 1e-6, "low DC");
        close(low_shelf_db(105e4, 105.0, g, q), 0.0, 1e-6, "low HF");
        close(high_shelf_db(2500e4, 2500.0, g, q), g, 1e-6, "high HF");
        close(high_shelf_db(2500e-4, 2500.0, g, q), 0.0, 1e-6, "high DC");
        // S = 1 (Q = 1/sqrt 2) is monotonic.
        let v: Vec<f64> = (0..400)
            .map(|k| low_shelf_db(10.0 * 2f64.powf(k as f64 / 40.0), 105.0, g, q))
            .collect();
        assert!(
            v.windows(2).all(|w| (w[1] - w[0]) * g <= 1e-12),
            "low shelf {g} dB is not monotonic"
        );
    }
    let r = reference();
    let s = &r["shelves"];
    let f = f64s(&s["f"]);
    let cases = [
        ("low_105_+6.44", false, 105.0, 6.44, q),
        ("low_105_-2_q0.5", false, 105.0, -2.0, 0.5),
        ("high_2500_-1.41", true, 2500.0, -1.41, q),
        ("high_2500_+4_q1.2", true, 2500.0, 4.0, 1.2),
    ];
    for (key, high, fc, g, qq) in cases {
        let fun = if high { high_shelf_db } else { low_shelf_db };
        for (x, y) in f.iter().zip(f64s(&s[key])) {
            close(fun(*x, fc, g, qq), y, 1e-10, key);
        }
    }
}

// ----------------------------------------------------------------- BS.708

#[test]
fn bs708_mask_has_the_figure_breakpoints() {
    for (f, t) in [
        (100.0, 2.0),
        (500.0, 1.5),
        (1000.0, 1.5),
        (4000.0, 1.5),
        (16000.0, 4.0),
    ] {
        close(bs708_limit(f).unwrap(), t, 1e-12, &format!("{f} Hz"));
    }
    // Log-linear between breakpoints (erratum E47: 1.71 dB at 250 Hz, not 2 dB).
    close(
        bs708_limit(250.0).unwrap(),
        2.0 - 0.5 * 2.5f64.ln() / 5f64.ln(),
        1e-12,
        "250 Hz",
    );
    close(
        bs708_limit(250.0).unwrap(),
        1.7153,
        1e-4,
        "250 Hz, as stated in E47",
    );
    close(bs708_limit(8000.0).unwrap(), 2.75, 1e-12, "8 kHz");
    assert_eq!(bs708_limit(90.0), None);
    assert_eq!(bs708_limit(17000.0), None);
    let r = reference();
    let g = grid::twelfth_octave();
    let lim: Vec<Option<f64>> = g.iter().map(|&f| bs708_limit(f)).collect();
    close_opt(
        &lim,
        &opts(&r["bs708_limit_on_grid"]),
        1e-12,
        "mask on grid",
    );
}

// ---------------------------------------------------------------- report

fn check_stats(s: &Option<metrics::Stats>, r: &Value, what: &str) {
    if r.is_null() {
        assert!(s.is_none(), "{what}: expected no points");
        return;
    }
    let s = s.as_ref().unwrap_or_else(|| panic!("{what}: no stats"));
    assert_eq!(s.n as u64, r["n"].as_u64().unwrap(), "{what}: n");
    assert_eq!(
        s.partial,
        r["partial"].as_bool().unwrap(),
        "{what}: partial"
    );
    let (a, b) = s.used_hz.unwrap();
    close(a, r["first"].as_f64().unwrap(), 1e-9, what);
    close(b, r["last"].as_f64().unwrap(), 1e-6, what);
    for (key, v) in [
        ("mean", s.mean),
        ("mae", s.mae),
        ("rms", s.rms),
        ("max_abs", s.max_abs),
    ] {
        close(v, r[key].as_f64().unwrap(), TOL, &format!("{what}: {key}"));
    }
    close(s.max_abs_hz, r["max_abs_hz"].as_f64().unwrap(), 1e-6, what);
    if let Some(sd) = r["sd"].as_f64() {
        close(s.sd.unwrap(), sd, TOL, &format!("{what}: sd"));
        close(
            s.slope.unwrap(),
            r["slope"].as_f64().unwrap(),
            TOL,
            &format!("{what}: slope"),
        );
    }
}

fn check_mask(m: &metrics::MaskResult, r: &Value, what: &str) {
    assert_eq!(m.n as u64, r["n"].as_u64().unwrap(), "{what}: n");
    assert_eq!(
        m.within as u64,
        r["within"].as_u64().unwrap(),
        "{what}: within"
    );
    if let Some(p) = r["partial"].as_bool() {
        assert_eq!(m.partial, p, "{what}: partial");
    }
    match (m.worst, r["worst_hz"].as_f64()) {
        (Some((f, d)), Some(rf)) => {
            close(f, rf, 1e-6, what);
            close(d, r["worst_excess"].as_f64().unwrap(), TOL, what);
        }
        (None, None) => {}
        (a, b) => panic!("{what}: worst {a:?} vs {b:?}"),
    }
}

fn check_report(case: &str, resp: Response, target: Target, o: Options) {
    let r = &reference()["cases"][case];
    let rep = evaluate(&resp, &target, &o).unwrap();
    close(
        rep.response_offset_db,
        r["response_offset"].as_f64().unwrap(),
        TOL,
        case,
    );
    close(
        rep.target_offset_db,
        r["target_offset"].as_f64().unwrap(),
        TOL,
        case,
    );
    close_opt(
        &rep.error,
        &opts(&r["error"]),
        TOL,
        &format!("{case}: error"),
    );
    check_stats(&rep.main, &r["main"], &format!("{case}: main"));
    check_stats(
        &rep.above_10k,
        &r["above_10k"],
        &format!("{case}: above 10 kHz"),
    );
    for (k, s) in rep.bands.iter().enumerate() {
        check_stats(s, &r["bands"][k], &format!("{case}: band {k}"));
    }
    check_mask(&rep.bs708, &r["bs708"], &format!("{case}: BS.708"));
    close_opt(
        &rep.band_lower,
        &opts(&r["band_lower"]),
        TOL,
        &format!("{case}: band lower"),
    );
    close_opt(
        &rep.band_upper,
        &opts(&r["band_upper"]),
        TOL,
        &format!("{case}: band upper"),
    );
    check_mask(
        &rep.band_compliance,
        &r["band"],
        &format!("{case}: preference band"),
    );
    for s in r["scores"].as_array().unwrap() {
        let id = s["model"].as_str().unwrap();
        let got = rep.score(id).unwrap();
        close(
            got.score.unwrap(),
            s["score"].as_f64().unwrap(),
            TOL,
            &format!("{case}: {id}"),
        );
        for ((_, v, _), w) in got.variables.iter().zip(f64s(&s["variables"])) {
            close(v.unwrap(), w, TOL, &format!("{case}: {id} variable"));
        }
    }
    if let Some(tr) = r.get("tracking") {
        let t = rep.tracking.as_ref().expect("tracking");
        close_opt(
            &t.difference,
            &opts(&tr["difference"]),
            TOL,
            &format!("{case}: L-R"),
        );
        for (b, rb) in t.bands.iter().zip(tr["bands"].as_array().unwrap()) {
            check_mask(&b.result, rb, &format!("{case}: tracking"));
            let (f, v) = b.max_abs.unwrap();
            close(v, rb["max_abs"].as_f64().unwrap(), TOL, case);
            close(f, rb["max_abs_hz"].as_f64().unwrap(), 1e-6, case);
        }
        let (f, v) = t.unspecified_max.unwrap();
        close(v, tr["gap_max"].as_f64().unwrap(), TOL, case);
        close(f, tr["gap_hz"].as_f64().unwrap(), 1e-6, case);
    } else {
        assert!(rep.tracking.is_none());
    }
    // The JSON form carries the same numbers.
    let j = rep.to_json();
    close(
        j["metrics"]["main"]["rms_dB"].as_f64().unwrap(),
        r["main"]["rms"].as_f64().unwrap(),
        TOL,
        case,
    );
}

fn response(c: &Curve) -> Response {
    Response {
        left: c.clone(),
        right: None,
        fixture: None,
        label: "test".into(),
        drive: None,
    }
}

#[test]
fn report_matches_reference_single_ear_sparse_target() {
    let r = reference();
    let a = curve(&r["curves"]["a"]);
    let t = curve(&r["curves"]["target_third"]);
    let target = user_target("third", "bk5128", "user", &t, (31.0, 20000.0));
    check_report("a_vs_third", response(&a), target, Options::default());
}

#[test]
fn report_matches_reference_dense_target() {
    let r = reference();
    let a = curve(&r["curves"]["a"]);
    let t = curve(&r["curves"]["target_dense"]);
    let (lo, hi) = t.range();
    let target = user_target("dense", "bk5128", "user", &t, (lo, hi));
    check_report("a_vs_dense", response(&a), target, Options::default());
}

#[test]
fn report_matches_reference_two_ears_band_mean_smoothing_shelves() {
    let r = reference();
    let a = curve(&r["curves"]["a"]);
    let b = curve(&r["curves"]["b"]);
    let t = curve(&r["curves"]["target_dense"]);
    let (lo, hi) = t.range();
    let target = user_target("dense", "bk5128", "user", &t, (lo, hi));
    let resp = Response {
        right: Some(b),
        ..response(&a)
    };
    let o = Options {
        normalisation: Normalisation::BandMean(200.0, 500.0),
        smoothing: Some(6),
        shelves: Some(Shelves::gains(2.5, -1.0)),
        ..Options::default()
    };
    check_report("ab_vs_dense_bandmean_s6_shelves", resp, target, o);
}

#[test]
fn report_matches_reference_against_the_bundled_target() {
    let r = reference();
    let a = curve(&r["curves"]["a"]);
    let target = target::find("ravizza2023_5128").unwrap().clone();
    check_report(
        "a_vs_ravizza_primary",
        response(&a),
        target,
        Options::default(),
    );
}

// ---------------------------------------------------------------- scores

#[test]
fn scores_agree_with_autoeq() {
    let x: Value = serde_json::from_str(include_str!("data/targets_autoeq.json")).unwrap();
    let r = reference();
    let a = curve(&r["curves"]["a"]);
    let t = curve(&r["curves"]["target_dense"]);
    let (lo, hi) = t.range();
    let target = user_target("dense", "bk5128", "user", &t, (lo, hi));
    let rep = evaluate(&response(&a), &target, &Options::default()).unwrap();
    let oe = rep.score("harman_oe_2018").unwrap();
    // AutoEq run on this project's exact grid: same points, same SD and slope.
    let ex = &x["over_ear_exact_grid"];
    let (sd, slope) = {
        let s = oe.variables[0].2.as_ref().unwrap();
        (s.sd.unwrap(), s.slope.unwrap())
    };
    close(sd, ex["sd"].as_f64().unwrap(), 1e-12, "SD vs AutoEq");
    close(
        slope,
        ex["slope"].as_f64().unwrap(),
        1e-12,
        "slope vs AutoEq",
    );
    // AutoEq's coefficients are the unrounded 114.490443008238 and
    // 15.5163857197367; the patent's rounded ones differ by this much.
    let d = 0.000443008238 + (15.52 - 15.5163857197367) * slope.abs();
    close(
        oe.score.unwrap() + d,
        ex["score"].as_f64().unwrap(),
        1e-9,
        "score vs AutoEq",
    );
    // On AutoEq's rounded R40 grid the numbers move: 0.37 points here (a
    // 7 dB notch 0.15 octave wide sits between shifted sample points). This
    // bounds the grid effect; it is not a tolerance on the algorithm.
    let own = &x["over_ear_autoeq_grid"];
    close(
        oe.score.unwrap(),
        own["score"].as_f64().unwrap(),
        0.5,
        "rounded grid, over-ear",
    );
    let ie = rep.score("harman_ie_listen").unwrap();
    let own = &x["in_ear_autoeq_grid"];
    close(
        ie.score.unwrap(),
        own["score"].as_f64().unwrap(),
        0.3,
        "rounded grid, in-ear",
    );
    close(
        ie.variables[2].1.unwrap(),
        own["me"].as_f64().unwrap(),
        0.03,
        "ME, rounded grid",
    );
    // Run on AutoEq's own R40 grid (`grid_Hz`), the engine reproduces
    // AutoEq exactly: the in-ear set in every variable and the score, the
    // over-ear SD and slope, and its score up to the rounded coefficients.
    let r40 = f64s(&x["r40_grid_Hz"]);
    let rep = evaluate(
        &response(&a),
        &target,
        &Options {
            grid_hz: r40,
            ..Options::default()
        },
    )
    .unwrap();
    let ie = rep.score("harman_ie_listen").unwrap();
    let own = &x["in_ear_autoeq_grid"];
    for (k, key) in ["sd", "slope", "me"].iter().enumerate() {
        let want = own[key].as_f64().unwrap();
        let want = if *key == "slope" { want.abs() } else { want };
        close(
            ie.variables[k].1.unwrap(),
            want,
            1e-12,
            &format!("in-ear {key}, R40"),
        );
    }
    close(
        ie.score.unwrap(),
        own["score"].as_f64().unwrap(),
        1e-9,
        "in-ear score, R40",
    );
    let oe = rep.score("harman_oe_2018").unwrap();
    let own = &x["over_ear_autoeq_grid"];
    let s = oe.variables[0].2.as_ref().unwrap();
    close(
        s.sd.unwrap(),
        own["sd"].as_f64().unwrap(),
        1e-12,
        "over-ear SD, R40",
    );
    close(
        s.slope.unwrap(),
        own["slope"].as_f64().unwrap(),
        1e-12,
        "over-ear slope, R40",
    );
    let d = 0.000443008238 + (15.52 - 15.5163857197367) * s.slope.unwrap().abs();
    close(
        oe.score.unwrap() + d,
        own["score"].as_f64().unwrap(),
        1e-9,
        "over-ear score, R40",
    );
}

/// An error that is exactly `a·ln(f/500)` has slope a, SD a·sd(ln f) and ME
/// a·mean|ln(f/500)| over each band, computed here by hand.
#[test]
fn score_formulas_by_hand() {
    let f: Vec<f64> = (0..600)
        .map(|k| 10.0 * 2f64.powf(k as f64 / 48.0))
        .collect();
    let a = 0.8;
    let t: Vec<f64> = f.iter().map(|x| 3.0 * (x / 1000.0).ln().sin()).collect();
    let resp: Vec<f64> = f
        .iter()
        .zip(&t)
        .map(|(x, y)| y + a * (x / 500.0).ln() + 90.0)
        .collect();
    let target = user_target(
        "t",
        "gras45ca_harman",
        "harman_ae_oe_2018",
        &Curve::new(f.clone(), t).unwrap(),
        (10.0, 30000.0),
    );
    let rep = evaluate(
        &response(&Curve::new(f.clone(), resp).unwrap()),
        &target,
        &Options::default(),
    )
    .unwrap();
    let g = grid::twelfth_octave();
    // Interpolation is exact for a·ln f; the grid's own points decide.
    let hand = |lo: f64, hi: f64| {
        let u: Vec<f64> = grid::band(&g, lo, hi).map(|k| g[k].ln()).collect();
        let n = u.len() as f64;
        let m = u.iter().sum::<f64>() / n;
        let sd = (u.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
        let me = u.iter().map(|x| (x - 500f64.ln()).abs()).sum::<f64>() / n;
        (a * sd, a * me)
    };
    let (sd50, _) = hand(50.0, 10000.0);
    let (sd20, _) = hand(20.0, 10000.0);
    let (_, me40) = hand(40.0, 10000.0);
    close(
        rep.score("harman_oe_2018").unwrap().score.unwrap(),
        114.49 - 12.62 * sd50 - 15.52 * a,
        1e-9,
        "over-ear",
    );
    close(
        rep.score("harman_oe_2018_20Hz").unwrap().score.unwrap(),
        114.49 - 12.62 * sd20 - 15.52 * a,
        1e-9,
        "over-ear, 20 Hz band",
    );
    close(
        rep.score("harman_ie_patent").unwrap().score.unwrap(),
        68.685 - 3.238 * sd20 - 4.473 * a - 2.658 * me40,
        1e-9,
        "in-ear, patent",
    );
    close(
        rep.score("harman_ie_listen").unwrap().score.unwrap(),
        100.0795 - 8.5 * sd20 - 6.796 * a - 3.475 * me40,
        1e-9,
        "in-ear, Listen/AutoEq",
    );
    let j = rep.to_json();
    let m = &j["metrics"]["main"];
    close(m["slope_dB_per_ln_f"].as_f64().unwrap(), a, 1e-12, "slope");
    close(
        m["slope_dB_per_octave"].as_f64().unwrap(),
        a * std::f64::consts::LN_2,
        1e-12,
        "slope per octave",
    );
    // The two in-ear sets differ by 10-30 points (E7).
    let d = rep.score("harman_ie_listen").unwrap().score.unwrap()
        - rep.score("harman_ie_patent").unwrap().score.unwrap();
    assert!((10.0..=32.0).contains(&d), "{d}");
}

#[test]
fn model_data_hold_the_published_coefficients() {
    let oe = scores::model("harman_oe_2018").unwrap();
    assert_eq!(oe.intercept, 114.49);
    let w: Vec<(f64, (f64, f64))> = oe.terms.iter().map(|t| (t.weight, t.band_hz)).collect();
    assert_eq!(w, vec![(12.62, (50.0, 10000.0)), (15.52, (50.0, 10000.0))]);
    // The patent's literal band, reported alongside (same coefficients).
    let o20 = scores::model("harman_oe_2018_20Hz").unwrap();
    assert_eq!(o20.intercept, 114.49);
    let w: Vec<(f64, (f64, f64))> = o20.terms.iter().map(|t| (t.weight, t.band_hz)).collect();
    assert_eq!(w, vec![(12.62, (20.0, 10000.0)), (15.52, (20.0, 10000.0))]);
    let ip = scores::model("harman_ie_patent").unwrap();
    assert_eq!(ip.intercept, 68.685);
    let w: Vec<f64> = ip.terms.iter().map(|t| t.weight).collect();
    assert_eq!(w, vec![3.238, 4.473, 2.658]);
    assert_eq!(ip.terms[2].band_hz, (40.0, 10000.0));
    let il = scores::model("harman_ie_listen").unwrap();
    assert_eq!(il.intercept, 100.0795);
    let w: Vec<f64> = il.terms.iter().map(|t| t.weight).collect();
    assert_eq!(w, vec![8.5, 6.796, 3.475]);
    for m in scores::models() {
        assert!(fixture::fixture(&m.training_fixture).is_some(), "{}", m.id);
        assert!(!m.source.is_empty());
    }
}

#[test]
fn score_greying_rules() {
    let f: Vec<f64> = (0..600)
        .map(|k| 10.0 * 2f64.powf(k as f64 / 48.0))
        .collect();
    let flat = Curve::new(f.clone(), vec![0.0; f.len()]).unwrap();
    let bump = Curve::new(
        f.clone(),
        f.iter().map(|x| 2.0 * (x / 300.0).ln().cos()).collect(),
    )
    .unwrap();
    let codes = |s: &scores::Score| s.flags.iter().map(|f| f.code).collect::<Vec<_>>();
    let harman = user_target(
        "h",
        "gras45ca_harman",
        "harman_ae_oe_2018",
        &flat,
        (10.0, 30000.0),
    );

    // The training pair, measured: nothing greyed, no flag.
    let mut resp = response(&bump);
    resp.fixture = Some("gras45ca_harman".into());
    let measured = Options {
        measured: true,
        ..Options::default()
    };
    let rep = evaluate(&resp, &harman, &measured).unwrap();
    let oe = rep.score("harman_oe_2018").unwrap();
    assert!(!oe.greyed, "{:?}", oe.flags);
    assert!(
        codes(oe) == vec!["coupler_extrapolated"],
        "the 45CA's RA0045 is IEC 60318-4: {:?}",
        codes(oe)
    );
    // Simulated: still not greyed, but it says so.
    let rep = evaluate(&resp, &harman, &Options::default()).unwrap();
    let oe = rep.score("harman_oe_2018").unwrap();
    assert!(!oe.greyed);
    assert!(codes(oe).contains(&"simulated"));
    // The in-ear models were trained on another pair.
    let ie = rep.score("harman_ie_patent").unwrap();
    assert!(ie.greyed);
    assert!(codes(ie).contains(&"training_fixture_mismatch"));
    assert!(codes(ie).contains(&"training_target_mismatch"));

    // A simulated IEC 60318-4 load against the bundled 5128 target.
    let mut sim = response(&bump);
    sim.fixture = Some("iec60318_4".into());
    let rv = target::find("ravizza2023_5128").unwrap();
    let rep = evaluate(&sim, rv, &Options::default()).unwrap();
    for s in &rep.scores {
        assert!(s.greyed, "{}", s.model.id);
        let c = codes(s);
        for code in [
            "training_fixture_mismatch",
            "training_target_mismatch",
            "simulated",
            "coupler_extrapolated",
        ] {
            assert!(c.contains(&code), "{}: {c:?}", s.model.id);
        }
    }
    // The in-ear models need 20 Hz; the 5128 curve set starts at 31 Hz.
    let ie = rep.score("harman_ie_listen").unwrap();
    assert!(codes(ie).contains(&"partial_range"));
    assert!(!codes(rep.score("harman_oe_2018").unwrap()).contains(&"partial_range"));
    let rc: Vec<&str> = rep.flags.iter().map(|f| f.code).collect();
    assert!(rc.contains(&"fixture_mismatch"), "{rc:?}");
    assert_eq!(rep.fixture_match, Some(Match::Different));
    assert!(rc.contains(&"coupler_extrapolated"));
    // A score never disappears: greyed scores keep their value.
    assert!(rep.scores.iter().all(|s| s.score.is_some()));
}

// -------------------------------------------------------------- fixtures

#[test]
fn fixtures_compare_by_ear_simulator() {
    assert_eq!(fixture::compare("bk5128", "bk5128"), Match::Same);
    assert_eq!(
        fixture::compare("p57_type4_3", "bk5128"),
        Match::SameEarSimulator
    );
    assert_eq!(
        fixture::compare("iec60318_4", "gras45ca_harman"),
        Match::SameEarSimulator
    );
    assert_eq!(
        fixture::compare("p57_type3_3", "iec60318_4_canal_extender"),
        Match::SameEarSimulator
    );
    assert_eq!(fixture::compare("iec60318_4", "bk5128"), Match::Different);
    assert_eq!(fixture::compare("my_rig", "my_rig"), Match::Same);
    assert_eq!(fixture::compare("my_rig", "bk5128"), Match::Different);
    for f in fixture::fixtures() {
        assert!(!f.source.is_empty() && !f.label.is_empty(), "{}", f.id);
    }
}

#[test]
fn fixture_is_inferred_from_the_ear_load() {
    let text = include_str!("../../../examples/design_over_ear.json");
    let p = acoustilab::params::Parametric::parse(text).unwrap();
    for (ear, want) in [("iec60318_4", "iec60318_4"), ("type43", "p57_type4_3")] {
        let mut ov = acoustilab::params::Overrides::new();
        ov.insert("ear".into(), acoustilab::expr::PValue::Str(ear.into()));
        let c = Circuit::from_parametric(&p, &ov).unwrap();
        assert_eq!(
            fixture::for_probe(&c, "p_drp").map(|f| f.id.as_str()),
            Some(want)
        );
        assert!(fixture::for_probe(&c, "p_front").is_none());
        assert!(fixture::for_probe(&c, "no_such_probe").is_none());
    }
}

// --------------------------------------------------------------- targets

#[test]
fn bundled_ravizza_set() {
    let all = target::bundled();
    assert_eq!(all.len(), 33);
    let p = &all[0];
    assert_eq!(p.name, "ravizza2023_5128");
    assert!(p.primary);
    assert_eq!(p.fixture, "bk5128");
    assert_eq!(p.provenance.licence, "CC-BY-4.0");
    assert_eq!(p.provenance.doi.as_deref(), Some("10.5281/zenodo.8388242"));
    assert_eq!(p.extra["dataset_curve"], "APHarm2018v2");
    assert_eq!(p.extra["ratings"]["rank"], 1);
    // The APHarm2018v2 row of MagnitudeFrequencyResponses.csv (md5
    // 70dc33df8b206801a9661d1f609b8e82), as published.
    let row = [
        -6.7, -7.0, -7.2, -7.7, -8.3, -9.2, -10.5, -11.9, -12.8, -13.0, -12.7, -12.4, -11.8, -11.2,
        -10.3, -9.9, -9.9, -9.3, -6.8, -3.7, -1.0, 0.0, -0.6, -1.3, -3.2, -5.4, -8.8, -8.6, -10.4,
        -10.4,
    ];
    assert_eq!(p.curve.db(), row);
    assert_eq!(p.curve.freqs()[0], 31.0);
    assert_eq!(p.curve.freqs()[29], 25000.0);
    assert_eq!(p.valid_range_hz, (31.0, 20000.0));
    let hp1 = target::find("ravizza2023:HP1").unwrap();
    assert_eq!(&hp1.curve.db()[..3], &[-9.1, -9.3, -9.8]);
    for t in all {
        assert_eq!(t.curve.freqs().len(), 30, "{}", t.name);
        assert_eq!(t.family, "ravizza2023");
        assert!(t.has_flag("third_octave"));
        assert!(t.extra["ratings"]["n"].as_u64() == Some(504), "{}", t.name);
    }
    for id in ["DF5128", "FF5128", "Soundguys", "APHarm2015"] {
        assert!(target::find(&format!("ravizza2023:{id}"))
            .unwrap()
            .has_flag("adaptation"));
    }
    assert!(target::find("ravizza2023:HP6")
        .unwrap()
        .has_flag("data_anomaly"));
    // Ranks are a permutation of 1..=32 and follow the means.
    let mut ranks: Vec<(u64, f64)> = all[1..]
        .iter()
        .map(|t| {
            (
                t.extra["ratings"]["rank"].as_u64().unwrap(),
                t.extra["ratings"]["mean"].as_f64().unwrap(),
            )
        })
        .collect();
    ranks.sort_by_key(|r| r.0);
    assert_eq!(
        ranks.iter().map(|r| r.0).collect::<Vec<_>>(),
        (1..=32).collect::<Vec<_>>()
    );
    assert!(ranks.windows(2).all(|w| w[0].1 >= w[1].1));
}

#[test]
fn target_json_round_trips_and_rejects_bad_objects() {
    let t = target::find("ravizza2023_5128").unwrap();
    let back = Target::from_json(&t.to_json()).unwrap();
    assert_eq!(&back, t);
    let mut v = t.to_json();
    v["colour"] = json!("red");
    assert!(Target::from_json(&v)
        .unwrap_err()
        .to_string()
        .contains("unknown key 'colour'"));
    let mut v = t.to_json();
    v.as_object_mut().unwrap().remove("fixture");
    assert!(Target::from_json(&v)
        .unwrap_err()
        .to_string()
        .contains("fixture"));
    assert!(target::resolve(&json!("no_such_target")).is_err());
    assert_eq!(&target::resolve(&json!("ravizza2023_5128")).unwrap(), t);
}

#[test]
fn csv_import() {
    let text = "# name: my target\n# fixture: bk5128\n# License: CC0-1.0\n# free comment: kept out\nfrequency_Hz,dB\n20,1\n100,2.5\n1000,0\n20000,-4\n";
    let t = import_target_csv(text, None, None).unwrap();
    assert_eq!(t.name, "my target");
    assert_eq!(t.fixture, "bk5128");
    assert_eq!(t.provenance.licence, "CC0-1.0");
    assert_eq!(t.curve.db(), [1.0, 2.5, 0.0, -4.0]);
    assert!(t.has_flag("user_supplied"));
    // Other separators, fixture as an argument.
    let t = import_target_csv("20;1\n100;2\n", Some("iec60318_4"), Some("x")).unwrap();
    assert_eq!((t.fixture.as_str(), t.name.as_str()), ("iec60318_4", "x"));
    let t = import_target_csv("20\t1\n100\t2\n", Some("f"), None).unwrap();
    assert_eq!(t.curve.freqs(), [20.0, 100.0]);
    let t = import_target_csv("20 1\n100   2\n", Some("f"), None).unwrap();
    assert_eq!(t.curve.db(), [1.0, 2.0]);
    // Errors.
    let err =
        |text: &str, fx: Option<&str>| import_target_csv(text, fx, None).unwrap_err().to_string();
    assert!(err("20,1\n100,2\n", None).contains("no fixture tag"));
    assert!(err("# fixture: a\n20,1\n100,2\n", Some("b")).contains("differs"));
    assert!(err("20,1\n10,2\n", Some("f")).contains("does not increase"));
    assert!(err("20,1,3\n100,2,4\n", Some("f")).contains("two columns"));
    assert!(err("f,dB\n20,1\nx,2\n", Some("f")).contains("line 3"));
    assert!(err("# fixture: a\n# fixture: b\n20,1\n", None).contains("twice"));
    assert!(err("20,1\n", Some("f")).contains("at least two"));
}

#[test]
fn personalisation_adds_the_shelves_on_the_grid() {
    let t = target::find("ravizza2023_5128").unwrap();
    let g = grid::twelfth_octave();
    let s = Shelves::gains(3.0, -2.0);
    let base = t.on_grid(&g, None);
    let pers = t.on_grid(&g, Some(&s));
    for (k, (b, p)) in base.iter().zip(&pers).enumerate() {
        match (b, p) {
            (Some(b), Some(p)) => close(*p, b + s.db_at(g[k]), 1e-12, "shelves"),
            (None, None) => {}
            _ => panic!("coverage changed"),
        }
    }
    let d = Shelves::default();
    assert_eq!((d.bass_fc_hz, d.treble_fc_hz), (105.0, 2500.0));
    close(d.bass_q, std::f64::consts::FRAC_1_SQRT_2, 1e-15, "Q");
    assert!(Shelves::from_json(&json!({"bass_dB": 1, "tilt": 2})).is_err());
    assert!(Shelves::from_json(&json!({"bass_Q": -1})).is_err());
    // Out-of-range settings are refused rather than turned into NaN levels.
    for bad in [
        json!({"bass_dB": 1e6}),
        json!({"treble_dB": -41}),
        json!({"treble_Q": 1e-320}),
        json!({"bass_fc_Hz": 1e9}),
    ] {
        assert!(Shelves::from_json(&bad).is_err(), "{bad}");
    }
    let s = Shelves::from_json(
        &json!({"bass_dB": 40, "treble_dB": -40, "bass_Q": 0.1, "treble_Q": 10}),
    )
    .unwrap();
    for f in [1.0, 105.0, 2500.0, 1e5] {
        assert!(s.db_at(f).is_finite(), "{f} Hz");
    }
    assert_eq!(
        Shelves::from_json(&json!({"bass_dB": 4})).unwrap(),
        Shelves::gains(4.0, 0.0)
    );
}

#[test]
fn harman_style_reconstruction() {
    let r = reference();
    let base = user_target(
        "baseline",
        "gras45ca_harman",
        "user",
        &curve(&r["curves"]["target_third"]),
        (31.0, 20000.0),
    );
    let rc = &r["reconstruction"];
    let s = recipe_preset(rc["preset"].as_str().unwrap()).unwrap();
    assert_eq!((s.bass_db, s.treble_db), (6.44, -1.41));
    let t = reconstruct_harman_style(&base, &s, "rebuilt").unwrap();
    assert_eq!(t.fixture, "gras45ca_harman");
    assert_eq!(t.family, "harman_style_reconstruction");
    assert!(t.has_flag("approximation") && t.has_flag("shelf_q_assumed"));
    for (f, v) in f64s(&rc["f"]).iter().zip(f64s(&rc["dB"])) {
        close(t.curve.at(*f).unwrap(), v, 1e-9, &format!("{f} Hz"));
    }
    // The shelves are resolved between the baseline's third-octave points:
    // the curve is stored at 1/48 octave, and reading it between its points
    // misses the shelves' curvature by less than 1e-3 dB.
    assert!(t.curve.freqs().len() > 4 * base.curve.freqs().len());
    for k in 0..400 {
        let f = 32.0 * 2f64.powf(k as f64 / 41.3);
        if f > 19900.0 {
            break;
        }
        let exact = base.curve.at(f).unwrap() + s.db_at(f);
        close(
            t.curve.at(f).unwrap(),
            exact,
            1e-3,
            &format!("{f} Hz between points"),
        );
    }
    // A reconstruction is never a model's training target.
    let resp = Response {
        fixture: Some("gras45ca_harman".into()),
        ..response(&curve(&r["curves"]["a"]))
    };
    let rep = evaluate(
        &resp,
        &t,
        &Options {
            measured: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert!(rep.score("harman_oe_2018").unwrap().greyed);
}

#[test]
fn preference_band_holds_every_class_variant() {
    let g = grid::twelfth_octave();
    let p = BandParams::default();
    let shares: f64 = p.classes.iter().map(|c| c.1).sum();
    close(shares, 1.0, 1e-12, "class shares");
    let norm = Normalisation::At(500.0);
    let band = preference_band(&g, &p, |v| norm.offset(&g, v));
    for gain in [-2.0, 0.0, 4.0, 5.0, 6.0] {
        let s = Shelves::gains(gain, 0.0);
        let v: Vec<Option<f64>> = g.iter().map(|&f| Some(s.db_at(f))).collect();
        let o = norm.offset(&g, &v).unwrap();
        for (k, x) in v.iter().enumerate() {
            let x = x.unwrap() - o;
            assert!(
                x >= band.lower[k] - 1e-12 && x <= band.upper[k] + 1e-12,
                "{gain} dB at {}",
                g[k]
            );
        }
    }
    // Widening: none below 2 kHz, 2.5 dB half way to 4 kHz, 5 dB above,
    // plus 3 dB from 16 kHz.
    close(p.widening_db(1000.0), 0.0, 1e-12, "1 kHz");
    close(p.widening_db(2000.0 * 2f64.sqrt()), 2.5, 1e-12, "2.8 kHz");
    close(p.widening_db(6000.0), 5.0, 1e-12, "6 kHz");
    close(p.widening_db(16000.0), 8.0, 1e-12, "16 kHz");
    // Around 1 kHz the band collapses to the class shelves (a few tenths of
    // a dB); at 30 Hz it spans -2 to +6 dB.
    let k30 = grid::nearest(&g, 30.0);
    assert!(
        band.lower[k30] < -1.9 && band.upper[k30] > 5.7,
        "{} {}",
        band.lower[k30],
        band.upper[k30]
    );
}

// -------------------------------------------------------------- tracking

#[test]
fn tracking_against_bs708_limits() {
    let f: Vec<f64> = (0..600)
        .map(|k| 10.0 * 2f64.powf(k as f64 / 48.0))
        .collect();
    let l = Curve::new(f.clone(), f.iter().map(|x| (x / 50.0).ln().sin()).collect()).unwrap();
    let g = grid::twelfth_octave();
    let t = metrics::tracking(&l, &l, &g, Some(3)).unwrap();
    assert!(t
        .bands
        .iter()
        .all(|b| b.pass() == Some(true) && b.max_abs.unwrap().1 == 0.0));
    // A 1.5 dB level offset fails 100 Hz-8 kHz (1 dB) and passes 10-16 kHz (2 dB).
    let r = l.shifted(-1.5);
    let t = metrics::tracking(&l, &r, &g, None).unwrap();
    assert_eq!(t.bands[0].pass(), Some(false));
    assert_eq!(t.bands[1].pass(), Some(true));
    close(t.bands[0].max_abs.unwrap().1, 1.5, 1e-12, "offset");
    close(t.bands[0].result.worst.unwrap().1, 0.5, 1e-12, "excess");
    assert_eq!(t.bands[0].result.percent(), Some(0.0));
    let j = t.to_json(&g);
    assert_eq!(j["bands"][0]["limit_dB"], 1.0);
    assert_eq!(j["bands"][1]["band_Hz"], json!([10000.0, 16000.0]));
}

// ------------------------------------------------------------ end to end

#[test]
fn design_template_scored_against_the_bundled_target() {
    let text = include_str!("../../../examples/design_over_ear.json");
    let c = Circuit::from_json(text).unwrap();
    let res = c.solve().unwrap();
    let p = res.probe("p_drp").unwrap();
    let curve = Curve::new(res.freqs_hz.clone(), p.spl_db()).unwrap();
    let resp = Response {
        left: curve,
        right: None,
        fixture: fixture::for_probe(&c, "p_drp").map(|f| f.id.clone()),
        label: "p_drp".into(),
        drive: Some(res.meta.drive.label.clone()),
    };
    let t = target::find("ravizza2023_5128").unwrap();
    let rep = evaluate(&resp, t, &Options::default()).unwrap();
    let m = rep.main.as_ref().unwrap();
    assert!(m.rms.is_finite() && m.sd.unwrap().is_finite());
    // The 5128 set starts at 31 Hz, so 20-31 Hz is missing: partial.
    assert!(m.partial);
    close(m.used_hz.unwrap().0, 31.62, 0.01, "first point used");
    // The response level at 500 Hz is its reference level (about 104 dB SPL
    // at 1 mW for this driver in the template's sealed cup).
    assert!(
        (80.0..120.0).contains(&rep.response_offset_db),
        "{}",
        rep.response_offset_db
    );
    let codes: Vec<&str> = rep.flags.iter().map(|f| f.code).collect();
    for c in [
        "fixture_mismatch",
        "simulated",
        "coupler_extrapolated",
        "partial_range",
    ] {
        assert!(codes.contains(&c), "{c} missing in {codes:?}");
    }
    assert!(rep.scores.iter().all(|s| s.greyed && s.score.is_some()));
    let j = rep.to_json();
    assert_eq!(j["target"]["licence"], "CC-BY-4.0");
    assert_eq!(j["coupler_extrapolated_Hz"], json!([20.0, 100.0]));
    assert_eq!(j["grid_Hz"].as_array().unwrap().len(), 121);
    assert!(j["response"]["drive"].as_str().unwrap().contains("mW"));
}

#[test]
fn normalisation_errors_are_reported() {
    let short = Curve::new(vec![1000.0, 2000.0], vec![0.0, 1.0]).unwrap();
    let t = target::find("ravizza2023_5128").unwrap();
    let e = evaluate(&response(&short), t, &Options::default()).unwrap_err();
    assert!(
        e.to_string()
            .contains("response is undefined at the normalisation"),
        "{e}"
    );
    let bad = Options {
        grid_hz: vec![100.0, 50.0],
        ..Options::default()
    };
    let wide = Curve::new(vec![10.0, 30000.0], vec![0.0, 0.0]).unwrap();
    assert!(evaluate(&response(&wide), t, &bad).is_err());
}
