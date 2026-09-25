//! The virtual rig: synthetic measurements generated from a netlist with
//! known "true" parameter values, written with a sidecar as a real rig
//! would, so that identification can be exercised end to end before a
//! measurement rig exists. Every curve it makes is marked
//! `provenance.origin = "virtual_rig"` and carries its settings and true
//! parameters in the sidecar's `virtual_rig` block.
//!
//! Noise model, per frequency f on the measurement grid, with
//! x = ln(f/f_first)/ln(f_last/f_first) ∈ [0, 1]:
//!
//! * Seating k of N (N = `seatings`) measures
//!   H_k = H·10^((d_k(f) + n_k)/20)·exp(j(φ_k − 2πf·τ_k)), where n_k and φ_k
//!   are independent normal deviates per point (`level_dB`, `phase_deg`),
//!   τ_k is a normal delay per seating (`repositioning_delay_us`), and
//!   d_k(f) = σ_r·[g/√2 + Σ_{j=1..3} (b_j·cos jπx + c_j·sin jπx)/√6] is a
//!   smooth level change per seating with standard deviation σ_r
//!   (`repositioning_dB`) at every f (g, b_j, c_j standard normal).
//! * The seatings are averaged: `complex` (vector mean), `magnitude` (RMS
//!   of the magnitudes, phase of the vector mean) or `db` (mean level,
//!   phase of the vector mean). A spread of delays makes the complex mean
//!   lose treble: its expected magnitude is |H|·exp(−(2πfσ_τ)²/2).
//! * Systematic errors, drawn once per curve: a sensor calibration offset
//!   (`microphone_offset_dB`, normal) and slope per decade from 1 kHz
//!   (`microphone_slope_dB_per_decade`, normal), and a smooth coupler
//!   error c(f) = σ_c·Σ_{j=1..4}(b_j·cos jπx + c_j·sin jπx)/2 with RMS σ_c
//!   (`coupler_dB`).
//!
//! Impedance curves get only the per-point noise and the averaging; pressure
//! curves get everything; displacement and velocity curves get the noise,
//! the averaging and the sensor calibration errors. All deviates come from
//! one [`Rng`] seeded by `seed`, drawn in a fixed order, so a seed always
//! gives the same curve.
//!
//! The sidecar's uncertainty budget states the noise settings (the per-
//! seating terms, the calibration standard deviations as a table when a
//! slope is set, the coupler RMS), unless the spec supplies its own budget.
//! A rig without noise states no budget, so a fit falls back to its
//! default uncertainties rather than trusting a zero one.

use super::rng::Rng;
use super::{parse_overrides, spec_err, FitError};
use crate::circuit::Circuit;
use crate::io::curve::{exchange_grid, EXCHANGE_POINTS_PER_OCTAVE};
use crate::io::sidecar::{Averaging, Origin, Profile, Provenance, Uncertainty};
use crate::io::{Curve, Quantity, Sidecar};
use crate::params::{Overrides, Parametric};
use crate::C64;
use serde_json::{json, Map, Value};
use std::f64::consts::PI;

/// Noise settings of the virtual rig (module documentation). All levels are
/// standard deviations.
#[derive(Debug, Clone, PartialEq)]
pub struct Noise {
    pub seed: u64,
    pub level_db: f64,
    pub phase_deg: f64,
    pub seatings: u32,
    pub averaging: Averaging,
    pub repositioning_db: f64,
    pub repositioning_delay_us: f64,
    pub microphone_offset_db: f64,
    pub microphone_slope_db_per_decade: f64,
    pub coupler_db: f64,
}

impl Default for Noise {
    fn default() -> Self {
        Noise {
            seed: 1,
            level_db: 0.0,
            phase_deg: 0.0,
            seatings: 1,
            averaging: Averaging::None,
            repositioning_db: 0.0,
            repositioning_delay_us: 0.0,
            microphone_offset_db: 0.0,
            microphone_slope_db_per_decade: 0.0,
            coupler_db: 0.0,
        }
    }
}

impl Noise {
    fn is_zero(&self) -> bool {
        [
            self.level_db,
            self.phase_deg,
            self.repositioning_db,
            self.repositioning_delay_us,
            self.microphone_offset_db,
            self.microphone_slope_db_per_decade,
            self.coupler_db,
        ]
        .iter()
        .all(|x| *x == 0.0)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "seed": self.seed,
            "level_dB": self.level_db,
            "phase_deg": self.phase_deg,
            "seatings": self.seatings,
            "averaging": self.averaging.name(),
            "repositioning_dB": self.repositioning_db,
            "repositioning_delay_us": self.repositioning_delay_us,
            "microphone_offset_dB": self.microphone_offset_db,
            "microphone_slope_dB_per_decade": self.microphone_slope_db_per_decade,
            "coupler_dB": self.coupler_db,
        })
    }

    pub fn from_json(v: &Value) -> Result<Noise, FitError> {
        let o = v
            .as_object()
            .ok_or_else(|| spec_err("'noise' must be an object"))?;
        const KEYS: [&str; 10] = [
            "seed",
            "level_dB",
            "phase_deg",
            "seatings",
            "averaging",
            "repositioning_dB",
            "repositioning_delay_us",
            "microphone_offset_dB",
            "microphone_slope_dB_per_decade",
            "coupler_dB",
        ];
        if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(spec_err(format!(
                "noise: unknown key '{k}' (known: {})",
                KEYS.join(", ")
            )));
        }
        let mut n = Noise::default();
        let sd = |k: &str| -> Result<f64, FitError> {
            match o.get(k) {
                None => Ok(0.0),
                Some(v) => v
                    .as_f64()
                    .filter(|x| x.is_finite() && *x >= 0.0)
                    .ok_or_else(|| spec_err(format!("noise '{k}' must be a non-negative number"))),
            }
        };
        n.level_db = sd("level_dB")?;
        n.phase_deg = sd("phase_deg")?;
        n.repositioning_db = sd("repositioning_dB")?;
        n.repositioning_delay_us = sd("repositioning_delay_us")?;
        n.microphone_offset_db = sd("microphone_offset_dB")?;
        n.microphone_slope_db_per_decade = sd("microphone_slope_dB_per_decade")?;
        n.coupler_db = sd("coupler_dB")?;
        if let Some(s) = o.get("seed") {
            n.seed = s
                .as_u64()
                .ok_or_else(|| spec_err("noise 'seed' must be a non-negative integer"))?;
        }
        if let Some(s) = o.get("seatings") {
            n.seatings = s
                .as_u64()
                .filter(|x| *x >= 1 && *x <= 10_000)
                .ok_or_else(|| spec_err("noise 'seatings' must be an integer from 1 to 10000"))?
                as u32;
        }
        n.averaging = match o.get("averaging").map(|a| a.as_str()) {
            None => {
                if n.seatings > 1 {
                    Averaging::Complex
                } else {
                    Averaging::None
                }
            }
            Some(Some(a)) => Averaging::parse(a).ok_or_else(|| {
                spec_err(format!(
                    "noise: unknown averaging '{a}' (none, magnitude, db, complex)"
                ))
            })?,
            Some(None) => return Err(spec_err("noise 'averaging' must be a string")),
        };
        if n.seatings > 1 && n.averaging == Averaging::None {
            return Err(spec_err("several seatings need an averaging method"));
        }
        Ok(n)
    }
}

/// A virtual measurement (JSON form in docs/fitting.md).
#[derive(Debug, Clone)]
pub struct RigSpec {
    /// Probe to measure.
    pub probe: String,
    /// The "true" parameter values (and the measurement condition).
    pub overrides: Overrides,
    /// Measurement frequencies (default: the exchange grid, 10 Hz to 20 kHz
    /// at 48 points per octave).
    pub freqs: Vec<f64>,
    pub noise: Noise,
    /// Sidecar fields to copy (fixture, ear simulator, device, ...).
    pub sidecar: Sidecar,
    /// Stated uncertainty budget; default: derived from the noise.
    pub uncertainty: Option<Uncertainty>,
}

impl RigSpec {
    pub fn new(probe: &str) -> RigSpec {
        RigSpec {
            probe: probe.to_string(),
            overrides: Overrides::new(),
            freqs: exchange_grid(10.0, 20_000.0, EXCHANGE_POINTS_PER_OCTAVE),
            noise: Noise::default(),
            sidecar: Sidecar::default(),
            uncertainty: None,
        }
    }

    pub fn from_json(v: &Value) -> Result<RigSpec, FitError> {
        let o = v
            .as_object()
            .ok_or_else(|| spec_err("the rig specification must be a JSON object"))?;
        const KEYS: [&str; 9] = [
            "probe",
            "overrides",
            "frequencies_Hz",
            "f_min_Hz",
            "f_max_Hz",
            "points_per_octave",
            "noise",
            "sidecar",
            "uncertainty",
        ];
        if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(spec_err(format!(
                "rig: unknown key '{k}' (known: {})",
                KEYS.join(", ")
            )));
        }
        let probe = o
            .get("probe")
            .and_then(Value::as_str)
            .ok_or_else(|| spec_err("rig: 'probe' is required"))?;
        let mut s = RigSpec::new(probe);
        if let Some(ov) = o.get("overrides") {
            s.overrides = parse_overrides(ov, "overrides")?;
        }
        let num = |k: &str| -> Result<Option<f64>, FitError> {
            match o.get(k) {
                None => Ok(None),
                Some(v) => v
                    .as_f64()
                    .filter(|x| x.is_finite() && *x > 0.0)
                    .map(Some)
                    .ok_or_else(|| spec_err(format!("rig: '{k}' must be a positive number"))),
            }
        };
        let (fmin, fmax, ppo) = (
            num("f_min_Hz")?,
            num("f_max_Hz")?,
            num("points_per_octave")?,
        );
        if let Some(list) = o.get("frequencies_Hz") {
            if fmin.is_some() || fmax.is_some() || ppo.is_some() {
                return Err(spec_err(
                    "rig: give 'frequencies_Hz' or 'f_min_Hz'/'f_max_Hz'/'points_per_octave', not both",
                ));
            }
            s.freqs = list
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .map(|x| x.as_f64().filter(|f| f.is_finite() && *f > 0.0))
                        .collect::<Option<Vec<f64>>>()
                })
                .ok_or_else(|| spec_err("rig: 'frequencies_Hz' must hold positive numbers"))?;
        } else {
            let (lo, hi) = (fmin.unwrap_or(10.0), fmax.unwrap_or(20_000.0));
            if hi <= lo {
                return Err(spec_err("rig: f_max_Hz must exceed f_min_Hz"));
            }
            s.freqs = exchange_grid(lo, hi, ppo.unwrap_or(EXCHANGE_POINTS_PER_OCTAVE));
        }
        if s.freqs.len() < 2 {
            return Err(spec_err("rig: the grid has fewer than 2 frequencies"));
        }
        if let Some(n) = o.get("noise") {
            s.noise = Noise::from_json(n)?;
        }
        if let Some(sc) = o.get("sidecar") {
            let mut v = sc.clone();
            if let Some(m) = v.as_object_mut() {
                m.entry("schema")
                    .or_insert_with(|| json!(crate::io::sidecar::SIDECAR_SCHEMA));
            }
            s.sidecar = Sidecar::from_json(&v)?;
        }
        if let Some(u) = o.get("uncertainty") {
            let sc = Sidecar::from_json(&json!({
                "schema": crate::io::sidecar::SIDECAR_SCHEMA,
                "uncertainty": u
            }))?;
            s.uncertainty = sc.uncertainty;
        }
        Ok(s)
    }
}

/// Smooth random function of x ∈ [0, 1] with unit standard deviation at
/// every x: Σ_{j=1..m}(b_j·cos jπx + c_j·sin jπx)/√m.
struct Ripple {
    b: Vec<f64>,
    c: Vec<f64>,
}

impl Ripple {
    fn draw(rng: &mut Rng, m: usize) -> Ripple {
        let b = (0..m).map(|_| rng.normal()).collect();
        let c = (0..m).map(|_| rng.normal()).collect();
        Ripple { b, c }
    }

    fn at(&self, x: f64) -> f64 {
        let m = self.b.len() as f64;
        self.b
            .iter()
            .zip(&self.c)
            .enumerate()
            .map(|(j, (b, c))| {
                let a = (j + 1) as f64 * PI * x;
                b * a.cos() + c * a.sin()
            })
            .sum::<f64>()
            / m.sqrt()
    }
}

fn quantity_of(c: &Circuit, probe: &str) -> Result<Quantity, FitError> {
    super::probe_quantity(c, probe)
        .map(|(q, _, _)| q)
        .ok_or_else(|| spec_err(format!("rig: no probe '{probe}' in the netlist")))
}

/// Generates one virtual measurement (module documentation).
pub fn measure(p: &Parametric, spec: &RigSpec) -> Result<Curve, FitError> {
    let mut freqs = spec.freqs.clone();
    if freqs.iter().any(|f| !(f.is_finite() && *f > 0.0)) {
        return Err(spec_err("rig: frequencies must be positive and finite"));
    }
    freqs.sort_by(f64::total_cmp);
    freqs.dedup();
    if freqs.len() < 2 || freqs.len() > crate::grid::MAX_POINTS {
        return Err(spec_err(format!(
            "rig: the grid needs 2 to {} distinct frequencies, got {}",
            crate::grid::MAX_POINTS,
            freqs.len()
        )));
    }
    let mut c = Circuit::from_parametric(p, &spec.overrides)?;
    let q = quantity_of(&c, &spec.probe)?;
    c.freqs = freqs.clone();
    let r = c.solve()?;
    let truth: Vec<C64> = r
        .probe(&spec.probe)
        .ok_or_else(|| spec_err(format!("rig: no probe '{}'", spec.probe)))?
        .values
        .clone();
    let nz = &spec.noise;
    let mut rng = Rng::new(nz.seed);
    let (f0, f1) = (freqs[0], freqs[freqs.len() - 1]);
    let x = |f: f64| (f / f0).ln() / (f1 / f0).ln().max(1e-300);
    let pressure = q == Quantity::Pressure;
    let sensor = q != Quantity::Impedance;
    // Systematic errors, drawn first.
    let offset = if sensor {
        nz.microphone_offset_db * rng.normal()
    } else {
        0.0
    };
    let slope = if sensor {
        nz.microphone_slope_db_per_decade * rng.normal()
    } else {
        0.0
    };
    let coupler = Ripple::draw(&mut rng, 4);
    let n = nz.seatings.max(1) as usize;
    let mut seatings: Vec<Vec<C64>> = Vec::with_capacity(n);
    for _ in 0..n {
        let g = rng.normal();
        let rip = Ripple::draw(&mut rng, 3);
        let tau = nz.repositioning_delay_us * 1e-6 * rng.normal();
        let values = freqs
            .iter()
            .zip(&truth)
            .map(|(&f, h)| {
                let d = if pressure {
                    nz.repositioning_db * (g / 2f64.sqrt() + rip.at(x(f)) / 2f64.sqrt())
                } else {
                    0.0
                };
                let nl = nz.level_db * rng.normal();
                let np = nz.phase_deg.to_radians() * rng.normal();
                let delay = if pressure { -2.0 * PI * f * tau } else { 0.0 };
                h * 10f64.powf((d + nl) / 20.0) * C64::from_polar(1.0, np + delay)
            })
            .collect();
        seatings.push(values);
    }
    let averaging = if n == 1 {
        Averaging::None
    } else {
        nz.averaging
    };
    let averaged: Vec<C64> = (0..freqs.len())
        .map(|i| {
            let mean: C64 = seatings.iter().map(|s| s[i]).sum::<C64>() / n as f64;
            match averaging {
                Averaging::None | Averaging::Complex => mean,
                Averaging::Magnitude => {
                    let rms =
                        (seatings.iter().map(|s| s[i].norm_sqr()).sum::<f64>() / n as f64).sqrt();
                    C64::from_polar(rms, mean.arg())
                }
                Averaging::Db => {
                    let l = seatings.iter().map(|s| s[i].norm().log10()).sum::<f64>() / n as f64;
                    C64::from_polar(10f64.powf(l), mean.arg())
                }
            }
        })
        .collect();
    let measured: Vec<C64> = freqs
        .iter()
        .zip(&averaged)
        .map(|(&f, h)| {
            let mut e = 0.0;
            if sensor {
                e += offset + slope * (f / 1000.0).log10();
            }
            if pressure {
                e += nz.coupler_db * coupler.at(x(f));
            }
            h * 10f64.powf(e / 20.0)
        })
        .collect();
    let mut curve = Curve::from_complex(q, &freqs, &measured)?;
    // Sidecar: the template's fields over those of the solve.
    let solved = crate::io::sidecar::of_solve(&c, &r, q);
    let mut sc = spec.sidecar.clone();
    sc.quantity = Some(q);
    sc.calibrated = solved.calibrated;
    sc.drive = solved.drive;
    sc.source_impedance_ohm = solved.source_impedance_ohm;
    sc.seatings = Some(n as u32);
    sc.averaging = Some(averaging);
    sc.smoothing = Some(crate::io::sidecar::Smoothing::None);
    let title = p
        .doc
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("netlist");
    sc.provenance = Some(Provenance {
        origin: Some(Origin::VirtualRig),
        source: Some(format!("virtual rig: probe '{}' of '{title}'", spec.probe)),
        url: None,
        licence: None,
        tool: Some(crate::solve::ENGINE.to_string()),
    });
    let mut rig = Map::new();
    rig.insert("probe".into(), json!(spec.probe));
    rig.insert("noise".into(), nz.to_json());
    rig.insert("true_parameters".into(), r.meta.parameters.clone());
    sc.virtual_rig = Some(Value::Object(rig));
    sc.uncertainty = match &spec.uncertainty {
        Some(u) => Some(u.clone()),
        None if nz.is_zero() => None,
        None => Some(stated_budget(nz, q, &freqs)),
    };
    curve.sidecar = sc;
    Ok(curve)
}

/// The uncertainty budget that describes the rig's own noise settings.
fn stated_budget(nz: &Noise, q: Quantity, freqs: &[f64]) -> Uncertainty {
    let pos = |x: f64| (x > 0.0).then_some(Profile::Constant(x));
    let pressure = q == Quantity::Pressure;
    let sensor = q != Quantity::Impedance;
    let table = |g: &dyn Fn(f64) -> f64| Profile::Table {
        freqs: freqs.to_vec(),
        values: freqs.iter().map(|&f| g(f)).collect(),
    };
    let mic = if !sensor {
        None
    } else if nz.microphone_slope_db_per_decade > 0.0 {
        Some(table(&|f: f64| {
            (nz.microphone_offset_db.powi(2)
                + (nz.microphone_slope_db_per_decade * (f / 1000.0).log10()).powi(2))
            .sqrt()
        }))
    } else {
        pos(nz.microphone_offset_db)
    };
    let phase = if nz.repositioning_delay_us > 0.0 && pressure {
        Some(table(&|f: f64| {
            (nz.phase_deg.powi(2) + (360.0 * f * nz.repositioning_delay_us * 1e-6).powi(2)).sqrt()
        }))
    } else {
        pos(nz.phase_deg)
    };
    Uncertainty {
        coupler_db: if pressure { pos(nz.coupler_db) } else { None },
        microphone_calibration_db: mic,
        repositioning_db: if pressure {
            pos(nz.repositioning_db)
        } else {
            None
        },
        fixture_to_human_db: None,
        numerical_db: None,
        noise_db: pos(nz.level_db),
        phase_deg: phase,
    }
}
