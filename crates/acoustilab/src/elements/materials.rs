//! Acoustic materials: meshes and screens, perforated plates, porous layers,
//! filled cavities and membrane vents (spec Sections 6, 8 and 13, App. C6).
//!
//! All are passive series or shunt impedances computed from datasheet
//! values and geometry:
//!
//! * `mesh` — a specific flow resistance R_s (rayl) over an area A, exactly
//!   R_s/A at DC, with the mass and √f rise of a pore-level model calibrated
//!   to R_s: equivalent cylindrical pores whose radius follows from R_s,
//!   thickness and open area by Poiseuille's law, each a thermoviscous tube
//!   with interaction-reduced end corrections.
//! * `perforated_plate` — Crandall's exact circular-hole impedance (the
//!   thermoviscous tube) plus Ingard's end corrections reduced by the Fok
//!   interaction function, over an area with a given open-area ratio. Maa's
//!   (1998) approximate formula is provided for comparison.
//! * `porous_layer` — a rigid-frame equivalent fluid (Johnson–Champoux–
//!   Allard, Lafarge's JCAL extension, or the one-parameter Delany–Bazley
//!   and Miki laws) as a slab two-port or a rigid-backed surface.
//! * `fill` — the thermal relaxation of fibrous fill as the extra cavity
//!   compliance C_ad·(γ−1)·φ/(1 + jωτ) (spec App. C6, after Leach 1989),
//!   with τ from fibre diameter and density (Tarnow 1996 cell model,
//!   Lafarge et al. 1997 thermal permeability) or given directly.
//! * `membrane_vent` — a porous membrane: its flow resistance in parallel
//!   with the mass and compliance of the membrane moving as a whole.
//!
//! Material entries with provenance live in `data/materials/*.json` and are
//! embedded at compile time; `material: "<id>"` fills in their parameters.

use super::ducts::{surface_resistance, EndResistance};
use super::{
    particle_velocity_check, Annotated, Build, Constructor, Element, FreqCx, OnePort, TwoPort,
};
use crate::air::AirState;
use crate::diag::{Note, Operating, Severity};
use crate::error::{Error, Result};
use crate::mna::{potential, Mna, Unknown};
use crate::netlist::Domain;
use crate::thermoviscous::{self, Section};
use crate::units::{Dim, Params};
use crate::validity::{self, ValidityLimit};
use crate::C64;
use serde_json::{Map, Value};
use std::any::Any;
use std::f64::consts::PI;
use std::sync::OnceLock;

pub const TYPES: &[&str] = &[
    "mesh",
    "perforated_plate",
    "porous_layer",
    "fill",
    "membrane_vent",
];

pub fn constructor(ty: &str) -> Option<Constructor> {
    Some(match ty {
        "mesh" => mesh,
        "perforated_plate" => perforated_plate,
        "porous_layer" => porous_layer,
        "fill" => fill,
        "membrane_vent" => membrane_vent,
        _ => return None,
    })
}

// ----- Material database -----------------------------------------------------

/// The embedded database files (name, JSON text).
pub const DATABASE_FILES: &[(&str, &str)] = &[
    (
        "meshes.json",
        include_str!("../../../../data/materials/meshes.json"),
    ),
    (
        "porous.json",
        include_str!("../../../../data/materials/porous.json"),
    ),
];

/// One material entry: parameters (unit-suffixed element keys) and
/// provenance metadata.
#[derive(Debug, Clone)]
pub struct MaterialEntry {
    pub id: String,
    /// `mesh`, `fabric` or `porous`.
    pub kind: String,
    pub params: Map<String, Value>,
    /// `measured`, `datasheet`, `estimated` or `unverified`.
    pub status: String,
    pub method: String,
    pub state: String,
    pub tolerance: String,
    pub source: String,
    pub licence: String,
}

fn parse_database() -> std::result::Result<Vec<MaterialEntry>, String> {
    let mut out: Vec<MaterialEntry> = Vec::new();
    for (name, text) in DATABASE_FILES {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
        let entries = v
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{name}: missing 'entries'"))?;
        for e in entries {
            let field = |k: &str| -> std::result::Result<String, String> {
                e.get(k)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| format!("{name}: entry lacks string '{k}': {e}"))
            };
            let id = field("id")?;
            if out.iter().any(|x| x.id == id) {
                return Err(format!("{name}: duplicate material id '{id}'"));
            }
            let status = field("status")?;
            if !["measured", "datasheet", "estimated", "unverified"].contains(&status.as_str()) {
                return Err(format!("{name}: '{id}' has unknown status '{status}'"));
            }
            out.push(MaterialEntry {
                kind: field("kind")?,
                params: e
                    .get("params")
                    .and_then(Value::as_object)
                    .cloned()
                    .ok_or_else(|| format!("{name}: '{id}' lacks 'params'"))?,
                status,
                method: field("method")?,
                state: field("state")?,
                tolerance: field("tolerance")?,
                source: field("source")?,
                licence: field("licence")?,
                id,
            });
        }
    }
    Ok(out)
}

/// All embedded material entries. The database is validated by the test
/// suite; a malformed entry is reported here as an error.
pub fn database() -> std::result::Result<&'static [MaterialEntry], String> {
    static DB: OnceLock<std::result::Result<Vec<MaterialEntry>, String>> = OnceLock::new();
    DB.get_or_init(parse_database)
        .as_ref()
        .map(|v| v.as_slice())
        .map_err(Clone::clone)
}

/// Looks up a material by id and checks its kind.
pub fn material(id: &str, kinds: &[&str]) -> std::result::Result<&'static MaterialEntry, String> {
    let db = database()?;
    let e = db
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| format!("unknown material '{id}'"))?;
    if !kinds.contains(&e.kind.as_str()) {
        return Err(format!(
            "material '{id}' is a {}, expected {}",
            e.kind,
            kinds.join(" or ")
        ));
    }
    Ok(e)
}

/// User parameters with an optional material entry as fallback. User keys
/// take precedence; every material key is still read, so that `finish`
/// catches malformed database entries.
struct Layered<'a> {
    user: &'a mut Params,
    mat: Option<Params>,
}

impl<'a> Layered<'a> {
    fn new(user: &'a mut Params, kinds: &[&str]) -> Result<Self> {
        let mat = match user.string_opt("material")? {
            None => None,
            Some(id) => {
                let e = material(&id, kinds)
                    .map_err(|m| Error::element(user.context().to_string(), m))?;
                Some(Params::new(
                    format!("{} (material {id})", user.context()),
                    e.params.clone(),
                ))
            }
        };
        Ok(Layered { user, mat })
    }

    fn positive(&mut self, base: &str, dim: Dim) -> Result<Option<f64>> {
        let u = self.user.positive_opt(base, dim)?;
        let m = match &mut self.mat {
            Some(m) => m.positive_opt(base, dim)?,
            None => None,
        };
        Ok(u.or(m))
    }

    fn number(&mut self, key: &str) -> Result<Option<f64>> {
        let u = self.user.number_opt(key)?;
        let m = match &mut self.mat {
            Some(m) => m.number_opt(key)?,
            None => None,
        };
        Ok(u.or(m))
    }

    fn finish(self) -> Result<()> {
        match self.mat {
            Some(m) => m.finish(),
            None => Ok(()),
        }
    }
}

// ----- Shared helpers ---------------------------------------------------------

/// Fok's interaction function ψ(ξ) for circular holes of radius a in cells
/// of equivalent radius b, ξ = a/b = sqrt(open-area ratio):
/// ψ = 1 − 1.40925ξ + 0.33818ξ³ + 0.06793ξ⁵ − 0.02287ξ⁶ + 0.03015ξ⁷ − 0.01641ξ⁸
/// (Fok 1941; the form used by Melling 1973, JSV 29, 1, to scale the end
/// correction of perforates). It multiplies an isolated hole's end
/// correction. The test `fok_function_matches_mode_matching` checks it
/// against an independent variational solution of the thin-orifice problem
/// (agreement within 1e-3 up to ξ = 0.6, 2 % of the end correction at
/// ξ = 0.7); beyond ξ ≈ 0.8 the truncated series degrades and it is clamped
/// at zero.
pub fn fok(xi: f64) -> f64 {
    let x = xi.clamp(0.0, 1.0);
    let x3 = x * x * x;
    let x5 = x3 * x * x;
    let psi = 1.0 - 1.40925 * x + 0.33818 * x3 + 0.06793 * x5 - 0.02287 * x5 * x
        + 0.03015 * x5 * x * x
        - 0.01641 * x5 * x3;
    psi.max(0.0)
}

/// Impedance (Pa·s/m³) of one circular hole of radius `a` through a plate of
/// thickness `t`: the thermoviscous tube (Crandall's solution; compressibility
/// of the hole's air is negligible for a thin plate), Ingard's piston end
/// correction 8a/3π per side scaled by ψ(sqrt σ) for neighbour interaction
/// (or unscaled when `interaction` is false), and the resistive end
/// correction `end`.
pub fn hole_impedance(
    air: &AirState,
    omega: f64,
    a: f64,
    t: f64,
    open_area: f64,
    end: EndResistance,
    interaction: bool,
) -> C64 {
    let s = PI * a * a;
    let psi = if interaction {
        fok(open_area.sqrt())
    } else {
        1.0
    };
    let delta = 8.0 * a / (3.0 * PI) * psi;
    thermoviscous::lumped_series_impedance(&Section::Circle { radius: a }, air, omega, t)
        + C64::new(
            2.0 * end.per_end() * surface_resistance(air, omega) / s,
            omega * air.rho * 2.0 * delta / s,
        )
}

/// Reads a surface area given as `area`, `radius` or `diameter`.
fn area_param(p: &mut Params) -> Result<Option<f64>> {
    let a = p.positive_opt("area", Dim::Area)?;
    let r = p.positive_opt("radius", Dim::Length)?;
    let d = p.positive_opt("diameter", Dim::Length)?;
    match (a, r, d) {
        (Some(a), None, None) => Ok(Some(a)),
        (None, Some(r), None) => Ok(Some(PI * r * r)),
        (None, None, Some(d)) => Ok(Some(0.25 * PI * d * d)),
        (None, None, None) => Ok(None),
        _ => Err(Error::element(
            p.context().to_string(),
            "give only one of 'area', 'radius' or 'diameter'",
        )),
    }
}

fn fraction(p: &Params, key: &str, v: f64, allow_one: bool) -> Result<f64> {
    if v > 0.0 && (v < 1.0 || (allow_one && v == 1.0)) {
        Ok(v)
    } else {
        Err(Error::element(
            p.context().to_string(),
            format!(
                "'{key}' must be in (0, 1{}",
                if allow_one { "]" } else { ")" }
            ),
        ))
    }
}

/// One-port series element between two acoustic terminals.
fn series_terminals(b: &Build) -> Result<(Unknown, Unknown)> {
    super::acoustic::acoustic_terminals(b)
}

// ----- Mesh ---------------------------------------------------------------------

/// Pore velocity (m/s RMS) above which the Ingard–Ising nonlinear
/// resistance becomes significant; the spec (Section 6, mesh row) sets the
/// warning at about 1 m/s. The linear solve does not apply the nonlinearity
/// (Ingard & Ising 1967, JASA 42, 6); callers compare
/// [`MeshModel::pore_velocity`] against this value.
pub const PORE_VELOCITY_WARNING: f64 = 1.0;

/// Default band for the mesh badge, Hz.
pub const BADGE_BAND: (f64, f64) = (20.0, 20_000.0);

/// Resistance change (dB) below which a mesh is badged resistive-flat.
pub const FLAT_BADGE_DB: f64 = 1.0;

/// Equivalent cylindrical pores of a mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pores {
    pub radius: f64,
    pub thickness: f64,
    pub open_area: f64,
}

/// Resistive-flat or frequency-dependent badge of a mesh over a band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshBadge {
    pub flat: bool,
    /// Largest |20·log10(Re Z(f)/(R_s/A))| over the band, dB.
    pub change_db: f64,
}

/// A mesh, screen or fabric: specific flow resistance `r_s` (Pa·s/m) over
/// `area` (m²), with optional pore geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshModel {
    pub r_s: f64,
    pub area: f64,
    /// `None`: a pure resistance R_s/A (only the rayl value is known).
    pub pores: Option<Pores>,
    pub end_resistance: EndResistance,
}

impl MeshModel {
    /// Calibrates the equivalent pores to R_s by Poiseuille's law for N
    /// straight cylindrical pores of radius a and length t covering an
    /// open-area fraction φ: R_s = 8μt/(φa²). Any two of thickness, open
    /// area and pore diameter determine the third.
    pub fn calibrate(
        r_s: f64,
        area: f64,
        thickness: Option<f64>,
        open_area: Option<f64>,
        pore_diameter: Option<f64>,
        mu: f64,
        end_resistance: EndResistance,
    ) -> std::result::Result<Self, String> {
        let pores = match (thickness, open_area, pore_diameter) {
            (None, None, None) => None,
            (Some(t), Some(phi), None) => Some(Pores {
                radius: (8.0 * mu * t / (phi * r_s)).sqrt(),
                thickness: t,
                open_area: phi,
            }),
            (None, Some(phi), Some(d)) => {
                let a = 0.5 * d;
                Some(Pores {
                    radius: a,
                    thickness: r_s * phi * a * a / (8.0 * mu),
                    open_area: phi,
                })
            }
            (Some(t), None, Some(d)) => {
                let a = 0.5 * d;
                let phi = 8.0 * mu * t / (r_s * a * a);
                if phi >= 1.0 {
                    return Err(format!(
                        "thickness and pore diameter imply an open area of {phi:.2} ≥ 1 at {r_s} rayl"
                    ));
                }
                Some(Pores {
                    radius: a,
                    thickness: t,
                    open_area: phi,
                })
            }
            _ => {
                return Err("give two of 'thickness', 'open_area' and 'pore_diameter' \
                            (the third follows from R_s), or none for a pure resistance"
                    .into())
            }
        };
        Ok(MeshModel {
            r_s,
            area,
            pores,
            end_resistance,
        })
    }

    /// Reads `R_s`, the area and the pore geometry, with an optional
    /// `material` entry. `default_area` applies when no area key is given.
    pub fn from_params(p: &mut Params, air: &AirState, default_area: Option<f64>) -> Result<Self> {
        let area = match area_param(p)? {
            Some(a) => a,
            None => default_area.ok_or_else(|| {
                Error::element(
                    p.context().to_string(),
                    "give 'area', 'radius' or 'diameter'",
                )
            })?,
        };
        let end = EndResistance::from_params(p, EndResistance::Maa)?;
        let ctx = p.context().to_string();
        let mut l = Layered::new(p, &["mesh", "fabric"])?;
        let r_s = l
            .positive("R_s", Dim::SpecificFlowResistance)?
            .ok_or_else(|| Error::element(&ctx, "missing 'R_s' (R_s_rayl or R_s_Pa_s_per_m)"))?;
        let thickness = l.positive("thickness", Dim::Length)?;
        let open_area = l.number("open_area")?;
        let pore = l.positive("pore_diameter", Dim::Length)?;
        l.finish()?;
        if let Some(phi) = open_area {
            fraction(p, "open_area", phi, false)?;
        }
        MeshModel::calibrate(r_s, area, thickness, open_area, pore, air.mu, end)
            .map_err(|m| Error::element(ctx, m))
    }

    /// DC resistance R_s/A (Pa·s/m³).
    pub fn dc_resistance(&self) -> f64 {
        self.r_s / self.area
    }

    /// Number of equivalent pores φA/(πa²) (not rounded).
    pub fn pore_count(&self) -> Option<f64> {
        self.pores
            .map(|p| p.open_area * self.area / (PI * p.radius * p.radius))
    }

    /// Acoustic impedance of the mesh (Pa·s/m³).
    pub fn impedance(&self, air: &AirState, omega: f64) -> C64 {
        match self.pores {
            None => C64::new(self.dc_resistance(), 0.0),
            Some(p) => {
                let n = self.pore_count().expect("pores");
                hole_impedance(
                    air,
                    omega,
                    p.radius,
                    p.thickness,
                    p.open_area,
                    self.end_resistance,
                    true,
                ) / n
            }
        }
    }

    /// Largest resistance change over [f_lo, f_hi] relative to R_s/A, dB.
    pub fn resistance_change_db(&self, air: &AirState, f_lo: f64, f_hi: f64) -> f64 {
        let n = ((f_hi / f_lo).log2() * 24.0).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| {
                let f = f_lo * (f_hi / f_lo).powf(i as f64 / n as f64);
                let r = self.impedance(air, 2.0 * PI * f).re;
                (20.0 * (r / self.dc_resistance()).log10()).abs()
            })
            .fold(0.0, f64::max)
    }

    /// The spec's resistive-flat badge: flat when the resistance changes by
    /// less than 1 dB over the band.
    pub fn badge(&self, air: &AirState, band: (f64, f64)) -> MeshBadge {
        let change_db = self.resistance_change_db(air, band.0, band.1);
        MeshBadge {
            flat: change_db < FLAT_BADGE_DB,
            change_db,
        }
    }

    /// Validity of the pore model: Stinson's low-reduced-frequency bound on
    /// the equivalent pore radius (none for a pure resistance).
    pub fn limits(&self, id: &str) -> Vec<ValidityLimit> {
        self.pores
            .map(|p| ValidityLimit {
                element: id.to_string(),
                criterion: "mesh pores: Stinson low-reduced-frequency bound",
                begin_hz: Some(0.8 * validity::stinson_bound(p.radius)),
                deep_hz: Some(validity::stinson_bound(p.radius)),
                ..Default::default()
            })
            .into_iter()
            .collect()
    }

    /// RMS mean velocity in the pores for a volume velocity through the mesh,
    /// |U|/(φA). `None` without pore geometry (open area unknown).
    pub fn pore_velocity(&self, volume_velocity: C64) -> Option<f64> {
        self.pores
            .map(|p| volume_velocity.norm() / (p.open_area * self.area))
    }
}

/// The `mesh` element: a series one-port. Downcast with `as_any` to read
/// the model or the pore velocity of a solution.
pub struct Mesh {
    pub id: String,
    pub n1: Unknown,
    pub n2: Unknown,
    pub model: MeshModel,
}

impl Mesh {
    /// Volume velocity through the mesh (first node to second).
    pub fn volume_velocity(&self, cx: &FreqCx, x: &[C64]) -> C64 {
        (potential(x, self.n1) - potential(x, self.n2)) / self.model.impedance(cx.air, cx.omega)
    }

    /// Pore velocity of a solution; compare with [`PORE_VELOCITY_WARNING`].
    pub fn pore_velocity(&self, cx: &FreqCx, x: &[C64]) -> Option<f64> {
        self.model.pore_velocity(self.volume_velocity(cx, x))
    }
}

impl Element for Mesh {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "mesh"
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, _br: &[usize]) {
        mna.admittance(
            self.n1,
            self.n2,
            self.model.impedance(cx.air, cx.omega).inv(),
        );
    }
    fn port_count(&self) -> usize {
        1
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        (port == 0).then(|| potential(x, self.n1) - potential(x, self.n2))
    }
    fn port_flow(&self, cx: &FreqCx, x: &[C64], _br: &[usize], port: usize) -> Option<C64> {
        (port == 0).then(|| self.volume_velocity(cx, x))
    }
    fn validity(&self, _air: &AirState, _level: u8) -> Vec<ValidityLimit> {
        self.model.limits(&self.id)
    }
    /// Pore velocity against the laminar-flow limit, when the pore geometry
    /// is known.
    fn operating(&self, cx: &FreqCx, x: &[C64], _br: &[usize], out: &mut Vec<Operating>) {
        if let Some(v) = self.pore_velocity(cx, x) {
            let mut c = particle_velocity_check(C64::new(v, 0.0), 1.0, "the mesh pores");
            c.value = v;
            c.limit = PORE_VELOCITY_WARNING;
            out.push(c);
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A note for a database entry whose values are not measured or from a
/// datasheet.
pub fn material_note(id: &str, kinds: &[&str]) -> Option<Note> {
    let e = material(id, kinds).ok()?;
    match e.status.as_str() {
        "estimated" | "unverified" => Some(Note {
            code: "material_data",
            severity: Severity::Info,
            message: format!(
                "material '{}' is {} data ({}); treat results that depend on it as indicative",
                e.id, e.status, e.source
            ),
        }),
        _ => None,
    }
}

/// Notes for the element's `material` key, if any.
fn material_notes(b: &Build, kinds: &[&str]) -> Vec<Note> {
    b.params
        .peek_str("material")
        .and_then(|m| material_note(m, kinds))
        .into_iter()
        .collect()
}

fn mesh(mut b: Build) -> Result<Box<dyn Element>> {
    let (n1, n2) = series_terminals(&b)?;
    let notes = material_notes(&b, &["mesh", "fabric"]);
    let air = b.air;
    let model = MeshModel::from_params(&mut b.params, &air, None)?;
    let id = b.id.clone();
    b.finish()?;
    Ok(Annotated::with_notes(
        Box::new(Mesh { id, n1, n2, model }),
        notes,
    ))
}

// ----- Perforated plate -----------------------------------------------------------

/// A perforated plate: holes of radius `hole_radius` through `thickness`,
/// open-area ratio `porosity`, over `area`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerforateModel {
    pub hole_radius: f64,
    pub thickness: f64,
    pub porosity: f64,
    pub area: f64,
    pub end_resistance: EndResistance,
    pub interaction: bool,
}

impl PerforateModel {
    pub fn hole_count(&self) -> f64 {
        self.porosity * self.area / (PI * self.hole_radius * self.hole_radius)
    }

    /// Mass end correction of one side of a hole.
    pub fn end_correction(&self) -> f64 {
        let psi = if self.interaction {
            fok(self.porosity.sqrt())
        } else {
            1.0
        };
        8.0 * self.hole_radius / (3.0 * PI) * psi
    }

    /// Acoustic impedance of the plate (Pa·s/m³).
    pub fn impedance(&self, air: &AirState, omega: f64) -> C64 {
        hole_impedance(
            air,
            omega,
            self.hole_radius,
            self.thickness,
            self.porosity,
            self.end_resistance,
            self.interaction,
        ) / self.hole_count()
    }

    /// Maa's perforate constant k = d·sqrt(ωρ/4μ), the hole's shear
    /// wavenumber: resistive below 1, transitional to 10, mass-dominated
    /// above (spec Section 6).
    pub fn perforate_constant(&self, air: &AirState, omega: f64) -> f64 {
        self.hole_radius * (omega * air.rho / air.mu).sqrt()
    }

    pub fn regime(k: f64) -> &'static str {
        if k < 1.0 {
            "resistive"
        } else if k <= 10.0 {
            "transitional"
        } else {
            "mass"
        }
    }
}

/// Maa's (1998, JASA 104, 2861) approximate micro-perforate impedance, as an
/// acoustic impedance over `area`: z = r + jωm relative to ρc with
/// r = 32μt/(σρc d²)·[sqrt(1 + k²/32) + (√2/32)·k·d/t] and
/// ωm = ωt/(σc)·[1 + 1/sqrt(9 + k²/2) + 0.85·d/t], k = d·sqrt(ωρ/4μ).
/// A comparison oracle for [`PerforateModel`]; it approximates Crandall's
/// function and carries end corrections without interaction.
pub fn maa_impedance(air: &AirState, omega: f64, d: f64, t: f64, sigma: f64, area: f64) -> C64 {
    let k = d * (omega * air.rho / (4.0 * air.mu)).sqrt();
    let rc = air.rho_c();
    let r = 32.0 * air.mu * t / (sigma * rc * d * d)
        * ((1.0 + k * k / 32.0).sqrt() + 2f64.sqrt() / 32.0 * k * d / t);
    let wm = omega * t / (sigma * air.c) * (1.0 + 1.0 / (9.0 + 0.5 * k * k).sqrt() + 0.85 * d / t);
    C64::new(r, wm) * rc / area
}

fn perforated_plate(mut b: Build) -> Result<Box<dyn Element>> {
    let (n1, n2) = series_terminals(&b)?;
    let p = &mut b.params;
    let hr = p.positive_opt("hole_radius", Dim::Length)?;
    let hd = p.positive_opt("hole_diameter", Dim::Length)?;
    let hole_radius = match (hr, hd) {
        (Some(r), None) => r,
        (None, Some(d)) => 0.5 * d,
        _ => return Err(b.err("give exactly one of 'hole_radius' or 'hole_diameter'")),
    };
    let p = &mut b.params;
    let thickness = p.positive("thickness", Dim::Length)?;
    let area = area_param(p)?.ok_or_else(|| b.err("give 'area', 'radius' or 'diameter'"))?;
    let p = &mut b.params;
    let porosity = match (p.number_opt("porosity")?, p.number_opt("holes")?) {
        (Some(s), None) => fraction(p, "porosity", s, false)?,
        (None, Some(n)) if n >= 1.0 && n.fract() == 0.0 => {
            let s = n * PI * hole_radius * hole_radius / area;
            fraction(p, "holes", s, false)
                .map_err(|_| b.err(format!("{n} holes cover {s:.3} of the area (must be < 1)")))?
        }
        _ => return Err(b.err("give exactly one of 'porosity' or 'holes' (a whole number)")),
    };
    let p = &mut b.params;
    let end_resistance = EndResistance::from_params(p, EndResistance::Maa)?;
    let interaction = p.bool_or("interaction", true)?;
    let model = PerforateModel {
        hole_radius,
        thickness,
        porosity,
        area,
        end_resistance,
        interaction,
    };
    let id = b.id.clone();
    let limits = vec![
        validity::lumped_duct(&id, thickness + 2.0 * model.end_correction(), b.air.c),
        ValidityLimit {
            element: id.clone(),
            criterion: "perforate holes: Stinson low-reduced-frequency bound",
            begin_hz: Some(0.8 * validity::stinson_bound(hole_radius)),
            deep_hz: Some(validity::stinson_bound(hole_radius)),
            ..Default::default()
        },
    ];
    b.finish()?;
    Ok(Box::new(OnePort {
        id,
        type_name: "perforated_plate",
        n1,
        n2,
        y: Box::new(move |cx: &FreqCx| model.impedance(cx.air, cx.omega).inv()),
        limits,
    }))
}

// ----- Porous layer ---------------------------------------------------------------

/// Rigid-frame equivalent-fluid model of a porous material. SI units:
/// flow resistivity σ in Pa·s/m², lengths in m, thermal permeability in m².
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PorousModel {
    /// Johnson–Champoux–Allard: porosity φ, σ, tortuosity α∞, viscous and
    /// thermal characteristic lengths Λ, Λ'.
    Jca {
        porosity: f64,
        sigma: f64,
        tortuosity: f64,
        viscous_length: f64,
        thermal_length: f64,
    },
    /// Lafarge's extension (JCAL) with static thermal permeability k0'.
    Jcal {
        porosity: f64,
        sigma: f64,
        tortuosity: f64,
        viscous_length: f64,
        thermal_length: f64,
        thermal_permeability: f64,
    },
    /// Delany & Bazley (1970, Applied Acoustics 3, 105), fitted for
    /// 0.01 ≤ f/σ ≤ 1 (σ in Pa·s/m²).
    DelanyBazley { sigma: f64 },
    /// Miki (1990, J. Acoust. Soc. Jpn (E) 11, 19), a re-fit of Delany and
    /// Bazley's data that behaves better below their window. Neither law is
    /// passive at very low f/σ; see [`PorousModel::equivalent_fluid`].
    Miki { sigma: f64 },
}

impl PorousModel {
    pub fn name(&self) -> &'static str {
        match self {
            PorousModel::Jca { .. } => "jca",
            PorousModel::Jcal { .. } => "jcal",
            PorousModel::DelanyBazley { .. } => "delany_bazley",
            PorousModel::Miki { .. } => "miki",
        }
    }

    pub fn sigma(&self) -> f64 {
        match *self {
            PorousModel::Jca { sigma, .. }
            | PorousModel::Jcal { sigma, .. }
            | PorousModel::DelanyBazley { sigma }
            | PorousModel::Miki { sigma } => sigma,
        }
    }

    /// Frequency window (Hz) of the one-parameter laws: 0.01 ≤ f/σ ≤ 1 with
    /// σ in Pa·s/m² (Delany & Bazley's range; the spec, Section 6, applies
    /// the same window to Miki). `None` for the physical models.
    pub fn window(&self) -> Option<(f64, f64)> {
        match *self {
            PorousModel::DelanyBazley { sigma } | PorousModel::Miki { sigma } => {
                Some((0.01 * sigma, sigma))
            }
            _ => None,
        }
    }

    /// Equivalent-fluid density and bulk modulus (per unit of total volume,
    /// i.e. including the 1/φ of the pore fraction), e^{+jωt}.
    ///
    /// JCA (Allard & Atalla, Propagation of Sound in Porous Media, 2nd ed.,
    /// 2009, which uses the same e^{jωt} convention):
    /// ρ(ω) = α∞ρ0·[1 + σφ/(jωρ0α∞)·sqrt(1 + 4jα∞²ηρ0ω/(σ²Λ²φ²))],
    /// K(ω) = γP0 / [γ − (γ−1)/(1 + ηφ/(jωρ0·Pr·k0')·sqrt(1 + 4jk0'²ρ0·Pr·ω/(ηΛ'²φ²)))]
    /// with k0' = φΛ'²/8 for JCA (which gives Champoux & Allard's form) and
    /// the given k0' for JCAL (Lafarge et al. 1997, JASA 102, 1995);
    /// ρ_eq = ρ/φ and K_eq = K/φ.
    ///
    /// Delany–Bazley and Miki give Z_c and k in terms of X = f/σ (σ in
    /// Pa·s/m², without air density — the variable of the original papers,
    /// whose coefficients are for f/σ with σ in CGS rayl/cm = 1000 Pa·s/m²):
    /// DB: Z_c/ρ0c0 = 1 + 9.08(10³X)^−0.75 − j11.9(10³X)^−0.73,
    ///     k·c0/ω = 1 + 10.8(10³X)^−0.70 − j10.3(10³X)^−0.59;
    /// Miki: Z_c/ρ0c0 = 1 + 5.50(10³X)^−0.632 − j8.43(10³X)^−0.632,
    ///     k·c0/ω = 1 + 7.81(10³X)^−0.618 − j11.41(10³X)^−0.618;
    /// then ρ_eq = Z_c·k/ω and K_eq = Z_c·ω/k.
    ///
    /// Passivity guard for the one-parameter laws. A passive rigid-frame
    /// fluid has Im ρ_eq ≤ 0 and Im K_eq ≥ 0 (e^{+jωt}). Both power laws
    /// violate the second at low f/σ (they are not physically admissible:
    /// Dragna, Attenborough & Blanc-Benon 2015, JASA 138, 2399): Im K_eq
    /// changes sign below f/σ ≈ 0.0106 for Delany–Bazley (just inside its
    /// window) and ≈ 0.00105 for Miki. A layer would then generate power —
    /// with Miki, a 50 kPa·s/m² pad foam below about 50 Hz. The engine
    /// clips Im K_eq (and, for safety, Im ρ_eq) at zero, which changes the
    /// laws only where they would be active and keeps every slab and
    /// rigid-backed layer passive at all frequencies. Below the window
    /// (see [`PorousModel::window`]) the results remain outside the fitted
    /// range.
    pub fn equivalent_fluid(&self, air: &AirState, omega: f64) -> (C64, C64) {
        let j = C64::new(0.0, 1.0);
        let (rho0, eta, pr, gamma, p0) = (air.rho, air.mu, air.prandtl, air.gamma, air.p0);
        let jca = |phi: f64, sigma: f64, ainf: f64, lv: f64, lt: f64, k0t: f64| {
            let gv = (1.0
                + j * 4.0 * ainf * ainf * eta * rho0 * omega
                    / (sigma * sigma * lv * lv * phi * phi))
                .sqrt();
            let rho = ainf * rho0 * (1.0 + sigma * phi / (j * omega * rho0 * ainf) * gv);
            let gt = (1.0 + j * 4.0 * k0t * k0t * rho0 * pr * omega / (eta * lt * lt * phi * phi))
                .sqrt();
            let k = gamma * p0
                / (gamma - (gamma - 1.0) / (1.0 + eta * phi / (j * omega * rho0 * pr * k0t) * gt));
            (rho / phi, k / phi)
        };
        let one_param = |a: [f64; 4], e: [f64; 4], sigma: f64| {
            let x = 1e3 * (omega / (2.0 * PI)) / sigma;
            let zc = air.rho_c() * C64::new(1.0 + a[0] * x.powf(-e[0]), -a[1] * x.powf(-e[1]));
            let k = omega / air.c * C64::new(1.0 + a[2] * x.powf(-e[2]), -a[3] * x.powf(-e[3]));
            let (rho, bulk) = (zc * k / omega, zc * omega / k);
            (
                C64::new(rho.re, rho.im.min(0.0)),
                C64::new(bulk.re, bulk.im.max(0.0)),
            )
        };
        match *self {
            PorousModel::Jca {
                porosity,
                sigma,
                tortuosity,
                viscous_length,
                thermal_length,
            } => jca(
                porosity,
                sigma,
                tortuosity,
                viscous_length,
                thermal_length,
                porosity * thermal_length * thermal_length / 8.0,
            ),
            PorousModel::Jcal {
                porosity,
                sigma,
                tortuosity,
                viscous_length,
                thermal_length,
                thermal_permeability,
            } => jca(
                porosity,
                sigma,
                tortuosity,
                viscous_length,
                thermal_length,
                thermal_permeability,
            ),
            PorousModel::DelanyBazley { sigma } => {
                one_param([9.08, 11.9, 10.8, 10.3], [0.75, 0.73, 0.70, 0.59], sigma)
            }
            PorousModel::Miki { sigma } => one_param(
                [5.50, 8.43, 7.81, 11.41],
                [0.632, 0.632, 0.618, 0.618],
                sigma,
            ),
        }
    }

    /// Propagation constant Γ (Re Γ ≥ 0) and specific characteristic
    /// impedance Z_c (Pa·s/m).
    pub fn propagation(&self, air: &AirState, omega: f64) -> (C64, C64) {
        let (rho, k) = self.equivalent_fluid(air, omega);
        let mut s = (rho / k).sqrt();
        if s.im > 0.0 {
            s = -s;
        }
        (C64::new(0.0, omega) * s, k * s)
    }
}

/// A slab of porous material of `thickness` over `area`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PorousLayer {
    pub model: PorousModel,
    pub thickness: f64,
    pub area: f64,
}

impl PorousLayer {
    /// Transfer matrix of the slab as a series two-port.
    pub fn abcd(&self, air: &AirState, omega: f64) -> [C64; 4] {
        let (g, zc) = self.model.propagation(air, omega);
        let zc = zc / self.area;
        let gl = g * self.thickness;
        let (ch, sh) = (gl.cosh(), gl.sinh());
        [ch, zc * sh, sh / zc, ch]
    }

    /// Lumped (L0) series impedance jωρ_eq·t/A: flow resistance and
    /// inertance, compressibility neglected. Its DC limit is σt/A.
    pub fn lumped_impedance(&self, air: &AirState, omega: f64) -> C64 {
        let (rho, _) = self.model.equivalent_fluid(air, omega);
        C64::new(0.0, omega) * rho * self.thickness / self.area
    }

    /// Acoustic impedance of the layer on a rigid backing, Z_c·coth(Γt)/A.
    pub fn surface_impedance(&self, air: &AirState, omega: f64) -> C64 {
        let (g, zc) = self.model.propagation(air, omega);
        zc / crate::special::tanh(g * self.thickness) / self.area
    }

    /// Frequency (Hz) where |Γ|·t reaches `x`, by bisection on log f.
    fn freq_at_gamma_t(&self, air: &AirState, x: f64) -> f64 {
        let at = |f: f64| self.model.propagation(air, 2.0 * PI * f).0.norm() * self.thickness;
        let (mut lo, mut hi) = (1e-3f64.ln(), 1e8f64.ln());
        for _ in 0..100 {
            let mid = 0.5 * (lo + hi);
            if at(mid.exp()) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (0.5 * (lo + hi)).exp()
    }

    fn limits(&self, id: &str, air: &AirState, level: u8, series: bool) -> Vec<ValidityLimit> {
        let mut v = Vec::new();
        if series && level == 0 {
            let x10 = validity::kl_at_error(validity::duct_error, validity::BEGIN_ERROR, PI / 2.0);
            let x36 = validity::kl_at_error(validity::duct_error, validity::DEEP_ERROR, PI / 2.0);
            v.push(ValidityLimit {
                element: id.to_string(),
                criterion: "lumped porous slab |Γ|t",
                begin_hz: Some(self.freq_at_gamma_t(air, x10)),
                deep_hz: Some(self.freq_at_gamma_t(air, x36)),
                ..Default::default()
            });
        }
        if let Some((lo, hi)) = self.model.window() {
            // The fitted range is 0.01 ≤ f/σ ≤ 1. Above it shading deepens
            // one octave out. Below it, shading deepens one octave below the
            // window or where the law's Im K_eq turns negative and is
            // clipped (f/σ ≈ 0.0106 Delany–Bazley, 0.00105 Miki), whichever
            // is higher: Delany–Bazley is clipped just inside its window.
            let clip = match self.model {
                PorousModel::DelanyBazley { sigma } => 0.0106 * sigma,
                PorousModel::Miki { sigma } => 0.00105 * sigma,
                _ => 0.5 * lo,
            };
            v.push(ValidityLimit {
                element: id.to_string(),
                criterion: "one-parameter porous law: fitted range 0.01 ≤ f/σ ≤ 1",
                begin_hz: Some(hi),
                deep_hz: Some(2.0 * hi),
                low_begin_hz: Some(lo),
                low_deep_hz: Some(clip.max(0.5 * lo)),
            });
        }
        v
    }
}

fn porous_model(p: &mut Params) -> Result<PorousModel> {
    let ctx = p.context().to_string();
    let requested = p.string_opt("model")?;
    let mut l = Layered::new(p, &["porous"])?;
    let sigma = l
        .positive("sigma", Dim::FlowResistivity)?
        .ok_or_else(|| Error::element(&ctx, "missing 'sigma' (e.g. sigma_Pa_s_per_m2)"))?;
    let porosity = l.number("porosity")?;
    let tortuosity = l.number("tortuosity")?;
    let lv = l.positive("viscous_length", Dim::Length)?;
    let lt = l.positive("thermal_length", Dim::Length)?;
    let k0t = l.positive("thermal_permeability", Dim::Area)?;
    l.finish()?;
    let full = [porosity, tortuosity, lv, lt];
    let have_full = full.iter().all(Option::is_some);
    let any_full = full.iter().any(Option::is_some) || k0t.is_some();
    let model = requested.unwrap_or_else(|| {
        if !have_full {
            "miki".into()
        } else if k0t.is_some() {
            "jcal".into()
        } else {
            "jca".into()
        }
    });
    let err = |m: String| Error::element(&ctx, m);
    match model.as_str() {
        "jca" | "jcal" => {
            let (Some(phi), Some(ainf), Some(lv), Some(lt)) = (porosity, tortuosity, lv, lt) else {
                return Err(err(format!(
                    "model '{model}' needs porosity, tortuosity, viscous_length and thermal_length"
                )));
            };
            if !(phi > 0.0 && phi <= 1.0) {
                return Err(err("'porosity' must be in (0, 1]".into()));
            }
            if ainf < 1.0 {
                return Err(err("'tortuosity' must be ≥ 1".into()));
            }
            if model == "jca" {
                if k0t.is_some() {
                    return Err(err("'thermal_permeability' needs model 'jcal'".into()));
                }
                Ok(PorousModel::Jca {
                    porosity: phi,
                    sigma,
                    tortuosity: ainf,
                    viscous_length: lv,
                    thermal_length: lt,
                })
            } else {
                Ok(PorousModel::Jcal {
                    porosity: phi,
                    sigma,
                    tortuosity: ainf,
                    viscous_length: lv,
                    thermal_length: lt,
                    thermal_permeability: k0t
                        .ok_or_else(|| err("model 'jcal' needs 'thermal_permeability'".into()))?,
                })
            }
        }
        "delany_bazley" | "miki" => {
            if any_full {
                return Err(err(format!(
                    "model '{model}' takes flow resistivity only; remove the JCA parameters"
                )));
            }
            Ok(if model == "miki" {
                PorousModel::Miki { sigma }
            } else {
                PorousModel::DelanyBazley { sigma }
            })
        }
        o => Err(err(format!(
            "unknown model '{o}' (jca, jcal, delany_bazley, miki)"
        ))),
    }
}

fn porous_layer(b: Build) -> Result<Box<dyn Element>> {
    let notes = material_notes(&b, &["porous"]);
    Ok(Annotated::with_notes(porous_layer_element(b)?, notes))
}

fn porous_layer_element(mut b: Build) -> Result<Box<dyn Element>> {
    let model = porous_model(&mut b.params)?;
    let p = &mut b.params;
    let thickness = p.positive("thickness", Dim::Length)?;
    let area = area_param(p)?.ok_or_else(|| b.err("give 'area', 'radius' or 'diameter'"))?;
    let backing = b.params.string_opt("backing")?;
    let layer = PorousLayer {
        model,
        thickness,
        area,
    };
    let id = b.id.clone();
    let level = b.level;
    match backing.as_deref() {
        Some("rigid") => {
            b.expect_terminals(1, 1)?;
            let n1 = b.terminal(0, Domain::Acoustic)?;
            let limits = layer.limits(&id, &b.air, level, false);
            b.finish()?;
            Ok(Box::new(OnePort {
                id,
                type_name: "porous_layer",
                n1,
                n2: None,
                y: Box::new(move |cx: &FreqCx| layer.surface_impedance(cx.air, cx.omega).inv()),
                limits,
            }))
        }
        None => {
            let (n1, n2) = series_terminals(&b)?;
            let limits = layer.limits(&id, &b.air, level, true);
            b.finish()?;
            if level == 0 {
                Ok(Box::new(OnePort {
                    id,
                    type_name: "porous_layer",
                    n1,
                    n2,
                    y: Box::new(move |cx: &FreqCx| layer.lumped_impedance(cx.air, cx.omega).inv()),
                    limits,
                }))
            } else {
                Ok(Box::new(TwoPort {
                    id,
                    type_name: "porous_layer",
                    port1: (n1, None),
                    port2: (n2, None),
                    abcd: Box::new(move |cx: &FreqCx| layer.abcd(cx.air, cx.omega)),
                    limits,
                }))
            }
        }
        Some(o) => Err(b.err(format!(
            "backing '{o}' must be 'rigid' (or omit it for a series slab)"
        ))),
    }
}

// ----- Fill -----------------------------------------------------------------------

/// Thermal relaxation time of air in a fibre array (s): fibres of diameter
/// `fibre_diameter` occupying the volume fraction `solid_fraction` c.
///
/// Cell model (Tarnow 1996, "Compressibility of air in fibrous materials",
/// JASA 99, 3010): each fibre of radius r is isothermal and sits in a
/// coaxial air cell of radius b = r/sqrt(c) with no heat flux at the cell
/// boundary. The single relaxation time that matches the exact cell
/// compressibility to first order in ω is τ = ⟨θ⟩/ν', with ν' = μ/(ρ·Pr)
/// the thermal diffusivity and ⟨θ⟩ the mean of the Poisson solution
/// ∇²θ = −1, θ(r) = 0, θ'(b) = 0:
/// ⟨θ⟩ = b²·[−ln(c)/4 − 3/8 + c/2 − c²/8]/(1 − c).
/// This is Lafarge's static thermal permeability divided by the porosity
/// (Lafarge et al. 1997, JASA 102, 1995), for which the relaxation form of
/// App. C6 is the low-frequency expansion. Against the exact cell
/// compressibility the single pole is within 2 % in magnitude up to three
/// times the relaxation frequency; above it both vanish, the exact one more
/// slowly (∝ ω^−½).
pub fn fibre_relaxation_time(air: &AirState, fibre_diameter: f64, solid_fraction: f64) -> f64 {
    let c = solid_fraction;
    let r = 0.5 * fibre_diameter;
    let b2 = r * r / c;
    let theta = b2 * (-0.25 * c.ln() - 0.375 + 0.5 * c - 0.125 * c * c) / (1.0 - c);
    let nu_t = air.mu / (air.rho * air.prandtl);
    theta / nu_t
}

/// Fibrous fill in a cavity of `volume`, filling the fraction `fraction`,
/// with thermal relaxation time `tau`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillModel {
    pub volume: f64,
    pub fraction: f64,
    pub tau: f64,
}

impl FillModel {
    /// Extra compliance ΔC = C_ad·(γ−1)·φ/(1 + jωτ), C_ad = V/(γP0) (spec
    /// App. C6). At low frequency the filled air is isothermal (+40 % of its
    /// compliance for γ = 1.4); the imaginary part is the thermal loss, and
    /// it is passive for e^{+jωt}. The volume of the fibres themselves
    /// (a solid fraction of order 1 %) is not subtracted.
    pub fn delta_compliance(&self, air: &AirState, omega: f64) -> C64 {
        let c_ad = self.volume / air.bulk_modulus();
        c_ad * (air.gamma - 1.0) * self.fraction / C64::new(1.0, omega * self.tau)
    }

    /// Relaxation frequency 1/(2πτ), Hz.
    pub fn relaxation_frequency(&self) -> f64 {
        1.0 / (2.0 * PI * self.tau)
    }
}

fn fill(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 1)?;
    let node = b.terminal(0, Domain::Acoustic)?;
    let p = &mut b.params;
    let volume = p.positive("volume", Dim::Volume)?;
    let phi = p.number_or("fraction", 1.0)?;
    let phi = fraction(p, "fraction", phi, true)?;
    let tau = p.positive_opt("tau", Dim::Time)?;
    let fr = p.positive_opt("f_relax", Dim::Frequency)?;
    let d = p.positive_opt("fibre_diameter", Dim::Length)?;
    let bulk = p.positive_opt("bulk_density", Dim::Density)?;
    let solid = p.positive_opt("fibre_density", Dim::Density)?;
    let tau = match (tau, fr, d, bulk, solid) {
        (Some(t), None, None, None, None) => t,
        (None, Some(f), None, None, None) => 1.0 / (2.0 * PI * f),
        (None, None, Some(d), Some(bulk), Some(solid)) => {
            let c = bulk / solid;
            if c >= 0.5 {
                return Err(b.err(format!(
                    "bulk density is {:.0} % of the fibre density; the cell model needs a dilute fill",
                    100.0 * c
                )));
            }
            fibre_relaxation_time(&b.air, d, c)
        }
        _ => return Err(b.err(
            "give one of 'tau', 'f_relax', or 'fibre_diameter' + 'bulk_density' + 'fibre_density'",
        )),
    };
    let model = FillModel {
        volume,
        fraction: phi,
        tau,
    };
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(OnePort {
        id,
        type_name: "fill",
        n1: node,
        n2: None,
        y: Box::new(move |cx: &FreqCx| cx.jw() * model.delta_compliance(cx.air, cx.omega)),
        limits: Vec::new(),
    }))
}

// ----- Membrane vent ------------------------------------------------------------------

/// A porous membrane vent: the flow resistance of its pores in parallel with
/// the membrane moving as a whole (mass, compliance and optional loss):
/// Z = R ∥ (R_m + jωM + 1/(jωC)).
///
/// The parallel form is the limp porous sheet of Beranek (Noise and
/// Vibration Control, 1971; Beranek & Vér 1992): the pressure difference
/// drives both the relative flow through the pores and the frame. At DC
/// only the pores conduct (Z = R); at the membrane resonance the frame
/// branch short-circuits the resistance, which is why insertion loss is not
/// monotonic in the rayl value (spec Section 6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MembraneVentModel {
    /// Pore flow resistance R_s/A, Pa·s/m³.
    pub r_flow: f64,
    /// Acoustic mass of the membrane, kg/m⁴.
    pub mass: f64,
    /// Acoustic compliance of the membrane, m³/Pa.
    pub compliance: f64,
    /// Loss of the membrane branch, Pa·s/m³.
    pub r_membrane: f64,
}

impl MembraneVentModel {
    pub fn impedance(&self, omega: f64) -> C64 {
        let jw = C64::new(0.0, omega);
        let zm = self.r_membrane + jw * self.mass + (jw * self.compliance).inv();
        (1.0 / self.r_flow + zm.inv()).inv()
    }

    pub fn resonance(&self) -> f64 {
        1.0 / (2.0 * PI * (self.mass * self.compliance).sqrt())
    }
}

fn membrane_vent(mut b: Build) -> Result<Box<dyn Element>> {
    let (n1, n2) = series_terminals(&b)?;
    let p = &mut b.params;
    let r_s = p.positive("R_s", Dim::SpecificFlowResistance)?;
    let area = area_param(p)?.ok_or_else(|| b.err("give 'area', 'radius' or 'diameter'"))?;
    let p = &mut b.params;
    let thickness = p.positive_opt("thickness", Dim::Length)?;
    let density = p.positive_opt("density", Dim::Density)?;
    let mass_total = p.positive_opt("mass", Dim::Mass)?;
    let surface_density = match (thickness, density, mass_total) {
        (Some(t), Some(rho), None) => t * rho,
        (None, None, Some(m)) => m / area,
        _ => return Err(b.err("give 'thickness' + 'density' or 'mass' for the membrane")),
    };
    let p = &mut b.params;
    // Acoustic mass of the fundamental deflection shape, referred to the
    // volume velocity: κ·m_s/A with κ = ∫w² dA·A/(∫w dA)². Tensioned
    // membrane, w ∝ 1 − r²/a²: κ = 4/3; clamped plate, w ∝ (1 − r²/a²)²:
    // κ = 9/5; rigid piston: κ = 1.
    let profile = p
        .string_opt("profile")?
        .unwrap_or_else(|| "membrane".into());
    let kappa = match profile.as_str() {
        "membrane" => 4.0 / 3.0,
        "plate" => 9.0 / 5.0,
        "piston" => 1.0,
        o => return Err(b.err(format!("profile '{o}' must be membrane, plate or piston"))),
    };
    let mass = kappa * surface_density / area;
    let c = p.positive_opt("C", Dim::AcousticCompliance)?;
    let fres = p.positive_opt("resonance", Dim::Frequency)?;
    let tension = p.positive_opt("tension", Dim::MechStiffness)?;
    let compliance =
        match (c, fres, tension) {
            (Some(c), None, None) => c,
            (None, Some(f), None) => 1.0 / ((2.0 * PI * f).powi(2) * mass),
            (None, None, Some(t)) if profile == "membrane" => {
                // Uniformly loaded circular membrane under tension T (N/m):
                // w = p(a² − r²)/(4T), volume πa⁴p/(8T).
                let a2 = area / PI;
                PI * a2 * a2 / (8.0 * t)
            }
            (None, None, Some(_)) => return Err(b.err("'tension' needs profile 'membrane'")),
            _ => return Err(b.err(
                "give one of 'C' (acoustic compliance), 'resonance' or 'tension' for the membrane",
            )),
        };
    // The membrane branch needs a loss: without one it is an ideal short at
    // its resonance. Datasheets do not give it, so it is a required input
    // (an estimate should be labelled as such by the caller).
    let r_m = b.params.positive_opt("R_m", Dim::AcousticResistance)?;
    let q_m = b.params.number_opt("Q_m")?;
    let r_membrane = match (r_m, q_m) {
        (Some(r), None) => r,
        (None, Some(q)) if q > 0.0 => (mass / compliance).sqrt() / q,
        _ => {
            return Err(b.err(
                "give the membrane-branch loss as 'R_m' (acoustic resistance) or 'Q_m' (> 0)",
            ))
        }
    };
    let model = MembraneVentModel {
        r_flow: r_s / area,
        mass,
        compliance,
        r_membrane,
    };
    let id = b.id.clone();
    b.finish()?;
    Ok(Box::new(OnePort {
        id,
        type_name: "membrane_vent",
        n1,
        n2,
        y: Box::new(move |cx: &FreqCx| model.impedance(cx.omega).inv()),
        limits: Vec::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_parses_and_every_entry_builds() {
        let db = database().unwrap();
        assert!(!db.is_empty());
        let air = AirState::spec_reference();
        for e in db {
            let mut m = Map::new();
            m.insert("material".into(), Value::String(e.id.clone()));
            m.insert("area_cm2".into(), Value::from(1.0));
            let mut p = Params::new(format!("t.{}", e.id), m);
            match e.kind.as_str() {
                "mesh" | "fabric" => {
                    MeshModel::from_params(&mut p, &air, None).unwrap();
                }
                "porous" => {
                    porous_model(&mut p).unwrap();
                    p.positive_opt("area", Dim::Area).unwrap();
                }
                k => panic!("unknown kind {k}"),
            }
            p.finish().unwrap();
        }
    }

    #[test]
    fn fok_is_one_at_zero_and_vanishes_at_full_porosity() {
        assert_eq!(fok(0.0), 1.0);
        assert!(fok(1.0) < 1e-12);
        assert!((fok(0.1) - 0.859_414).abs() < 1e-5);
    }
}
