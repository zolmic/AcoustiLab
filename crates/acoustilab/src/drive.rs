//! Drive conventions and sensitivity readouts (spec Section 4; erratum E32).
//!
//! Every SPL figure states its drive. The network is linear, so with a
//! single independent source every drive is a scale factor on a solve made
//! at the netlist's source voltage `v_solve` (usually the default 1 V RMS):
//!
//! 1. **Characteristic voltage** (IEC 60268-7): the voltage that gives
//!    94 dB SPL at 500 Hz at a named pressure probe through the netlist's
//!    source impedance, found by scaling the solve linearly.
//! 2. **One milliwatt into the rated impedance**: V = sqrt(P·Z_rated).
//! 3. **One volt RMS.**
//!
//! plus a constant-current diagnostic (a per-frequency factor from a
//! current probe). Sensitivity is reported in dB SPL per volt and per
//! milliwatt at 500 Hz and 1 kHz, converted with the **rated** impedance:
//! dB/V = dB/mW + 10·log10(1000/Z_rated) (Sections 3 and 4; erratum E32:
//! Appendix C7's use of Re is wrong).
//!
//! These are pure functions over a [`Circuit`] or a [`SolveResult`]; they do
//! not change netlist parsing. Phasors are RMS, so dB SPL is
//! 20·log10(|p|/20 µPa).

use crate::air::P_REF;
use crate::circuit::Circuit;
use crate::elements::electrical::VSource;
use crate::error::{Error, Result};
use crate::solve::SolveResult;
use crate::C64;
use serde::Serialize;
use std::f64::consts::PI;

/// IEC 60268-7 characteristic level, dB SPL.
pub const CHARACTERISTIC_LEVEL_DB: f64 = 94.0;
/// IEC 60268-7 reference frequency, Hz.
pub const CHARACTERISTIC_FREQUENCY_HZ: f64 = 500.0;
/// Sensitivity reference frequencies: 500 Hz per the standard, 1 kHz per
/// most datasheets (spec Section 4).
pub const SENSITIVITY_FREQUENCIES_HZ: [f64; 2] = [500.0, 1000.0];
/// The rated-impedance check flags |Z| below this fraction of the rated
/// impedance (spec Section 4).
pub const RATED_IMPEDANCE_FRACTION: f64 = 0.8;

/// A drive convention.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Drive {
    /// Open-circuit source voltage, V RMS.
    Voltage(f64),
    /// Power into the rated impedance, V = sqrt(P·Z_rated).
    Power { watts: f64, rated_ohm: f64 },
    /// The voltage giving `level_db` at `f_hz` at pressure probe `probe`.
    Characteristic {
        probe: String,
        level_db: f64,
        f_hz: f64,
    },
    /// Constant source current (diagnostic), read from current probe `probe`.
    Current { amps: f64, probe: String },
}

impl Drive {
    /// Convention 3: one volt RMS.
    pub fn one_volt() -> Drive {
        Drive::Voltage(1.0)
    }

    /// Convention 2: one milliwatt into the rated impedance.
    pub fn one_milliwatt(rated_ohm: f64) -> Drive {
        Drive::Power {
            watts: 1e-3,
            rated_ohm,
        }
    }

    /// Convention 1: IEC 60268-7 characteristic voltage at a pressure probe.
    pub fn characteristic(probe: &str) -> Drive {
        Drive::Characteristic {
            probe: probe.to_string(),
            level_db: CHARACTERISTIC_LEVEL_DB,
            f_hz: CHARACTERISTIC_FREQUENCY_HZ,
        }
    }

    /// Human-readable label for plot legends.
    pub fn label(&self) -> String {
        match self {
            Drive::Voltage(v) => format!("{v} V RMS"),
            Drive::Power { watts, rated_ohm } => format!(
                "{} mW into {rated_ohm} ohm rated ({:.4} V RMS)",
                watts * 1e3,
                voltage_for_power(*watts, *rated_ohm)
            ),
            Drive::Characteristic {
                probe,
                level_db,
                f_hz,
            } => format!("characteristic voltage ({level_db} dB SPL at {f_hz} Hz at '{probe}')"),
            Drive::Current { amps, .. } => format!("{amps} A RMS constant current"),
        }
    }
}

/// Voltage that dissipates `watts` in `rated_ohm`: sqrt(P·Z).
pub fn voltage_for_power(watts: f64, rated_ohm: f64) -> f64 {
    (watts * rated_ohm).sqrt()
}

/// dB SPL of an RMS pressure phasor.
pub fn spl_db(p: C64) -> f64 {
    20.0 * (p.norm() / P_REF).log10()
}

/// Open-circuit voltage of the circuit's only voltage source (the `V_V` of
/// its `vsource`).
pub fn source_voltage(circuit: &Circuit) -> Result<f64> {
    let sources: Vec<&VSource> = circuit
        .elements
        .iter()
        .filter_map(|e| e.as_any().downcast_ref::<VSource>())
        .collect();
    let others = circuit
        .elements
        .iter()
        .filter(|e| e.is_source() && e.as_any().downcast_ref::<VSource>().is_none())
        .count();
    match (sources.as_slice(), others) {
        ([s], 0) => Ok(s.v),
        _ => Err(Error::Netlist(format!(
            "drive scaling needs exactly one independent source, a vsource; found {} vsource(s) and {others} other source(s)",
            sources.len()
        ))),
    }
}

/// Value of probe `probe` at frequency `f`, solved exactly at `f`.
pub fn probe_at(circuit: &Circuit, probe: &str, f: f64) -> Result<C64> {
    let pr = circuit
        .probes
        .iter()
        .find(|p| p.id == probe)
        .ok_or_else(|| Error::Probe {
            id: probe.to_string(),
            msg: "no such probe".into(),
        })?;
    let x = circuit.solve_at(f)?;
    circuit.probe_value(pr, f, &x)
}

/// Value of probe `probe` at `f` interpolated from a result: log magnitude
/// and unwrapped phase linear in ln f between the bracketing points (exact
/// at grid frequencies).
pub fn interpolate(result: &SolveResult, probe: &str, f: f64) -> Result<C64> {
    let pr = result.probe(probe).ok_or_else(|| Error::Probe {
        id: probe.to_string(),
        msg: "no such probe in the result".into(),
    })?;
    let fs = &result.freqs_hz;
    let out_of_range = || Error::Probe {
        id: probe.to_string(),
        msg: format!("{f} Hz is outside the solved range"),
    };
    if fs.is_empty() || f < fs[0] || f > fs[fs.len() - 1] {
        return Err(out_of_range());
    }
    if let Some(k) = fs.iter().position(|&x| x == f) {
        return Ok(pr.values[k]);
    }
    let k = fs.iter().position(|&x| x > f).ok_or_else(out_of_range)?;
    let (f0, f1) = (fs[k - 1], fs[k]);
    let (v0, v1) = (pr.values[k - 1], pr.values[k]);
    if v0.norm() == 0.0 || v1.norm() == 0.0 {
        let t = (f - f0) / (f1 - f0);
        return Ok(v0 + (v1 - v0) * t);
    }
    let t = (f / f0).ln() / (f1 / f0).ln();
    let lm = v0.norm().ln() + t * (v1.norm().ln() - v0.norm().ln());
    let mut dphi = v1.arg() - v0.arg();
    while dphi > PI {
        dphi -= 2.0 * PI;
    }
    while dphi <= -PI {
        dphi += 2.0 * PI;
    }
    Ok(C64::from_polar(lm.exp(), v0.arg() + t * dphi))
}

/// Characteristic voltage: the source voltage that gives `level_db` at
/// `f_hz` at pressure probe `probe`, by linear scaling of an exact solve.
pub fn characteristic_voltage(
    circuit: &Circuit,
    probe: &str,
    level_db: f64,
    f_hz: f64,
) -> Result<f64> {
    let v = source_voltage(circuit)?;
    let p = probe_at(circuit, probe, f_hz)?;
    scale_to_level(v, p, level_db, probe)
}

/// Characteristic voltage from a result solved at source voltage `v_solve`
/// (interpolated when `f_hz` is between grid points).
pub fn characteristic_voltage_from(
    result: &SolveResult,
    v_solve: f64,
    probe: &str,
    level_db: f64,
    f_hz: f64,
) -> Result<f64> {
    let p = interpolate(result, probe, f_hz)?;
    scale_to_level(v_solve, p, level_db, probe)
}

fn scale_to_level(v: f64, p: C64, level_db: f64, probe: &str) -> Result<f64> {
    if p.norm() == 0.0 || !p.norm().is_finite() {
        return Err(Error::Probe {
            id: probe.to_string(),
            msg: "zero or non-finite pressure; cannot scale to a level".into(),
        });
    }
    Ok(v * 10f64.powf((level_db - spl_db(p)) / 20.0))
}

/// Per-frequency factors that turn a result solved at source voltage
/// `v_solve` into the given drive. Constant for voltage, power and
/// characteristic drives; for constant current, I_target/I_solve(f) from the
/// drive's current probe.
pub fn scale_factors(drive: &Drive, result: &SolveResult, v_solve: f64) -> Result<Vec<C64>> {
    let n = result.freqs_hz.len();
    let constant = |v: f64| vec![C64::new(v / v_solve, 0.0); n];
    Ok(match drive {
        Drive::Voltage(v) => constant(*v),
        Drive::Power { watts, rated_ohm } => constant(voltage_for_power(*watts, *rated_ohm)),
        Drive::Characteristic {
            probe,
            level_db,
            f_hz,
        } => constant(characteristic_voltage_from(
            result, v_solve, probe, *level_db, *f_hz,
        )?),
        Drive::Current { amps, probe } => {
            let pr = result.probe(probe).ok_or_else(|| Error::Probe {
                id: probe.clone(),
                msg: "no such probe in the result".into(),
            })?;
            pr.values.iter().map(|i| C64::new(*amps, 0.0) / i).collect()
        }
    })
}

/// Applies per-frequency factors to every probe except impedances (which
/// are ratios and do not scale). Valid when the result has one source.
pub fn apply(result: &SolveResult, factors: &[C64]) -> SolveResult {
    let mut out = result.clone();
    for p in &mut out.probes {
        if p.quantity == "impedance" {
            continue;
        }
        for (v, k) in p.values.iter_mut().zip(factors) {
            *v *= k;
        }
    }
    out
}

/// A result re-scaled to `drive`.
pub fn drive_result(drive: &Drive, result: &SolveResult, v_solve: f64) -> Result<SolveResult> {
    Ok(apply(result, &scale_factors(drive, result, v_solve)?))
}

/// Sensitivity at one frequency.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sensitivity {
    pub f_hz: f64,
    /// dB SPL for 1 V RMS.
    pub db_per_v: f64,
    /// dB SPL for 1 mW into the rated impedance.
    pub db_per_mw: f64,
    pub rated_ohm: f64,
    /// dB/V − dB/mW = 10·log10(1000/Z_rated).
    pub conversion_db: f64,
}

/// dB/V − dB/mW = 10·log10(1000/Z_rated): 1 mW needs sqrt(1e-3·Z) volts.
pub fn conversion_db(rated_ohm: f64) -> f64 {
    10.0 * (1000.0 / rated_ohm).log10()
}

impl Sensitivity {
    /// From the pressure produced per volt of source voltage.
    pub fn from_pressure_per_volt(p_per_v: C64, f_hz: f64, rated_ohm: f64) -> Self {
        let db_per_v = spl_db(p_per_v);
        let conversion_db = conversion_db(rated_ohm);
        Sensitivity {
            f_hz,
            db_per_v,
            db_per_mw: db_per_v - conversion_db,
            rated_ohm,
            conversion_db,
        }
    }
}

/// Sensitivity at 500 Hz and 1 kHz at pressure probe `probe`, solved
/// exactly at those frequencies.
pub fn sensitivity(circuit: &Circuit, probe: &str, rated_ohm: f64) -> Result<[Sensitivity; 2]> {
    let v = source_voltage(circuit)?;
    let at = |f: f64| -> Result<Sensitivity> {
        Ok(Sensitivity::from_pressure_per_volt(
            probe_at(circuit, probe, f)? / v,
            f,
            rated_ohm,
        ))
    };
    Ok([
        at(SENSITIVITY_FREQUENCIES_HZ[0])?,
        at(SENSITIVITY_FREQUENCIES_HZ[1])?,
    ])
}

/// Sensitivity from a result solved at `v_solve` (interpolated).
pub fn sensitivity_from(
    result: &SolveResult,
    v_solve: f64,
    probe: &str,
    rated_ohm: f64,
) -> Result<[Sensitivity; 2]> {
    let at = |f: f64| -> Result<Sensitivity> {
        Ok(Sensitivity::from_pressure_per_volt(
            interpolate(result, probe, f)? / v_solve,
            f,
            rated_ohm,
        ))
    };
    Ok([
        at(SENSITIVITY_FREQUENCIES_HZ[0])?,
        at(SENSITIVITY_FREQUENCIES_HZ[1])?,
    ])
}

/// Level change from a source impedance: 20·log10(|Z|/|Z + Zs|), the
/// divider between the source and the load (spec Section 4).
pub fn source_colouring_db(z_load: C64, z_source: C64) -> f64 {
    20.0 * (z_load.norm() / (z_load + z_source).norm()).log10()
}

/// What is held constant when comparing two coil windings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RewindBasis {
    Voltage,
    /// Power into the rated impedance, which scales with Re.
    Power,
    Current,
}

/// Level change of a coil rewind at constant moving mass (spec p. 15 and
/// Appendix C7): Bl ∝ sqrt(Re), so p ∝ Bl·i gives −10·log10(Re2/Re1) at
/// constant voltage, 0 at constant power and +10·log10(Re2/Re1) at constant
/// current (±9.72 dB for 32 → 300 Ω). The response shape is unchanged when
/// every electrical impedance of the coil scales with Re.
pub fn rewind_level_change_db(re_from: f64, re_to: f64, basis: RewindBasis) -> f64 {
    let d = 10.0 * (re_to / re_from).log10();
    match basis {
        RewindBasis::Voltage => -d,
        RewindBasis::Power => 0.0,
        RewindBasis::Current => d,
    }
}

/// Frequencies where |Z| falls below 80 % of the rated impedance (spec
/// Section 4, rated-impedance check).
pub fn rated_impedance_violations(freqs: &[f64], z: &[C64], rated_ohm: f64) -> Vec<f64> {
    freqs
        .iter()
        .zip(z)
        .filter(|(_, z)| z.norm() < RATED_IMPEDANCE_FRACTION * rated_ohm)
        .map(|(f, _)| *f)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert!((voltage_for_power(1e-3, 32.0) - 0.178_885_438_199_983).abs() < 1e-12);
        assert!((conversion_db(32.0) - 14.948_500_216_800_94).abs() < 1e-12);
        assert!((rewind_level_change_db(32.0, 300.0, RewindBasis::Voltage) + 9.7197).abs() < 1e-4);
        assert_eq!(rewind_level_change_db(32.0, 300.0, RewindBasis::Power), 0.0);
        // Spec Section 4: 120 Ω on 32 Ω / 52 Ω peak adds about 3 dB at the peak.
        let lift = source_colouring_db(C64::new(52.0, 0.0), C64::new(120.0, 0.0))
            - source_colouring_db(C64::new(32.0, 0.0), C64::new(120.0, 0.0));
        assert!((lift - 3.1434).abs() < 1e-4, "{lift}");
    }
}
