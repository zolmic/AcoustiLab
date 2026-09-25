//! WebAssembly bindings of the AcoustiLab engine (wasm-bindgen).
//!
//! Every export takes and returns JSON text, so the JavaScript side needs no
//! generated types beyond strings:
//!
//! * `solve(netlist_json)`: the engine's result document (see
//!   `acoustilab::SolveResult::to_json`) with per-probe `domain` and filled
//!   `unit`, or `{"error": .., "kind": .., ..}`.
//! * `solve_with(netlist_json, overrides_json)`: as `solve`, with parameter
//!   overrides `{"name": value}`.
//! * `parameters(netlist_json, overrides_json)`: the parameter descriptions
//!   for a design panel, with resolved values, and the netlist's `ui` block.
//! * `check(netlist_json)`: `{"ok": true, "nodes": .., ..}` or an error.
//! * `element_types()`: a JSON array of element type names.
//! * `engine_version()`: the engine's name and version.
//! * `take_last_panic()`: the message of the last Rust panic, if any. A panic
//!   aborts (traps) on wasm32; the caller reads the message from the trapped
//!   instance and then instantiates a fresh module.
//!
//! Build with `web/scripts/build-wasm.sh` (see docs/web.md).

pub mod api;
pub mod time;

use std::sync::{Mutex, Once};
use wasm_bindgen::prelude::*;

static LAST_PANIC: Mutex<String> = Mutex::new(String::new());
static HOOK: Once = Once::new();

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(s: &str);
}

/// Stores a panic message for [`take_last_panic`].
pub fn record_panic(msg: &str) {
    if let Ok(mut s) = LAST_PANIC.lock() {
        s.clear();
        s.push_str(msg);
    }
}

fn install_panic_hook() {
    HOOK.call_once(|| {
        // Natively the default hook is kept (tests rely on it); on wasm32
        // the message would otherwise be lost in an `unreachable` trap.
        #[cfg(target_arch = "wasm32")]
        std::panic::set_hook(Box::new(|info| {
            let msg = format!("engine panic: {info}");
            record_panic(&msg);
            console_error(&msg);
        }));
    });
}

/// Test hook for the UI's panic recovery: a netlist whose top level has
/// `"debug_panic": true` makes the wrapper panic before the engine sees it.
/// The engine itself never panics on input (it rejects the key), so the
/// Playwright test needs a deliberate trigger.
fn debug_panic_hook(netlist_json: &str) {
    let requested = serde_json::from_str::<serde_json::Value>(netlist_json)
        .ok()
        .and_then(|v| v.get("debug_panic").and_then(|b| b.as_bool()))
        .unwrap_or(false);
    if requested {
        panic!("debug panic requested by the netlist (test hook)");
    }
}

/// Solves a netlist; returns the result JSON or an error object.
#[wasm_bindgen]
pub fn solve(netlist_json: &str) -> String {
    install_panic_hook();
    debug_panic_hook(netlist_json);
    api::to_string(&api::solve_value(netlist_json))
}

/// Solves a netlist with parameter overrides given as `{"name": value}`
/// JSON ("" for none).
#[wasm_bindgen]
pub fn solve_with(netlist_json: &str, overrides_json: &str) -> String {
    install_panic_hook();
    debug_panic_hook(netlist_json);
    api::to_string(&api::solve_with_value(netlist_json, overrides_json))
}

/// Describes the netlist's parameters (with values under the overrides)
/// and its `ui` block.
#[wasm_bindgen]
pub fn parameters(netlist_json: &str, overrides_json: &str) -> String {
    install_panic_hook();
    api::to_string(&api::parameters_value(netlist_json, overrides_json))
}

/// Parses and compiles a netlist without solving it.
#[wasm_bindgen]
pub fn check(netlist_json: &str) -> String {
    install_panic_hook();
    debug_panic_hook(netlist_json);
    api::to_string(&api::check_value(netlist_json))
}

/// JSON array of the element type names the engine accepts.
#[wasm_bindgen]
pub fn element_types() -> String {
    api::to_string(&api::element_types_value())
}

/// Engine name and version, e.g. "acoustilab 0.1.0".
#[wasm_bindgen]
pub fn engine_version() -> String {
    api::engine_version().to_string()
}

/// Returns and clears the last recorded panic message ("" if none).
#[wasm_bindgen]
pub fn take_last_panic() -> String {
    LAST_PANIC
        .lock()
        .map(|mut s| std::mem::take(&mut *s))
        .unwrap_or_default()
}

// ---------------------------------------------------------------- targets
//
// Target curves, smoothing, response metrics and preference scores
// (docs/targets.md, "Interfaces"). The logic is in `targets.rs`.

pub mod targets;

/// `{"targets": [..], "fixtures": [..], "models": [..], "smoothing_fractions": [..]}`.
#[wasm_bindgen]
pub fn targets_list() -> String {
    install_panic_hook();
    api::to_string(&targets::targets_list_value())
}

/// The full target object for a bundled name or a target object (JSON).
#[wasm_bindgen]
pub fn target(spec: &str) -> String {
    install_panic_hook();
    api::to_string(&targets::target_value(spec))
}

/// A target on the evaluation grid, normalised and personalised, with its
/// preference band.
#[wasm_bindgen]
pub fn target_curve(spec: &str, options_json: &str) -> String {
    install_panic_hook();
    api::to_string(&targets::target_curve_value(spec, options_json))
}

/// Error metrics, BS.708 mask, preference band, tracking and preference
/// scores of a result or curve against a target, with the flags to show.
#[wasm_bindgen]
pub fn target_metrics(input_json: &str, target_spec: &str, options_json: &str) -> String {
    install_panic_hook();
    api::to_string(&targets::target_metrics_value(
        input_json,
        target_spec,
        options_json,
    ))
}

/// Fractional-octave smoothing of a curve ("none", "N" or "1/N").
#[wasm_bindgen]
pub fn smooth(curve_json: &str, fraction: &str) -> String {
    install_panic_hook();
    api::to_string(&targets::smooth_value(curve_json, fraction))
}

/// Imports a fixture-tagged target CSV.
#[wasm_bindgen]
pub fn import_target_csv(text: &str, fixture: &str, name: &str) -> String {
    install_panic_hook();
    api::to_string(&targets::import_target_csv_value(text, fixture, name))
}

/// The fixture a probe of a netlist reads, inferred from its ear load.
#[wasm_bindgen]
pub fn probe_fixture(netlist_json: &str, overrides_json: &str, probe: &str) -> String {
    install_panic_hook();
    api::to_string(&targets::probe_fixture_value(
        netlist_json,
        overrides_json,
        probe,
    ))
}

/// Harman-style reconstruction of a target on a user-supplied baseline.
#[wasm_bindgen]
pub fn reconstruct_target(baseline_spec: &str, options_json: &str) -> String {
    install_panic_hook();
    api::to_string(&targets::reconstruct_target_value(
        baseline_spec,
        options_json,
    ))
}

// ----- Curves, fitting and the virtual rig (docs/fitting.md) -----------------
//
// JSON in, JSON out; logic and error shapes in `fit.rs`.

pub mod fit;

/// Reads an FRD, ZMA, REW text or CSV file into a curve document. `options`
/// is a format name (`auto`, `frd`, `zma`, `rew`, `csv`) or
/// `{"format", "quantity", "sidecar"}`.
#[wasm_bindgen]
pub fn import_curve(text: &str, options: &str) -> String {
    install_panic_hook();
    api::to_string(&fit::import_curve_value(text, options))
}

/// Writes a curve document as `format` (`frd`, `zma`, `rew`, `csv`):
/// `{"format", "extension", "text", "sidecar"}`.
#[wasm_bindgen]
pub fn export_curve(curve_json: &str, format: &str) -> String {
    install_panic_hook();
    api::to_string(&fit::export_curve_value(curve_json, format))
}

/// Checks that two curves were measured under the same conditions
/// (fixture, compensation, drive, ...); `allow_json` lists fields to
/// ignore.
#[wasm_bindgen]
pub fn compare_curves(a_json: &str, b_json: &str, allow_json: &str) -> String {
    install_panic_hook();
    api::to_string(&fit::compare_curves_value(a_json, b_json, allow_json))
}

/// Fits netlist parameters to curves (`acoustilab-fit/0.1` spec); returns
/// the fit report. Bounded by the spec's `max_iterations` and
/// `max_evaluations`.
#[wasm_bindgen]
pub fn fit(netlist_json: &str, spec_json: &str) -> String {
    install_panic_hook();
    api::to_string(&fit::fit_value(netlist_json, spec_json))
}

/// A synthetic measurement from the virtual rig: `{"curve", "format",
/// "extension", "text", "sidecar"}`.
#[wasm_bindgen]
pub fn virtual_measure(netlist_json: &str, spec_json: &str) -> String {
    install_panic_hook();
    api::to_string(&fit::virtual_measure_value(netlist_json, spec_json))
}

/// Probe `probe` of the solved netlist (with parameter overrides, "" for
/// none) as a curve document with a `simulated` sidecar.
#[wasm_bindgen]
pub fn probe_curve(netlist_json: &str, overrides_json: &str, probe: &str) -> String {
    install_panic_hook();
    api::to_string(&fit::probe_curve_value(netlist_json, overrides_json, probe))
}

// ----- Time domain, vector fitting and isolation (docs/time-domain.md, docs/isolation.md)

/// Impulse, step and energy-time curve (mixed and minimum phase), excess
/// phase and group delay, the Section 16 phase decision and the E46
/// impulse-length check of a probe (options: see `time::impulse_value`).
#[wasm_bindgen]
pub fn impulse(netlist_json: &str, overrides_json: &str, options_json: &str) -> String {
    install_panic_hook();
    debug_panic_hook(netlist_json);
    api::to_string(&time::impulse_value(
        netlist_json,
        overrides_json,
        options_json,
    ))
}

/// Vector fit of a probe: poles/Q table, zeros, fit error, group delay and
/// optionally the attribution of resonances to parameters (options: see
/// `time::vector_fit_value`).
#[wasm_bindgen]
pub fn vector_fit(netlist_json: &str, overrides_json: &str, options_json: &str) -> String {
    install_panic_hook();
    debug_panic_hook(netlist_json);
    api::to_string(&time::vector_fit_value(
        netlist_json,
        overrides_json,
        options_json,
    ))
}

/// Passive insertion loss at the drum and the bleed estimate (options: see
/// `time::isolation_value`).
#[wasm_bindgen]
pub fn isolation(netlist_json: &str, overrides_json: &str, options_json: &str) -> String {
    install_panic_hook();
    debug_panic_hook(netlist_json);
    api::to_string(&time::isolation_value(
        netlist_json,
        overrides_json,
        options_json,
    ))
}

// ----- Auralization (docs/auralization.md) -----------------------------------
//
// The audition filter of spec Section 16; logic in `audition.rs`.

pub mod audition;

/// The audition filter (FIR taps and report) of a candidate design against
/// a baseline (another netlist, a target or an imported curve), or the
/// candidate alone in the absolute diagnostic mode. Arguments are JSON
/// texts; see `audition::audition_filter_value`.
#[wasm_bindgen]
pub fn audition_filter(candidate_json: &str, baseline_json: &str, options_json: &str) -> String {
    install_panic_hook();
    api::to_string(&audition::audition_filter_value(
        candidate_json,
        baseline_json,
        options_json,
    ))
}
