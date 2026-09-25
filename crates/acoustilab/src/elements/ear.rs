//! Ear, head and fixture loads (spec Section 7; errata E6, E19, E36, E41).
//!
//! * `canal` — an ear canal as ONE two-port: a chain of truncated cones built
//!   from an area function, cascaded into a single transfer matrix (two MNA
//!   branch unknowns whatever the segment count). Each cone uses the exact
//!   lossless conical-horn matrix with the thermoviscous medium of its mean
//!   radius substituted, which is exact for a cone filled with a uniform
//!   effective medium; the only discretisation error is the variation of the
//!   wall losses along the segment (second order in the segment length).
//! * `eardrum` — drum terminations: the Hudde & Engel (1998) human model,
//!   the drum network fitted to ITU-T P.57 Type 4.3, the IEC 60318-4
//!   equivalent (its two Helmholtz branches and the microphone), or rigid.
//! * `iec60318_4`, `type33`, `type43` — ear-simulator macros exposing the
//!   internal nodes `<id>.eep` (canal entrance), `<id>.drp` (drum reference
//!   point = microphone plane) and, where the simulator has one, `<id>.ref`
//!   (reference plane).
//!
//! Every empirical element value is read from `data/ear/*.json`, which
//! records its source; derivations and the fits are in `docs/ear-loads.md`
//! and `tools/ear/`. The only other numbers in this file are the Type 3.3
//! extension defaults and the validity bands, each cited where it is used.

use super::{Build, Constructor, Element, FreqCx, OnePort, TwoPort};
use crate::air::AirState;
use crate::error::{Error, Result};
use crate::mna::{abcd_mul, abcd_series, abcd_shunt, potential, Mna, Unknown, ONE, ZERO};
use crate::netlist::Domain;
use crate::thermoviscous::{self, Section};
use crate::units::{Dim, Params};
use crate::validity::{self, ValidityLimit};
use crate::C64;
use serde::Deserialize;
use serde_json::Value;
use std::any::Any;
use std::f64::consts::PI;
use std::sync::OnceLock;

pub const TYPES: &[&str] = &["canal", "eardrum", "iec60318_4", "type33", "type43"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    Some(match ty {
        "canal" => canal,
        "eardrum" => eardrum,
        "iec60318_4" => iec60318_4,
        "type33" => type33,
        "type43" => type43,
        _ => return None,
    })
}

const J: C64 = C64::new(0.0, 1.0);

// ----- Conical segments ---------------------------------------------------

/// A truncated cone: length and the radii at its first and second ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cone {
    pub length: f64,
    pub r1: f64,
    pub r2: f64,
}

/// q(u) = (u·cosh u − sinh u)/u², by its Taylor series near zero where the
/// direct form cancels.
fn cone_q(u: C64) -> C64 {
    if u.norm() < 0.1 {
        cone_q_series(u)
    } else {
        (u * u.cosh() - u.sinh()) / (u * u)
    }
}

/// Σ_{k≥1} 2k/(2k+1)! · u^{2k−1} = u/3 + u³/30 + u⁵/840 + u⁷/45360 + u⁹/3991680;
/// the first omitted term is below 1e-19 of the sum for |u| < 0.1.
fn cone_q_series(u: C64) -> C64 {
    let u2 = u * u;
    let mut term = u;
    let mut sum = ZERO;
    for c in [
        1.0 / 3.0,
        1.0 / 30.0,
        1.0 / 840.0,
        1.0 / 45_360.0,
        1.0 / 3_991_680.0,
    ] {
        sum += term * c;
        term *= u2;
    }
    sum
}

/// Propagation constant Γ and specific characteristic impedance
/// sqrt(ρ_eff·K_eff) of a circular duct of radius `r` (or of free air when
/// `lossy` is false).
fn medium_of(r: f64, air: &AirState, omega: f64, lossy: bool) -> (C64, C64) {
    if lossy {
        let section = Section::Circle { radius: r };
        let (gamma, zc) = thermoviscous::propagation(&section, air, omega);
        (gamma, zc * section.area())
    } else {
        (C64::new(0.0, omega / air.c), C64::new(air.rho_c(), 0.0))
    }
}

/// Transfer matrix [p1; U1] = T·[p2; U2] of a truncated cone.
///
/// With q = x·p (x measured from the apex) the horn equation of a cone
/// filled with a uniform medium reduces to q'' = Γ²q, which integrates
/// exactly (Mapes-Riordan, JAES 41(6) 1993; Chaigne & Kergomard 2016,
/// Ch. 7). Written without apex distances, with u = ΓL and Z0 = sqrt(ρK):
///
/// A = (r2/r1)·cosh u − ((r2−r1)/r1)·sinh(u)/u
/// B = Z0·sinh(u)/(π r1 r2)
/// C = (π r1 r2/Z0)·[sinh u + ((r2−r1)²/(r1 r2))·(u cosh u − sinh u)/u²]
/// D = (r1/r2)·cosh u + ((r2−r1)/r2)·sinh(u)/u
///
/// which reduces to the uniform tube when r1 = r2, and has det T = 1. With
/// `lossy` the thermoviscous medium is that of the mean radius.
pub fn cone_abcd(cone: Cone, air: &AirState, omega: f64, lossy: bool) -> [C64; 4] {
    let Cone { length, r1, r2 } = cone;
    let (gamma, z0) = medium_of(0.5 * (r1 + r2), air, omega, lossy);
    let u = gamma * length;
    let (ch, sh) = (u.cosh(), u.sinh());
    let shu = if u.norm() < 1e-12 { ONE } else { sh / u };
    let s12 = PI * r1 * r2;
    let dr = r2 - r1;
    [
        ch * (r2 / r1) - shu * (dr / r1),
        z0 * sh / s12,
        (sh + cone_q(u) * (dr * dr / (r1 * r2))) * s12 / z0,
        ch * (r1 / r2) + shu * (dr / r2),
    ]
}

/// Cascade of cones (first to last).
pub fn chain_abcd(cones: &[Cone], air: &AirState, omega: f64, lossy: bool) -> [C64; 4] {
    cones.iter().fold([ONE, ZERO, ZERO, ONE], |t, c| {
        abcd_mul(t, cone_abcd(*c, air, omega, lossy))
    })
}

/// Lumped (L0) T-section of a cone chain: series Z/2, shunt Y, series Z/2,
/// with Z = jω Σ ρ_eff·∫dx/S (exactly L/(π r1 r2) per cone) and
/// Y = jω Σ V/K_eff (exact frustum volumes), the thermoviscous factors taken
/// at each cone's mean radius.
pub fn chain_lumped_abcd(cones: &[Cone], air: &AirState, omega: f64, lossy: bool) -> [C64; 4] {
    let jw = C64::new(0.0, omega);
    let mut z = ZERO;
    let mut y = ZERO;
    for c in cones {
        let (rho, k) = if lossy {
            let m = thermoviscous::medium(
                &Section::Circle {
                    radius: 0.5 * (c.r1 + c.r2),
                },
                air,
                omega,
            );
            (m.rho_eff, m.k_eff)
        } else {
            (C64::new(air.rho, 0.0), C64::new(air.bulk_modulus(), 0.0))
        };
        z += jw * rho * (c.length / (PI * c.r1 * c.r2));
        y += jw * (PI * c.length * (c.r1 * c.r1 + c.r1 * c.r2 + c.r2 * c.r2) / 3.0) / k;
    }
    let half = abcd_series(z * 0.5);
    abcd_mul(abcd_mul(half, abcd_shunt(y)), half)
}

// ----- Area functions ---------------------------------------------------------

/// Cross-section area against axial position, with the radius linear between
/// knots (so each interval is a cone). Positions run from the first port to
/// the second and may increase or decrease.
#[derive(Debug, Clone, PartialEq)]
pub struct AreaFunction {
    positions: Vec<f64>,
    areas: Vec<f64>,
}

impl AreaFunction {
    pub fn new(positions: Vec<f64>, areas: Vec<f64>) -> std::result::Result<Self, String> {
        if positions.len() < 2 || positions.len() != areas.len() {
            return Err(
                "an area function needs at least two positions and one area per position".into(),
            );
        }
        if positions.iter().chain(&areas).any(|v| !v.is_finite()) {
            return Err("positions and areas must be finite".into());
        }
        if areas.iter().any(|&a| a <= 0.0) {
            return Err("areas must be positive".into());
        }
        let dir = (positions[1] - positions[0]).signum();
        if dir == 0.0 || positions.windows(2).any(|w| (w[1] - w[0]) * dir <= 0.0) {
            return Err("positions must be strictly increasing or strictly decreasing".into());
        }
        Ok(AreaFunction { positions, areas })
    }

    /// Uniform area over `length`.
    pub fn uniform(length: f64, area: f64) -> std::result::Result<Self, String> {
        Self::new(vec![0.0, length], vec![area, area])
    }

    pub fn positions(&self) -> &[f64] {
        &self.positions
    }

    pub fn areas(&self) -> &[f64] {
        &self.areas
    }

    pub fn length(&self) -> f64 {
        (self.positions[self.positions.len() - 1] - self.positions[0]).abs()
    }

    fn radius(area: f64) -> f64 {
        (area / PI).sqrt()
    }

    pub fn max_radius(&self) -> f64 {
        Self::radius(self.areas.iter().cloned().fold(0.0, f64::max))
    }

    /// Radius at position `x` (inside the range), linear between knots.
    pub fn radius_at(&self, x: f64) -> f64 {
        let (xs, n) = (&self.positions, self.positions.len());
        let inc = xs[1] > xs[0];
        for i in 0..n - 1 {
            let (a, b) = (xs[i], xs[i + 1]);
            let inside = if inc {
                x >= a && x <= b
            } else {
                x <= a && x >= b
            };
            if inside || i == n - 2 {
                let (ra, rb) = (Self::radius(self.areas[i]), Self::radius(self.areas[i + 1]));
                return (rb - ra) / (b - a) * (x - a) + ra;
            }
        }
        unreachable!()
    }

    /// The part between `x_from` and `x_to` (both inside the range, in either
    /// order): the knots strictly between them are kept and the ends are
    /// interpolated.
    pub fn sub(&self, x_from: f64, x_to: f64) -> AreaFunction {
        let (lo, hi) = (x_from.min(x_to), x_from.max(x_to));
        let area = |x: f64| PI * self.radius_at(x).powi(2);
        let mut pts = vec![(lo, area(lo))];
        let mut inner: Vec<(f64, f64)> = self
            .positions
            .iter()
            .zip(&self.areas)
            .filter(|(&x, _)| lo + 1e-12 < x && x < hi - 1e-12)
            .map(|(&x, &a)| (x, a))
            .collect();
        inner.sort_by(|a, b| a.0.total_cmp(&b.0));
        pts.extend(inner);
        pts.push((hi, area(hi)));
        if x_from > x_to {
            pts.reverse();
        }
        AreaFunction {
            positions: pts.iter().map(|p| p.0).collect(),
            areas: pts.iter().map(|p| p.1).collect(),
        }
    }

    /// Conical segments in traversal order. Every knot is a segment boundary;
    /// interval i is split into ⌈|span_i|/(L/n) − 1e-9⌉ equal parts (at least
    /// one), so the count is about `n` and never below the number of intervals.
    pub fn segments(&self, n: usize) -> Vec<Cone> {
        let dx = self.length() / n.max(1) as f64;
        let mut out = Vec::new();
        for i in 0..self.positions.len() - 1 {
            let span = self.positions[i + 1] - self.positions[i];
            let m = ((span.abs() / dx - 1e-9).ceil() as usize).max(1);
            let (ra, rb) = (Self::radius(self.areas[i]), Self::radius(self.areas[i + 1]));
            let mut r_prev = ra;
            let mut x_prev = self.positions[i];
            for k in 1..=m {
                let t = k as f64 / m as f64;
                let x = self.positions[i] + t * span;
                let r = ra + t * (rb - ra);
                out.push(Cone {
                    length: (x - x_prev).abs(),
                    r1: r_prev,
                    r2: r,
                });
                r_prev = r;
                x_prev = x;
            }
        }
        out
    }

    /// Air volume (exact frusta between knots).
    pub fn volume(&self) -> f64 {
        self.segments(1)
            .iter()
            .map(|c| PI * c.length * (c.r1 * c.r1 + c.r1 * c.r2 + c.r2 * c.r2) / 3.0)
            .sum()
    }
}

/// Validity limits of a distributed canal: first transverse mode and
/// Stinson's low-reduced-frequency bound, both at the widest section.
fn canal_limits(element: &str, max_radius: f64, air: &AirState) -> Vec<ValidityLimit> {
    let cut_on = validity::circular_cut_on(max_radius, air.c);
    let stinson = validity::stinson_bound(max_radius);
    vec![
        ValidityLimit {
            element: element.to_string(),
            criterion: "canal: first transverse mode at the widest section",
            begin_hz: Some(0.7 * cut_on),
            deep_hz: Some(cut_on),
        },
        ValidityLimit {
            element: element.to_string(),
            criterion: "canal: Stinson low-reduced-frequency bound at the widest section",
            begin_hz: Some(0.8 * stinson),
            deep_hz: Some(stinson),
        },
    ]
}

// ----- Parameter helpers ----------------------------------------------------------

/// Optional array quantity `base_<suffix>` converted to SI.
fn quantity_array(p: &mut Params, base: &str, dim: Dim) -> Result<Option<Vec<f64>>> {
    let ctx = p.context().to_string();
    let mut found: Option<(String, Vec<f64>)> = None;
    for (suffix, factor) in dim.suffixes() {
        let key = format!("{base}_{suffix}");
        if let Some(v) = p.value_opt(&key) {
            if let Some((prev, _)) = &found {
                return Err(Error::element(
                    &ctx,
                    format!("both '{prev}' and '{key}' given"),
                ));
            }
            let arr = v
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .map(|x| x.as_f64().filter(|x| x.is_finite()).map(|x| x * factor))
                        .collect::<Option<Vec<f64>>>()
                })
                .ok_or_else(|| {
                    Error::element(&ctx, format!("'{key}' must be an array of finite numbers"))
                })?;
            found = Some((key, arr));
        }
    }
    Ok(found.map(|f| f.1))
}

/// Optional strictly positive dimensionless number.
fn positive_number_or(p: &mut Params, key: &str, default: f64) -> Result<f64> {
    let v = p.number_or(key, default)?;
    if v > 0.0 {
        Ok(v)
    } else {
        Err(Error::element(
            p.context(),
            format!("'{key}' must be positive"),
        ))
    }
}

// ----- canal ----------------------------------------------------------------------

/// Reads the canal profile. `segments` sets the knot count of an
/// exponential profile (one cone per knot interval).
fn parse_profile(b: &mut Build, segments: usize) -> Result<AreaFunction> {
    let p = &mut b.params;
    let positions = quantity_array(p, "positions", Dim::Length)?;
    let areas = quantity_array(p, "areas", Dim::Area)?;
    let length = p.positive_opt("length", Dim::Length)?;
    let area = p.positive_opt("area", Dim::Area)?;
    let area_in = p.positive_opt("area_entrance", Dim::Area)?;
    let area_end = p.positive_opt("area_end", Dim::Area)?;
    let taper = p.number_opt("taper")?;
    let shape = p.string_opt("profile")?;
    let parametric = area.is_some() || area_in.is_some() || area_end.is_some();
    match (positions, areas, length) {
        (Some(x), Some(a), None) => {
            if parametric || taper.is_some() || shape.is_some() {
                return Err(b.err("'positions' + 'areas' take no other profile keys"));
            }
            AreaFunction::new(x, a).map_err(|m| b.err(m))
        }
        (None, None, Some(len)) => {
            let (a0, a1) = match (area, area_in, area_end) {
                (Some(mid), None, None) => {
                    // Mid-canal area kept: r_mid = (r0 + r1)/2 and r1/r0 = sqrt(taper).
                    let t = taper.unwrap_or(1.0);
                    if t <= 0.0 {
                        return Err(b.err("'taper' (end area / entrance area) must be positive"));
                    }
                    let r0 = 2.0 * (mid / PI).sqrt() / (1.0 + t.sqrt());
                    let r1 = r0 * t.sqrt();
                    (PI * r0 * r0, PI * r1 * r1)
                }
                (None, Some(a0), Some(a1)) if taper.is_none() => (a0, a1),
                _ => {
                    return Err(b.err(
                        "with 'length' give 'area' [+ 'taper'] or 'area_entrance' + 'area_end'",
                    ))
                }
            };
            let (x, a) = match shape.as_deref().unwrap_or("conical") {
                "conical" => (vec![0.0, len], vec![a0, a1]),
                "exponential" => {
                    let n = segments;
                    let t = |i: usize| i as f64 / n as f64;
                    (
                        (0..=n).map(|i| len * t(i)).collect(),
                        (0..=n).map(|i| a0 * (a1 / a0).powf(t(i))).collect(),
                    )
                }
                other => {
                    return Err(b.err(format!(
                        "unknown profile '{other}' (conical or exponential)"
                    )))
                }
            };
            AreaFunction::new(x, a).map_err(|m| b.err(m))
        }
        _ => Err(b.err(
            "give either 'positions' + 'areas' arrays, or 'length' with an area specification",
        )),
    }
}

fn canal(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(2, 2)?;
    let n1 = b.terminal(0, Domain::Acoustic)?;
    let n2 = b.terminal(1, Domain::Acoustic)?;
    let segments = b.params.count_or("segments", 40)?;
    if segments > 5000 {
        return Err(b.err("'segments' must be at most 5000"));
    }
    let mut profile = parse_profile(&mut b, segments)?;
    let lossy = b.params.bool_or("wall_loss", true)?;
    if let Some(off) = b.params.quantity_opt("entrance_offset", Dim::Length)? {
        if off < 0.0 || off >= profile.length() {
            return Err(b.err("'entrance_offset' must lie within the canal length"));
        }
        let x = profile.positions();
        let (x0, x1) = (x[0], x[x.len() - 1]);
        let start = x0 + off * (x1 - x0).signum();
        profile = profile.sub(start, x1);
    }
    let cones = profile.segments(segments);
    let id = b.id.clone();
    let level = b.level;
    let limits = if level == 0 {
        vec![validity::lumped_duct(&id, profile.length(), b.air.c)]
    } else {
        canal_limits(&id, profile.max_radius(), &b.air)
    };
    b.finish()?;
    let abcd: super::AbcdFn = if level == 0 {
        Box::new(move |cx: &FreqCx| chain_lumped_abcd(&cones, cx.air, cx.omega, lossy))
    } else {
        Box::new(move |cx: &FreqCx| chain_abcd(&cones, cx.air, cx.omega, lossy))
    };
    Ok(Box::new(TwoPort {
        id,
        type_name: "canal",
        port1: (n1, None),
        port2: (n2, None),
        abcd,
        limits,
    }))
}

// ----- Data files -----------------------------------------------------------------

fn parse_data<T: serde::de::DeserializeOwned>(name: &str, text: &str, key: &str) -> T {
    let v: Value =
        serde_json::from_str(text).unwrap_or_else(|e| panic!("data/ear/{name}: invalid JSON: {e}"));
    let part = if key.is_empty() { v } else { v[key].clone() };
    serde_json::from_value(part).unwrap_or_else(|e| panic!("data/ear/{name}: {e}"))
}

// ----- Hudde & Engel eardrum ------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct HeRaw {
    #[serde(rename = "R_tcav_Pa_s_per_m3")]
    r_tcav: f64,
    #[serde(rename = "V_tcav_cm3")]
    v_tcav: f64,
    #[serde(rename = "R_ada_Pa_s_per_m3")]
    r_ada: f64,
    #[serde(rename = "L_ada_kg_per_m4")]
    l_ada: f64,
    #[serde(rename = "V_ant_cm3")]
    v_ant: f64,
    #[serde(rename = "Q_mac")]
    q_mac: f64,
    #[serde(rename = "V_mac_cm3")]
    v_mac: f64,
    #[serde(rename = "f_mac_Hz")]
    f_mac: f64,
    #[serde(rename = "R_ac_Pa_s_per_m3")]
    r_ac: f64,
    #[serde(rename = "C_ac_m3_per_Pa")]
    c_ac: f64,
    #[serde(rename = "L_ac0_kg_per_m4")]
    l_ac0: f64,
    #[serde(rename = "f_Lac_Hz")]
    f_lac: f64,
    #[serde(rename = "f_Yph_Hz")]
    f_yph: f64,
    #[serde(rename = "s_Yph")]
    s_yph: f64,
    #[serde(rename = "A0_mm2")]
    a0: f64,
    #[serde(rename = "A_inf_mm2")]
    a_inf: f64,
    #[serde(rename = "f_A_Hz")]
    f_a: f64,
    #[serde(rename = "Q_A")]
    q_a: f64,
    #[serde(rename = "s_Aph")]
    s_aph: f64,
    #[serde(rename = "f_Aph_Hz")]
    f_aph: f64,
    #[serde(rename = "phi_A_rad")]
    phi_a: f64,
    #[serde(rename = "R_mi_Ns_per_m")]
    r_mi: f64,
    #[serde(rename = "C_mi_m_per_N")]
    c_mi: f64,
    #[serde(rename = "C_oss_m_per_N")]
    c_oss: f64,
    #[serde(rename = "L_oss_kg")]
    l_oss: f64,
    #[serde(rename = "C_cpl_m_per_N")]
    c_cpl: f64,
    #[serde(rename = "R_cpl_Ns_per_m")]
    r_cpl: f64,
    #[serde(rename = "R_free_Ns_per_m")]
    r_free: f64,
    #[serde(rename = "L_free_kg")]
    l_free: f64,
    #[serde(rename = "R_st_Ns_per_m")]
    r_st: f64,
    #[serde(rename = "L_st_kg")]
    l_st: f64,
    #[serde(rename = "C_st_m_per_N")]
    c_st: f64,
    #[serde(rename = "R_c_AF2_Ns_per_m")]
    r_c: f64,
    #[serde(rename = "L_c_AF2_kg")]
    l_c: f64,
    #[serde(rename = "C_c_over_AF2_m_per_N")]
    c_c: f64,
}

/// The Hudde & Engel (1998) eardrum impedance: middle-ear cavities in series
/// with a drum / ossicle / cochlea chain whose effective drum area and shunt
/// admittance have frequency-dependent phase laws. Equations and values as
/// documented in the COMSOL Acoustics Module User's Guide 6.4, Eqs. 2-34 and
/// 2-35 and Table 2-8 (data/ear/hudde_engel.json). SI units; the cavity
/// volumes are air volumes, compliance V/(γP0).
#[derive(Debug, Clone, PartialEq)]
pub struct HuddeEngel {
    pub r_tcav: f64,
    pub v_tcav: f64,
    pub r_ada: f64,
    pub l_ada: f64,
    pub v_ant: f64,
    pub q_mac: f64,
    pub v_mac: f64,
    pub f_mac: f64,
    pub r_ac: f64,
    pub c_ac: f64,
    pub l_ac0: f64,
    pub f_lac: f64,
    pub f_yph: f64,
    pub s_yph: f64,
    pub a0: f64,
    pub a_inf: f64,
    pub f_a: f64,
    pub q_a: f64,
    pub s_aph: f64,
    pub f_aph: f64,
    pub phi_a: f64,
    pub r_mi: f64,
    pub c_mi: f64,
    pub c_oss: f64,
    pub l_oss: f64,
    pub c_cpl: f64,
    pub r_cpl: f64,
    pub r_free: f64,
    pub l_free: f64,
    pub r_st: f64,
    pub l_st: f64,
    pub c_st: f64,
    /// Cochlea, referred to the stapes: A_F²·R_c, A_F²·L_c, C_c/A_F².
    pub r_c: f64,
    pub l_c: f64,
    pub c_c: f64,
}

impl HuddeEngel {
    /// The published parameter set.
    pub fn human() -> &'static HuddeEngel {
        static HE: OnceLock<HuddeEngel> = OnceLock::new();
        HE.get_or_init(|| {
            let r: HeRaw = parse_data(
                "hudde_engel.json",
                include_str!("../../../../data/ear/hudde_engel.json"),
                "parameters",
            );
            HuddeEngel {
                r_tcav: r.r_tcav,
                v_tcav: r.v_tcav * 1e-6,
                r_ada: r.r_ada,
                l_ada: r.l_ada,
                v_ant: r.v_ant * 1e-6,
                q_mac: r.q_mac,
                v_mac: r.v_mac * 1e-6,
                f_mac: r.f_mac,
                r_ac: r.r_ac,
                c_ac: r.c_ac,
                l_ac0: r.l_ac0,
                f_lac: r.f_lac,
                f_yph: r.f_yph,
                s_yph: r.s_yph,
                a0: r.a0 * 1e-6,
                a_inf: r.a_inf * 1e-6,
                f_a: r.f_a,
                q_a: r.q_a,
                s_aph: r.s_aph,
                f_aph: r.f_aph,
                phi_a: r.phi_a,
                r_mi: r.r_mi,
                c_mi: r.c_mi,
                c_oss: r.c_oss,
                l_oss: r.l_oss,
                c_cpl: r.c_cpl,
                r_cpl: r.r_cpl,
                r_free: r.r_free,
                l_free: r.l_free,
                r_st: r.r_st,
                l_st: r.l_st,
                c_st: r.c_st,
                r_c: r.r_c,
                l_c: r.l_c,
                c_c: r.c_c,
            }
        })
    }

    /// Scales every resistance, mass (inertance) and compliance of the drum,
    /// ossicles and cochlea; the cavities are left alone.
    pub fn scaled(&self, s: DrumScales) -> HuddeEngel {
        HuddeEngel {
            r_ac: self.r_ac * s.r,
            r_mi: self.r_mi * s.r,
            r_cpl: self.r_cpl * s.r,
            r_free: self.r_free * s.r,
            r_st: self.r_st * s.r,
            r_c: self.r_c * s.r,
            l_ac0: self.l_ac0 * s.m,
            l_oss: self.l_oss * s.m,
            l_free: self.l_free * s.m,
            l_st: self.l_st * s.m,
            l_c: self.l_c * s.m,
            c_ac: self.c_ac * s.c,
            c_mi: self.c_mi * s.c,
            c_oss: self.c_oss * s.c,
            c_cpl: self.c_cpl * s.c,
            c_st: self.c_st * s.c,
            c_c: self.c_c * s.c,
            ..self.clone()
        }
    }

    /// Middle-ear cavities: tympanic cavity (R_tcav + C_tcav) in parallel
    /// with the aditus (R_ada + jωL_ada) leading to the antrum and the
    /// resonant mastoid air cells.
    pub fn cavity(&self, omega: f64, gamma_p0: f64) -> C64 {
        let jw = J * omega;
        let w_mac = 2.0 * PI * self.f_mac;
        let y_t = (self.r_tcav + (jw * self.v_tcav / gamma_p0).inv()).inv();
        let z_ada = self.r_ada + jw * self.l_ada;
        let y_ant = jw * self.v_ant / gamma_p0
            + jw * (self.v_mac / gamma_p0)
                / (1.0 - (omega / w_mac).powi(2) + jw / (self.q_mac * w_mac));
        (y_t + (z_ada + y_ant.inv()).inv()).inv()
    }

    /// Drum, ossicles and cochlea (the eardrum impedance without the cavities).
    pub fn drum(&self, omega: f64) -> C64 {
        let jw = J * omega;
        let w_a = 2.0 * PI * self.f_a;
        let w_aph = 2.0 * PI * self.f_aph;
        let a = (self.a0 + self.a_inf) / (1.0 - (omega / w_a).powi(2) + jw / (self.q_a * w_a))
            - self.a_inf;
        let a_d = if omega < w_aph {
            a
        } else {
            C64::from_polar(a.norm(), self.s_aph * (omega / w_aph).ln() + self.phi_a)
        };
        let l_ac = self.l_ac0 * (1.0 + (omega / (2.0 * PI * self.f_lac)).sqrt());
        let phi_y = self.s_yph * (1.0 + omega / (2.0 * PI * self.f_yph)).ln();
        let y_ac = C64::from_polar(1.0, phi_y) / (self.r_ac + jw * l_ac + (jw * self.c_ac).inv());
        let y_mi = (self.r_mi + (jw * self.c_mi).inv()).inv();
        let y_cpl = (self.r_cpl + (jw * self.c_cpl).inv()).inv();
        let y_free = (self.r_free + jw * self.l_free).inv();
        let z_dmi = (jw * self.c_oss).inv() + jw * self.l_oss + (y_cpl + y_free).inv();
        let z_dm = z_dmi * (2.0 / 3.0);
        let z_inc = z_dmi * (1.0 / 3.0);
        let z_sc = self.r_st
            + jw * self.l_st
            + (jw * self.c_st).inv()
            + self.r_c
            + jw * self.l_c
            + (jw * self.c_c).inv();
        let k11 = (1.0 + y_mi * z_dm) / a_d;
        let k12 = z_dmi / a_d * (1.0 + y_mi * z_inc * z_dm / z_dmi);
        let k21 = y_ac / a_d * (1.0 + y_mi * z_dm) + a_d * y_mi;
        let k22 =
            y_ac * z_dmi / a_d * (1.0 + y_mi * z_inc * z_dm / z_dmi) + a_d * (1.0 + y_mi * z_inc);
        (k11 * z_sc + k12) / (k21 * z_sc + k22)
    }

    /// Acoustic impedance looking into the eardrum, Pa·s/m³.
    pub fn impedance(&self, omega: f64, gamma_p0: f64) -> C64 {
        self.cavity(omega, gamma_p0) + self.drum(omega)
    }
}

// ----- Type 4.3 drum network --------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct Type43DrumRaw {
    #[serde(rename = "C_m_volume_cm3")]
    v_m: f64,
    #[serde(rename = "R_o_Pa_s_per_m3")]
    r_o: f64,
    #[serde(rename = "M_o_kg_per_m4")]
    m_o: f64,
    #[serde(rename = "C_t_volume_cm3")]
    v_t: f64,
    #[serde(rename = "R_a_Pa_s_per_m3")]
    r_a: f64,
    #[serde(rename = "C_a_volume_cm3")]
    v_a: f64,
}

/// Drum network of the Type 4.3 ear simulator, fitted to ITU-T P.57 Table 5-c
/// (tools/ear/fit_type43.py, data/ear/type43_drum.json):
///
/// Z = Z_cav + 1/(jωC_m + 1/(R_o + jωM_o)),
/// Z_cav = 1/(jωC_t + 1/(R_a + 1/(jωC_a))),
///
/// C_m the drum-membrane compliance, R_o and M_o the path through which the
/// drum couples to the middle-ear cavity, C_t the (exposed) middle-ear
/// cavity, R_a and C_a an aditus resistance and antrum volume. The
/// compliances are air volumes, C = V/(γP0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Type43Drum {
    pub v_m: f64,
    pub r_o: f64,
    pub m_o: f64,
    pub v_t: f64,
    pub r_a: f64,
    pub v_a: f64,
}

impl Type43Drum {
    pub fn fitted() -> &'static Type43Drum {
        static D: OnceLock<Type43Drum> = OnceLock::new();
        D.get_or_init(|| {
            let r: Type43DrumRaw = parse_data(
                "type43_drum.json",
                include_str!("../../../../data/ear/type43_drum.json"),
                "parameters",
            );
            Type43Drum {
                v_m: r.v_m * 1e-6,
                r_o: r.r_o,
                m_o: r.m_o,
                v_t: r.v_t * 1e-6,
                r_a: r.r_a,
                v_a: r.v_a * 1e-6,
            }
        })
    }

    /// Scales the drum's resistance R_o, inertance M_o and compliance C_m.
    pub fn scaled(&self, s: DrumScales) -> Type43Drum {
        Type43Drum {
            r_o: self.r_o * s.r,
            m_o: self.m_o * s.m,
            v_m: self.v_m * s.c,
            ..*self
        }
    }

    pub fn impedance(&self, omega: f64, gamma_p0: f64) -> C64 {
        let jw = J * omega;
        let z_a = self.r_a + (jw * self.v_a / gamma_p0).inv();
        let z_cav = (jw * self.v_t / gamma_p0 + z_a.inv()).inv();
        z_cav + (jw * self.v_m / gamma_p0 + (self.r_o + jw * self.m_o).inv()).inv()
    }
}

// ----- IEC 60318-4 literature model -------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct Iec711GeomRaw {
    #[serde(rename = "R0")]
    r0: f64,
    #[serde(rename = "L1")]
    l1: f64,
    #[serde(rename = "L3")]
    l3: f64,
    #[serde(rename = "L5")]
    l5: f64,
    a2: f64,
    b2: f64,
    h2: f64,
    r2: f64,
    #[serde(rename = "R2")]
    big_r2: f64,
    d1: f64,
    r4: f64,
    h4: f64,
    #[serde(rename = "R4")]
    big_r4: f64,
    d2: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct Iec711SlitRaw {
    parts: usize,
    part_angle_deg: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct MicRaw {
    #[serde(rename = "C_m3_per_Pa")]
    c: f64,
    #[serde(rename = "R_Pa_s_per_m3")]
    r: f64,
    #[serde(rename = "M_kg_per_m4")]
    m: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct Iec711FitRaw {
    side_volume_scale: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct Iec711Raw {
    geometry_mm: Iec711GeomRaw,
    annular_slit: Iec711SlitRaw,
    microphone_bk4192: MicRaw,
    fit: Iec711FitRaw,
}

/// Microphone termination as a series R–M–C acoustic impedance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Microphone {
    pub c: f64,
    pub r: f64,
    pub m: f64,
}

impl Microphone {
    pub fn admittance(&self, omega: f64) -> C64 {
        let jw = J * omega;
        (self.r + jw * self.m + (jw * self.c).inv()).inv()
    }
}

/// Transfer-matrix model of the IEC 60318-4 occluded-ear simulator: main
/// cavity sections L1, L3, L5 in cascade with two shunt Helmholtz branches
/// (Nielsen et al. 2004; Luan et al. 2019 Eq. 6), terminated by the
/// microphone. Branch 2: rectangular slit (a2 × b2 × h2, end corrections of
/// Luan Eq. A.7 at both ends) into an annular cavity (r2..R2, thickness d1).
/// Branch 4: radial slit of gap h4 from R0 to r4 over `parts` arcs, into an
/// annular cavity (r4..R4, thickness d2). Slits use the thermoviscous slit
/// medium; cavities are compliances with the thermal wall layer. Lengths in
/// metres (data/ear/iec60318_4.json).
#[derive(Debug, Clone, PartialEq)]
pub struct Iec711 {
    pub r0: f64,
    pub l1: f64,
    pub l3: f64,
    pub l5: f64,
    pub a2: f64,
    pub b2: f64,
    pub h2: f64,
    pub r2: f64,
    pub big_r2: f64,
    pub d1: f64,
    pub r4: f64,
    pub h4: f64,
    pub big_r4: f64,
    pub d2: f64,
    pub parts4: usize,
    pub part_angle4: f64,
    /// Microphone termination; `None` is rigid.
    pub mic: Option<Microphone>,
}

/// Number of stepped segments of the radial slit (its radial length is
/// under 1 mm, so the chain is converged far beyond test tolerances).
const RADIAL_SLIT_STEPS: usize = 32;

/// End correction of a baffled rectangular opening of sides h (narrow) and b:
/// Munjal et al., Formulas of Acoustics (2008) p. 319, as Luan et al. Eq. A.7.
pub fn rect_end_correction(h: f64, b: f64) -> f64 {
    let beta = h / b;
    let eps = 1.0 + beta * beta;
    let t1 = (beta + (1.0 - eps.powf(1.5)) / (beta * beta)) / (3.0 * PI);
    let t2 = ((beta + eps.sqrt()).ln() / beta + ((1.0 + eps.sqrt()) / beta).ln()) / PI;
    h * (t1 + t2)
}

/// Impedance of a closed cavity of volume `v` and wall area `a` with the
/// thermal wall-layer correction (as the lumped `cavity` element).
fn lumped_cavity_impedance(v: f64, a: f64, air: &AirState, omega: f64) -> C64 {
    let eps = (air.gamma - 1.0) * air.thermal_layer(omega) * a / (2.0 * v);
    let c = v / air.bulk_modulus() * C64::new(1.0 + eps, -eps);
    (J * omega * c).inv()
}

fn load_impedance(t: [C64; 4], z: C64) -> C64 {
    (t[0] * z + t[1]) / (t[2] * z + t[3])
}

impl Iec711 {
    /// Literature geometry with the fitted side-volume scale and the
    /// B&K 4192 microphone.
    pub fn literature() -> &'static Iec711 {
        static M: OnceLock<Iec711> = OnceLock::new();
        M.get_or_init(|| {
            let r: Iec711Raw = parse_data(
                "iec60318_4.json",
                include_str!("../../../../data/ear/iec60318_4.json"),
                "",
            );
            let g = r.geometry_mm;
            let mm = 1e-3;
            Iec711 {
                r0: g.r0 * mm,
                l1: g.l1 * mm,
                l3: g.l3 * mm,
                l5: g.l5 * mm,
                a2: g.a2 * mm,
                b2: g.b2 * mm,
                h2: g.h2 * mm,
                r2: g.r2 * mm,
                big_r2: g.big_r2 * mm,
                d1: g.d1 * mm,
                r4: g.r4 * mm,
                h4: g.h4 * mm,
                big_r4: g.big_r4 * mm,
                d2: g.d2 * mm,
                parts4: r.annular_slit.parts,
                part_angle4: r.annular_slit.part_angle_deg.to_radians(),
                mic: Some(Microphone {
                    c: r.microphone_bk4192.c,
                    r: r.microphone_bk4192.r,
                    m: r.microphone_bk4192.m,
                }),
            }
            .with_side_volume_scale(r.fit.side_volume_scale)
        })
    }

    /// The fitted side-volume scale recorded in data/ear/iec60318_4.json.
    pub fn fitted_side_volume_scale() -> f64 {
        static S: OnceLock<f64> = OnceLock::new();
        *S.get_or_init(|| {
            let r: Iec711Raw = parse_data(
                "iec60318_4.json",
                include_str!("../../../../data/ear/iec60318_4.json"),
                "",
            );
            r.fit.side_volume_scale
        })
    }

    /// Multiplies both side-cavity thicknesses (hence volumes) by `s`.
    pub fn with_side_volume_scale(&self, s: f64) -> Iec711 {
        Iec711 {
            d1: self.d1 * s,
            d2: self.d2 * s,
            ..self.clone()
        }
    }

    fn annulus(inner: f64, outer: f64, d: f64) -> (f64, f64) {
        let face = PI * (outer * outer - inner * inner);
        (face * d, 2.0 * face + 2.0 * PI * (outer + inner) * d)
    }

    /// Input impedance of the rectangular-slit Helmholtz branch.
    pub fn branch2_impedance(&self, air: &AirState, omega: f64) -> C64 {
        let dl = rect_end_correction(self.h2, self.b2);
        let neck = thermoviscous::abcd(
            &Section::Slit {
                gap: self.h2,
                width: self.b2,
            },
            air,
            omega,
            self.a2 + 2.0 * dl,
        );
        let (v, a) = Self::annulus(self.r2, self.big_r2, self.d1);
        load_impedance(neck, lumped_cavity_impedance(v, a, air, omega))
    }

    /// Input impedance of the radial-slit Helmholtz branch.
    pub fn branch4_impedance(&self, air: &AirState, omega: f64) -> C64 {
        let angle = self.parts4 as f64 * self.part_angle4;
        let r_in = self.r0 - rect_end_correction(self.h4, self.part_angle4 * self.r0);
        let r_out = self.r4 + rect_end_correction(self.h4, self.part_angle4 * self.r4);
        let dr = (r_out - r_in) / RADIAL_SLIT_STEPS as f64;
        let mut neck = [ONE, ZERO, ZERO, ONE];
        for k in 0..RADIAL_SLIT_STEPS {
            let rm = r_in + (k as f64 + 0.5) * dr;
            let seg = thermoviscous::abcd(
                &Section::Slit {
                    gap: self.h4,
                    width: angle * rm,
                },
                air,
                omega,
                dr,
            );
            neck = abcd_mul(neck, seg);
        }
        let (v, a) = Self::annulus(self.r4, self.big_r4, self.d2);
        load_impedance(neck, lumped_cavity_impedance(v, a, air, omega))
    }

    /// Transfer matrix from the reference plane to the microphone plane
    /// (microphone not included).
    pub fn abcd(&self, air: &AirState, omega: f64) -> [C64; 4] {
        let main = Section::Circle { radius: self.r0 };
        let t1 = thermoviscous::abcd(&main, air, omega, self.l1);
        let t3 = thermoviscous::abcd(&main, air, omega, self.l3);
        let t5 = thermoviscous::abcd(&main, air, omega, self.l5);
        let y2 = self.branch2_impedance(air, omega).inv();
        let y4 = self.branch4_impedance(air, omega).inv();
        [t1, abcd_shunt(y2), t3, abcd_shunt(y4), t5]
            .into_iter()
            .reduce(abcd_mul)
            .expect("five factors")
    }

    pub fn mic_admittance(&self, omega: f64) -> C64 {
        self.mic.map_or(ZERO, |m| m.admittance(omega))
    }

    /// Transfer impedance p_mic / U_ref.
    pub fn transfer_impedance(&self, air: &AirState, omega: f64) -> C64 {
        let t = self.abcd(air, omega);
        (t[2] + t[3] * self.mic_admittance(omega)).inv()
    }

    /// Input impedance at the reference plane.
    pub fn input_impedance(&self, air: &AirState, omega: f64) -> C64 {
        let t = self.abcd(air, omega);
        let y = self.mic_admittance(omega);
        (t[0] + t[1] * y) / (t[2] + t[3] * y)
    }

    /// Equivalent drum termination: the two Helmholtz branches and the
    /// microphone lumped at one plane (the "traditional" tympanic impedance
    /// of Luan et al. 2019, model 3). For diagnostics only.
    pub fn drum_admittance(&self, air: &AirState, omega: f64) -> C64 {
        self.branch2_impedance(air, omega).inv()
            + self.branch4_impedance(air, omega).inv()
            + self.mic_admittance(omega)
    }
}

// ----- Type 4.3 geometry ------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct SectionRaw {
    position_mm: f64,
    area_mm2: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct Type43GeomRaw {
    ref_plane_mm: f64,
    eep_mm: f64,
    drp_axial_mm: f64,
    sections: Vec<SectionRaw>,
}

/// ITU-T P.57 Type 4.3 canal: area function derived from Annex B (positions
/// from the canal tip along the curved centre line), with the reference
/// plane, the EEP and the DRP's axial position (data/ear/type43_geometry.json).
#[derive(Debug, Clone)]
pub struct Type43Geometry {
    pub profile: AreaFunction,
    pub x_tip: f64,
    pub x_drp: f64,
    pub x_ref: f64,
    pub x_eep: f64,
}

impl Type43Geometry {
    pub fn p57() -> &'static Type43Geometry {
        static G: OnceLock<Type43Geometry> = OnceLock::new();
        G.get_or_init(|| {
            let r: Type43GeomRaw = parse_data(
                "type43_geometry.json",
                include_str!("../../../../data/ear/type43_geometry.json"),
                "",
            );
            let profile = AreaFunction::new(
                r.sections.iter().map(|s| s.position_mm * 1e-3).collect(),
                r.sections.iter().map(|s| s.area_mm2 * 1e-6).collect(),
            )
            .expect("data/ear/type43_geometry.json: valid area function");
            Type43Geometry {
                x_tip: profile.positions()[0],
                x_drp: r.drp_axial_mm * 1e-3,
                x_ref: r.ref_plane_mm * 1e-3,
                x_eep: r.eep_mm * 1e-3,
                profile,
            }
        })
    }

    /// Cones of the canal from `x_from` to `x_to`, with the segment count
    /// shared out over the whole modelled canal (tip to EEP) by length:
    /// max(1, ⌊n·ℓ/L_total + ½⌋).
    pub fn cones(&self, x_from: f64, x_to: f64, n_total: usize) -> Vec<Cone> {
        let total = self.x_eep - self.x_tip;
        let len = (x_to - x_from).abs();
        let n = ((n_total as f64 * len / total + 0.5).floor() as usize).max(1);
        self.profile.sub(x_from, x_to).segments(n)
    }
}

/// Admittance of a rigid-ended cone chain seen from its first end: C/A.
fn stub_admittance(cones: &[Cone], air: &AirState, omega: f64, lossy: bool) -> C64 {
    let t = chain_abcd(cones, air, omega, lossy);
    t[2] / t[0]
}

// ----- Drum terminations ------------------------------------------------------------

/// User scale factors on a drum model's resistive, mass and compliance parts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrumScales {
    pub r: f64,
    pub m: f64,
    pub c: f64,
}

impl Default for DrumScales {
    fn default() -> Self {
        DrumScales {
            r: 1.0,
            m: 1.0,
            c: 1.0,
        }
    }
}

/// A drum termination.
#[derive(Debug, Clone)]
pub enum DrumModel {
    Rigid,
    HuddeEngel(Box<HuddeEngel>),
    Type43(Type43Drum),
    Iec60318Part4(Box<Iec711>),
}

impl DrumModel {
    pub fn admittance(&self, air: &AirState, omega: f64) -> C64 {
        let g = air.bulk_modulus();
        match self {
            DrumModel::Rigid => ZERO,
            DrumModel::HuddeEngel(m) => m.impedance(omega, g).inv(),
            DrumModel::Type43(m) => m.impedance(omega, g).inv(),
            DrumModel::Iec60318Part4(m) => m.drum_admittance(air, omega),
        }
    }

    fn limits(&self, element: &str) -> Vec<ValidityLimit> {
        let one = |criterion, begin: f64, deep: Option<f64>| {
            vec![ValidityLimit {
                element: element.to_string(),
                criterion,
                begin_hz: Some(begin),
                deep_hz: deep,
            }]
        };
        match self {
            DrumModel::Rigid => Vec::new(),
            DrumModel::HuddeEngel(_) => {
                one("Hudde & Engel eardrum: defined to 16 kHz", 16_000.0, None)
            }
            DrumModel::Type43(_) => one("Type 4.3 drum: fitted 20 Hz to 20 kHz", 20_000.0, None),
            DrumModel::Iec60318Part4(_) => one(
                "IEC 60318-4 equivalent drum: human-valid to 10 kHz, coupler-only above",
                10_000.0,
                Some(16_000.0),
            ),
        }
    }
}

/// Reads the drum scale factors and the middle-ear volume override.
fn drum_options(p: &mut Params) -> Result<(DrumScales, Option<f64>)> {
    let s = DrumScales {
        r: positive_number_or(p, "R_scale", 1.0)?,
        m: positive_number_or(p, "M_scale", 1.0)?,
        c: positive_number_or(p, "C_scale", 1.0)?,
    };
    let v = p.positive_opt("middle_ear_volume", Dim::Volume)?;
    Ok((s, v))
}

fn drum_model(b: &Build, name: &str, s: DrumScales, v_me: Option<f64>) -> Result<DrumModel> {
    let default_scales = s == DrumScales::default();
    Ok(match name {
        "hudde_engel" => {
            let mut m = HuddeEngel::human().scaled(s);
            if let Some(v) = v_me {
                m.v_tcav = v;
            }
            DrumModel::HuddeEngel(Box::new(m))
        }
        "type43" => {
            let mut m = Type43Drum::fitted().scaled(s);
            if let Some(v) = v_me {
                m.v_t = v;
            }
            DrumModel::Type43(m)
        }
        "iec60318_4" | "rigid" => {
            if !default_scales || v_me.is_some() {
                return Err(b.err(format!(
                    "drum model '{name}' takes no scale factors or middle-ear volume"
                )));
            }
            if name == "rigid" {
                DrumModel::Rigid
            } else {
                DrumModel::Iec60318Part4(Box::new(Iec711::literature().clone()))
            }
        }
        other => {
            return Err(b.err(format!(
                "unknown drum model '{other}' (hudde_engel, type43, iec60318_4 or rigid)"
            )))
        }
    })
}

fn eardrum(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let n1 = b.terminal(0, Domain::Acoustic)?;
    let n2 = b.terminal_or_ground(1, Domain::Acoustic)?;
    let name = b
        .params
        .string_opt("model")?
        .unwrap_or_else(|| "hudde_engel".into());
    let (scales, v_me) = drum_options(&mut b.params)?;
    let model = drum_model(&b, &name, scales, v_me)?;
    let id = b.id.clone();
    let limits = model.limits(&id);
    b.finish()?;
    Ok(Box::new(OnePort {
        id,
        type_name: "eardrum",
        n1,
        n2,
        y: Box::new(move |cx: &FreqCx| model.admittance(cx.air, cx.omega)),
        limits,
    }))
}

// ----- Ear-simulator macros -----------------------------------------------------------

/// An ideal short (zero pressure difference) joining a macro's terminal to
/// its named entrance node. Its branch unknown is the flow it delivers out of
/// `p` (see `Mna::potential_source`).
struct Short {
    id: String,
    p: Unknown,
    n: Unknown,
}

impl Element for Short {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "short"
    }
    fn branch_count(&self) -> usize {
        1
    }
    fn stamp(&self, _cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        mna.short(self.p, self.n, br[0]);
    }
    fn port_count(&self) -> usize {
        1
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        (port == 0).then(|| potential(x, self.p) - potential(x, self.n))
    }
    /// Flow entering the short at `p`.
    fn port_flow(&self, _cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        (port == 0).then(|| -x[br[0]])
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One part port contributing to a macro node's flow.
#[derive(Debug, Clone, Copy)]
struct Tap {
    part: usize,
    port: usize,
    sign: f64,
}

/// A node the macro touches, exposed as a port to ambient whose flow is the
/// net flow entering the macro there. Internal nodes with nothing else
/// attached carry zero net flow, so the power balance holds whether or not
/// other elements are connected to them.
struct NodePort {
    node: Unknown,
    taps: Vec<Tap>,
}

/// An ear-simulator macro: parts stamped with contiguous branch unknowns and
/// one port per node it touches (terminal first).
pub struct EarMacro {
    id: String,
    type_name: &'static str,
    parts: Vec<Box<dyn Element>>,
    ports: Vec<NodePort>,
    limits: Vec<ValidityLimit>,
}

impl EarMacro {
    fn offsets(&self) -> Vec<usize> {
        let mut acc = 0;
        self.parts
            .iter()
            .map(|p| {
                let o = acc;
                acc += p.branch_count();
                o
            })
            .collect()
    }
}

impl Element for EarMacro {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        self.type_name
    }
    fn branch_count(&self) -> usize {
        self.parts.iter().map(|p| p.branch_count()).sum()
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        for (p, o) in self.parts.iter().zip(self.offsets()) {
            p.stamp(cx, mna, &br[o..o + p.branch_count()]);
        }
    }
    fn port_count(&self) -> usize {
        self.ports.len()
    }
    /// Pressure of the port's node (all ports are referred to ambient).
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        self.ports.get(port).map(|p| potential(x, p.node))
    }
    /// Net volume velocity entering the macro at the port's node.
    fn port_flow(&self, cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        let p = self.ports.get(port)?;
        let offs = self.offsets();
        let mut sum = ZERO;
        for t in &p.taps {
            let e = &self.parts[t.part];
            let o = offs[t.part];
            sum += e.port_flow(cx, x, &br[o..o + e.branch_count()], t.port)? * t.sign;
        }
        Some(sum)
    }
    fn validity(&self, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        let mut v = self.limits.clone();
        for p in &self.parts {
            v.extend(p.validity(air, level));
        }
        v
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Builder collecting parts and node taps.
struct MacroBuilder {
    parts: Vec<Box<dyn Element>>,
    ports: Vec<NodePort>,
}

impl MacroBuilder {
    fn new(terminal: Unknown) -> Self {
        MacroBuilder {
            parts: Vec::new(),
            ports: vec![NodePort {
                node: terminal,
                taps: Vec::new(),
            }],
        }
    }

    fn tap(&mut self, node: Unknown, t: Tap) {
        match self.ports.iter_mut().find(|p| p.node == node) {
            Some(p) => p.taps.push(t),
            None => self.ports.push(NodePort {
                node,
                taps: vec![t],
            }),
        }
    }

    fn short(&mut self, id: &str, p: Unknown, n: Unknown) {
        let part = self.parts.len();
        self.parts.push(Box::new(Short {
            id: format!("{id} (entrance)"),
            p,
            n,
        }));
        self.tap(
            p,
            Tap {
                part,
                port: 0,
                sign: 1.0,
            },
        );
        self.tap(
            n,
            Tap {
                part,
                port: 0,
                sign: -1.0,
            },
        );
    }

    fn two_port(&mut self, e: TwoPort) {
        let part = self.parts.len();
        let (a, b) = (e.port1.0, e.port2.0);
        self.parts.push(Box::new(e));
        self.tap(
            a,
            Tap {
                part,
                port: 0,
                sign: 1.0,
            },
        );
        self.tap(
            b,
            Tap {
                part,
                port: 1,
                sign: 1.0,
            },
        );
    }

    fn shunt(&mut self, e: OnePort) {
        let part = self.parts.len();
        let a = e.n1;
        self.parts.push(Box::new(e));
        self.tap(
            a,
            Tap {
                part,
                port: 0,
                sign: 1.0,
            },
        );
    }

    fn finish(
        self,
        id: String,
        type_name: &'static str,
        limits: Vec<ValidityLimit>,
    ) -> Box<dyn Element> {
        Box::new(EarMacro {
            id,
            type_name,
            parts: self.parts,
            ports: self.ports,
            limits,
        })
    }
}

fn microphone_option(b: &mut Build) -> Result<bool> {
    match b.params.string_opt("microphone")?.as_deref() {
        None | Some("bk4192") => Ok(true),
        Some("rigid") => Ok(false),
        Some(other) => Err(b.err(format!("unknown microphone '{other}' (bk4192 or rigid)"))),
    }
}

/// Coupler model from the macro parameters `side_volume_scale` and `microphone`.
fn coupler_model(b: &mut Build) -> Result<Iec711> {
    let scale = positive_number_or(
        &mut b.params,
        "side_volume_scale",
        Iec711::fitted_side_volume_scale(),
    )?;
    let mic = microphone_option(b)?;
    let base = Iec711::literature();
    let mut m = base.with_side_volume_scale(scale / Iec711::fitted_side_volume_scale());
    if !mic {
        m.mic = None;
    }
    Ok(m)
}

fn coupler_limits(id: &str, m: &Iec711, air: &AirState) -> Vec<ValidityLimit> {
    let mut v = vec![ValidityLimit {
        element: id.to_string(),
        criterion: "IEC 60318-4 literature model: human-valid 100 Hz to 10 kHz, coupler-only above",
        begin_hz: Some(10_000.0),
        deep_hz: Some(16_000.0),
    }];
    v.extend(canal_limits(id, m.r0, air));
    v
}

/// Adds the coupler (reference plane `rp` → microphone plane `drp`).
fn add_coupler(mb: &mut MacroBuilder, id: &str, rp: Unknown, drp: Unknown, m: &Iec711) {
    let chain = m.clone();
    mb.two_port(TwoPort {
        id: format!("{id} (coupler)"),
        type_name: "iec60318_4",
        port1: (rp, None),
        port2: (drp, None),
        abcd: Box::new(move |cx: &FreqCx| chain.abcd(cx.air, cx.omega)),
        limits: Vec::new(),
    });
    if let Some(mic) = m.mic {
        mb.shunt(OnePort {
            id: format!("{id} (microphone)"),
            type_name: "microphone",
            n1: drp,
            n2: None,
            y: Box::new(move |cx: &FreqCx| mic.admittance(cx.omega)),
            limits: Vec::new(),
        });
    }
}

/// The macro's single terminal: the node the source couples to.
fn macro_terminal(b: &Build) -> Result<Unknown> {
    b.expect_terminals(1, 1)?;
    let term = b.terminal(0, Domain::Acoustic)?;
    if term.is_none() {
        return Err(b.err("an ear simulator cannot be connected to ambient"));
    }
    Ok(term)
}

fn iec60318_4(mut b: Build) -> Result<Box<dyn Element>> {
    let term = macro_terminal(&b)?;
    let m = coupler_model(&mut b)?;
    let eep = b.internal_node("eep", Domain::Acoustic)?;
    let drp = b.internal_node("drp", Domain::Acoustic)?;
    let id = b.id.clone();
    let limits = coupler_limits(&id, &m, &b.air);
    b.finish()?;
    let mut mb = MacroBuilder::new(term);
    mb.short(&id, term, eep);
    add_coupler(&mut mb, &id, eep, drp, &m);
    Ok(mb.finish(id, "iec60318_4", limits))
}

fn type33(mut b: Build) -> Result<Box<dyn Element>> {
    let term = macro_terminal(&b)?;
    let m = coupler_model(&mut b)?;
    // Ear-canal extension: 10.0 mm is stated for Type 3.1 in ITU-T P.57
    // clause 6.3.1 (and used for Type 3.3 by Nielsen & Herring Jensen,
    // DAGA 2022); to be verified for Type 3.3. Bore 7.5 mm as the principal
    // cavity (P.57 clause 6.4.4.4.1).
    let ext_len = b
        .params
        .positive_opt("extension_length", Dim::Length)?
        .unwrap_or(10.0e-3);
    let ext_d = b
        .params
        .positive_opt("extension_diameter", Dim::Length)?
        .unwrap_or(7.5e-3);
    let eep = b.internal_node("eep", Domain::Acoustic)?;
    let rp = b.internal_node("ref", Domain::Acoustic)?;
    let drp = b.internal_node("drp", Domain::Acoustic)?;
    let id = b.id.clone();
    let mut limits = coupler_limits(&id, &m, &b.air);
    limits.extend(canal_limits(&id, 0.5 * ext_d, &b.air));
    b.finish()?;
    let mut mb = MacroBuilder::new(term);
    mb.short(&id, term, eep);
    let ext = Section::Circle {
        radius: 0.5 * ext_d,
    };
    mb.two_port(TwoPort {
        id: format!("{id} (extension)"),
        type_name: "tube",
        port1: (eep, None),
        port2: (rp, None),
        abcd: Box::new(move |cx: &FreqCx| thermoviscous::abcd(&ext, cx.air, cx.omega, ext_len)),
        limits: Vec::new(),
    });
    add_coupler(&mut mb, &id, rp, drp, &m);
    Ok(mb.finish(id, "type33", limits))
}

fn type43(mut b: Build) -> Result<Box<dyn Element>> {
    let term = macro_terminal(&b)?;
    let input = b
        .params
        .string_opt("input")?
        .unwrap_or_else(|| "eep".into());
    if input != "eep" && input != "ref" {
        return Err(b.err("'input' must be 'eep' or 'ref'"));
    }
    let n_total = b.params.count_or("segments", 48)?;
    if n_total > 5000 {
        return Err(b.err("'segments' must be at most 5000"));
    }
    let lossy = b.params.bool_or("wall_loss", true)?;
    let drum_name = b
        .params
        .string_opt("drum")?
        .unwrap_or_else(|| "type43".into());
    let (scales, v_me) = drum_options(&mut b.params)?;
    let drum = drum_model(&b, &drum_name, scales, v_me)?;
    let g = Type43Geometry::p57();
    let eep = if input == "eep" {
        Some(b.internal_node("eep", Domain::Acoustic)?)
    } else {
        None
    };
    let rp = b.internal_node("ref", Domain::Acoustic)?;
    let drp = b.internal_node("drp", Domain::Acoustic)?;
    let id = b.id.clone();

    let outer = g.cones(g.x_eep, g.x_ref, n_total);
    let inner = g.cones(g.x_ref, g.x_drp, n_total);
    let tip = g.cones(g.x_drp, g.x_tip, n_total);
    let mut limits = vec![ValidityLimit {
        element: id.clone(),
        criterion: "ITU-T P.57 Type 4.3: specified 20 Hz to 20 kHz",
        begin_hz: Some(20_000.0),
        deep_hz: None,
    }];
    limits.extend(drum.limits(&id));
    let r_max = |c: &[Cone]| c.iter().map(|c| c.r1.max(c.r2)).fold(0.0, f64::max);
    let widest = if eep.is_some() {
        r_max(&outer).max(r_max(&inner))
    } else {
        r_max(&inner)
    };
    limits.extend(canal_limits(&id, widest, &b.air));
    b.finish()?;

    let mut mb = MacroBuilder::new(term);
    match eep {
        Some(eep) => {
            mb.short(&id, term, eep);
            mb.two_port(TwoPort {
                id: format!("{id} (canal EEP-ref)"),
                type_name: "canal",
                port1: (eep, None),
                port2: (rp, None),
                abcd: Box::new(move |cx: &FreqCx| chain_abcd(&outer, cx.air, cx.omega, lossy)),
                limits: Vec::new(),
            });
        }
        None => mb.short(&id, term, rp),
    }
    mb.two_port(TwoPort {
        id: format!("{id} (canal ref-DRP)"),
        type_name: "canal",
        port1: (rp, None),
        port2: (drp, None),
        abcd: Box::new(move |cx: &FreqCx| chain_abcd(&inner, cx.air, cx.omega, lossy)),
        limits: Vec::new(),
    });
    mb.shunt(OnePort {
        id: format!("{id} (tip stub)"),
        type_name: "canal",
        n1: drp,
        n2: None,
        y: Box::new(move |cx: &FreqCx| stub_admittance(&tip, cx.air, cx.omega, lossy)),
        limits: Vec::new(),
    });
    mb.shunt(OnePort {
        id: format!("{id} (drum)"),
        type_name: "eardrum",
        n1: drp,
        n2: None,
        y: Box::new(move |cx: &FreqCx| drum.admittance(cx.air, cx.omega)),
        limits: Vec::new(),
    });
    Ok(mb.finish(id, "type43", limits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cone_q_series_matches_direct_form_at_switch() {
        // At |u| = 0.1 the direct form loses about three digits to
        // cancellation (u cosh u and sinh u agree to 3e-3), leaving ~1e-13.
        for th in [0.0, 0.7, 1.5, 2.9] {
            let u = C64::from_polar(0.1, th);
            let a = cone_q_series(u);
            let b = (u * u.cosh() - u.sinh()) / (u * u);
            assert!((a - b).norm() < 1e-12 * a.norm(), "{a} vs {b}");
        }
    }

    #[test]
    fn end_correction_of_a_square_is_close_to_the_circular_piston() {
        // Square of side h: Munjal's formula gives 0.4732 h; a circular
        // piston of equal area has 8a/(3π) = 0.4789 h.
        let d = rect_end_correction(1.0, 1.0);
        assert!((d - 0.4732).abs() < 1e-4, "{d}");
    }

    #[test]
    fn segments_keep_knots_and_cover_the_length() {
        let f = AreaFunction::new(vec![0.0, 1.0, 3.0], vec![1.0, 2.0, 4.0]).unwrap();
        let s = f.segments(6);
        assert_eq!(s.len(), 6);
        let total: f64 = s.iter().map(|c| c.length).sum();
        assert!((total - 3.0).abs() < 1e-12);
        // Knot at x = 1 is a boundary: after two unit-span halves.
        assert!((s[1].r2 - (2.0 / PI).sqrt()).abs() < 1e-12);
        let r = f.sub(3.0, 0.5);
        assert_eq!(r.positions(), &[3.0, 1.0, 0.5]);
    }
}
