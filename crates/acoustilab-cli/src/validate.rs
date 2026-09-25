//! `validate`: the open reference headphone's measurement session against
//! its frozen blind predictions (spec Section 17; docs/reference-cup.md).
//!
//! ```text
//! acoustilab validate <measurement dir> [--predictions DIR] [--no-anchor]
//!                     [--max-evals N] [--json] [--out REPORT.json] [--set NAME=VALUE]...
//! acoustilab validate --session OUT_DIR [--predictions DIR]
//! acoustilab validate --simulate OUT_DIR [--predictions DIR] [--seed S] [--ppo N]
//!                     [--omit ID]... [--truth-netlist FILE] [--set NAME=VALUE]...
//! acoustilab validate --predict --out DIR [--protocol FILE] [--runs N]
//! acoustilab validate --verify [--predictions DIR]
//! acoustilab validate --drift [--predictions DIR] [--netlist FILE]
//! ```
//!
//! The protocol defaults to `validation/reference_cup/protocol.json` and
//! the predictions to the highest `v<N>` directory under the protocol's
//! `predictions/`. `--set` changes the fitted model of a validation (e.g.
//! `fidelity=0` for a quick lumped run) or the true device of a simulation.

use acoustilab::params::Overrides;
use acoustilab::validation::predict::{self, Frozen, PredictOptions};
use acoustilab::validation::session::{self, ValidateOptions};
use acoustilab::validation::simulate::{self, SimulateOptions};
use acoustilab::validation::{Files, Protocol};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

pub const USAGE: &str = "  acoustilab validate <measurement dir> [--predictions DIR] [--no-anchor] [--max-evals N]
      [--json] [--out REPORT.json] [--set NAME=VALUE]...
                                                         a measurement session against the frozen predictions
  acoustilab validate --session OUT_DIR                  sidecar templates and the checklist of a session
  acoustilab validate --simulate OUT_DIR [--seed S] [--ppo N] [--omit ID]... [--truth-netlist FILE]
                                                         a synthetic session from the virtual rig
  acoustilab validate --predict --out DIR [--protocol FILE] [--runs N]
                                                         freeze blind predictions into a new version directory
  acoustilab validate --verify | --drift [--netlist FILE]
                                                         check the frozen files / report the engine's drift
    validate takes --protocol FILE (default validation/reference_cup/protocol.json) and
    --predictions DIR (default: the highest v<N> under the protocol's predictions/)";

const DEFAULT_PROTOCOL: &str = "validation/reference_cup/protocol.json";

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every regular file of a directory (not recursive), by name.
fn read_dir(dir: &Path) -> Result<Files, String> {
    let mut files = Files::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in entries {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = e.path();
        if path.is_file() {
            let name = e
                .file_name()
                .into_string()
                .map_err(|_| format!("{}: a file name is not UTF-8", dir.display()))?;
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            files.insert(name, bytes);
        }
    }
    Ok(files)
}

/// Writes files into a directory that must not exist or be empty.
fn write_new_dir(
    dir: &Path,
    files: &std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    if dir.exists() {
        let mut entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if entries.next().is_some() {
            return Err(format!(
                "{} exists and is not empty: never overwrite; choose a new directory",
                dir.display()
            ));
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, content) in files {
        let p = dir.join(name);
        std::fs::write(&p, content).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

/// Lexical normalisation (`a/b/../c` → `a/c`), for the paths recorded in
/// a manifest.
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir
                if out
                    .components()
                    .next_back()
                    .is_some_and(|l| matches!(l, Component::Normal(_))) =>
            {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The highest `v<N>` directory under `<protocol dir>/predictions`.
fn latest_predictions(protocol: &Path) -> Result<PathBuf, String> {
    let dir = protocol
        .parent()
        .unwrap_or(Path::new("."))
        .join("predictions");
    let mut best: Option<(u64, PathBuf)> = None;
    for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(n) = name.strip_prefix('v').and_then(|n| n.parse::<u64>().ok()) {
            if e.path().is_dir() && best.as_ref().is_none_or(|(b, _)| n > *b) {
                best = Some((n, e.path()));
            }
        }
    }
    best.map(|(_, p)| p).ok_or_else(|| {
        format!(
            "no prediction version (v1, v2, ...) under {}",
            dir.display()
        )
    })
}

/// UTC now as ISO 8601 (civil date from days since 1970, Hinnant's
/// algorithm).
fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// `git rev-parse HEAD` and whether `git status --porcelain` is empty.
fn git_state() -> (Option<String>, Option<bool>) {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let commit = run(&["rev-parse", "HEAD"]);
    let dirty = run(&["status", "--porcelain"]).map(|s| !s.is_empty());
    (commit, dirty)
}

fn number(opt: &str, v: Option<&String>) -> Result<f64, String> {
    v.ok_or_else(|| format!("{opt} needs a value"))?
        .parse::<f64>()
        .map_err(|_| format!("{opt} needs a number"))
}

/// Runs `validate`; `args[0]` is "validate".
pub fn run(args: &[String], overrides: &Overrides) -> Result<(), String> {
    let mut mode = "validate";
    let mut target: Option<String> = None;
    let mut protocol = PathBuf::from(DEFAULT_PROTOCOL);
    let mut predictions: Option<PathBuf> = None;
    let mut out: Option<String> = None;
    let mut json_out = false;
    let mut anchor = true;
    let mut max_evals: Option<usize> = None;
    let mut runs: Option<usize> = None;
    let mut netlist: Option<String> = None;
    let mut truth_netlist: Option<String> = None;
    let mut sim = SimulateOptions::default();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--predict" => mode = "predict",
            "--verify" => mode = "verify",
            "--drift" => mode = "drift",
            "--session" => {
                mode = "session";
                target = Some(it.next().ok_or("--session needs a directory")?.clone());
            }
            "--simulate" => {
                mode = "simulate";
                target = Some(it.next().ok_or("--simulate needs a directory")?.clone());
            }
            "--protocol" => protocol = PathBuf::from(it.next().ok_or("--protocol needs a file")?),
            "--predictions" => {
                predictions = Some(PathBuf::from(
                    it.next().ok_or("--predictions needs a directory")?,
                ))
            }
            "--out" => out = Some(it.next().ok_or("--out needs a path")?.clone()),
            "--json" => json_out = true,
            "--no-anchor" => anchor = false,
            "--max-evals" => max_evals = Some(number(a, it.next())?.max(1.0) as usize),
            "--runs" => runs = Some(number(a, it.next())?.max(1.0) as usize),
            "--netlist" => netlist = Some(it.next().ok_or("--netlist needs a file")?.clone()),
            "--truth-netlist" => {
                truth_netlist = Some(it.next().ok_or("--truth-netlist needs a file")?.clone())
            }
            "--seed" => sim.seed = number(a, it.next())? as u64,
            "--ppo" => sim.points_per_octave = number(a, it.next())?,
            "--omit" => sim
                .omit
                .push(it.next().ok_or("--omit needs a measurement id")?.clone()),
            other if !other.starts_with("--") && target.is_none() => {
                target = Some(other.to_string())
            }
            other => return Err(format!("unknown option '{other}'\n{USAGE}")),
        }
    }
    if mode == "predict" {
        if !overrides.is_empty() {
            return Err(
                "--predict takes no --set: predictions come from the protocol alone".into(),
            );
        }
        let out = PathBuf::from(out.ok_or("--predict needs --out DIR (a new version directory)")?);
        let ptext = read(&protocol)?;
        let p = Protocol::parse(&ptext).map_err(|e| format!("{}: {e}", protocol.display()))?;
        let npath = normalise(&protocol.parent().unwrap_or(Path::new(".")).join(&p.netlist));
        let ntext = read(&npath)?;
        let (commit, dirty) = git_state();
        let opts = PredictOptions {
            version: out
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            created: now_utc(),
            git_commit: commit,
            git_dirty: dirty,
            command: format!("acoustilab {}", args.join(" ")),
            protocol_source: normalise(&protocol).display().to_string(),
            netlist_source: npath.display().to_string(),
            runs,
        };
        let files = predict::predict(&ptext, &ntext, &opts, &mut |line| eprintln!("{line}"))
            .map_err(|e| e.to_string())?;
        write_new_dir(&out, &files)?;
        eprintln!("wrote {} files to {}", files.len(), out.display());
        return Ok(());
    }
    let pred_dir = match predictions {
        Some(p) => p,
        None => latest_predictions(&protocol)?,
    };
    let pred_files = read_dir(&pred_dir)?;
    if mode == "verify" {
        let r = predict::verify(&pred_files);
        println!(
            "{}: {} files checked, manifest SHA-256 {}",
            pred_dir.display(),
            r.files_checked,
            r.manifest_sha256.as_deref().unwrap_or("-")
        );
        for p in &r.problems {
            println!("  {p}");
        }
        return if r.ok {
            println!("ok: every frozen file matches its manifest");
            Ok(())
        } else {
            Err("the frozen predictions do not match their manifest".into())
        };
    }
    let frozen = Frozen::load(&pred_files).map_err(|e| format!("{}: {e}", pred_dir.display()))?;
    match mode {
        "drift" => {
            let current = netlist.map(|n| read(Path::new(&n))).transpose()?;
            let d = predict::drift(&frozen, current.as_deref()).map_err(|e| e.to_string())?;
            print!("{}", d.summary());
            Ok(())
        }
        "session" => {
            let dir = PathBuf::from(target.ok_or("--session needs a directory")?);
            write_new_dir(&dir, &session::session_templates(&frozen))?;
            eprintln!(
                "wrote the session templates to {}; see SESSION.txt",
                dir.display()
            );
            Ok(())
        }
        "simulate" => {
            let dir = PathBuf::from(target.ok_or("--simulate needs a directory")?);
            sim.truth = overrides.clone();
            sim.netlist_text = truth_netlist.map(|n| read(Path::new(&n))).transpose()?;
            sim.date = now_utc().chars().take(10).collect();
            let files = simulate::simulate(&frozen, &sim).map_err(|e| e.to_string())?;
            write_new_dir(&dir, &files)?;
            eprintln!(
                "wrote a synthetic session ({} files) to {}",
                files.len(),
                dir.display()
            );
            Ok(())
        }
        _ => {
            let dir =
                target.ok_or_else(|| format!("validate needs a measurement directory\n{USAGE}"))?;
            let files = read_dir(Path::new(&dir))?;
            let mut opts = ValidateOptions {
                extra_overrides: overrides.clone(),
                anchor_driver: anchor,
                ..ValidateOptions::default()
            };
            if let Some(n) = max_evals {
                opts.max_evaluations = n;
            }
            let report = session::validate(&frozen, &files, &opts, &mut |line| eprintln!("{line}"))
                .map_err(|e| e.to_string())?;
            let doc = format!("{:#}\n", report.to_json());
            if let Some(f) = &out {
                std::fs::write(f, &doc).map_err(|e| format!("{f}: {e}"))?;
            }
            let text = if json_out { doc } else { report.summary() };
            std::io::stdout()
                .write_all(text.as_bytes())
                .map_err(|e| e.to_string())
        }
    }
}
