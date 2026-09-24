//! Frequency sweeps and result serialisation.

use crate::air::{AirState, P_REF};
use crate::circuit::Circuit;
use crate::error::Result;
use crate::validity::{Shading, ValidityLimit};
use crate::C64;
use serde::Serialize;
use serde_json::{json, Value};

pub const ENGINE: &str = concat!("acoustilab ", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub id: String,
    pub quantity: String,
    pub unit: String,
    pub is_pressure: bool,
    pub values: Vec<C64>,
}

impl ProbeResult {
    /// dB SPL re 20 µPa (values are RMS phasors).
    pub fn spl_db(&self) -> Vec<f64> {
        self.values
            .iter()
            .map(|p| 20.0 * (p.norm() / P_REF).log10())
            .collect()
    }

    pub fn magnitude(&self) -> Vec<f64> {
        self.values.iter().map(|v| v.norm()).collect()
    }

    pub fn phase_deg(&self) -> Vec<f64> {
        self.values.iter().map(|v| v.arg().to_degrees()).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub engine: &'static str,
    pub level: u8,
    pub air: AirState,
}

#[derive(Debug, Clone)]
pub struct SolveResult {
    pub meta: Meta,
    pub freqs_hz: Vec<f64>,
    pub probes: Vec<ProbeResult>,
    pub validity: Vec<ValidityLimit>,
    pub shading: Shading,
}

impl SolveResult {
    pub fn probe(&self, id: &str) -> Option<&ProbeResult> {
        self.probes.iter().find(|p| p.id == id)
    }

    pub fn to_json(&self) -> Value {
        let probes: Vec<Value> = self
            .probes
            .iter()
            .map(|p| {
                let mut o = json!({
                    "id": p.id,
                    "quantity": p.quantity,
                    "unit": p.unit,
                    "re": p.values.iter().map(|v| v.re).collect::<Vec<_>>(),
                    "im": p.values.iter().map(|v| v.im).collect::<Vec<_>>(),
                    "magnitude": p.magnitude(),
                    "phase_deg": p.phase_deg(),
                });
                if p.is_pressure {
                    o["spl_dB"] = json!(p.spl_db());
                }
                o
            })
            .collect();
        json!({
            "meta": self.meta,
            "frequencies_Hz": self.freqs_hz,
            "probes": probes,
            "validity": self.validity,
            "shading": self.shading,
        })
    }
}

impl Circuit {
    /// Solves every frequency of the sweep and evaluates all probes.
    pub fn solve(&self) -> Result<SolveResult> {
        let mut probes: Vec<ProbeResult> = self
            .probes
            .iter()
            .map(|p| ProbeResult {
                id: p.id.clone(),
                quantity: p.quantity.clone(),
                unit: p.unit.to_string(),
                is_pressure: p.is_pressure,
                values: Vec::with_capacity(self.freqs.len()),
            })
            .collect();
        for &f in &self.freqs {
            let x = self.solve_at(f)?;
            for (p, r) in self.probes.iter().zip(probes.iter_mut()) {
                r.values.push(self.probe_value(p, f, &x)?);
            }
        }
        let validity = self.validity();
        Ok(SolveResult {
            meta: Meta {
                engine: ENGINE,
                level: self.level,
                air: self.air,
            },
            freqs_hz: self.freqs.clone(),
            probes,
            shading: crate::validity::aggregate(&validity),
            validity,
        })
    }
}
