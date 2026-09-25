//! Automated readouts (spec Sections 4, 5 and 10; erratum E32).
//!
//! Each readout states its reference and method (also in
//! [`Readouts::methods`]). Values between grid points are refined with exact
//! re-solves, never interpolated: maxima by Brent's method and level
//! crossings by the Illinois method in ln f ([`super::search`]).
//!
//! **Impedance** (default: the impedance the `vsource` sees, i.e. the load
//! after its source impedance):
//!
//! * peaks: interior local maxima of |Z| on the grid that stand at least
//!   [`PEAK_PROMINENCE_DB`] above the higher of the two valleys separating
//!   them from higher ground (topographic prominence), each refined;
//! * in-situ resonance: the first (lowest) such peak; Zmax: the highest;
//! * Re: the `Re_ohm` option, else Re Z extrapolated to 0 Hz from the two
//!   lowest frequencies assuming Re Z − Re ∝ f² (the low-frequency law of a
//!   moving-coil driver behind any passive acoustic load);
//! * Qms, Qes, Qts by the sqrt(r0) method (Small 1972; derivation at
//!   [`crate::ts::extract_ts`]): r0 = Z(fres)/Re, f1 < fres < f2 where
//!   |Z| = Re·√r0, Qms = fres·√r0/(f2 − f1), Qes = Qms/(r0 − 1),
//!   Qts = Qms/r0. Exact for a single lumped resonance with a real Re and
//!   no inductance; in situ, the acoustic load's losses count as mechanical
//!   ones. A note is added when √(f1·f2) is more than 1 % from fres;
//! * nominal impedance |Z(1 kHz)| (exact solve);
//! * minimum |Z| above the resonance (refined unless at the sweep's end);
//! * the rated-impedance check: |Z| below 80 % of the rated impedance
//!   anywhere in the sweep (spec Section 4). The rated impedance is the
//!   `rated_ohm` option, else the drive key's `rated_ohm`.
//!
//! **Drivers**: for every `driver` element, the D0 identities (fs, Qms,
//! Qes, Qts from Re, Bl, Mms, Cms, Rms) and the same sqrt(r0) readouts on
//! its unloaded impedance `Driver::unloaded_impedance` (free air, both
//! acoustic ports at ambient; includes Le, LR-2, creep and D2 when set).
//!
//! **Response** at a pressure probe (the `probe` option, else the netlist's
//! `ui.primary_probe`, else its first pressure probe), under the stated
//! drive:
//!
//! * level at 500 Hz and 1 kHz (exact solves);
//! * sensitivity in dB SPL per volt of source EMF and per milliwatt into
//!   the rated impedance at 500 Hz and 1 kHz: dB/mW = dB/V −
//!   10·log10(1000/Z_rated) (erratum E32: never Re);
//! * bass extension: the frequency where the level is 3 dB below its 500 Hz
//!   level, found by walking the grid down from 500 Hz to the first point
//!   below that level and refining the crossing;
//! * coupled resonance: the frequency of maximum diaphragm velocity per
//!   unit coil current, |v/i|, of the driver (the `driver` option, else the
//!   first `driver` element). Since F = Bl·i, this is the minimum of the
//!   total mechanical impedance the motor drives (suspension, moving mass
//!   and acoustic loads); it does not depend on the drive convention, source
//!   impedance or coil inductance, and for a lossless lumped cavity it is
//!   exactly spec Appendix C2's f_c = (1/2π)·√((1/Cms + Sd²/Caf)/Mms). It is
//!   `robust` when |v/i| at the peak exceeds its values one octave below and
//!   above by at least [`RESONANCE_PROMINENCE_DB`].

use super::search::{brent_max, illinois_root};
use super::{Design, Point};
use crate::circuit::{Probe, ProbeKind};
use crate::drive::{self, DriveInfo, DriveSpec, RATED_IMPEDANCE_FRACTION};
use crate::elements::driver::Driver;
use crate::elements::electrical::VSource;
use crate::error::{Error, Result};
use crate::netlist::Domain;
use crate::solve::{ProbeResult, SolveResult};
use crate::C64;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f64::consts::PI;

/// Minimum prominence of an impedance peak, dB.
pub const PEAK_PROMINENCE_DB: f64 = 0.1;
/// A coupled resonance is robust when |v/i| at the peak is this many dB
/// above its values one octave either side.
pub const RESONANCE_PROMINENCE_DB: f64 = 1.0;
/// Bass extension reference: dB below the 500 Hz level.
pub const BASS_EXTENSION_DB: f64 = 3.0;
/// Reference frequency of the bass extension and the first sensitivity.
pub const REFERENCE_HZ: f64 = 500.0;
/// Convergence of the refinements, in ln f.
const LN_F_TOL: f64 = 1e-10;

/// Options of [`readouts`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadoutOptions {
    /// Pressure probe of the response readouts.
    pub probe: Option<String>,
    /// Impedance probe of the impedance readouts.
    pub impedance: Option<String>,
    /// Rated impedance for dB/mW and the rated-impedance check.
    pub rated_ohm: Option<f64>,
    /// DC resistance for the Q readouts.
    #[serde(rename = "Re_ohm")]
    pub re_ohm: Option<f64>,
    /// Driver element of the coupled resonance.
    pub driver: Option<String>,
}

impl ReadoutOptions {
    pub fn validate(&self) -> std::result::Result<(), String> {
        for (k, v) in [("rated_ohm", self.rated_ohm), ("Re_ohm", self.re_ohm)] {
            if let Some(x) = v {
                if !(x.is_finite() && x > 0.0) {
                    return Err(format!("'{k}' must be positive, got {x}"));
                }
            }
        }
        Ok(())
    }
}

/// A frequency with its validity shading (0 unshaded, 1 light, 2 dark).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AtFrequency {
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    pub value: f64,
    pub shading: u8,
}

/// A refined impedance peak.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Peak {
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    #[serde(rename = "z_ohm")]
    pub z: f64,
    #[serde(rename = "prominence_dB")]
    pub prominence_db: f64,
}

/// sqrt(r0) estimates around one resonance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QEstimate {
    #[serde(rename = "Re_ohm")]
    pub re: f64,
    #[serde(rename = "f_Hz")]
    pub fres: f64,
    pub r0: f64,
    #[serde(rename = "f1_Hz")]
    pub f1: f64,
    #[serde(rename = "f2_Hz")]
    pub f2: f64,
    #[serde(rename = "Qms")]
    pub qms: f64,
    #[serde(rename = "Qes")]
    pub qes: f64,
    #[serde(rename = "Qts")]
    pub qts: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Violation {
    #[serde(rename = "f_min_Hz")]
    pub f_min: f64,
    #[serde(rename = "f_max_Hz")]
    pub f_max: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RatedCheck {
    pub rated_ohm: f64,
    /// 80 % of the rated impedance.
    pub limit_ohm: f64,
    /// Smallest |Z| in the sweep (refined when interior).
    pub z_min_ohm: f64,
    #[serde(rename = "z_min_Hz")]
    pub z_min_hz: f64,
    pub pass: bool,
    /// Grid ranges where |Z| is below the limit.
    pub violations: Vec<Violation>,
}

/// Readouts of one impedance curve.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImpedanceReadouts {
    pub probe: String,
    #[serde(rename = "Re_ohm")]
    pub re: f64,
    pub re_source: String,
    pub peaks: Vec<Peak>,
    /// In-situ resonance: the first peak.
    pub resonance: Option<Peak>,
    /// The highest peak.
    pub z_max: Option<Peak>,
    pub q: Option<QEstimate>,
    #[serde(rename = "z_1kHz_ohm")]
    pub z_1khz: Option<f64>,
    /// Minimum |Z| above the resonance, and whether it is at the sweep's end.
    pub min_above_resonance: Option<AtFrequency>,
    pub min_at_sweep_end: bool,
    pub rated_check: Option<RatedCheck>,
    pub notes: Vec<String>,
}

/// D0 identities of a driver element.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriverD0 {
    #[serde(rename = "fs_Hz")]
    pub fs: f64,
    #[serde(rename = "Qms")]
    pub qms: f64,
    #[serde(rename = "Qes")]
    pub qes: f64,
    #[serde(rename = "Qts")]
    pub qts: f64,
    #[serde(rename = "Re_ohm")]
    pub re: f64,
    #[serde(rename = "Bl_Tm")]
    pub bl: f64,
    #[serde(rename = "Mms_kg")]
    pub mms: f64,
    #[serde(rename = "Cms_m_per_N")]
    pub cms: f64,
    #[serde(rename = "Rms_Ns_per_m")]
    pub rms: f64,
    #[serde(rename = "Sd_m2")]
    pub sd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriverReadouts {
    pub element: String,
    pub model: String,
    pub d0: DriverD0,
    /// Free-air readouts from the unloaded impedance on the netlist's grid.
    pub free_air: Option<QEstimate>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SensitivityReadout {
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    #[serde(rename = "dB_per_V")]
    pub db_per_v: f64,
    #[serde(rename = "dB_per_mW")]
    pub db_per_mw: Option<f64>,
    pub rated_ohm: Option<f64>,
    /// dB/V − dB/mW = 10·log10(1000/Z_rated).
    #[serde(rename = "conversion_dB")]
    pub conversion_db: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoupledResonance {
    #[serde(rename = "f_Hz")]
    pub f_hz: f64,
    pub driver: String,
    /// |v/i| at the peak over the larger of its values one octave either
    /// side.
    #[serde(rename = "prominence_dB")]
    pub prominence_db: f64,
    pub robust: bool,
    pub shading: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResponseReadouts {
    pub probe: String,
    pub probe_source: &'static str,
    #[serde(rename = "level_500Hz_dB")]
    pub level_500: f64,
    #[serde(rename = "level_1kHz_dB")]
    pub level_1k: f64,
    pub sensitivity: Vec<SensitivityReadout>,
    /// The frequency 3 dB below the 500 Hz level (`value` is the level
    /// there, dB SPL).
    pub bass_extension: Option<AtFrequency>,
    pub coupled_resonance: Option<CoupledResonance>,
    pub notes: Vec<String>,
}

/// All readouts of one design point.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Readouts {
    pub drive: DriveInfo,
    pub impedance: Option<ImpedanceReadouts>,
    pub drivers: Vec<DriverReadouts>,
    pub response: Option<ResponseReadouts>,
    pub notes: Vec<String>,
    pub methods: BTreeMap<&'static str, &'static str>,
    pub hash: String,
    pub engine: &'static str,
}

/// Names of the scalar readouts (see [`Readouts::scalars`]); a tornado or a
/// Monte Carlo run can use any of them as a metric.
pub const SCALARS: &[&str] = &[
    "level_500Hz_dB",
    "level_1kHz_dB",
    "sensitivity_500Hz_dB_per_V",
    "sensitivity_1kHz_dB_per_V",
    "sensitivity_500Hz_dB_per_mW",
    "sensitivity_1kHz_dB_per_mW",
    "bass_extension_Hz",
    "coupled_resonance_Hz",
    "z_resonance_Hz",
    "z_resonance_ohm",
    "z_max_Hz",
    "z_max_ohm",
    "Re_ohm",
    "Qms",
    "Qes",
    "Qts",
    "z_1kHz_ohm",
    "z_min_above_resonance_Hz",
    "z_min_above_resonance_ohm",
    "z_min_ohm",
    "z_min_over_rated",
];

impl Readouts {
    /// The scalar readouts by name ([`SCALARS`]); `None` where a readout
    /// does not exist for this design.
    pub fn scalars(&self) -> BTreeMap<String, Option<f64>> {
        let mut m: BTreeMap<String, Option<f64>> =
            SCALARS.iter().map(|k| (k.to_string(), None)).collect();
        let mut set = |k: &str, v: Option<f64>| {
            m.insert(k.to_string(), v.filter(|x| x.is_finite()));
        };
        if let Some(r) = &self.response {
            set("level_500Hz_dB", Some(r.level_500));
            set("level_1kHz_dB", Some(r.level_1k));
            for s in &r.sensitivity {
                let tag = if s.f_hz == 500.0 { "500Hz" } else { "1kHz" };
                set(&format!("sensitivity_{tag}_dB_per_V"), Some(s.db_per_v));
                set(&format!("sensitivity_{tag}_dB_per_mW"), s.db_per_mw);
            }
            set("bass_extension_Hz", r.bass_extension.map(|b| b.f_hz));
            set(
                "coupled_resonance_Hz",
                r.coupled_resonance.as_ref().map(|c| c.f_hz),
            );
        }
        if let Some(z) = &self.impedance {
            set("z_resonance_Hz", z.resonance.map(|p| p.f_hz));
            set("z_resonance_ohm", z.resonance.map(|p| p.z));
            set("z_max_Hz", z.z_max.map(|p| p.f_hz));
            set("z_max_ohm", z.z_max.map(|p| p.z));
            set("Re_ohm", Some(z.re));
            set("Qms", z.q.as_ref().map(|q| q.qms));
            set("Qes", z.q.as_ref().map(|q| q.qes));
            set("Qts", z.q.as_ref().map(|q| q.qts));
            set("z_1kHz_ohm", z.z_1khz);
            set(
                "z_min_above_resonance_Hz",
                z.min_above_resonance.map(|m| m.f_hz),
            );
            set(
                "z_min_above_resonance_ohm",
                z.min_above_resonance.map(|m| m.value),
            );
            if let Some(c) = &z.rated_check {
                set("z_min_ohm", Some(c.z_min_ohm));
                set("z_min_over_rated", Some(c.z_min_ohm / c.rated_ohm));
            }
        }
        m
    }
}

fn methods() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("peaks", "interior local maxima of |Z| with at least 0.1 dB topographic prominence, refined by Brent's method on exact solves"),
        ("resonance", "in-situ resonance: the first impedance peak; Zmax: the highest"),
        ("Re", "the Re_ohm option, else Re Z extrapolated to 0 Hz from the two lowest frequencies assuming Re Z - Re proportional to f^2"),
        ("Q", "sqrt(r0) method (Small 1972): r0 = Z(fres)/Re, |Z(f1)| = |Z(f2)| = Re*sqrt(r0), Qms = fres*sqrt(r0)/(f2 - f1), Qes = Qms/(r0 - 1), Qts = Qms/r0; crossings refined on exact solves"),
        ("z_1kHz", "|Z| at 1 kHz, exact solve"),
        ("min_above_resonance", "smallest |Z| above the in-situ resonance, refined unless at the sweep's end"),
        ("rated_check", "|Z| below 80 % of the rated impedance anywhere in the sweep (spec Section 4)"),
        ("free_air", "sqrt(r0) readouts of the driver's unloaded impedance (both acoustic ports at ambient) with its DC resistance"),
        ("level", "dB SPL re 20 uPa (RMS) at the response probe under the stated drive, exact solves at 500 Hz and 1 kHz"),
        ("sensitivity", "dB SPL per volt of source EMF and per milliwatt into the rated impedance at 500 Hz and 1 kHz: dB/mW = dB/V - 10*log10(1000/Z_rated) (erratum E32)"),
        ("bass_extension", "frequency 3 dB below the 500 Hz level, walking the grid down from 500 Hz, crossing refined on exact solves"),
        ("coupled_resonance", "maximum of diaphragm velocity per unit coil current |v/i| (minimum of the total mechanical impedance the motor drives), refined by Brent's method; robust when at least 1 dB above |v/i| one octave either side"),
    ])
}

/// What to read, resolved against one compiled point.
pub(crate) struct Plan {
    pressure: Option<(usize, &'static str)>,
    /// Index of the impedance probe among the netlist probes, or of the
    /// synthetic one among the extras.
    impedance: Option<ZSource>,
    driver: Option<DriverTap>,
    pub extras: Vec<Probe>,
    rated: Option<f64>,
    re_ohm: Option<f64>,
    notes: Vec<String>,
}

enum ZSource {
    Netlist(usize),
    Extra(usize, String),
}

struct DriverTap {
    id: String,
    v: usize,
    i: usize,
}

/// Prefix of the synthetic probes the readouts add while solving.
pub const SYNTHETIC_PREFIX: &str = "#readout:";

impl Plan {
    pub(crate) fn new(
        point: &Point,
        opts: &ReadoutOptions,
        ui_probe: Option<&str>,
    ) -> Result<Plan> {
        opts.validate()
            .map_err(|m| super::options_error("readout", m))?;
        let c = &point.circuit;
        let mut notes = Vec::new();
        let mut extras = Vec::new();
        let pressure = if let Some(id) = &opts.probe {
            let i = c
                .probes
                .iter()
                .position(|p| &p.id == id)
                .ok_or_else(|| Error::Probe {
                    id: id.clone(),
                    msg: "no such probe".into(),
                })?;
            if !c.probes[i].is_pressure {
                return Err(Error::Probe {
                    id: id.clone(),
                    msg: "response readouts need an acoustic pressure probe".into(),
                });
            }
            Some((i, "option"))
        } else if let Some(i) =
            ui_probe.and_then(|id| c.probes.iter().position(|p| p.id == id && p.is_pressure))
        {
            Some((i, "ui.primary_probe"))
        } else if let Some(i) = c.probes.iter().position(|p| p.is_pressure) {
            Some((i, "first pressure probe"))
        } else {
            notes.push("no acoustic pressure probe: no response readouts".into());
            None
        };
        let impedance = if let Some(id) = &opts.impedance {
            let i = c
                .probes
                .iter()
                .position(|p| &p.id == id)
                .ok_or_else(|| Error::Probe {
                    id: id.clone(),
                    msg: "no such probe".into(),
                })?;
            if c.probes[i].quantity != "impedance" {
                return Err(Error::Probe {
                    id: id.clone(),
                    msg: "impedance readouts need an impedance probe".into(),
                });
            }
            Some(ZSource::Netlist(i))
        } else if let Some(e) = c
            .elements
            .iter()
            .position(|e| e.as_any().downcast_ref::<VSource>().is_some())
        {
            let label = format!("impedance seen by vsource '{}'", c.elements[e].id());
            extras.push(Probe {
                id: format!("{SYNTHETIC_PREFIX}z"),
                quantity: "impedance".into(),
                unit: "ohm",
                kind: ProbeKind::Impedance {
                    element: e,
                    port: 0,
                },
                is_pressure: false,
            });
            Some(ZSource::Extra(extras.len() - 1, label))
        } else {
            notes.push("no vsource and no impedance probe given: no impedance readouts".into());
            None
        };
        let driver = match driver_tap(point, opts.driver.as_deref())? {
            None => {
                if pressure.is_some() {
                    notes.push("no driver element: no coupled resonance".into());
                }
                None
            }
            Some((id, [vp, ip])) => {
                extras.push(vp);
                extras.push(ip);
                Some(DriverTap {
                    id,
                    v: extras.len() - 2,
                    i: extras.len() - 1,
                })
            }
        };
        let rated = opts.rated_ohm.or(match &c.drive {
            Some(DriveSpec::Power { rated_ohm, .. }) => Some(*rated_ohm),
            _ => None,
        });
        Ok(Plan {
            pressure,
            impedance,
            driver,
            extras,
            rated,
            re_ohm: opts.re_ohm,
            notes,
        })
    }

    /// Computes the readouts from a solve made with [`Plan::extras`].
    pub(crate) fn compute(
        &self,
        point: &mut Point,
        result: &SolveResult,
        extras: &[ProbeResult],
    ) -> Result<Readouts> {
        let impedance = match &self.impedance {
            None => None,
            Some(src) => {
                let (probe, values, label) = match src {
                    ZSource::Netlist(i) => (
                        point.circuit.probes[*i].clone(),
                        &result.probes[*i].values,
                        point.circuit.probes[*i].id.clone(),
                    ),
                    ZSource::Extra(k, label) => {
                        (self.extras[*k].clone(), &extras[*k].values, label.clone())
                    }
                };
                Some(self.impedance_readouts(point, result, &probe, values, label)?)
            }
        };
        let drivers = driver_readouts(point);
        let response = match self.pressure {
            None => None,
            Some((i, source)) => Some(self.response_readouts(point, result, extras, i, source)?),
        };
        Ok(Readouts {
            drive: result.meta.drive.clone(),
            impedance,
            drivers,
            response,
            notes: self.notes.clone(),
            methods: methods(),
            hash: point.hash(),
            engine: crate::solve::ENGINE,
        })
    }

    fn impedance_readouts(
        &self,
        point: &Point,
        result: &SolveResult,
        probe: &Probe,
        z: &[C64],
        label: String,
    ) -> Result<ImpedanceReadouts> {
        let freqs = result.freqs_hz.clone();
        let circuit = &point.circuit;
        let mut eval = |f: f64| -> Result<C64> {
            let x = circuit.solve_at(f)?;
            circuit.probe_value(probe, f, &x)
        };
        let mut notes = Vec::new();
        let (re, re_source) = match self.re_ohm {
            Some(r) => (r, "given (Re_ohm option)".to_string()),
            None => estimate_re(&freqs, z),
        };
        let core = analyse_impedance(&freqs, z, re, &mut eval, &mut notes)?;
        let z_1khz = Some(eval(1000.0)?.norm());
        let rated_check = match self.rated {
            None => {
                notes.push(
                    "no rated impedance (rated_ohm option or drive key): no rated-impedance check"
                        .into(),
                );
                None
            }
            Some(rated) => Some(rated_check(&freqs, z, rated, &mut eval)?),
        };
        Ok(ImpedanceReadouts {
            probe: label,
            re,
            re_source,
            peaks: core.peaks,
            resonance: core.resonance,
            z_max: core.z_max,
            q: core.q,
            z_1khz,
            min_above_resonance: core.min_above.map(|(f, v)| AtFrequency {
                f_hz: f,
                value: v,
                shading: result.shading.band(f),
            }),
            min_at_sweep_end: core.min_at_end,
            rated_check,
            notes,
        })
    }

    fn response_readouts(
        &self,
        point: &mut Point,
        result: &SolveResult,
        extras: &[ProbeResult],
        i: usize,
        probe_source: &'static str,
    ) -> Result<ResponseReadouts> {
        let pr = &result.probes[i];
        let id = pr.id.clone();
        let mut notes = Vec::new();
        let exact = point.solve_at_freqs(&[REFERENCE_HZ, 1000.0])?;
        let level_500 = drive::spl_db(exact.probes[i].values[0]);
        let level_1k = drive::spl_db(exact.probes[i].values[1]);
        let mut sensitivity = Vec::new();
        match drive::source_voltage(&point.circuit) {
            Ok(v) if v != 0.0 => {
                let conv = self.rated.map(drive::conversion_db);
                for f in [REFERENCE_HZ, 1000.0] {
                    let db_per_v = drive::spl_db(drive::probe_at(&point.circuit, &id, f)? / v);
                    sensitivity.push(SensitivityReadout {
                        f_hz: f,
                        db_per_v,
                        db_per_mw: conv.map(|c| db_per_v - c),
                        rated_ohm: self.rated,
                        conversion_db: conv,
                    });
                }
                if self.rated.is_none() {
                    notes.push("no rated impedance: sensitivity in dB/V only (erratum E32: dB/mW needs the rated impedance, not Re)".into());
                }
            }
            _ => notes.push(
                "sensitivity needs exactly one independent source, a vsource with non-zero V_V"
                    .into(),
            ),
        }
        let bass_extension = bass_extension(point, result, i, level_500, &mut notes)?;
        let coupled_resonance = match &self.driver {
            None => None,
            Some(tap) => coupled_resonance(
                point,
                result,
                (&self.extras[tap.v], &extras[tap.v]),
                (&self.extras[tap.i], &extras[tap.i]),
                &tap.id,
                &mut notes,
            )?,
        };
        Ok(ResponseReadouts {
            probe: id,
            probe_source,
            level_500,
            level_1k,
            sensitivity,
            bass_extension,
            coupled_resonance,
            notes,
        })
    }
}

/// Re estimated from the curve: Re Z extrapolated to 0 Hz in f² from the
/// two lowest frequencies, else |Z| at the lowest frequency.
fn estimate_re(freqs: &[f64], z: &[C64]) -> (f64, String) {
    if freqs.len() >= 2 {
        let (f1, f2) = (freqs[0], freqs[1]);
        let (r1, r2) = (z[0].re, z[1].re);
        let re = (r1 * f2 * f2 - r2 * f1 * f1) / (f2 * f2 - f1 * f1);
        if re.is_finite() && re > 0.0 {
            return (
                re,
                format!("Re Z extrapolated to 0 Hz (f^2 law) from {f1:.4} and {f2:.4} Hz"),
            );
        }
    }
    (
        z.first().map_or(f64::NAN, |v| v.norm()),
        format!(
            "|Z| at the lowest frequency, {:.4} Hz",
            freqs.first().copied().unwrap_or(f64::NAN)
        ),
    )
}

pub(crate) struct ImpedanceCore {
    pub peaks: Vec<Peak>,
    pub resonance: Option<Peak>,
    pub z_max: Option<Peak>,
    pub q: Option<QEstimate>,
    pub min_above: Option<(f64, f64)>,
    pub min_at_end: bool,
}

/// Grid indices of interior local maxima of `mag` with their topographic
/// prominence in dB.
pub(crate) fn prominent_maxima(mag: &[f64], min_db: f64) -> Vec<(usize, f64)> {
    let n = mag.len();
    let mut out = Vec::new();
    for i in 1..n.saturating_sub(1) {
        if !(mag[i] > mag[i - 1] && mag[i] >= mag[i + 1]) {
            continue;
        }
        let mut left = mag[i];
        let mut j = i;
        while j > 0 && mag[j - 1] <= mag[i] {
            j -= 1;
            left = left.min(mag[j]);
        }
        let mut right = mag[i];
        let mut k = i;
        while k + 1 < n && mag[k + 1] <= mag[i] {
            k += 1;
            right = right.min(mag[k]);
        }
        let prom = 20.0 * (mag[i] / left.max(right)).log10();
        if prom >= min_db {
            out.push((i, prom));
        }
    }
    out
}

/// Refines a grid maximum of |g| at index i on [f(i−1), f(i+1)].
fn refine_max(
    freqs: &[f64],
    i: usize,
    grid_value: f64,
    g: &mut dyn FnMut(f64) -> Result<f64>,
) -> Result<(f64, f64)> {
    let (x, v) = brent_max(
        &mut |u| g(u.exp()),
        freqs[i - 1].ln(),
        freqs[i + 1].ln(),
        LN_F_TOL,
    )?;
    Ok(if v >= grid_value {
        (x.exp(), v)
    } else {
        (freqs[i], grid_value)
    })
}

/// The frequency between grid points `lo` and `lo + 1` where `g` crosses
/// `level` (g(f[lo]) and g(f[lo+1]) on opposite sides).
fn refine_crossing(
    freqs: &[f64],
    mag: &[f64],
    lo: usize,
    level: f64,
    g: &mut dyn FnMut(f64) -> Result<f64>,
) -> Result<f64> {
    let (a, b) = (freqs[lo].ln(), freqs[lo + 1].ln());
    let u = illinois_root(
        &mut |u| Ok(g(u.exp())? - level),
        a,
        b,
        mag[lo] - level,
        mag[lo + 1] - level,
        LN_F_TOL,
    )?;
    Ok(u.exp())
}

/// Peaks, resonance, Zmax, Q and the minimum above resonance of an
/// impedance curve; `eval` gives Z at any frequency.
pub(crate) fn analyse_impedance(
    freqs: &[f64],
    z: &[C64],
    re: f64,
    eval: &mut dyn FnMut(f64) -> Result<C64>,
    notes: &mut Vec<String>,
) -> Result<ImpedanceCore> {
    let mag: Vec<f64> = z.iter().map(|v| v.norm()).collect();
    let mut g = |f: f64| -> Result<f64> { Ok(eval(f)?.norm()) };
    let mut peaks = Vec::new();
    let mut first_index = None;
    for (i, prom) in prominent_maxima(&mag, PEAK_PROMINENCE_DB) {
        let (f, v) = refine_max(freqs, i, mag[i], &mut g)?;
        if first_index.is_none() {
            first_index = Some(i);
        }
        peaks.push(Peak {
            f_hz: f,
            z: v,
            prominence_db: prom,
        });
    }
    let resonance = peaks.first().copied();
    let z_max = peaks.iter().copied().max_by(|a, b| a.z.total_cmp(&b.z));
    if peaks.len() > 1 {
        notes.push(format!(
            "{} impedance peaks: the resonance is the first; the sqrt(r0) Q estimates assume an isolated resonance",
            peaks.len()
        ));
    }
    let (mut q, mut min_above, mut min_at_end) = (None, None, false);
    if let (Some(res), Some(i)) = (resonance, first_index) {
        q = q_estimate(freqs, &mag, i, res, re, &mut g, notes)?;
        let n = freqs.len();
        if i + 1 < n {
            let k = (i + 1..n)
                .min_by(|&a, &b| mag[a].total_cmp(&mag[b]))
                .expect("non-empty");
            if k == n - 1 {
                min_at_end = true;
                min_above = Some((freqs[k], mag[k]));
            } else {
                let mut neg = |f: f64| Ok(-g(f)?);
                let (f, v) = refine_max(freqs, k, -mag[k], &mut neg)?;
                min_above = Some((f, -v));
            }
        }
    } else {
        notes.push("no impedance peak with at least 0.1 dB prominence in the sweep".into());
    }
    Ok(ImpedanceCore {
        peaks,
        resonance,
        z_max,
        q,
        min_above,
        min_at_end,
    })
}

fn q_estimate(
    freqs: &[f64],
    mag: &[f64],
    i: usize,
    res: Peak,
    re: f64,
    g: &mut dyn FnMut(f64) -> Result<f64>,
    notes: &mut Vec<String>,
) -> Result<Option<QEstimate>> {
    let r0 = res.z / re;
    if r0.is_nan() || r0 <= 1.0 {
        notes.push(format!(
            "the resonance peak {:.4} ohm does not exceed Re {re:.4} ohm: no Q estimates",
            res.z
        ));
        return Ok(None);
    }
    let level = re * r0.sqrt();
    let mut j = i;
    while j > 0 && mag[j] > level {
        j -= 1;
    }
    let mut k = i;
    while k + 1 < mag.len() && mag[k] > level {
        k += 1;
    }
    if mag[j] > level || mag[k] > level {
        notes.push("|Z| does not fall to Re*sqrt(r0) on both sides of the resonance within the sweep: no Q estimates".into());
        return Ok(None);
    }
    let f1 = refine_crossing(freqs, mag, j, level, g)?;
    let f2 = refine_crossing(freqs, mag, k - 1, level, g)?;
    let qms = res.f_hz * r0.sqrt() / (f2 - f1);
    let mut q_notes = Vec::new();
    let fg = (f1 * f2).sqrt();
    if (fg / res.f_hz - 1.0).abs() > 0.01 {
        q_notes.push(format!(
            "sqrt(f1*f2) = {fg:.4} Hz differs from the peak {:.4} Hz by more than 1 %: not a single lumped resonance, so the Q values are indicative",
            res.f_hz
        ));
    }
    Ok(Some(QEstimate {
        re,
        fres: res.f_hz,
        r0,
        f1,
        f2,
        qms,
        qes: qms / (r0 - 1.0),
        qts: qms / r0,
        notes: q_notes,
    }))
}

fn rated_check(
    freqs: &[f64],
    z: &[C64],
    rated: f64,
    eval: &mut dyn FnMut(f64) -> Result<C64>,
) -> Result<RatedCheck> {
    let limit = RATED_IMPEDANCE_FRACTION * rated;
    let mag: Vec<f64> = z.iter().map(|v| v.norm()).collect();
    let mut violations: Vec<Violation> = Vec::new();
    let mut open: Option<usize> = None;
    for k in 0..=mag.len() {
        let below = k < mag.len() && mag[k] < limit;
        match (below, open) {
            (true, None) => open = Some(k),
            (false, Some(s)) => {
                violations.push(Violation {
                    f_min: freqs[s],
                    f_max: freqs[k - 1],
                });
                open = None;
            }
            _ => {}
        }
    }
    let k = (0..mag.len())
        .min_by(|&a, &b| mag[a].total_cmp(&mag[b]))
        .expect("non-empty sweep");
    let (mut fmin, mut zmin) = (freqs[k], mag[k]);
    if k > 0 && k + 1 < mag.len() {
        let mut neg = |f: f64| Ok(-eval(f)?.norm());
        let (f, v) = refine_max(freqs, k, -mag[k], &mut neg)?;
        fmin = f;
        zmin = -v;
    }
    Ok(RatedCheck {
        rated_ohm: rated,
        limit_ohm: limit,
        z_min_ohm: zmin,
        z_min_hz: fmin,
        pass: violations.is_empty() && zmin >= limit,
        violations,
    })
}

fn driver_readouts(point: &Point) -> Vec<DriverReadouts> {
    let freqs = &point.circuit.freqs;
    point
        .circuit
        .elements
        .iter()
        .filter_map(|e| e.as_any().downcast_ref::<Driver>())
        .map(|d| {
            let p = d.params;
            let d0 = DriverD0 {
                fs: p.fs(),
                qms: p.qms(),
                qes: p.qes(),
                qts: p.qts(),
                re: p.re,
                bl: p.bl,
                mms: p.mms,
                cms: p.cms,
                rms: p.rms,
                sd: p.sd,
            };
            let z: Vec<C64> = freqs
                .iter()
                .map(|f| d.unloaded_impedance(2.0 * PI * f))
                .collect();
            let mut notes = Vec::new();
            let mut eval = |f: f64| Ok(d.unloaded_impedance(2.0 * PI * f));
            let free_air = analyse_impedance(freqs, &z, p.re, &mut eval, &mut notes)
                .ok()
                .and_then(|c| c.q);
            DriverReadouts {
                element: d.id.clone(),
                model: format!("{:?}", d.level),
                d0,
                free_air,
                notes,
            }
        })
        .collect()
}

fn bass_extension(
    point: &mut Point,
    result: &SolveResult,
    i: usize,
    level_500: f64,
    notes: &mut Vec<String>,
) -> Result<Option<AtFrequency>> {
    let freqs = &result.freqs_hz;
    let db = result.probes[i].spl_db();
    let target = level_500 - BASS_EXTENSION_DB;
    let below: Vec<usize> = (0..freqs.len())
        .filter(|&k| freqs[k] < REFERENCE_HZ)
        .collect();
    let Some(&top) = below.last() else {
        notes.push("the sweep has no frequency below 500 Hz: no bass extension".into());
        return Ok(None);
    };
    let mut k = top;
    loop {
        if db[k] < target {
            break;
        }
        if k == 0 {
            notes.push(format!(
                "the level stays within 3 dB of its 500 Hz level down to {:.4} Hz: no bass extension in the sweep",
                freqs[0]
            ));
            return Ok(None);
        }
        k -= 1;
    }
    // Bracket: freqs[k] below the target; above it the next grid point, or
    // 500 Hz itself when k is the last grid point below 500 Hz.
    let (f_hi, db_hi) = if k < top {
        (freqs[k + 1], db[k + 1])
    } else {
        (REFERENCE_HZ, level_500)
    };
    let mut g = |f: f64| -> Result<f64> {
        let r = point.solve_at_freqs(&[f])?;
        Ok(drive::spl_db(r.probes[i].values[0]))
    };
    let u = illinois_root(
        &mut |u| Ok(g(u.exp())? - target),
        freqs[k].ln(),
        f_hi.ln(),
        db[k] - target,
        db_hi - target,
        LN_F_TOL,
    )?;
    let f = u.exp();
    Ok(Some(AtFrequency {
        f_hz: f,
        value: target,
        shading: result.shading.band(f),
    }))
}

/// The driver element for the coupled resonance (`driver`, else the first
/// driver) and two synthetic probes: its dome velocity `<id>.m` and its
/// coil current. `None` when the netlist has no driver.
pub(crate) fn driver_tap(
    point: &Point,
    driver: Option<&str>,
) -> Result<Option<(String, [Probe; 2])>> {
    let c = &point.circuit;
    let index = match driver {
        Some(id) => {
            let e = c.element_index(id).ok_or_else(|| {
                Error::element(id.to_string(), "no such element (option 'driver')")
            })?;
            if c.elements[e].as_any().downcast_ref::<Driver>().is_none() {
                return Err(Error::element(
                    id.to_string(),
                    "the option 'driver' must name a driver element",
                ));
            }
            Some(e)
        }
        None => c
            .elements
            .iter()
            .position(|e| e.as_any().downcast_ref::<Driver>().is_some()),
    };
    let Some(e) = index else {
        return Ok(None);
    };
    let id = c.elements[e].id().to_string();
    let (node, domain) = c
        .nodes
        .lookup(&format!("{id}.m"))
        .ok_or_else(|| Error::element(id.clone(), "driver has no '.m' node"))?;
    debug_assert_eq!(domain, Domain::Mechanical);
    let v = Probe {
        id: format!("{SYNTHETIC_PREFIX}v"),
        quantity: "velocity".into(),
        unit: "m/s",
        kind: ProbeKind::Node(node),
        is_pressure: false,
    };
    let i = Probe {
        id: format!("{SYNTHETIC_PREFIX}i"),
        quantity: "current".into(),
        unit: "A",
        kind: ProbeKind::Flow {
            element: e,
            port: 0,
        },
        is_pressure: false,
    };
    Ok(Some((id, [v, i])))
}

/// Coupled resonance from the |v/i| curve of a solve (`v`, `i`: the
/// results of the probes `vp`, `ip`).
pub(crate) fn coupled_resonance(
    point: &Point,
    result: &SolveResult,
    (vp, v): (&Probe, &ProbeResult),
    (ip, i): (&Probe, &ProbeResult),
    driver: &str,
    notes: &mut Vec<String>,
) -> Result<Option<CoupledResonance>> {
    let freqs = &result.freqs_hz;
    let ratio: Vec<f64> = v
        .values
        .iter()
        .zip(&i.values)
        .map(|(v, i)| (v / i).norm())
        .collect();
    let Some(k) = prominent_maxima(&ratio, 0.0)
        .into_iter()
        .map(|(k, _)| k)
        .max_by(|&a, &b| ratio[a].total_cmp(&ratio[b]))
    else {
        notes.push(format!(
            "|v/i| of driver '{driver}' has no interior maximum in the sweep: no coupled resonance"
        ));
        return Ok(None);
    };
    let c = &point.circuit;
    let mut g = |f: f64| -> Result<f64> {
        let x = c.solve_at(f)?;
        Ok((c.probe_value(vp, f, &x)? / c.probe_value(ip, f, &x)?).norm())
    };
    let (f, peak) = refine_max(freqs, k, ratio[k], &mut g)?;
    let side = g(f / 2.0)?.max(g(2.0 * f)?);
    let prominence_db = 20.0 * (peak / side).log10();
    Ok(Some(CoupledResonance {
        f_hz: f,
        driver: driver.to_string(),
        prominence_db,
        robust: prominence_db >= RESONANCE_PROMINENCE_DB,
        shading: result.shading.band(f),
    }))
}

/// Readouts of the design at its base overrides.
pub fn readouts(design: &Design, opts: &ReadoutOptions) -> Result<Readouts> {
    let mut point = design.base_point()?;
    readouts_at(&mut point, opts, design.ui_primary_probe().as_deref())
}

/// Readouts of one point (solves it).
pub fn readouts_at(
    point: &mut Point,
    opts: &ReadoutOptions,
    ui_probe: Option<&str>,
) -> Result<Readouts> {
    let plan = Plan::new(point, opts, ui_probe)?;
    let (result, extras) = point.solve_with(&plan.extras)?;
    plan.compute(point, &result, &extras)
}
