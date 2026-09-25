//! JSON-in, JSON-out API of the targets module (`acoustilab::targets`)
//! behind the wasm exports appended to `lib.rs`. Shapes are documented in
//! docs/targets.md ("Interfaces"). Failures are `{"error": .., "kind": ..}`
//! with kind `target`, `options`, `curve`, `csv`, `input`, or an engine
//! error kind from [`crate::api::error_value`].

use crate::api;
use acoustilab::targets::curve::Curve;
use acoustilab::targets::metrics::{evaluate, Normalisation, Options, Response};
use acoustilab::targets::smoothing::{smooth_complex, smooth_power, FRACTIONS};
use acoustilab::targets::target::{self, BandParams, Shelves, Target};
use acoustilab::targets::{fixture, grid, import, scores, TargetError};
use acoustilab::{Circuit, C64};
use serde_json::{json, Value};

fn target_error(e: &TargetError) -> Value {
    let kind = match e {
        TargetError::Curve(_) => "curve",
        TargetError::Target { .. } => "target",
        TargetError::Options(_) => "options",
        TargetError::Csv { .. } => "csv",
    };
    let mut v = json!({"error": e.to_string(), "kind": kind});
    if let TargetError::Csv { line, .. } = e {
        v["line"] = json!(line);
    }
    v
}

fn input_error(msg: impl Into<String>) -> Value {
    json!({"error": msg.into(), "kind": "input"})
}

fn options_error(msg: impl Into<String>) -> Value {
    json!({"error": msg.into(), "kind": "options"})
}

fn parse_json(text: &str, what: &str) -> Result<Value, Value> {
    serde_json::from_str(text).map_err(|e| input_error(format!("{what} JSON: {e}")))
}

/// Summary of one target for lists.
fn summary(t: &Target) -> Value {
    json!({
        "name": t.name,
        "label": t.label,
        "group": t.group,
        "primary": t.primary,
        "fixture": t.fixture,
        "fixture_label": fixture::fixture(&t.fixture).map(|f| f.label.clone()),
        "family": t.family,
        "flags": t.flags,
        "licence": t.provenance.licence,
        "provenance_class": t.provenance.class.as_str(),
        "valid_range_Hz": [t.valid_range_hz.0, t.valid_range_hz.1],
    })
}

/// `{"targets": [summary], "fixtures": [fixture], "models": [model],
/// "smoothing_fractions": [1, 2, 3, 6, 12, 24, 48]}`.
pub fn targets_list_value() -> Value {
    json!({
        "targets": target::bundled().iter().map(summary).collect::<Vec<_>>(),
        "fixtures": fixture::fixtures(),
        "models": scores::models().iter().map(|m| json!({
            "id": m.id, "label": m.label, "kind": m.kind,
            "training": {"fixture": m.training_fixture, "target_family": m.training_family},
            "source": m.source,
        })).collect::<Vec<_>>(),
        "smoothing_fractions": FRACTIONS,
    })
}

/// A target given as a bundled name, as JSON text of a name (`"\"name\""`),
/// or as a target object.
fn resolve_spec(spec: &str) -> Result<Target, Value> {
    let s = spec.trim();
    let v = if s.starts_with('{') || s.starts_with('"') {
        parse_json(s, "target")?
    } else {
        Value::String(s.to_string())
    };
    target::resolve(&v).map_err(|e| target_error(&e))
}

/// The full target object.
pub fn target_value(spec: &str) -> Value {
    match resolve_spec(spec) {
        Ok(t) => t.to_json(),
        Err(e) => e,
    }
}

fn smoothing_option(v: &Value, key: &str) -> Result<Option<u32>, Value> {
    let not_offered = || {
        options_error(format!(
            "{key}: that fraction is not offered; use one of {FRACTIONS:?} or none"
        ))
    };
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s == "none" => Ok(None),
        Value::String(s) => match s.strip_prefix("1/").map(str::parse::<u64>) {
            Some(Ok(n)) => u32::try_from(n).map(Some).map_err(|_| not_offered()),
            _ => Err(options_error(format!(
                "{key}: '{s}' is not 'none', N or '1/N'"
            ))),
        },
        Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| options_error(format!("{key} must be a positive integer")))
            .and_then(|n| u32::try_from(n).map(Some).map_err(|_| not_offered())),
        _ => Err(options_error(format!("{key} must be 'none', N or '1/N'"))),
    }
    .and_then(|n| match n {
        Some(n) if !FRACTIONS.contains(&n) => Err(options_error(format!(
            "{key}: 1/{n} octave is not offered; use one of {FRACTIONS:?} or none"
        ))),
        n => Ok(n),
    })
}

/// Options shared by `target_metrics` and `target_curve`, plus the keys
/// only `target_metrics` reads (`probe`, `fixture`).
struct Parsed {
    options: Options,
    probe: Option<String>,
    fixture: Option<String>,
}

fn parse_options(text: &str) -> Result<Parsed, Value> {
    let mut p = Parsed {
        options: Options::default(),
        probe: None,
        fixture: None,
    };
    if text.trim().is_empty() {
        return Ok(p);
    }
    let v = parse_json(text, "options")?;
    let obj = v
        .as_object()
        .ok_or_else(|| options_error("options must be a JSON object"))?;
    let o = &mut p.options;
    for (k, x) in obj {
        let num = |x: &Value| {
            x.as_f64()
                .filter(|f| f.is_finite() && *f > 0.0)
                .ok_or_else(|| options_error(format!("{k} must hold positive numbers")))
        };
        match k.as_str() {
            "probe" => {
                p.probe = Some(
                    x.as_str()
                        .ok_or_else(|| options_error("probe must be a string"))?
                        .to_string(),
                )
            }
            "fixture" => {
                p.fixture = match x {
                    Value::Null => None,
                    Value::String(s) => Some(s.clone()),
                    _ => return Err(options_error("fixture must be a string or null")),
                }
            }
            "measured" => {
                o.measured = x
                    .as_bool()
                    .ok_or_else(|| options_error("measured must be true or false"))?
            }
            "normalisation" => {
                let n = x.as_object().filter(|n| n.len() == 1).ok_or_else(|| {
                    options_error(
                        "normalisation must be {\"at_Hz\": f} or {\"band_mean_Hz\": [lo, hi]}",
                    )
                })?;
                o.normalisation =
                    match (n.get("at_Hz"), n.get("band_mean_Hz")) {
                        (Some(f), None) => Normalisation::At(num(f)?),
                        (None, Some(Value::Array(b))) if b.len() == 2 => {
                            let (a, c) = (num(&b[0])?, num(&b[1])?);
                            if c <= a {
                                return Err(options_error("band_mean_Hz must be [low, high]"));
                            }
                            Normalisation::BandMean(a, c)
                        }
                        _ => return Err(options_error(
                            "normalisation must be {\"at_Hz\": f} or {\"band_mean_Hz\": [lo, hi]}",
                        )),
                    };
            }
            "smoothing" => o.smoothing = smoothing_option(x, k)?,
            "tracking_smoothing" => o.tracking_smoothing = smoothing_option(x, k)?,
            "personalisation" => {
                o.shelves = match x {
                    Value::Null => None,
                    _ => Some(Shelves::from_json(x).map_err(|e| target_error(&e))?),
                }
            }
            "grid_Hz" => {
                let g = x
                    .as_array()
                    .ok_or_else(|| options_error("grid_Hz must be an array"))?
                    .iter()
                    .map(num)
                    .collect::<Result<Vec<f64>, Value>>()?;
                grid::check(&g).map_err(options_error)?;
                o.grid_hz = g;
            }
            "band" => {
                let b = x
                    .as_object()
                    .ok_or_else(|| options_error("band must be an object"))?;
                let mut params = BandParams::default();
                for (bk, bv) in b {
                    let w = bv
                        .as_f64()
                        .filter(|w| w.is_finite() && *w >= 0.0)
                        .ok_or_else(|| {
                            options_error(format!("band.{bk} must be a non-negative number"))
                        })?;
                    let i = match bk.as_str() {
                        "above_2kHz_dB" => 0,
                        "above_8kHz_dB" => 1,
                        _ => return Err(options_error(format!("unknown band key '{bk}'"))),
                    };
                    params.widening[i].2 = w;
                }
                o.band = params;
            }
            _ => return Err(options_error(format!("unknown option '{k}'"))),
        }
    }
    Ok(p)
}

/// Reads a curve object `{"frequencies_Hz", "dB" | "spl_dB"}` or one
/// pressure probe of a result document.
fn read_curve(v: &Value, probe: Option<&str>) -> Result<(Curve, Option<String>, String), Value> {
    let nums = |x: &Value, what: &str| -> Result<Vec<f64>, Value> {
        x.as_array()
            .ok_or_else(|| input_error(format!("{what} must be an array of numbers")))?
            .iter()
            .map(|n| {
                n.as_f64()
                    .ok_or_else(|| input_error(format!("{what} must hold numbers (no nulls)")))
            })
            .collect()
    };
    let f = nums(&v["frequencies_Hz"], "frequencies_Hz")?;
    if let Some(probes) = v.get("probes").and_then(Value::as_array) {
        let pressure: Vec<&Value> = probes
            .iter()
            .filter(|p| p.get("spl_dB").is_some())
            .collect();
        let p = match probe {
            Some(id) => probes
                .iter()
                .find(|p| p["id"] == id)
                .ok_or_else(|| input_error(format!("the result has no probe '{id}'")))?,
            None if pressure.len() == 1 => pressure[0],
            None => {
                let ids: Vec<&str> = pressure.iter().filter_map(|p| p["id"].as_str()).collect();
                return Err(input_error(format!(
                    "choose a pressure probe with the 'probe' option: {}",
                    ids.join(", ")
                )));
            }
        };
        let id = p["id"].as_str().unwrap_or_default().to_string();
        if p.get("spl_dB").is_none() {
            return Err(input_error(format!(
                "probe '{id}' is not an acoustic pressure"
            )));
        }
        let c = Curve::new(f, nums(&p["spl_dB"], "spl_dB")?).map_err(|e| target_error(&e))?;
        let drive = v["meta"]["drive"]["label"].as_str().map(str::to_string);
        return Ok((c, drive, id));
    }
    let db = match (v.get("dB"), v.get("spl_dB")) {
        (Some(d), None) | (None, Some(d)) => nums(d, "dB")?,
        _ => {
            return Err(input_error(
                "a curve needs frequencies_Hz and one of dB or spl_dB",
            ))
        }
    };
    let label = v["label"].as_str().unwrap_or("curve").to_string();
    Ok((
        Curve::new(f, db).map_err(|e| target_error(&e))?,
        None,
        label,
    ))
}

/// Evaluates a response against a target: the full report of
/// `acoustilab::targets::metrics::Report::to_json`.
///
/// `input_json` is a result document of `solve`, a curve object
/// `{"frequencies_Hz", "dB"}`, or `{"left": .., "right": ..}` of either.
/// `target_spec` is a bundled name or a target object. `options_json` (""
/// for defaults) is described in docs/targets.md.
pub fn target_metrics_value(input_json: &str, target_spec: &str, options_json: &str) -> Value {
    let run = || -> Result<Value, Value> {
        let input = parse_json(input_json, "input")?;
        let target = resolve_spec(target_spec)?;
        let p = parse_options(options_json)?;
        let is_result = |v: Option<&Value>| v.is_some_and(|v| v.get("probes").is_some());
        if p.options.measured
            && (is_result(Some(&input))
                || is_result(input.get("left"))
                || is_result(input.get("right")))
        {
            return Err(options_error(
                "a solve result is simulated: 'measured' is only for imported measurements",
            ));
        }
        let probe = p.probe.as_deref();
        let resp = match (input.get("left"), input.get("right")) {
            (Some(l), Some(r)) => {
                let (lc, drive, label) = read_curve(l, probe)?;
                let (rc, _, _) = read_curve(r, probe)?;
                Response {
                    left: lc,
                    right: Some(rc),
                    fixture: p.fixture.clone(),
                    label: format!("{label} (left and right)"),
                    drive,
                }
            }
            (None, None) => {
                let (c, drive, label) = read_curve(&input, probe)?;
                Response {
                    left: c,
                    right: None,
                    fixture: p.fixture.clone(),
                    label,
                    drive,
                }
            }
            _ => return Err(input_error("give both 'left' and 'right', or neither")),
        };
        evaluate(&resp, &target, &p.options)
            .map(|r| r.to_json())
            .map_err(|e| target_error(&e))
    };
    run().unwrap_or_else(|e| e)
}

/// A target on the evaluation grid for plotting: normalised as the options
/// say, with personalisation, and its preference band.
/// `{"name", "fixture", "grid_Hz", "reference_dB", "target_dB",
/// "band_lower_dB", "band_upper_dB", "band": {classes, widening, source}}`.
pub fn target_curve_value(target_spec: &str, options_json: &str) -> Value {
    let run = || -> Result<Value, Value> {
        let t = resolve_spec(target_spec)?;
        let o = parse_options(options_json)?.options;
        let g = &o.grid_hz;
        let norm = o.normalisation;
        let undefined = || {
            options_error(format!(
                "the target is undefined at the normalisation ({})",
                norm.to_json()
            ))
        };
        let shift = |v: Vec<Option<f64>>| -> Result<Vec<Option<f64>>, Value> {
            let off = norm.offset(g, &v).ok_or_else(undefined)?;
            Ok(v.into_iter().map(|x| x.map(|x| x - off)).collect())
        };
        let reference = shift(t.on_grid(g, None))?;
        let pers = shift(t.on_grid(g, o.shelves.as_ref()))?;
        let band = target::preference_band(g, &o.band, |v| norm.offset(g, v));
        let edge = |e: &[f64]| -> Vec<Option<f64>> {
            reference
                .iter()
                .zip(e)
                .map(|(r, d)| r.map(|r| r + d))
                .collect()
        };
        Ok(json!({
            "name": t.name,
            "label": t.label,
            "fixture": t.fixture,
            "flags": t.flags,
            "normalisation": norm.to_json(),
            "personalisation": o.shelves.map(|s| s.to_json()),
            "grid_Hz": g,
            "reference_dB": reference,
            "target_dB": pers,
            "band_lower_dB": edge(&band.lower),
            "band_upper_dB": edge(&band.upper),
            "band": target::classes_json(&o.band),
        }))
    };
    run().unwrap_or_else(|e| e)
}

/// Smooths a curve. `curve_json` is `{"frequencies_Hz", "dB"}` (power
/// smoothing) or `{"frequencies_Hz", "re", "im"}` (complex smoothing);
/// `fraction` is "none", "N" or "1/N" with N in 1, 2, 3, 6, 12, 24, 48.
pub fn smooth_value(curve_json: &str, fraction: &str) -> Value {
    let run = || -> Result<Value, Value> {
        let v = parse_json(curve_json, "curve")?;
        let frac = fraction.trim();
        let n = smoothing_option(
            &if frac.chars().all(|c| c.is_ascii_digit()) && !frac.is_empty() {
                json!(frac.parse::<u64>().unwrap_or(0))
            } else {
                json!(frac)
            },
            "fraction",
        )?;
        let label = n.map_or("none".to_string(), |n| format!("1/{n} octave"));
        if v.get("re").is_some() || v.get("im").is_some() {
            let get = |k: &str| -> Result<Vec<f64>, Value> {
                v[k].as_array()
                    .ok_or_else(|| input_error(format!("{k} must be an array")))?
                    .iter()
                    .map(|x| {
                        x.as_f64()
                            .ok_or_else(|| input_error(format!("{k} must hold numbers")))
                    })
                    .collect()
            };
            let f = get("frequencies_Hz")?;
            let (re, im) = (get("re")?, get("im")?);
            if re.len() != im.len() {
                return Err(input_error("re and im differ in length"));
            }
            let h: Vec<C64> = re.iter().zip(&im).map(|(a, b)| C64::new(*a, *b)).collect();
            let s = match n {
                None => h,
                Some(n) => smooth_complex(&f, &h, n).map_err(|e| target_error(&e))?,
            };
            return Ok(json!({
                "frequencies_Hz": f,
                "re": s.iter().map(|z| z.re).collect::<Vec<_>>(),
                "im": s.iter().map(|z| z.im).collect::<Vec<_>>(),
                "smoothing": label,
                "method": "complex",
            }));
        }
        let (c, _, _) = read_curve(&v, None)?;
        let s = match n {
            None => c,
            Some(n) => smooth_power(&c, n).map_err(|e| target_error(&e))?,
        };
        Ok(json!({
            "frequencies_Hz": s.freqs(),
            "dB": s.db(),
            "smoothing": label,
            "method": "power",
        }))
    };
    run().unwrap_or_else(|e| e)
}

/// Imports a fixture-tagged target CSV; `fixture` and `name` ("" for none)
/// supply the tags the file lacks. Returns the target object.
pub fn import_target_csv_value(text: &str, fixture: &str, name: &str) -> Value {
    let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    match import::import_target_csv(text, opt(fixture).as_deref(), opt(name).as_deref()) {
        Ok(t) => t.to_json(),
        Err(e) => target_error(&e),
    }
}

/// The fixture a probe of a netlist reads (inferred from its ear-load
/// macro): `{"probe", "fixture": id | null, "fixture_label": .. | null}`.
pub fn probe_fixture_value(netlist_json: &str, overrides_json: &str, probe: &str) -> Value {
    let overrides = match api::parse_overrides(overrides_json) {
        Ok(o) => o,
        Err(e) => return e,
    };
    match Circuit::from_json_with(netlist_json, &overrides) {
        Ok(c) => {
            if !c.probes.iter().any(|p| p.id == probe) {
                return json!({"error": format!("no probe '{probe}'"), "kind": "probe", "probe": probe});
            }
            let f = fixture::for_probe(&c, probe);
            json!({
                "probe": probe,
                "fixture": f.map(|f| f.id.clone()),
                "fixture_label": f.map(|f| f.label.clone()),
            })
        }
        Err(e) => api::error_value(&e),
    }
}

/// Harman-style reconstruction on a baseline target (bundled name or
/// object). `options_json`: `{"preset": "olive_welti_2015_mean"}` or shelf
/// keys of `personalisation`, and an optional `"name"`.
pub fn reconstruct_target_value(baseline_spec: &str, options_json: &str) -> Value {
    let run = || -> Result<Value, Value> {
        let base = resolve_spec(baseline_spec)?;
        let mut v = if options_json.trim().is_empty() {
            json!({})
        } else {
            parse_json(options_json, "options")?
        };
        let obj = v
            .as_object_mut()
            .ok_or_else(|| options_error("options must be a JSON object"))?;
        let name = match obj.remove("name") {
            None => format!("{} (Harman-style)", base.name),
            Some(Value::String(s)) => s,
            Some(_) => return Err(options_error("name must be a string")),
        };
        let shelves = match obj.remove("preset") {
            Some(Value::String(id)) => {
                if !obj.is_empty() {
                    return Err(options_error("give a preset or shelf settings, not both"));
                }
                target::recipe_preset(&id)
                    .ok_or_else(|| options_error(format!("no recipe preset '{id}'")))?
            }
            Some(_) => return Err(options_error("preset must be a string")),
            None => Shelves::from_json(&v).map_err(|e| target_error(&e))?,
        };
        target::reconstruct_harman_style(&base, &shelves, &name)
            .map(|t| t.to_json())
            .map_err(|e| target_error(&e))
    };
    run().unwrap_or_else(|e| e)
}
