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
