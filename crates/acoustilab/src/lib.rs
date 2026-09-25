//! AcoustiLab engine: a generic modified-nodal-analysis solver over a typed
//! electro-mechano-acoustic netlist, with a thermoviscous element library
//! for headphone design (see docs/spec and docs/conventions.md).
//!
//! ```no_run
//! let text = std::fs::read_to_string("examples/sealed_cup.json").unwrap();
//! let circuit = acoustilab::Circuit::from_json(&text).unwrap();
//! let result = circuit.solve().unwrap();
//! println!("{}", result.to_json());
//! ```

pub mod air;
pub mod circuit;
pub mod diag;
pub mod drive;
pub mod elements;
pub mod error;
pub mod expr;
pub mod grid;
pub mod linalg;
pub mod mna;
pub mod modes;
pub mod netlist;
pub mod params;
pub mod solve;
pub mod special;
pub mod targets;
pub mod thermoviscous;
pub mod ts;
pub mod units;
pub mod validity;

/// Complex double, the scalar of every phasor in the engine (RMS amplitude,
/// time convention e^{+jωt}).
pub type C64 = num_complex::Complex64;

pub use air::AirState;
pub use circuit::Circuit;
pub use error::{Error, Result};
pub use solve::SolveResult;
