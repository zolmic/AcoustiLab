//! Integration tests of the `audition` subcommand: the report equals the
//! engine's, the WAV holds the taps sample for sample, and the baseline
//! defaults to the netlist without the candidate's `--set` overrides.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_acoustilab"))
        .args(args)
        .output()
        .expect("run acoustilab")
}

fn design() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/design_over_ear.json")
        .to_str()
        .unwrap()
        .to_string()
}

fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("audition-{}-{name}", std::process::id()))
}

fn ok(o: &Output) -> String {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout.clone()).unwrap()
}

/// Float samples of a 32-bit IEEE WAV (data chunk found by name).
fn wav_samples(bytes: &[u8]) -> Vec<f32> {
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            return bytes[i + 8..i + 8 + len]
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
        }
        i += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

#[test]
fn audition_report_and_wav() {
    let wav = scratch("f.wav");
    let o = cli(&[
        "audition",
        &design(),
        "--set",
        "vent_count=3",
        "--taps",
        "--wav",
        wav.to_str().unwrap(),
    ]);
    let v: Value = serde_json::from_str(&ok(&o)).unwrap();
    assert_eq!(v["phase"]["used"], "minimum");
    assert_eq!(v["check"]["met"], true);
    assert_eq!(v["state"]["candidate"]["overrides"]["vent_count"], 3.0);
    assert_eq!(v["state"]["baseline"]["overrides"], serde_json::json!({}));
    let taps: Vec<f32> = v["taps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect();
    assert_eq!(wav_samples(&std::fs::read(&wav).unwrap()), taps);
    let _ = std::fs::remove_file(&wav);
}

#[test]
fn audition_baselines() {
    let t = cli(&["audition", &design(), "--target", "ravizza2023_5128"]);
    let v: Value = serde_json::from_str(&ok(&t)).unwrap();
    assert_eq!(v["baseline"]["kind"], "target");
    assert_eq!(v["band_Hz"], serde_json::json!([31.0, 10000.0]));
    let a = cli(&["audition", &design(), "--absolute", "--phase", "linear"]);
    let v: Value = serde_json::from_str(&ok(&a)).unwrap();
    assert_eq!(v["mode"], "absolute");
    assert_eq!(v["latency_samples"], 4096);
    let bad = cli(&["audition", &design(), "--phase", "sideways"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("'phase'"));
}
