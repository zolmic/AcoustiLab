//! `acoustilab` command-line runner.
//!
//! ```text
//! acoustilab solve <netlist.json> [--csv] [--out FILE] [--set NAME=VALUE]...
//! acoustilab check <netlist.json> [--set NAME=VALUE]...
//! acoustilab params <netlist.json> [--set NAME=VALUE]...
//! acoustilab fit <netlist.json> --curve PROBE=FILE[:SIDECAR] --param NAME ...
//! acoustilab measure <netlist.json> --probe ID --out FILE [--noise-db X --seed S]
//! acoustilab convert IN OUT
//! acoustilab types
//! acoustilab help | --help | -h
//! acoustilab version | --version | -V
//! ```

mod fit_cmd;

use acoustilab::expr::PValue;
use acoustilab::params::{Overrides, Parametric};
use acoustilab::Circuit;
use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "usage:
  acoustilab solve <netlist.json> [--csv] [--out FILE]   solve and print results (JSON by default)
  acoustilab check <netlist.json>                        parse and validate only
  acoustilab params <netlist.json>                       list the parameters and their values
    solve, check and params take --set NAME=VALUE (repeatable) to override a parameter
  acoustilab types                                       list element types
  acoustilab help                                        print this message
  acoustilab version                                     print the engine version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn load(path: &str, overrides: &Overrides) -> Result<Circuit, String> {
    Circuit::from_json_with(&read(path)?, overrides).map_err(|e| e.to_string())
}

/// Parses `NAME=VALUE`: a number, `true`/`false`, or else a string.
fn parse_set(arg: &str) -> Result<(String, PValue), String> {
    let (name, value) = arg
        .split_once('=')
        .ok_or_else(|| format!("--set needs NAME=VALUE, got '{arg}'"))?;
    let v = match value {
        "true" => PValue::Bool(true),
        "false" => PValue::Bool(false),
        _ => value
            .parse::<f64>()
            .map(PValue::Num)
            .unwrap_or_else(|_| PValue::Str(value.to_string())),
    };
    Ok((name.trim().to_string(), v))
}

/// Splits `--set` options from the other arguments.
fn split_sets(args: &[String]) -> Result<(Vec<String>, Overrides), String> {
    let mut rest = Vec::new();
    let mut ov = Overrides::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--set" {
            let (k, v) = parse_set(it.next().ok_or("--set needs NAME=VALUE")?)?;
            ov.insert(k, v);
        } else if let Some(kv) = a.strip_prefix("--set=") {
            let (k, v) = parse_set(kv)?;
            ov.insert(k, v);
        } else {
            rest.push(a.clone());
        }
    }
    Ok((rest, ov))
}

fn run(args: &[String]) -> Result<(), String> {
    let (args, overrides) = split_sets(args)?;
    let args = args.as_slice();
    let Some(cmd) = args.first() else {
        return Err(USAGE.into());
    };
    match cmd.as_str() {
        "help" | "--help" | "-h" => {
            println!("{USAGE}\n{}", fit_cmd::USAGE);
            Ok(())
        }
        "fit" | "measure" | "convert" => fit_cmd::run(args, &overrides),
        "version" | "--version" | "-V" => {
            println!("{}", acoustilab::solve::ENGINE);
            Ok(())
        }
        "types" => {
            for t in acoustilab::elements::known_types() {
                println!("{t}");
            }
            Ok(())
        }
        "check" => {
            let path = args.get(1).ok_or(USAGE)?;
            let c = load(path, &overrides)?;
            println!(
                "ok: {} nodes, {} elements, {} unknowns, {} probes, {} frequencies, level {}",
                c.nodes.len(),
                c.elements.len(),
                c.dim,
                c.probes.len(),
                c.freqs.len(),
                c.level
            );
            Ok(())
        }
        "params" => {
            let path = args.get(1).ok_or(USAGE)?;
            let p = Parametric::parse(&read(path)?).map_err(|e| e.to_string())?;
            let values = p.values(&overrides).map_err(|e| e.to_string())?;
            for (d, (_, v)) in p.defs.iter().zip(&values) {
                let unit = d.display_unit().unwrap_or_default();
                let kind = if d.is_derived() { " (derived)" } else { "" };
                println!("{} = {v} {unit}{kind}", d.name);
            }
            Ok(())
        }
        "solve" => {
            let path = args.get(1).ok_or(USAGE)?;
            let mut csv = false;
            let mut out: Option<String> = None;
            let mut it = args[2..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--csv" => csv = true,
                    "--out" => out = Some(it.next().ok_or("--out needs a file")?.clone()),
                    other => return Err(format!("unknown option '{other}'\n{USAGE}")),
                }
            }
            let c = load(path, &overrides)?;
            let r = c.solve().map_err(|e| e.to_string())?;
            let text = if csv {
                to_csv(&r)
            } else {
                format!("{:#}\n", r.to_json())
            };
            match out {
                Some(f) => std::fs::write(&f, text).map_err(|e| format!("{f}: {e}")),
                None => std::io::stdout()
                    .write_all(text.as_bytes())
                    .map_err(|e| e.to_string()),
            }
        }
        _ => Err(USAGE.into()),
    }
}

fn to_csv(r: &acoustilab::SolveResult) -> String {
    let mut s = String::from("frequency_Hz");
    for p in &r.probes {
        s += &format!(",{0}_re,{0}_im,{0}_mag,{0}_phase_deg", p.id);
        if p.is_pressure {
            s += &format!(",{}_spl_dB", p.id);
        }
    }
    s.push('\n');
    for (k, f) in r.freqs_hz.iter().enumerate() {
        s += &format!("{f}");
        for p in &r.probes {
            let v = p.values[k];
            s += &format!(",{},{},{},{}", v.re, v.im, v.norm(), v.arg().to_degrees());
            if p.is_pressure {
                s += &format!(",{}", 20.0 * (v.norm() / acoustilab::air::P_REF).log10());
            }
        }
        s.push('\n');
    }
    s
}
