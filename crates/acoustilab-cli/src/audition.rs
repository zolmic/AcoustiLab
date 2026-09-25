//! `audition` subcommand: the audition filter of spec Section 16
//! (`docs/auralization.md`), as a JSON report and optionally the FIR as a
//! 32-bit float WAV file.
//!
//! The candidate is the netlist with its `--set` overrides. The baseline is,
//! in order of precedence: `--target NAME|FILE.csv` (a bundled target or a
//! fixture-tagged CSV), `--curve FILE` (a measured curve: FRD, ZMA, REW text,
//! CSV or an acoustilab-curve JSON document), `--baseline FILE.json` (another
//! netlist, with `--baseline-set`), else the same netlist with only the
//! `--baseline-set` overrides: by default the template the candidate was
//! edited from. `--absolute` needs no baseline.

use acoustilab::audition::report::{curve_baseline, parse_options, target_baseline, to_json};
use acoustilab::audition::{audition, Baseline, Design, MagnitudeBaseline};
use acoustilab::io::text::{self, Format};
use acoustilab::params::Overrides;
use acoustilab::time::wav::float_wav;
use serde_json::{json, Map, Value};
use std::io::Write;

pub const USAGE: &str = "  acoustilab audition <netlist.json> [--baseline FILE.json] [--baseline-set NAME=VALUE]...
                 [--target NAME|FILE.csv] [--curve FILE] [--absolute] [--phase auto|minimum|mixed|linear]
                 [--fs 48000] [--n N] [--probe ID] [--wav FILE] [--taps] [--out FILE]
                                                        audition filter: candidate (with --set) over a baseline";

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn overrides_json(o: &Overrides) -> Value {
    Value::Object(
        o.iter()
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect::<Map<_, _>>(),
    )
}

pub fn run(path: &str, args: &[String], overrides: &Overrides) -> Result<(), String> {
    let mut baseline_file = None;
    let mut baseline_sets = Overrides::new();
    let mut target = None;
    let mut curve = None;
    let mut opts = Map::new();
    let mut probe = None;
    let mut wav = None;
    let mut out = None;
    let mut taps = false;
    let mut it = args.iter();
    let value = |it: &mut std::slice::Iter<String>, name: &str| {
        it.next()
            .cloned()
            .ok_or_else(|| format!("{name} needs a value"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--baseline" => baseline_file = Some(value(&mut it, a)?),
            "--baseline-set" => {
                let (k, v) = crate::parse_set(&value(&mut it, a)?)?;
                baseline_sets.insert(k, v);
            }
            "--target" => target = Some(value(&mut it, a)?),
            "--curve" => curve = Some(value(&mut it, a)?),
            "--absolute" => {
                opts.insert("mode".into(), json!("absolute"));
            }
            "--phase" => {
                opts.insert("phase".into(), json!(value(&mut it, a)?));
            }
            "--fs" => {
                let v = value(&mut it, a)?;
                let fs: f64 = v.parse().map_err(|_| format!("--fs: bad number '{v}'"))?;
                opts.insert("fs_Hz".into(), json!(fs));
            }
            "--n" => {
                let v = value(&mut it, a)?;
                let n: u64 = v.parse().map_err(|_| format!("--n: bad number '{v}'"))?;
                opts.insert("n".into(), json!(n));
            }
            "--probe" => probe = Some(value(&mut it, a)?),
            "--wav" => wav = Some(value(&mut it, a)?),
            "--out" => out = Some(value(&mut it, a)?),
            "--taps" => taps = true,
            other => return Err(format!("unknown option '{other}'\n{USAGE}")),
        }
    }
    let (options, _) = parse_options(&Value::Object(opts)).map_err(|e| e.message)?;
    let text = read(path)?;
    let mut cand = Design::new(
        &text,
        overrides,
        overrides_json(overrides),
        probe.as_deref(),
    )
    .map_err(|e| format!("candidate: {e}"))?;
    let magnitude: Option<MagnitudeBaseline> = match (&target, &curve) {
        (Some(t), _) if t.ends_with(".csv") => {
            let tt = acoustilab::targets::import::import_target_csv(&read(t)?, None, None)
                .map_err(|e| format!("{t}: {e}"))?;
            Some(target_baseline(&tt.to_json()).map_err(|e| e.message)?)
        }
        (Some(t), _) => Some(target_baseline(&json!(t)).map_err(|e| e.message)?),
        (None, Some(c)) => {
            let body = read(c)?;
            let doc = if c.ends_with(".json") {
                serde_json::from_str(&body).map_err(|e| format!("{c}: {e}"))?
            } else {
                let format =
                    Format::from_path(c).ok_or_else(|| format!("{c}: unknown curve format"))?;
                text::import(&body, format, None)
                    .map_err(|e| format!("{c}: {e}"))?
                    .to_json()
            };
            Some(curve_baseline(&doc, Some(c.clone())).map_err(|e| e.message)?)
        }
        (None, None) => None,
    };
    let absolute = options.mode == acoustilab::audition::Mode::Absolute;
    let result = if absolute {
        audition(&mut cand, Baseline::None, &options)
    } else if let Some(m) = &magnitude {
        audition(&mut cand, Baseline::Magnitude(m), &options)
    } else {
        let btext = match &baseline_file {
            Some(f) => read(f)?,
            None => text.clone(),
        };
        let mut base = Design::new(
            &btext,
            &baseline_sets,
            overrides_json(&baseline_sets),
            probe.as_deref(),
        )
        .map_err(|e| format!("baseline: {e}"))?;
        audition(&mut cand, Baseline::Design(&mut base), &options)
    }
    .map_err(|e| e.to_string())?;
    if let Some(file) = wav {
        let h: Vec<f64> = result.taps.iter().map(|&v| v as f64).collect();
        let bytes = float_wav(&[&h], result.fs_hz.round() as u32)?;
        std::fs::write(&file, bytes).map_err(|e| format!("{file}: {e}"))?;
        eprintln!(
            "wrote {file}: {} taps at {} Hz, {} phase, latency {} samples, 0 dB = the filter's 500 Hz-2 kHz mean",
            result.taps.len(),
            result.fs_hz,
            result.phase.used,
            result.latency_samples
        );
    }
    let text = format!("{:#}\n", to_json(&result, taps));
    match out {
        Some(f) => std::fs::write(&f, text).map_err(|e| format!("{f}: {e}")),
        None => std::io::stdout()
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string()),
    }
}
