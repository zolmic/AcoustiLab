//! `ir`, `poles` and `isolation` subcommands.

use acoustilab::isolation::{self, IsolationOptions};
use acoustilab::params::{Overrides, Parametric};
use acoustilab::time::report::{self, ImpulseRequest, PoleRequest};
use acoustilab::time::wav::float_wav;
use acoustilab::Circuit;
use std::io::Write;

pub const USAGE: &str = "  acoustilab ir <netlist.json> [--probe ID] [--fs 48000] [--n 8192] [--phase min|mixed]
                 [--pre SAMPLES] [--align] [--wav FILE] [--wav-raw] [--csv] [--out FILE] [--no-check]
                                                        impulse, step, ETC, minimum phase and excess phase
  acoustilab poles <netlist.json> [--probe ID] [--order 30] [--attribute] [--json] [--out FILE]
                                                        vector fit: poles, zeros, Q and fit error
  acoustilab isolation <netlist.json> [--probe ID] [--entrance NODE] [--csv] [--out FILE]
                                                        passive insertion loss at the drum and bleed";

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn emit(out: Option<&str>, text: &str) -> Result<(), String> {
    match out {
        Some(f) => std::fs::write(f, text).map_err(|e| format!("{f}: {e}")),
        None => std::io::stdout()
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string()),
    }
}

/// Minimal flag reader: `--name value` pairs and bare switches.
struct Flags<'a> {
    args: &'a [String],
    used: Vec<bool>,
}

impl<'a> Flags<'a> {
    fn new(args: &'a [String]) -> Self {
        Flags {
            args,
            used: vec![false; args.len()],
        }
    }

    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        for i in 0..self.args.len() {
            if self.args[i] == name {
                let v = self
                    .args
                    .get(i + 1)
                    .ok_or_else(|| format!("{name} needs a value"))?;
                self.used[i] = true;
                self.used[i + 1] = true;
                return Ok(Some(v.clone()));
            }
        }
        Ok(None)
    }

    fn number<T: std::str::FromStr>(&mut self, name: &str) -> Result<Option<T>, String> {
        self.value(name)?
            .map(|v| {
                v.parse::<T>()
                    .map_err(|_| format!("{name}: bad number '{v}'"))
            })
            .transpose()
    }

    fn switch(&mut self, name: &str) -> bool {
        match self.args.iter().position(|a| a == name) {
            Some(i) => {
                self.used[i] = true;
                true
            }
            None => false,
        }
    }

    fn finish(self) -> Result<(), String> {
        match self.args.iter().zip(&self.used).find(|(_, u)| !**u) {
            Some((a, _)) => Err(format!("unknown option '{a}'")),
            None => Ok(()),
        }
    }
}

fn load(path: &str, overrides: &Overrides) -> Result<(Parametric, Circuit), String> {
    let p = Parametric::parse(&read(path)?).map_err(|e| e.to_string())?;
    let c = Circuit::from_parametric(&p, overrides).map_err(|e| e.to_string())?;
    Ok((p, c))
}

pub fn ir(path: &str, args: &[String], overrides: &Overrides) -> Result<(), String> {
    let mut f = Flags::new(args);
    let mut req = ImpulseRequest {
        probe: f.value("--probe")?,
        ..Default::default()
    };
    if let Some(fs) = f.number("--fs")? {
        req.uniform.fs_hz = fs;
    }
    if let Some(n) = f.number("--n")? {
        req.uniform.n = n;
    }
    req.pre = f.number("--pre")?;
    req.align_delay = f.switch("--align");
    let phase = f.value("--phase")?;
    let wav = f.value("--wav")?;
    let raw = f.switch("--wav-raw");
    let csv = f.switch("--csv");
    let out = f.value("--out")?;
    req.length_check = !f.switch("--no-check");
    f.finish()?;
    let (p, c) = load(path, overrides)?;
    let probe = match &req.probe {
        Some(id) => id.clone(),
        None => report::default_probe(&p, &c).map_err(|e| e.to_string())?,
    };
    let r = report::impulses(&c, &probe, &req).map_err(|e| e.to_string())?;
    let mode = match phase.as_deref() {
        None => r.decision.mode,
        Some("min") | Some("minimum") => "minimum",
        Some("mixed") => "mixed",
        Some(o) => return Err(format!("--phase '{o}' must be min or mixed")),
    };
    if let Some(file) = wav {
        let h = if mode == "minimum" {
            &r.minimum.h
        } else {
            &r.mixed.h
        };
        let peak = h.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let gain = if raw || peak == 0.0 { 1.0 } else { 1.0 / peak };
        let scaled: Vec<f64> = h.iter().map(|v| v * gain).collect();
        let bytes = float_wav(&[&scaled], r.response.fs_hz.round() as u32)?;
        std::fs::write(&file, bytes).map_err(|e| format!("{file}: {e}"))?;
        eprintln!(
            "wrote {file}: {mode}-phase IR of '{probe}', {} samples at {} Hz, first sample at t = {} s, gain {gain:e} ({})",
            h.len(),
            r.response.fs_hz,
            r.mixed.t0_s,
            if gain == 1.0 { "raw values" } else { "normalised to peak 1" }
        );
    }
    let text = if csv {
        let mut s = String::from("t_s,ir,ir_min_phase,step,etc_dB\n");
        for i in 0..r.mixed.h.len() {
            s += &format!(
                "{},{},{},{},{}\n",
                r.mixed.t0_s + i as f64 / r.response.fs_hz,
                r.mixed.h[i],
                r.minimum.h[i],
                r.mixed.step[i],
                r.mixed.etc_db[i]
            );
        }
        s
    } else {
        format!("{:#}\n", r.to_json(true))
    };
    emit(out.as_deref(), &text)
}

pub fn poles(path: &str, args: &[String], overrides: &Overrides) -> Result<(), String> {
    let mut f = Flags::new(args);
    let mut req = PoleRequest {
        probe: f.value("--probe")?,
        ..Default::default()
    };
    if let Some(o) = f.number("--order")? {
        req.fit.vf.order = o;
    }
    req.attribute = f.switch("--attribute");
    let json = f.switch("--json");
    let out = f.value("--out")?;
    f.finish()?;
    let (p, c) = load(path, overrides)?;
    let probe = match &req.probe {
        Some(id) => id.clone(),
        None => report::default_probe(&p, &c).map_err(|e| e.to_string())?,
    };
    let (fit, v) =
        report::poles_report(&p, overrides, &c, &probe, &req).map_err(|e| e.to_string())?;
    if json {
        return emit(out.as_deref(), &format!("{v:#}\n"));
    }
    let e = &fit.report.error;
    let mut s = format!(
        "probe {probe}: order {} over {:.1}-{:.1} Hz, error rms {:.2e} relative, max {:.4} dB, {:.3} deg\n",
        fit.report.order,
        fit.band().0,
        fit.band().1,
        e.rms_relative,
        e.max_db,
        e.max_deg
    );
    s += "poles (in band):\n      f_Hz         Q   weight_dB  resonant   t60_s\n";
    for r in fit.poles.iter().filter(|r| r.in_band) {
        s += &format!(
            "{:10.2} {:>9} {:11.1} {:>9} {:9.4}\n",
            r.f_hz,
            r.q.map_or("real".to_string(), |q| format!("{q:.2}")),
            r.weight_db,
            if r.resonant { "yes" } else { "" },
            r.t60_s
        );
    }
    s += "zeros (in band):\n      f_Hz         Q   half-plane\n";
    for z in fit.zeros.iter().filter(|z| z.in_band) {
        s += &format!(
            "{:10.2} {:>9} {:>12}\n",
            z.f_hz,
            z.q.map_or("real".to_string(), |q| format!("{q:.2}")),
            if z.rhp { "right" } else { "left" }
        );
    }
    let chk = &v["ir_length"];
    s += &format!(
        "impulse length (E46, N/(2 fs) >= 2.93 Q/f): N = {} at {} Hz {}; recommended N = {}\n",
        chk["n"],
        chk["fs_Hz"],
        if chk["covered"].as_bool() == Some(true) {
            "covers every resonant pole"
        } else {
            "is too short"
        },
        chk["recommended_n"]
    );
    if let Some(a) = v["attribution"]["poles"].as_array() {
        s += "attribution (d ln f / d ln p):\n";
        for pa in a {
            s += &format!("  {:.1} Hz:", pa["f_Hz"].as_f64().unwrap_or(0.0));
            for q in pa["parameters"].as_array().into_iter().flatten() {
                s += &format!(
                    " {} {:+.3} ({})",
                    q["parameter"].as_str().unwrap_or(""),
                    q["dlnf_dlnp"].as_f64().unwrap_or(0.0),
                    q["elements"]
                        .as_array()
                        .map(|e| e
                            .iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                );
            }
            s.push('\n');
        }
    }
    emit(out.as_deref(), &s)
}

pub fn isolation(path: &str, args: &[String], overrides: &Overrides) -> Result<(), String> {
    let mut f = Flags::new(args);
    let opts = IsolationOptions {
        drum_probe: f.value("--probe")?,
        entrance_node: f.value("--entrance")?,
        paths: true,
    };
    let csv = f.switch("--csv");
    let out = f.value("--out")?;
    f.finish()?;
    let p = Parametric::parse(&read(path)?).map_err(|e| e.to_string())?;
    let iso = isolation::insertion_loss(&p, overrides, &opts).map_err(|e| e.to_string())?;
    let text = if csv {
        let mut s = String::from("frequency_Hz,insertion_loss_dB,p_occluded_spl_dB,p_open_spl_dB");
        for pr in &iso.paths {
            s += &format!(",{}_re_open_dB", pr.element);
        }
        s.push('\n');
        let levels: Vec<Vec<f64>> = iso.paths.iter().map(|pr| iso.path_levels_db(pr)).collect();
        for (i, fr) in iso.freqs_hz.iter().enumerate() {
            s += &format!(
                "{fr},{},{},{}",
                iso.insertion_loss_db[i],
                acoustilab::drive::spl_db(iso.p_occluded[i]),
                acoustilab::drive::spl_db(iso.p_open[i])
            );
            for l in &levels {
                s += &format!(",{}", l[i]);
            }
            s.push('\n');
        }
        s
    } else {
        let mut v = iso.to_json();
        v["bleed"] = match isolation::bleed(&p, overrides, &[0.3, 1.0]) {
            Ok(b) => b.to_json(),
            Err(e) => serde_json::json!({"error": e.to_string()}),
        };
        format!("{v:#}\n")
    };
    emit(out.as_deref(), &text)
}
