//! Synthetic sessions from the virtual rig ([`crate::fit::rig`]): every
//! file a protocol asks for, measured on a "true" device, so that the
//! pipeline of [`super::session`] runs end to end before a measurement rig
//! exists. Every curve is marked `provenance.origin = "virtual_rig"`, and a
//! session made of them is reported as synthetic.
//!
//! The true device is the frozen netlist (or another netlist, to put a
//! model-form error into the data) with the configuration's overrides and
//! [`SimulateOptions::truth`] on top. Seatings on a fixture differ as real
//! ones do: each seating k of a fixture draws its own value of every
//! parameter in [`SimulateOptions::seating_spread`], shared by all the
//! configurations measured in that seating (plugs are swapped without
//! lifting the cup), as x·exp(σ·z) with z standard normal. The rig then
//! adds its per-point noise. Pressure curves are written as FRD and
//! impedances as ZMA, as an analyser exports them, each with a sidecar
//! that states what the protocol asks.

use super::predict::Frozen;
use super::{netlist_on_grid, seating_stem, VResult, ValidationError};
use crate::expr::PValue;
use crate::fit::rig::{self, Noise, RigSpec};
use crate::fit::rng::Rng;
use crate::io::curve::exchange_grid;
use crate::io::text::{self, Format};
use crate::io::{Quantity, Sidecar};
use crate::params::Overrides;
use std::collections::BTreeMap;

/// Options of [`simulate`].
#[derive(Debug, Clone)]
pub struct SimulateOptions {
    /// True parameter values, on top of every configuration's overrides.
    pub truth: Overrides,
    /// The true device's netlist (default: the frozen one).
    pub netlist_text: Option<String>,
    /// Spread between seatings on a fixture: (parameter, σ of ln x).
    pub seating_spread: Vec<(String, f64)>,
    /// Per-point noise of every curve (its seed is replaced per file).
    pub noise: Noise,
    pub seed: u64,
    /// Measurements to leave out.
    pub omit: Vec<String>,
    /// Density of the measurement grid (exchange grid).
    pub points_per_octave: f64,
    /// Date written to the sidecars.
    pub date: String,
}

impl Default for SimulateOptions {
    fn default() -> Self {
        SimulateOptions {
            truth: Overrides::new(),
            netlist_text: None,
            seating_spread: vec![
                ("residual_leak_gap_mm".into(), 0.3),
                ("front_volume_factor".into(), 0.01),
            ],
            noise: Noise {
                level_db: 0.05,
                phase_deg: 0.3,
                ..Noise::default()
            },
            seed: 1,
            omit: Vec::new(),
            points_per_octave: 24.0,
            date: "2026-01-01".into(),
        }
    }
}

/// SplitMix64 step: decorrelates the seeds of the files.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn text_hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Writes a synthetic session (module documentation): curve files and
/// sidecars by name.
pub fn simulate(frozen: &Frozen, opts: &SimulateOptions) -> VResult<BTreeMap<String, String>> {
    let protocol = &frozen.protocol;
    let netlist = opts.netlist_text.as_deref().unwrap_or(&frozen.netlist_text);
    let mut out = BTreeMap::new();
    for c in &protocol.configurations {
        let (lo, hi) = (
            c.band.0.max(protocol.grid.f_min),
            c.band.1.min(protocol.grid.f_max),
        );
        let freqs = exchange_grid(lo, hi, opts.points_per_octave);
        let p = netlist_on_grid(netlist, &freqs)?;
        let fixture = c
            .sidecar
            .get("fixture")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let group = format!(
            "{fixture}|{}",
            c.sidecar
                .get("ear_simulator")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        );
        let on_fixture = fixture != "free air";
        for k in 1..=c.seatings {
            let mut truth = protocol.overrides_for(c);
            truth.extend(opts.truth.clone());
            if on_fixture {
                let mut rng = Rng::new(mix(opts.seed ^ mix(text_hash(&group)) ^ u64::from(k)));
                for (name, sd) in &opts.seating_spread {
                    let z = rng.normal();
                    let base = match truth.get(name) {
                        Some(v) => v.as_num(),
                        None => p
                            .values(&truth)?
                            .into_iter()
                            .find(|(n, _)| n == name)
                            .and_then(|(_, v)| v.as_num()),
                    }
                    .ok_or_else(|| {
                        ValidationError(format!("simulate: no numeric parameter '{name}'"))
                    })?;
                    truth.insert(name.clone(), PValue::Num(base * (sd * z).exp()));
                }
            }
            for m in &c.measurements {
                if opts.omit.contains(&m.id) {
                    continue;
                }
                let expected = &frozen.nominal[&m.id].sidecar;
                let mut spec = RigSpec::new(&m.probe);
                spec.overrides = truth.clone();
                spec.freqs = freqs.clone();
                spec.noise = opts.noise.clone();
                spec.noise.seed = mix(opts.seed ^ mix(text_hash(&m.id)) ^ mix(u64::from(k)));
                spec.noise.seatings = 1;
                let mut sc = Sidecar::default();
                for (key, v) in &c.sidecar {
                    let v = v.as_str().map(str::to_string);
                    match key.as_str() {
                        "fixture" => sc.fixture = v,
                        "ear_simulator" => sc.ear_simulator = v,
                        "pinna" => sc.pinna = v,
                        _ => {}
                    }
                }
                sc.reference_point = Some(m.reference_point.clone());
                sc.compensation = Some("none".into());
                sc.temperature_c = Some(23.0);
                sc.date = Some(opts.date.clone());
                sc.device = Some(format!("virtual reference cup (seed {})", opts.seed));
                spec.sidecar = sc;
                let curve = rig::measure(&p, &spec)?;
                let same = match (&curve.sidecar.drive, &expected.drive) {
                    (Some(a), Some(b)) => a.same_as(b),
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    return Err(ValidationError(format!(
                        "simulate: '{}' is driven differently from its frozen prediction",
                        m.id
                    )));
                }
                let (ext, fmt) = match curve.quantity {
                    Quantity::Impedance => ("zma", Format::Zma),
                    _ => ("frd", Format::Frd),
                };
                let name = format!("{}.{ext}", seating_stem(&m.id, k));
                out.insert(name.clone(), text::export(&curve, fmt)?);
                out.insert(format!("{name}.sidecar.json"), curve.sidecar.to_text());
            }
        }
    }
    Ok(out)
}
