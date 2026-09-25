//! `acoustilab validate`: the plumbing of the command line (the pipeline's
//! numbers are checked in crates/acoustilab/tests/reference_cup.rs).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_acoustilab"))
        .args(args)
        .output()
        .expect("run acoustilab")
}

fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("validatecli-{}-{name}", std::process::id()))
}

fn repo(rel: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
        .to_str()
        .unwrap()
        .to_string()
}

fn v1() -> String {
    repo("validation/reference_cup/predictions/v1")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn verify_accepts_the_frozen_predictions() {
    let o = cli(&["validate", "--verify", "--predictions", &v1()]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("every frozen file matches its manifest"));
}

#[test]
fn predictions_are_never_overwritten() {
    let o = cli(&[
        "validate",
        "--predict",
        "--protocol",
        &repo("validation/reference_cup/protocol.json"),
        "--out",
        &v1(),
    ]);
    assert!(!o.status.success());
    assert!(text(&o).contains("exists and is not empty"), "{}", text(&o));
    let o = cli(&["validate", "--predict", "--out", "x", "--set", "fidelity=0"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("takes no --set"), "{}", text(&o));
}

#[test]
fn session_templates_and_an_empty_session() {
    let dir = scratch("session");
    let o = cli(&[
        "validate",
        "--session",
        dir.to_str().unwrap(),
        "--predictions",
        &v1(),
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let list = std::fs::read_to_string(dir.join("SESSION.txt")).unwrap();
    assert!(
        list.contains("iec_ref_p_s5") && list.contains("t43_ref_z"),
        "{list}"
    );
    assert!(dir.join("driver_free_z_s3.sidecar.json").exists());
    // Templates alone are no measurements: everything is missing, nothing
    // is accepted, and the run needs no fit.
    let o = cli(&[
        "validate",
        dir.to_str().unwrap(),
        "--predictions",
        &v1(),
        "--no-anchor",
    ]);
    assert!(o.status.success(), "{}", text(&o));
    let t = text(&o);
    assert!(
        t.contains("Incomplete session") && t.contains("not evaluated"),
        "{t}"
    );
    // A second session into the same directory is refused.
    let o = cli(&[
        "validate",
        "--session",
        dir.to_str().unwrap(),
        "--predictions",
        &v1(),
    ]);
    assert!(!o.status.success());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn predict_writes_a_verifiable_set_blind_only_on_request() {
    // The protocol on a coarse grid at level 0 with two configurations, so
    // that the command runs in a debug test.
    let mut p: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo("validation/reference_cup/protocol.json")).unwrap(),
    )
    .unwrap();
    p["netlist"] = serde_json::json!(repo("examples/reference_cup.json"));
    p["grid"] = serde_json::json!({"f_min_Hz": 100, "f_max_Hz": 1000, "points_per_octave": 3});
    p["overrides"]["fidelity"] = serde_json::json!(0);
    p["configurations"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["id"] == "driver_free" || c["id"] == "iec_ref");
    let dir = scratch("predict");
    std::fs::create_dir_all(&dir).unwrap();
    let protocol = dir.join("protocol.json");
    std::fs::write(&protocol, p.to_string()).unwrap();
    for (name, blind) in [("v1", true), ("v2", false)] {
        let out = dir.join(name);
        let mut args = vec![
            "validate",
            "--predict",
            "--protocol",
            protocol.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--runs",
            "2",
            "--allow-dirty",
        ];
        if blind {
            args.push("--blind");
        }
        let o = cli(&args);
        assert!(o.status.success(), "{}", text(&o));
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap())
                .unwrap();
        let status = manifest["status"].as_str().unwrap();
        assert_eq!(status.starts_with("blind:"), blind, "{status}");
        let o = cli(&[
            "validate",
            "--verify",
            "--predictions",
            out.to_str().unwrap(),
        ]);
        assert!(o.status.success(), "{}", text(&o));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
