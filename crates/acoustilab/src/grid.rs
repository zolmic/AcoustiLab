//! Frequency grids.

use crate::error::{Error, Result};
use crate::units::{Dim, Params};
use serde_json::Value;

/// Largest sweep the engine accepts (points).
pub const MAX_POINTS: usize = 1_000_000;

/// Log-spaced grid from `f_min` to `f_max` inclusive with the given density.
pub fn log_grid(f_min: f64, f_max: f64, points_per_octave: f64) -> Vec<f64> {
    assert!(f_min > 0.0 && f_max >= f_min && points_per_octave > 0.0);
    let octaves = (f_max / f_min).log2();
    let n = (octaves * points_per_octave).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| f_min * 2f64.powf(octaves * i as f64 / n as f64))
        .collect()
}

/// Parses the netlist `sweep` object:
/// `{"f_min_Hz": 10, "f_max_Hz": 40000, "points_per_octave": 48}` or
/// `{"frequencies_Hz": [..]}`. Default: 10 Hz to 40 kHz at 48 per octave
/// (spec Sections 3 and 14).
pub fn from_json(v: Option<&Value>) -> Result<Vec<f64>> {
    let Some(v) = v else {
        return Ok(log_grid(10.0, 40_000.0, 48.0));
    };
    let map = v
        .as_object()
        .ok_or_else(|| Error::Netlist("'sweep' must be an object".into()))?
        .clone();
    let mut p = Params::new("sweep", map);
    if let Some(list) = p.value_opt("frequencies_Hz") {
        let freqs: Vec<f64> = list
            .as_array()
            .ok_or_else(|| Error::Netlist("'frequencies_Hz' must be an array".into()))?
            .iter()
            .map(|x| x.as_f64().filter(|f| *f > 0.0 && f.is_finite()))
            .collect::<Option<_>>()
            .ok_or_else(|| Error::Netlist("frequencies must be positive numbers".into()))?;
        p.finish()?;
        if freqs.is_empty() || freqs.len() > MAX_POINTS {
            return Err(Error::Netlist(format!(
                "'frequencies_Hz' must hold 1 to {MAX_POINTS} values"
            )));
        }
        return Ok(freqs);
    }
    let f_min = p.positive_opt("f_min", Dim::Frequency)?.unwrap_or(10.0);
    let f_max = p.positive_opt("f_max", Dim::Frequency)?.unwrap_or(40_000.0);
    let ppo = p.number_or("points_per_octave", 48.0)?;
    // Reserved for the adaptive refinement pass; accepted and ignored for now.
    let _ = p.bool_or("adaptive", false)?;
    p.finish()?;
    if f_max < f_min || ppo <= 0.0 {
        return Err(Error::Netlist(
            "sweep needs f_max >= f_min and points_per_octave > 0".into(),
        ));
    }
    let n = (f_max / f_min).log2() * ppo;
    if n >= MAX_POINTS as f64 {
        return Err(Error::Netlist(format!(
            "sweep would have {n:.3e} points; the limit is {MAX_POINTS}"
        )));
    }
    Ok(log_grid(f_min, f_max, ppo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_is_inclusive_and_log_spaced() {
        let g = log_grid(10.0, 40_000.0, 48.0);
        assert_eq!(g.first().copied(), Some(10.0));
        assert!((g.last().unwrap() - 40_000.0).abs() < 1e-6);
        let r = g[1] / g[0];
        assert!((r.log2() * 48.0 - 1.0).abs() < 0.01);
    }
}
