//! Curve import and export (FRD, ZMA, REW text, CSV), sidecars and their
//! compatibility check, resampling (docs/fitting.md, spec Section 14).
//!
//! Round trips are exact for CSV (shortest round-trip numbers) and within
//! the six decimals FRD, ZMA and REW text carry (5e-7 absolute, which is
//! 5e-7 dB, 5e-7 ohm, 5e-5 degrees at four decimals for the phase).

use acoustilab::io::curve::{exchange_grid, unwrap_deg, wrap_deg};
use acoustilab::io::sidecar::{self, Averaging, Profile, Smoothing, Uncertainty};
use acoustilab::io::text::{export, import};
use acoustilab::io::{Curve, Format, Quantity, Sidecar};
use serde_json::json;

fn pressure_curve() -> Curve {
    let f = exchange_grid(20.0, 20_000.0, 12.0);
    let level: Vec<f64> = f
        .iter()
        .map(|f| 94.0 + 6.0 * (f / 1000.0).log10().sin())
        .collect();
    let phase: Vec<f64> = f.iter().map(|f| wrap_deg(-0.03 * f)).collect();
    Curve::from_db(Quantity::Pressure, &f, &level, Some(&phase)).unwrap()
}

fn impedance_curve() -> Curve {
    let f = exchange_grid(10.0, 20_000.0, 24.0);
    let z: Vec<acoustilab::C64> = f
        .iter()
        .map(|&f| {
            let w = f / 81.8;
            let jw = acoustilab::C64::new(0.0, w);
            32.8 + (32.8 / 1.01) / (jw + 1.0 / 2.71 + (jw).inv())
        })
        .collect();
    Curve::from_complex(Quantity::Impedance, &f, &z).unwrap()
}

fn close(a: &[f64], b: &[f64], tol: f64, what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: lengths");
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        assert!((x - y).abs() <= tol, "{what}[{i}]: {x} vs {y}");
    }
}

fn phase_close(a: &[f64], b: &[f64], tol: f64) {
    for (x, y) in a.iter().zip(b) {
        assert!(wrap_deg(x - y).abs() <= tol, "{x} vs {y}");
    }
}

// ----- Round trips ------------------------------------------------------------

#[test]
fn csv_round_trip_is_exact() {
    for c in [pressure_curve(), impedance_curve()] {
        let text = export(&c, Format::Csv).unwrap();
        let back = import(&text, Format::Csv, None).unwrap();
        assert_eq!(back.quantity, c.quantity);
        assert_eq!(back.freqs_hz, c.freqs_hz);
        for (x, y) in back.magnitude.iter().zip(&c.magnitude) {
            assert!((x / y - 1.0).abs() < 1e-14, "{x} vs {y}");
        }
        assert_eq!(back.phase_deg, c.phase_deg);
    }
}

#[test]
fn frd_zma_and_rew_round_trips() {
    let p = pressure_curve();
    let z = impedance_curve();
    for (c, fmt) in [
        (&p, Format::Frd),
        (&p, Format::Rew),
        (&z, Format::Zma),
        (&z, Format::Rew),
    ] {
        let text = export(c, fmt).unwrap();
        let back = import(&text, fmt, None).unwrap();
        assert_eq!(back.quantity, c.quantity, "{fmt:?}");
        close(&back.freqs_hz, &c.freqs_hz, 5e-7, "f");
        if c.quantity == Quantity::Pressure {
            close(&back.level_db(), &c.level_db(), 5.1e-7, "dB");
        } else {
            close(&back.magnitude, &c.magnitude, 5.1e-7, "ohm");
        }
        phase_close(
            back.phase_deg.as_ref().unwrap(),
            c.phase_deg.as_ref().unwrap(),
            5.1e-5,
        );
        // The same file read without a format hint.
        let auto = import(&text, Format::Auto, None);
        match fmt {
            // FRD has no units in it: without the hint it is a bare
            // two-column file whose unit cannot be known.
            Format::Frd | Format::Zma => {
                assert!(auto.is_err() || auto.unwrap().quantity == c.quantity)
            }
            _ => assert_eq!(auto.unwrap().quantity, c.quantity),
        }
    }
}

#[test]
fn magnitude_only_curves_round_trip() {
    let mut p = pressure_curve();
    p.phase_deg = None;
    for fmt in [Format::Frd, Format::Rew, Format::Csv] {
        let back = import(&export(&p, fmt).unwrap(), fmt, None).unwrap();
        assert!(back.phase_deg.is_none(), "{fmt:?}");
        close(&back.level_db(), &p.level_db(), 5.1e-7, "dB");
    }
    // REW writes 0.0 for a missing phase, and reads it back as none.
    let rew = export(&p, Format::Rew).unwrap();
    assert!(rew.lines().nth(12).unwrap().ends_with("\t0.0000"), "{rew}");
}

#[test]
fn exports_refuse_the_wrong_quantity() {
    assert!(export(&impedance_curve(), Format::Frd).is_err());
    assert!(export(&pressure_curve(), Format::Zma).is_err());
    assert!(export(&pressure_curve(), Format::Auto).is_err());
}

#[test]
fn curve_json_round_trip_and_unknown_keys() {
    let mut c = impedance_curve();
    c.sidecar.fixture = Some("free air".into());
    let v = c.to_json();
    assert!(v.get("magnitude_ohm").is_some());
    let back = Curve::from_json(&v).unwrap();
    assert_eq!(back.freqs_hz, c.freqs_hz);
    assert_eq!(back.magnitude, c.magnitude);
    assert_eq!(back.sidecar.fixture.as_deref(), Some("free air"));
    let p = pressure_curve();
    let back = Curve::from_json(&p.to_json()).unwrap();
    for (x, y) in back.magnitude.iter().zip(&p.magnitude) {
        assert!((x / y - 1.0).abs() < 1e-14);
    }
    let mut bad = v.clone();
    bad["magnitude_Pa"] = json!([1.0]);
    assert!(Curve::from_json(&bad)
        .unwrap_err()
        .msg
        .contains("unknown key 'magnitude_Pa'"));
    let re_im = json!({"quantity": "impedance", "frequencies_Hz": [10, 20],
                       "re": [3.0, 4.0], "im": [4.0, 3.0]});
    let c = Curve::from_json(&re_im).unwrap();
    assert!((c.magnitude[0] - 5.0).abs() < 1e-15);
    let mismatch = json!({"quantity": "pressure", "frequencies_Hz": [10, 20], "level_dB": [90, 91],
                          "sidecar": {"schema": "acoustilab-curve-sidecar/0.1", "quantity": "impedance"}});
    assert!(Curve::from_json(&mismatch).is_err());
}

// ----- Reading real-world layouts ---------------------------------------------

#[test]
fn reads_a_rew_frequency_response_export() {
    // Layout of REW's "Export measurement as text" (header lines as REW
    // V5 writes them; the data are made up).
    let text = "* Measurement data measured by REW V5.31\r\n\
* Source: USB interface, MICROPHONE, L, volume: 0.390\r\n\
* Format: 256k Log Swept Sine, 1 sweep at -10.0 dBFS with no timing reference\r\n\
* Dated: Oct 9, 2024 12:46:14 AM\r\n\
* REW Settings:\r\n\
*  C-weighting compensation: Off\r\n\
*  Target level: 75.0 dB\r\n\
* Note: left channel\r\n\
* Measurement: prototype L\r\n\
* Smoothing: 1/12 octave\r\n\
* Frequency Step: 96 ppo\r\n\
* Start Frequency: 20.0 Hz\r\n\
*\r\n\
* Freq(Hz)\tSPL(dB)\tPhase(degrees)\r\n\
20.000000\t101.464462\t-137.7926\r\n\
20.600000\t101.484512\t-137.9498\r\n\
21.200000\t101.503000\t-138.1000\r\n";
    let c = import(text, Format::Auto, None).unwrap();
    assert_eq!(c.quantity, Quantity::Pressure);
    assert_eq!(c.len(), 3);
    assert!((c.level_db()[1] - 101.484512).abs() < 1e-12);
    assert_eq!(c.phase_deg.as_ref().unwrap()[2], -138.1);
    let sc = &c.sidecar;
    assert_eq!(sc.smoothing, Some(Smoothing::Octave(12)));
    assert_eq!(sc.date.as_deref(), Some("Oct 9, 2024 12:46:14 AM"));
    assert_eq!(
        sc.provenance.as_ref().unwrap().tool.as_deref(),
        Some("REW V5.31")
    );
    assert!(sc
        .notes
        .as_ref()
        .unwrap()
        .contains("Measurement: prototype L"));
    assert!(c.comments.iter().any(|l| l.starts_with("Source:")));
}

#[test]
fn reads_a_rew_impedance_export_and_its_older_layout() {
    let text = "* Measurement data saved by REW V5.00\n\
* Sense Resistor: 100.0\n\
* Format:   1M Log Swept Sine, 1 sweep at -30,0 dBFS\n\
*\n\
* Freq(Hz) Z(Ohms) Phase(degrees)\n\
1.831 6.423 5.392\n\
2.197 6.444 6.426\n\
2.563 6.481 7.302\n";
    let c = import(text, Format::Auto, None).unwrap();
    assert_eq!(c.quantity, Quantity::Impedance);
    assert_eq!(c.magnitude, vec![6.423, 6.444, 6.481]);
    // Old comma-delimited layout without phase, with a comment after a value.
    let text = "SPL measurements acquired by REW V3.08\n\
Format: Comma delimited data\n\
Channel: Left, Bass limited 80Hz\n\
\n\
20.0, 65.01\n\
21.0, 65.77\n\
27.0, 68.31, this line has a comment\n";
    let c = import(text, Format::Frd, None).unwrap();
    assert_eq!(c.quantity, Quantity::Pressure);
    assert_eq!(c.len(), 3);
    assert!(c.phase_deg.is_none());
    assert!((c.level_db()[2] - 68.31).abs() < 1e-12);
}

#[test]
fn generic_text_with_headers_units_and_decimal_commas() {
    // Semicolon-separated, decimal commas, kHz and radians in the header.
    let text = "\u{feff}# exported by some analyser\nFrequency [kHz];Level (dB);Phase [rad]\n0,1;80,5;0,5\n1;90;-1,25\n";
    let c = import(text, Format::Csv, None).unwrap();
    assert_eq!(c.freqs_hz, vec![100.0, 1000.0]);
    assert!((c.level_db()[0] - 80.5).abs() < 1e-12);
    assert!((c.phase_deg.as_ref().unwrap()[1] + 1.25f64.to_degrees()).abs() < 1e-12);
    // White space with decimal commas, no header, impedance by hint.
    let c = import(
        "10,5 32,8 1,5\n20,25 33,1 3,0\n",
        Format::Csv,
        Some(Quantity::Impedance),
    )
    .unwrap();
    assert_eq!(c.freqs_hz, vec![10.5, 20.25]);
    assert_eq!(c.magnitude, vec![32.8, 33.1]);
    // Tab-separated real and imaginary parts.
    let c = import(
        "f_Hz\tRe (ohm)\tIm (ohm)\n100\t3\t4\n200\t6\t8\n",
        Format::Csv,
        None,
    )
    .unwrap();
    assert_eq!(c.quantity, Quantity::Impedance);
    assert!((c.magnitude[1] - 10.0).abs() < 1e-12);
    assert!((c.phase_deg.as_ref().unwrap()[0] - 4f64.atan2(3.0).to_degrees()).abs() < 1e-12);
    // Quoted fields.
    let c = import(
        "\"Frequency (Hz)\",\"SPL (dB)\"\n\"20\",\"80.5\"\n\"40\",\"81\"\n",
        Format::Csv,
        None,
    )
    .unwrap();
    assert_eq!(c.freqs_hz, vec![20.0, 40.0]);
    assert!((c.level_db()[1] - 81.0).abs() < 1e-12);
    // AutoEq-style CSV: the first magnitude column, "raw", is in dB.
    let c = import(
        "frequency,raw,error,smoothed\n20,1.5,0.2,1.4\n21,1.6,0.1,1.5\n",
        Format::Csv,
        None,
    )
    .unwrap();
    assert_eq!(c.quantity, Quantity::Pressure);
    assert!((c.level_db()[1] - 1.6).abs() < 1e-12);
    // A displacement column in micrometres: the unit identifies the
    // magnitude even under an unknown name; a column without a unit and
    // without a known name does not.
    let c = import("freq (Hz), x (um)\n10, 850\n20, 800\n", Format::Csv, None).unwrap();
    assert_eq!(c.quantity, Quantity::Displacement);
    let e = import("freq (Hz), x\n10, 850\n20, 800\n", Format::Csv, None).unwrap_err();
    assert!(e.msg.contains("cannot tell the magnitude unit"), "{e}");
    let c = import(
        "frequency_Hz,displacement (um)\n10,850\n20,800\n",
        Format::Csv,
        None,
    )
    .unwrap();
    assert_eq!(c.quantity, Quantity::Displacement);
    assert!((c.magnitude[0] - 850e-6).abs() < 1e-18);
}

#[test]
fn sorts_and_merges_duplicate_frequencies() {
    let c = import(
        "* unsorted\n300 3 0\n100 1 0\n200 2 0\n100 1 0\n",
        Format::Zma,
        None,
    )
    .unwrap();
    assert_eq!(c.freqs_hz, vec![100.0, 200.0, 300.0]);
    assert_eq!(c.magnitude, vec![1.0, 2.0, 3.0]);
    let e = import("100 1 0\n200 2 0\n100 1.5 0\n", Format::Zma, None).unwrap_err();
    assert_eq!(e.line, Some(3));
    assert!(e.msg.contains("lines 1 and 3"), "{e}");
}

#[test]
fn malformed_inputs_name_the_line() {
    let cases: [(&str, Option<usize>, &str); 9] = [
        ("", None, "no data"),
        ("* only comments\n# here\n", None, "no data"),
        ("100 1 2\n200 2\n", Some(2), "numbers, but line 1 has"),
        ("100 1\n200 abc\n", Some(2), "expected a frequency"),
        ("100\t1,5\n200\t2.5\n", Some(2), "mixes decimal commas"),
        ("100;1.234,5\n", Some(1), "ambiguous"),
        ("100 1\n200 -2\n", Some(2), "not positive"),
        ("100 1\n-200 2\n", Some(2), "not positive"),
        ("100 1 2 3\n200 1 2 3\n", Some(1), "no header"),
    ];
    for (text, line, msg) in cases {
        let e = import(text, Format::Zma, None).unwrap_err();
        assert_eq!(e.line, line, "{text:?}: {e}");
        assert!(e.msg.contains(msg), "{text:?}: {e}");
    }
    // One point is not a curve.
    assert!(import("100 1\n", Format::Zma, None)
        .unwrap_err()
        .msg
        .contains("at least 2"));
    // A bare two-column file of unknown unit needs a hint.
    let e = import("100 1\n200 2\n", Format::Csv, None).unwrap_err();
    assert!(e.msg.contains("cannot tell the magnitude unit"), "{e}");
    // dB into an impedance is a contradiction.
    assert!(import(
        "f (Hz), SPL (dB)\n100, 90\n200, 91\n",
        Format::Csv,
        Some(Quantity::Impedance)
    )
    .is_err());
    // Non-finite values.
    assert!(import("100 inf\n200 2\n", Format::Zma, None).is_err());
}

// ----- Sidecars -----------------------------------------------------------------

fn full_sidecar() -> serde_json::Value {
    json!({
        "schema": "acoustilab-curve-sidecar/0.1",
        "quantity": "pressure", "calibrated": true,
        "fixture": "GRAS 45CA", "ear_simulator": "iec60318_4", "pinna": "KB5000",
        "seatings": 5, "averaging": "complex", "smoothing": "1/12",
        "drive": {"power_mW": 1, "rated_ohm": 32}, "source_impedance_ohm": 0,
        "compensation": "none", "reference_point": "drp",
        "temperature_C": 23, "date": "2026-09-25", "device": "prototype A, left",
        "provenance": {"origin": "measured", "source": "bench session 12", "licence": "internal", "tool": "REW V5.31"},
        "uncertainty": {"coupler_dB": {"frequencies_Hz": [100, 1000, 10000], "values_dB": [0.3, 0.3, 1.0]},
                        "microphone_calibration_dB": 0.2, "repositioning_dB": 0.8,
                        "noise_dB": 0.05, "phase_deg": 1.0},
        "notes": "left cup"
    })
}

#[test]
fn sidecar_round_trip_and_rejections() {
    let sc = Sidecar::from_json(&full_sidecar()).unwrap();
    assert_eq!(sc.seatings, Some(5));
    assert_eq!(sc.averaging, Some(Averaging::Complex));
    assert_eq!(Sidecar::from_json(&sc.to_json()).unwrap(), sc);
    assert_eq!(Sidecar::parse(&sc.to_text()).unwrap(), sc);
    let bad = |edit: &dyn Fn(&mut serde_json::Value), msg: &str| {
        let mut v = full_sidecar();
        edit(&mut v);
        let e = Sidecar::from_json(&v).unwrap_err();
        assert!(e.msg.contains(msg), "{msg}: {e}");
    };
    bad(&|v| v["fixtrue"] = json!("x"), "unknown key 'fixtrue'");
    bad(
        &|v| v["schema"] = json!("acoustilab-curve-sidecar/9"),
        "schema",
    );
    bad(
        &|v| {
            v.as_object_mut().unwrap().remove("schema");
        },
        "needs \"schema\"",
    );
    bad(
        &|v| v["provenance"]["origni"] = json!("x"),
        "provenance: unknown key 'origni'",
    );
    bad(
        &|v| v["uncertainty"]["coupler"] = json!(0.3),
        "unknown key 'coupler'",
    );
    bad(
        &|v| v["uncertainty"]["noise_dB"] = json!(-1),
        "non-negative",
    );
    bad(&|v| v["drive"] = json!({"power_mW": 1}), "rated_ohm");
    bad(&|v| v["averaging"] = json!("mean"), "averaging");
    bad(&|v| v["smoothing"] = json!("third"), "smoothing");
    bad(&|v| v["seatings"] = json!(0), "seatings");
    bad(&|v| v["temperature_C"] = json!(-300), "absolute zero");
}

#[test]
fn uncertainty_budget_combines_terms() {
    let sc = Sidecar::from_json(&full_sidecar()).unwrap();
    let u = sc.uncertainty.unwrap();
    // At 1 kHz: coupler 0.3, microphone 0.2, (repositioning 0.8, noise
    // 0.05) / √5.
    let want = (0.3f64.powi(2) + 0.2f64.powi(2) + (0.8f64.powi(2) + 0.05f64.powi(2)) / 5.0).sqrt();
    assert!((u.level_db(1000.0, 5).unwrap() - want).abs() < 1e-15);
    // The coupler table is linear in ln f: halfway (in ln f) between 1 and
    // 10 kHz it is 0.65 dB; beyond its ends it is held.
    let mid = (1000.0f64 * 10_000.0).sqrt();
    let want = (0.65f64.powi(2) + 0.2f64.powi(2) + (0.8f64.powi(2) + 0.05f64.powi(2)) / 5.0).sqrt();
    assert!((u.level_db(mid, 5).unwrap() - want).abs() < 1e-12);
    assert_eq!(u.coupler_db.as_ref().unwrap().at(20.0), 0.3);
    assert_eq!(u.coupler_db.as_ref().unwrap().at(40_000.0), 1.0);
    assert!((u.phase_deg(1000.0, 4).unwrap() - 0.5).abs() < 1e-15);
    assert!(Uncertainty::default().level_db(1000.0, 1).is_none());
    assert_eq!(Profile::Constant(0.4).at(123.0), 0.4);
}

#[test]
fn comparison_refuses_different_conditions_and_names_them() {
    let a = Sidecar::from_json(&full_sidecar()).unwrap();
    assert!(sidecar::compare(&a, &a).check(&[]).is_ok());
    let mut b = a.clone();
    b.fixture = Some("gras  45ca".into()); // case and spacing are not differences
    assert!(sidecar::compare(&a, &b).check(&[]).is_ok());
    b.fixture = Some("flat plate".into());
    b.compensation = Some("diffuse_field".into());
    b.drive = Some(sidecar::Drive::from_spec(
        &acoustilab::drive::DriveSpec::Voltage(1.0),
    ));
    let c = sidecar::compare(&a, &b);
    let fields: Vec<&str> = c.blocking.iter().map(|d| d.field).collect();
    assert_eq!(fields, vec!["fixture", "compensation", "drive"]);
    let e = c.check(&[]).unwrap_err();
    assert!(
        e.msg.contains("fixture differs (GRAS 45CA vs flat plate)"),
        "{e}"
    );
    assert!(
        e.msg
            .contains("drive differs (1 mW into 32 ohm vs 1 V RMS)"),
        "{e}"
    );
    assert!(c
        .check(&["fixture", "compensation"])
        .unwrap_err()
        .msg
        .contains("drive"));
    assert!(c.check(&["fixture", "compensation", "drive"]).is_ok());
    assert!(c.check(&["all"]).is_ok());
    // A field stated on one side only cannot be confirmed.
    let mut d = a.clone();
    d.reference_point = None;
    let c = sidecar::compare(&a, &d);
    assert_eq!(c.blocking[0].field, "reference_point");
    assert_eq!(c.blocking[0].b, "unstated");
    // Stated on neither side: a note, not a refusal.
    let (mut x, mut y) = (a.clone(), a.clone());
    x.pinna = None;
    y.pinna = None;
    let c = sidecar::compare(&x, &y);
    assert!(c.blocking.is_empty());
    assert!(c.notes.iter().any(|n| n.field == "pinna"));
    // Averaging differs: a note only.
    y.pinna = None;
    y.averaging = Some(Averaging::Magnitude);
    let c = sidecar::compare(&x, &y);
    assert!(c.blocking.is_empty());
    assert!(c.notes.iter().any(|n| n.field == "averaging"));
    // The same drive written in other units is the same drive.
    let mut z = a.clone();
    z.drive = Some(sidecar::Drive {
        json: json!({"power_W": 0.001, "rated_ohm": 32}),
        spec: acoustilab::drive::DriveSpec::Power {
            watts: 0.001,
            rated_ohm: 32.0,
        },
    });
    assert!(sidecar::compare(&a, &z).check(&[]).is_ok());
}

#[test]
fn smoothing_names() {
    assert_eq!(Smoothing::parse("1/12 octave"), Some(Smoothing::Octave(12)));
    assert_eq!(Smoothing::parse("None"), Some(Smoothing::None));
    assert_eq!(Smoothing::parse("No smoothing"), Some(Smoothing::None));
    assert_eq!(Smoothing::parse("1/0"), None);
    assert_eq!(Smoothing::Octave(3).name(), "1/3");
}

// ----- Resampling -----------------------------------------------------------------

#[test]
fn resampling_is_exact_for_levels_and_phases_linear_in_ln_f() {
    // L = 80 + 6·log2(f/100) dB, φ = −170·ln(f/20) degrees (unwrapped;
    // stored wrapped): both linear in ln f, so interpolation is exact.
    let f = exchange_grid(20.0, 20_000.0, 6.0);
    let lv = |f: f64| 80.0 + 6.0 * (f / 100.0).log2();
    let ph = |f: f64| -170.0 * (f / 20.0).ln();
    let level: Vec<f64> = f.iter().map(|&f| lv(f)).collect();
    let phase: Vec<f64> = f.iter().map(|&f| wrap_deg(ph(f))).collect();
    let c = Curve::from_db(Quantity::Pressure, &f, &level, Some(&phase)).unwrap();
    // The coarse grid's steps of 170·ln 2^(1/6) = 19.6° never exceed 180°.
    let r = c.resample_exchange(48.0).unwrap();
    assert!(r.len() > 6 * c.len());
    for (i, &fr) in r.freqs_hz.iter().enumerate() {
        assert!((r.level_db()[i] - lv(fr)).abs() < 1e-9, "{fr}");
        assert!(
            wrap_deg(r.phase_deg.as_ref().unwrap()[i] - ph(fr)).abs() < 1e-9,
            "{fr}"
        );
    }
    // Exact at the original points.
    let back = c.resample(&c.freqs_hz).unwrap();
    close(&back.level_db(), &c.level_db(), 1e-9, "dB");
    // No extrapolation.
    assert!(c.resample(&[10.0]).is_err());
    assert!(c.resample(&[25_000.0]).is_err());
    // Unwrapping.
    let u = unwrap_deg(&phase);
    for (x, &fr) in u.iter().zip(&f) {
        assert!((x - ph(fr)).abs() < 1e-9);
    }
}
