//! Cup-wall transmission (spec Section 10, "Passive isolation"; Section 8,
//! "Cup and cushion as a mechanical branch").
//!
//! `shell` is a panel between an inside and an outside acoustic node: the
//! mass, stiffness and loss of the part of the cup that moves, referred to
//! the volume velocity it displaces,
//!
//!   Z = R + jωM_a + 1/(jωC_a),   M_a = κ·m/A²,
//!
//! with m the moving mass, A the panel area and κ the acoustic-mass factor
//! of the deflection shape, κ = A·∫w² dA/(∫w dA)². The default profile is
//! the rigid piston, κ = 1: the whole cup (or a stiff wall section) moving
//! as one on its compliant mounting, the cushion — the path by which, per
//! Section 8, the cup mass on the cushion spring sets the sealed-cup
//! isolation plateau. A panel clamped at its rim and flexing in its
//! fundamental mode uses `plate` (w ∝ (1 − r²/a²)², κ = 9/5); a tensioned
//! membrane uses `membrane` (w ∝ 1 − r²/a², κ = 4/3), as in
//! `membrane_vent`.
//!
//! The stiffness is given as the resonance f0 of the panel's own mass on it
//! (C_a = 1/(ω0²·M_a)) or directly as C_a; without either the panel is
//! limp. The loss is R, or Q = sqrt(M_a/C_a)/R. Above f0 the panel is
//! mass-controlled: |Z| grows 6 dB per doubling of frequency or of mass,
//! and between two radiation loads ρc/A it reproduces the normal-incidence
//! mass law τ = 1/|1 + jωm_s/(2ρc)|² exactly (m_s = m/A; `tests/isolation.rs`).
//!
//! The element is the lumped fundamental of the panel: it is valid below
//! the panel's next mode (for a clamped circular plate the second
//! axisymmetric mode is at 3.89·f0 of its bending fundamental). The
//! radiation mass of the air on either face is not included; add a
//! `radiation` element where it matters.

use super::acoustic::acoustic_terminals;
use super::{Build, Constructor, Element, OnePort};
use crate::error::Result;
use crate::units::Dim;
use crate::C64;
use std::f64::consts::PI;

pub const TYPES: &[&str] = &["shell"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    match ty {
        "shell" => Some(shell),
        _ => None,
    }
}

/// Series impedance of a panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShellModel {
    /// Panel area, m².
    pub area: f64,
    /// Moving mass, kg.
    pub mass: f64,
    /// Acoustic-mass factor of the deflection shape.
    pub kappa: f64,
    /// Acoustic compliance, m³/Pa (`None`: limp).
    pub compliance: Option<f64>,
    /// Acoustic resistance, Pa·s/m³.
    pub resistance: f64,
}

impl ShellModel {
    /// Acoustic mass κ·m/A², kg/m⁴.
    pub fn acoustic_mass(&self) -> f64 {
        self.kappa * self.mass / (self.area * self.area)
    }

    pub fn impedance(&self, omega: f64) -> C64 {
        let jw = C64::new(0.0, omega);
        let mut z = C64::new(self.resistance, 0.0) + jw * self.acoustic_mass();
        if let Some(c) = self.compliance {
            z += (jw * c).inv();
        }
        z
    }

    /// Resonance of the panel mass on its stiffness, Hz.
    pub fn resonance(&self) -> Option<f64> {
        self.compliance
            .map(|c| 1.0 / (2.0 * PI * (self.acoustic_mass() * c).sqrt()))
    }
}

fn shell(mut b: Build) -> Result<Box<dyn Element>> {
    let (n1, n2) = acoustic_terminals(&b)?;
    let p = &mut b.params;
    let area = {
        let a = p.positive_opt("area", Dim::Area)?;
        let r = p.positive_opt("radius", Dim::Length)?;
        let d = p.positive_opt("diameter", Dim::Length)?;
        match (a, r, d) {
            (Some(a), None, None) => a,
            (None, Some(r), None) => PI * r * r,
            (None, None, Some(d)) => 0.25 * PI * d * d,
            _ => return Err(b.err("give exactly one of 'area', 'radius' or 'diameter'")),
        }
    };
    let p = &mut b.params;
    let sigma = p.number_opt("surface_density_kg_per_m2")?;
    let mass = p.positive_opt("mass", Dim::Mass)?;
    let thickness = p.positive_opt("thickness", Dim::Length)?;
    let density = p.positive_opt("density", Dim::Density)?;
    let mass = match (sigma, mass, thickness, density) {
        (Some(s), None, None, None) if s > 0.0 && s.is_finite() => s * area,
        (Some(_), None, None, None) => {
            return Err(b.err("'surface_density_kg_per_m2' must be positive"))
        }
        (None, Some(m), None, None) => m,
        (None, None, Some(t), Some(rho)) => t * rho * area,
        _ => {
            return Err(b.err(
                "give the moving mass as one of 'surface_density_kg_per_m2', 'mass' or 'thickness' + 'density'",
            ))
        }
    };
    let p = &mut b.params;
    let profile = p.string_opt("profile")?.unwrap_or_else(|| "piston".into());
    let kappa = match profile.as_str() {
        "piston" => 1.0,
        "plate" => 9.0 / 5.0,
        "membrane" => 4.0 / 3.0,
        o => return Err(b.err(format!("profile '{o}' must be piston, plate or membrane"))),
    };
    let m_a = kappa * mass / (area * area);
    let p = &mut b.params;
    let f0 = p.positive_opt("resonance", Dim::Frequency)?;
    let c = p.positive_opt("C", Dim::AcousticCompliance)?;
    let compliance = match (f0, c) {
        (Some(f), None) => Some(1.0 / ((2.0 * PI * f).powi(2) * m_a)),
        (None, Some(c)) => Some(c),
        (None, None) => None,
        _ => return Err(b.err("give at most one of 'resonance' and 'C'")),
    };
    let p = &mut b.params;
    let r = p.quantity_opt("R", Dim::AcousticResistance)?;
    let q = p.number_opt("Q")?;
    let resistance = match (r, q, compliance) {
        (Some(r), None, _) if r >= 0.0 => r,
        (Some(_), None, _) => return Err(b.err("'R' must be non-negative")),
        (None, Some(q), Some(c)) if q > 0.0 && q.is_finite() => (m_a / c).sqrt() / q,
        (None, Some(_), Some(_)) => return Err(b.err("'Q' must be positive")),
        (None, Some(_), None) => {
            return Err(b.err("'Q' needs a stiffness ('resonance' or 'C'); give 'R' for a limp panel"))
        }
        (None, None, Some(_)) => {
            return Err(b.err(
                "a panel with a stiffness needs a loss, 'Q' or 'R' (without one it is a short at its resonance)",
            ))
        }
        (None, None, None) => 0.0,
        (Some(_), Some(_), _) => return Err(b.err("give 'Q' or 'R', not both")),
    };
    let model = ShellModel {
        area,
        mass,
        kappa,
        compliance,
        resistance,
    };
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(OnePort {
        id,
        type_name: "shell",
        n1,
        n2,
        y: Box::new(move |cx| model.impedance(cx.omega).inv()),
        limits: Vec::new(),
    }))
}
