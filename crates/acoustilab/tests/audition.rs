//! Audition filters (spec Section 16): the FIR against the analytic ratio,
//! minimum-phase causality and phase, mixed phase with delay alignment and
//! the hybrid, linear phase, the E46 length rule, the regularised inversion
//! of a target (boost cap, notch rule, band limit), rates, determinism and
//! the serialised state.
//!
//! References: closed forms of ideal electrical networks evaluated here
//! (independently of the engine's solver), the FIR's frequency response
//! computed here by a direct sum over its taps (independently of
//! `audition::dtft`), and `tools/audition/reference.py` (numpy/scipy) for
//! the inversion and the target-baseline filter
//! (`tests/data/audition_reference.json`). Each tolerance is stated where
//! it is used.

use acoustilab::audition::report::{parse_options, target_baseline, to_json};
use acoustilab::audition::sha256::sha256_hex;
use acoustilab::audition::{
    audition, n_default, Audition, Baseline, Design, MagnitudeBaseline, Options, PhaseRequest,
};
use acoustilab::params::Overrides;
use acoustilab::C64;
use serde_json::{json, Value};
use std::f64::consts::PI;

fn fixture() -> Value {
    let path = format!(
        "{}/tests/data/audition_reference.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
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

fn design(text: &str, overrides: Value) -> Design {
    let ov: Overrides = overrides
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), acoustilab::expr::PValue::from_json(v).unwrap()))
        .collect();
    Design::new(text, &ov, overrides, None).unwrap_or_else(|e| panic!("{e}"))
}

fn opts(v: Value) -> Options {
    parse_options(&v).unwrap().0
}

/// H(f) of the taps, by a direct sum of h[n]·e^{−j2πfn/fs}.
fn response(taps: &[f32], fs: f64, f: f64) -> C64 {
    let w = 2.0 * PI * f / fs;
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &h) in taps.iter().enumerate() {
        let (s, c) = (w * n as f64).sin_cos();
        re += h as f64 * c;
        im -= h as f64 * s;
    }
    C64::new(re, im)
}

fn wrap(x: f64) -> f64 {
    x - 2.0 * PI * (x / (2.0 * PI)).round()
}

/// Mean of dB values over 500 Hz–2 kHz with the log-interval weights of
/// the anchor (as documented).
fn anchor(grid: &[f64], db: &[f64]) -> f64 {
    let idx: Vec<usize> = (0..grid.len())
        .filter(|&i| grid[i] >= 500.0 * (1.0 - 1e-9) && grid[i] <= 2000.0 * (1.0 + 1e-9))
        .collect();
    let (mut sw, mut s) = (0.0, 0.0);
    for (j, &i) in idx.iter().enumerate() {
        let a = if j == 0 {
            grid[i]
        } else {
            (grid[idx[j - 1]] * grid[i]).sqrt()
        };
        let b = if j + 1 == idx.len() {
            grid[i]
        } else {
            (grid[i] * grid[idx[j + 1]]).sqrt()
        };
        sw += (b / a).ln();
        s += (b / a).ln() * db[i];
    }
    s / sw
}

// ----- Electrical networks with closed forms -----------------------------------

fn vin() -> Value {
    json!({"id": "v", "type": "vsource", "node": "in", "V_V": 1})
}

fn netlist(nodes: &[&str], elements: Vec<Value>, probe: Value) -> String {
    json!({
        "schema": "acoustilab-netlist/0.2",
        "sweep": {"f_min_Hz": 10, "f_max_Hz": 20000, "points_per_octave": 24},
        "nodes": nodes.iter().map(|n| json!({"id": n, "domain": "electrical"})).collect::<Vec<_>>(),
        "elements": elements,
        "probes": [probe],
    })
    .to_string()
}

/// Series R-L into C to ground: V(C)/V = 1/(1 + sRC + s²LC).
#[derive(Clone, Copy)]
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
    fn h(&self, f: f64) -> C64 {
        let s = C64::new(0.0, 2.0 * PI * f);
        (1.0 + s * self.r * self.c + s * s * self.l * self.c).inv()
    }
    fn text(&self) -> String {
        netlist(
            &["in", "a", "out"],
            vec![
                vin(),
                json!({"id": "r", "type": "resistor", "nodes": ["in", "a"], "R_ohm": self.r}),
                json!({"id": "l", "type": "inductor", "nodes": ["a", "out"], "L_H": self.l}),
                json!({"id": "c", "type": "capacitor", "node": "out", "C_F": self.c}),
            ],
            json!({"id": "out", "quantity": "voltage", "node": "out"}),
        )
    }
}

fn run(cand: &mut Design, base: Baseline, o: Value) -> Audition {
    audition(cand, base, &opts(o)).unwrap_or_else(|e| panic!("{e}"))
}

/// The FIR against the closed-form ratio of two minimum-phase low-passes
/// (1 kHz, Q = 2 over 1.5 kHz, Q = 0.7): a 9 dB peak near 1 kHz and a
/// shelf of 20·log10(L₂C₂/L₁C₁) = 7.0 dB above it.
///
/// * The analytic filter is the engine's exact solves: it equals the
///   closed form to 1e-9 dB.
/// * The taps reproduce it within 0.1 dB from 20 Hz to 20 kHz (Section
///   16); observed 3e-5 dB, asserted 0.01 dB.
/// * The ratio is minimum phase, so the minimum-phase filter's phase is
///   the ratio's own phase. The filter is band-limited (held outside
///   20 Hz–20 kHz, which changes the magnitude little here: the ratio is
///   flat below 100 Hz and above 5 kHz). A discrete filter's minimum phase
///   is the Hilbert transform of its log magnitude over 0 to Nyquist only,
///   which differs from the analog one (over all frequencies) by a
///   near-constant group delay: here 0.38 µs (0.018 sample). With that
///   delay fitted and removed, the phases agree to 0.05° up to 10 kHz
///   (observed 0.020°: the difference is not exactly a delay).
/// * The taps are the minimum-phase sequence of their own magnitude: their
///   phase equals the one computed here from a real cepstrum of their log
///   magnitude on a grid 16 times finer (engine FFT, which is checked
///   against numpy in `tests/time.rs`), to 1e-6° (observed 7e-14°: the
///   window has nothing left to cut, the tail being at −250 dB).
#[test]
fn ratio_of_closed_forms() {
    let (a, b) = (Lp::new(1000.0, 2.0, 1e-6), Lp::new(1500.0, 0.7, 1e-6));
    let mut ca = design(&a.text(), json!({}));
    let mut cb = design(&b.text(), json!({}));
    let r = run(&mut ca, Baseline::Design(&mut cb), json!({}));
    assert_eq!(r.phase.used, "minimum");
    assert_eq!((r.n, r.latency_samples), (8192, 0));
    let exact: Vec<f64> = r
        .grid_hz
        .iter()
        .map(|&f| 20.0 * (a.h(f) / b.h(f)).norm().log10())
        .collect();
    let off = anchor(&r.grid_hz, &exact);
    let mut worst_fir = 0.0f64;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    let mut errs = Vec::new();
    for (i, &f) in r.grid_hz.iter().enumerate() {
        assert!(
            (r.design_db[i] - (exact[i] - off)).abs() < 1e-9,
            "{f} Hz: {} vs {}",
            r.design_db[i],
            exact[i] - off
        );
        let h = response(&r.taps, r.fs_hz, f);
        worst_fir = worst_fir.max((20.0 * h.norm().log10() - r.design_db[i]).abs());
        if f <= 10_000.0 {
            let e = wrap(h.arg() - (a.h(f) / b.h(f)).arg());
            let w = 2.0 * PI * f;
            sxy += w * e;
            sxx += w * w;
            errs.push((w, e));
        }
    }
    assert!(worst_fir < 0.01, "{worst_fir} dB");
    // Phase error = −ω·δ: the least-squares delay δ through the origin.
    let delay = -sxy / sxx;
    assert!(delay.abs() < 1e-6, "{delay} s");
    let worst_phase = errs
        .iter()
        .map(|(w, e)| (e + w * delay).to_degrees().abs())
        .fold(0.0, f64::max);
    assert!(
        worst_phase < 0.05,
        "{worst_phase} degrees after removing {delay} s"
    );
    // Minimum phase of its own magnitude: cepstrum on 16·N points.
    let m = 16 * r.n;
    let mut buf: Vec<C64> = (0..m)
        .map(|i| C64::new(if i < r.n { r.taps[i] as f64 } else { 0.0 }, 0.0))
        .collect();
    let fft = acoustilab::time::fft::Fft::new(m);
    fft.forward(&mut buf);
    let spectrum = buf.clone();
    let mut cep: Vec<C64> = buf.iter().map(|v| C64::new(v.norm().ln(), 0.0)).collect();
    fft.inverse(&mut cep);
    for (i, v) in cep.iter_mut().enumerate() {
        *v = match i {
            0 => C64::new(v.re, 0.0),
            i if i < m / 2 => C64::new(2.0 * v.re, 0.0),
            i if i == m / 2 => C64::new(v.re, 0.0),
            _ => C64::new(0.0, 0.0),
        };
    }
    fft.forward(&mut cep);
    let mut worst_min = 0.0f64;
    for k in (1..m / 2).step_by(7) {
        let f = k as f64 * r.fs_hz / m as f64;
        if !(20.0..=20_000.0).contains(&f) {
            continue;
        }
        let e = wrap(spectrum[k].arg() - cep[k].im).to_degrees().abs();
        worst_min = worst_min.max(e);
    }
    assert!(
        worst_min < 1e-6,
        "{worst_min} degrees from the cepstral minimum phase"
    );
    let c = r.check.as_ref().unwrap();
    assert!(c.met && (c.max_abs_error_db - worst_fir).abs() < 1e-6);
    // Causal by construction: before the window, the last N/16 samples
    // hold nothing of consequence.
    assert!(r.tail_energy_db < -150.0, "{}", r.tail_energy_db);
    // The largest boost (the peak near 1 kHz, re the anchor band) and cut
    // (the −7 dB shelf at 20 kHz, re the anchor) are the closed form's.
    let (i_max, max) =
        exact.iter().enumerate().fold(
            (0, f64::MIN),
            |m, (i, v)| if *v > m.1 { (i, *v) } else { m },
        );
    let min = exact.iter().cloned().fold(f64::MAX, f64::min);
    assert!((r.max_boost_db.0 - (max - off)).abs() < 1e-9 && r.max_boost_db.1 == r.grid_hz[i_max]);
    assert!(
        (800.0..1300.0).contains(&r.max_boost_db.1),
        "{:?}",
        r.max_boost_db
    );
    assert!((r.max_cut_db.0 - (min - off)).abs() < 1e-9);
}

/// Mixed phase with delay alignment (Section 16, mode 2). The candidate
/// is an all-pass lattice, (1 − sRC)/(1 + sRC) with RC = 0.5 ms, whose
/// group delay is 2RC/(1 + (ωRC)²), 1 ms at DC; the baseline a divider
/// (0.5). The filter's excess group delay (1 ms) fails the 0.5 ms test, so
/// `auto` takes mixed phase, and the filter's phase is the model's own
/// ratio advanced by the pure-delay estimate (the smallest excess group
/// delay in the band, 0.2 µs at 21.6 kHz), delayed by the `pre` samples.
/// Magnitude flat (0 dB) within 0.001 dB up to 10 kHz (observed 5e-6 dB;
/// the check's largest error is 0.011 dB, at 20 kHz); phase within 0.01°
/// over 20 Hz to 10 kHz (observed 1.2e-4°; the phase is crossfaded to
/// minimum phase only above the 20 kHz band top). The
/// crossfade takes the accumulated −178° down to −360°, not back to 0:
/// the energy before t = 0 is then −12.2 dB of the filter's (the
/// band-limited pulse's own neighbours within 0.13 ms), against −5.7 dB
/// the other way (docs/auralization.md).
#[test]
fn all_pass_goes_mixed_with_delay_alignment() {
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
    );
    let divider = netlist(
        &["in", "a"],
        vec![
            vin(),
            json!({"id": "ra", "type": "resistor", "nodes": ["in", "a"], "R_ohm": r}),
            json!({"id": "rb", "type": "resistor", "node": "a", "R_ohm": r}),
            json!({"id": "diff", "type": "isource", "node": "a", "I_A": 0}),
        ],
        json!({"id": "out", "quantity": "port_potential", "element": "diff"}),
    );
    let mut ca = design(&lattice, json!({}));
    let mut cb = design(&divider, json!({}));
    let a = run(&mut ca, Baseline::Design(&mut cb), json!({}));
    assert_eq!(a.phase.used, "mixed", "{}", a.phase.reason);
    let d = &a.phase.decision;
    assert!(
        (d.max_excess_gd_s / 1e-3 - 1.0).abs() < 2e-3,
        "{}",
        d.max_excess_gd_s
    );
    let tau = a.phase.delay_removed_s;
    assert!(tau > 0.0 && tau < 1e-6, "{tau}");
    assert_eq!(a.latency_samples, 8192 / 32);
    let rc = r * c;
    let (mut worst_mag, mut worst_ph) = (0.0f64, 0.0f64);
    for &f in a.grid_hz.iter().filter(|f| **f <= 10_000.0) {
        let w = 2.0 * PI * f;
        let h = response(&a.taps, a.fs_hz, f);
        worst_mag = worst_mag.max((20.0 * h.norm().log10()).abs());
        let want = -2.0 * (w * rc).atan() + w * tau - w * a.latency_samples as f64 / a.fs_hz;
        worst_ph = worst_ph.max(wrap(h.arg() - want).to_degrees().abs());
    }
    assert!(worst_mag < 0.001, "{worst_mag} dB");
    assert!(worst_ph < 0.01, "{worst_ph} degrees");
    assert!(a.check.as_ref().unwrap().met);
    let pre = a.pre_energy_db.unwrap();
    assert!(pre < -12.0 && pre > -12.5, "{pre}");
    assert!(a.tail_energy_db < -90.0, "{}", a.tail_energy_db);
    // Minimum phase, if forced, has the same magnitude but no excess
    // phase: its phase is 0 wherever the magnitude is flat.
    let m = run(
        &mut ca,
        Baseline::Design(&mut cb),
        json!({"phase": "minimum"}),
    );
    let h = response(&m.taps, m.fs_hz, 1000.0);
    assert!(
        h.arg().to_degrees().abs() < 0.05,
        "{}",
        h.arg().to_degrees()
    );
}

/// The hybrid of Section 16 on the design template (3 vents against the
/// template's one), with mixed phase forced: below the validity frequency
/// (the rear cavity's lumped limit, 1011 Hz) the filter is the minimum
/// phase of its magnitude times the model's excess phase (delay aligned);
/// from a third of an octave above it, the minimum-phase filter.
///
/// * Below f_v the phase follows the model's ratio (delay aligned) to 1°
///   from 50 Hz (observed 0.55°): the two differ by the minimum phase of
///   the band-limited (held) magnitude against that of the model's whole
///   magnitude. The reference is the engine's exact solves at each
///   frequency, not its FFT-grid spectra.
/// * Above it the phase is the minimum-phase filter's to 0.01° (observed
///   4e-5°).
/// * The taps meet 0.1 dB from 20 Hz to 20 kHz (observed 0.028 dB), the
///   tail is below −80 dB (observed −83 dB) and the pre-response below
///   −80 dB (observed −86 dB). Before the review the mixed filter carried
///   the model's own phase in band, paired with the held magnitude
///   outside it: 0.14 dB at 20 Hz on this template (tolerance not met),
///   tail −60 dB, pre-response −63 dB.
#[test]
fn mixed_phase_is_minimum_above_the_validity_frequency() {
    let text = example("design_over_ear.json");
    let mut cand = design(&text, json!({"vent_count": 3}));
    let mut base = design(&text, json!({}));
    let mixed = run(
        &mut cand,
        Baseline::Design(&mut base),
        json!({"phase": "mixed"}),
    );
    let minimum = run(
        &mut cand,
        Baseline::Design(&mut base),
        json!({"phase": "minimum"}),
    );
    // f_v is the lower of the designs' 10 % lumped-error frequencies (the
    // FFT grid's reading of it; the netlists' 24-per-octave sweeps give
    // it to their spacing): 1011 Hz for the template.
    let fv = mixed.phase.hybrid_from_hz.unwrap();
    let begin = |d: &Design| d.shading.begin_hz.unwrap();
    let want = begin(&cand).min(begin(&base));
    assert!((fv / want - 1.0).abs() < 0.03, "{fv} Hz against {want} Hz");
    assert!((fv - 1011.2).abs() < 1.0, "{fv}");
    let pre = mixed.latency_samples as f64 / mixed.fs_hz;
    let tau = mixed.phase.delay_removed_s;
    let (mut below, mut above) = (0.0f64, 0.0f64);
    for &f in &mixed.grid_hz {
        let w = 2.0 * PI * f;
        let h = response(&mixed.taps, mixed.fs_hz, f) * C64::from_polar(1.0, w * pre);
        if (50.0..=fv).contains(&f) {
            let ratio = cand.value_at(f).unwrap() / base.value_at(f).unwrap();
            below = below.max(wrap(h.arg() - ratio.arg() - w * tau).to_degrees().abs());
        } else if f >= fv * 2f64.powf(1.0 / 3.0) && f <= 16_000.0 {
            let hm = response(&minimum.taps, minimum.fs_hz, f);
            above = above.max(wrap(h.arg() - hm.arg()).to_degrees().abs());
        }
    }
    assert!(below < 1.0, "{below} degrees");
    assert!(above < 0.01, "{above} degrees");
    let c = mixed.check.as_ref().unwrap();
    assert!(c.met && c.max_abs_error_db < 0.05, "{c:?}");
    assert!(mixed.tail_energy_db < -80.0, "{}", mixed.tail_energy_db);
    assert!(mixed.pre_energy_db.unwrap() < -80.0);
    assert!(mixed.flags.is_empty(), "{:?}", mixed.flags);
}

/// Linear phase: the taps are symmetric about N/2 (f32 rounding aside),
/// the latency is N/2 and the diagnostic flag is raised.
#[test]
fn linear_phase_is_symmetric() {
    let text = example("design_over_ear.json");
    let mut cand = design(&text, json!({"vent_count": 3}));
    let mut base = design(&text, json!({}));
    let a = run(
        &mut cand,
        Baseline::Design(&mut base),
        json!({"phase": "linear"}),
    );
    let n = a.n;
    assert_eq!(a.latency_samples, n / 2);
    let peak = a.taps.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    for m in 1..n / 2 {
        let (x, y) = (a.taps[n / 2 + m], a.taps[n / 2 - m]);
        assert!((x - y).abs() <= 1e-6 * peak, "{m}: {x} {y}");
    }
    assert!(a.flags.iter().any(|f| f.code == "linear_phase_diagnostic"));
    assert!(a.check.as_ref().unwrap().met);
}

/// The E46 rule sizes N from the filter's lowest high-Q pole. A 200 Hz,
/// Q = 8 resonance (against a flat divider) needs 2.93·Q/f = 0.117 s,
/// so N = 16384 at 48 kHz (0.171 s), and the check is met. A 100 Hz,
/// Q = 20 resonance needs 0.586 s: N = 65536, beyond the audition limit
/// of 16384; the filter is capped and flagged. Its decay (−46 dB at N/fs)
/// wraps around the buffer, and the report says where the 0.1 dB criterion
/// fails: around the resonance, and wherever the ratio lies far below its
/// peak (above 230 Hz it falls 12 dB/octave, so an error −60 dB re the peak
/// is several dB there).
#[test]
fn e46_sizes_the_filter() {
    let divider = netlist(
        &["in", "out"],
        vec![
            vin(),
            json!({"id": "ra", "type": "resistor", "nodes": ["in", "out"], "R_ohm": 1000.0}),
            json!({"id": "rb", "type": "resistor", "node": "out", "R_ohm": 1000.0}),
        ],
        json!({"id": "out", "quantity": "voltage", "node": "out"}),
    );
    let mut base = design(&divider, json!({}));
    let lp = Lp::new(200.0, 8.0, 1e-5);
    let mut cand = design(&lp.text(), json!({}));
    let a = run(&mut cand, Baseline::Design(&mut base), json!({}));
    let l = a.length.as_ref().unwrap();
    let b = l.binding.as_ref().unwrap();
    assert!((b.f_hz / 200.0 - 1.0).abs() < 1e-3 && (b.q / 8.0 - 1.0).abs() < 1e-2);
    assert!(
        (b.needed_s - 2.9317 * 8.0 / 200.0).abs() < 2e-3,
        "{}",
        b.needed_s
    );
    assert_eq!((l.recommended_n, a.n), (16384, 16384));
    assert!(l.covered);
    assert!(a.check.as_ref().unwrap().met, "{:?}", a.check);

    let lp = Lp::new(100.0, 20.0, 1e-5);
    let mut cand = design(&lp.text(), json!({}));
    let a = run(&mut cand, Baseline::Design(&mut base), json!({}));
    let l = a.length.as_ref().unwrap();
    assert_eq!((l.recommended_n, a.n), (65536, 16384));
    assert!(!l.covered);
    assert!(a.flags.iter().any(|f| f.code == "ir_length_short"));
    let c = a.check.as_ref().unwrap();
    assert!(!c.met);
    assert!(
        c.exceeded_hz
            .iter()
            .any(|(lo, hi)| *lo < 100.0 && *hi > 100.0 || (*lo > 90.0 && *hi < 110.0)),
        "{:?}",
        c.exceeded_hz
    );
    assert!(a.flags.iter().any(|f| f.code == "tolerance_not_met"));
    // An explicit N outside the range at the rate is refused.
    assert!(parse_options(&json!({"n": 32768})).is_err());
    assert!(parse_options(&json!({"n": 32768, "fs_Hz": 96000})).is_ok());
}

/// The template against variants: every minimum-phase filter meets the
/// 0.1 dB of Section 16 over 20 Hz–20 kHz at N = 8192 (observed 0.0006 to
/// 0.012 dB, the largest for the leak, whose ratio falls 24 dB below the
/// anchor at 20 Hz), and its last N/16 samples hold less than −150 dB of
/// its energy before the window (observed −165 to −231 dB). Another ear
/// load is flagged as a different reference point.
#[test]
fn template_variants_meet_the_tolerance() {
    let text = example("design_over_ear.json");
    let mut base = design(&text, json!({}));
    for ov in [
        json!({"vent_count": 3}),
        json!({"front_depth_mm": 14}),
        json!({"leak_gap_mm": 0.3}),
        json!({"rear": "open"}),
        json!({"ear": "type43"}),
    ] {
        let mut cand = design(&text, ov.clone());
        let a = run(&mut cand, Baseline::Design(&mut base), json!({}));
        let c = a.check.as_ref().unwrap();
        assert!(c.met && c.max_abs_error_db < 0.02, "{ov}: {c:?}");
        assert_eq!(a.phase.used, "minimum", "{ov}");
        assert!(a.tail_energy_db < -150.0, "{ov}: {}", a.tail_energy_db);
        let reference = a.flags.iter().any(|f| f.code == "reference_point_mismatch");
        assert_eq!(reference, ov.get("ear").is_some(), "{ov}");
    }
}

/// Other rates: the filter keeps its duration (N = 8192 at 44.1 kHz,
/// 16384 at 96 kHz) and meets the tolerance (observed 0.002 and 0.003 dB);
/// at 44.1 kHz the band stops at 0.9 of Nyquist (19.845 kHz).
#[test]
fn other_rates() {
    assert_eq!(
        (
            n_default(44_100.0),
            n_default(48_000.0),
            n_default(96_000.0)
        ),
        (8192, 8192, 16384)
    );
    let text = example("design_over_ear.json");
    let mut cand = design(&text, json!({"vent_count": 3}));
    let mut base = design(&text, json!({}));
    for fs in [44_100.0, 96_000.0] {
        let a = run(&mut cand, Baseline::Design(&mut base), json!({"fs_Hz": fs}));
        assert_eq!(a.n, n_default(fs));
        assert_eq!(a.fs_hz, fs);
        assert_eq!(a.band_hz.1, if fs < 48_000.0 { 19_845.0 } else { 20_000.0 });
        let c = a.check.as_ref().unwrap();
        assert!(c.met, "{fs}: {c:?}");
    }
}

/// The inversion against the independent reference: working grid,
/// smoothed and notch-processed baseline, reference level, the
/// Kirkeby–Nelson inverse and the notches found, to 1e-6 dB (both sides in
/// double precision; the smoothing integrals agree to 1e-13 dB, see
/// docs/targets.md).
#[test]
fn inversion_matches_the_reference() {
    let fx = fixture();
    for (name, case) in fx["cases"].as_object().unwrap() {
        let target = json!({
            "name": format!("synthetic_{name}"),
            "fixture": "iec60318_4",
            "frequencies_Hz": case["frequencies_Hz"],
            "dB": case["dB"],
        });
        let m = target_baseline(&target).unwrap();
        let inv = acoustilab::audition::inversion::invert(&m.curve, &Default::default()).unwrap();
        let grid = f64s(&case["grid_Hz"]);
        assert_eq!(inv.freqs.len(), grid.len(), "{name}");
        for (a, b) in inv.freqs.iter().zip(&grid) {
            assert!((a / b - 1.0).abs() < 1e-12, "{name}");
        }
        assert!((inv.reference_db - case["reference_dB"].as_f64().unwrap()).abs() < 1e-6);
        for (key, got) in [
            ("smoothed_dB", &inv.smoothed_db),
            ("inverse_dB", &inv.inverse_db),
        ] {
            for (i, (a, b)) in got.iter().zip(f64s(&case[key])).enumerate() {
                assert!(
                    (a - b).abs() < 1e-6,
                    "{name} {key} at {} Hz: {a} vs {b}",
                    grid[i]
                );
            }
        }
        let want = case["notches"].as_array().unwrap();
        assert_eq!(inv.notches.len(), want.len(), "{name}");
        for (n, w) in inv.notches.iter().zip(want) {
            assert_eq!(n.inverted, w["inverted"].as_bool().unwrap());
            assert!((n.depth_db - w["depth_dB"].as_f64().unwrap()).abs() < 1e-6);
            assert!((n.at_hz / w["at_Hz"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        }
    }
}

/// Kirkeby–Nelson behaviour on the synthetic baselines:
/// * `cap`: a −24 dB dip at 5 kHz; the inverse never boosts beyond the
///   12 dB cap and reaches it (where the smoothed dip crosses √β).
/// * `notches`: the 30 dB notch at 3 kHz is deeper than 15 dB after
///   smoothing (21.4 dB): it is not inverted; the inverse follows the
///   envelope there, which the notch itself pulls down, a boost of 2.2 dB
///   where inverting the notch would have hit the 12 dB cap. The 8 dB notch
///   at 300 Hz (5.5 dB after smoothing) is inverted: a boost above 4 dB.
#[test]
fn kirkeby_cap_and_notch_rule() {
    let fx = fixture();
    let inv_of = |name: &str| {
        let case = &fx["cases"][name];
        let target = json!({"name": name, "fixture": "iec60318_4",
            "frequencies_Hz": case["frequencies_Hz"], "dB": case["dB"]});
        acoustilab::audition::inversion::invert(
            &target_baseline(&target).unwrap().curve,
            &Default::default(),
        )
        .unwrap()
    };
    let cap = inv_of("cap");
    let (boost, at) = cap.max_boost();
    assert!(boost <= 12.0 + 1e-12 && boost > 11.99, "{boost} at {at}");
    assert!((3000.0..8000.0).contains(&at), "{at}");
    let notches = inv_of("notches");
    let deep = notches
        .notches
        .iter()
        .find(|n| (2500.0..3500.0).contains(&n.at_hz))
        .unwrap();
    assert!(!deep.inverted && deep.depth_db > 15.0);
    let shallow = notches
        .notches
        .iter()
        .find(|n| (250.0..350.0).contains(&n.at_hz))
        .unwrap();
    assert!(shallow.inverted && shallow.depth_db > 3.0 && shallow.depth_db < 15.0);
    let at = |f: f64| notches.at(f);
    assert!(at(3000.0) < 3.0, "{}", at(3000.0));
    assert!(at(300.0) > 4.0, "{}", at(300.0));
}

/// A target baseline: the analytic filter of an RLC candidate (1 kHz,
/// Q = 2) against the synthetic targets, with the band limit (inverted
/// over 20 Hz–10 kHz, held with a continuous slope above) and the anchor,
/// equals the independent reference to 1e-6 dB; above 10 kHz the filter
/// no longer follows the candidate (it is held), and the taps meet the
/// tolerance.
#[test]
fn target_baseline_matches_the_reference() {
    let fx = fixture();
    let p = &fx["candidate_rlc"];
    let lp = Lp {
        r: p["R_ohm"].as_f64().unwrap(),
        l: p["L_H"].as_f64().unwrap(),
        c: p["C_F"].as_f64().unwrap(),
    };
    let mut cand = design(&lp.text(), json!({}));
    for name in ["smooth", "notches"] {
        let case = &fx["cases"][name];
        let target = json!({"name": name, "fixture": "iec60318_4",
            "frequencies_Hz": case["frequencies_Hz"], "dB": case["dB"]});
        let m: MagnitudeBaseline = target_baseline(&target).unwrap();
        let a = run(&mut cand, Baseline::Magnitude(&m), json!({}));
        let want = &fx["target_design"][name];
        let grid = f64s(&want["grid_Hz"]);
        assert_eq!(a.grid_hz.len(), grid.len());
        for (i, d) in f64s(&want["design_dB"]).iter().enumerate() {
            assert!((a.grid_hz[i] / grid[i] - 1.0).abs() < 1e-12);
            assert!(
                (a.design_db[i] - d).abs() < 1e-6,
                "{name} at {} Hz: {} vs {d}",
                grid[i],
                a.design_db[i]
            );
        }
        assert_eq!(a.band_hz, (20.0, 10_000.0));
        // Held above 10 kHz: within 6 dB of the 10 kHz level, and flat
        // from the end of the transition on (at most an octave: 20 kHz).
        let at10 = a.design_db[a.grid_hz.iter().position(|f| *f == 10_000.0).unwrap()];
        let above: Vec<(f64, f64)> = a
            .grid_hz
            .iter()
            .zip(&a.design_db)
            .filter(|(f, _)| **f >= 10_000.0)
            .map(|(f, d)| (*f, *d))
            .collect();
        assert!(above.iter().all(|(_, d)| (d - at10).abs() <= 6.0 + 1e-9));
        let last = above[above.len() - 1].1;
        let flat_from = above
            .iter()
            .position(|(_, d)| (d - last).abs() < 1e-9)
            .unwrap();
        assert!(above[flat_from].0 <= 20_000.0);
        assert!(a.check.as_ref().unwrap().met, "{:?}", a.check);
        assert!(a.flags.iter().all(|f| f.code != "fixture_mismatch"));
    }
}

/// Same inputs, same filter: bit-identical taps and report; the state
/// names the netlist by the SHA-256 of its text.
#[test]
fn deterministic_and_serialisable() {
    let text = example("design_over_ear.json");
    let mut cand = design(&text, json!({"vent_count": 3}));
    let mut base = design(&text, json!({}));
    let a = run(&mut cand, Baseline::Design(&mut base), json!({}));
    let mut cand2 = design(&text, json!({"vent_count": 3}));
    let mut base2 = design(&text, json!({}));
    let b = run(&mut cand2, Baseline::Design(&mut base2), json!({}));
    assert!(a
        .taps
        .iter()
        .zip(&b.taps)
        .all(|(x, y)| x.to_bits() == y.to_bits()));
    assert_eq!(to_json(&a, true), to_json(&b, true));
    let s = &a.state;
    assert_eq!(s["schema"], "acoustilab-audition/0.1");
    assert_eq!(
        s["candidate"]["netlist_sha256"],
        sha256_hex(text.as_bytes())
    );
    assert_eq!(s["candidate"]["overrides"], json!({"vent_count": 3}));
    assert_eq!(s["candidate"]["probe"], "p_drp");
    assert_eq!(s["baseline"]["kind"], "netlist");
    assert_eq!(s["mode"], "difference");
    assert_eq!(s["phase"], json!({"requested": "auto", "used": "minimum"}));
    assert_eq!(
        (s["fs_Hz"].as_f64(), s["n"].as_u64()),
        (Some(48_000.0), Some(8192))
    );
}

/// Absolute mode is flagged and needs no baseline; difference mode needs
/// one; a target on another fixture is flagged.
#[test]
fn modes_and_flags() {
    let text = example("design_over_ear.json");
    let mut cand = design(&text, json!({}));
    let a = run(&mut cand, Baseline::None, json!({"mode": "absolute"}));
    assert!(a.flags.iter().any(|f| f.code == "absolute_diagnostic"));
    assert!(audition(&mut cand, Baseline::None, &Options::default()).is_err());
    let m = target_baseline(&json!("ravizza2023_5128")).unwrap();
    let t = run(&mut cand, Baseline::Magnitude(&m), json!({}));
    assert!(t.flags.iter().any(|f| f.code == "fixture_mismatch"));
    assert_eq!(t.band_hz, (31.0, 10_000.0));
    assert!(t.check.as_ref().unwrap().met);
    assert_eq!(PhaseRequest::parse("mixed"), Some(PhaseRequest::Mixed));
    // An inversion band that leaves no audition band is an error, not a
    // panic (an index out of bounds before the review).
    for o in [
        json!({"fs_Hz": 16000, "inversion": {"band_Hz": [8000, 16000]}}),
        json!({"band_Hz": [20, 5000], "inversion": {"band_Hz": [6000, 16000]}}),
    ] {
        let e = audition(&mut cand, Baseline::Magnitude(&m), &opts(o.clone())).unwrap_err();
        assert!(e.to_string().contains("inversion band"), "{o}: {e}");
    }
}

/// "The same reference point": a baseline read at the canal entrance of
/// the same ear simulator is flagged, although the fixture is the same;
/// the same point is not.
#[test]
fn reference_point_is_the_fixture_and_its_node() {
    let text = example("design_over_ear.json");
    let mut doc: Value = serde_json::from_str(&text).unwrap();
    doc["probes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "p_eep", "quantity": "pressure", "node": "ear.eep"}));
    let eep = doc.to_string();
    let mut cand = design(&text, json!({"vent_count": 3}));
    let ov = Overrides::new();
    let mut at_eep = Design::new(&eep, &ov, json!({}), Some("p_eep")).unwrap();
    assert_eq!(at_eep.fixture, cand.fixture);
    assert_ne!(at_eep.reference_key, cand.reference_key);
    let a = run(
        &mut cand,
        Baseline::Design(&mut at_eep),
        json!({"verify": false}),
    );
    let f = a
        .flags
        .iter()
        .find(|f| f.code == "reference_point_mismatch")
        .expect("DRP against EEP is flagged");
    assert!(f.message.contains("p_eep"), "{}", f.message);
    let mut at_drp = Design::new(&eep, &ov, json!({}), None).unwrap();
    assert_eq!(at_drp.reference_key, cand.reference_key);
    let b = run(
        &mut cand,
        Baseline::Design(&mut at_drp),
        json!({"verify": false}),
    );
    assert!(b.flags.iter().all(|f| f.code != "reference_point_mismatch"));
}

/// Options are checked: unknown keys, bad values and ranges are refused
/// with a message naming them.
#[test]
fn options_are_checked() {
    for (o, msg) in [
        (json!({"phase": "maximum"}), "phase"),
        (json!({"n": 1000}), "power of two"),
        (json!({"n": 2048}), "4096"),
        (json!({"fs_Hz": 4000}), "fs"),
        (json!({"band_Hz": [20, 23000]}), "Nyquist"),
        (json!({"mode": "loud"}), "mode"),
        (json!({"colour": 1}), "unknown key 'colour'"),
        (json!({"inversion": {"boost_cap_dB": 60}}), "boost cap"),
        (json!({"inversion": {"smoothing": 5}}), "1/N octave"),
        // The anchor band must lie inside the audition band (these panicked).
        (json!({"band_Hz": [3000, 10000]}), "anchor band"),
        (json!({"band_Hz": [20, 400]}), "anchor band"),
    ] {
        let e = parse_options(&o).unwrap_err();
        assert!(e.message.contains(msg), "{o}: {}", e.message);
        assert_eq!(e.kind, "options");
    }
}
