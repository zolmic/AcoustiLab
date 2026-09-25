//! Measured and simulated curves: the [`Curve`] type, its metadata
//! [`Sidecar`] (spec Section 14), and the FRD, ZMA, REW-text and CSV
//! formats (docs/fitting.md).
//!
//! Every curve that enters or leaves the engine carries a sidecar, and two
//! curves whose fixture, compensation or drive differ are not compared
//! without an explicit override ([`sidecar::compare`]).

pub mod curve;
pub mod sidecar;
pub mod text;

pub use curve::{Curve, Quantity};
pub use sidecar::Sidecar;
pub use text::Format;

use std::fmt;

/// A curve or sidecar that cannot be read, written or combined. `line` is
/// the 1-based line of the input file, when the error has one.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveError {
    pub line: Option<usize>,
    pub msg: String,
}

impl CurveError {
    pub fn new(msg: impl Into<String>) -> Self {
        CurveError {
            line: None,
            msg: msg.into(),
        }
    }

    pub fn at(line: usize, msg: impl Into<String>) -> Self {
        CurveError {
            line: Some(line),
            msg: msg.into(),
        }
    }
}

impl fmt::Display for CurveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(l) => write!(f, "line {l}: {}", self.msg),
            None => write!(f, "{}", self.msg),
        }
    }
}

impl std::error::Error for CurveError {}
