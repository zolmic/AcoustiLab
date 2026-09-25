//! The [`Curve`] type: a frequency response or impedance on positive,
//! strictly increasing frequencies, as linear magnitude (SI units, RMS)
//! with an optional phase, plus its [`Sidecar`].
//!
//! JSON form (`acoustilab-curve/0.1`, unit-suffixed keys, unknown keys
//! rejected):
//!
//! ```json
//! {"schema": "acoustilab-curve/0.1", "quantity": "pressure",
//!  "frequencies_Hz": [20, 25], "level_dB": [96.1, 96.3], "phase_deg": [170.2, 168.9],
//!  "sidecar": {"schema": "acoustilab-curve-sidecar/0.1", "fixture": "..."}}
//! ```
//!
//! The magnitude key names its unit: `magnitude_ohm` (impedance),
//! `level_dB` (pressure, dB SPL re 20 µPa; `magnitude_Pa` is also read),
//! `magnitude_m` (displacement), `magnitude_m_per_s` (velocity),
//! `magnitude` or `level_dB` (generic, dB re 1). `re` and `im` arrays may
//! replace magnitude and phase.
//!
//! Resampling interpolates the level in dB and the unwrapped phase linearly
//! in ln f ([`Curve::resample`]). The exchange grid is log-spaced at 48
//! points per octave (spec Section 14) and anchored at 1 kHz, so curves
//! from different sources share their points ([`exchange_grid`]).

use super::sidecar::Sidecar;
use super::CurveError;
use crate::air::P_REF;
use crate::C64;
use serde_json::{json, Map, Value};

/// Schema tag of curve documents.
pub const CURVE_SCHEMA: &str = "acoustilab-curve/0.1";

/// Density of the exchange grid, points per octave (spec Section 14).
pub const EXCHANGE_POINTS_PER_OCTAVE: f64 = 48.0;

/// What a curve measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quantity {
    /// Electrical input impedance, ohm.
    Impedance,
    /// Sound pressure, Pa RMS; levels in dB SPL re 20 µPa.
    Pressure,
    /// Diaphragm displacement, m RMS (e.g. a laser transfer function).
    Displacement,
    /// Diaphragm velocity, m/s RMS.
    Velocity,
    /// Anything else, in the unit the sidecar states.
    Generic,
}

impl Quantity {
    pub const ALL: [Quantity; 5] = [
        Quantity::Impedance,
        Quantity::Pressure,
        Quantity::Displacement,
        Quantity::Velocity,
        Quantity::Generic,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Impedance => "impedance",
            Quantity::Pressure => "pressure",
            Quantity::Displacement => "displacement",
            Quantity::Velocity => "velocity",
            Quantity::Generic => "generic",
        }
    }

    pub fn parse(s: &str) -> Option<Quantity> {
        Quantity::ALL.into_iter().find(|q| q.name() == s)
    }

    /// SI unit of the magnitude.
    pub fn unit(self) -> &'static str {
        match self {
            Quantity::Impedance => "ohm",
            Quantity::Pressure => "Pa",
            Quantity::Displacement => "m",
            Quantity::Velocity => "m/s",
            Quantity::Generic => "",
        }
    }

    /// Reference of the dB scale: 20 µPa for pressure, one unit otherwise.
    pub fn db_reference(self) -> f64 {
        match self {
            Quantity::Pressure => P_REF,
            _ => 1.0,
        }
    }

    /// Key of the linear magnitude in curve documents.
    pub fn magnitude_key(self) -> &'static str {
        match self {
            Quantity::Impedance => "magnitude_ohm",
            Quantity::Pressure => "magnitude_Pa",
            Quantity::Displacement => "magnitude_m",
            Quantity::Velocity => "magnitude_m_per_s",
            Quantity::Generic => "magnitude",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub quantity: Quantity,
    /// Positive, strictly increasing, Hz.
    pub freqs_hz: Vec<f64>,
    /// Linear RMS magnitude in the quantity's SI unit, positive.
    pub magnitude: Vec<f64>,
    /// Phase in degrees, as given (wrapped or not), if known.
    pub phase_deg: Option<Vec<f64>>,
    pub sidecar: Sidecar,
    /// Comment and header lines of the file the curve was read from.
    pub comments: Vec<String>,
}

/// One point with where it came from (a 1-based line, or an index when
/// `lines` is false), before sorting.
pub(crate) struct RawPoint {
    pub f: f64,
    pub mag: f64,
    pub phase: Option<f64>,
    pub origin: usize,
}

fn same(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
}

/// Sorted frequencies, magnitudes and optional phases.
pub(crate) type Ordered = (Vec<f64>, Vec<f64>, Option<Vec<f64>>);

/// Validates, sorts and de-duplicates points. Exact repeats of a frequency
/// with the same values are dropped; a repeat with different values is an
/// error naming both origins.
pub(crate) fn order_points(mut pts: Vec<RawPoint>, lines: bool) -> Result<Ordered, CurveError> {
    let err = |origin: usize, msg: String| {
        if lines {
            CurveError::at(origin, msg)
        } else {
            CurveError::new(format!("point {origin}: {msg}"))
        }
    };
    for p in &pts {
        if !(p.f.is_finite() && p.f > 0.0) {
            return Err(err(p.origin, format!("frequency {} is not positive", p.f)));
        }
        if !(p.mag.is_finite() && p.mag > 0.0) {
            return Err(err(
                p.origin,
                format!("magnitude {} is not positive and finite", p.mag),
            ));
        }
        if let Some(ph) = p.phase {
            if !ph.is_finite() {
                return Err(err(p.origin, "phase is not finite".into()));
            }
        }
    }
    let with_phase = pts.iter().filter(|p| p.phase.is_some()).count();
    if with_phase != 0 && with_phase != pts.len() {
        return Err(CurveError::new(
            "some points have a phase and others do not",
        ));
    }
    pts.sort_by(|a, b| a.f.total_cmp(&b.f));
    let mut kept: Vec<RawPoint> = Vec::with_capacity(pts.len());
    for p in pts {
        if let Some(q) = kept.last() {
            if q.f == p.f {
                let equal = same(q.mag, p.mag)
                    && match (q.phase, p.phase) {
                        (Some(a), Some(b)) => same(a, b),
                        _ => true,
                    };
                if equal {
                    continue;
                }
                let (a, b) = (q.origin.min(p.origin), q.origin.max(p.origin));
                let what = if lines { "lines" } else { "points" };
                return Err(err(
                    b,
                    format!(
                        "frequency {} Hz appears twice with different values ({what} {a} and {b})",
                        p.f
                    ),
                ));
            }
        }
        kept.push(p);
    }
    if kept.len() < 2 {
        return Err(CurveError::new(format!(
            "a curve needs at least 2 distinct frequencies, got {}",
            kept.len()
        )));
    }
    let freqs = kept.iter().map(|p| p.f).collect();
    let mags = kept.iter().map(|p| p.mag).collect();
    let phase = (with_phase > 0).then(|| kept.iter().map(|p| p.phase.unwrap()).collect());
    Ok((freqs, mags, phase))
}

impl Curve {
    /// A curve from magnitudes (linear, SI) and optional phases in degrees,
    /// in any frequency order (see [`order_points`] for duplicates).
    pub fn new(
        quantity: Quantity,
        freqs: &[f64],
        magnitude: &[f64],
        phase_deg: Option<&[f64]>,
    ) -> Result<Curve, CurveError> {
        if freqs.len() != magnitude.len() || phase_deg.is_some_and(|p| p.len() != freqs.len()) {
            return Err(CurveError::new(
                "frequency, magnitude and phase arrays differ in length",
            ));
        }
        let pts = (0..freqs.len())
            .map(|i| RawPoint {
                f: freqs[i],
                mag: magnitude[i],
                phase: phase_deg.map(|p| p[i]),
                origin: i,
            })
            .collect();
        let (freqs_hz, magnitude, phase_deg) = order_points(pts, false)?;
        Ok(Curve {
            quantity,
            freqs_hz,
            magnitude,
            phase_deg,
            sidecar: Sidecar::for_quantity(quantity),
            comments: Vec::new(),
        })
    }

    /// A curve from complex values (RMS phasors).
    pub fn from_complex(
        quantity: Quantity,
        freqs: &[f64],
        values: &[C64],
    ) -> Result<Curve, CurveError> {
        let mag: Vec<f64> = values.iter().map(|v| v.norm()).collect();
        let ph: Vec<f64> = values.iter().map(|v| v.arg().to_degrees()).collect();
        Curve::new(quantity, freqs, &mag, Some(&ph))
    }

    /// A curve from levels in dB re the quantity's reference.
    pub fn from_db(
        quantity: Quantity,
        freqs: &[f64],
        level_db: &[f64],
        phase_deg: Option<&[f64]>,
    ) -> Result<Curve, CurveError> {
        let r = quantity.db_reference();
        let mag: Vec<f64> = level_db.iter().map(|l| r * 10f64.powf(l / 20.0)).collect();
        Curve::new(quantity, freqs, &mag, phase_deg)
    }

    pub fn len(&self) -> usize {
        self.freqs_hz.len()
    }

    pub fn is_empty(&self) -> bool {
        self.freqs_hz.is_empty()
    }

    /// Levels in dB re the quantity's reference (dB SPL for pressure).
    pub fn level_db(&self) -> Vec<f64> {
        let r = self.quantity.db_reference();
        self.magnitude
            .iter()
            .map(|m| 20.0 * (m / r).log10())
            .collect()
    }

    /// Complex values, if the curve has a phase.
    pub fn complex(&self) -> Option<Vec<C64>> {
        let ph = self.phase_deg.as_ref()?;
        Some(
            self.magnitude
                .iter()
                .zip(ph)
                .map(|(m, p)| C64::from_polar(*m, p.to_radians()))
                .collect(),
        )
    }

    /// The curve on other frequencies: level in dB and unwrapped phase,
    /// each linear in ln f between the bracketing points (exact at the
    /// curve's own frequencies). Frequencies outside the curve's range are
    /// an error; the output phase is wrapped to (−180°, 180°].
    pub fn resample(&self, freqs: &[f64]) -> Result<Curve, CurveError> {
        let (lo, hi) = (self.freqs_hz[0], self.freqs_hz[self.len() - 1]);
        let lnf: Vec<f64> = self.freqs_hz.iter().map(|f| f.ln()).collect();
        let level = self.level_db();
        let phase = self.phase_deg.as_ref().map(|p| unwrap_deg(p));
        let mut out_l = Vec::with_capacity(freqs.len());
        let mut out_p = Vec::with_capacity(freqs.len());
        for &f in freqs {
            if !(f.is_finite() && f >= lo * (1.0 - 1e-12) && f <= hi * (1.0 + 1e-12)) {
                return Err(CurveError::new(format!(
                    "cannot resample to {f} Hz: the curve covers {lo} to {hi} Hz"
                )));
            }
            let x = f.ln().clamp(lnf[0], lnf[lnf.len() - 1]);
            let i = match lnf.partition_point(|&v| v <= x) {
                0 => 0,
                k if k >= lnf.len() => lnf.len() - 2,
                k => k - 1,
            };
            let t = (x - lnf[i]) / (lnf[i + 1] - lnf[i]);
            out_l.push(level[i] + t * (level[i + 1] - level[i]));
            if let Some(p) = &phase {
                out_p.push(wrap_deg(p[i] + t * (p[i + 1] - p[i])));
            }
        }
        let mut c = Curve::from_db(
            self.quantity,
            freqs,
            &out_l,
            phase.as_ref().map(|_| out_p.as_slice()),
        )?;
        c.sidecar = self.sidecar.clone();
        c.comments = self.comments.clone();
        Ok(c)
    }

    /// The curve on the exchange grid ([`exchange_grid`]) within its own
    /// frequency range.
    pub fn resample_exchange(&self, points_per_octave: f64) -> Result<Curve, CurveError> {
        let g = exchange_grid(
            self.freqs_hz[0],
            self.freqs_hz[self.len() - 1],
            points_per_octave,
        );
        if g.len() < 2 {
            return Err(CurveError::new(
                "the curve spans fewer than two exchange-grid points",
            ));
        }
        self.resample(&g)
    }

    /// Indices of the points within [f_min, f_max].
    pub fn indices_within(&self, f_min: f64, f_max: f64) -> Vec<usize> {
        (0..self.len())
            .filter(|&i| self.freqs_hz[i] >= f_min && self.freqs_hz[i] <= f_max)
            .collect()
    }

    /// The JSON form (module documentation).
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("schema".into(), json!(CURVE_SCHEMA));
        o.insert("quantity".into(), json!(self.quantity.name()));
        o.insert("frequencies_Hz".into(), json!(self.freqs_hz));
        match self.quantity {
            Quantity::Pressure => o.insert("level_dB".into(), json!(self.level_db())),
            q => o.insert(q.magnitude_key().into(), json!(self.magnitude)),
        };
        if let Some(p) = &self.phase_deg {
            o.insert("phase_deg".into(), json!(p));
        }
        let mut sc = self.sidecar.clone();
        sc.quantity = Some(self.quantity);
        o.insert("sidecar".into(), sc.to_json());
        if !self.comments.is_empty() {
            o.insert("comments".into(), json!(self.comments));
        }
        Value::Object(o)
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Value) -> Result<Curve, CurveError> {
        let o = v
            .as_object()
            .ok_or_else(|| CurveError::new("a curve must be a JSON object"))?;
        let quantity = match o.get("quantity") {
            Some(Value::String(s)) => Quantity::parse(s).ok_or_else(|| {
                CurveError::new(format!(
                    "unknown quantity '{s}' (impedance, pressure, displacement, velocity, generic)"
                ))
            })?,
            _ => return Err(CurveError::new("a curve needs 'quantity'")),
        };
        let mut allowed = vec![
            "schema",
            "quantity",
            "frequencies_Hz",
            "phase_deg",
            "sidecar",
            "comments",
            "re",
            "im",
            quantity.magnitude_key(),
        ];
        if matches!(quantity, Quantity::Pressure | Quantity::Generic) {
            allowed.push("level_dB");
        }
        if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(CurveError::new(format!(
                "unknown key '{k}' in a {} curve (allowed: {})",
                quantity.name(),
                allowed.join(", ")
            )));
        }
        if let Some(s) = o.get("schema") {
            if s.as_str() != Some(CURVE_SCHEMA) {
                return Err(CurveError::new(format!(
                    "schema must be '{CURVE_SCHEMA}', got {s}"
                )));
            }
        }
        let arr = |key: &str| -> Result<Option<Vec<f64>>, CurveError> {
            match o.get(key) {
                None => Ok(None),
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|x| {
                        x.as_f64()
                            .ok_or_else(|| CurveError::new(format!("'{key}' must hold numbers")))
                    })
                    .collect::<Result<Vec<f64>, _>>()
                    .map(Some),
                Some(_) => Err(CurveError::new(format!("'{key}' must be an array"))),
            }
        };
        let freqs = arr("frequencies_Hz")?
            .ok_or_else(|| CurveError::new("a curve needs 'frequencies_Hz'"))?;
        let mag = arr(quantity.magnitude_key())?;
        let level = arr("level_dB")?;
        let phase = arr("phase_deg")?;
        let (re, im) = (arr("re")?, arr("im")?);
        let given = [mag.is_some(), level.is_some(), re.is_some() || im.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if given != 1 {
            return Err(CurveError::new(format!(
                "give exactly one of '{}'{} or 're' + 'im'",
                quantity.magnitude_key(),
                if allowed.contains(&"level_dB") {
                    ", 'level_dB'"
                } else {
                    ""
                }
            )));
        }
        let mut c = if let Some(m) = mag {
            Curve::new(quantity, &freqs, &m, phase.as_deref())?
        } else if let Some(l) = level {
            Curve::from_db(quantity, &freqs, &l, phase.as_deref())?
        } else {
            let (Some(re), Some(im)) = (re, im) else {
                return Err(CurveError::new("'re' and 'im' go together"));
            };
            if phase.is_some() {
                return Err(CurveError::new(
                    "'phase_deg' does not go with 're' and 'im'",
                ));
            }
            if re.len() != im.len() {
                return Err(CurveError::new("'re' and 'im' differ in length"));
            }
            let z: Vec<C64> = re.iter().zip(&im).map(|(a, b)| C64::new(*a, *b)).collect();
            Curve::from_complex(quantity, &freqs, &z)?
        };
        if let Some(s) = o.get("sidecar") {
            c.sidecar = Sidecar::from_json(s)?;
            if let Some(q) = c.sidecar.quantity {
                if q != quantity {
                    return Err(CurveError::new(format!(
                        "the sidecar says '{}' but the curve is '{}'",
                        q.name(),
                        quantity.name()
                    )));
                }
            }
            c.sidecar.quantity = Some(quantity);
        }
        if let Some(cm) = o.get("comments") {
            c.comments = cm
                .as_array()
                .and_then(|a| a.iter().map(|x| x.as_str().map(String::from)).collect())
                .ok_or_else(|| CurveError::new("'comments' must be an array of strings"))?;
        }
        Ok(c)
    }
}

/// Wraps an angle in degrees to (−180, 180].
pub fn wrap_deg(x: f64) -> f64 {
    let y = x - 360.0 * (x / 360.0).round();
    if y <= -180.0 {
        y + 360.0
    } else if y > 180.0 {
        y - 360.0
    } else {
        y
    }
}

/// Removes 360° jumps: each step between neighbours is taken as the one
/// within ±180°. A true phase change of more than 180° between two points
/// cannot be told apart from a wrap.
pub fn unwrap_deg(p: &[f64]) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::with_capacity(p.len());
    let mut offset = 0.0_f64;
    for (i, &x) in p.iter().enumerate() {
        if i > 0 {
            let d = x + offset - out[i - 1];
            offset -= 360.0 * (d / 360.0).round();
        }
        out.push(x + offset);
    }
    out
}

/// Exchange grid: the frequencies 1 kHz·2^(k/n) within [f_lo, f_hi] for
/// integer k, with n points per octave. Empty for invalid arguments or a
/// grid of more than [`crate::grid::MAX_POINTS`] points.
pub fn exchange_grid(f_lo: f64, f_hi: f64, points_per_octave: f64) -> Vec<f64> {
    let n = points_per_octave;
    if !(f_lo > 0.0 && f_hi >= f_lo && f_hi.is_finite() && n > 0.0 && n.is_finite()) {
        return Vec::new();
    }
    let k0 = ((f_lo / 1000.0).log2() * n - 1e-9).ceil();
    let k1 = ((f_hi / 1000.0).log2() * n + 1e-9).floor();
    if k1 - k0 >= crate::grid::MAX_POINTS as f64 {
        return Vec::new();
    }
    (k0 as i64..=k1 as i64)
        .map(|k| 1000.0 * 2f64.powf(k as f64 / n))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_and_unwrap() {
        assert_eq!(wrap_deg(180.0), 180.0);
        assert_eq!(wrap_deg(-180.0), 180.0);
        assert!((wrap_deg(370.0) - 10.0).abs() < 1e-12);
        assert!((wrap_deg(-190.0) - 170.0).abs() < 1e-12);
        let wrapped: Vec<f64> = (0..50).map(|i| wrap_deg(-40.0 * i as f64)).collect();
        let un = unwrap_deg(&wrapped);
        for (i, x) in un.iter().enumerate() {
            assert!((x + 40.0 * i as f64).abs() < 1e-9, "{i}: {x}");
        }
    }

    #[test]
    fn exchange_grid_is_anchored_at_1_khz() {
        let g = exchange_grid(10.0, 20_000.0, 48.0);
        assert!(g.contains(&1000.0));
        assert!(g[0] >= 10.0 && *g.last().unwrap() <= 20_000.0);
        assert!((g[1] / g[0]).log2() * 48.0 - 1.0 < 1e-12);
        // 10 Hz to 20 kHz is 10.97 octaves: 526 or 527 points.
        assert!(g.len() == 527 || g.len() == 526, "{}", g.len());
        assert_eq!(exchange_grid(1000.0, 2000.0, 48.0).len(), 49);
        assert!(exchange_grid(10.0, 20.0, 0.0).is_empty());
        assert!(exchange_grid(10.0, 20_000.0, 1e9).is_empty());
        assert!(exchange_grid(-1.0, 20.0, 48.0).is_empty());
    }
}
