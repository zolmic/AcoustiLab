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
//! * design analyses (`sensitivity`, `tornado`, `explain`, `readouts`,
//!   `mc_plan`, `mc_run`, `mc_envelope`, `mc_csv`): see `analysis.rs` and
//!   docs/analysis.md.
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

// ----- Design analyses (docs/analysis.md; logic in analysis.rs) -----------

pub mod analysis;

/// Sensitivities (Jacobian in dB and degrees per percent) of a design.
#[wasm_bindgen]
pub fn sensitivity(netlist_json: &str, overrides_json: &str, options_json: &str) -> String {
    install_panic_hook();
    api::to_string(&analysis::sensitivity_value(
        netlist_json,
        overrides_json,
        options_json,
    ))
}

/// Tornado chart of one metric over the parameters' tolerances.
#[wasm_bindgen]
pub fn tornado(netlist_json: &str, overrides_json: &str, options_json: &str) -> String {
    install_panic_hook();
    api::to_string(&analysis::tornado_value(
        netlist_json,
        overrides_json,
        options_json,
    ))
}

/// Explain sentences generated from re-solves.
#[wasm_bindgen]
pub fn explain(netlist_json: &str, overrides_json: &str, options_json: &str) -> String {
    install_panic_hook();
    api::to_string(&analysis::explain_value(
        netlist_json,
        overrides_json,
        options_json,
    ))
}

/// Impedance, driver and response readouts.
#[wasm_bindgen]
pub fn readouts(netlist_json: &str, overrides_json: &str, options_json: &str) -> String {
    install_panic_hook();
    api::to_string(&analysis::readouts_value(
        netlist_json,
        overrides_json,
        options_json,
    ))
}

/// Monte Carlo or design-of-experiments plan.
#[wasm_bindgen]
pub fn mc_plan(netlist_json: &str, overrides_json: &str, spec_json: &str) -> String {
    install_panic_hook();
    api::to_string(&analysis::mc_plan_value(
        netlist_json,
        overrides_json,
        spec_json,
    ))
}

/// Solves one chunk of a plan's samples.
#[wasm_bindgen]
pub fn mc_run(
    netlist_json: &str,
    overrides_json: &str,
    samples_json: &str,
    options_json: &str,
) -> String {
    install_panic_hook();
    api::to_string(&analysis::mc_run_value(
        netlist_json,
        overrides_json,
        samples_json,
        options_json,
    ))
}

/// Median, 5/10/90/95 % and extreme envelopes of collected runs.
#[wasm_bindgen]
pub fn mc_envelope(runs_json: &str) -> String {
    install_panic_hook();
    api::to_string(&analysis::mc_envelope_value(runs_json))
}

/// Design-of-experiments table of collected runs, as `{"csv": ".."}`.
#[wasm_bindgen]
pub fn mc_csv(runs_json: &str, parameters_json: &str) -> String {
    install_panic_hook();
    api::to_string(&analysis::mc_csv_value(runs_json, parameters_json))
}
