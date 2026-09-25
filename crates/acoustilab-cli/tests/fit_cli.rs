//! `acoustilab measure`, `fit` and `convert`: the virtual rig writes files
//! a real rig would, the fit reads them back with their sidecars, and
//! convert moves curves between formats. Numbers are checked in the engine
//! tests (crates/acoustilab/tests/fit.rs); these check the plumbing.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_acoustilab"))
        .args(args)
        .output()
        .expect("run acoustilab")
}

fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fitcli-{}-{name}", std::process::id()))
}

fn bench() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/driver_bench.json")
        .to_str()
        .unwrap()
        .to_string()
}

fn ok(o: &Output) -> String {
    assert!(
        o.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn measure_then_fit_with_an_added_mass() {
    let free = scratch("free.zma");
    let mass = scratch("mass.zma");
    let truth = ["--set", "Bl_Tm=2.5", "--set", "Mms_g=0.33"];
    for (file, seed, extra) in [
        (&free, "3", vec![]),
        (&mass, "4", vec!["--set", "added_mass_mg=150"]),
    ] {
        let mut args = vec![
            "measure",
            &bench(),
            "--probe",
            "zin",
            "--out",
            file.to_str().unwrap(),
            "--noise-db",
            "0.05",
            "--noise-deg",
            "0.3",
            "--seed",
            seed,
            "--ppo",
            "12",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
        args.extend(truth.iter().chain(&extra).map(|s| s.to_string()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = ok(&cli(&refs));
        assert!(out.contains("impedance"), "{out}");
        let sc: Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{}.sidecar.json", file.display())).unwrap(),
        )
        .unwrap();
        assert_eq!(sc["provenance"]["origin"], "virtual_rig");
    }
    let report = scratch("report.json");
    let curve_free = format!("zin={}", free.display());
    let curve_mass = format!("zin={}", mass.display());
    let out = ok(&cli(&[
        "fit",
        &bench(),
        "--curve",
        &curve_free,
        "--curve",
        &curve_mass,
        "--condition",
        "added_mass_mg=150",
        "--param",
        "Re_ohm",
        "--param",
        "Bl_Tm=2.0",
        "--param",
        "Mms_g",
        "--param",
        "Cms_mm_per_N",
        "--param",
        "Rms_Ns_per_m",
        "--out",
        report.to_str().unwrap(),
    ]));
    assert!(out.contains("Bl_Tm"), "{out}");
    assert!(out.contains("added-mass method is unreliable"), "{out}");
    let r: Value = serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(r["converged"], true);
    let bl = r["fitted"]["Bl_Tm"].as_f64().unwrap();
    assert!((bl / 2.5 - 1.0).abs() < 0.02, "{bl}");
    let start = r["parameters"][1]["start"].as_f64().unwrap();
    assert_eq!(start, 2.0);
    // Without the added-mass curve the scale is ambiguous.
    let out = ok(&cli(&[
        "fit",
        &bench(),
        "--curve",
        &curve_free,
        "--param",
        "Bl_Tm",
        "--param",
        "Mms_g",
        "--param",
        "Cms_mm_per_N",
        "--param",
        "Rms_Ns_per_m",
        "--json",
    ]));
    let r: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(r["parameters"][0]["status"], "scale_ambiguous");
}

#[test]
fn convert_between_formats_keeps_the_sidecar() {
    let zma = scratch("c.zma");
    ok(&cli(&[
        "measure",
        &bench(),
        "--probe",
        "zin",
        "--out",
        zma.to_str().unwrap(),
        "--ppo",
        "12",
        "--band",
        "20:20000",
    ]));
    let csv = scratch("c.csv");
    let out = ok(&cli(&[
        "convert",
        zma.to_str().unwrap(),
        csv.to_str().unwrap(),
        "--ppo",
        "6",
    ]));
    assert!(out.contains("points"), "{out}");
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(
        text.starts_with("frequency_Hz,magnitude_ohm,phase_deg\n"),
        "{text}"
    );
    let sc: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}.sidecar.json", csv.display())).unwrap(),
    )
    .unwrap();
    assert_eq!(sc["quantity"], "impedance");
    assert_eq!(sc["provenance"]["origin"], "virtual_rig");
    // A pressure-only format for an impedance is refused, naming the file.
    let frd = scratch("c.frd");
    let o = cli(&["convert", zma.to_str().unwrap(), frd.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("ZMA"));
}

#[test]
fn export_writes_a_simulated_probe() {
    let out = scratch("p.frd");
    let o = ok(&cli(&[
        "export",
        &bench(),
        "--probe",
        "p_box",
        "--out",
        out.to_str().unwrap(),
        "--set",
        "box_volume_cm3=20",
        "--ppo",
        "6",
        "--band",
        "20:2000",
    ]));
    assert!(o.contains("pressure"), "{o}");
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.starts_with("* FRD"), "{text}");
    let sc: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}.sidecar.json", out.display())).unwrap(),
    )
    .unwrap();
    assert_eq!(sc["provenance"]["origin"], "simulated");
    assert_eq!(sc["drive"]["voltage_V"], 1.0);
    let o = cli(&[
        "export",
        &bench(),
        "--probe",
        "nope",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("no probe 'nope'"));
}

#[test]
fn errors_name_what_is_wrong() {
    for (args, msg) in [
        (vec!["measure", "x.json"], "x.json"),
        (vec!["fit", "missing.json"], "missing.json"),
        (vec!["convert", "a.zma"], "acoustilab convert"),
        (vec!["convert", "a.xyz", "b.zma"], "a.xyz"),
    ] {
        let o = cli(&args);
        assert!(!o.status.success(), "{args:?}");
        let e = String::from_utf8_lossy(&o.stderr);
        assert!(e.contains(msg), "{args:?}: {e}");
    }
    let o = cli(&["measure", &bench(), "--out", "x.zma"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("--probe"));
    let o = cli(&["fit", &bench(), "--condition", "a=1"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("after a --curve"));
    let help = ok(&cli(&["help"]));
    assert!(help.contains("acoustilab measure"));
}
