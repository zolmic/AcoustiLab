//! JSON requests and reports of the audition filter, shared by the
//! WebAssembly export and the command-line runner (`docs/auralization.md`,
//! "Interfaces"). Unknown keys are rejected everywhere.
//!
//! * A design: `{"netlist": "<text>", "overrides": {..}, "probe": "p_drp"}`
//!   (`overrides` and `probe` optional).
//! * A baseline: `{"kind": "netlist", ...design}`,
//!   `{"kind": "target", "target": "<bundled name>" | {target object}}`,
//!   `{"kind": "curve", "curve": {acoustilab-curve document}}` or
//!   `{"kind": "none"}` (absolute mode only).
//! * Options: `mode` (`difference` | `absolute`), `phase` (`auto` |
//!   `minimum` | `mixed` | `linear`), `fs_Hz`, `n` (a power of two, or
//!   `"auto"`), `band_Hz` [lo, hi] (default 20 Hz to 20 kHz or 0.9 of
//!   Nyquist, whichever is lower), `threshold_ms`, `pre_samples`,
//!   `ir_length_check`, `verify`, `taps`, and `inversion` {`band_Hz`,
//!   `smoothing`, `boost_cap_dB`, `notch_limit_dB`}.

use super::inversion::InversionOptions;
use super::{sha256, Audition, MagnitudeBaseline, Mode, Options, PhaseRequest};
use crate::expr::PValue;
use crate::io::curve::{Curve as IoCurve, Quantity};
use crate::params::Overrides;
use crate::targets::curve::Curve;
use crate::targets::target;
use serde_json::{json, Map, Value};

/// A design as the request names it.
#[derive(Debug, Clone)]
pub struct DesignSpec {
    pub netlist: String,
    pub overrides: Overrides,
    pub overrides_json: Value,
    pub probe: Option<String>,
}

/// A baseline as the request names it.
#[derive(Debug, Clone)]
pub enum BaselineSpec {
    Netlist(DesignSpec),
    Magnitude(MagnitudeBaseline),
    None,
}

/// An error in the request (kind `options` or `baseline` at the wasm
/// boundary).
#[derive(Debug, Clone, PartialEq)]
pub struct RequestError {
    pub kind: &'static str,
    pub message: String,
}

fn options_err(m: impl Into<String>) -> RequestError {
    RequestError {
        kind: "options",
        message: m.into(),
    }
}

fn baseline_err(m: impl Into<String>) -> RequestError {
    RequestError {
        kind: "baseline",
        message: m.into(),
    }
}

fn object<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, RequestError> {
    v.as_object()
        .ok_or_else(|| options_err(format!("{what} must be an object")))
}

fn only(o: &Map<String, Value>, keys: &[&str], what: &str) -> Result<(), RequestError> {
    match o.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(k) => Err(options_err(format!(
            "{what}: unknown key '{k}' (expected {})",
            keys.join(", ")
        ))),
        None => Ok(()),
    }
}

/// Reads a design object (`kind` allowed when `with_kind`).
pub fn parse_design(v: &Value, what: &str, with_kind: bool) -> Result<DesignSpec, RequestError> {
    let o = object(v, what)?;
    let keys: &[&str] = if with_kind {
        &["kind", "netlist", "overrides", "probe", "label"]
    } else {
        &["netlist", "overrides", "probe", "label"]
    };
    only(o, keys, what)?;
    let netlist = o
        .get("netlist")
        .and_then(Value::as_str)
        .ok_or_else(|| options_err(format!("{what}: 'netlist' (the netlist text) is required")))?
        .to_string();
    let overrides_json = o.get("overrides").cloned().unwrap_or_else(|| json!({}));
    let ov = object(&overrides_json, &format!("{what} overrides"))?;
    let overrides = ov
        .iter()
        .map(|(k, x)| {
            PValue::from_json(x).map(|p| (k.clone(), p)).ok_or_else(|| {
                options_err(format!(
                    "{what}: override '{k}' must be a number, boolean or string"
                ))
            })
        })
        .collect::<Result<Overrides, _>>()?;
    let probe = match o.get("probe") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(options_err(format!("{what}: 'probe' must be a string"))),
    };
    Ok(DesignSpec {
        netlist,
        overrides,
        overrides_json,
        probe,
    })
}

/// Reads a baseline object.
pub fn parse_baseline(v: &Value) -> Result<BaselineSpec, RequestError> {
    let o = object(v, "baseline")?;
    let kind = o
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| options_err("baseline: 'kind' must be netlist, target, curve or none"))?;
    match kind {
        "netlist" => Ok(BaselineSpec::Netlist(parse_design(v, "baseline", true)?)),
        "target" => {
            only(o, &["kind", "target"], "baseline")?;
            let spec = o.get("target").ok_or_else(|| {
                options_err("baseline: 'target' (a name or a target object) is required")
            })?;
            Ok(BaselineSpec::Magnitude(target_baseline(spec)?))
        }
        "curve" => {
            only(o, &["kind", "curve", "label"], "baseline")?;
            let c = o
                .get("curve")
                .ok_or_else(|| options_err("baseline: 'curve' (a curve document) is required"))?;
            let label = o.get("label").and_then(Value::as_str).map(str::to_string);
            Ok(BaselineSpec::Magnitude(curve_baseline(c, label)?))
        }
        "none" => {
            only(o, &["kind"], "baseline")?;
            Ok(BaselineSpec::None)
        }
        k => Err(options_err(format!(
            "baseline: unknown kind '{k}' (netlist, target, curve or none)"
        ))),
    }
}

/// A target (bundled name or object) as a magnitude-only baseline: its
/// points inside its valid range.
pub fn target_baseline(spec: &Value) -> Result<MagnitudeBaseline, RequestError> {
    let t = target::resolve(spec).map_err(|e| baseline_err(e.to_string()))?;
    let (lo, hi) = t.valid_range_hz;
    let (f, l): (Vec<f64>, Vec<f64>) = t
        .curve
        .freqs()
        .iter()
        .zip(t.curve.db())
        .filter(|(f, _)| **f >= lo * (1.0 - 1e-9) && **f <= hi * (1.0 + 1e-9))
        .map(|(f, l)| (*f, *l))
        .unzip();
    let curve = Curve::new(f, l).map_err(|e| {
        baseline_err(format!(
            "target '{}': fewer than two points in its valid range ({e})",
            t.name
        ))
    })?;
    let serial = t.to_json().to_string();
    let mut notes = vec![format!(
        "Baseline: target '{}' ({}), fixture {}, reference point {}, valid {:.0} Hz to {:.0} Hz; licence {}.",
        t.name, t.label, t.fixture, t.reference_point, lo, hi, t.provenance.licence
    )];
    if !t.flags.is_empty() {
        notes.push(format!("Target flags: {}.", t.flags.join(", ")));
    }
    Ok(MagnitudeBaseline {
        kind: "target",
        name: t.name.clone(),
        label: t.label.clone(),
        fixture: Some(t.fixture.clone()),
        sha256: sha256::sha256_hex(serial.as_bytes()),
        curve,
        notes,
    })
}

/// An imported curve (an `acoustilab-curve` document of a pressure) as a
/// magnitude-only baseline. Its phase, if any, is not used.
pub fn curve_baseline(v: &Value, label: Option<String>) -> Result<MagnitudeBaseline, RequestError> {
    let c = IoCurve::from_json(v).map_err(|e| baseline_err(format!("curve: {e}")))?;
    if c.quantity != Quantity::Pressure {
        return Err(baseline_err(format!(
            "curve: a baseline must be a sound pressure curve, not {}",
            c.quantity.name()
        )));
    }
    let curve = Curve::new(c.freqs_hz.clone(), c.level_db())
        .map_err(|e| baseline_err(format!("curve: {e}")))?;
    let serial = v.to_string();
    let origin = c
        .sidecar
        .provenance
        .as_ref()
        .and_then(|p| p.origin)
        .map_or("not stated".to_string(), |o| o.name().to_string());
    let name = label.unwrap_or_else(|| "imported curve".into());
    let mut notes = vec![format!(
        "Baseline: {name}, {} points from {:.1} Hz to {:.0} Hz, origin {origin}, fixture {}. Its phase is not used: the curve is inverted as a magnitude.",
        c.len(),
        c.freqs_hz[0],
        c.freqs_hz[c.len() - 1],
        c.sidecar.fixture.as_deref().unwrap_or("not stated"),
    )];
    if let Some(s) = &c.sidecar.smoothing {
        notes.push(format!(
            "The curve was smoothed ({}) before import.",
            s.name()
        ));
    }
    Ok(MagnitudeBaseline {
        kind: "curve",
        label: name.clone(),
        name,
        fixture: c.sidecar.fixture.clone(),
        sha256: sha256::sha256_hex(serial.as_bytes()),
        curve,
        notes,
    })
}

fn num(o: &Map<String, Value>, k: &str) -> Result<Option<f64>, RequestError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map(Some)
            .ok_or_else(|| options_err(format!("'{k}' must be a finite number"))),
    }
}

fn boolean(o: &Map<String, Value>, k: &str) -> Result<Option<bool>, RequestError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| options_err(format!("'{k}' must be true or false"))),
    }
}

fn pair(o: &Map<String, Value>, k: &str) -> Result<Option<(f64, f64)>, RequestError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) if a.len() == 2 => match (a[0].as_f64(), a[1].as_f64()) {
            (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok(Some((x, y))),
            _ => Err(options_err(format!("'{k}' must be two finite numbers"))),
        },
        Some(_) => Err(options_err(format!("'{k}' must be [low, high]"))),
    }
}

/// Reads the options object ("" or null for defaults). Returns the
/// options and whether the taps are wanted in the report.
pub fn parse_options(v: &Value) -> Result<(Options, bool), RequestError> {
    let mut opts = Options::default();
    if v.is_null() {
        return Ok((opts, true));
    }
    let o = object(v, "options")?;
    only(
        o,
        &[
            "mode",
            "phase",
            "fs_Hz",
            "n",
            "band_Hz",
            "threshold_ms",
            "pre_samples",
            "ir_length_check",
            "verify",
            "taps",
            "inversion",
        ],
        "options",
    )?;
    if let Some(m) = o.get("mode").filter(|m| !m.is_null()) {
        opts.mode = match m.as_str() {
            Some("difference") => Mode::Difference,
            Some("absolute") => Mode::Absolute,
            _ => return Err(options_err("'mode' must be difference or absolute")),
        };
    }
    if let Some(p) = o.get("phase").filter(|p| !p.is_null()) {
        opts.phase = p
            .as_str()
            .and_then(PhaseRequest::parse)
            .ok_or_else(|| options_err("'phase' must be auto, minimum, mixed or linear"))?;
    }
    if let Some(fs) = num(o, "fs_Hz")? {
        opts.fs_hz = fs;
    }
    match o.get("n") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if s == "auto" => opts.n = None,
        Some(v) => {
            opts.n = Some(
                v.as_u64()
                    .ok_or_else(|| options_err("'n' must be a power of two or \"auto\""))?
                    as usize,
            )
        }
    }
    match pair(o, "band_Hz")? {
        Some(b) => opts.band_hz = b,
        // 20 kHz, or 0.9 of Nyquist when that is lower (19.8 kHz at 44.1 kHz).
        None => opts.band_hz.1 = opts.band_hz.1.min(super::GUARD_FRACTION * opts.fs_hz / 2.0),
    }
    if let Some(t) = num(o, "threshold_ms")? {
        opts.threshold_s = t * 1e-3;
    }
    match o.get("pre_samples") {
        None | Some(Value::Null) => {}
        Some(v) => {
            opts.pre = Some(
                v.as_u64()
                    .ok_or_else(|| options_err("'pre_samples' must be a non-negative integer"))?
                    as usize,
            )
        }
    }
    if let Some(b) = boolean(o, "ir_length_check")? {
        opts.length_check = b;
    }
    if let Some(b) = boolean(o, "verify")? {
        opts.verify = b;
    }
    let taps = boolean(o, "taps")?.unwrap_or(true);
    if let Some(inv) = o.get("inversion").filter(|i| !i.is_null()) {
        let io = object(inv, "inversion")?;
        only(
            io,
            &["band_Hz", "smoothing", "boost_cap_dB", "notch_limit_dB"],
            "inversion",
        )?;
        let mut x = InversionOptions::default();
        if let Some(b) = pair(io, "band_Hz")? {
            x.band_hz = b;
        }
        if let Some(s) = io.get("smoothing").filter(|s| !s.is_null()) {
            x.smoothing = s
                .as_u64()
                .ok_or_else(|| options_err("inversion 'smoothing' must be N of 1/N octave"))?
                as u32;
        }
        if let Some(c) = num(io, "boost_cap_dB")? {
            x.boost_cap_db = c;
        }
        if let Some(c) = num(io, "notch_limit_dB")? {
            x.notch_limit_db = c;
        }
        opts.inversion = x;
    }
    opts.check().map_err(options_err)?;
    Ok((opts, taps))
}

fn pairs(v: &[(f64, f64)]) -> Value {
    Value::Array(v.iter().map(|(a, b)| json!([a, b])).collect())
}

/// The report of an audition filter.
pub fn to_json(a: &Audition, taps: bool) -> Value {
    let mut o = json!({
        "schema": "acoustilab-audition-filter/0.1",
        "mode": a.mode.as_str(),
        "fs_Hz": a.fs_hz,
        "n": a.n,
        "latency_samples": a.latency_samples,
        "latency_s": a.latency_samples as f64 / a.fs_hz,
        "band_Hz": [a.band_hz.0, a.band_hz.1],
        "anchor_Hz": [super::ANCHOR_HZ.0, super::ANCHOR_HZ.1],
        "anchor_gain_dB": a.anchor_gain_db,
        "phase": a.phase,
        "ir_length": a.length,
        "ir_length_error": a.length_error,
        "check": a.check,
        "tail_energy_dB": a.tail_energy_db,
        "pre_energy_dB": a.pre_energy_db,
        "max_boost_dB": {"dB": a.max_boost_db.0, "at_Hz": a.max_boost_db.1},
        "max_cut_dB": {"dB": a.max_cut_db.0, "at_Hz": a.max_cut_db.1},
        "frequencies_Hz": a.grid_hz,
        "design_dB": a.design_db,
        "fir_dB": a.fir_db,
        "error_dB": a.error_db,
        "candidate": a.candidate,
        "baseline": a.baseline,
        "flags": a.flags,
        "notes": a.notes,
        "state": a.state,
    });
    o["inversion"] = match &a.inverse {
        None => Value::Null,
        Some(inv) => {
            let (boost, boost_at) = inv.max_boost();
            let (dep, dep_at) = inv.max_departure();
            json!({
                "band_Hz": [inv.band_hz.0, inv.band_hz.1],
                "smoothing": format!("1/{}", inv.options.smoothing),
                "envelope_smoothing": format!("1/{}", inv.options.envelope_smoothing),
                "boost_cap_dB": inv.options.boost_cap_db,
                "notch_limit_dB": inv.options.notch_limit_db,
                "beta": inv.beta,
                "reference_dB": inv.reference_db,
                "method": "Kirkeby-Nelson: c = b/(b^2 + beta), b = baseline re its band mean after 1/6-octave power smoothing and the notch rule; beta = 1/(4*10^(cap/10)) inside the band; outside it the filter holds its band-edge level",
                "notches": inv.notches,
                "max_boost_dB": {"dB": boost, "at_Hz": boost_at},
                "max_departure_from_exact_dB": {"dB": dep, "at_Hz": dep_at},
                "frequencies_Hz": inv.freqs,
                "inverse_dB": inv.inverse_db,
                "exact_inverse_dB": inv.exact_db,
                "smoothed_baseline_dB": inv.smoothed_db,
            })
        }
    };
    if taps {
        o["taps"] = json!(a.taps);
    }
    if let Some(c) = &a.check {
        o["check"]["exceeded_Hz"] = pairs(&c.exceeded_hz);
    }
    o
}
