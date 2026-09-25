//! Thermoviscous duct propagation (Zwikker–Kosten / Stinson low-reduced-
//! frequency model), spec Section 6 and Appendix D (corrected).
//!
//! Time convention e^{+jωt}. With k_v = sqrt(jωρ/μ) and k_t = k_v·sqrt(Pr):
//!   ρ_eff = ρ / (1 − F(k_v s)),   K_eff = γP0 / (1 + (γ−1) F(k_t s)),
//! where s is the half-gap h/2 for slits and the radius a for circles, and
//! F is the shape function of `special` (a function of k and both sides for
//! rectangles). A uniform duct of area S and length l has
//! Γ = jω sqrt(ρ_eff/K_eff), Z_c = sqrt(ρ_eff K_eff)/S and the transfer
//! matrix [[cosh Γl, Z_c sinh Γl], [sinh Γl / Z_c, cosh Γl]].
//!
//! The model is checked against an independent mpmath implementation in
//! `tests/thermoviscous.rs` (references from `tools/refgen/thermo_refs.py`).

use crate::air::AirState;
use crate::mna::Transfer;
use crate::special::{shape_circle, shape_rect, shape_slit, Shape};
use crate::C64;

/// Duct cross-section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Section {
    /// Circular tube of radius `radius`.
    Circle { radius: f64 },
    /// Slit of gap `gap` (narrow dimension) and `width` (wide dimension,
    /// assumed much larger than the gap).
    Slit { gap: f64, width: f64 },
    /// Wide duct of arbitrary cross-section, modelled with the circular
    /// shape function at the equivalent radius 2·area/perimeter. Exact in the
    /// thin-boundary-layer limit (loss proportional to perimeter/area); use
    /// only where the boundary layers are thin compared with the section.
    Equivalent { area: f64, perimeter: f64 },
    /// Rectangular duct with sides `a` and `b` (full lengths, both finite),
    /// using Stinson's (1991) double-series shape function; see
    /// [`shape_rect`]. It tends to the slit of gap min(a, b) as the aspect
    /// ratio grows.
    Rect { a: f64, b: f64 },
}

impl Section {
    pub fn area(&self) -> f64 {
        match *self {
            Section::Circle { radius } => std::f64::consts::PI * radius * radius,
            Section::Slit { gap, width } => gap * width,
            Section::Equivalent { area, .. } => area,
            Section::Rect { a, b } => a * b,
        }
    }

    /// The transverse dimension that enters the shape function: radius for
    /// circles, half-gap for slits, 2A/P for equivalent sections. For
    /// rectangles, where both sides enter, it is half the shorter side (the
    /// half-gap of the limiting slit).
    pub fn shape_length(&self) -> f64 {
        match *self {
            Section::Circle { radius } => radius,
            Section::Slit { gap, .. } => 0.5 * gap,
            Section::Equivalent { area, perimeter } => 2.0 * area / perimeter,
            Section::Rect { a, b } => 0.5 * a.min(b),
        }
    }

    /// Shape function for the complex wavenumber `k` (k_v or k_t).
    pub fn shape(&self, k: C64) -> Shape {
        match *self {
            Section::Circle { .. } | Section::Equivalent { .. } => {
                shape_circle(k * self.shape_length())
            }
            Section::Slit { .. } => shape_slit(k * self.shape_length()),
            Section::Rect { a, b } => shape_rect(k, a, b),
        }
    }

    /// Shear wavenumber: transverse dimension over the viscous layer, using
    /// the radius for circles, the half-gap for slits and half the shorter
    /// side for rectangles.
    pub fn shear_wavenumber(&self, air: &AirState, omega: f64) -> f64 {
        self.shape_length() * (omega * air.rho / air.mu).sqrt()
    }
}

/// Effective density and bulk modulus of the air in a duct.
#[derive(Debug, Clone, Copy)]
pub struct DuctMedium {
    pub rho_eff: C64,
    pub k_eff: C64,
}

pub fn medium(section: &Section, air: &AirState, omega: f64) -> DuctMedium {
    let j = C64::new(0.0, 1.0);
    let kv = (j * omega * air.rho / air.mu).sqrt();
    let kt = kv * air.prandtl.sqrt();
    let visc = section.shape(kv);
    let therm = section.shape(kt);
    DuctMedium {
        rho_eff: air.rho / visc.one_minus,
        k_eff: air.bulk_modulus() / (1.0 + (air.gamma - 1.0) * therm.f),
    }
}

/// Propagation constant Γ and characteristic impedance Z_c (acoustic,
/// Pa·s/m³) of a duct, on the branch with Re Γ ≥ 0 and Re Z_c ≥ 0.
pub fn propagation(section: &Section, air: &AirState, omega: f64) -> (C64, C64) {
    let m = medium(section, air, omega);
    let mut s = (m.rho_eff / m.k_eff).sqrt();
    if s.im > 0.0 {
        s = -s;
    }
    let j = C64::new(0.0, 1.0);
    (j * omega * s, m.k_eff * s / section.area())
}

/// Transfer matrix of a uniform duct of the given length.
pub fn abcd(section: &Section, air: &AirState, omega: f64, length: f64) -> [C64; 4] {
    let (gamma, zc) = propagation(section, air, omega);
    let gl = gamma * length;
    let (ch, sh) = (gl.cosh(), gl.sinh());
    [ch, zc * sh, sh / zc, ch]
}

/// Transfer matrix of a uniform duct with the growth e^{Re Γl} factored out,
/// for stamping (see [`Transfer`]).
pub fn transfer(section: &Section, air: &AirState, omega: f64, length: f64) -> Transfer {
    let (gamma, zc) = propagation(section, air, omega);
    Transfer::line(gamma * length, zc)
}

/// Series impedance of a duct treated as lumped (L0): jωρ_eff·l/S, i.e. the
/// frequency-dependent resistance plus inertance, compressibility neglected.
pub fn lumped_series_impedance(section: &Section, air: &AirState, omega: f64, length: f64) -> C64 {
    let m = medium(section, air, omega);
    C64::new(0.0, omega) * m.rho_eff * length / section.area()
}

/// Low-frequency (Poiseuille) oracles.
pub mod poiseuille {
    use std::f64::consts::PI;

    /// Tube: R = 8μL/(πa⁴), M = 4ρL/(3πa²).
    pub fn tube(mu: f64, rho: f64, radius: f64, length: f64) -> (f64, f64) {
        (
            8.0 * mu * length / (PI * radius.powi(4)),
            4.0 * rho * length / (3.0 * PI * radius * radius),
        )
    }

    /// Slit: R = 12μl/(w h³), M = 6ρl/(5 w h).
    pub fn slit(mu: f64, rho: f64, gap: f64, width: f64, length: f64) -> (f64, f64) {
        (
            12.0 * mu * length / (width * gap.powi(3)),
            6.0 * rho * length / (5.0 * width * gap),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn check_limit(section: Section, r0: f64, m0: f64) {
        let air = AirState::spec_reference();
        let omega = 2.0 * PI * 0.01;
        let z = lumped_series_impedance(&section, &air, omega, 1e-2);
        assert!((z.re / r0 - 1.0).abs() < 1e-3, "R {} vs {}", z.re, r0);
        assert!(
            (z.im / omega / m0 - 1.0).abs() < 1e-3,
            "M {} vs {}",
            z.im / omega,
            m0
        );
    }

    #[test]
    fn poiseuille_limits_recovered() {
        let air = AirState::spec_reference();
        let (r, m) = poiseuille::tube(air.mu, air.rho, 1e-3, 1e-2);
        check_limit(Section::Circle { radius: 1e-3 }, r, m);
        let (r, m) = poiseuille::slit(air.mu, air.rho, 0.2e-3, 30e-3, 1e-2);
        check_limit(
            Section::Slit {
                gap: 0.2e-3,
                width: 30e-3,
            },
            r,
            m,
        );
    }

    #[test]
    fn resistance_positive_and_grows_as_sqrt_f_at_high_frequency() {
        let air = AirState::spec_reference();
        let sec = Section::Circle { radius: 2e-3 };
        let r = |f: f64| lumped_series_impedance(&sec, &air, 2.0 * PI * f, 1e-2).re;
        for f in [10.0, 100.0, 1e3, 1e4, 4e4] {
            assert!(r(f) > 0.0);
        }
        let ratio = r(40_000.0) / r(10_000.0);
        assert!((ratio - 2.0).abs() < 0.1, "ratio {ratio}");
    }

    #[test]
    fn two_port_is_reciprocal() {
        let air = AirState::spec_reference();
        for sec in [
            Section::Circle { radius: 1e-3 },
            Section::Slit {
                gap: 0.3e-3,
                width: 20e-3,
            },
        ] {
            for f in [20.0, 1e3, 2e4] {
                let [a, b, c, d] = abcd(&sec, &air, 2.0 * PI * f, 0.03);
                let det = a * d - b * c;
                assert!((det - C64::new(1.0, 0.0)).norm() < 1e-9);
            }
        }
    }
}
