//! Cavities (spec Sections 6 and 9).
//!
//! One object whose representation depends on the fidelity level:
//! * L0, or a single node: a lumped compliance with the thermal wall-layer
//!   correction C = V/(γP0)·[1 + ε(1 − j)], ε = (γ−1)·δt·A_wall/(2V).
//! * L1 with two nodes (driver face, far face): a thermoviscous
//!   transmission line along the depth, with the end faces' thermal loss as
//!   shunt admittances so the low-frequency limit equals the L0 element.
//!   At L0 the two faces are joined by an ideal short, so switching levels
//!   never changes the netlist.
//!
//! `modal_cavity` is the L1 closed-form modal expansion of a box or
//! cylinder with N ports of finite footprint (see [`ModalCavity`]); at L0 it
//! collapses to the same lumped compliance with all its ports joined.

use super::{Build, Constructor, Element, FreqCx};
use crate::air::AirState;
use crate::error::{Error, Result};
use crate::mna::{abcd_shunt, potential, Mna, Transfer, Unknown};
use crate::modes::{self, Face, Footprint, Mode, Patch, Shape};
use crate::netlist::Domain;
use crate::thermoviscous::{self, Section};
use crate::units::{Dim, Params};
use crate::validity::{self, ValidityLimit};
use crate::C64;
use serde_json::Value;
use std::any::Any;
use std::f64::consts::PI;
use std::sync::OnceLock;

pub const TYPES: &[&str] = &["cavity", "modal_cavity"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    match ty {
        "cavity" => Some(cavity),
        "modal_cavity" => Some(modal_cavity),
        _ => None,
    }
}

/// Cavity geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Geometry {
    /// Rectangular box, `lz` being the depth (driver face to far face).
    Box { lx: f64, ly: f64, lz: f64 },
    /// Cylinder of given radius and depth.
    Cylinder { radius: f64, depth: f64 },
    /// Volume only, optionally with a depth (circular cross-section assumed).
    Volume { volume: f64, depth: Option<f64> },
}

impl Geometry {
    pub fn volume(&self) -> f64 {
        match *self {
            Geometry::Box { lx, ly, lz } => lx * ly * lz,
            Geometry::Cylinder { radius, depth } => PI * radius * radius * depth,
            Geometry::Volume { volume, .. } => volume,
        }
    }

    pub fn depth(&self) -> Option<f64> {
        match *self {
            Geometry::Box { lz, .. } => Some(lz),
            Geometry::Cylinder { depth, .. } => Some(depth),
            Geometry::Volume { depth, .. } => depth,
        }
    }

    /// Cross-section area and perimeter normal to the depth axis.
    pub fn cross_section(&self) -> Option<(f64, f64)> {
        match *self {
            Geometry::Box { lx, ly, .. } => Some((lx * ly, 2.0 * (lx + ly))),
            Geometry::Cylinder { radius, .. } => Some((PI * radius * radius, 2.0 * PI * radius)),
            Geometry::Volume { volume, depth } => depth.map(|d| {
                let s = volume / d;
                (s, 2.0 * (PI * s).sqrt())
            }),
        }
    }

    /// Total wall area, if the geometry defines it.
    pub fn wall_area(&self) -> Option<f64> {
        let (s, p) = self.cross_section()?;
        Some(2.0 * s + p * self.depth()?)
    }

    /// Default characteristic length for the lumped criterion: the largest
    /// dimension (conservative stand-in for the driver-to-observation distance).
    pub fn largest_dimension(&self) -> f64 {
        match *self {
            Geometry::Box { lx, ly, lz } => lx.max(ly).max(lz),
            Geometry::Cylinder { radius, depth } => (2.0 * radius).max(depth),
            Geometry::Volume { volume, depth } => match depth {
                Some(d) => d.max(2.0 * (volume / d / PI).sqrt()),
                None => 3f64.sqrt() * volume.cbrt(),
            },
        }
    }

    /// First transverse cut-on, above which the depth line misses modes.
    pub fn transverse_cut_on(&self, c: f64) -> Option<f64> {
        match *self {
            Geometry::Box { lx, ly, .. } => Some(c / (2.0 * lx.max(ly))),
            Geometry::Cylinder { radius, .. } => Some(validity::circular_cut_on(radius, c)),
            Geometry::Volume { .. } => self
                .cross_section()
                .map(|(s, _)| validity::circular_cut_on((s / PI).sqrt(), c)),
        }
    }
}

pub struct Cavity {
    pub id: String,
    pub n1: Unknown,
    pub n2: Option<Unknown>,
    pub geometry: Geometry,
    /// Wall area used for thermal loss (already multiplied by the surface factor).
    pub wall_area: f64,
    pub surface_factor: f64,
    pub wall_loss: bool,
    pub max_distance: f64,
}

/// Lumped compliance of a volume V with thermal wall loss on `wall_area`:
/// C = V/(γP0)·[1 + ε(1 − j)], ε = (γ − 1)·δt·A/(2V) (spec Section 6).
pub fn wall_layer_compliance(
    air: &AirState,
    omega: f64,
    volume: f64,
    wall_area: f64,
    wall_loss: bool,
) -> C64 {
    let c0 = volume / air.bulk_modulus();
    if !wall_loss {
        return C64::new(c0, 0.0);
    }
    let eps = (air.gamma - 1.0) * air.thermal_layer(omega) * wall_area / (2.0 * volume);
    c0 * (C64::new(1.0, 0.0) + eps * C64::new(1.0, -1.0))
}

impl Cavity {
    /// Lumped compliance C(ω) including the thermal wall layer.
    pub fn compliance(&self, air: &AirState, omega: f64) -> C64 {
        wall_layer_compliance(
            air,
            omega,
            self.geometry.volume(),
            self.wall_area,
            self.wall_loss,
        )
    }

    fn distributed(&self, level: u8) -> bool {
        level >= 1 && self.n2.is_some()
    }

    /// Transfer matrix face 1 → face 2 at L1.
    fn line_transfer(&self, air: &AirState, omega: f64) -> Transfer {
        let (s, p) = self
            .geometry
            .cross_section()
            .expect("two-node cavity has a depth");
        let d = self.geometry.depth().expect("two-node cavity has a depth");
        let jw = C64::new(0.0, omega);
        if !self.wall_loss {
            let k = omega / air.c;
            let zc = air.rho_c() / s;
            let (ch, sh) = (C64::new((k * d).cos(), 0.0), C64::new(0.0, (k * d).sin()));
            return Transfer::from([ch, zc * sh, sh / zc, ch]);
        }
        let section = Section::Equivalent {
            area: s,
            perimeter: p * self.surface_factor,
        };
        let line = thermoviscous::transfer(&section, air, omega, d);
        // Thermal loss on each end face: extra compliance
        // (γ−1)·δt·S·(1 − j)/(2γP0), as a shunt admittance.
        let dc = (air.gamma - 1.0) * air.thermal_layer(omega) * s * self.surface_factor
            / (2.0 * air.bulk_modulus());
        let y_end = jw * dc * C64::new(1.0, -1.0);
        let end = Transfer::from(abcd_shunt(y_end));
        end * line * end
    }
}

impl Element for Cavity {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "cavity"
    }
    fn branch_count(&self) -> usize {
        if self.n2.is_some() {
            2
        } else {
            0
        }
    }
    fn port_count(&self) -> usize {
        if self.n2.is_some() {
            2
        } else {
            1
        }
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        match self.n2 {
            Some(n2) if self.distributed(cx.level) => {
                mna.two_port(
                    (self.n1, None),
                    (n2, None),
                    (br[0], br[1]),
                    &self.line_transfer(cx.air, cx.omega),
                );
            }
            Some(n2) => {
                mna.admittance(self.n1, None, cx.jw() * self.compliance(cx.air, cx.omega));
                mna.short(self.n1, n2, br[0]);
                // Keep the second branch unknown determined.
                mna.add(Some(br[1]), Some(br[1]), C64::new(1.0, 0.0));
            }
            None => {
                mna.admittance(self.n1, None, cx.jw() * self.compliance(cx.air, cx.omega));
            }
        }
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        match (port, self.n2) {
            (0, _) => Some(potential(x, self.n1)),
            (1, Some(n2)) => Some(potential(x, n2)),
            _ => None,
        }
    }
    /// Flow entering the cavity at each face (to ambient).
    fn port_flow(&self, cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        let lumped = cx.jw() * self.compliance(cx.air, cx.omega) * potential(x, self.n1);
        match (port, self.n2) {
            (0, None) => Some(lumped),
            // L1 depth line: branch 0 enters face 1, branch 1 leaves face 2.
            (0, Some(_)) if self.distributed(cx.level) => Some(x[br[0]]),
            (1, Some(_)) if self.distributed(cx.level) => Some(-x[br[1]]),
            // L0: the short's branch unknown is the flow it delivers into
            // face 1, i.e. it carries −x[br0] from face 1 to face 2.
            (0, Some(_)) => Some(lumped - x[br[0]]),
            (1, Some(_)) => Some(x[br[0]]),
            _ => None,
        }
    }
    fn validity(&self, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        if self.distributed(level) {
            let cut = self.geometry.transverse_cut_on(air.c);
            vec![ValidityLimit {
                element: self.id.clone(),
                criterion: "cavity depth line: first transverse mode",
                begin_hz: cut.map(|f| 0.7 * f),
                deep_hz: cut,
                ..Default::default()
            }]
        } else {
            vec![validity::lumped_cavity(&self.id, self.max_distance, air.c)]
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn cavity(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let n1 = b.terminal(0, Domain::Acoustic)?;
    let n2 = if b.terminals.len() == 2 {
        Some(b.terminal(1, Domain::Acoustic)?)
    } else {
        None
    };
    let p = &mut b.params;
    let lx = p.positive_opt("lx", Dim::Length)?;
    let ly = p.positive_opt("ly", Dim::Length)?;
    let lz = p.positive_opt("lz", Dim::Length)?;
    let radius = p.positive_opt("radius", Dim::Length)?;
    let depth = p.positive_opt("depth", Dim::Length)?;
    let volume = p.positive_opt("volume", Dim::Volume)?;
    let geometry = match (lx, ly, lz, radius, volume) {
        (Some(lx), Some(ly), Some(lz), None, None) if depth.is_none() => Geometry::Box { lx, ly, lz },
        (None, None, None, Some(radius), None) => Geometry::Cylinder {
            radius,
            depth: depth.ok_or_else(|| b.err("cylinder needs 'depth'"))?,
        },
        (None, None, None, None, Some(volume)) => Geometry::Volume { volume, depth },
        _ => {
            return Err(b.err(
                "give exactly one geometry: lx/ly/lz (box), radius+depth (cylinder), or volume [+depth]",
            ))
        }
    };
    let p = &mut b.params;
    let surface_factor = p.number_or("surface_factor", 1.0)?;
    let wall_loss = p.bool_or("wall_loss", true)?;
    let wall_area_override = p.positive_opt("wall_area", Dim::Area)?;
    let max_distance = p.positive_opt("max_distance", Dim::Length)?;
    if !(1.0..=10.0).contains(&surface_factor) {
        return Err(b.err("'surface_factor' must be between 1 and 10"));
    }
    if n2.is_some() && geometry.depth().is_none() {
        return Err(b.err("a two-node cavity needs a depth"));
    }
    let wall_area = match (wall_area_override, geometry.wall_area()) {
        (Some(a), _) => a,
        (None, Some(a)) => a,
        // Volume only: wall area of a cube of the same volume.
        (None, None) => 6.0 * geometry.volume().powf(2.0 / 3.0),
    } * surface_factor;
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(Cavity {
        id,
        n1,
        n2,
        geometry,
        wall_area,
        surface_factor,
        wall_loss,
        max_distance: max_distance.unwrap_or_else(|| geometry.largest_dimension()),
    }))
}

// ----- Modal cavity (L1 closed-form modal expansion) ---------------------

/// Default highest analysed frequency of a modal cavity: the top of the
/// engine's internal range (spec Section 2, rule 6).
pub const MODAL_DEFAULT_F_MAX: f64 = 40_000.0;

/// Eigenwavenumbers are kept below this multiple of the highest analysed
/// wavenumber (spec Section 9; erratum E27 for the resulting counts).
pub const MODAL_TRUNCATION_FACTOR: f64 = 3.0;

/// Pairs of ports on walls with different normals take the quasi-static
/// share of the modes up to this multiple of the truncation...
pub const MODAL_CROSS_FACTOR: f64 = 4.0;
/// ...unless that exceeds this many modes (build cost), in which case the
/// multiple shrinks to fit (never below 1).
pub const MODAL_CROSS_BUDGET: f64 = 2.0e5;

/// Largest number of retained modes (Weyl estimate) a modal cavity accepts.
/// A numerical budget, not physical data: 50× the ≈10k modes of a
/// 60 × 45 × 20 mm box at the default 40 kHz, i.e. volumes up to about 2.7 L
/// at 40 kHz. Beyond it the mode list alone would need hundreds of
/// megabytes (and each frequency milliseconds), so the element asks for a
/// lower `f_max_Hz` instead.
pub const MODAL_MAX_MODES: f64 = 5.0e5;

/// Modes whose mean over every port footprint is below this (the modes are
/// normalised to unit mean square) are dropped: a centred piston does not
/// couple to antisymmetric modes, and their terms would only add rounding.
pub const MODAL_COUPLING_FLOOR: f64 = 1e-12;

/// One port of a modal cavity: a node and its footprint on a wall.
#[derive(Debug, Clone)]
pub struct ModalPort {
    pub node_name: String,
    pub node: Unknown,
    pub patch: Patch,
}

/// Precomputed, frequency-independent data of a modal cavity.
#[derive(Debug, Clone)]
pub struct ModalData {
    /// Retained modes (k > 0, below the truncation, coupled to some port).
    pub modes: Vec<Mode>,
    /// Number of non-uniform modes below the truncation, before dropping
    /// uncoupled ones.
    pub modes_below_cutoff: usize,
    /// k_n² of the retained modes.
    pub kn2: Vec<f64>,
    /// Thermal wall-loss coefficient s·∮φ²dS/(2V) per retained mode (1/m).
    pub thermal: Vec<f64>,
    /// Viscous wall-loss coefficient s·∮|∇_tφ|²dS/(2V) per retained mode (1/m³).
    pub viscous: Vec<f64>,
    /// Footprint means φ̄ of the retained modes, `[port][mode]`.
    pub couplings: Vec<Vec<f64>>,
    /// φ̄_i·φ̄_j per mode for i ≤ j (row-major upper triangle), contiguous.
    pub pairs: Vec<f64>,
    /// Quasi-static residual of the truncated modes, Σ φ̄_iφ̄_j/k_n² (m²),
    /// P × P row-major: all omitted modes for ports whose walls share a
    /// normal, the modes between the cutoff and `k_cross` otherwise.
    pub residual1: Vec<f64>,
    /// Second-order residual Σ φ̄_iφ̄_j/k_n⁴ (m⁴).
    pub residual2: Vec<f64>,
    /// Transverse cutoff used for the residual of each port (rad/m).
    pub k_transverse: Vec<f64>,
    /// Upper eigenwavenumber of the cross-wall residual (rad/m; 0 if unused).
    pub k_cross: f64,
}

/// N-port rigid-wall modal model of a box or cylinder (spec Section 9, "L1:
/// closed-form modal expansion").
///
/// # Model
///
/// Port j is a footprint S_j on a wall through which the volume velocity
/// U_j enters the cavity with uniform normal velocity (a rigid piston, or a
/// small aperture); the port pressure is the mean pressure over S_j, which
/// makes Σ Re(p̄_j·conj(U_j)) the exact power delivered. With modes φ_n of
/// unit mean square (see [`crate::modes`]) and φ̄_{n,j} their mean over S_j,
/// Green's second identity gives (e^{+jωt}, k = ω/c)
///
///   Z_ij = p̄_i/U_j = (jωρ/V)·Σ_n φ̄_{n,i}·φ̄_{n,j} / (k_n² − k² + D_n),
///
/// which is symmetric by construction (reciprocity). The uniform mode
/// (k₀ = 0, φ̄ = 1) gives exactly the lumped compliance, Z = 1/(jωC), so L0
/// is the first term of this sum. It is evaluated with the same function as
/// the `cavity` element, so the two agree exactly; the other terms use
/// ρ = γP0/c².
///
/// # Wall loss
///
/// The boundary layers of a rigid isothermal wall act as a normal specific
/// admittance (outward velocity over p/ρc)
/// β = ½(1 + j)·[(γ − 1)·k·δt + (k_t²/k)·δv], with k_t the wavenumber
/// tangential to the wall: the thermal term is the excess compressibility of
/// the thermal layer (the same (γ − 1)·δt·(1 − j)/2 as the lumped wall
/// correction), the viscous term the displacement flux of the viscous layer
/// driven by the tangential pressure gradient (Morse & Ingard, *Theoretical
/// Acoustics*, 1968; the same result is the standard boundary-layer
/// admittance β = ½(1 − i)k[sin²φ·d_v + (γ − 1)d_h] for e^{−iωt}). Keeping
/// only the diagonal term of the first-order perturbation:
///
///   D_n = (j − 1)·η_n,  η_n = s·[(γ − 1)·k²·δt·∮φ_n² dS + δv·∮|∇_tφ_n|² dS] / (2V),
///
/// with s the surface factor (and wall-area override) of the `cavity`
/// element. For the uniform mode this is exactly the lumped C·[1 + ε(1 − j)].
/// The dropped off-diagonal terms (modes whose traces are not orthogonal on
/// a wall, e.g. the axial modes on the end faces) matter only where the
/// modal amplitudes cancel: at the anti-resonance notches of a depth line the
/// result differs from the thermoviscous line by about 1e-3·ρc/S, and by
/// less than 1e-3 relative elsewhere (test `depth_line_agrees_with_axial_modes`).
/// The modal quality factor is Q_n = k_n²/η_n(k_n): 200–400 for the first
/// modes of a 60 × 45 × 20 mm box and 280–540 for a 25 × 25 mm cylinder.
/// Erratum E29's 500–1000 is the thermal part alone (1/ε at the mode
/// frequency); the viscous layer roughly halves it for modes with tangential
/// motion. Bulk (classical and relaxation) absorption in the air is not
/// modelled.
///
/// # Truncation and residual
///
/// Modes are kept with k_n below 3× the highest analysed wavenumber
/// 2π·f_max/c (spec Section 9; erratum E27 gives ≈1.4k modes to 20 kHz and
/// ≈10k to 40 kHz for a 60 × 45 × 20 mm box). The omitted modes are not
/// negligible for a driving-point impedance: the sum over the index normal
/// to a port converges only as 1/N, so plain truncation leaves an error of
/// about (2/3π)·(k/k_max)·ρc/S for a full-face piston, and 10–100 % in the
/// driving-point impedance of a small aperture, whose near-field mass sits
/// in the omitted modes (see the test `box_impedance_matches_mixed_
/// representation`). Their contribution is therefore added quasi-statically:
///
///   Z_ij += (jωρ/V)·[R1_ij + k²·R2_ij],  R_s = Σ_{omitted} φ̄_iφ̄_j/k_n^{2s},
///
/// which leaves an error of relative order (k/k_n)⁴ in each omitted term,
/// at most (1/3)⁴ at f_max. R1 and R2 are the
/// all-mode sums of [`crate::modes::static_sums`] minus the retained modes'
/// share, for ports whose walls share a normal (box faces normal to the same
/// axis, cylinder end–end or side–side); their continuum part carries the
/// footprint's mirror images in nearby walls, so a slit along an edge is as
/// accurate as one in mid-face (test
/// `impedance_near_walls_matches_tail_free_reference`). Mutual terms of two
/// footprints on the same face have no continuum part: two 0.3 mm slits
/// 0.2 mm apart get a mutual impedance about 1e-3 off (against 1e-5 for
/// footprints millimetres apart). For ports on walls with different
/// normals (e.g. a driver on z0 and a leak on a side wall)
/// the omitted terms are damped by both footprints, and their quasi-static
/// share is summed explicitly over the modes from the cutoff up to 4× it
/// (fewer if that would exceed 2·10⁵ modes). The residual is reactive, so
/// passivity is unaffected. `"residual": false` disables it (diagnostics).
///
/// # Stamp
///
/// Each port k has a branch unknown I_k, the flow entering the cavity from
/// node n_k, with the branch equation p(n_k) − Σ_l Z_kl·I_l = 0. At level 0
/// Z_kl = 1/(jωC) for every pair: one lumped compliance with all port nodes
/// joined, so switching levels never changes the netlist.
pub struct ModalCavity {
    pub id: String,
    pub shape: Shape,
    pub ports: Vec<ModalPort>,
    pub wall_loss: bool,
    /// Wall area for the thermal loss of the uniform mode (surface factor included).
    pub wall_area: f64,
    /// Scale of every wall integral: `wall_area` / geometric wall area.
    pub loss_scale: f64,
    /// Highest analysed frequency (Hz).
    pub f_max: f64,
    /// Eigenwavenumber cutoff (rad/m), 3·2π·f_max/c at build.
    pub k_cut: f64,
    /// Whether the quasi-static residual of the omitted modes is added.
    pub residual: bool,
    /// Length of the lumped-validity criterion at L0.
    pub max_distance: f64,
    data: OnceLock<ModalData>,
}

impl ModalCavity {
    /// Lumped compliance of the whole volume with thermal wall loss; the
    /// uniform-mode term of the modal sum is 1/(jω·this).
    pub fn lumped_compliance(&self, air: &AirState, omega: f64) -> C64 {
        wall_layer_compliance(
            air,
            omega,
            self.shape.volume(),
            self.wall_area,
            self.wall_loss,
        )
    }

    /// Precomputed modal data (built on first use, so that L0 stays cheap).
    pub fn data(&self) -> &ModalData {
        self.data.get_or_init(|| self.build_data())
    }

    fn build_data(&self) -> ModalData {
        let patches: Vec<Patch> = self.ports.iter().map(|p| p.patch).collect();
        let all = self.shape.modes_below(self.k_cut);
        let coup_all = modes::patch_averages(&self.shape, &all, &patches);
        let v = self.shape.volume();
        let p = patches.len();
        let mut data = ModalData {
            modes: Vec::new(),
            modes_below_cutoff: 0,
            kn2: Vec::new(),
            thermal: Vec::new(),
            viscous: Vec::new(),
            couplings: vec![Vec::new(); p],
            pairs: Vec::new(),
            residual1: vec![0.0; p * p],
            residual2: vec![0.0; p * p],
            k_transverse: vec![0.0; p],
            k_cross: 0.0,
        };
        for (n, md) in all.iter().enumerate() {
            if md.k == 0.0 {
                continue;
            }
            data.modes_below_cutoff += 1;
            if coup_all.iter().all(|c| c[n].abs() < MODAL_COUPLING_FLOOR) {
                continue;
            }
            let (i_n, j_n) = md.wall_integrals(&self.shape);
            data.modes.push(*md);
            data.kn2.push(md.k * md.k);
            data.thermal.push(self.loss_scale * i_n / (2.0 * v));
            data.viscous.push(self.loss_scale * j_n / (2.0 * v));
            for (i, c) in coup_all.iter().enumerate() {
                data.couplings[i].push(c[n]);
            }
            for i in 0..p {
                for j in i..p {
                    data.pairs.push(coup_all[i][n] * coup_all[j][n]);
                }
            }
        }
        if self.residual {
            let st = modes::static_sums(&self.shape, &patches, self.k_cut);
            data.k_transverse = st.k_transverse.clone();
            for i in 0..p {
                for j in 0..p {
                    if !st.known[i * p + j] {
                        continue;
                    }
                    let (mut k1, mut k2) = (0.0, 0.0);
                    for (n, &kn2) in data.kn2.iter().enumerate() {
                        let w = data.couplings[i][n] * data.couplings[j][n];
                        k1 += w / kn2;
                        k2 += w / (kn2 * kn2);
                    }
                    data.residual1[i * p + j] = st.s1[i * p + j] - k1;
                    data.residual2[i * p + j] = st.s2[i * p + j] - k2;
                }
            }
            // Pairs on walls with different normals: quasi-static share of
            // the modes between the cutoff and a larger one.
            let cross: Vec<(usize, usize)> = (0..p)
                .flat_map(|i| (i + 1..p).map(move |j| (i, j)))
                .filter(|&(i, j)| !st.known[i * p + j])
                .collect();
            if !cross.is_empty() {
                let factor = MODAL_CROSS_FACTOR
                    .min((MODAL_CROSS_BUDGET / all.len().max(1) as f64).cbrt())
                    .max(1.0);
                data.k_cross = self.k_cut * factor;
                let extra: Vec<Mode> = self
                    .shape
                    .modes_below(data.k_cross)
                    .into_iter()
                    .filter(|m| m.k >= self.k_cut)
                    .collect();
                let coup = modes::patch_averages(&self.shape, &extra, &patches);
                for (n, md) in extra.iter().enumerate() {
                    let kn2 = md.k * md.k;
                    for &(i, j) in &cross {
                        let w = coup[i][n] * coup[j][n];
                        data.residual1[i * p + j] += w / kn2;
                        data.residual2[i * p + j] += w / (kn2 * kn2);
                    }
                }
                for &(i, j) in &cross {
                    data.residual1[j * p + i] = data.residual1[i * p + j];
                    data.residual2[j * p + i] = data.residual2[i * p + j];
                }
            }
        }
        data
    }

    /// Thermal and viscous parts of η for a mode at angular frequency ω
    /// (1/m²); the modal denominator is k_n² − k² + (j − 1)·η.
    pub fn loss_parts(&self, mode: &Mode, air: &AirState, omega: f64) -> (f64, f64) {
        if !self.wall_loss {
            return (0.0, 0.0);
        }
        let (i_n, j_n) = mode.wall_integrals(&self.shape);
        let v = self.shape.volume();
        let k = omega / air.c;
        (
            self.loss_scale * (air.gamma - 1.0) * k * k * air.thermal_layer(omega) * i_n
                / (2.0 * v),
            self.loss_scale * air.viscous_layer(omega) * j_n / (2.0 * v),
        )
    }

    /// Modal denominator k_n² − k² + (j − 1)·η_n(ω) of any mode (including
    /// the uniform one).
    pub fn denominator(&self, mode: &Mode, air: &AirState, omega: f64) -> C64 {
        let (t, v) = self.loss_parts(mode, air, omega);
        let eta = t + v;
        let k = omega / air.c;
        C64::new(mode.k * mode.k - k * k - eta, eta)
    }

    /// Wall-loss quality factor of a mode at its own frequency, k_n²/η_n.
    pub fn quality_factor(&self, mode: &Mode, air: &AirState) -> f64 {
        let (t, v) = self.loss_parts(mode, air, mode.k * air.c);
        mode.k * mode.k / (t + v)
    }

    /// The prefactor jωρ/V of the modal sum, with ρ = γP0/c².
    pub fn prefactor(&self, air: &AirState, omega: f64) -> C64 {
        let rho = air.bulk_modulus() / (air.c * air.c);
        C64::new(0.0, omega * rho / self.shape.volume())
    }

    /// Port impedance matrix Z (P × P, row-major; Pa·s/m³) at level `level`.
    pub fn impedance_matrix(&self, air: &AirState, omega: f64, level: u8) -> Vec<C64> {
        let p = self.ports.len();
        let z0 = (C64::new(0.0, omega) * self.lumped_compliance(air, omega)).inv();
        if level == 0 {
            return vec![z0; p * p];
        }
        let d = self.data();
        let np = p * (p + 1) / 2;
        let k = omega / air.c;
        let k2 = k * k;
        let (dv, dt) = (air.viscous_layer(omega), air.thermal_layer(omega));
        let gt = (air.gamma - 1.0) * k2 * dt;
        let mut acc = vec![C64::new(0.0, 0.0); np];
        for (n, row) in d.pairs.chunks_exact(np).enumerate() {
            let eta = if self.wall_loss {
                gt * d.thermal[n] + dv * d.viscous[n]
            } else {
                0.0
            };
            let g = C64::new(d.kn2[n] - k2 - eta, eta).inv();
            for (a, &w) in acc.iter_mut().zip(row) {
                *a += g * w;
            }
        }
        let pre = self.prefactor(air, omega);
        let mut z = vec![C64::new(0.0, 0.0); p * p];
        let mut idx = 0;
        for i in 0..p {
            for j in i..p {
                let r = d.residual1[i * p + j] + k2 * d.residual2[i * p + j];
                let zij = pre * (acc[idx] + r) + z0;
                z[i * p + j] = zij;
                z[j * p + i] = zij;
                idx += 1;
            }
        }
        z
    }
}

impl Element for ModalCavity {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "modal_cavity"
    }
    fn branch_count(&self) -> usize {
        self.ports.len()
    }
    fn port_count(&self) -> usize {
        self.ports.len()
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        let z = self.impedance_matrix(cx.air, cx.omega, cx.level);
        let p = self.ports.len();
        for (k, port) in self.ports.iter().enumerate() {
            let b = Some(br[k]);
            // KCL: I_k leaves node n_k into the cavity.
            mna.add(port.node, b, C64::new(1.0, 0.0));
            // p(n_k) − Σ_l Z_kl·I_l = 0.
            mna.add(b, port.node, C64::new(1.0, 0.0));
            for l in 0..p {
                mna.add(b, Some(br[l]), -z[k * p + l]);
            }
        }
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        self.ports.get(port).map(|p| potential(x, p.node))
    }
    /// Volume velocity entering the cavity through port `port`.
    fn port_flow(&self, _cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        (port < self.ports.len()).then(|| x[br[port]])
    }
    fn validity(&self, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        if level == 0 {
            vec![validity::lumped_cavity(&self.id, self.max_distance, air.c)]
        } else {
            // Above f_max the retained modes no longer reach 3× the analysed
            // wavenumber; at 3·f_max the quasi-static residual breaks down.
            vec![ValidityLimit {
                element: self.id.clone(),
                criterion: "modal cavity: mode truncation (3x highest analysed wavenumber)",
                begin_hz: Some(self.f_max),
                deep_hz: Some(MODAL_TRUNCATION_FACTOR * self.f_max),
                ..Default::default()
            }]
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Face names, position keys and rectangle-extent keys of each wall.
fn face_spec(
    shape: &Shape,
    name: &str,
) -> Option<(Face, &'static str, &'static str, [&'static str; 2])> {
    let bx = |axis: usize, far: bool| {
        let (u, v, e) = match axis {
            0 => ("y", "z", ["ly", "lz"]),
            1 => ("x", "z", ["lx", "lz"]),
            _ => ("x", "y", ["lx", "ly"]),
        };
        (Face::Box { axis, far }, u, v, e)
    };
    Some(match (shape, name) {
        (Shape::Box { .. }, "x0") => bx(0, false),
        (Shape::Box { .. }, "x1") => bx(0, true),
        (Shape::Box { .. }, "y0") => bx(1, false),
        (Shape::Box { .. }, "y1") => bx(1, true),
        (Shape::Box { .. }, "z0") => bx(2, false),
        (Shape::Box { .. }, "z1") => bx(2, true),
        (Shape::Cylinder { .. }, "z0") => (Face::End { far: false }, "x", "y", ["lx", "ly"]),
        (Shape::Cylinder { .. }, "z1") => (Face::End { far: true }, "x", "y", ["lx", "ly"]),
        (Shape::Cylinder { .. }, "side") => (Face::Side, "angle", "z", ["arc", "lz"]),
        _ => return None,
    })
}

/// Parses one entry of `ports`.
fn parse_port(id: &str, i: usize, v: &Value, shape: &Shape) -> Result<(String, Patch)> {
    let ctx = format!("{id}.ports[{i}]");
    let obj = v
        .as_object()
        .ok_or_else(|| Error::element(&ctx, "each port must be an object"))?;
    let mut pp = Params::new(ctx.clone(), obj.clone());
    let node = pp
        .string_opt("node")?
        .ok_or_else(|| Error::element(&ctx, "missing 'node'"))?;
    let face_name = pp
        .string_opt("face")?
        .ok_or_else(|| Error::element(&ctx, "missing 'face'"))?;
    let (face, ukey, vkey, [ea, eb]) = face_spec(shape, &face_name).ok_or_else(|| {
        let allowed = match shape {
            Shape::Box { .. } => "x0, x1, y0, y1, z0 or z1",
            Shape::Cylinder { .. } => "z0, z1 or side",
        };
        Error::element(
            &ctx,
            format!("unknown face '{face_name}', expected {allowed}"),
        )
    })?;
    let u = if face == Face::Side {
        let Shape::Cylinder { radius, .. } = *shape else {
            unreachable!()
        };
        radius * pp.number("angle_deg")?.to_radians()
    } else {
        pp.quantity(ukey, Dim::Length)?
    };
    let v = pp.quantity(vkey, Dim::Length)?;
    let radius = pp.positive_opt("radius", Dim::Length)?;
    let diameter = pp.positive_opt("diameter", Dim::Length)?;
    let ext_a = pp.positive_opt(ea, Dim::Length)?;
    let ext_b = pp.positive_opt(eb, Dim::Length)?;
    let footprint = match (radius, diameter, ext_a, ext_b) {
        (Some(r), None, None, None) => Footprint::Disk { radius: r },
        (None, Some(d), None, None) => Footprint::Disk { radius: 0.5 * d },
        (None, None, Some(du), Some(dv)) => Footprint::Rect { du, dv },
        _ => {
            return Err(Error::element(
                &ctx,
                format!(
                    "give one footprint: 'radius_mm' (or 'diameter_mm') for a disk, \
                     or '{ea}_mm' and '{eb}_mm' for a rectangle on face '{face_name}'"
                ),
            ))
        }
    };
    pp.finish()?;
    let patch = Patch {
        face,
        u,
        v,
        footprint,
    };
    patch.check(shape).map_err(|m| Error::element(&ctx, m))?;
    Ok((node, patch))
}

fn modal_cavity(mut b: Build) -> Result<Box<dyn Element>> {
    let id = b.id.clone();
    let err = |m: String| Error::element(id.clone(), m);
    if !b.terminals.is_empty() {
        return Err(err(
            "modal_cavity takes its nodes from 'ports'; remove 'node'/'nodes'".into(),
        ));
    }
    let p = &mut b.params;
    let lx = p.positive_opt("lx", Dim::Length)?;
    let ly = p.positive_opt("ly", Dim::Length)?;
    let lz = p.positive_opt("lz", Dim::Length)?;
    let radius = p.positive_opt("radius", Dim::Length)?;
    let depth = p.positive_opt("depth", Dim::Length)?;
    let volume = p.positive_opt("volume", Dim::Volume)?;
    let shape = match (lx, ly, lz, radius, depth, volume) {
        (Some(lx), Some(ly), Some(lz), None, None, None) => Shape::Box { lx, ly, lz },
        (None, None, None, Some(radius), Some(depth), None) => Shape::Cylinder { radius, depth },
        _ => {
            return Err(err(
                "give exactly one geometry: lx/ly/lz (box) or radius + depth (cylinder); \
                 a volume alone has no modes"
                    .into(),
            ))
        }
    };
    let surface_factor = p.number_or("surface_factor", 1.0)?;
    let wall_loss = p.bool_or("wall_loss", true)?;
    let wall_area_override = p.positive_opt("wall_area", Dim::Area)?;
    let max_distance = p.positive_opt("max_distance", Dim::Length)?;
    let f_max = p
        .positive_opt("f_max", Dim::Frequency)?
        .unwrap_or(MODAL_DEFAULT_F_MAX);
    let residual = p.bool_or("residual", true)?;
    let ports_json = p.value_opt("ports");
    if !(1.0..=10.0).contains(&surface_factor) {
        return Err(err("'surface_factor' must be between 1 and 10".into()));
    }
    let list = match ports_json {
        Some(Value::Array(a)) if !a.is_empty() => a,
        Some(_) => return Err(err("'ports' must be a non-empty array".into())),
        None => return Err(err("missing 'ports'".into())),
    };
    let mut ports = Vec::with_capacity(list.len());
    for (i, v) in list.iter().enumerate() {
        let (node_name, patch) = parse_port(&id, i, v, &shape)?;
        let node = b
            .nodes
            .resolve(&node_name, Domain::Acoustic)
            .map_err(|e| err(format!("ports[{i}]: {e}")))?;
        ports.push(ModalPort {
            node_name,
            node,
            patch,
        });
    }
    for i in 0..ports.len() {
        for j in 0..i {
            if ports[i].node == ports[j].node {
                return Err(err(format!(
                    "ports[{j}] and ports[{i}] share node '{}'; each port needs its own node",
                    ports[i].node_name
                )));
            }
            if ports[i].patch.overlaps(&ports[j].patch, &shape) {
                return Err(err(format!(
                    "the footprints of ports[{j}] and ports[{i}] overlap"
                )));
            }
        }
    }
    let c = b.air.c;
    b.finish()?;
    let k_cut = MODAL_TRUNCATION_FACTOR * 2.0 * PI * f_max / c;
    let estimate = shape.mode_count_estimate(k_cut);
    if estimate > MODAL_MAX_MODES {
        return Err(err(format!(
            "'f_max' of {f_max:.0} Hz keeps about {estimate:.0} modes in this cavity \
             (limit {MODAL_MAX_MODES:.0}); lower 'f_max_Hz'"
        )));
    }
    let geometric = shape.wall_area();
    let wall_area = wall_area_override.unwrap_or(geometric) * surface_factor;
    Ok(Box::new(ModalCavity {
        id,
        shape,
        ports,
        wall_loss,
        wall_area,
        loss_scale: wall_area / geometric,
        f_max,
        k_cut,
        residual,
        max_distance: max_distance.unwrap_or_else(|| shape.largest_dimension()),
        data: OnceLock::new(),
    }))
}
