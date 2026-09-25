//! `fit`, `measure` and `convert`: curve files, parameter identification and
//! the virtual rig from the command line (docs/fitting.md).
//!
//! ```text
//! acoustilab fit <netlist> --curve PROBE=FILE[:SIDECAR] [--condition NAME=VALUE]...
//!                --param NAME[=START]... [--spec FIT.json] [--band F1:F2]
//!                [--starts N] [--seed S] [--max-iter N] [--allow-spl-only]
//!                [--json] [--out REPORT.json]
//! acoustilab measure <netlist> --probe ID --out FILE [--format frd|zma|rew|csv]
//!                [--noise-db X] [--noise-deg Y] [--seed S] [--seatings N]
//!                [--averaging complex|magnitude|db] [--ppo N] [--band F1:F2]
//! acoustilab convert IN OUT [--quantity Q] [--ppo N]
//! ```
//!
//! A curve file's sidecar is `FILE.sidecar.json` unless one is named after
//! a colon. `--condition` sets a parameter for the preceding `--curve`
//! only (e.g. the added mass of that measurement); `--set` (parsed by the
//! caller) fixes a parameter for every curve, or sets the true values for
//! `measure`. Formats follow the file extension: `.frd`, `.zma`, `.csv`,
//! `.txt` (REW text).

use acoustilab::expr::PValue;
use acoustilab::fit::rig::{self, Noise, RigSpec};
use acoustilab::fit::{self, CurveSpec, FitParam, FitReport, FitSpec};
use acoustilab::io::curve::exchange_grid;
use acoustilab::io::sidecar::Averaging;
use acoustilab::io::{text, Curve, Format, Quantity, Sidecar};
use acoustilab::params::{Overrides, Parametric};
use std::path::Path;

pub const USAGE: &str = "  acoustilab fit <netlist.json> --curve PROBE=FILE[:SIDECAR] [--condition NAME=VALUE]...
      --param NAME[=START]... [--spec FIT.json] [--band F1:F2] [--starts N] [--seed S]
      [--max-iter N] [--allow-spl-only] [--json] [--out REPORT.json]
                                                         fit parameters to measured curves
  acoustilab measure <netlist.json> --probe ID --out FILE [--format frd|zma|rew|csv]
      [--noise-db X] [--noise-deg Y] [--seed S] [--seatings N]
      [--averaging complex|magnitude|db] [--ppo N] [--band F1:F2]
                                                         virtual-rig measurement (+ FILE.sidecar.json)
  acoustilab convert IN OUT [--quantity Q] [--ppo N]    convert curve files (sidecars follow)";

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn write(path: &str, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{path}: {e}"))
}

/// Sidecar file of a curve file.
pub fn sidecar_path(file: &str) -> String {
    format!("{file}.sidecar.json")
}

fn format_of(path: &str) -> Result<Format, String> {
    Format::from_path(path)
        .ok_or_else(|| format!("{path}: unknown curve format (use .frd, .zma, .csv or .txt)"))
}

/// Reads a curve file and its sidecar (`sidecar` if given, else
/// `FILE.sidecar.json` if it exists).
pub fn load_curve(
    file: &str,
    sidecar: Option<&str>,
    quantity: Option<Quantity>,
) -> Result<Curve, String> {
    let sc_path = sidecar
        .map(str::to_string)
        .or_else(|| Some(sidecar_path(file)).filter(|p| Path::new(p).exists()));
    let sc = sc_path
        .as_deref()
        .map(|p| Sidecar::parse(&read(p)?).map_err(|e| format!("{p}: {e}")))
        .transpose()?;
    let quantity = quantity.or(sc.as_ref().and_then(|s| s.quantity));
    let mut c = text::import(&read(file)?, format_of(file)?, quantity)
        .map_err(|e| format!("{file}: {e}"))?;
    if let Some(s) = sc {
        c.sidecar = s;
        c.sidecar.quantity = Some(c.quantity);
    }
    Ok(c)
}

/// Writes a curve file and its sidecar.
pub fn save_curve(c: &Curve, file: &str) -> Result<(), String> {
    let t = text::export(c, format_of(file)?).map_err(|e| format!("{file}: {e}"))?;
    write(file, &t)?;
    let mut sc = c.sidecar.clone();
    sc.quantity = Some(c.quantity);
    write(&sidecar_path(file), &sc.to_text())
}

fn value_of(s: &str) -> PValue {
    match s {
        "true" => PValue::Bool(true),
        "false" => PValue::Bool(false),
        _ => s
            .parse::<f64>()
            .map(PValue::Num)
            .unwrap_or_else(|_| PValue::Str(s.to_string())),
    }
}

fn number(opt: &str, v: Option<&String>) -> Result<f64, String> {
    v.ok_or_else(|| format!("{opt} needs a value"))?
        .parse::<f64>()
        .map_err(|_| format!("{opt} needs a number"))
}

fn band(v: Option<&String>) -> Result<(f64, f64), String> {
    let s = v.ok_or("--band needs F1:F2")?;
    let (a, b) = s.split_once(':').ok_or("--band needs F1:F2")?;
    let (a, b) = (
        a.parse::<f64>().map_err(|_| "--band needs numbers")?,
        b.parse::<f64>().map_err(|_| "--band needs numbers")?,
    );
    if !(a > 0.0 && b > a) {
        return Err("--band needs 0 < F1 < F2".into());
    }
    Ok((a, b))
}

/// Runs `fit`, `measure` or `convert`; `args[0]` is the command.
pub fn run(args: &[String], overrides: &Overrides) -> Result<(), String> {
    match args[0].as_str() {
        "fit" => fit_cmd(args, overrides),
        "measure" => measure_cmd(args, overrides),
        "convert" => convert_cmd(args),
        _ => Err(USAGE.into()),
    }
}

fn fit_cmd(args: &[String], overrides: &Overrides) -> Result<(), String> {
    let path = args.get(1).ok_or(USAGE)?;
    let p = Parametric::parse(&read(path)?).map_err(|e| e.to_string())?;
    let mut spec = FitSpec::new(Vec::new(), Vec::new());
    let mut json_out = false;
    let mut out: Option<String> = None;
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--spec" => {
                let f = it.next().ok_or("--spec needs a file")?;
                let v: serde_json::Value =
                    serde_json::from_str(&read(f)?).map_err(|e| format!("{f}: {e}"))?;
                let s = FitSpec::from_json(&v).map_err(|e| format!("{f}: {e}"))?;
                spec.parameters.extend(s.parameters);
                spec.curves.extend(s.curves);
                spec.overrides.extend(s.overrides);
                spec.f_min = s.f_min;
                spec.f_max = s.f_max;
                spec.max_iterations = s.max_iterations;
                spec.max_evaluations = s.max_evaluations;
                spec.starts = s.starts;
                spec.seed = s.seed;
                spec.allow_spl_only |= s.allow_spl_only;
                spec.rank_tolerance = s.rank_tolerance;
            }
            "--curve" => {
                let c = it.next().ok_or("--curve needs PROBE=FILE")?;
                let (probe, rest) = c.split_once('=').ok_or("--curve needs PROBE=FILE")?;
                let (file, sidecar) = match rest.rsplit_once(':') {
                    Some((f, s)) if s.ends_with(".json") => (f, Some(s)),
                    _ => (rest, None),
                };
                spec.curves
                    .push(CurveSpec::new(probe, load_curve(file, sidecar, None)?));
            }
            "--condition" => {
                let c = it.next().ok_or("--condition needs NAME=VALUE")?;
                let (k, v) = c.split_once('=').ok_or("--condition needs NAME=VALUE")?;
                spec.curves
                    .last_mut()
                    .ok_or("--condition belongs after a --curve")?
                    .overrides
                    .insert(k.to_string(), value_of(v));
            }
            "--param" => {
                let s = it.next().ok_or("--param needs NAME")?;
                let mut fp = FitParam::new(s);
                if let Some((n, v)) = s.split_once('=') {
                    fp = FitParam::new(n);
                    fp.start = Some(
                        v.parse::<f64>()
                            .map_err(|_| format!("--param {s}: the start must be a number"))?,
                    );
                }
                spec.parameters.push(fp);
            }
            "--band" => (spec.f_min, spec.f_max) = band(it.next())?,
            "--starts" => spec.starts = number(a, it.next())?.max(1.0) as usize,
            "--seed" => spec.seed = number(a, it.next())? as u64,
            "--max-iter" => spec.max_iterations = number(a, it.next())?.max(1.0) as usize,
            "--allow-spl-only" => spec.allow_spl_only = true,
            "--json" => json_out = true,
            "--out" => out = Some(it.next().ok_or("--out needs a file")?.clone()),
            other => return Err(format!("unknown option '{other}'\n{USAGE}")),
        }
    }
    spec.overrides.extend(overrides.clone());
    let rep = fit::fit(&p, &spec).map_err(|e| e.to_string())?;
    let doc = format!("{:#}\n", rep.to_json());
    if let Some(f) = &out {
        write(f, &doc)?;
    }
    if json_out {
        print!("{doc}");
    } else {
        print!("{}", summary(&rep));
    }
    Ok(())
}

/// Human-readable fit report.
pub fn summary(r: &FitReport) -> String {
    let mut s = format!(
        "{} after {} iteration(s), {} evaluation(s); reduced chi-square {}\n\n",
        r.stop_reason,
        r.iterations,
        r.evaluations,
        r.reduced_chi2
            .map_or("n/a".to_string(), |x| format!("{x:.4}"))
    );
    for p in &r.parameters {
        let ci = p.ci95.map_or("no interval".to_string(), |[a, b]| {
            format!("95 % [{a:.6}, {b:.6}]")
        });
        s += &format!(
            "  {:<24} {:>14.6} {:<8} {:<30} {:?}\n",
            p.name,
            p.value,
            p.unit.as_deref().unwrap_or(""),
            ci,
            p.status
        );
    }
    for o in &r.offsets {
        s += &format!(
            "  level offset of curve {} ({}): {:.4} dB\n",
            o.curve, o.probe, o.value_db
        );
    }
    s.push('\n');
    for c in &r.curves {
        s += &format!(
            "  curve {} ({}, {} points): RMS {:.4} dB{}{}{}\n",
            c.index,
            c.probe,
            c.points,
            c.rms_db,
            c.rms_deg.map_or(String::new(), |d| format!(", {d:.4} deg")),
            c.rms_ohm.map_or(String::new(), |o| format!(", {o:.4} ohm")),
            if c.structured { ", structured" } else { "" }
        );
    }
    for line in r.summary.iter().chain(&r.warnings) {
        s += &format!("\n{line}\n");
    }
    s
}

fn measure_cmd(args: &[String], overrides: &Overrides) -> Result<(), String> {
    let path = args.get(1).ok_or(USAGE)?;
    let p = Parametric::parse(&read(path)?).map_err(|e| e.to_string())?;
    let mut probe: Option<String> = None;
    let mut out: Option<String> = None;
    let mut format: Option<Format> = None;
    let mut noise = Noise::default();
    let mut ppo = 48.0;
    let (mut f_lo, mut f_hi) = (10.0, 20_000.0);
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--probe" => probe = Some(it.next().ok_or("--probe needs an id")?.clone()),
            "--out" => out = Some(it.next().ok_or("--out needs a file")?.clone()),
            "--format" => {
                let f = it.next().ok_or("--format needs frd, zma, rew or csv")?;
                format = Some(
                    Format::parse(f)
                        .filter(|f| *f != Format::Auto)
                        .ok_or_else(|| format!("unknown format '{f}'"))?,
                );
            }
            "--noise-db" => noise.level_db = number(a, it.next())?,
            "--noise-deg" => noise.phase_deg = number(a, it.next())?,
            "--seed" => noise.seed = number(a, it.next())? as u64,
            "--seatings" => noise.seatings = number(a, it.next())?.max(1.0) as u32,
            "--averaging" => {
                let v = it.next().ok_or("--averaging needs a method")?;
                noise.averaging =
                    Averaging::parse(v).ok_or_else(|| format!("unknown averaging '{v}'"))?;
            }
            "--ppo" => ppo = number(a, it.next())?,
            "--band" => (f_lo, f_hi) = band(it.next())?,
            other => return Err(format!("unknown option '{other}'\n{USAGE}")),
        }
    }
    if noise.seatings > 1 && noise.averaging == Averaging::None {
        noise.averaging = Averaging::Complex;
    }
    let probe = probe.ok_or("measure needs --probe")?;
    let out = out.ok_or("measure needs --out FILE")?;
    let mut spec = RigSpec::new(&probe);
    spec.overrides = overrides.clone();
    spec.freqs = exchange_grid(f_lo, f_hi, ppo);
    spec.noise = noise;
    let c = rig::measure(&p, &spec).map_err(|e| e.to_string())?;
    let file_format = match format {
        Some(f) => f,
        None => format_of(&out)?,
    };
    let t = text::export(&c, file_format).map_err(|e| format!("{out}: {e}"))?;
    write(&out, &t)?;
    write(&sidecar_path(&out), &c.sidecar.to_text())?;
    println!(
        "wrote {out} ({} points, {}) and {}",
        c.len(),
        c.quantity.name(),
        sidecar_path(&out)
    );
    Ok(())
}

fn convert_cmd(args: &[String]) -> Result<(), String> {
    let (inp, outp) = match (args.get(1), args.get(2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return Err(USAGE.into()),
    };
    let mut quantity = None;
    let mut ppo: Option<f64> = None;
    let mut it = args[3..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--quantity" => {
                let q = it.next().ok_or("--quantity needs a name")?;
                quantity =
                    Some(Quantity::parse(q).ok_or_else(|| format!("unknown quantity '{q}'"))?);
            }
            "--ppo" => ppo = Some(number(a, it.next())?),
            other => return Err(format!("unknown option '{other}'\n{USAGE}")),
        }
    }
    let mut c = load_curve(inp, None, quantity)?;
    if let Some(n) = ppo {
        c = c.resample_exchange(n).map_err(|e| e.to_string())?;
        let note = format!(
            "resampled to {n} points per octave (level in dB and unwrapped phase, linear in ln f)"
        );
        c.sidecar.notes = Some(match c.sidecar.notes.take() {
            Some(old) => format!("{old}; {note}"),
            None => note,
        });
    }
    save_curve(&c, outp)?;
    println!(
        "wrote {outp} ({} points) and {}",
        c.len(),
        sidecar_path(outp)
    );
    Ok(())
}
