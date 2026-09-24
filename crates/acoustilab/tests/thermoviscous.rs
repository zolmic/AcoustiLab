//! Thermoviscous ducts (spec Section 6 element test) against an independent
//! mpmath implementation of the Zwikker–Kosten / Stinson model
//! (tools/refgen/thermo_refs.py), closed-form limits and textbook series.

use acoustilab::special::{shape_rect, shape_slit};
use acoustilab::thermoviscous::{
    abcd, lumped_series_impedance, medium, poiseuille, propagation, Section,
};
use acoustilab::{AirState, Circuit, C64};
use serde_json::json;
use std::f64::consts::PI;

struct Table {
    meta: serde_json::Value,
    cols: Vec<String>,
    rows: Vec<Vec<f64>>,
}

impl Table {
    /// Loads a fixture whose numbers are decimal strings (exact doubles).
    fn load(name: &str) -> Table {
        let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let meta: serde_json::Value = serde_json::from_str(&text).unwrap();
        let strs = |x: &serde_json::Value| -> Vec<String> {
            x.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        let cols = strs(&meta["columns"]);
        let rows: Vec<Vec<f64>> = meta["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| strs(r).iter().map(|s| s.parse::<f64>().unwrap()).collect())
            .collect();
        assert!(!rows.is_empty() && rows.iter().all(|r| r.len() == cols.len()));
        Table { meta, cols, rows }
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

    /// The fixture's air, which must be the spec_reference preset.
    fn air(&self) -> AirState {
        let g = |k: &str| {
            self.meta["air"][k]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap()
        };
        let air = AirState {
            temperature_k: 293.15,
            p0: g("p0"),
            rho: g("rho"),
            c: g("c"),
            mu: g("mu"),
            gamma: g("gamma"),
            prandtl: g("prandtl"),
        };
        assert_eq!(air, AirState::spec_reference());
        air
    }
}

fn rel(a: C64, b: C64) -> f64 {
    (a - b).norm() / b.norm()
}

fn section(kind: f64, d1: f64, d2: f64) -> Section {
    match kind as u32 {
        0 => Section::Circle { radius: d1 },
        1 => Section::Slit { gap: d1, width: d2 },
        _ => Section::Rect { a: d1, b: d2 },
    }
}

#[test]
fn duct_model_matches_independent_mpmath_implementation() {
    // Spec Section 6: "tube impedance against the exact solution for shear
    // wavenumbers from 0.1 to 100", here also 1000, for circles, slits and
    // rectangles. Tolerance 1e-10 relative on every quantity.
    let t = Table::load("thermo_duct.json");
    let air = t.air();
    let names = ["rho_eff", "k_eff", "gamma", "zc", "a", "b", "c", "d"];
    let mut worst: [(f64, String); 8] = std::array::from_fn(|_| (0.0, String::new()));
    let mut shear = (f64::INFINITY, 0.0f64);
    for r in &t.rows {
        let sec = section(r[t.col("kind")], r[t.col("d1")], r[t.col("d2")]);
        let (omega, length) = (r[t.col("omega")], r[t.col("length")]);
        let s = sec.shear_wavenumber(&air, omega);
        shear = (shear.0.min(s), shear.1.max(s));
        let m = medium(&sec, &air, omega);
        let (g, zc) = propagation(&sec, &air, omega);
        let [a, b, c, d] = abcd(&sec, &air, omega, length);
        let got = [m.rho_eff, m.k_eff, g, zc, a, b, c, d];
        for (i, n) in names.iter().enumerate() {
            let e = rel(got[i], t.c64(r, n));
            if e > worst[i].0 {
                worst[i] = (e, format!("{sec:?}, s = {s:.3}, l = {length}"));
            }
        }
    }
    println!(
        "shear wavenumbers covered: {:.3} to {:.1}",
        shear.0, shear.1
    );
    assert!(shear.0 <= 0.1 * (1.0 + 1e-9) && shear.1 >= 1000.0 * (1.0 - 1e-9));
    for (i, n) in names.iter().enumerate() {
        println!("{n}: worst {:.2e} at {}", worst[i].0, worst[i].1);
        assert!(worst[i].0 < 1e-10, "{n}: {:?}", worst[i]);
    }
}

#[test]
fn rect_shape_function_matches_stinson_double_series() {
    let t = Table::load("thermo_rect.json");
    let (mut wf, mut wom) = (0.0f64, 0.0f64);
    for r in &t.rows {
        let (a, b) = (r[t.col("a")], r[t.col("b")]);
        let k = C64::new(r[t.col("k_re")], r[t.col("k_im")]);
        let s = shape_rect(k, a, b);
        let at = format!("b/a = {}, |k|a = {}", b / a, k.norm() * a);
        let ef = rel(s.f, t.c64(r, "f"));
        let eom = rel(s.one_minus, t.c64(r, "om"));
        assert!(ef < 1e-12 && eom < 1e-12, "{at}: F {ef:e}, 1-F {eom:e}");
        // Symmetric in the two sides.
        let sw = shape_rect(k, b, a);
        assert!(rel(sw.one_minus, s.one_minus) < 1e-15);
        wf = wf.max(ef);
        wom = wom.max(eom);
    }
    println!("rect F worst {wf:.2e}, 1-F worst {wom:.2e}");
}

#[test]
fn rect_shape_function_is_continuous_across_its_switches() {
    let a = 1e-3;
    for b in [a, 3.0 * a] {
        for c in [PI / 3.0, 11.0 * PI, 42.0 * 2f64.sqrt()] {
            let k = |x: f64| C64::from_polar(x / a, PI / 4.0);
            let lo = shape_rect(k(c * (1.0 - 1e-13)), a, b);
            let hi = shape_rect(k(c * (1.0 + 1e-13)), a, b);
            assert!(rel(lo.f, hi.f) < 1e-12 && rel(lo.one_minus, hi.one_minus) < 1e-12);
        }
    }
}

/// Resistance and inertance of a duct of length `l` at a frequency where
/// the shear wavenumber is 1e-3 (deep in the Poiseuille regime).
fn low_frequency_rm(sec: &Section, air: &AirState, l: f64) -> (f64, f64) {
    let h = sec.shape_length();
    let omega = 1e-6 * air.mu / (air.rho * h * h);
    let z = lumped_series_impedance(sec, air, omega, l);
    (z.re, z.im / omega)
}

/// Poiseuille resistance of a rectangular duct, sides h ≤ w (the classical
/// series, e.g. F. M. White, Viscous Fluid Flow, 3rd ed., 2006, Sec. 3-3.3):
/// R = 12μl/(w h³) / [1 − (192 h/(π⁵ w)) Σ_{n odd} tanh(nπw/(2h))/n⁵].
fn rect_poiseuille_r(mu: f64, h: f64, w: f64, l: f64) -> f64 {
    let (h, w) = (h.min(w), h.max(w));
    let mut sum = 0.0;
    let mut n = 1.0;
    while n < 2001.0 {
        sum += (n * PI * w / (2.0 * h)).tanh() / n.powi(5);
        n += 2.0;
    }
    12.0 * mu * l / (w * h.powi(3)) / (1.0 - 192.0 * h / (PI.powi(5) * w) * sum)
}

/// Low-frequency inertance factor M/(ρl/S) of a rectangle from the double
/// series: with w_mn = 64/(π⁴m²n²) and γ² = (mπ/a)² + (nπ/b)²,
/// 1 − F = k²·D1 − k⁴·D2 + …, D1 = Σ w/γ², D2 = Σ w/γ⁴, so
/// ρ_eff = ρ/(1 − F) gives M = (ρl/S)·D2/D1².
fn rect_mass_factor(a: f64, b: f64) -> f64 {
    let (mut d1, mut d2) = (0.0, 0.0);
    let mut m = 1.0;
    while m < 1200.0 {
        let mut n = 1.0;
        while n < 1200.0 {
            let w = 64.0 / (PI.powi(4) * m * m * n * n);
            let g2 = (m * PI / a).powi(2) + (n * PI / b).powi(2);
            d1 += w / g2;
            d2 += w / (g2 * g2);
            n += 2.0;
        }
        m += 2.0;
    }
    d2 / (d1 * d1)
}

#[test]
fn poiseuille_limits_within_0_1_percent() {
    let air = AirState::spec_reference();
    let l = 1e-2;
    let check = |name: &str, sec: Section, r0: f64, m0: f64| {
        let (r, m) = low_frequency_rm(&sec, &air, l);
        println!(
            "{name}: R/R0 - 1 = {:.2e}, M/M0 - 1 = {:.2e}",
            r / r0 - 1.0,
            m / m0 - 1.0
        );
        assert!((r / r0 - 1.0).abs() < 1e-3, "{name}: R {r} vs {r0}");
        assert!((m / m0 - 1.0).abs() < 1e-3, "{name}: M {m} vs {m0}");
    };
    let (r, m) = poiseuille::tube(air.mu, air.rho, 0.5e-3, l);
    check("tube", Section::Circle { radius: 0.5e-3 }, r, m);
    let (r, m) = poiseuille::slit(air.mu, air.rho, 0.1e-3, 10e-3, l);
    check(
        "slit",
        Section::Slit {
            gap: 0.1e-3,
            width: 10e-3,
        },
        r,
        m,
    );
    for (a, b) in [(1e-3, 1e-3), (0.5e-3, 1.5e-3), (0.2e-3, 2e-3)] {
        let s = a * b;
        let r = rect_poiseuille_r(air.mu, a, b, l);
        let m = rect_mass_factor(a, b) * air.rho * l / s;
        check(&format!("rect {a}x{b}"), Section::Rect { a, b }, r, m);
    }
    // The square's constants: R·a⁴/(μl) = 28.45 (White's series) and
    // M = 1.378·ρl/S (double series, 4000² terms).
    let a = 1e-3;
    let sq = Section::Rect { a, b: a };
    let (r, m) = low_frequency_rm(&sq, &air, l);
    let r = r * a.powi(4) / (air.mu * l);
    let m = m / (air.rho * l / (a * a));
    assert!((r - 28.454).abs() < 0.001, "{r}");
    assert!((m - 1.3784).abs() < 0.0001, "{m}");
}

#[test]
fn rect_tends_to_the_slit_as_the_aspect_ratio_grows() {
    // At every frequency the difference is of order a/b (the two short
    // walls), so it must fall in proportion to the aspect ratio.
    let a = 0.2e-3;
    for ratio in [1e2, 1e3, 1e4] {
        let b = a * ratio;
        for x in [1e-2, 1.0, 10.0, 100.0, 1e3] {
            let k = C64::from_polar(x / a, PI / 4.0);
            let r = shape_rect(k, a, b);
            let s = shape_slit(k * (0.5 * a));
            let (ef, eom) = (rel(r.f, s.f), rel(r.one_minus, s.one_minus));
            assert!(
                ef < 1.0 / ratio && eom < 1.0 / ratio,
                "b/a = {ratio}, |k|a = {x}: {ef:e} {eom:e}"
            );
        }
    }
    // And the low-frequency resistance approaches 12μl/(w h³) like
    // 1/(1 − 0.630 a/b) (White's series), not merely within 1/ratio.
    let air = AirState::spec_reference();
    let (r_slit, _) = poiseuille::slit(air.mu, air.rho, a, 100.0 * a, 1e-2);
    let (r, _) = low_frequency_rm(&Section::Rect { a, b: 100.0 * a }, &air, 1e-2);
    let expect = 1.0 / (1.0 - 192.0 / PI.powi(5) * 1.004_523_762 / 100.0);
    assert!((r / r_slit / expect - 1.0).abs() < 1e-5, "{}", r / r_slit);
}

#[test]
fn resistance_grows_as_sqrt_f_at_high_shear_wavenumber() {
    // Boundary-layer limit: R = (P/(2A²))·sqrt(2ωρμ)·l, with relative
    // corrections of order 1/s (next term of the shape function).
    let air = AirState::spec_reference();
    let l = 1e-2;
    for (sec, perimeter) in [
        (Section::Circle { radius: 2e-3 }, 2.0 * PI * 2e-3),
        (
            Section::Slit {
                gap: 0.5e-3,
                width: 1.0,
            },
            2.0 * (1.0 + 0.5e-3),
        ),
        (Section::Rect { a: 2e-3, b: 5e-3 }, 2.0 * 7e-3),
    ] {
        let area = sec.area();
        let h = sec.shape_length();
        for s in [100.0, 300.0, 1000.0] {
            let omega = s * s * air.mu / (air.rho * h * h);
            let r = lumped_series_impedance(&sec, &air, omega, l).re;
            let r_bl =
                perimeter / (2.0 * area * area) * (2.0 * omega * air.rho * air.mu).sqrt() * l;
            assert!(
                (r / r_bl - 1.0).abs() < 3.0 / s,
                "{sec:?} s = {s}: {}",
                r / r_bl
            );
            let r4 = lumped_series_impedance(&sec, &air, 4.0 * omega, l).re;
            assert!(
                (r4 / r - 2.0).abs() < 3.0 / s,
                "{sec:?} s = {s}: {}",
                r4 / r
            );
        }
    }
}

#[test]
fn every_section_is_passive_and_reciprocal() {
    let air = AirState::spec_reference();
    let sections = [
        Section::Circle { radius: 0.3e-3 },
        Section::Circle { radius: 5e-3 },
        Section::Slit {
            gap: 0.05e-3,
            width: 5e-3,
        },
        Section::Slit {
            gap: 2e-3,
            width: 40e-3,
        },
        Section::Rect {
            a: 0.1e-3,
            b: 0.1e-3,
        },
        Section::Rect { a: 1e-3, b: 3e-3 },
        Section::Rect { a: 8e-3, b: 12e-3 },
        Section::Equivalent {
            area: 2e-4,
            perimeter: 0.06,
        },
    ];
    let mut f = 1.0;
    while f < 2e5 {
        let omega = 2.0 * PI * f;
        for sec in &sections {
            let m = medium(sec, &air, omega);
            // Positive viscous resistance and thermal loss (e^{+jωt}: a lossy
            // compliance has Im(1/K) < 0).
            assert!((C64::new(0.0, omega) * m.rho_eff).re > 0.0, "{sec:?} {f}");
            assert!(m.k_eff.inv().im < 0.0, "{sec:?} {f}");
            let (g, zc) = propagation(sec, &air, omega);
            assert!(g.re >= 0.0 && zc.re >= 0.0, "{sec:?} {f}: {g} {zc}");
            for l in [1e-3, 3e-2, 0.3] {
                let [a, b, c, d] = abcd(sec, &air, omega, l);
                let det = a * d - b * c;
                let scale = (a * d).norm().max(1.0);
                assert!((det - 1.0).norm() < 1e-12 * scale, "{sec:?} {f} {l}: {det}");
            }
        }
        f *= 1.5;
    }
}

#[test]
fn power_balances_with_ducts_and_radiation() {
    // Tellegen over a network with a tube to an unflanged opening, a slit to
    // a baffled opening and a cavity, at both fidelity levels. Radiation
    // loads absorb positive power.
    for level in [0, 1] {
        let doc = json!({
            "schema": "acoustilab-netlist/0.1",
            "air": {"preset": "spec_reference"},
            "sweep": {"frequencies_Hz": [20.0, 500.0, 3000.0, 15000.0]},
            "level": level,
            "nodes": [
                {"id": "a1", "domain": "acoustic"},
                {"id": "a2", "domain": "acoustic"},
                {"id": "a3", "domain": "acoustic"}
            ],
            "elements": [
                {"id": "src", "type": "pressure_source", "nodes": ["a1"], "p_Pa": 1.0,
                 "Zs_Pa_s_per_m3": 1e6},
                {"id": "cup", "type": "cavity", "node": "a1", "volume_cm3": 10},
                {"id": "port", "type": "tube", "nodes": ["a1", "a2"], "radius_mm": 1.5,
                 "length_mm": 8},
                {"id": "open", "type": "radiation", "nodes": ["a2"], "radius_mm": 1.5,
                 "baffle": "free"},
                {"id": "gap", "type": "slit", "nodes": ["a1", "a3"], "gap_mm": 0.3,
                 "width_mm": 6, "length_mm": 4},
                {"id": "hole", "type": "radiation", "nodes": ["a3"], "area_cm2": 0.03,
                 "baffle": "infinite", "count": 2}
            ]
        });
        let c = Circuit::from_json(&doc.to_string()).unwrap();
        for &f in &c.freqs {
            let x = c.solve_at(f).unwrap();
            let p = c.power_absorbed(f, &x);
            assert!(p.iter().all(|(_, v)| v.is_some()), "{p:?}");
            let get = |id: &str| p.iter().find(|(n, _)| n == id).unwrap().1.unwrap();
            let delivered = -get("src");
            let sum: f64 = p.iter().map(|(_, v)| v.unwrap()).sum();
            assert!(delivered > 0.0 && get("open") > 0.0 && get("hole") > 0.0);
            assert!(sum.abs() < 1e-12 * delivered, "L{level} f={f}: {p:?}");
        }
    }
}

#[test]
fn rect_duct_limits_are_reported() {
    // A Rect section's first transverse mode is set by its longer side.
    let air = AirState::spec_reference();
    let sec = Section::Rect { a: 2e-3, b: 6e-3 };
    assert_eq!(sec.shape_length(), 1e-3);
    assert!((sec.area() - 12e-6).abs() < 1e-18);
    let s = sec.shear_wavenumber(&air, 2.0 * PI * 1000.0);
    assert!((s - 1e-3 * (2.0 * PI * 1000.0 * air.rho / air.mu).sqrt()).abs() < 1e-12);
}
