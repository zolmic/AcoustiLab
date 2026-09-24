//! Air state: density, sound speed, viscosity, conductivity and derived
//! boundary-layer thicknesses (spec Section 6, "Air state").

use crate::error::{Error, Result};
use crate::units::{Dim, Params};
use serde::Serialize;
use serde_json::Value;

/// Specific gas constant of dry air, J/(kg·K).
pub const R_AIR: f64 = 287.05;
/// Specific heat at constant pressure of air, J/(kg·K).
pub const CP_AIR: f64 = 1005.0;
/// Reference RMS pressure for dB SPL.
pub const P_REF: f64 = 20e-6;

/// Thermodynamic and transport properties used by every element.
///
/// `p0` is the static pressure and satisfies `gamma * p0 == rho * c^2`
/// exactly, so adiabatic compliances computed as `V / (gamma p0)` and
/// `V / (rho c^2)` agree.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AirState {
    pub temperature_k: f64,
    pub p0: f64,
    pub rho: f64,
    pub c: f64,
    pub mu: f64,
    pub gamma: f64,
    pub prandtl: f64,
}

impl AirState {
    /// The constants the spec's analytical checks use (Section 17, App. C):
    /// ρ = 1.204 kg/m³, c = 343 m/s, μ = 1.81e-5 Pa·s, γ = 1.4, Pr = 0.71.
    /// Static pressure is derived as ρc²/γ for internal consistency.
    pub fn spec_reference() -> Self {
        let (rho, c, gamma) = (1.204, 343.0, 1.4);
        AirState {
            temperature_k: 293.15,
            p0: rho * c * c / gamma,
            rho,
            c,
            mu: 1.81e-5,
            gamma,
            prandtl: 0.71,
        }
    }

    /// Dry air at temperature `t_k` (K) and static pressure `p0` (Pa):
    /// ideal gas, Sutherland viscosity and conductivity.
    pub fn from_conditions(t_k: f64, p0: f64) -> Self {
        let gamma = 1.4;
        let rho = p0 / (R_AIR * t_k);
        let c = (gamma * R_AIR * t_k).sqrt();
        let mu = 1.458e-6 * t_k.powf(1.5) / (t_k + 110.4);
        let kappa = 2.646e-3 * t_k.powf(1.5) / (t_k + 245.4 * 10f64.powf(-12.0 / t_k));
        AirState {
            temperature_k: t_k,
            p0,
            rho,
            c,
            mu,
            gamma,
            prandtl: mu * CP_AIR / kappa,
        }
    }

    /// Default measurement conditions of IEC 60318-4: 23 °C, 101.325 kPa.
    pub fn standard_23c() -> Self {
        Self::from_conditions(296.15, 101_325.0)
    }

    /// Parses the netlist `air` object: either `{"preset": "spec_reference" |
    /// "standard_23C"}` or `{"T_C": .., "P_kPa": ..}` with optional overrides.
    pub fn from_json(v: Option<&Value>) -> Result<Self> {
        let Some(v) = v else {
            return Ok(Self::standard_23c());
        };
        let map = v
            .as_object()
            .ok_or_else(|| Error::Netlist("'air' must be an object".into()))?
            .clone();
        let mut p = Params::new("air", map);
        let air = match p.string_opt("preset")?.as_deref() {
            Some("spec_reference") => Self::spec_reference(),
            Some("standard_23C") => Self::standard_23c(),
            Some(other) => {
                return Err(Error::Netlist(format!("unknown air preset '{other}'")));
            }
            None => {
                let t = p.quantity_opt("T", Dim::Temperature)?.unwrap_or(296.15);
                let p0 = p.quantity_opt("P", Dim::Pressure)?.unwrap_or(101_325.0);
                Self::from_conditions(t, p0)
            }
        };
        // Relative humidity is accepted for forward compatibility; its
        // ~0.1-0.3 % effect on sound speed is not modelled yet.
        let _ = p.number_opt("RH")?;
        p.finish()?;
        Ok(air)
    }

    /// Adiabatic bulk modulus γP0 = ρc².
    pub fn bulk_modulus(&self) -> f64 {
        self.gamma * self.p0
    }

    /// Characteristic impedance ρc.
    pub fn rho_c(&self) -> f64 {
        self.rho * self.c
    }

    /// Viscous boundary-layer thickness sqrt(2μ/(ρω)).
    pub fn viscous_layer(&self, omega: f64) -> f64 {
        (2.0 * self.mu / (self.rho * omega)).sqrt()
    }

    /// Thermal boundary-layer thickness, viscous layer over sqrt(Pr).
    pub fn thermal_layer(&self, omega: f64) -> f64 {
        self.viscous_layer(omega) / self.prandtl.sqrt()
    }

    /// Wavenumber ω/c.
    pub fn k(&self, omega: f64) -> f64 {
        omega / self.c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn spec_boundary_layers() {
        // Section 6: 0.49 mm at 20 Hz, 0.22 at 100 Hz, 0.069 at 1 kHz, 0.022 at 10 kHz.
        let air = AirState::spec_reference();
        let d = |f: f64| air.viscous_layer(2.0 * PI * f) * 1e3;
        assert!((d(20.0) - 0.489).abs() < 0.001);
        assert!((d(100.0) - 0.219).abs() < 0.001);
        assert!((d(1000.0) - 0.0692).abs() < 0.0005);
        assert!((d(10000.0) - 0.0219).abs() < 0.0005);
        let ratio = air.thermal_layer(1.0) / air.viscous_layer(1.0);
        assert!((ratio - 1.187).abs() < 0.001);
    }

    #[test]
    fn conditions_match_spec_reference_at_20c() {
        let a = AirState::from_conditions(293.15, 101_325.0);
        assert!((a.rho - 1.204).abs() < 0.001);
        assert!((a.c - 343.2).abs() < 0.1);
        assert!((a.mu - 1.813e-5).abs() < 0.005e-5);
        assert!((a.prandtl - 0.71).abs() < 0.01);
    }
}
