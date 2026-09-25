//! `acoustilab score`: error metrics and preference scores of a netlist's
//! drum response against a target (docs/targets.md).

use acoustilab::params::Overrides;
use acoustilab::targets::curve::Curve;
use acoustilab::targets::import::import_target_csv;
use acoustilab::targets::metrics::{evaluate, Options, Report, Response, Stats};
use acoustilab::targets::target::{self, Target};
use acoustilab::targets::{fixture, Flag};
use acoustilab::Circuit;
use std::fmt::Write as _;

struct Args {
    netlist: String,
    target: String,
    probe: Option<String>,
    fixture: Option<String>,
    smoothing: Option<u32>,
    json: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut it = args.iter();
    let mut netlist = None;
    let mut a = Args {
        netlist: String::new(),
        target: String::new(),
        probe: None,
        fixture: None,
        smoothing: None,
        json: false,
    };
    let mut target = None;
    while let Some(x) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match x.as_str() {
            "--target" => target = Some(value("--target")?),
            "--probe" => a.probe = Some(value("--probe")?),
            "--fixture" => a.fixture = Some(value("--fixture")?),
            "--smoothing" => {
                let v = value("--smoothing")?;
                let n = v.strip_prefix("1/").unwrap_or(&v);
                a.smoothing = match n {
                    "none" => None,
                    _ => Some(
                        n.parse()
                            .map_err(|_| format!("--smoothing takes N or 1/N, got '{v}'"))?,
                    ),
                };
            }
            "--json" => a.json = true,
            o if o.starts_with("--") => return Err(format!("unknown option '{o}'")),
            p if netlist.is_none() => netlist = Some(p.to_string()),
            p => return Err(format!("unexpected argument '{p}'")),
        }
    }
    a.netlist = netlist.ok_or("score needs a netlist")?;
    a.target = target.ok_or("score needs --target NAME or --target FILE.csv")?;
    Ok(a)
}

fn load_target(spec: &str) -> Result<Target, String> {
    if spec.to_ascii_lowercase().ends_with(".csv") {
        let text = std::fs::read_to_string(spec).map_err(|e| format!("{spec}: {e}"))?;
        let stem = std::path::Path::new(spec)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported");
        return import_target_csv(&text, None, Some(stem)).map_err(|e| format!("{spec}: {e}"));
    }
    target::find(spec).cloned().ok_or_else(|| {
        format!("no bundled target '{spec}' (acoustilab score --list names them; a CSV file must end in .csv)")
    })
}

/// The probe to score: `--probe`, else the netlist's `ui.primary_probe`,
/// else its only pressure probe on an ear's drum reference point, else its
/// only pressure probe.
fn choose_probe(text: &str, c: &Circuit, given: Option<&str>) -> Result<String, String> {
    let pressure: Vec<&str> = c
        .probes
        .iter()
        .filter(|p| p.is_pressure)
        .map(|p| p.id.as_str())
        .collect();
    if let Some(p) = given {
        return if pressure.contains(&p) {
            Ok(p.to_string())
        } else {
            Err(format!(
                "'{p}' is not a pressure probe of this netlist (pressure probes: {})",
                pressure.join(", ")
            ))
        };
    }
    let ui = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v["ui"]["primary_probe"].as_str().map(str::to_string));
    if let Some(p) = ui.filter(|p| pressure.contains(&p.as_str())) {
        return Ok(p);
    }
    let drp: Vec<&str> = pressure
        .iter()
        .copied()
        .filter(|p| fixture::for_probe(c, p).is_some())
        .collect();
    match (drp.as_slice(), pressure.as_slice()) {
        ([p], _) | ([], [p]) => Ok(p.to_string()),
        _ => Err(format!(
            "choose a probe with --probe (pressure probes: {})",
            pressure.join(", ")
        )),
    }
}

pub fn run(args: &[String], overrides: &Overrides) -> Result<String, String> {
    if args.first().map(String::as_str) == Some("--list") {
        let mut s = String::new();
        for t in target::bundled() {
            let _ = writeln!(
                s,
                "{}\t{}\t{}\t{}",
                t.name, t.fixture, t.provenance.licence, t.label
            );
        }
        return Ok(s);
    }
    let a = parse(args)?;
    let text = std::fs::read_to_string(&a.netlist).map_err(|e| format!("{}: {e}", a.netlist))?;
    let c = Circuit::from_json_with(&text, overrides).map_err(|e| e.to_string())?;
    let target = load_target(&a.target)?;
    let probe = choose_probe(&text, &c, a.probe.as_deref())?;
    let fixture = a
        .fixture
        .clone()
        .or_else(|| fixture::for_probe(&c, &probe).map(|f| f.id.clone()));
    let r = c.solve().map_err(|e| e.to_string())?;
    let p = r.probe(&probe).ok_or("probe vanished")?;
    let resp = Response {
        left: Curve::new(r.freqs_hz.clone(), p.spl_db()).map_err(|e| e.to_string())?,
        right: None,
        fixture,
        label: probe.clone(),
        drive: Some(r.meta.drive.label.clone()),
    };
    let opts = Options {
        smoothing: a.smoothing,
        ..Options::default()
    };
    let rep = evaluate(&resp, &target, &opts).map_err(|e| e.to_string())?;
    Ok(if a.json {
        format!("{:#}\n", rep.to_json())
    } else {
        text_report(&rep)
    })
}

/// "31.6 Hz", "200 Hz", "7.94 kHz", "10 kHz".
fn hz(f: f64) -> String {
    let trim = |s: String| s.trim_end_matches('0').trim_end_matches('.').to_string();
    if f >= 1000.0 {
        format!("{} kHz", trim(format!("{:.2}", f / 1000.0)))
    } else {
        format!("{} Hz", trim(format!("{f:.1}")))
    }
}

fn stats_line(name: &str, s: &Option<Stats>) -> String {
    let Some(s) = s else {
        return format!("  {name}: no data\n");
    };
    let (a, b) = s.used_hz.unwrap_or((f64::NAN, f64::NAN));
    let mut line = format!(
        "  {name} (used {}-{}{}): RMS {:.2} dB, mean {:+.2} dB, MAE {:.2} dB",
        hz(a),
        hz(b),
        if s.partial { ", partial" } else { "" },
        s.rms,
        s.mean,
        s.mae
    );
    if let (Some(sd), Some(b)) = (s.sd, s.slope) {
        let _ = write!(
            line,
            ", SD {sd:.2} dB, |slope| {:.3} dB per ln f ({:+.2} dB/oct)",
            b.abs(),
            b * std::f64::consts::LN_2
        );
    }
    line + "\n"
}

fn flags(f: &[Flag]) -> String {
    f.iter().map(|f| f.code).collect::<Vec<_>>().join(", ")
}

fn text_report(r: &Report) -> String {
    let mut s = String::new();
    let t = &r.target;
    let _ = writeln!(s, "target:   {} [{}]", t.name, t.label);
    let _ = writeln!(
        s,
        "          fixture {}, licence {}, {}",
        t.fixture,
        t.provenance.licence,
        t.provenance.class.as_str()
    );
    let _ = writeln!(
        s,
        "response: {} on {}; drive {}",
        r.response_label,
        r.response_fixture
            .as_deref()
            .unwrap_or("an unspecified fixture"),
        r.drive.as_deref().unwrap_or("not stated")
    );
    let _ = writeln!(
        s,
        "          reference level {:.2} dB at the normalisation ({})",
        r.response_offset_db,
        r.options.normalisation.to_json()
    );
    let _ = writeln!(s, "error metrics on the 1/12-octave grid:");
    s += &stats_line("20 Hz-10 kHz", &r.main);
    s += &stats_line("10-20 kHz (reported separately)", &r.above_10k);
    for (b, st) in acoustilab::targets::metrics::RMS_BANDS.iter().zip(&r.bands) {
        s += &stats_line(&format!("{}-{}", hz(b.0), hz(b.1)), st);
    }
    let pct = |m: &acoustilab::targets::metrics::MaskResult| {
        m.percent().map_or("no data".into(), |p| {
            format!("{p:.0} % of {} points within", m.n)
        })
    };
    let _ = writeln!(s, "  ITU-R BS.708 mask, 100 Hz-16 kHz: {}", pct(&r.bs708));
    let _ = writeln!(
        s,
        "  preference band, 20 Hz-20 kHz: {}",
        pct(&r.band_compliance)
    );
    let _ = writeln!(s, "preference scores:");
    for sc in &r.scores {
        let _ = writeln!(
            s,
            "  {:<18} {:>7}{}  {}",
            sc.model.id,
            sc.score.map_or("-".into(), |v| format!("{v:.1}")),
            if sc.greyed { " (greyed)" } else { "" },
            flags(&sc.flags)
        );
    }
    let _ = writeln!(s, "flags:");
    for f in &r.flags {
        let _ = writeln!(s, "  {}: {}", f.code, f.message);
    }
    s
}
