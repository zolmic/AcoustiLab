//! JSON API of the audition filter (spec Section 16; `docs/auralization.md`).
//!
//! [`audition_filter_value`]`(candidate, baseline, options)` takes three
//! JSON texts (`acoustilab::audition::report` documents their keys) and
//! returns the filter report with its f32 taps, or `{"error", "kind", ..}`
//! with kind `options` or `baseline` for a bad request, or the engine's
//! kinds (`json`, `netlist`, `probe`, ...) with `"design": "candidate"` or
//! `"baseline"` naming the netlist at fault.
//!
//! The worker that runs the export keeps the last few prepared designs
//! (compiled circuits and their solves) keyed by the SHA-256 of the
//! netlist text, the overrides and the probe. A page that re-designs the
//! filter while the candidate changes (a slider drag during playback)
//! re-solves only the candidate; the result is the same as without the
//! cache (`tests/audition.rs`).

use crate::api::error_value;
use acoustilab::audition::report::{
    parse_baseline, parse_design, parse_options, to_json, BaselineSpec, DesignSpec, RequestError,
};
use acoustilab::audition::{audition, sha256::sha256_hex, Baseline, Design};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;

/// Prepared designs kept between calls.
pub const CACHE_SIZE: usize = 4;

thread_local! {
    static CACHE: RefCell<Vec<(String, Design)>> = const { RefCell::new(Vec::new()) };
}

fn request_error(e: RequestError) -> Value {
    json!({"error": e.message, "kind": e.kind})
}

fn parse_text(text: &str, what: &str) -> Result<Value, Value> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text)
        .map_err(|e| json!({"error": format!("{what} JSON: {e}"), "kind": "options"}))
}

/// Cache key: the netlist's hash, the overrides with sorted keys, the probe.
fn key(d: &DesignSpec) -> String {
    let sorted: BTreeMap<String, Value> = d
        .overrides_json
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    format!(
        "{}|{}|{}",
        sha256_hex(d.netlist.as_bytes()),
        serde_json::to_string(&sorted).unwrap_or_default(),
        d.probe.as_deref().unwrap_or("")
    )
}

/// Takes the cached design for `d` out of the cache, or prepares it.
fn take(d: &DesignSpec, which: &str) -> Result<(String, Design), Value> {
    let k = key(d);
    let hit = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.iter().position(|(x, _)| *x == k).map(|i| c.remove(i))
    });
    if let Some(e) = hit {
        return Ok(e);
    }
    Design::new(
        &d.netlist,
        &d.overrides,
        d.overrides_json.clone(),
        d.probe.as_deref(),
    )
    .map(|design| (k, design))
    .map_err(|e| {
        let mut v = error_value(&e);
        v["design"] = json!(which);
        v
    })
}

/// Returns designs to the cache (most recent last), dropping the oldest.
fn give_back(entries: Vec<(String, Design)>) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        for e in entries {
            c.retain(|(k, _)| *k != e.0);
            c.push(e);
        }
        while c.len() > CACHE_SIZE {
            c.remove(0);
        }
    });
}

/// Empties the design cache (a test hook; also frees memory).
pub fn clear_cache() {
    CACHE.with(|c| c.borrow_mut().clear());
}

/// The audition filter of `candidate` against `baseline` (see the module
/// docs).
pub fn audition_filter_value(candidate: &str, baseline: &str, options: &str) -> Value {
    let run = || -> Result<Value, Value> {
        let c = parse_text(candidate, "candidate")?;
        let cand = parse_design(&c, "candidate", false).map_err(request_error)?;
        let b = parse_text(baseline, "baseline")?;
        let base = if b.is_null() {
            BaselineSpec::None
        } else {
            parse_baseline(&b).map_err(request_error)?
        };
        let (opts, taps) =
            parse_options(&parse_text(options, "options")?).map_err(request_error)?;
        let mut cd = take(&cand, "candidate")?;
        let result = match &base {
            BaselineSpec::Netlist(d) => match take(d, "baseline") {
                Ok(mut bd) => {
                    let r = audition(&mut cd.1, Baseline::Design(&mut bd.1), &opts);
                    give_back(vec![bd]);
                    r
                }
                Err(e) => {
                    give_back(vec![cd]);
                    return Err(e);
                }
            },
            BaselineSpec::Magnitude(m) => audition(&mut cd.1, Baseline::Magnitude(m), &opts),
            BaselineSpec::None => audition(&mut cd.1, Baseline::None, &opts),
        };
        give_back(vec![cd]);
        result
            .map(|a| to_json(&a, taps))
            .map_err(|e| error_value(&e))
    };
    run().unwrap_or_else(|e| e)
}
