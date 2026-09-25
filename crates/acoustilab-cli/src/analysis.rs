//! Design-analysis subcommands (docs/analysis.md): `sens`, `tornado`,
//! `explain`, `readouts` and `mc`.

use acoustilab::analysis::explain::{self, ExplainOptions};
use acoustilab::analysis::mc::{self, PlanSpec, RunOptions};
use acoustilab::analysis::readouts::{self, ReadoutOptions, Readouts};
use acoustilab::analysis::sensitivity::{self, SensitivityOptions};
use acoustilab::analysis::tornado::{self, Metric, TornadoOptions};
use acoustilab::analysis::{fmt_hz, fmt_hz_range, Design};
use acoustilab::params::Overrides;
use std::io::Write;

/// Pretty JSON text of a serialisable result.
macro_rules! pretty {
    ($v:expr) => {
        serde_json::to_string_pretty($v)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string())
    };
}

/// Options common to the subcommands.
#[derive(Default)]
struct Args {
    path: String,
    probes: Vec<String>,
    params: Vec<String>,
    json: bool,
    csv: bool,
    out: Option<String>,
    num: Vec<(String, f64)>,
    text: Vec<(String, String)>,
}

impl Args {
    fn num(&self, key: &str) -> Option<f64> {
        self.num
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| *v)
    }
    fn text(&self, key: &str) -> Option<String> {
        self.text
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
}

fn parse(cmd: &str, args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.iter();
    let value = |it: &mut std::slice::Iter<String>, opt: &str| -> Result<String, String> {
        it.next()
            .cloned()
            .ok_or_else(|| format!("{opt} needs a value"))
    };
    let numeric = [
        "--step",
        "--f",
        "--top",
        "--threshold",
        "--rated",
        "--re",
        "-n",
        "--seed",
        "--chunk",
    ];
    while let Some(arg) = it.next() {
        let s = arg.as_str();
        match s {
            "--json" => a.json = true,
            "--csv" => a.csv = true,
            "--probe" => a.probes.push(value(&mut it, s)?),
            "--param" => a.params.push(value(&mut it, s)?),
            "--out" => a.out = Some(value(&mut it, s)?),
            "--impedance" | "--readout" | "--driver" => {
                let v = value(&mut it, s)?;
                a.text.push((s.to_string(), v));
            }
            "--band" => {
                let lo = value(&mut it, s)?;
                let hi = value(&mut it, s)?;
                for (k, v) in [("--band-lo", lo), ("--band-hi", hi)] {
                    let x = v
                        .parse::<f64>()
                        .map_err(|_| format!("--band needs two numbers, got '{v}'"))?;
                    a.num.push((k.to_string(), x));
                }
            }
            _ if numeric.contains(&s) => {
                let v = value(&mut it, s)?;
                let x = v
                    .parse::<f64>()
                    .map_err(|_| format!("{s} needs a number, got '{v}'"))?;
                a.num.push((s.to_string(), x));
            }
            _ if a.path.is_empty() && !s.starts_with('-') => a.path = arg.clone(),
            _ => return Err(format!("{cmd}: unknown option '{s}'")),
        }
    }
    if a.path.is_empty() {
        return Err(format!("{cmd} needs a netlist file"));
    }
    if a.json && a.csv {
        return Err("give --json or --csv, not both".into());
    }
    Ok(a)
}

fn count(a: &Args, key: &str) -> Result<Option<usize>, String> {
    match a.num(key) {
        None => Ok(None),
        Some(x) if x >= 0.0 && x.fract() == 0.0 => Ok(Some(x as usize)),
        Some(x) => Err(format!("{key} needs a non-negative integer, got {x}")),
    }
}

fn emit(a: &Args, text: String) -> Result<(), String> {
    match &a.out {
        Some(f) => std::fs::write(f, text).map_err(|e| format!("{f}: {e}")),
        None => std::io::stdout()
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string()),
    }
}

fn some<T: Clone>(v: &[T]) -> Option<Vec<T>> {
    (!v.is_empty()).then(|| v.to_vec())
}

/// Runs an analysis subcommand; `args` follow the subcommand name.
pub fn run(cmd: &str, args: &[String], overrides: &Overrides) -> Result<(), String> {
    let a = parse(cmd, args)?;
    let text = std::fs::read_to_string(&a.path).map_err(|e| format!("{}: {e}", a.path))?;
    let design = Design::parse(&text, overrides).map_err(|e| e.to_string())?;
    let out = match cmd {
        "sens" => sens(&design, &a)?,
        "tornado" => tornado_cmd(&design, &a)?,
        "explain" => explain_cmd(&design, &a)?,
        "readouts" => readouts_cmd(&design, &a)?,
        "mc" => mc_cmd(&design, &a)?,
        _ => return Err(format!("unknown analysis '{cmd}'")),
    };
    emit(&a, out)
}

fn sens(design: &Design, a: &Args) -> Result<String, String> {
    let opts = SensitivityOptions {
        parameters: some(&a.params),
        probes: some(&a.probes),
        step: a.num("--step"),
    };
    let j = sensitivity::jacobian(design, &opts).map_err(|e| e.to_string())?;
    if a.json {
        return pretty!(&j);
    }
    if a.csv {
        let mut s = String::from("frequency_Hz");
        for p in &j.parameters {
            for probe in &j.probes {
                s += &format!(",{}@{probe}_dB_per_pct", p.name);
            }
        }
        s.push('\n');
        for (k, f) in j.freqs_hz.iter().enumerate() {
            s += &format!("{f}");
            for p in &j.parameters {
                for row in &p.db_per_pct {
                    s += &format!(",{}", row[k]);
                }
            }
            s.push('\n');
        }
        return Ok(s);
    }
    let mut s = format!(
        "sensitivities by {} (step {} in ln p), in dB per percent\ndrive: {}\n",
        j.method, j.step, j.drive.label
    );
    let credible: Vec<usize> = (0..j.freqs_hz.len())
        .filter(|&k| j.shading.band(j.freqs_hz[k]) == 0)
        .collect();
    if let (Some(&lo), Some(&hi)) = (credible.first(), credible.last()) {
        s += &format!(
            "largest |dB/%| in the credible band {}\n",
            fmt_hz_range(j.freqs_hz[lo], j.freqs_hz[hi])
        );
    }
    for (pj, probe) in j.probes.iter().enumerate() {
        s += &format!(
            "\n{probe}\n  {:<26} {:<9} {:>10}  at\n",
            "parameter", "scheme", "dB/%"
        );
        for p in &j.parameters {
            let (k, v) = credible
                .iter()
                .map(|&k| (k, p.db_per_pct[pj][k]))
                .filter(|(_, v)| v.is_finite())
                .fold((usize::MAX, 0.0f64), |acc, x| {
                    if x.1.abs() > acc.1.abs() {
                        x
                    } else {
                        acc
                    }
                });
            let at = if k == usize::MAX {
                "-".to_string()
            } else {
                fmt_hz(j.freqs_hz[k])
            };
            s += &format!(
                "  {:<26} {:<9} {:>+10.5}  {at}\n",
                p.name,
                format!("{:?}", p.scheme).to_lowercase(),
                v
            );
        }
    }
    for p in &j.parameters {
        for n in p.note.iter().chain(&p.warnings) {
            s += &format!("note: {}: {n}\n", p.name);
        }
    }
    for e in &j.excluded {
        s += &format!("excluded: {}: {}\n", e.name, e.reason);
    }
    Ok(s)
}

fn tornado_cmd(design: &Design, a: &Args) -> Result<String, String> {
    let probe = a.probes.first().cloned();
    let metric = if let Some(name) = a.text("--readout") {
        Some(Metric::Readout { name })
    } else if let (Some(lo), Some(hi)) = (a.num("--band-lo"), a.num("--band-hi")) {
        Some(Metric::BandMean {
            probe: probe.clone().ok_or("--band needs --probe")?,
            f_min_hz: lo,
            f_max_hz: hi,
        })
    } else if let Some(f) = a.num("--f") {
        Some(Metric::Level {
            probe: probe.clone().ok_or("--f needs --probe")?,
            f_hz: f,
        })
    } else {
        probe.map(|p| Metric::Level {
            probe: p,
            f_hz: 1000.0,
        })
    };
    let opts = TornadoOptions {
        metric,
        parameters: some(&a.params),
        default_rel: None,
        readouts: None,
    };
    let t = tornado::tornado(design, &opts).map_err(|e| e.to_string())?;
    if a.json {
        return pretty!(&t);
    }
    let mut s =
        format!(
        "tornado of {} [{}]: base {:.4}\ndrive: {}\n  {:<26} {:>12} {:>10} {:>12} {:>10}  basis\n",
        t.description, t.unit, t.base, t.drive.label, "parameter", "low", "change", "high", "change"
    );
    let fmt = |x: Option<f64>| x.map_or("-".to_string(), |v| format!("{v:+.4}"));
    for r in &t.rows {
        s += &format!(
            "  {:<26} {:>12.5} {:>10} {:>12.5} {:>10}  {}{}{}\n",
            r.name,
            r.low_value,
            fmt(r.delta_low),
            r.high_value,
            fmt(r.delta_high),
            r.basis,
            if r.clipped_low || r.clipped_high {
                ", clipped"
            } else {
                ""
            },
            if r.topology_changed {
                ", topology changes"
            } else {
                ""
            }
        );
        for e in &r.errors {
            s += &format!("    {e}\n");
        }
    }
    for e in &t.excluded {
        s += &format!("excluded: {}: {}\n", e.name, e.reason);
    }
    Ok(s)
}

fn explain_cmd(design: &Design, a: &Args) -> Result<String, String> {
    let opts = ExplainOptions {
        probe: a.probes.first().cloned(),
        top: count(a, "--top")?,
        step_pct: a.num("--step"),
        threshold_db: a.num("--threshold"),
        parameters: some(&a.params),
        driver: None,
    };
    let e = explain::explain(design, &opts).map_err(|e| e.to_string())?;
    if a.json {
        return pretty!(&e);
    }
    let mut s = format!(
        "changes of '{}' (credible band only; {} over each band; drive: {})\n",
        e.probe, e.statistic, e.drive.label
    );
    for x in &e.sentences {
        s += &format!("- {}\n", x.text);
    }
    if e.sentences.is_empty() {
        s += &format!(
            "- no parameter changes the level by more than {} dB in the credible band\n",
            e.threshold_db
        );
    }
    Ok(s)
}

fn readouts_text(r: &Readouts) -> String {
    let mut s = format!("drive: {}\n", r.drive.label);
    if let Some(z) = &r.impedance {
        s += &format!("impedance ({})\n", z.probe);
        s += &format!("  Re {:.4} ohm ({})\n", z.re, z.re_source);
        for p in &z.peaks {
            s += &format!(
                "  peak {:.4} ohm at {:.4} Hz (prominence {:.2} dB)\n",
                p.z, p.f_hz, p.prominence_db
            );
        }
        if let Some(q) = &z.q {
            s += &format!(
                "  in situ: fres {:.4} Hz, r0 {:.4}, f1 {:.4} Hz, f2 {:.4} Hz, Qms {:.4}, Qes {:.4}, Qts {:.4}\n",
                q.fres, q.r0, q.f1, q.f2, q.qms, q.qes, q.qts
            );
        }
        if let Some(v) = z.z_1khz {
            s += &format!("  |Z(1 kHz)| {v:.4} ohm\n");
        }
        if let Some(m) = &z.min_above_resonance {
            s += &format!(
                "  minimum above resonance {:.4} ohm at {:.4} Hz{}\n",
                m.value,
                m.f_hz,
                if z.min_at_sweep_end {
                    " (sweep end)"
                } else {
                    ""
                }
            );
        }
        if let Some(c) = &z.rated_check {
            s += &format!(
                "  rated-impedance check ({} ohm, limit {:.2} ohm): {} (minimum {:.4} ohm at {:.4} Hz)\n",
                c.rated_ohm,
                c.limit_ohm,
                if c.pass { "pass" } else { "FAIL" },
                c.z_min_ohm,
                c.z_min_hz
            );
        }
        for n in &z.notes {
            s += &format!("  note: {n}\n");
        }
    }
    for d in &r.drivers {
        s += &format!(
            "driver '{}' ({}): D0 fs {:.4} Hz, Qms {:.4}, Qes {:.4}, Qts {:.4}\n",
            d.element, d.model, d.d0.fs, d.d0.qms, d.d0.qes, d.d0.qts
        );
        if let Some(q) = &d.free_air {
            s += &format!(
                "  free-air impedance: fres {:.4} Hz, Qms {:.4}, Qes {:.4}, Qts {:.4}\n",
                q.fres, q.qms, q.qes, q.qts
            );
        }
    }
    if let Some(x) = &r.response {
        s += &format!("response ({}, {})\n", x.probe, x.probe_source);
        s += &format!(
            "  level {:.3} dB SPL at 500 Hz, {:.3} dB SPL at 1 kHz\n",
            x.level_500, x.level_1k
        );
        for v in &x.sensitivity {
            s += &format!(
                "  sensitivity at {}: {:.3} dB/V{}\n",
                fmt_hz(v.f_hz),
                v.db_per_v,
                v.db_per_mw.map_or(String::new(), |m| format!(
                    ", {m:.3} dB/mW into {} ohm rated",
                    v.rated_ohm.unwrap_or(f64::NAN)
                ))
            );
        }
        match &x.bass_extension {
            Some(b) => s += &format!("  bass extension (-3 dB re 500 Hz): {:.4} Hz\n", b.f_hz),
            None => s += "  bass extension: none in the sweep\n",
        }
        if let Some(c) = &x.coupled_resonance {
            s += &format!(
                "  coupled resonance {:.4} Hz (driver '{}', prominence {:.2} dB{})\n",
                c.f_hz,
                c.driver,
                c.prominence_db,
                if c.robust { "" } else { ", not robust" }
            );
        }
        for n in &x.notes {
            s += &format!("  note: {n}\n");
        }
    }
    for n in &r.notes {
        s += &format!("note: {n}\n");
    }
    s
}

fn readouts_cmd(design: &Design, a: &Args) -> Result<String, String> {
    let opts = ReadoutOptions {
        probe: a.probes.first().cloned(),
        impedance: a.text("--impedance"),
        rated_ohm: a.num("--rated"),
        re_ohm: a.num("--re"),
        driver: a.text("--driver"),
    };
    let r = readouts::readouts(design, &opts).map_err(|e| e.to_string())?;
    if a.json {
        return pretty!(&r);
    }
    Ok(readouts_text(&r))
}

fn mc_cmd(design: &Design, a: &Args) -> Result<String, String> {
    let n = count(a, "-n")?.unwrap_or(200);
    let seed = match a.num("--seed") {
        None => 1,
        Some(x) if x >= 0.0 && x.fract() == 0.0 && x < 9.007_199_254_740_992e15 => x as u64,
        Some(x) => return Err(format!("--seed needs a non-negative integer, got {x}")),
    };
    let spec = PlanSpec::Lhs {
        n,
        seed,
        parameters: some(&a.params),
    };
    let plan = mc::plan(design, &spec).map_err(|e| e.to_string())?;
    let opts = RunOptions {
        probes: some(&a.probes),
        metrics: None,
        readouts: None,
    };
    let chunk = count(a, "--chunk")?.unwrap_or(20);
    let mut progress = |done: usize, total: usize| {
        eprint!("\rmc: {done}/{total} runs");
        true
    };
    let runs = mc::run_chunked(design, &plan.samples, &opts, chunk, &mut progress)
        .map_err(|e| e.to_string())?;
    eprintln!();
    if a.csv {
        return Ok(mc::to_csv(&plan.parameters, &runs.samples, &runs.engine));
    }
    let env = mc::envelope(&runs.freqs_hz, &runs.samples);
    if a.json {
        return pretty!(&serde_json::json!({"plan": plan, "envelope": env}));
    }
    let mut s = format!(
        "Latin hypercube, {} runs, seed {seed}, {} parameters ({} values clipped); {} failed\n",
        plan.samples.len(),
        plan.parameters.len(),
        plan.clipped,
        env.failed
    );
    for d in &plan.distributions {
        s += &format!(
            "  {:<24} {:?} around {} +/- {}\n",
            d.name, d.dist, d.nominal, d.half_width
        );
    }
    s += &format!(
        "  {:<28} {:>12} {:>12} {:>12} {:>6}\n",
        "metric", "median", "5 %", "95 %", "n"
    );
    for (m, st) in &env.metrics {
        if st.n > 0 {
            s += &format!(
                "  {m:<28} {:>12.5} {:>12.5} {:>12.5} {:>6}\n",
                st.median, st.p5, st.p95, st.n
            );
        }
    }
    for r in runs.samples.iter().filter(|r| !r.ok) {
        s += &format!(
            "  run {} failed: {}\n",
            r.index,
            r.error.as_deref().unwrap_or("")
        );
    }
    Ok(s)
}
