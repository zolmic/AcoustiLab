//! `acoustilab` command-line runner.
//!
//! ```text
//! acoustilab solve <netlist.json> [--csv] [--out FILE]
//! acoustilab check <netlist.json>
//! acoustilab types
//! ```

use acoustilab::Circuit;
use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "usage:
  acoustilab solve <netlist.json> [--csv] [--out FILE]   solve and print results (JSON by default)
  acoustilab check <netlist.json>                        parse and validate only
  acoustilab types                                       list element types";

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

fn load(path: &str) -> Result<Circuit, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Circuit::from_json(&text).map_err(|e| e.to_string())
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first() else {
        return Err(USAGE.into());
    };
    match cmd.as_str() {
        "types" => {
            for t in acoustilab::elements::known_types() {
                println!("{t}");
            }
            Ok(())
        }
        "check" => {
            let path = args.get(1).ok_or(USAGE)?;
            let c = load(path)?;
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
            let c = load(path)?;
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
