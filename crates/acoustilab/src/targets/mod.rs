//! Target curves, fractional-octave smoothing, response error metrics and
//! headphone preference scores (spec Sections 10, 11 and 12; errata E6, E7,
//! E36, E47). Definitions, sources and tolerances are in docs/targets.md.
//!
//! * [`Curve`]: a magnitude response in dB on strictly increasing
//!   frequencies, read between samples by linear interpolation in dB on log
//!   frequency.
//! * [`grid`]: the 1/12-octave evaluation grid and band membership.
//! * [`smoothing`]: 1/N-octave power smoothing (and complex smoothing).
//! * [`shelf`]: bass and treble shelving filters as magnitude responses.
//! * [`fixture`]: the fixtures targets and results belong to, and which ear
//!   load of a netlist stands for which fixture.
//! * [`target`]: typed targets with provenance, the bundled set, CSV import,
//!   personalisation, the Harman-style reconstruction and preference bands.
//! * [`metrics`]: the error metric set, the ITU-R BS.708 mask, left-right
//!   tracking, and the report that ties them together.
//! * [`scores`]: the Harman preference models with their greying rules.
//!
//! Every empirical number (model coefficients, masks, shelf settings,
//! listener classes, curves) is read from `data/targets/*.json`, which
//! carries its provenance and licence.

pub mod curve;
pub mod fixture;
pub mod grid;
pub mod import;
pub mod metrics;
pub mod scores;
pub mod shelf;
pub mod smoothing;
pub mod target;

pub use curve::Curve;
pub use metrics::{evaluate, Normalisation, Options, Report, Response};
pub use target::Target;

/// Failures of the targets module: malformed curves, unknown targets or
/// options, and CSV import errors. Messages name what is wrong.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TargetError {
    #[error("curve: {0}")]
    Curve(String),
    #[error("target '{name}': {msg}")]
    Target { name: String, msg: String },
    #[error("options: {0}")]
    Options(String),
    #[error("CSV line {line}: {msg}")]
    Csv { line: usize, msg: String },
}

pub type Result<T> = std::result::Result<T, TargetError>;

/// A flag attached to a result: a stable `code` for the interface and a
/// sentence saying what it means for this result.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Flag {
    pub code: &'static str,
    pub message: String,
}

impl Flag {
    pub fn new(code: &'static str, message: impl Into<String>) -> Flag {
        Flag {
            code,
            message: message.into(),
        }
    }
}

/// Parses one of the embedded data files; they are checked by the tests, so
/// a failure here is a build defect, not an input error.
pub(crate) fn embedded_json(name: &str, text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("data/targets/{name}: {e}"))
}
