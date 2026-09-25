//! Integration tests of the `ir`, `poles` and `isolation` subcommands.

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
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("time-{}-{name}", std::process::id()))
}

fn ok(o: &Output) -> String {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout.clone()).unwrap()
}

/// `ir` prints the JSON report and writes the chosen phase as a 32-bit
/// float WAV normalised to a peak of 1 (header per the RIFF/WAVE float
/// format: tag 3, 32 bits, `fact` chunk).
#[test]
fn ir_json_and_wav() {
    let wav = scratch("ir.wav");
    let o = cli(&[
        "ir",
        &design(),
        "--n",
        "4096",
        "--no-check",
        "--pre",
        "0",
        "--wav",
        wav.to_str().unwrap(),
    ]);
    let v: Value = serde_json::from_str(&ok(&o)).unwrap();
    assert_eq!(v["n"], 4096);
    assert_eq!(v["probe"], "p_drp");
    assert_eq!(v["t0_s"], 0.0);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("normalised to peak 1"), "{stderr}");
    let b = std::fs::read(&wav).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    assert_eq!(&b[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes([b[20], b[21]]), 3);
    assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), 48_000);
    assert_eq!(u16::from_le_bytes([b[34], b[35]]), 32);
    assert_eq!(&b[38..42], b"fact");
    assert_eq!(u32::from_le_bytes(b[46..50].try_into().unwrap()), 4096);
    assert_eq!(b.len(), 58 + 4 * 4096);
    let peak = b[58..]
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()).abs())
        .fold(0.0f32, f32::max);
    assert_eq!(peak, 1.0);
    std::fs::remove_file(&wav).ok();
}

#[test]
fn ir_csv_and_options() {
    let text = ok(&cli(&[
        "ir",
        &design(),
        "--n",
        "2048",
        "--no-check",
        "--csv",
        "--phase",
        "mixed",
        "--set",
        "rear=open",
    ]));
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("t_s,ir,ir_min_phase,step,etc_dB"));
    assert_eq!(lines.count(), 2048);
    for bad in [
        vec!["ir", "--frobnicate"],
        vec!["ir", "--phase", "linear"],
        vec!["ir", "--n", "1000"],
        vec!["ir", "--probe", "nope"],
    ] {
        let mut args = vec![bad[0].to_string(), design()];
        args.extend(bad[1..].iter().map(|s| s.to_string()));
        let o = cli(&args.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(!o.status.success(), "{bad:?}");
        assert!(String::from_utf8_lossy(&o.stderr).starts_with("error:"));
    }
}

#[test]
fn poles_table_and_json() {
    let text = ok(&cli(&["poles", &design()]));
    assert!(text.contains("poles (in band):"), "{text}");
    assert!(text.contains("zeros (in band):"));
    assert!(text.contains("impulse length (E46"));
    let v: Value =
        serde_json::from_str(&ok(&cli(&["poles", &design(), "--json", "--order", "20"]))).unwrap();
    assert_eq!(v["order"], 20);
    assert!(v["attribution"].is_null());
}

#[test]
fn isolation_csv_and_json() {
    let text = ok(&cli(&["isolation", &design(), "--csv"]));
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("frequency_Hz,insertion_loss_dB,p_occluded_spl_dB,p_open_spl_dB,leak_re_open_dB,vent_re_open_dB")
    );
    assert_eq!(lines.count(), 265);
    let v: Value = serde_json::from_str(&ok(&cli(&["isolation", &design()]))).unwrap();
    assert!(v["bleed"]["spl_dB"].is_array());
    let o = cli(&["isolation", &design(), "--entrance", "a_front"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("does not separate"));
}
