//! `acoustilab score`: the drum response of a netlist against a target.
//! The numbers must be the engine's (compared with `targets::evaluate`
//! called directly); the probe and fixture are chosen as documented.

use acoustilab::targets::curve::Curve;
use acoustilab::targets::metrics::{evaluate, Options, Response};
use acoustilab::targets::target;
use acoustilab::Circuit;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_acoustilab"))
        .args(args)
        .output()
        .expect("run acoustilab")
}

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

#[test]
fn score_json_equals_the_engine() {
    let design = example("design_over_ear.json");
    let o = cli(&[
        "score",
        design.to_str().unwrap(),
        "--target",
        "ravizza2023_5128",
        "--json",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let got: Value = serde_json::from_str(&stdout(&o)).unwrap();
    // The template's ui.primary_probe is p_drp, on the IEC 60318-4 macro.
    assert_eq!(got["response"]["label"], "p_drp");
    assert_eq!(got["response"]["fixture"], "iec60318_4");

    let c = Circuit::from_json(&std::fs::read_to_string(&design).unwrap()).unwrap();
    let r = c.solve().unwrap();
    let resp = Response {
        left: Curve::new(r.freqs_hz.clone(), r.probe("p_drp").unwrap().spl_db()).unwrap(),
        right: None,
        fixture: Some("iec60318_4".into()),
        label: "p_drp".into(),
        drive: Some(r.meta.drive.label.clone()),
    };
    let t = target::find("ravizza2023_5128").unwrap();
    let direct = evaluate(&resp, t, &Options::default()).unwrap().to_json();
    for path in [
        "/metrics/main/rms_dB",
        "/metrics/main/sd_dB",
        "/metrics/main/abs_slope",
        "/scores/0/score",
        "/scores/2/score",
    ] {
        let (a, b) = (
            got.pointer(path).unwrap().as_f64().unwrap(),
            direct.pointer(path).unwrap().as_f64().unwrap(),
        );
        assert!(
            (a - b).abs() <= 1e-12 * a.abs().max(1.0),
            "{path}: {a} vs {b}"
        );
    }
}

#[test]
fn score_text_overrides_probe_and_errors() {
    let design = example("design_over_ear.json");
    let d = design.to_str().unwrap();
    let o = cli(&[
        "score",
        d,
        "--target",
        "ravizza2023_5128",
        "--set",
        "ear=type43",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    for want in [
        "response: p_drp on p57_type4_3",
        "20 Hz-10 kHz (used 31.6 Hz-10 kHz, partial)",
        "ITU-R BS.708 mask, 100 Hz-16 kHz",
        "harman_oe_2018",
        "fixture_mismatch",
    ] {
        assert!(s.contains(want), "'{want}' missing:\n{s}");
    }
    // Another probe, with smoothing; a front-cavity probe has no fixture.
    let o = cli(&[
        "score",
        d,
        "--target",
        "ravizza2023_5128",
        "--probe",
        "p_front",
        "--smoothing",
        "1/3",
    ]);
    assert!(
        stdout(&o).contains("p_front on an unspecified fixture"),
        "{}",
        stdout(&o)
    );
    // Errors name the problem.
    for (args, want) in [
        (vec!["--target", "nope"], "no bundled target 'nope'"),
        (
            vec!["--probe", "zin", "--target", "ravizza2023_5128"],
            "not a pressure probe",
        ),
        (vec![], "--target"),
        (
            vec!["--target", "ravizza2023_5128", "--smoothing", "5"],
            "1/5 octave is not offered",
        ),
        (
            vec!["--target", "ravizza2023_5128", "--bogus"],
            "unknown option '--bogus'",
        ),
    ] {
        let mut a = vec!["score", d];
        a.extend(args);
        let o = cli(&a);
        assert!(!o.status.success());
        assert!(stderr(&o).contains(want), "{a:?}: {}", stderr(&o));
    }
}

#[test]
fn score_with_a_csv_target_and_the_list() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let csv = dir.join(format!("score-{}-flat.csv", std::process::id()));
    std::fs::write(
        &csv,
        "# fixture: iec60318_4\nfrequency_Hz,dB\n10,0\n40000,0\n",
    )
    .unwrap();
    let o = cli(&[
        "score",
        example("design_over_ear.json").to_str().unwrap(),
        "--target",
        csv.to_str().unwrap(),
        "--json",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["fixture_match"], "same");
    assert_eq!(v["target"]["provenance_class"], "user");
    assert_eq!(v["metrics"]["main"]["partial"], false);
    let missing = dir.join(format!("score-{}-untagged.csv", std::process::id()));
    std::fs::write(&missing, "10,0\n40000,0\n").unwrap();
    let o = cli(&[
        "score",
        example("design_over_ear.json").to_str().unwrap(),
        "--target",
        missing.to_str().unwrap(),
    ]);
    assert!(stderr(&o).contains("no fixture tag"), "{}", stderr(&o));
    let o = cli(&["score", "--list"]);
    let s = stdout(&o);
    assert!(s
        .lines()
        .next()
        .unwrap()
        .starts_with("ravizza2023_5128\tbk5128\tCC-BY-4.0"));
    assert_eq!(s.lines().count(), 33);
}
