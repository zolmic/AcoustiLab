//! JSON API of the time-domain, rational-fit and isolation exports.
//!
//! Each function takes the netlist, the parameter overrides (`{"name":
//! value}`, "" for none) and an options object ("" for defaults) and
//! returns the engine's report (`acoustilab::time::report`,
//! `acoustilab::isolation`) or `{"error", "kind", ..}`. Unknown option keys
//! are rejected (kind `options`). Runtime is bounded: at most 16384 FFT
//! points, order 80, 64 perturbed parameters.
//!
//! * [`impulse_value`] options: `probe`, `fs_Hz` (48000), `n` (8192),
//!   `f_min_Hz`, `f_max_Hz`, `pre_samples` (n/32), `refine` (4), `extend`
//!   (8), `threshold_ms` (0.5), `ir_length_check` (true), `order` (30),
//!   `align_delay` (false), `spectrum` (true).
//! * [`vector_fit_value`] options: `probe`, `order` (30), `iterations`
//!   (12), `asymptote` (`zero` | `constant` | `linear`), `weighting`
//!   (`relative` | `uniform`), `f_min_Hz`, `f_max_Hz`, `q_min` (1),
//!   `weight_min_dB` (−20), `attribute` (false), `max_parameters` (32),
//!   `step` (0.01), `fs_Hz`, `n` (for the impulse-length check),
//!   `state_space` (false).
//! * [`isolation_value`] options: `drum_probe`, `entrance_node`, `paths`
//!   (true), `bleed` (true), `bleed_distances_m` ([0.3, 1]).

use crate::api::{error_value, parse_overrides};
use acoustilab::isolation::{self, IsolationOptions};
use acoustilab::params::Parametric;
use acoustilab::time::report::{self, ImpulseRequest, PoleRequest};
use acoustilab::time::vfit::{Asymptote, Weighting};
use acoustilab::Circuit;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

/// Largest FFT length a call may request.
pub const N_LIMIT: usize = 16384;
/// Largest fit order a call may request.
pub const ORDER_LIMIT: usize = 80;
/// Largest number of parameters an attribution may perturb.
pub const PARAMETER_LIMIT: usize = 64;

fn options_error(msg: impl Into<String>) -> Value {
    json!({"error": msg.into(), "kind": "options"})
}

/// An options object whose keys must all be used.
struct Opts {
    map: Map<String, Value>,
    used: BTreeSet<String>,
}

impl Opts {
    fn parse(text: &str) -> Result<Opts, Value> {
        if text.trim().is_empty() {
            return Ok(Opts {
                map: Map::new(),
                used: BTreeSet::new(),
            });
        }
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => Ok(Opts {
                map,
                used: BTreeSet::new(),
            }),
            Ok(_) => Err(options_error("options must be an object")),
            Err(e) => Err(options_error(format!("options JSON: {e}"))),
        }
    }

    fn get(&mut self, key: &str) -> Option<&Value> {
        let v = self.map.get(key).filter(|v| !v.is_null());
        if v.is_some() {
            self.used.insert(key.to_string());
        }
        v
    }

    fn f64(&mut self, key: &str) -> Result<Option<f64>, Value> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite())
                .map(Some)
                .ok_or_else(|| options_error(format!("'{key}' must be a finite number"))),
        }
    }

    fn usize(&mut self, key: &str) -> Result<Option<usize>, Value> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .map(|x| Some(x as usize))
                .ok_or_else(|| options_error(format!("'{key}' must be a non-negative integer"))),
        }
    }

    fn bool(&mut self, key: &str) -> Result<Option<bool>, Value> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_bool()
                .map(Some)
                .ok_or_else(|| options_error(format!("'{key}' must be true or false"))),
        }
    }

    fn string(&mut self, key: &str) -> Result<Option<String>, Value> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(|s| Some(s.to_string()))
                .ok_or_else(|| options_error(format!("'{key}' must be a string"))),
        }
    }

    fn finish(self) -> Result<(), Value> {
        match self.map.keys().find(|k| !self.used.contains(*k)) {
            Some(k) => Err(options_error(format!("unknown option '{k}'"))),
            None => Ok(()),
        }
    }
}

/// Parses the netlist and overrides and compiles the circuit.
fn load(
    netlist_json: &str,
    overrides_json: &str,
) -> Result<(Parametric, acoustilab::params::Overrides, Circuit), Value> {
    let overrides = parse_overrides(overrides_json)?;
    let p = Parametric::parse(netlist_json).map_err(|e| error_value(&e))?;
    let c = Circuit::from_parametric(&p, &overrides).map_err(|e| error_value(&e))?;
    Ok((p, overrides, c))
}

fn run(f: impl FnOnce() -> Result<Value, Value>) -> Value {
    f().unwrap_or_else(|e| e)
}

/// Impulse, step and energy-time curve (mixed and minimum phase), excess
/// phase and group delay, the phase-mode decision and the E46 check.
pub fn impulse_value(netlist_json: &str, overrides_json: &str, options_json: &str) -> Value {
    run(|| {
        let mut o = Opts::parse(options_json)?;
        let mut req = ImpulseRequest {
            probe: o.string("probe")?,
            ..Default::default()
        };
        if let Some(fs) = o.f64("fs_Hz")? {
            req.uniform.fs_hz = fs;
        }
        if let Some(n) = o.usize("n")? {
            if n > N_LIMIT {
                return Err(options_error(format!("'n' is limited to {N_LIMIT}")));
            }
            req.uniform.n = n;
        }
        req.uniform.f_min_hz = o.f64("f_min_Hz")?;
        req.uniform.f_max_hz = o.f64("f_max_Hz")?;
        req.pre = o.usize("pre_samples")?;
        if let Some(r) = o.usize("refine")? {
            req.min_phase.refine = r.clamp(1, 16);
        }
        if let Some(e) = o.usize("extend")? {
            req.min_phase.extend = e.clamp(1, 16);
        }
        if let Some(t) = o.f64("threshold_ms")? {
            req.threshold_s = t * 1e-3;
        }
        if let Some(b) = o.bool("ir_length_check")? {
            req.length_check = b;
        }
        if let Some(order) = o.usize("order")? {
            if !(1..=ORDER_LIMIT).contains(&order) {
                return Err(options_error(format!("'order' must be 1 to {ORDER_LIMIT}")));
            }
            req.fit.vf.order = order;
        }
        req.align_delay = o.bool("align_delay")?.unwrap_or(false);
        let spectrum = o.bool("spectrum")?.unwrap_or(true);
        o.finish()?;
        let (p, _, c) = load(netlist_json, overrides_json)?;
        let probe = match &req.probe {
            Some(id) => id.clone(),
            None => report::default_probe(&p, &c).map_err(|e| error_value(&e))?,
        };
        let r = report::impulses(&c, &probe, &req).map_err(|e| error_value(&e))?;
        let mut v = r.to_json(spectrum);
        v["parameters"] = parameters_json(&c);
        Ok(v)
    })
}

fn parameters_json(c: &Circuit) -> Value {
    Value::Object(
        c.parameters
            .iter()
            .map(|(n, v)| (n.clone(), v.to_json()))
            .collect(),
    )
}

/// Rational fit of a probe: poles/Q table, zeros, fit error, group delay,
/// the impulse-length check, and optionally the attribution of resonances
/// to parameters and the state-space realisation.
pub fn vector_fit_value(netlist_json: &str, overrides_json: &str, options_json: &str) -> Value {
    run(|| {
        let mut o = Opts::parse(options_json)?;
        let mut req = PoleRequest {
            probe: o.string("probe")?,
            ..Default::default()
        };
        if let Some(order) = o.usize("order")? {
            if !(1..=ORDER_LIMIT).contains(&order) {
                return Err(options_error(format!("'order' must be 1 to {ORDER_LIMIT}")));
            }
            req.fit.vf.order = order;
        }
        if let Some(it) = o.usize("iterations")? {
            req.fit.vf.iterations = it.clamp(1, 50);
        }
        if let Some(a) = o.string("asymptote")? {
            req.fit.vf.asymptote = Asymptote::parse(&a)
                .ok_or_else(|| options_error("'asymptote' must be zero, constant or linear"))?;
        }
        if let Some(w) = o.string("weighting")? {
            req.fit.vf.weighting = match w.as_str() {
                "relative" => Weighting::Relative,
                "uniform" => Weighting::Uniform,
                _ => return Err(options_error("'weighting' must be relative or uniform")),
            };
        }
        req.fit.f_min_hz = o.f64("f_min_Hz")?;
        req.fit.f_max_hz = o.f64("f_max_Hz")?;
        if let Some(q) = o.f64("q_min")? {
            req.fit.q_min = q;
        }
        if let Some(w) = o.f64("weight_min_dB")? {
            req.fit.weight_min_db = w;
        }
        req.attribute = o.bool("attribute")?.unwrap_or(false);
        req.sensitivity.max_parameters = o.usize("max_parameters")?.unwrap_or(32);
        if req.sensitivity.max_parameters > PARAMETER_LIMIT {
            return Err(options_error(format!(
                "'max_parameters' is limited to {PARAMETER_LIMIT}"
            )));
        }
        if let Some(s) = o.f64("step")? {
            if !(s > 0.0 && s < 0.5) {
                return Err(options_error("'step' must be in (0, 0.5)"));
            }
            req.sensitivity.step = s;
        }
        if let Some(fs) = o.f64("fs_Hz")? {
            req.fs_hz = fs;
        }
        if let Some(n) = o.usize("n")? {
            req.n = n;
        }
        req.state_space = o.bool("state_space")?.unwrap_or(false);
        o.finish()?;
        let (p, ov, c) = load(netlist_json, overrides_json)?;
        let probe = match &req.probe {
            Some(id) => id.clone(),
            None => report::default_probe(&p, &c).map_err(|e| error_value(&e))?,
        };
        let (_, v) =
            report::poles_report(&p, &ov, &c, &probe, &req).map_err(|e| error_value(&e))?;
        Ok(v)
    })
}

/// Passive insertion loss at the drum, per path and in 1/3-octave bands,
/// with the fixture note, and the bleed estimate.
pub fn isolation_value(netlist_json: &str, overrides_json: &str, options_json: &str) -> Value {
    run(|| {
        let mut o = Opts::parse(options_json)?;
        let opts = IsolationOptions {
            drum_probe: o.string("drum_probe")?,
            entrance_node: o.string("entrance_node")?,
            paths: o.bool("paths")?.unwrap_or(true),
        };
        let with_bleed = o.bool("bleed")?.unwrap_or(true);
        let distances: Vec<f64> = match o.get("bleed_distances_m") {
            None => vec![0.3, 1.0],
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| x.as_f64().filter(|d| d.is_finite() && *d > 0.0))
                .collect::<Option<_>>()
                .ok_or_else(|| options_error("'bleed_distances_m' must be positive numbers"))?,
            Some(_) => return Err(options_error("'bleed_distances_m' must be an array")),
        };
        o.finish()?;
        let overrides = parse_overrides(overrides_json)?;
        let p = Parametric::parse(netlist_json).map_err(|e| error_value(&e))?;
        let iso = isolation::insertion_loss(&p, &overrides, &opts).map_err(|e| error_value(&e))?;
        let mut v = iso.to_json();
        v["bleed"] = if with_bleed {
            match isolation::bleed(&p, &overrides, &distances) {
                Ok(b) => b.to_json(),
                Err(e) => error_value(&e),
            }
        } else {
            Value::Null
        };
        Ok(v)
    })
}
