//! Frequency sweeps and result serialisation.

use crate::air::{AirState, P_REF};
use crate::circuit::Circuit;
use crate::diag::{Collector, Warning};
use crate::drive::{DriveInfo, DriveSpec};
use crate::elements::electrical::VSource;
use crate::error::{Error, Result};
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
    /// How the result is driven (every SPL figure states its drive).
    pub drive: DriveInfo,
    /// Resolved parameter values, derived ones included.
    pub parameters: Value,
    /// Names of the parameters whose value reached the netlist.
    pub parameters_used: Vec<String>,
    /// The elements of the resolved netlist, `{"id", "type"}`.
    pub elements: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct SolveResult {
    pub meta: Meta,
    pub freqs_hz: Vec<f64>,
    pub probes: Vec<ProbeResult>,
    pub validity: Vec<ValidityLimit>,
    pub shading: Shading,
    /// Element notes and operating limits exceeded at the stated drive.
    pub warnings: Vec<Warning>,
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
            "warnings": self.warnings,
        })
    }
}

/// Per-frequency factor applied to the solution for the stated drive.
enum Scale {
    Constant(f64),
    /// Constant current `amps` through the source element `source`.
    Current {
        amps: f64,
        source: usize,
    },
}

impl Circuit {
    /// The single `vsource`, if the netlist has exactly one independent
    /// source and it is a `vsource`.
    fn sole_vsource(&self) -> Option<(usize, &VSource)> {
        let mut sources = self
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_source());
        let first = sources.next()?;
        if sources.next().is_some() {
            return None;
        }
        first
            .1
            .as_any()
            .downcast_ref::<VSource>()
            .map(|v| (first.0, v))
    }

    fn drive_scale(&self) -> Result<(Scale, DriveInfo)> {
        let vs = self.sole_vsource();
        let zs = vs.map(|(_, v)| v.zs);
        let zs_text =
            |z: Option<f64>| z.map_or(String::new(), |z| format!(", source impedance {z} ohm"));
        let Some(spec) = &self.drive else {
            let label = match vs {
                Some((_, v)) => format!("vsource '{}': {} V RMS (EMF){}", v.id, v.v, zs_text(zs)),
                None => {
                    let s: Vec<String> = self
                        .elements
                        .iter()
                        .filter(|e| e.is_source())
                        .map(|e| format!("{} '{}'", e.type_name(), e.id()))
                        .collect();
                    if s.is_empty() {
                        "no independent source".into()
                    } else {
                        format!("sources as written: {}", s.join(", "))
                    }
                }
            };
            return Ok((
                Scale::Constant(1.0),
                DriveInfo {
                    convention: "netlist",
                    label,
                    source_voltage_v: vs.map(|(_, v)| v.v),
                    source_impedance_ohm: zs,
                    rated_ohm: None,
                    probe: None,
                },
            ));
        };
        let v_solve = crate::drive::source_voltage(self)?;
        if !(v_solve.is_finite() && v_solve != 0.0) {
            return Err(Error::Netlist(
                "drive: the vsource's V_V must be non-zero to scale from".into(),
            ));
        }
        let (idx, _) = vs.expect("source_voltage checked for one vsource");
        let mut info = DriveInfo {
            convention: spec.convention(),
            label: String::new(),
            source_voltage_v: None,
            source_impedance_ohm: zs,
            rated_ohm: None,
            probe: None,
        };
        let scale = match spec {
            DriveSpec::Voltage(v) => {
                info.label = format!("{v} V RMS (EMF){}", zs_text(zs));
                info.source_voltage_v = Some(*v);
                Scale::Constant(v / v_solve)
            }
            DriveSpec::Power { watts, rated_ohm } => {
                let v = crate::drive::voltage_for_power(*watts, *rated_ohm);
                info.label = format!(
                    "{} mW into {rated_ohm} ohm rated ({v:.4} V RMS EMF){}",
                    watts * 1e3,
                    zs_text(zs)
                );
                info.source_voltage_v = Some(v);
                info.rated_ohm = Some(*rated_ohm);
                Scale::Constant(v / v_solve)
            }
            DriveSpec::Characteristic {
                probe,
                level_db,
                f_hz,
            } => {
                let v = crate::drive::characteristic_voltage(self, probe, *level_db, *f_hz)?;
                info.label = format!(
                    "characteristic voltage {v:.4} V RMS (EMF): {level_db} dB SPL at {f_hz} Hz at '{probe}'{}",
                    zs_text(zs)
                );
                info.source_voltage_v = Some(v);
                info.probe = Some(probe.clone());
                Scale::Constant(v / v_solve)
            }
            DriveSpec::Current(amps) => {
                info.label = format!("{} mA RMS constant current (diagnostic)", amps * 1e3);
                Scale::Current {
                    amps: *amps,
                    source: idx,
                }
            }
        };
        Ok((scale, info))
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
        let (scale, drive) = self.drive_scale()?;
        let mut collector = Collector::default();
        let mut checks = Vec::new();
        for &f in &self.freqs {
            let mut x = self.solve_at(f)?;
            let k = match scale {
                Scale::Constant(k) => C64::new(k, 0.0),
                Scale::Current { amps, source } => {
                    let cx = self.cx(f);
                    let i = self.elements[source]
                        .port_flow(&cx, &x, &self.branches(source), 0)
                        .unwrap_or_default();
                    if i.norm() == 0.0 || !i.norm().is_finite() {
                        return Err(Error::Netlist(format!(
                            "drive: the source current is zero at {f} Hz; cannot hold it constant"
                        )));
                    }
                    C64::new(amps, 0.0) / i
                }
            };
            if k != C64::new(1.0, 0.0) {
                x.iter_mut().for_each(|v| *v *= k);
            }
            for (p, r) in self.probes.iter().zip(probes.iter_mut()) {
                r.values.push(self.probe_value(p, f, &x)?);
            }
            let cx = self.cx(f);
            for (i, e) in self.elements.iter().enumerate() {
                checks.clear();
                e.operating(&cx, &x, &self.branches(i), &mut checks);
                collector.add(e.id(), f, &checks);
            }
        }
        let validity = self.validity();
        let mut warnings: Vec<Warning> = self
            .elements
            .iter()
            .flat_map(|e| {
                e.notes()
                    .into_iter()
                    .map(|n| Warning::from_note(e.id(), &n))
            })
            .collect();
        warnings.extend(collector.into_warnings());
        Ok(SolveResult {
            meta: Meta {
                engine: ENGINE,
                level: self.level,
                air: self.air,
                drive,
                parameters: Value::Object(
                    self.parameters
                        .iter()
                        .map(|(n, v)| (n.clone(), v.to_json()))
                        .collect(),
                ),
                parameters_used: self.parameters_used.clone(),
                elements: self
                    .elements
                    .iter()
                    .map(|e| json!({"id": e.id(), "type": e.type_name()}))
                    .collect(),
            },
            freqs_hz: self.freqs.clone(),
            probes,
            shading: crate::validity::aggregate(&validity),
            validity,
            warnings,
        })
    }
}
