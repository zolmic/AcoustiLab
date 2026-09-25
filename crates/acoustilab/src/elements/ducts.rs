//! Thermoviscous ducts: circular tubes, slits, area steps, vents and pad
//! leaks (spec Sections 6 and 8).
//!
//! * L0: lumped series impedance jωρ_eff·l/S — frequency-dependent
//!   resistance plus inertance, compressibility neglected.
//! * L1: the full transmission-line two-port.
//!
//! Tubes take optional end corrections: `"flanged"` (0.8216a, Norris &
//! Sheng / Nomura et al.), `"piston"` (8a/3π ≈ 0.8488a, the low-frequency
//! baffled-piston limit the spec's worked examples use) or `"unflanged"`
//! (0.6127a, the exact value of Levine & Schwinger's integral; they printed
//! 0.6133a). An outlet that radiates should use
//! `"none"` and connect a `radiation` element instead, whose reactance
//! already contains the end correction.
//!
//! Each corrected end can also carry the viscous end resistance of the flow
//! over the baffle surface around the orifice (key `end_resistance`, see
//! [`EndResistance`]). The macros `vent` (tube + inner end correction +
//! radiation outlet + optional mesh) and `leak` (parallel slit segments
//! around a pad perimeter) build on the same duct model, and `area_step`
//! is the static discontinuity inertance at a change of cross-section.

use super::materials::{Mesh, MeshModel};
use super::radiation::{radiation_impedance, Baffle};
use super::{Build, Composite, CompositePort, Constructor, Element, FreqCx, OnePort, TwoPort};
use crate::air::AirState;
use crate::error::Result;
use crate::mna::{abcd_mul, abcd_series, potential, Mna, Unknown};
use crate::netlist::Domain;
use crate::thermoviscous::{self, Section};
use crate::units::{Dim, Params};
use crate::validity::{self, ValidityLimit};
use crate::C64;
use serde_json::Value;
use std::any::Any;
use std::f64::consts::PI;

pub const TYPES: &[&str] = &["tube", "slit", "rect_duct", "area_step", "vent", "leak"];

pub fn constructor(ty: &str) -> Option<Constructor> {
    Some(match ty {
        "tube" => tube,
        "slit" => slit,
        "rect_duct" => rect_duct,
        "area_step" => area_step,
        "vent" => vent,
        "leak" => leak,
        _ => return None,
    })
}

/// End-correction length for a tube end of radius `a`.
pub fn end_correction(kind: &str, a: f64) -> Option<f64> {
    Some(match kind {
        "none" => 0.0,
        "flanged" => 0.8216 * a,
        "piston" => 8.0 * a / (3.0 * PI),
        "unflanged" => 0.6127 * a,
        _ => return None,
    })
}

/// Ingard's surface resistance R_s = ½·sqrt(2μρω) = sqrt(μρω/2), in Pa·s/m:
/// the viscous resistance per unit area of the oscillating flow over a rigid
/// wall, which Ingard (1953, "On the theory and design of acoustic
/// resonators", JASA 25, 1037) applies to the baffle surface around an
/// orifice.
pub fn surface_resistance(air: &AirState, omega: f64) -> f64 {
    (air.mu * air.rho * omega / 2.0).sqrt()
}

/// Resistive end correction of an orifice end, as a multiple of Ingard's
/// surface resistance R_s referred to the hole area (specific resistance
/// R_s per end means acoustic R_s/S per end).
///
/// The literature writes the total for both ends of a hole as α·R_s/2 with
/// 2 ≤ α ≤ 4 (see Temiz et al. 2015, JASA 138, 3668, and the MPP
/// literature they review):
/// * `Ingard`: α = 4, i.e. R_s per end — Ingard's value, found appropriate
///   for sharp-edged holes.
/// * `Maa`: α = 2, i.e. R_s/2 per end — the term (√2/32)·k·d/t of Maa's
///   (1998) micro-perforate resistance, which equals R_s in total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndResistance {
    None,
    Maa,
    Ingard,
}

impl EndResistance {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => EndResistance::None,
            "maa" => EndResistance::Maa,
            "ingard" => EndResistance::Ingard,
            _ => return None,
        })
    }

    /// Multiple of R_s added at each end.
    pub fn per_end(self) -> f64 {
        match self {
            EndResistance::None => 0.0,
            EndResistance::Maa => 0.5,
            EndResistance::Ingard => 1.0,
        }
    }

    /// Reads the optional `end_resistance` key.
    pub fn from_params(p: &mut Params, default: EndResistance) -> Result<Self> {
        match p.string_opt("end_resistance")? {
            None => Ok(default),
            Some(s) => EndResistance::parse(&s).ok_or_else(|| {
                crate::Error::element(
                    p.context().to_string(),
                    format!("end_resistance '{s}' must be none, maa or ingard"),
                )
            }),
        }
    }
}

/// Static end correction of a coaxial area step, δ/a, as a function of the
/// radius ratio α = a/b (a the smaller radius). The excess inertance of the
/// evanescent fields on both sides of the step is ρ·δ/(πa²): the
/// discontinuity inertance of Karal (1953, JASA 25, 327).
///
/// Karal represented the step with a uniform-velocity piston, which by the
/// variational principle over-estimates the inertance (δ → 8a/3π as α → 0).
/// The engine uses the exact static value instead, computed by the Galerkin
/// mode-matching script `tools/materials/end_corrections.py` and fitted as
/// 0.82159·(1 − α)·(1 + Σ c_k α^k), k = 1..6. The α → 0 limit 0.82159a is
/// the exact flanged-pipe end correction (Norris & Sheng 1989). The fit is
/// within 2.6e-4·a of the computed table for 0 < α ≤ 0.95 (the test
/// `area_step_matches_mode_matching_table` checks it against the fixture).
pub fn step_end_correction(alpha: f64) -> f64 {
    const C: [f64; 6] = [
        -0.325_627, -0.677_056, 1.722_617, -4.452_943, 4.953_099, -2.188_302,
    ];
    let a = alpha.clamp(0.0, 1.0);
    let mut poly = 1.0;
    let mut p = 1.0;
    for c in C {
        p *= a;
        poly += c * p;
    }
    0.82159 * (1.0 - a) * poly
}

/// Low-frequency end correction of one end of a slit of `gap` × `width`
/// opening into a rigid baffle: the radiation mass of a uniformly moving
/// rectangular piston, δ = I/(2π·w·h), where
/// I = ∬∬ dS dS'/|r − r'| = (2/3)(w³ + h³ − d³) + 2wh²·asinh(w/h) +
/// 2w²h·asinh(h/w), d = sqrt(w² + h²).
///
/// For a circle the same construction gives 8a/3π, the piston value, so this
/// shares its small over-estimate against the exact (non-uniform velocity)
/// solution (0.8216 vs 0.8488 for a circle, about 3 %). For w ≫ h it tends
/// to (h/π)·[ln(2w/h) + 1/2]. The closed form of I was checked against
/// direct numerical quadrature.
pub fn slit_end_correction(gap: f64, width: f64) -> f64 {
    let (w, h) = (width, gap);
    let d = w.hypot(h);
    let i = (2.0 / 3.0) * (w.powi(3) + h.powi(3) - d.powi(3))
        + 2.0 * w * h * h * (w / h).asinh()
        + 2.0 * w * w * h * (h / w).asinh();
    i / (2.0 * PI * w * h)
}

/// A duct: section, length, number of identical parallel copies, and the
/// total end-correction length (added as inertance ρδ/S).
#[derive(Debug, Clone, Copy)]
pub struct Duct {
    pub section: Section,
    pub length: f64,
    pub count: usize,
    pub end_length: f64,
}

impl Duct {
    /// Series impedance of the lumped representation (all copies in parallel).
    pub fn lumped_impedance(&self, air: &AirState, omega: f64) -> C64 {
        let s = self.section.area();
        let z = thermoviscous::lumped_series_impedance(&self.section, air, omega, self.length)
            + C64::new(0.0, omega * air.rho * self.end_length / s);
        z / self.count as f64
    }

    /// Transfer matrix of the distributed representation (all copies).
    pub fn abcd(&self, air: &AirState, omega: f64) -> [C64; 4] {
        let s = self.section.area();
        let half_end = abcd_series(C64::new(0.0, omega * air.rho * 0.5 * self.end_length / s));
        let line = thermoviscous::abcd(&self.section, air, omega, self.length);
        let [a, b, c, d] = abcd_mul(abcd_mul(half_end, line), half_end);
        let n = self.count as f64;
        [a, b / n, c * n, d]
    }

    /// Validity of the lumped (L0) representation: where the inertance
    /// error |tan(x)/x − 1| reaches 10 % and 36 %, with x = −jΓ·l + k·δ the
    /// complex electrical length. Using the lossy Γ rather than the lossless
    /// k matters for narrow ducts, whose |Γ| exceeds k by the viscous factor.
    pub fn lumped_limit(&self, id: &str, air: &AirState) -> ValidityLimit {
        let error = |f: f64| -> f64 {
            let omega = 2.0 * PI * f;
            let (gamma, _) = thermoviscous::propagation(&self.section, air, omega);
            let x = C64::new(0.0, -1.0) * gamma * self.length
                + C64::new(omega / air.c * self.end_length, 0.0);
            if x.norm() < 1e-4 {
                (x * x / 3.0).norm()
            } else {
                (x.tan() / x - 1.0).norm()
            }
        };
        // Bisection in log frequency between 0.01 Hz and 10 MHz.
        let at = |target: f64| -> Option<f64> {
            let (mut lo, mut hi) = (-2.0f64, 7.0f64);
            if error(10f64.powf(hi)) < target {
                return None;
            }
            if error(10f64.powf(lo)) >= target {
                return Some(10f64.powf(lo));
            }
            for _ in 0..80 {
                let mid = 0.5 * (lo + hi);
                if error(10f64.powf(mid)) < target {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            Some(10f64.powf(0.5 * (lo + hi)))
        };
        ValidityLimit {
            element: id.to_string(),
            criterion: "lumped duct |Γ|l",
            begin_hz: at(validity::BEGIN_ERROR),
            deep_hz: at(validity::DEEP_ERROR),
        }
    }

    pub fn limits(&self, id: &str, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        if level == 0 {
            return vec![self.lumped_limit(id, air)];
        }
        let (cut_on, stinson) = match self.section {
            Section::Circle { radius } => (
                validity::circular_cut_on(radius, air.c),
                validity::stinson_bound(radius),
            ),
            Section::Slit { gap, width } => {
                (air.c / (2.0 * width), validity::stinson_bound(0.5 * gap))
            }
            Section::Rect { a, b } => (
                air.c / (2.0 * a.max(b)),
                validity::stinson_bound(0.5 * a.min(b)),
            ),
            Section::Equivalent { area, perimeter } => {
                let r = 2.0 * area / perimeter;
                (
                    validity::circular_cut_on(r, air.c),
                    validity::stinson_bound(r),
                )
            }
        };
        vec![
            ValidityLimit {
                element: id.to_string(),
                criterion: "duct: first transverse mode",
                begin_hz: Some(0.7 * cut_on),
                deep_hz: Some(cut_on),
            },
            ValidityLimit {
                element: id.to_string(),
                criterion: "duct: Stinson low-reduced-frequency bound",
                begin_hz: Some(0.8 * stinson),
                deep_hz: Some(stinson),
            },
        ]
    }
}

/// A [`Duct`] with a resistive end correction: `end_rs` multiples of
/// Ingard's surface resistance R_s, summed over both ends, each referred to
/// the section area of one copy.
#[derive(Debug, Clone, Copy)]
pub struct DuctModel {
    pub duct: Duct,
    pub end_rs: f64,
}

impl DuctModel {
    pub fn lossless_ends(duct: Duct) -> Self {
        DuctModel { duct, end_rs: 0.0 }
    }

    /// Total end resistance of all copies in parallel, Pa·s/m³.
    pub fn end_resistance(&self, air: &AirState, omega: f64) -> f64 {
        self.end_rs * surface_resistance(air, omega)
            / (self.duct.section.area() * self.duct.count as f64)
    }

    pub fn lumped_impedance(&self, air: &AirState, omega: f64) -> C64 {
        self.duct.lumped_impedance(air, omega) + self.end_resistance(air, omega)
    }

    pub fn abcd(&self, air: &AirState, omega: f64) -> [C64; 4] {
        let line = self.duct.abcd(air, omega);
        if self.end_rs == 0.0 {
            return line;
        }
        let half = abcd_series(C64::new(0.5 * self.end_resistance(air, omega), 0.0));
        abcd_mul(abcd_mul(half, line), half)
    }
}

/// The element for a duct model between `n1` and `n2` at the given level.
fn duct_part(
    id: String,
    type_name: &'static str,
    n1: Unknown,
    n2: Unknown,
    model: DuctModel,
    level: u8,
    limits: Vec<ValidityLimit>,
) -> Box<dyn Element> {
    if level == 0 {
        Box::new(OnePort {
            id,
            type_name,
            n1,
            n2,
            y: Box::new(move |cx: &FreqCx| model.lumped_impedance(cx.air, cx.omega).inv()),
            limits,
        })
    } else {
        Box::new(TwoPort {
            id,
            type_name,
            port1: (n1, None),
            port2: (n2, None),
            abcd: Box::new(move |cx: &FreqCx| model.abcd(cx.air, cx.omega)),
            limits,
        })
    }
}

/// Builds the element for a duct between the first two terminals.
fn duct_element(b: Build, type_name: &'static str, model: DuctModel) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let n1 = b.terminal(0, Domain::Acoustic)?;
    let n2 = b.terminal_or_ground(1, Domain::Acoustic)?;
    let id = b.id.clone();
    let level = b.level;
    let limits = model.duct.limits(&id, &b.air, level);
    b.finish()?;
    Ok(duct_part(id, type_name, n1, n2, model, level, limits))
}

/// Reads a circular radius given as `<base>radius` or `<base>diameter`
/// (and optionally `<base>area`).
fn radius_param(b: &mut Build, suffix: &str, allow_area: bool) -> Result<Option<f64>> {
    let p = &mut b.params;
    let r = p.positive_opt(&format!("radius{suffix}"), Dim::Length)?;
    let d = p.positive_opt(&format!("diameter{suffix}"), Dim::Length)?;
    let a = if allow_area {
        p.positive_opt(&format!("area{suffix}"), Dim::Area)?
    } else {
        None
    };
    match (r, d, a) {
        (Some(r), None, None) => Ok(Some(r)),
        (None, Some(d), None) => Ok(Some(0.5 * d)),
        (None, None, Some(a)) => Ok(Some((a / PI).sqrt())),
        (None, None, None) => Ok(None),
        _ => Err(b.err(format!(
            "give only one of radius{suffix}, diameter{suffix}{}",
            if allow_area {
                format!(" or area{suffix}")
            } else {
                String::new()
            }
        ))),
    }
}

fn tube(mut b: Build) -> Result<Box<dyn Element>> {
    let radius = radius_param(&mut b, "", false)?
        .ok_or_else(|| b.err("give exactly one of 'radius' or 'diameter'"))?;
    let p = &mut b.params;
    let length = p.positive("length", Dim::Length)?;
    let count = p.count_or("count", 1)?;
    let inlet = p.string_opt("inlet")?.unwrap_or_else(|| "none".into());
    let outlet = p.string_opt("outlet")?.unwrap_or_else(|| "none".into());
    let end_res = EndResistance::from_params(p, EndResistance::None)?;
    let end_length = match (
        end_correction(&inlet, radius),
        end_correction(&outlet, radius),
    ) {
        (Some(i), Some(o)) => i + o,
        _ => return Err(b.err("end corrections must be none, flanged, piston or unflanged")),
    };
    let corrected_ends = [&inlet, &outlet].iter().filter(|k| **k != "none").count();
    if end_res != EndResistance::None && corrected_ends == 0 {
        return Err(b.err(
            "end_resistance applies to ends with an end correction; set inlet and/or outlet",
        ));
    }
    let duct = Duct {
        section: Section::Circle { radius },
        length,
        count,
        end_length,
    };
    let model = DuctModel {
        duct,
        end_rs: end_res.per_end() * corrected_ends as f64,
    };
    duct_element(b, "tube", model)
}

fn slit(mut b: Build) -> Result<Box<dyn Element>> {
    let p = &mut b.params;
    let gap = p.positive("gap", Dim::Length)?;
    let width = p.positive("width", Dim::Length)?;
    let length = p.positive("length", Dim::Length)?;
    let count = p.count_or("count", 1)?;
    if width < 5.0 * gap {
        return Err(b.err("slit model needs width ≥ 5 × gap"));
    }
    let duct = Duct {
        section: Section::Slit { gap, width },
        length,
        count,
        end_length: 0.0,
    };
    duct_element(b, "slit", DuctModel::lossless_ends(duct))
}

/// Rectangular duct with both sides finite (spec Section 8 eyeglass
/// channels; Stinson 1991 double-series shape function).
fn rect_duct(mut b: Build) -> Result<Box<dyn Element>> {
    let p = &mut b.params;
    let a = p.positive("side_a", Dim::Length)?;
    let side_b = p.positive("side_b", Dim::Length)?;
    let length = p.positive("length", Dim::Length)?;
    let count = p.count_or("count", 1)?;
    let duct = Duct {
        section: Section::Rect { a, b: side_b },
        length,
        count,
        end_length: 0.0,
    };
    duct_element(b, "rect_duct", DuctModel::lossless_ends(duct))
}

// ----- Area step -----------------------------------------------------------

/// Excess inertance (kg/m⁴) of a coaxial step between circular sections of
/// radii `r1` and `r2`: ρ·δ(α)/(π·a²) with a the smaller radius.
pub fn area_step_inertance(rho: f64, r1: f64, r2: f64) -> f64 {
    let (a, big) = if r1 < r2 { (r1, r2) } else { (r2, r1) };
    rho * step_end_correction(a / big) * a / (PI * a * a)
}

fn area_step(mut b: Build) -> Result<Box<dyn Element>> {
    let r1 = radius_param(&mut b, "1", true)?
        .ok_or_else(|| b.err("give radius1, diameter1 or area1"))?;
    let r2 = radius_param(&mut b, "2", true)?
        .ok_or_else(|| b.err("give radius2, diameter2 or area2"))?;
    if (r1 - r2).abs() <= 1e-9 * r1.max(r2) {
        return Err(b.err("the two sections must differ"));
    }
    let (n1, n2) = super::acoustic::acoustic_terminals(&b)?;
    let id = b.id.clone();
    let big = r1.max(r2);
    let cut = validity::circular_cut_on(big, b.air.c);
    let limits = vec![ValidityLimit {
        element: id.clone(),
        criterion: "area step: static inertance below the wide duct's first cut-on",
        begin_hz: Some(0.7 * cut),
        deep_hz: Some(cut),
    }];
    b.finish()?;
    Ok(Box::new(OnePort {
        id,
        type_name: "area_step",
        n1,
        n2,
        y: Box::new(move |cx: &FreqCx| (cx.jw() * area_step_inertance(cx.air.rho, r1, r2)).inv()),
        limits,
    }))
}

// ----- Vent macro ------------------------------------------------------------

/// Parses the optional `mesh` of a vent: either a material id (string) or a
/// nested object of mesh keys. The default area is the total hole area.
fn vent_mesh(b: &mut Build, default_area: f64, air: &AirState) -> Result<Option<MeshModel>> {
    let Some(v) = b.params.value_opt("mesh") else {
        return Ok(None);
    };
    let ctx = format!("{}.mesh", b.id);
    let map = match v {
        Value::String(s) => {
            let mut m = serde_json::Map::new();
            m.insert("material".into(), Value::String(s));
            m
        }
        Value::Object(m) => m,
        _ => return Err(b.err("'mesh' must be a material id or an object")),
    };
    let mut p = Params::new(ctx, map);
    let model = MeshModel::from_params(&mut p, air, Some(default_area))?;
    p.finish()?;
    Ok(Some(model))
}

/// Helmholtz vent through a wall: a thermoviscous tube with the inner end
/// correction (`inlet`, default `"flanged"`), an optional mesh over the
/// outer face, and the radiation impedance of the outer end (`baffle`,
/// default `"infinite"`; its reactance is the outer end correction). The
/// viscous end resistance defaults to Maa's (R_s/2 per end), which gives
/// the Q ≈ 8–9 of erratum E22 for the spec's worked vent.
fn vent(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let radius = radius_param(&mut b, "", false)?
        .ok_or_else(|| b.err("give exactly one of 'radius' or 'diameter'"))?;
    let p = &mut b.params;
    let length = p.positive("length", Dim::Length)?;
    let count = p.count_or("count", 1)?;
    let inlet = p.string_opt("inlet")?.unwrap_or_else(|| "flanged".into());
    let baffle = match p.string_opt("baffle")?.as_deref() {
        None | Some("infinite") => Baffle::Infinite,
        Some("free") => Baffle::Free,
        Some(o) => return Err(b.err(format!("unknown baffle '{o}' (infinite, free)"))),
    };
    let end_res = EndResistance::from_params(&mut b.params, EndResistance::Maa)?;
    let inner_len = end_correction(&inlet, radius)
        .ok_or_else(|| b.err("inlet must be none, flanged, piston or unflanged"))?;
    let hole_area = PI * radius * radius;
    let air = b.air;
    let mesh = vent_mesh(&mut b, hole_area * count as f64, &air)?;

    let inner = b.terminal(0, Domain::Acoustic)?;
    let outer = b.terminal_or_ground(1, Domain::Acoustic)?;
    let mouth = b.internal_node("mouth", Domain::Acoustic)?;
    let mesh_node = if mesh.is_some() {
        b.internal_node("mesh", Domain::Acoustic)?
    } else {
        mouth
    };
    let id = b.id.clone();
    let level = b.level;

    let duct = Duct {
        section: Section::Circle { radius },
        length,
        count,
        end_length: inner_len,
    };
    let inner_rs = if inlet == "none" {
        0.0
    } else {
        end_res.per_end()
    };
    let model = DuctModel {
        duct,
        end_rs: inner_rs,
    };
    let mut limits = duct.limits(&format!("{id}.tube"), &air, level);
    if baffle == Baffle::Free {
        let f = 0.5 * air.c / (2.0 * PI * radius);
        limits.push(ValidityLimit {
            element: format!("{id}.radiation"),
            criterion: "unflanged radiation approximation (ka < 0.5)",
            begin_hz: Some(f),
            deep_hz: Some(2.0 * f),
        });
    }
    let mut parts: Vec<Box<dyn Element>> = vec![duct_part(
        format!("{id}.tube"),
        "tube",
        inner,
        mouth,
        model,
        level,
        limits,
    )];
    if let Some(m) = mesh {
        // A `Mesh` part, so that callers can downcast it and read the pore
        // velocity of a solution (spec: flag above about 1 m/s in a vent).
        parts.push(Box::new(Mesh {
            id: format!("{id}.mesh"),
            n1: mouth,
            n2: mesh_node,
            model: m,
        }));
    }
    // Outer end: radiation plus the outer end resistance, oriented from the
    // outside node so its port flow is the flow entering the vent there.
    let n = count as f64;
    let outer_rs = end_res.per_end();
    parts.push(Box::new(OnePort {
        id: format!("{id}.radiation"),
        type_name: "radiation",
        n1: outer,
        n2: mesh_node,
        y: Box::new(move |cx: &FreqCx| {
            let z = radiation_impedance(baffle, radius, cx.air, cx.omega)
                + outer_rs * surface_resistance(cx.air, cx.omega) / hole_area;
            n / z
        }),
        limits: Vec::new(),
    }));
    let last = parts.len() - 1;
    b.finish()?;
    Ok(Box::new(Composite {
        id,
        type_name: "vent",
        parts,
        ports: vec![
            CompositePort {
                plus: inner,
                minus: None,
                flow_from: (0, 0),
            },
            CompositePort {
                plus: outer,
                minus: None,
                flow_from: (last, 0),
            },
        ],
    }))
}

// ----- Leak macro --------------------------------------------------------------

/// Cushion leak: parallel thermoviscous slit segments around the pad
/// perimeter (spec Section 8). Each segment has breadth perimeter/N, depth
/// equal to the pad face width, its own gap (zero means sealed) and, with
/// `ends: "flanged"`, the baffled end correction of [`slit_end_correction`]
/// at both ends.
///
/// Each segment's end correction is that of an isolated aperture: the
/// mutual radiation mass of open neighbours is neglected. For N equal,
/// adjacent open segments of gap h the continuous slit would have
/// (h/π)·ln N more per end (0.66 mm for 1 mm and N = 8), so the default
/// flanged leak slightly under-estimates the mass of a uniform gap.
pub struct Leak {
    pub id: String,
    pub n1: Unknown,
    pub n2: Unknown,
    pub segments: Vec<Duct>,
    pub level: u8,
}

impl Leak {
    /// Total series admittance of the lumped representation.
    pub fn lumped_admittance(&self, air: &AirState, omega: f64) -> C64 {
        self.segments
            .iter()
            .map(|d| d.lumped_impedance(air, omega).inv())
            .sum()
    }
}

impl Element for Leak {
    fn id(&self) -> &str {
        &self.id
    }
    fn type_name(&self) -> &'static str {
        "leak"
    }
    fn branch_count(&self) -> usize {
        if self.level == 0 {
            0
        } else {
            2 * self.segments.len()
        }
    }
    fn stamp(&self, cx: &FreqCx, mna: &mut Mna, br: &[usize]) {
        if self.level == 0 {
            mna.admittance(self.n1, self.n2, self.lumped_admittance(cx.air, cx.omega));
        } else {
            for (i, d) in self.segments.iter().enumerate() {
                mna.two_port_abcd(
                    (self.n1, None),
                    (self.n2, None),
                    (br[2 * i], br[2 * i + 1]),
                    d.abcd(cx.air, cx.omega),
                );
            }
        }
    }
    fn port_count(&self) -> usize {
        2
    }
    fn port_potential(&self, x: &[C64], port: usize) -> Option<C64> {
        match port {
            0 => Some(potential(x, self.n1)),
            1 => Some(potential(x, self.n2)),
            _ => None,
        }
    }
    /// Flow entering the leak at its first node (port 0) or at its second
    /// node (port 1).
    fn port_flow(&self, cx: &FreqCx, x: &[C64], br: &[usize], port: usize) -> Option<C64> {
        if port > 1 {
            return None;
        }
        if self.level == 0 {
            let through = self.lumped_admittance(cx.air, cx.omega)
                * (potential(x, self.n1) - potential(x, self.n2));
            return Some(if port == 0 { through } else { -through });
        }
        // Each segment's first branch unknown enters at n1; its second is
        // the flow it delivers out of n2.
        let n = self.segments.len();
        Some(if port == 0 {
            (0..n).map(|i| x[br[2 * i]]).sum()
        } else {
            -(0..n).map(|i| x[br[2 * i + 1]]).sum::<C64>()
        })
    }
    fn validity(&self, air: &AirState, level: u8) -> Vec<ValidityLimit> {
        // The widest gap sets the Stinson bound; all segments share the
        // breadth and depth.
        self.segments
            .iter()
            .max_by(|a, b| a.section.area().total_cmp(&b.section.area()))
            .map(|d| d.limits(&self.id, air, level))
            .unwrap_or_default()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Reads a list of lengths given as `<base>_<suffix>: [..]`, converting to
/// SI. Zero entries are allowed (sealed segments).
fn length_list(b: &mut Build, base: &str) -> Result<Option<Vec<f64>>> {
    if b.params.value_opt(base).is_some() {
        return Err(b.err(format!("'{base}' needs a unit suffix, e.g. '{base}_mm'")));
    }
    let mut found: Option<(String, Vec<f64>)> = None;
    for (suffix, factor) in Dim::Length.suffixes() {
        let key = format!("{base}_{suffix}");
        if let Some(v) = b.params.value_opt(&key) {
            if let Some((prev, _)) = &found {
                return Err(b.err(format!("both '{prev}' and '{key}' given")));
            }
            let arr = v
                .as_array()
                .ok_or_else(|| b.err(format!("'{key}' must be a list of numbers")))?;
            let vals = arr
                .iter()
                .map(|x| {
                    x.as_f64()
                        .filter(|x| x.is_finite() && *x >= 0.0)
                        .map(|x| x * factor)
                        .ok_or_else(|| {
                            b.err(format!("'{key}' entries must be non-negative numbers"))
                        })
                })
                .collect::<Result<Vec<f64>>>()?;
            found = Some((key, vals));
        }
    }
    Ok(found.map(|(_, v)| v))
}

fn leak(mut b: Build) -> Result<Box<dyn Element>> {
    b.expect_terminals(1, 2)?;
    let gaps_list = length_list(&mut b, "gaps")?;
    let p = &mut b.params;
    let perimeter = p.positive("perimeter", Dim::Length)?;
    let depth = p.positive("depth", Dim::Length)?;
    let gap = p.positive_opt("gap", Dim::Length)?;
    let n_given = p
        .has("segments")
        .then(|| p.count_or("segments", 8))
        .transpose()?;
    let ends = p.string_opt("ends")?.unwrap_or_else(|| "flanged".into());
    let flanged = match ends.as_str() {
        "flanged" => true,
        "none" => false,
        o => return Err(b.err(format!("ends '{o}' must be flanged or none"))),
    };
    let gaps = match (gap, gaps_list) {
        (Some(g), None) => vec![g; n_given.unwrap_or(8)],
        (None, Some(list)) => {
            if let Some(n) = n_given {
                if n != list.len() {
                    return Err(b.err(format!(
                        "'segments' is {n} but {} gaps are listed",
                        list.len()
                    )));
                }
            }
            if list.is_empty() {
                return Err(b.err("the gap list is empty"));
            }
            list
        }
        _ => return Err(b.err("give either 'gap' (uniform) or a 'gaps' list")),
    };
    let width = perimeter / gaps.len() as f64;
    let mut segments = Vec::new();
    for &g in gaps.iter().filter(|g| **g > 0.0) {
        if width < 5.0 * g {
            return Err(b.err(format!(
                "segment breadth {:.3} mm is less than 5 × its gap {:.3} mm; use fewer segments",
                width * 1e3,
                g * 1e3
            )));
        }
        let end_length = if flanged {
            2.0 * slit_end_correction(g, width)
        } else {
            0.0
        };
        segments.push(Duct {
            section: Section::Slit { gap: g, width },
            length: depth,
            count: 1,
            end_length,
        });
    }
    let n1 = b.terminal(0, Domain::Acoustic)?;
    let n2 = b.terminal_or_ground(1, Domain::Acoustic)?;
    let id = b.id.clone();
    let level = b.level;
    b.finish()?;
    Ok(Box::new(Leak {
        id,
        n1,
        n2,
        segments,
        level,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_duct_lumped_limit_uses_lossy_propagation() {
        // A 0.05 mm slit: |Γ| far exceeds k, so the lumped representation
        // fails well below the lossless k·l estimate.
        let air = AirState::spec_reference();
        let duct = Duct {
            section: Section::Slit {
                gap: 0.05e-3,
                width: 10e-3,
            },
            length: 10e-3,
            count: 1,
            end_length: 0.0,
        };
        let lossy = duct.lumped_limit("s", &air).begin_hz.unwrap();
        let lossless = validity::lumped_duct("s", 10e-3, air.c).begin_hz.unwrap();
        assert!(lossy < 0.5 * lossless, "{lossy} vs {lossless}");
        // A wide tube reduces to the lossless estimate within a few percent.
        let wide = Duct {
            section: Section::Circle { radius: 10e-3 },
            length: 20e-3,
            count: 1,
            end_length: 0.0,
        };
        let f = wide.lumped_limit("t", &air).begin_hz.unwrap();
        let f0 = validity::lumped_duct("t", 20e-3, air.c).begin_hz.unwrap();
        assert!((f / f0 - 1.0).abs() < 0.03, "{f} vs {f0}");
    }
}
