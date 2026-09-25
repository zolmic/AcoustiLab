//! Explain sentences (spec Section 15, "Explain panel"; erratum E24).
//!
//! No cause-and-effect text is written by hand: every sentence is generated
//! from two solves of the design. For each continuous parameter the design
//! is re-solved with the parameter raised by `step_pct` (10 %; lowered by
//! the same fraction when raising would pass its maximum), and the change
//! of the chosen pressure probe, ΔdB(f) = L_new(f) − L_base(f), is read on
//! the grid:
//!
//! * only the **credible band** counts: grid frequencies unshaded (neither
//!   upper nor lower validity shading) in both solves;
//! * a parameter's effect is max |ΔdB| over the credible band; parameters
//!   are ranked by it and the top `top` (5) with an effect of at least
//!   `threshold_dB` (0.3 dB) get a sentence;
//! * a **band** is a run of consecutive credible grid frequencies where ΔdB
//!   stays beyond ±threshold with one sign. A sentence states at most two
//!   bands (the two with the largest |mean| × points), each with the
//!   **mean** ΔdB over its grid points ("on average") and its first and
//!   last grid frequencies; the structured data lists every band with its
//!   mean, its largest |ΔdB| and where that occurs;
//! * the coupled resonance ([`super::readouts`]) is mentioned only when it
//!   is robust and unshaded in both solves and the two values differ at
//!   three significant digits.
//!
//! Example (the template's front depth): "Raising driver-to-ear depth 10 %
//! lowers p_drp by 0.72 dB on average from 100 to 891 Hz and moves the
//! coupled resonance from 934 to 900 Hz." The numbers come from the solves.

use super::readouts::{self, CoupledResonance};
use super::{fmt_hz, fmt_hz_range, nan_vec, options_error, Design, Excluded, Point};
use crate::circuit::Probe;
use crate::drive::DriveInfo;
use crate::error::Result;
use crate::expr::PValue;
use crate::params::Overrides;
use crate::solve::SolveResult;
use serde::{Deserialize, Serialize};

/// Options of [`explain`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplainOptions {
    /// Pressure probe (default: `ui.primary_probe`, else the first pressure
    /// probe).
    pub probe: Option<String>,
    /// Number of sentences (default 5).
    pub top: Option<usize>,
    /// Relative change in percent (default 10).
    pub step_pct: Option<f64>,
    /// Smallest |ΔdB| that counts (default 0.3 dB).
    #[serde(rename = "threshold_dB")]
    pub threshold_db: Option<f64>,
    /// Parameters to consider (default: every continuous one).
    pub parameters: Option<Vec<String>>,
    /// Driver of the coupled resonance (default: the first driver).
    pub driver: Option<String>,
}

impl ExplainOptions {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(s) = self.step_pct {
            if !(s > 0.0 && s < 100.0) {
                return Err(format!("'step_pct' must be in (0, 100), got {s}"));
            }
        }
        if let Some(t) = self.threshold_db {
            if !(t > 0.0 && t.is_finite()) {
                return Err(format!("'threshold_dB' must be positive, got {t}"));
            }
        }
        Ok(())
    }
}

/// A run of credible grid frequencies where the change keeps one sign
/// beyond the threshold.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Band {
    #[serde(rename = "f_min_Hz")]
    pub f_min: f64,
    #[serde(rename = "f_max_Hz")]
    pub f_max: f64,
    /// `raises` or `lowers`.
    pub effect: &'static str,
    /// Mean ΔdB over the band's grid points.
    #[serde(rename = "mean_dB")]
    pub mean_db: f64,
    /// Largest |ΔdB| in the band and where it occurs.
    #[serde(rename = "max_abs_dB")]
    pub max_abs_db: f64,
    #[serde(rename = "at_Hz")]
    pub at_hz: f64,
    /// Grid indices of the first and last points.
    pub first: usize,
    pub last: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResonanceShift {
    #[serde(rename = "from_Hz")]
    pub from_hz: f64,
    #[serde(rename = "to_Hz")]
    pub to_hz: f64,
}

/// One generated sentence and the numbers behind it.
#[derive(Debug, Clone, Serialize)]
pub struct Sentence {
    pub text: String,
    pub parameter: String,
    pub label: String,
    /// `raise` or `lower`.
    pub direction: &'static str,
    pub from_value: f64,
    pub to_value: f64,
    /// Largest |ΔdB| over the credible band.
    #[serde(rename = "effect_dB")]
    pub effect_db: f64,
    pub bands: Vec<Band>,
    /// Indices into `bands` of the bands the text states.
    pub stated: Vec<usize>,
    pub resonance: Option<ResonanceShift>,
    /// ΔdB at every grid frequency (credible or not).
    #[serde(rename = "delta_dB", serialize_with = "nan_vec::serialize")]
    pub delta_db: Vec<f64>,
}

/// A parameter whose largest change stays below the threshold.
#[derive(Debug, Clone, Serialize)]
pub struct Quiet {
    pub name: String,
    #[serde(rename = "effect_dB")]
    pub effect_db: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Explanation {
    pub probe: String,
    pub step_pct: f64,
    #[serde(rename = "threshold_dB")]
    pub threshold_db: f64,
    /// The band statistic stated in the text.
    pub statistic: &'static str,
    #[serde(rename = "frequencies_Hz")]
    pub freqs_hz: Vec<f64>,
    /// Credible (unshaded) grid points of the base design.
    pub credible: Vec<bool>,
    pub base_resonance: Option<CoupledResonance>,
    pub sentences: Vec<Sentence>,
    /// Parameters ranked below the top or under the threshold.
    pub quiet: Vec<Quiet>,
    pub skipped: Vec<Excluded>,
    pub drive: DriveInfo,
    pub hash: String,
    pub engine: &'static str,
}

struct Solved {
    result: SolveResult,
    resonance: Option<CoupledResonance>,
}

fn solve(point: &mut Point, tap: &Option<(String, [Probe; 2])>) -> Result<Solved> {
    let extra: Vec<Probe> = tap.iter().flat_map(|(_, p)| p.iter().cloned()).collect();
    let (result, extras) = point.solve_with(&extra)?;
    let resonance = match tap {
        None => None,
        Some((id, [vp, ip])) => readouts::coupled_resonance(
            point,
            &result,
            (vp, &extras[0]),
            (ip, &extras[1]),
            id,
            &mut Vec::new(),
        )?,
    };
    Ok(Solved { result, resonance })
}

/// ΔdB magnitude with up to two significant digits ("1.8", "0.42", "12").
pub fn fmt_db(x: f64) -> String {
    let a = x.abs();
    if a >= 9.95 {
        format!("{a:.0}")
    } else if a >= 0.995 {
        format!("{a:.1}")
    } else {
        let d = (1 - a.log10().floor() as i32).max(2) as usize;
        format!("{a:.d$}")
    }
}

/// The bands of a ΔdB curve (see the module documentation).
pub fn bands(freqs: &[f64], delta: &[f64], credible: &[bool], threshold: f64) -> Vec<Band> {
    let sign = |k: usize| -> i8 {
        if !credible[k] || !delta[k].is_finite() {
            0
        } else if delta[k] > threshold {
            1
        } else if delta[k] < -threshold {
            -1
        } else {
            0
        }
    };
    let mut out = Vec::new();
    let mut start: Option<(usize, i8)> = None;
    let close = |s: usize, e: usize, sg: i8, out: &mut Vec<Band>| {
        let pts = &delta[s..=e];
        let mean = pts.iter().sum::<f64>() / pts.len() as f64;
        let (at, max) = (s..=e)
            .map(|k| (k, delta[k].abs()))
            .fold((s, 0.0), |acc, x| if x.1 > acc.1 { x } else { acc });
        out.push(Band {
            f_min: freqs[s],
            f_max: freqs[e],
            effect: if sg > 0 { "raises" } else { "lowers" },
            mean_db: mean,
            max_abs_db: max,
            at_hz: freqs[at],
            first: s,
            last: e,
        });
    };
    for k in 0..freqs.len() {
        let s = sign(k);
        if let Some((st, sg)) = start {
            if s == sg {
                continue;
            }
            close(st, k - 1, sg, &mut out);
            start = None;
        }
        if s != 0 {
            start = Some((k, s));
        }
    }
    if let Some((st, sg)) = start {
        close(st, freqs.len() - 1, sg, &mut out);
    }
    out
}

/// Lowercases a label's first letter for use inside a sentence when its
/// first word looks like an ordinary word (four or more letters, the rest
/// lowercase, or a few short nouns), so symbols such as "Qms" or "IEC" keep
/// their case.
fn lower_first(label: &str) -> String {
    let first_word = label.split_whitespace().next().unwrap_or("");
    let mut chars = first_word.chars();
    let rest_lower = chars.next().is_some_and(char::is_alphabetic)
        && chars.all(|c| c.is_lowercase() || c == '-');
    let short = ["Pad", "Cup", "Ear", "Air", "Gap", "Box", "Top", "Net"];
    if rest_lower && (first_word.chars().count() >= 4 || short.contains(&first_word)) {
        let mut c = label.chars();
        let f = c.next().expect("non-empty");
        f.to_lowercase().chain(c).collect()
    } else {
        label.to_string()
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn band_clause(b: &Band, object: &str) -> String {
    let span = if b.first == b.last {
        format!("at {}", fmt_hz(b.f_min))
    } else {
        format!("from {}", fmt_hz_range(b.f_min, b.f_max))
    };
    format!(
        "{} {object} by {} dB on average {span}",
        b.effect,
        fmt_db(b.mean_db)
    )
}

/// Generates the explain sentences of `design` (see the module
/// documentation).
pub fn explain(design: &Design, opts: &ExplainOptions) -> Result<Explanation> {
    opts.validate().map_err(|m| options_error("explain", m))?;
    let step = opts.step_pct.unwrap_or(10.0) / 100.0;
    let threshold = opts.threshold_db.unwrap_or(0.3);
    let top = opts.top.unwrap_or(5);
    let mut base = design.base_point()?;
    let (pi, _) = super::pressure_probe(
        &base,
        opts.probe.as_deref(),
        design.ui_primary_probe().as_deref(),
    )?
    .ok_or_else(|| options_error("explain", "the netlist has no pressure probe"))?;
    let probe = base.circuit.probes[pi].id.clone();
    let tap = readouts::driver_tap(&base, opts.driver.as_deref())?;
    let b = solve(&mut base, &tap)?;
    let freqs = b.result.freqs_hz.clone();
    let base_db = b.result.probes[pi].spl_db();
    let base_credible: Vec<bool> = freqs
        .iter()
        .map(|f| b.result.shading.band(*f) == 0)
        .collect();
    let resonance_ok = |r: &Option<CoupledResonance>| -> Option<f64> {
        r.as_ref()
            .filter(|r| r.robust && r.shading == 0)
            .map(|r| r.f_hz)
    };
    let mut ranked: Vec<(Sentence, f64)> = Vec::new();
    let mut skipped = Vec::new();
    for def in design.selected(opts.parameters.as_deref())? {
        let value = match design.continuous_value(def) {
            Ok(v) if v != 0.0 => v,
            Ok(_) => {
                skipped.push(Excluded {
                    name: def.name.clone(),
                    reason: "value is 0: a relative change is empty".into(),
                });
                continue;
            }
            Err(reason) => {
                skipped.push(Excluded {
                    name: def.name.clone(),
                    reason,
                });
                continue;
            }
        };
        let (min, max) = def.bounds();
        let up = value * (1.0 + step);
        let down = value * (1.0 - step);
        let fits = |x: f64| min.is_none_or(|m| x >= m) && max.is_none_or(|m| x <= m);
        let (direction, x) = if fits(up) {
            ("raise", up)
        } else if fits(down) {
            ("lower", down)
        } else {
            skipped.push(Excluded {
                name: def.name.clone(),
                reason: format!("its bounds do not allow a {} % change", step * 100.0),
            });
            continue;
        };
        let mut ov = Overrides::new();
        ov.insert(def.name.clone(), PValue::Num(x));
        let mut point = match design.point(&ov) {
            Ok(p) => p,
            Err(e) => {
                skipped.push(Excluded {
                    name: def.name.clone(),
                    reason: format!("the design fails at {x}: {e}"),
                });
                continue;
            }
        };
        if let Some(d) = base.topology_change(&point) {
            skipped.push(Excluded {
                name: def.name.clone(),
                reason: format!("moving it to {x} {d}; not explained as a smooth change"),
            });
            continue;
        }
        let n = match solve(&mut point, &tap) {
            Ok(n) if n.result.freqs_hz == freqs => n,
            Ok(_) => {
                skipped.push(Excluded {
                    name: def.name.clone(),
                    reason: "the change alters the frequency grid".into(),
                });
                continue;
            }
            Err(e) => {
                skipped.push(Excluded {
                    name: def.name.clone(),
                    reason: format!("the solve fails at {x}: {e}"),
                });
                continue;
            }
        };
        let new_db = n.result.probes[pi].spl_db();
        let delta: Vec<f64> = new_db.iter().zip(&base_db).map(|(a, b)| a - b).collect();
        let credible: Vec<bool> = freqs
            .iter()
            .zip(&base_credible)
            .map(|(f, c)| *c && n.result.shading.band(*f) == 0)
            .collect();
        let effect = delta
            .iter()
            .zip(&credible)
            .filter(|(d, c)| **c && d.is_finite())
            .fold(0.0f64, |m, (d, _)| m.max(d.abs()));
        let resonance = match (resonance_ok(&b.resonance), resonance_ok(&n.resonance)) {
            (Some(a), Some(c)) if fmt_hz(a) != fmt_hz(c) => Some(ResonanceShift {
                from_hz: a,
                to_hz: c,
            }),
            _ => None,
        };
        let label = def.label.clone().unwrap_or_else(|| def.name.clone());
        ranked.push((
            Sentence {
                text: String::new(),
                parameter: def.name.clone(),
                label,
                direction,
                from_value: value,
                to_value: x,
                effect_db: effect,
                bands: bands(&freqs, &delta, &credible, threshold),
                stated: Vec::new(),
                resonance,
                delta_db: delta,
            },
            effect,
        ));
    }
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut sentences = Vec::new();
    let mut quiet = Vec::new();
    for (mut s, effect) in ranked {
        if sentences.len() >= top || effect < threshold || s.bands.is_empty() {
            quiet.push(Quiet {
                name: s.parameter,
                effect_db: effect,
            });
            continue;
        }
        let mut order: Vec<usize> = (0..s.bands.len()).collect();
        order.sort_by(|&a, &b| {
            let w = |k: usize| {
                s.bands[k].mean_db.abs() * (s.bands[k].last - s.bands[k].first + 1) as f64
            };
            w(b).total_cmp(&w(a))
        });
        order.truncate(2);
        order.sort_unstable();
        let mut clauses: Vec<String> = order
            .iter()
            .enumerate()
            .map(|(n, &k)| band_clause(&s.bands[k], if n == 0 { &probe } else { "it" }))
            .collect();
        if let Some(r) = &s.resonance {
            clauses.push(format!(
                "moves the coupled resonance from {}",
                fmt_hz_range(r.from_hz, r.to_hz)
            ));
        }
        let body = match clauses.len() {
            1 => clauses[0].clone(),
            n => format!("{} and {}", clauses[..n - 1].join(", "), clauses[n - 1]),
        };
        let verb = if s.direction == "raise" {
            "raising"
        } else {
            "lowering"
        };
        s.text = capitalise(&format!(
            "{verb} {} {} % {body}.",
            lower_first(&s.label),
            (step * 100.0 * 1000.0).round() / 1000.0
        ));
        s.stated = order;
        sentences.push(s);
    }
    Ok(Explanation {
        probe,
        step_pct: step * 100.0,
        threshold_db: threshold,
        statistic: "mean",
        freqs_hz: freqs,
        credible: base_credible,
        base_resonance: b.resonance,
        sentences,
        quiet,
        skipped,
        drive: b.result.meta.drive.clone(),
        hash: base.hash(),
        engine: crate::solve::ENGINE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_split_on_sign_threshold_and_credibility() {
        let f: Vec<f64> = (0..10).map(|k| 100.0 * (k + 1) as f64).collect();
        let d = [0.1, 0.5, 0.6, -0.4, -0.5, 0.2, 0.7, 0.8, 0.9, 1.0];
        let c = [true, true, true, true, true, true, true, true, false, true];
        let b = bands(&f, &d, &c, 0.3);
        assert_eq!(b.len(), 4);
        assert_eq!((b[0].first, b[0].last, b[0].effect), (1, 2, "raises"));
        assert!((b[0].mean_db - 0.55).abs() < 1e-12);
        assert_eq!((b[1].first, b[1].last, b[1].effect), (3, 4, "lowers"));
        assert_eq!((b[2].first, b[2].last), (6, 7));
        assert_eq!((b[3].first, b[3].last), (9, 9));
        assert_eq!(fmt_db(1.84), "1.8");
        assert_eq!(fmt_db(-0.423), "0.42");
        assert_eq!(fmt_db(0.0423), "0.042");
        assert_eq!(fmt_db(12.3), "12");
        assert_eq!(lower_first("Rear cavity volume"), "rear cavity volume");
        assert_eq!(lower_first("IEC ear"), "IEC ear");
        assert_eq!(lower_first("Qms"), "Qms");
        assert_eq!(lower_first("Pad leak gap"), "pad leak gap");
        assert_eq!(lower_first("Driver-to-ear depth"), "driver-to-ear depth");
    }
}
