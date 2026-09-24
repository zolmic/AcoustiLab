//! Unit-suffixed parameter keys.
//!
//! Every dimensional value in a netlist carries its unit in the key, e.g.
//! `radius_mm`, `volume_cm3`, `Bl_Tm`. The reader accepts exactly one
//! spelling per quantity, converts it to SI, and rejects unknown or unused
//! keys, so a misread file fails loudly (spec Appendix B).

use crate::error::{Error, Result};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Physical dimension of a parameter, with the unit suffixes it accepts
/// and the factor that converts each to SI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dim {
    Length,
    Area,
    Volume,
    Mass,
    Time,
    Frequency,
    Temperature,
    Pressure,
    Voltage,
    Current,
    Power,
    ElecResistance,
    Inductance,
    Capacitance,
    ForceFactor,
    Force,
    MechCompliance,
    MechStiffness,
    MechResistance,
    Velocity,
    AcousticCompliance,
    AcousticResistance,
    AcousticInertance,
    VolumeVelocity,
    /// Specific flow resistance of a mesh or fabric, Pa·s/m (MKS rayl).
    SpecificFlowResistance,
    /// Flow resistivity of a porous material, Pa·s/m².
    FlowResistivity,
    Density,
}

impl Dim {
    /// (suffix, factor-to-SI). Temperature is special-cased.
    pub fn suffixes(self) -> &'static [(&'static str, f64)] {
        use Dim::*;
        match self {
            Length => &[("m", 1.0), ("cm", 1e-2), ("mm", 1e-3), ("um", 1e-6)],
            Area => &[("m2", 1.0), ("cm2", 1e-4), ("mm2", 1e-6)],
            Volume => &[("m3", 1.0), ("L", 1e-3), ("cm3", 1e-6), ("mm3", 1e-9)],
            Mass => &[("kg", 1.0), ("g", 1e-3), ("mg", 1e-6)],
            Time => &[("s", 1.0), ("ms", 1e-3), ("us", 1e-6)],
            Frequency => &[("Hz", 1.0), ("kHz", 1e3)],
            Temperature => &[("C", 1.0), ("K", 1.0)],
            Pressure => &[("Pa", 1.0), ("kPa", 1e3)],
            Voltage => &[("V", 1.0), ("mV", 1e-3)],
            Current => &[("A", 1.0), ("mA", 1e-3)],
            Power => &[("W", 1.0), ("mW", 1e-3)],
            ElecResistance => &[("ohm", 1.0)],
            Inductance => &[("H", 1.0), ("mH", 1e-3), ("uH", 1e-6)],
            Capacitance => &[("F", 1.0), ("uF", 1e-6), ("nF", 1e-9), ("pF", 1e-12)],
            ForceFactor => &[("Tm", 1.0), ("N_per_A", 1.0)],
            Force => &[("N", 1.0), ("mN", 1e-3)],
            MechCompliance => &[("m_per_N", 1.0), ("mm_per_N", 1e-3), ("um_per_N", 1e-6)],
            MechStiffness => &[("N_per_m", 1.0), ("N_per_mm", 1e3)],
            MechResistance => &[("Ns_per_m", 1.0), ("kg_per_s", 1.0)],
            Velocity => &[("m_per_s", 1.0), ("mm_per_s", 1e-3)],
            AcousticCompliance => &[("m3_per_Pa", 1.0), ("m5_per_N", 1.0)],
            AcousticResistance => &[("Pa_s_per_m3", 1.0), ("MPa_s_per_m3", 1e6)],
            AcousticInertance => &[("kg_per_m4", 1.0)],
            VolumeVelocity => &[("m3_per_s", 1.0), ("cm3_per_s", 1e-6)],
            SpecificFlowResistance => &[("rayl", 1.0), ("Pa_s_per_m", 1.0)],
            FlowResistivity => &[("Pa_s_per_m2", 1.0), ("kPa_s_per_m2", 1e3)],
            Density => &[("kg_per_m3", 1.0), ("g_per_cm3", 1e3)],
        }
    }
}

/// Reads parameters from an element's JSON object, tracking which keys were
/// consumed so that `finish` can reject anything left over.
pub struct Params {
    ctx: String,
    map: Map<String, Value>,
    used: BTreeSet<String>,
}

impl Params {
    pub fn new(ctx: impl Into<String>, map: Map<String, Value>) -> Self {
        Params {
            ctx: ctx.into(),
            map,
            used: BTreeSet::new(),
        }
    }

    pub fn context(&self) -> &str {
        &self.ctx
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error::element(self.ctx.clone(), msg)
    }

    /// Marks a key as consumed without reading it (e.g. structural keys).
    pub fn mark_used(&mut self, key: &str) {
        self.used.insert(key.to_string());
    }

    pub fn has(&self, base: &str) -> bool {
        self.map.contains_key(base)
            || self
                .map
                .keys()
                .any(|k| k.starts_with(base) && k[base.len()..].starts_with('_'))
    }

    fn number_of(&self, key: &str, v: &Value) -> Result<f64> {
        v.as_f64()
            .filter(|x| x.is_finite())
            .ok_or_else(|| self.err(format!("'{key}' must be a finite number")))
    }

    /// Optional dimensional quantity `base_<suffix>`, converted to SI.
    pub fn quantity_opt(&mut self, base: &str, dim: Dim) -> Result<Option<f64>> {
        let mut found: Option<(String, f64)> = None;
        for (suffix, factor) in dim.suffixes() {
            let key = format!("{base}_{suffix}");
            if let Some(v) = self.map.get(&key) {
                if let Some((prev, _)) = &found {
                    return Err(self.err(format!("both '{prev}' and '{key}' given")));
                }
                let x = self.number_of(&key, v)?;
                let si = if dim == Dim::Temperature {
                    if *suffix == "C" {
                        x + 273.15
                    } else {
                        x
                    }
                } else {
                    x * factor
                };
                found = Some((key, si));
            }
        }
        if self.map.contains_key(base) {
            return Err(self.err(format!(
                "'{base}' needs a unit suffix, one of: {}",
                dim.suffixes()
                    .iter()
                    .map(|(s, _)| format!("{base}_{s}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(found.map(|(key, v)| {
            self.used.insert(key);
            v
        }))
    }

    /// Required dimensional quantity.
    pub fn quantity(&mut self, base: &str, dim: Dim) -> Result<f64> {
        self.quantity_opt(base, dim)?.ok_or_else(|| {
            self.err(format!(
                "missing '{base}' (one of: {})",
                dim.suffixes()
                    .iter()
                    .map(|(s, _)| format!("{base}_{s}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
    }

    /// Required strictly positive quantity.
    pub fn positive(&mut self, base: &str, dim: Dim) -> Result<f64> {
        let v = self.quantity(base, dim)?;
        if v > 0.0 {
            Ok(v)
        } else {
            Err(self.err(format!("'{base}' must be positive")))
        }
    }

    /// Optional strictly positive quantity.
    pub fn positive_opt(&mut self, base: &str, dim: Dim) -> Result<Option<f64>> {
        match self.quantity_opt(base, dim)? {
            Some(v) if v <= 0.0 => Err(self.err(format!("'{base}' must be positive"))),
            other => Ok(other),
        }
    }

    /// Optional dimensionless number stored under the bare key.
    pub fn number_opt(&mut self, key: &str) -> Result<Option<f64>> {
        match self.map.get(key) {
            None => Ok(None),
            Some(v) => {
                let x = self.number_of(key, v)?;
                self.used.insert(key.to_string());
                Ok(Some(x))
            }
        }
    }

    pub fn number(&mut self, key: &str) -> Result<f64> {
        self.number_opt(key)?
            .ok_or_else(|| self.err(format!("missing '{key}'")))
    }

    pub fn number_or(&mut self, key: &str, default: f64) -> Result<f64> {
        Ok(self.number_opt(key)?.unwrap_or(default))
    }

    pub fn count_or(&mut self, key: &str, default: usize) -> Result<usize> {
        match self.map.get(key) {
            None => Ok(default),
            Some(v) => {
                let n = v
                    .as_u64()
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| self.err(format!("'{key}' must be a positive integer")))?;
                self.used.insert(key.to_string());
                Ok(n as usize)
            }
        }
    }

    pub fn bool_or(&mut self, key: &str, default: bool) -> Result<bool> {
        match self.map.get(key) {
            None => Ok(default),
            Some(v) => {
                let b = v
                    .as_bool()
                    .ok_or_else(|| self.err(format!("'{key}' must be true or false")))?;
                self.used.insert(key.to_string());
                Ok(b)
            }
        }
    }

    pub fn string_opt(&mut self, key: &str) -> Result<Option<String>> {
        match self.map.get(key) {
            None => Ok(None),
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| self.err(format!("'{key}' must be a string")))?
                    .to_string();
                self.used.insert(key.to_string());
                Ok(Some(s))
            }
        }
    }

    /// Raw JSON value (for nested objects such as distributions).
    pub fn value_opt(&mut self, key: &str) -> Option<Value> {
        let v = self.map.get(key).cloned();
        if v.is_some() {
            self.used.insert(key.to_string());
        }
        v
    }

    /// Errors if any key was not consumed.
    pub fn finish(self) -> Result<()> {
        let unused: Vec<&String> = self
            .map
            .keys()
            .filter(|k| !self.used.contains(*k))
            .collect();
        if unused.is_empty() {
            Ok(())
        } else {
            Err(self.err(format!(
                "unknown or misspelled key(s): {}",
                unused
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(v: Value) -> Params {
        Params::new("t", v.as_object().unwrap().clone())
    }

    #[test]
    fn converts_suffixes_to_si() {
        let mut p = params(json!({"radius_mm": 1.5, "volume_cm3": 30, "T_C": 20}));
        assert_eq!(p.quantity("radius", Dim::Length).unwrap(), 1.5e-3);
        assert!((p.quantity("volume", Dim::Volume).unwrap() - 30e-6).abs() < 1e-18);
        assert!((p.quantity("T", Dim::Temperature).unwrap() - 293.15).abs() < 1e-12);
        p.finish().unwrap();
    }

    #[test]
    fn rejects_bare_duplicate_and_unknown_keys() {
        let mut p = params(json!({"radius": 1.0}));
        assert!(p.quantity("radius", Dim::Length).is_err());
        let mut p = params(json!({"radius_mm": 1.0, "radius_m": 0.001}));
        assert!(p.quantity("radius", Dim::Length).is_err());
        let mut p = params(json!({"radius_mm": 1.0, "raduis_mm": 2.0}));
        p.quantity("radius", Dim::Length).unwrap();
        assert!(p.finish().is_err());
    }
}
