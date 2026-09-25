//! Design templates (`examples/design_*.json`, docs/templates.md).
//!
//! * Every template solves, with finite results, at both fidelity levels
//!   across the corners of its parameter ranges and every combination of
//!   its choices.
//! * A parameter the engine reports as inactive has no effect on the result.
//! * Each template's defaults behave as its description says. The oracle is
//!   a lumped network written out here by hand: the driver's primary set,
//!   adiabatic compliances with the first-order wall-layer correction
//!   (spec Section 6), incompressible thermoviscous ducts (slits: tanh form;
//!   tubes: Bessel form by power series), textbook end corrections, meshes
//!   as R_s/A, and the ear simulator's input impedance taken from the engine
//!   (its model is checked in `tests/ear.rs`).

use acoustilab::elements::ducts::step_end_correction;
use acoustilab::expr::PValue;
use acoustilab::params::{Overrides, ParamKind, Parametric};
use acoustilab::{AirState, Circuit, SolveResult, C64};
use serde_json::{json, Value};
use std::f64::consts::PI;
use std::path::Path;

/// Every `examples/design_*.json`, by file stem.
fn templates() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            let stem = p.file_stem()?.to_str()?.to_string();
            (stem.starts_with("design_") && p.extension()? == "json")
                .then(|| (stem, std::fs::read_to_string(&p).unwrap()))
        })
        .collect();
    out.sort();
    out
}

fn template(name: &str) -> Parametric {
    let (_, text) = templates()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("examples/{name}.json"));
    Parametric::parse(&text).unwrap()
}

fn ov(pairs: &[(&str, PValue)]) -> Overrides {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn num(x: f64) -> PValue {
    PValue::Num(x)
}

fn s(x: &str) -> PValue {
    PValue::Str(x.into())
}

fn solve(p: &Parametric, o: &Overrides) -> SolveResult {
    Circuit::from_parametric(p, o)
        .and_then(|c| c.solve())
        .unwrap_or_else(|e| panic!("{o:?}: {e}"))
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

/// Index of the grid point nearest to `f` (log distance).
fn at(r: &SolveResult, f: f64) -> usize {
    (0..r.freqs_hz.len())
        .min_by(|&a, &b| {
            let da = (r.freqs_hz[a] / f).ln().abs();
            let db = (r.freqs_hz[b] / f).ln().abs();
            da.total_cmp(&db)
        })
        .unwrap()
}

fn spl(r: &SolveResult, probe: &str, f: f64) -> f64 {
    db(r.probe(probe).unwrap().values[at(r, f)].norm() / 20e-6)
}

// ------------------------------------------------------------ parameters

/// A numeric, non-derived parameter and its bounds.
struct Range {
    name: String,
    min: f64,
    max: f64,
}

/// Numeric parameters (bounds required), and the values of every discrete
/// one. `fidelity` and `points_per_octave` are set by the tests themselves.
fn ranges(p: &Parametric) -> (Vec<Range>, Vec<(String, Vec<PValue>)>) {
    let mut numeric = Vec::new();
    let mut discrete = Vec::new();
    for d in &p.defs {
        if d.name == "fidelity" || d.name == "points_per_octave" {
            continue;
        }
        match &d.kind {
            ParamKind::Number { min, max, .. } => numeric.push(Range {
                name: d.name.clone(),
                min: min.unwrap_or_else(|| panic!("{} has no min", d.name)),
                max: max.unwrap_or_else(|| panic!("{} has no max", d.name)),
            }),
            ParamKind::Bool { .. } => discrete.push((
                d.name.clone(),
                vec![PValue::Bool(false), PValue::Bool(true)],
            )),
            ParamKind::Choice { choices, .. } => discrete.push((
                d.name.clone(),
                choices.iter().map(|c| s(&c.value)).collect(),
            )),
            ParamKind::Derived { .. } => {}
        }
    }
    (numeric, discrete)
}

/// Every combination of the discrete parameters.
fn combos(discrete: &[(String, Vec<PValue>)]) -> Vec<Overrides> {
    let mut out = vec![Overrides::new()];
    for (name, values) in discrete {
        out = out
            .into_iter()
            .flat_map(|o| {
                values.iter().map(move |v| {
                    let mut o = o.clone();
                    o.insert(name.clone(), v.clone());
                    o
                })
            })
            .collect();
    }
    out
}

/// Deterministic generator for the random corners (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn assert_finite(r: &SolveResult, what: &str) {
    for p in &r.probes {
        for (k, v) in p.values.iter().enumerate() {
            assert!(
                v.re.is_finite() && v.im.is_finite(),
                "{what}: probe {} is {v} at {} Hz",
                p.id,
                r.freqs_hz[k]
            );
        }
    }
}

/// The corners of every template's ranges, at L0 and L1: all at the minimum
/// and all at the maximum for every combination of choices; each parameter
/// alone at either bound; and 64 random corners (each parameter at one of
/// its bounds, random choices and level). The grid is 6 points per octave
/// over the templates' 10 Hz-20 kHz sweep, plus one solve per level on the
/// densest grid.
#[test]
fn every_template_solves_at_the_corners_of_its_ranges() {
    let all = templates();
    assert!(
        all.len() >= 3,
        "{:?}",
        all.iter().map(|t| &t.0).collect::<Vec<_>>()
    );
    let mut solves = 0;
    for (name, text) in &all {
        let p = Parametric::parse(text).unwrap();
        let (numeric, discrete) = ranges(&p);
        let base = |level: u8| {
            let mut o = Overrides::new();
            o.insert("fidelity".into(), num(level as f64));
            o.insert("points_per_octave".into(), num(6.0));
            o
        };
        let mut run = |o: &Overrides| {
            let r = solve(&p, o);
            assert_finite(&r, &format!("{name} {o:?}"));
            solves += 1;
        };
        for level in [0u8, 1] {
            let mut dense = base(level);
            dense.insert("points_per_octave".into(), num(96.0));
            run(&dense);
            let picks: [fn(&Range) -> f64; 2] = [|r| r.min, |r| r.max];
            for c in combos(&discrete) {
                for pick in picks {
                    let mut o = base(level);
                    o.extend(c.clone());
                    for r in &numeric {
                        o.insert(r.name.clone(), num(pick(r)));
                    }
                    run(&o);
                }
            }
            for r in &numeric {
                for v in [r.min, r.max] {
                    let mut o = base(level);
                    o.insert(r.name.clone(), num(v));
                    run(&o);
                }
            }
        }
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ name.len() as u64);
        for _ in 0..64 {
            let mut o = base((rng.next() & 1) as u8);
            for (n, values) in &discrete {
                o.insert(n.clone(), values[rng.below(values.len())].clone());
            }
            for r in &numeric {
                let v = if rng.next() & 1 == 0 { r.min } else { r.max };
                o.insert(r.name.clone(), num(v));
            }
            run(&o);
        }
    }
    assert!(solves > 600, "{solves} solves");
}

/// For every combination of choices, a parameter reported inactive can be
/// moved to either bound without changing any probe value.
#[test]
fn inactive_parameters_have_no_effect() {
    for (name, text) in templates() {
        let p = Parametric::parse(&text).unwrap();
        let (numeric, discrete) = ranges(&p);
        let mut checked = 0;
        for mut c in combos(&discrete) {
            c.insert("points_per_octave".into(), num(6.0));
            let desc = p.describe(&c).unwrap();
            let base = solve(&p, &c);
            for d in desc["parameters"].as_array().unwrap() {
                if d["active"] != json!(false) {
                    continue;
                }
                let n = d["name"].as_str().unwrap();
                let Some(r) = numeric.iter().find(|r| r.name == n) else {
                    continue;
                };
                for v in [r.min, r.max] {
                    let mut o = c.clone();
                    o.insert(n.into(), num(v));
                    let moved = solve(&p, &o);
                    for (a, b) in base.probes.iter().zip(&moved.probes) {
                        assert_eq!(a.id, b.id, "{name} {o:?}");
                        assert_eq!(
                            a.values, b.values,
                            "{name}: inactive {n} = {v} moved {}",
                            a.id
                        );
                    }
                    assert_eq!(base.probes.len(), moved.probes.len());
                }
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "{name}: no inactive parameter in any combination"
        );
    }
}

fn inactive(p: &Parametric, o: &Overrides) -> Vec<String> {
    p.describe(o).unwrap()["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["active"] == json!(false))
        .map(|d| d["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn topology_switches_mark_the_right_parameters_inactive() {
    let has = |list: &[String], names: &[&str]| names.iter().all(|n| list.iter().any(|x| x == n));
    let none = |list: &[String], names: &[&str]| names.iter().all(|n| !list.iter().any(|x| x == n));

    let over = template("design_over_ear");
    let d = inactive(&over, &ov(&[]));
    assert!(
        none(
            &d,
            &[
                "damping_rayl",
                "damping_area_cm2",
                "vent_count",
                "rear_volume_cm3"
            ]
        ),
        "{d:?}"
    );
    assert!(has(&d, &["grille_rayl"]), "{d:?}");
    let d = inactive(&over, &ov(&[("rear", s("open"))]));
    assert!(
        has(
            &d,
            &[
                "vent_count",
                "vent_diameter_mm",
                "vent_mesh_rayl",
                "rear_volume_cm3"
            ]
        ),
        "{d:?}"
    );
    assert!(none(&d, &["grille_rayl", "damping_rayl"]), "{d:?}");
    // Without the cloth the air space behind the diaphragm stays, joined to
    // the rear cavity: only the cloth's area is unused.
    let d = inactive(&over, &ov(&[("damping_rayl", num(0.0))]));
    assert!(has(&d, &["damping_area_cm2"]), "{d:?}");
    assert!(none(&d, &["driver_back_volume_cm3"]), "{d:?}");

    let on = template("design_on_ear");
    let d = inactive(&on, &ov(&[]));
    assert!(
        has(&d, &["custom_gap_mm", "custom_breadth_mm", "grille_rayl"]),
        "{d:?}"
    );
    assert!(
        none(&d, &["leak_gap_mm", "leak_breadth_mm", "concha_volume_cm3"]),
        "{d:?}"
    );
    let d = inactive(&on, &ov(&[("fit", s("sealed"))]));
    assert!(
        has(
            &d,
            &[
                "leak_gap_mm",
                "leak_depth_mm",
                "leak_breadth_mm",
                "custom_gap_mm"
            ]
        ),
        "{d:?}"
    );
    // The pad width still sets the cup radius the rear depth is drawn from.
    assert!(none(&d, &["pad_width_mm"]), "{d:?}");
    let d = inactive(&on, &ov(&[("fit", s("custom"))]));
    assert!(
        none(&d, &["custom_gap_mm", "custom_breadth_mm", "pad_width_mm"]),
        "{d:?}"
    );

    let iem = template("design_in_ear");
    let d = inactive(&iem, &ov(&[]));
    assert!(
        has(
            &d,
            &[
                "leak_diameter_mm",
                "leak_length_mm",
                "custom_leak_diameter_mm"
            ]
        ),
        "{d:?}"
    );
    let d = inactive(&iem, &ov(&[("fit", s("loose"))]));
    assert!(has(&d, &["custom_leak_diameter_mm"]), "{d:?}");
    assert!(none(&d, &["leak_diameter_mm", "leak_length_mm"]), "{d:?}");
    let d = inactive(&iem, &ov(&[("fit", s("custom"))]));
    assert!(none(&d, &["custom_leak_diameter_mm"]), "{d:?}");
    for p in [&on, &iem] {
        let d = inactive(p, &ov(&[("damping_rayl", num(0.0))]));
        assert!(has(&d, &["damping_area_cm2"]), "{d:?}");
        assert!(none(&d, &["driver_back_volume_cm3"]), "{d:?}");
    }
    let d = inactive(&iem, &ov(&[("vent_count", num(0.0))]));
    assert!(
        has(
            &d,
            &["vent_diameter_mm", "vent_length_mm", "vent_mesh_rayl"]
        ),
        "{d:?}"
    );
}

/// Conventions every template follows (docs/templates.md): labelled and
/// grouped parameters, bounded numbers, sourced tolerances, and a `ui` block
/// whose primary probe, ear-load choice and sketch binding exist.
#[test]
fn templates_follow_the_template_conventions() {
    for (name, text) in templates() {
        let doc: Value = serde_json::from_str(&text).unwrap();
        let p = Parametric::parse(&text).unwrap();
        for d in &p.defs {
            assert!(
                d.label.is_some() && d.group.is_some(),
                "{name}: {} needs label and group",
                d.name
            );
            if let Some(t) = &d.tolerance {
                assert!(
                    t.source.as_deref().is_some_and(|s| !s.is_empty()),
                    "{name}: {} tolerance source",
                    d.name
                );
            }
        }
        let ui = &doc["ui"];
        let probes: Vec<&str> = doc["probes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert!(
            probes.contains(&ui["primary_probe"].as_str().unwrap()),
            "{name}"
        );
        let ear = ui["ear_load"].as_str().unwrap();
        assert!(
            matches!(p.def(ear).unwrap().kind, ParamKind::Choice { .. }),
            "{name}"
        );
        let kind = ui["sketch"]["kind"].as_str().unwrap();
        assert_eq!(
            format!("design_{kind}"),
            name,
            "sketch kind names the template"
        );
        let bind = ui["sketch"]["bind"].as_object().unwrap();
        for (slot, param) in bind {
            assert!(
                p.def(param.as_str().unwrap()).is_some(),
                "{name}: slot {slot} -> {param}"
            );
        }
        for (part, list) in ui["sketch"]["parts"].as_object().unwrap() {
            for param in list.as_array().unwrap() {
                assert!(
                    p.def(param.as_str().unwrap()).is_some(),
                    "{name}: part {part} -> {param}"
                );
            }
        }
        for key in ["title", "description"] {
            assert!(
                doc[key].as_str().is_some_and(|s| s.len() > 20),
                "{name}: {key}"
            );
        }
        assert!(
            doc["description"]
                .as_str()
                .unwrap()
                .contains("docs/templates.md"),
            "{name}"
        );
    }
}

// ------------------------------------------------ closed-form oracle

fn j() -> C64 {
    C64::new(0.0, 1.0)
}

fn par(a: C64, b: C64) -> C64 {
    a * b / (a + b)
}

/// Modified Bessel function I_n(z) by its power series (converges for every
/// z; the arguments here have |z| < 60).
fn bessel_i(n: u32, z: C64) -> C64 {
    let q = z * z / 4.0;
    let mut term = (z / 2.0).powu(n) / (1..=n).map(f64::from).product::<f64>();
    let mut sum = term;
    for k in 1..400 {
        term *= q / (k as f64 * (k + n) as f64);
        sum += term;
        if term.norm() < 1e-18 * sum.norm() {
            break;
        }
    }
    sum
}

/// Effective density of a duct's incompressible oscillating flow (Stinson
/// 1991): circles ρ/(I2/I0) at z = k_v·a, slits ρ/(1 − tanh(z)/z) at
/// z = k_v·h/2, with k_v = sqrt(jωρ/μ).
fn rho_circle(air: &AirState, w: f64, a: f64) -> C64 {
    let z = (j() * w * air.rho / air.mu).sqrt() * a;
    air.rho * bessel_i(0, z) / bessel_i(2, z)
}

fn rho_slit(air: &AirState, w: f64, h: f64) -> C64 {
    let z = (j() * w * air.rho / air.mu).sqrt() * (h / 2.0);
    air.rho / (1.0 - z.tanh() / z)
}

/// Compliance impedance with the first-order thermal wall layer:
/// C = V/(γP0)·[1 + ε(1 − j)], ε = (γ − 1)·δt·A/(2V).
fn cavity(air: &AirState, w: f64, volume: f64, area: f64) -> C64 {
    let dt = (2.0 * air.mu / (air.rho * w * air.prandtl)).sqrt();
    let eps = (air.gamma - 1.0) * dt * area / (2.0 * volume);
    let c = volume / (air.gamma * air.p0) * (1.0 + eps * C64::new(1.0, -1.0));
    1.0 / (j() * w * c)
}

/// Wall area the engine assumes for a volume-only cavity (a cube).
fn cube_area(volume: f64) -> f64 {
    6.0 * volume.powf(2.0 / 3.0)
}

fn cylinder(air: &AirState, w: f64, r: f64, d: f64) -> C64 {
    cavity(air, w, PI * r * r * d, 2.0 * PI * r * r + 2.0 * PI * r * d)
}

/// A two-node cylinder cavity (driver face, far face) as a Π section: at L0
/// one compliance at a single node (the engine joins the faces); at L1 half
/// the compliance at each face with the column's air mass ρd/S between them,
/// the first-order form of the depth line. Returns (face 1, series, face 2)
/// impedances; at L0 the series term is zero and face 2 is open.
fn depth_line(air: &AirState, w: f64, r: f64, d: f64, level: u8) -> (C64, C64, C64) {
    if level == 0 {
        return (cylinder(air, w, r, d), C64::new(0.0, 0.0), open());
    }
    let half = cavity(air, w, PI * r * r * d / 2.0, PI * r * r + PI * r * d);
    (half, j() * w * air.rho * d / (PI * r * r), half)
}

fn open() -> C64 {
    C64::new(f64::INFINITY, 0.0)
}

/// Parallel combination of impedances (an open branch is infinite).
fn shunt(zs: &[C64]) -> C64 {
    1.0 / zs
        .iter()
        .filter(|z| z.re.is_finite())
        .map(|z| 1.0 / z)
        .sum::<C64>()
}

/// Ingard's surface resistance, Maa's share per end (R_s/2), referred to area `s`.
fn maa_end(air: &AirState, w: f64, s: f64) -> f64 {
    0.5 * (air.mu * air.rho * w / 2.0).sqrt() / s
}

/// A vent: circular tube, flanged inner end correction 0.8216a (or
/// unflanged 0.6127a), Maa end resistance at both ends, optional mesh R_s
/// over the hole area, and the low-ka radiation load of a baffled piston
/// (R = (ka)²/2·ρc/S, mass 8a/3π) or an unflanged pipe (R = (ka)²/4·ρc/S,
/// mass 0.6127a). `n` holes in parallel.
struct Vent {
    a: f64,
    l: f64,
    n: f64,
    mesh_rayl: f64,
    inner: f64,
    free: bool,
}

impl Vent {
    fn z(&self, air: &AirState, w: f64) -> C64 {
        let sa = PI * self.a * self.a;
        let k = w / air.c;
        let (r_rad, outer) = if self.free {
            (
                (k * self.a).powi(2) / 4.0 * air.rho_c() / sa,
                0.6127 * self.a,
            )
        } else {
            (
                (k * self.a).powi(2) / 2.0 * air.rho_c() / sa,
                8.0 * self.a / (3.0 * PI),
            )
        };
        let z = j() * w * rho_circle(air, w, self.a) * self.l / sa
            + j() * w * air.rho * (self.inner + outer) / sa
            + 2.0 * maa_end(air, w, sa)
            + r_rad
            + self.mesh_rayl / sa;
        z / self.n
    }
}

/// `n` parallel slit segments of height h, breadth w_each, depth l, with the
/// wide-slit end correction (h/π)·[ln(2w/h) + 1/2] at both ends.
fn slits(air: &AirState, w: f64, n: f64, h: f64, breadth: f64, l: f64) -> C64 {
    let sa = breadth * h;
    let end = 2.0 * h / PI * ((2.0 * breadth / h).ln() + 0.5);
    (j() * w * rho_slit(air, w, h) * l / sa + j() * w * air.rho * end / sa) / n
}

/// Input impedance of an ear element on its own, from the engine.
fn ear_impedance(ty: &str, extra: Value, freqs: &[f64], level: u8) -> Vec<C64> {
    let mut ear = json!({"id": "ear", "type": ty, "node": "a"});
    for (k, v) in extra.as_object().unwrap() {
        ear[k] = v.clone();
    }
    let doc = json!({
        "air": {"preset": "standard_23C"}, "level": level,
        "sweep": {"frequencies_Hz": freqs},
        "nodes": [{"id": "a", "domain": "acoustic"}],
        "elements": [{"id": "q", "type": "flow_source", "node": "a", "U_m3_per_s": 1e-6}, ear],
        "probes": [{"id": "p", "quantity": "pressure", "node": "a"}]
    });
    let r = Circuit::from_json(&doc.to_string())
        .unwrap()
        .solve()
        .unwrap();
    r.probe("p")
        .unwrap()
        .values
        .iter()
        .map(|p| p / 1e-6)
        .collect()
}

/// A moving-coil driver from its primary set, driven by an EMF through Re.
struct Driver {
    fs: f64,
    qms: f64,
    qes: f64,
    re: f64,
    mms: f64,
    sd: f64,
}

impl Driver {
    /// Diaphragm volume velocity U = Sd·v for EMF `e`, with the acoustic
    /// load `za` (front plus rear impedance, in series across the piston).
    fn volume_velocity(&self, w: f64, e: f64, za: C64) -> C64 {
        let ws = 2.0 * PI * self.fs;
        let kms = ws * ws * self.mms;
        let rms = ws * self.mms / self.qms;
        let bl2 = ws * self.mms * self.re / self.qes;
        let zm =
            j() * w * self.mms + rms + kms / (j() * w) + bl2 / self.re + self.sd * self.sd * za;
        self.sd * bl2.sqrt() * e / (self.re * zm)
    }
}

fn tymphany() -> Driver {
    Driver {
        fs: 81.8,
        qms: 2.71,
        qes: 1.01,
        re: 32.8,
        mms: 0.3e-3,
        sd: 10e-4,
    }
}

fn grid(r: &SolveResult, lo: f64, hi: f64) -> Vec<usize> {
    (0..r.freqs_hz.len())
        .filter(|&k| r.freqs_hz[k] >= lo && r.freqs_hz[k] <= hi)
        .collect()
}

/// Agreement required between the engine and the oracle. At L0 the engine
/// solves the same lumped network, which the oracle reproduces to about
/// 1e-5 dB (observed 1e-8 to 7e-6 dB); with the on-ear template's wide slit
/// leaks the oracle's end correction (the wide-slit asymptote instead of the
/// rectangular-piston integral) differs by up to 0.003 dB. At L1 the
/// oracle's Π sections are the first-order form of the engine's lines;
/// below 500 Hz the remainder, O((kd)²), stays under 0.035 dB.
fn tolerance(level: u8, wide_slit: bool) -> f64 {
    match (level, wide_slit) {
        (0, false) => 1e-4,
        (0, true) => 0.01,
        _ => 0.05,
    }
}

/// Largest |dB difference| between a probe and the closed form over [lo, hi].
fn worst(
    r: &SolveResult,
    probe: &str,
    lo: f64,
    hi: f64,
    oracle: impl Fn(f64, usize) -> C64,
) -> f64 {
    let p = &r.probe(probe).unwrap().values;
    grid(r, lo, hi)
        .into_iter()
        .map(|k| db(p[k].norm() / oracle(r.freqs_hz[k], k).norm()).abs())
        .fold(0.0, f64::max)
}

/// The closed rear shared by the over-ear and on-ear templates: the air space
/// behind the diaphragm, the damping cloth (R_s/A) into the rear cavity, and
/// the meshed vent to the room. Without the cloth the air space opens
/// straight into the rear cavity.
fn closed_rear(
    air: &AirState,
    w: f64,
    damping: Option<(f64, f64)>,
    rear_cm3: f64,
    vent: &Vent,
) -> C64 {
    let back_v = 1e-6;
    let rear_v = rear_cm3 * 1e-6;
    let zback = cavity(air, w, back_v, cube_area(back_v));
    let zr = par(cavity(air, w, rear_v, cube_area(rear_v)), vent.z(air, w));
    match damping {
        Some((rayl, area)) => par(zback, rayl / area + zr),
        None => par(zback, zr),
    }
}

// ----------------------------------------------------------- over-ear

/// The over-ear defaults (closed back, damping cloth, fixture leak) against
/// the lumped network, and with the cloth removed: the front pressure over
/// 20 Hz-2 kHz at L0 and 20-500 Hz at L1 (see [`tolerance`]).
#[test]
fn over_ear_matches_its_lumped_network() {
    let p = template("design_over_ear");
    let air = AirState::standard_23c();
    let d = tymphany();
    let e = (1e-3f64 * 32.0).sqrt();
    let (r, depth) = (27.5e-3, 20e-3);
    let vent = Vent {
        a: 1.5e-3,
        l: 2e-3,
        n: 1.0,
        mesh_rayl: 160.0,
        inner: 0.8216 * 1.5e-3,
        free: false,
    };
    for (damping, level, hi) in [
        (true, 0u8, 2000.0),
        (true, 1, 500.0),
        (false, 0, 2000.0),
        (false, 1, 500.0),
    ] {
        let mut o = ov(&[("fidelity", num(level as f64))]);
        if !damping {
            o.insert("damping_rayl".into(), num(0.0));
        }
        let res = solve(&p, &o);
        let zear = ear_impedance("iec60318_4", json!({}), &res.freqs_hz, level);
        let oracle = |f: f64, k: usize| {
            let w = 2.0 * PI * f;
            let perimeter = 2.0 * PI * r;
            let zleak = slits(&air, w, 8.0, 0.08e-3, perimeter / 8.0, 15e-3);
            let (z1, zm, z2) = depth_line(&air, w, r, depth, level);
            let zf = shunt(&[z1, zm + shunt(&[z2, zleak, zear[k]])]);
            let zb = closed_rear(&air, w, damping.then_some((600.0, 2e-4)), 25.0, &vent);
            d.volume_velocity(w, e, zf + zb) * zf
        };
        let err = worst(&res, "p_front", 20.0, hi, oracle);
        assert!(
            err < tolerance(level, false),
            "damping {damping}, L{level}: {err} dB"
        );
    }
}

/// What the description claims for the defaults: the damped coupled
/// resonance leaves no peak above the bass level at the drum below 2 kHz,
/// the input impedance stays within 10 % of Re, and the front volume is the
/// 47.5 cm³ of docs/templates.md. Without the cloth the diaphragm resonates
/// on the cup's air springs at 0.5-1.2 kHz, at least 8 dB above the bass.
#[test]
fn over_ear_defaults_are_damped() {
    let p = template("design_over_ear");
    let res = solve(&p, &ov(&[]));
    let v = res.meta.parameters["front_volume_cm3"].as_f64().unwrap();
    assert!((v - PI * 27.5 * 27.5 * 20.0 / 1000.0).abs() < 1e-9 && (40.0..60.0).contains(&v));
    let base = spl(&res, "p_drp", 100.0);
    for k in grid(&res, 100.0, 2000.0) {
        let l = db(res.probe("p_drp").unwrap().values[k].norm() / 20e-6);
        assert!(l < base + 0.5, "{} Hz: {l} vs {base}", res.freqs_hz[k]);
    }
    let re = 32.8;
    let zmax = res
        .probe("zin")
        .unwrap()
        .magnitude()
        .into_iter()
        .fold(0.0, f64::max);
    assert!(zmax > re && zmax < 1.1 * re, "{zmax}");

    let bare = solve(&p, &ov(&[("damping_rayl", num(0.0))]));
    let (k, peak) = grid(&bare, 300.0, 2000.0)
        .into_iter()
        .map(|k| (k, db(bare.probe("p_drp").unwrap().values[k].norm() / 20e-6)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!(peak > spl(&bare, "p_drp", 100.0) + 8.0, "{peak}");
    assert!(
        (500.0..1200.0).contains(&bare.freqs_hz[k]),
        "{}",
        bare.freqs_hz[k]
    );
}

// ------------------------------------------------------------- on-ear

/// The on-ear leak states against the lumped network (L0, 20 Hz-2 kHz; L1,
/// 20-500 Hz). The P.57 Type 3.2 slits: 0.26 mm × 2.8 mm over 84° as two
/// slits, 0.50 mm × 1.9 mm over 240° as one, at mid-depth of the 25 mm
/// cavity's rim.
#[test]
fn on_ear_matches_its_lumped_network() {
    let p = template("design_on_ear");
    let air = AirState::standard_23c();
    let d = tymphany();
    let e = (1e-3f64 * 32.0).sqrt();
    let vent = Vent {
        a: 1e-3,
        l: 1.5e-3,
        n: 1.0,
        mesh_rayl: 160.0,
        inner: 0.8216 * 1e-3,
        free: false,
    };
    let concha = 4.3e-6;
    for (fit, level, hi) in [
        ("sealed", 0u8, 2000.0),
        ("low", 0, 2000.0),
        ("high", 0, 2000.0),
        ("low", 1, 500.0),
        ("high", 1, 500.0),
    ] {
        let res = solve(&p, &ov(&[("fit", s(fit)), ("fidelity", num(level as f64))]));
        let zear = ear_impedance("type33", json!({}), &res.freqs_hz, level);
        let oracle = |f: f64, k: usize| {
            let w = 2.0 * PI * f;
            let arc = |deg: f64, depth: f64| deg * PI / 180.0 * (12.5e-3 + depth / 2.0);
            let zleak = match fit {
                "low" => slits(&air, w, 2.0, 0.26e-3, arc(84.0, 2.8e-3) / 2.0, 2.8e-3),
                "high" => slits(&air, w, 1.0, 0.5e-3, arc(240.0, 1.9e-3), 1.9e-3),
                _ => open(),
            };
            let (z1, zm, z2) = depth_line(&air, w, 18e-3, 6e-3, level);
            let zconcha = cavity(&air, w, concha, cube_area(concha));
            let zf = shunt(&[z1, zm + shunt(&[z2, zconcha, zleak, zear[k]])]);
            let zb = closed_rear(&air, w, Some((600.0, 2e-4)), 12.0, &vent);
            d.volume_velocity(w, e, zf + zb) * zf
        };
        let err = worst(&res, "p_front", 20.0, hi, oracle);
        assert!(
            err < tolerance(level, fit != "sealed"),
            "{fit}, L{level}: {err} dB"
        );
    }
}

/// Spec Section 18: with a small front chamber the bass is set by the leak.
/// At 100 Hz the level falls by more than 10 dB from sealed to the low leak
/// and again to the high leak; in the low-leak state most of the
/// diaphragm's volume velocity leaves through the slit.
#[test]
fn on_ear_bass_is_set_by_the_leak() {
    let p = template("design_on_ear");
    let at100 = |fit: &str| {
        let r = solve(&p, &ov(&[("fit", s(fit))]));
        (spl(&r, "p_drp", 100.0), r)
    };
    let (sealed, _) = at100("sealed");
    let (low, r) = at100("low");
    let (high, _) = at100("high");
    assert!(
        low < sealed - 10.0 && high < low - 10.0,
        "{sealed} {low} {high}"
    );
    let k = at(&r, 100.0);
    let w = 2.0 * PI * r.freqs_hz[k];
    let u_diaphragm = (j() * w * r.probe("x").unwrap().values[k] * 10e-4).norm();
    let u_leak = r.probe("u_leak").unwrap().values[k].norm();
    assert!(u_leak > 0.8 * u_diaphragm, "{u_leak} vs {u_diaphragm}");
}

// -------------------------------------------------------------- in-ear

/// The IEM network (L0, 20 Hz-2 kHz; L1, 20-500 Hz) sealed and with the
/// loose-fit leak tube: diaphragm, front volume, nozzle (Bessel-form tube,
/// flanged inlet), area step into the 7.5 mm canal (the engine's static
/// step correction), the coupler, the damped rear and the meshed relief
/// vent. At L1 the nozzle carries half its air volume at each end.
#[test]
fn in_ear_matches_its_lumped_network() {
    let p = template("design_in_ear");
    let air = AirState::standard_23c();
    let (fs, mms, re, bl, sd) = (255.0, 17.5e-6, 14.4, 0.46, 0.8e-4);
    let d = Driver {
        fs,
        qms: 3.0,
        qes: 2.0 * PI * fs * mms * re / (bl * bl),
        re,
        mms,
        sd,
    };
    let e = (1e-3f64 * 16.0).sqrt();
    let rd = (sd / PI).sqrt();
    let depth = |v: f64| v / (PI * rd * rd);
    let vent = Vent {
        a: 0.25e-3,
        l: 1.5e-3,
        n: 1.0,
        mesh_rayl: 260.0,
        inner: 0.8216 * 0.25e-3,
        free: false,
    };
    let leak = Vent {
        a: 0.508e-3,
        l: 13e-3,
        n: 1.0,
        mesh_rayl: 0.0,
        inner: 0.6127 * 0.508e-3,
        free: true,
    };
    for (fit, level, hi) in [
        ("sealed", 0u8, 2000.0),
        ("loose", 0, 2000.0),
        ("sealed", 1, 500.0),
        ("loose", 1, 500.0),
    ] {
        let res = solve(&p, &ov(&[("fit", s(fit)), ("fidelity", num(level as f64))]));
        let zear = ear_impedance("iec60318_4", json!({}), &res.freqs_hz, level);
        let oracle = |f: f64, k: usize| {
            let w = 2.0 * PI * f;
            let a = 1e-3;
            let sa = PI * a * a;
            let step = step_end_correction(a / 3.75e-3) * a;
            let znoz =
                j() * w * rho_circle(&air, w, a) * 7e-3 / sa + j() * w * air.rho * 0.8216 * a / sa;
            let zstep = j() * w * air.rho * step / sa;
            let zleak = if fit == "loose" {
                leak.z(&air, w)
            } else {
                open()
            };
            let ztip = shunt(&[zear[k], zleak]);
            // At L1 the nozzle is a line: half its air volume at each end.
            let zhalf = if level == 0 {
                open()
            } else {
                cavity(&air, w, sa * 7e-3 / 2.0, PI * a * 7e-3 + sa)
            };
            let zout = shunt(&[zhalf, zstep + ztip]);
            let zf = shunt(&[cylinder(&air, w, rd, depth(0.1e-6)), zhalf, znoz + zout]);
            let zr = par(cylinder(&air, w, rd, depth(0.5e-6)), vent.z(&air, w));
            let zb = par(cylinder(&air, w, rd, depth(0.15e-6)), 300.0 / 0.1e-4 + zr);
            let u = d.volume_velocity(w, e, zf + zb);
            // Pressure at the canal entrance: the front pressure divided
            // along the nozzle and the area step.
            u * zf * zout / (znoz + zout) * ztip / (zstep + ztip)
        };
        let err = worst(&res, "p_tip", 20.0, hi, oracle);
        assert!(err < tolerance(level, false), "{fit}, L{level}: {err} dB");
    }
}

/// Sealed, the IEM is a pressure chamber: the drum level is flat within
/// 1 dB from 50 Hz up to a third of the in-situ resonance (the input
/// impedance maximum). The fit states lose bass in order: at 50 Hz the
/// slight leak is at least 1 dB and the loose fit at least 15 dB below
/// sealed.
#[test]
fn in_ear_is_flat_sealed_and_loses_bass_when_loose() {
    let p = template("design_in_ear");
    let sealed = solve(&p, &ov(&[]));
    let z = sealed.probe("zin").unwrap().magnitude();
    let kz = grid(&sealed, 300.0, 3000.0)
        .into_iter()
        .max_by(|&a, &b| z[a].total_cmp(&z[b]))
        .unwrap();
    let fc = sealed.freqs_hz[kz];
    assert!((800.0..3000.0).contains(&fc), "{fc}");
    let band: Vec<f64> = grid(&sealed, 50.0, fc / 3.0)
        .into_iter()
        .map(|k| db(sealed.probe("p_drp").unwrap().values[k].norm() / 20e-6))
        .collect();
    let spread = band.iter().cloned().fold(f64::MIN, f64::max)
        - band.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        band.len() > 10 && spread < 1.0,
        "{spread} dB over 50 Hz-{} Hz",
        fc / 3.0
    );

    let l50 = |fit: &str| spl(&solve(&p, &ov(&[("fit", s(fit))])), "p_drp", 50.0);
    let (s0, typical, loose) = (spl(&sealed, "p_drp", 50.0), l50("typical"), l50("loose"));
    assert!(
        typical < s0 - 1.0 && loose < s0 - 15.0 && loose < typical,
        "{s0} {typical} {loose}"
    );
}
