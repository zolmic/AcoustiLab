//! Tornado charts (spec Sections 10 and 12): the change of one metric when
//! each parameter moves to the ends of its tolerance, by actual re-solves
//! (not linearised).
//!
//! The ends are the tolerance's 2σ points for `normal` (value ± tolerance)
//! and `lognormal` (value/(1 + rel) and value·(1 + rel)), and the full range
//! for `uniform` (value ± tolerance), clipped to the parameter's bounds.
//! Continuous parameters without a tolerance use ±10 % (`default_rel`) and
//! are marked `assumed`. Rows are ranked by the larger absolute change.
//!
//! Metrics ([`Metric`]):
//!
//! * `level`: the probe at a pinned frequency (exact solve): dB SPL for a
//!   pressure, |y| in the probe's unit otherwise (ohm for impedances);
//! * `band_mean`: the mean of 20·log10|y| over the grid frequencies in a
//!   band (dB SPL for pressures), a mean over log frequency on the usual
//!   log-spaced grid;
//! * `readout`: any scalar readout ([`super::readouts::SCALARS`]), such as
//!   `coupled_resonance_Hz` or `bass_extension_Hz`.
//!
//! A move that changes the netlist's structure (an `enabled` condition
//! crossed) is still a valid design and is evaluated; the row says so.

use super::readouts::{readouts_at, ReadoutOptions, SCALARS};
use super::{options_error, Design, Excluded, Point};
use crate::drive::{self, DriveInfo};
use crate::error::{Error, Result};
use crate::expr::PValue;
use crate::params::{Dist, Overrides, Tolerance};
use serde::{Deserialize, Serialize};

/// A scalar output of a design.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Metric {
    /// A probe at a pinned frequency.
    Level {
        probe: String,
        #[serde(rename = "f_Hz")]
        f_hz: f64,
    },
    /// Mean of 20·log10|y| over the grid frequencies in [f_min, f_max].
    BandMean {
        probe: String,
        #[serde(rename = "f_min_Hz")]
        f_min_hz: f64,
        #[serde(rename = "f_max_Hz")]
        f_max_hz: f64,
    },
    /// A scalar readout by name.
    Readout { name: String },
}

impl Metric {
    pub fn validate(&self) -> std::result::Result<(), String> {
        match self {
            Metric::Level { f_hz, .. } if !(f_hz.is_finite() && *f_hz > 0.0) => {
                Err(format!("'f_Hz' must be positive, got {f_hz}"))
            }
            Metric::BandMean {
                f_min_hz, f_max_hz, ..
            } if !(*f_min_hz > 0.0 && f_max_hz >= f_min_hz && f_max_hz.is_finite()) => Err(
                format!("need 0 < f_min_Hz <= f_max_Hz, got {f_min_hz} and {f_max_hz}"),
            ),
            Metric::Readout { name } if !SCALARS.contains(&name.as_str()) => Err(format!(
                "unknown readout '{name}' (known: {})",
                SCALARS.join(", ")
            )),
            _ => Ok(()),
        }
    }

    /// Human-readable name and unit.
    pub fn describe(&self, point: &Point) -> (String, String) {
        match self {
            Metric::Level { probe, f_hz } => {
                let unit = unit_of(point, probe);
                (format!("'{probe}' at {}", super::fmt_hz(*f_hz)), unit)
            }
            Metric::BandMean {
                probe,
                f_min_hz,
                f_max_hz,
            } => {
                let unit = if is_pressure(point, probe) {
                    "dB SPL".to_string()
                } else {
                    "dB".to_string()
                };
                (
                    format!(
                        "mean level of '{probe}' from {}",
                        super::fmt_hz_range(*f_min_hz, *f_max_hz)
                    ),
                    unit,
                )
            }
            Metric::Readout { name } => (format!("readout {name}"), readout_unit(name).into()),
        }
    }
}

/// Unit of a scalar readout, from its name's suffix.
pub fn readout_unit(name: &str) -> &'static str {
    if name.ends_with("_dB_per_V") {
        "dB SPL per V"
    } else if name.ends_with("_dB_per_mW") {
        "dB SPL per mW"
    } else if name.ends_with("_dB") {
        "dB SPL"
    } else if name.ends_with("_Hz") {
        "Hz"
    } else if name.ends_with("_ohm") {
        "ohm"
    } else {
        ""
    }
}

fn is_pressure(point: &Point, probe: &str) -> bool {
    point
        .circuit
        .probes
        .iter()
        .any(|p| p.id == probe && p.is_pressure)
}

fn unit_of(point: &Point, probe: &str) -> String {
    match point.circuit.probes.iter().find(|p| p.id == probe) {
        Some(p) if p.is_pressure => "dB SPL".into(),
        Some(p) => super::probe_unit(&point.circuit, p),
        None => String::new(),
    }
}

/// Evaluates a metric at one point.
pub fn evaluate(
    point: &mut Point,
    metric: &Metric,
    readout_opts: &ReadoutOptions,
    ui_probe: Option<&str>,
) -> Result<f64> {
    match metric {
        Metric::Level { probe, f_hz } => {
            let i = probe_index(point, probe)?;
            let r = point.solve_at_freqs(&[*f_hz])?;
            let y = r.probes[i].values[0];
            Ok(if r.probes[i].is_pressure {
                drive::spl_db(y)
            } else {
                y.norm()
            })
        }
        Metric::BandMean {
            probe,
            f_min_hz,
            f_max_hz,
        } => {
            let i = probe_index(point, probe)?;
            let band: Vec<f64> = point
                .circuit
                .freqs
                .iter()
                .copied()
                .filter(|f| f >= f_min_hz && f <= f_max_hz)
                .collect();
            if band.is_empty() {
                return Err(options_error(
                    "metric",
                    format!("no grid frequency between {f_min_hz} and {f_max_hz} Hz"),
                ));
            }
            let r = point.solve_at_freqs(&band)?;
            let p = &r.probes[i];
            let db: Vec<f64> = if p.is_pressure {
                p.spl_db()
            } else {
                p.values.iter().map(|y| 20.0 * y.norm().log10()).collect()
            };
            Ok(db.iter().sum::<f64>() / db.len() as f64)
        }
        Metric::Readout { name } => {
            let r = readouts_at(point, readout_opts, ui_probe)?;
            Ok(r.scalars().get(name).copied().flatten().unwrap_or(f64::NAN))
        }
    }
}

fn probe_index(point: &Point, probe: &str) -> Result<usize> {
    point
        .circuit
        .probes
        .iter()
        .position(|p| p.id == probe)
        .ok_or_else(|| Error::Probe {
            id: probe.to_string(),
            msg: "no such probe".into(),
        })
}

/// Options of [`tornado`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TornadoOptions {
    /// Default: the level of the response probe (the readouts' choice) at
    /// 1 kHz.
    pub metric: Option<Metric>,
    /// Default: every continuous parameter.
    pub parameters: Option<Vec<String>>,
    /// Relative change for parameters without a tolerance (default 0.1).
    pub default_rel: Option<f64>,
    /// Options of the readouts (response probe, rated impedance, ...).
    pub readouts: Option<ReadoutOptions>,
}

impl TornadoOptions {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(m) = &self.metric {
            m.validate()?;
        }
        if let Some(r) = self.default_rel {
            if !(r > 0.0 && r < 1.0) {
                return Err(format!("'default_rel' must be in (0, 1), got {r}"));
            }
        }
        if let Some(r) = &self.readouts {
            r.validate()?;
        }
        Ok(())
    }
}

/// One bar of a tornado chart.
#[derive(Debug, Clone, Serialize)]
pub struct TornadoRow {
    pub name: String,
    pub label: String,
    pub value: f64,
    pub low_value: f64,
    pub high_value: f64,
    /// `tolerance` or `assumed` (±default_rel, no tolerance declared).
    pub basis: &'static str,
    pub tolerance: Option<Tolerance>,
    /// An end was clipped to the parameter's bounds.
    pub clipped_low: bool,
    pub clipped_high: bool,
    pub metric_low: Option<f64>,
    pub metric_high: Option<f64>,
    pub delta_low: Option<f64>,
    pub delta_high: Option<f64>,
    /// max(|delta_low|, |delta_high|).
    pub effect: f64,
    /// Moving to an end changes the netlist's structure.
    pub topology_changed: bool,
    pub errors: Vec<String>,
}

/// A tornado chart.
#[derive(Debug, Clone, Serialize)]
pub struct Tornado {
    pub metric: Metric,
    pub description: String,
    pub unit: String,
    pub base: f64,
    /// Validity shading at a pinned frequency (level metrics).
    pub shading: Option<u8>,
    pub rows: Vec<TornadoRow>,
    pub excluded: Vec<Excluded>,
    pub drive: DriveInfo,
    pub hash: String,
    pub engine: &'static str,
}

/// The ends of a parameter's range for the tornado, before clipping.
pub fn tolerance_ends(value: f64, tol: Option<&Tolerance>, default_rel: f64) -> (f64, f64) {
    let (a, b) = match tol {
        Some(t) if t.dist == Dist::Lognormal => {
            let r = t.rel.unwrap_or(0.0);
            (value / (1.0 + r), value * (1.0 + r))
        }
        Some(t) => {
            let hw = t.half_width(value);
            (value - hw, value + hw)
        }
        None => (value * (1.0 - default_rel), value * (1.0 + default_rel)),
    };
    (a.min(b), a.max(b))
}

/// Builds a tornado chart of `design` (see the module documentation).
pub fn tornado(design: &Design, opts: &TornadoOptions) -> Result<Tornado> {
    opts.validate().map_err(|m| options_error("tornado", m))?;
    let ropts = opts.readouts.clone().unwrap_or_default();
    let ui = design.ui_primary_probe();
    let mut base = design.base_point()?;
    let metric = match &opts.metric {
        Some(m) => m.clone(),
        None => {
            let (i, _) = super::pressure_probe(&base, ropts.probe.as_deref(), ui.as_deref())?
                .ok_or_else(|| {
                    options_error(
                        "tornado",
                        "the netlist has no pressure probe; give a metric",
                    )
                })?;
            let probe = base.circuit.probes[i].id.clone();
            Metric::Level {
                probe,
                f_hz: 1000.0,
            }
        }
    };
    let (description, unit) = metric.describe(&base);
    let base_value = evaluate(&mut base, &metric, &ropts, ui.as_deref())?;
    if !base_value.is_finite() {
        return Err(options_error(
            "tornado",
            format!("{description} is undefined for the base design; choose another metric"),
        ));
    }
    let meta = base.solve_at_freqs(&[1000.0])?;
    let shading = match &metric {
        Metric::Level { f_hz, .. } => Some(meta.shading.band(*f_hz)),
        _ => None,
    };
    let default_rel = opts.default_rel.unwrap_or(0.1);
    let mut rows = Vec::new();
    let mut excluded = Vec::new();
    for def in design.selected(opts.parameters.as_deref())? {
        let value = match design.continuous_value(def) {
            Ok(v) => v,
            Err(reason) => {
                excluded.push(Excluded {
                    name: def.name.clone(),
                    reason,
                });
                continue;
            }
        };
        let (lo, hi) = tolerance_ends(value, def.tolerance.as_ref(), default_rel);
        let (min, max) = def.bounds();
        let clip = |x: f64| {
            let y = min.map_or(x, |m| x.max(m));
            max.map_or(y, |m| y.min(m))
        };
        let (low, high) = (clip(lo), clip(hi));
        if low == value && high == value {
            excluded.push(Excluded {
                name: def.name.clone(),
                reason: if value == 0.0 && def.tolerance.is_none() {
                    "value is 0 and it has no tolerance: a relative change is empty".into()
                } else {
                    "its bounds leave no room to move".into()
                },
            });
            continue;
        }
        let mut row = TornadoRow {
            name: def.name.clone(),
            label: def.label.clone().unwrap_or_else(|| def.name.clone()),
            value,
            low_value: low,
            high_value: high,
            basis: if def.tolerance.is_some() {
                "tolerance"
            } else {
                "assumed"
            },
            tolerance: def.tolerance.clone(),
            clipped_low: low != lo,
            clipped_high: high != hi,
            metric_low: None,
            metric_high: None,
            delta_low: None,
            delta_high: None,
            effect: 0.0,
            topology_changed: false,
            errors: Vec::new(),
        };
        for (x, is_low) in [(low, true), (high, false)] {
            if x == value {
                if is_low {
                    row.metric_low = Some(base_value);
                } else {
                    row.metric_high = Some(base_value);
                }
                continue;
            }
            let mut ov = Overrides::new();
            ov.insert(def.name.clone(), PValue::Num(x));
            let m = design.point(&ov).and_then(|mut p| {
                if base.topology_change(&p).is_some() {
                    row.topology_changed = true;
                }
                evaluate(&mut p, &metric, &ropts, ui.as_deref())
            });
            match m {
                Ok(v) if v.is_finite() => {
                    if is_low {
                        row.metric_low = Some(v);
                    } else {
                        row.metric_high = Some(v);
                    }
                }
                Ok(_) => row
                    .errors
                    .push(format!("the metric is undefined at {} = {x}", def.name)),
                Err(e) => row.errors.push(format!("at {} = {x}: {e}", def.name)),
            }
        }
        row.delta_low = row.metric_low.map(|v| v - base_value);
        row.delta_high = row.metric_high.map(|v| v - base_value);
        row.effect = [row.delta_low, row.delta_high]
            .iter()
            .flatten()
            .fold(0.0f64, |m, d| m.max(d.abs()));
        rows.push(row);
    }
    rows.sort_by(|a, b| b.effect.total_cmp(&a.effect));
    Ok(Tornado {
        metric,
        description,
        unit,
        base: base_value,
        shading,
        rows,
        excluded,
        drive: meta.meta.drive,
        hash: base.hash(),
        engine: crate::solve::ENGINE,
    })
}
