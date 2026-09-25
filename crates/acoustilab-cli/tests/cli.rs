//! Integration tests of the `acoustilab` command-line runner, run on every
//! netlist in `examples/`: `check`, `solve` (JSON fields and values, CSV
//! header and row count, `--out`), `types`, `help`, `version`, and error
//! exits that name the offending file, element or option.
//!
//! These live in the CLI package because Cargo only provides
//! `CARGO_BIN_EXE_acoustilab` to integration tests of the package that
//! builds the binary.

use acoustilab::{Circuit, SolveResult};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_acoustilab"))
        .args(args)
        .output()
        .expect("run acoustilab")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

fn examples() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    assert!(v.len() >= 4, "{v:?}");
    v
}

fn library(path: &Path) -> (Circuit, SolveResult) {
    let c = Circuit::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let r = c.solve().unwrap();
    (c, r)
}

/// A scratch file in Cargo's per-target temporary directory.
fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{}-{name}", std::process::id()))
}

fn arg(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn types_lists_every_registered_element_type() {
    let o = cli(&["types"]);
    assert!(o.status.success());
    let listed: Vec<String> = stdout(&o).lines().map(str::to_string).collect();
    let known: Vec<String> = acoustilab::elements::known_types()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(listed, known);
    for t in [
        "coil",
        "motor",
        "piston",
        "cavity",
        "tube",
        "slit",
        "radiation",
    ] {
        assert!(listed.iter().any(|l| l == t), "{t} missing");
    }
}

#[test]
fn help_and_version() {
    for flag in ["help", "--help", "-h"] {
        let o = cli(&[flag]);
        assert!(o.status.success(), "{flag}");
        assert!(stdout(&o).contains("acoustilab solve <netlist.json>"));
    }
    for flag in ["version", "--version", "-V"] {
        let o = cli(&[flag]);
        assert!(o.status.success(), "{flag}");
        assert_eq!(stdout(&o).trim(), acoustilab::solve::ENGINE);
    }
}

#[test]
fn check_reports_the_compiled_network_of_every_example() {
    for path in examples() {
        let o = cli(&["check", arg(&path)]);
        assert!(o.status.success(), "{}: {}", path.display(), stderr(&o));
        let (c, _) = library(&path);
        let expect = format!(
            "ok: {} nodes, {} elements, {} unknowns, {} probes, {} frequencies, level {}",
            c.nodes.len(),
            c.elements.len(),
            c.dim,
            c.probes.len(),
            c.freqs.len(),
            c.level
        );
        assert_eq!(stdout(&o).trim(), expect, "{}", path.display());
    }
}

/// Equal to within four units in the last place.
fn ulps(a: f64, b: f64) -> bool {
    (a - b).abs() <= 4.0 * f64::EPSILON * a.abs().max(b.abs())
}

fn floats(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

#[test]
fn solve_json_has_the_documented_fields_and_the_library_values() {
    for path in examples() {
        let o = cli(&["solve", arg(&path)]);
        assert!(o.status.success(), "{}: {}", path.display(), stderr(&o));
        let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
        let (c, r) = library(&path);
        let ctx = path.display();
        // Metadata.
        assert_eq!(v["meta"]["engine"], acoustilab::solve::ENGINE);
        assert_eq!(v["meta"]["level"], c.level);
        for k in ["temperature_k", "p0", "rho", "c", "mu", "gamma", "prandtl"] {
            assert!(v["meta"]["air"][k].is_f64(), "{ctx}: meta.air.{k}");
        }
        assert_eq!(v["meta"]["air"]["c"].as_f64().unwrap(), c.air.c);
        // Frequencies. The CLI prints shortest round-trip floats, but
        // serde_json's default parser (no `float_roundtrip` feature) may land
        // one ulp off, so values compare to a few ulps.
        let freqs = floats(&v["frequencies_Hz"]);
        assert_eq!(freqs.len(), r.freqs_hz.len(), "{ctx}");
        assert!(
            freqs.iter().zip(&r.freqs_hz).all(|(a, b)| ulps(*a, *b)),
            "{ctx}"
        );
        let n = r.freqs_hz.len();
        // Probes, in netlist order, with complex values and derived fields.
        let probes = v["probes"].as_array().unwrap();
        assert_eq!(probes.len(), r.probes.len(), "{ctx}");
        for (p, lib) in probes.iter().zip(&r.probes) {
            assert_eq!(p["id"], lib.id.as_str());
            assert_eq!(p["quantity"], lib.quantity.as_str());
            assert!(p["unit"].is_string());
            let (re, im) = (floats(&p["re"]), floats(&p["im"]));
            let mag = floats(&p["magnitude"]);
            let phase = floats(&p["phase_deg"]);
            assert!([re.len(), im.len(), mag.len(), phase.len()]
                .iter()
                .all(|&l| l == n));
            for k in 0..n {
                assert!(
                    ulps(re[k], lib.values[k].re) && ulps(im[k], lib.values[k].im),
                    "{ctx} {}",
                    lib.id
                );
                assert!((mag[k] - re[k].hypot(im[k])).abs() <= 1e-12 * mag[k]);
                assert!((phase[k] - im[k].atan2(re[k]).to_degrees()).abs() < 1e-9);
            }
            // Acoustic pressures also carry dB SPL re 20 µPa (RMS phasors).
            assert_eq!(
                p.get("spl_dB").is_some(),
                lib.is_pressure,
                "{ctx} {}",
                lib.id
            );
            if lib.is_pressure {
                let spl = floats(&p["spl_dB"]);
                for k in 0..n {
                    assert!((spl[k] - 20.0 * (mag[k] / 20e-6).log10()).abs() < 1e-9);
                }
            }
        }
        // Validity limits and the aggregated shading band.
        assert_eq!(
            v["validity"].as_array().unwrap().len(),
            r.validity.len(),
            "{ctx}"
        );
        for l in v["validity"].as_array().unwrap() {
            assert!(l["element"].is_string() && l["criterion"].is_string());
        }
        assert!(v["shading"].get("begin_hz").is_some() && v["shading"].get("deep_hz").is_some());
    }
}

#[test]
fn solve_csv_has_one_header_and_one_row_per_frequency() {
    for path in examples() {
        let o = cli(&["solve", arg(&path), "--csv"]);
        assert!(o.status.success(), "{}: {}", path.display(), stderr(&o));
        let text = stdout(&o);
        let (_, r) = library(&path);
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        let mut expect = vec!["frequency_Hz".to_string()];
        for p in &r.probes {
            for s in ["re", "im", "mag", "phase_deg"] {
                expect.push(format!("{}_{s}", p.id));
            }
            if p.is_pressure {
                expect.push(format!("{}_spl_dB", p.id));
            }
        }
        assert_eq!(header, expect, "{}", path.display());
        let rows: Vec<Vec<f64>> = lines
            .map(|l| l.split(',').map(|x| x.parse::<f64>().unwrap()).collect())
            .collect();
        assert_eq!(rows.len(), r.freqs_hz.len(), "{}", path.display());
        for (row, f) in rows.iter().zip(&r.freqs_hz) {
            assert_eq!(row.len(), header.len());
            assert_eq!(row[0], *f);
        }
        // Spot-check the first probe's real part against the library.
        for (k, row) in rows.iter().enumerate() {
            assert_eq!(row[1], r.probes[0].values[k].re);
        }
    }
}

#[test]
fn out_option_writes_the_file_instead_of_stdout() {
    let path = &examples()[0];
    let direct = stdout(&cli(&["solve", arg(path)]));
    let out = scratch("out.json");
    let o = cli(&["solve", arg(path), "--out", arg(&out)]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(o.stdout.is_empty());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), direct);
    let csv = scratch("out.csv");
    let o = cli(&["solve", arg(path), "--csv", "--out", arg(&csv)]);
    assert!(o.status.success(), "{}", stderr(&o));
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with("frequency_Hz,"));
    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(csv);
}

#[test]
fn errors_exit_nonzero_and_name_the_culprit() {
    let fail = |args: &[&str], needles: &[&str]| {
        let o = cli(args);
        assert!(!o.status.success(), "{args:?} succeeded");
        let msg = stderr(&o);
        assert!(msg.starts_with("error: "), "{msg}");
        for n in needles {
            assert!(msg.contains(n), "{args:?}: {msg:?} lacks {n:?}");
        }
    };
    fail(&[], &["usage:"]);
    fail(&["frobnicate"], &["usage:"]);
    fail(&["check"], &["usage:"]);
    fail(
        &["check", "no/such/netlist.json"],
        &["no/such/netlist.json"],
    );
    let example = examples()[0].clone();
    fail(
        &["solve", arg(&example), "--pdf"],
        &["unknown option '--pdf'"],
    );
    fail(&["solve", arg(&example), "--out"], &["--out needs a file"]);

    let bad = scratch("unitless.json");
    std::fs::write(
        &bad,
        r#"{"nodes": [{"id": "a", "domain": "acoustic"}],
            "elements": [{"id": "cup", "type": "cavity", "node": "a", "volume": 30}]}"#,
    )
    .unwrap();
    fail(&["check", arg(&bad)], &["cup", "volume"]);
    fail(&["solve", arg(&bad)], &["cup", "volume"]);

    let json = scratch("broken.json");
    std::fs::write(&json, "{ not json").unwrap();
    fail(&["check", arg(&json)], &["netlist JSON"]);

    // Parses, but a floating node makes the solve fail and name it.
    let floating = scratch("floating.json");
    std::fs::write(
        &floating,
        r#"{"nodes": [{"id": "a", "domain": "acoustic"}, {"id": "orphan", "domain": "acoustic"}],
            "sweep": {"frequencies_Hz": [100]},
            "elements": [{"id": "u", "type": "flow_source", "node": "a"},
                         {"id": "cup", "type": "cavity", "node": "a", "volume_cm3": 30}]}"#,
    )
    .unwrap();
    assert!(cli(&["check", arg(&floating)]).status.success());
    fail(&["solve", arg(&floating)], &["singular", "orphan"]);
    for f in [bad, json, floating] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn set_overrides_parameters_and_params_lists_them() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/design_over_ear.json");
    let path = path.to_str().unwrap();
    let o = cli(&["params", path, "--set", "front_radius_mm=20"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let text = stdout(&o);
    assert!(text.contains("front_radius_mm = 20 mm"), "{text}");
    assert!(
        text.contains("front_volume_cm3 = 25.13274122871835 cm3 (derived)"),
        "{text}"
    );
    assert!(text.contains("ear = 'iec60318_4'"), "{text}");

    let o = cli(&["solve", path, "--set", "rear=open", "--set=vent_count=0"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["meta"]["parameters"]["rear"], "open");
    assert_eq!(v["meta"]["parameters"]["vent_count"], 0);

    let o = cli(&["solve", path, "--set", "vent_count=2.5"]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("parameter 'vent_count'"),
        "{}",
        stderr(&o)
    );
    let o = cli(&["check", path, "--set", "novalue"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("NAME=VALUE"), "{}", stderr(&o));
}
