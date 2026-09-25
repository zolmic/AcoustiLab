//! Time-domain outputs and rational models of a solved network (spec
//! Sections 3, 10 and 16; erratum E46). See `docs/time-domain.md`.
//!
//! * [`uniform`]: the network re-solved on the linear FFT grid, with the
//!   DC rule, the high-frequency asymptote and the taper.
//! * [`impulse`]: impulse, step and energy-time curve; late (negative-time)
//!   energy; group delay of the uniform re-solve.
//! * [`minphase`]: minimum-phase counterpart by the real cepstrum, excess
//!   phase and group delay, the minimum-phase decision.
//! * [`vfit`]: vector fitting (rational model, zeros, state space, group
//!   delay); [`poles`]: the fit of a probe, the pole/zero table, the
//!   impulse-length check and the attribution of poles to parameters.
//! * [`fft`], [`dense`]: the numerical kernels; [`wav`]: float WAV output.
//!
//! The uniform grid is the interface for the auralization layer:
//! [`uniform::uniform_responses`] gives the spectra of any probes at a
//! chosen fs and N, [`minphase::min_phase`] their minimum-phase
//! counterparts, and [`fft::irfft`] turns either into a filter.

pub mod dense;
pub mod fft;
pub mod impulse;
pub mod minphase;
pub mod poles;
pub mod report;
pub mod uniform;
pub mod vfit;
pub mod wav;

pub use impulse::{group_delay_dense, impulse, Impulse};
pub use minphase::{excess_phase, min_phase, phase_decision, MinPhase, MinPhaseOptions};
pub use poles::{fit_probe, ir_length_check, PoleFit, PoleFitOptions};
pub use uniform::{uniform_response, uniform_responses, UniformOptions, UniformResponse};
pub use vfit::{vector_fit, RationalModel, VfOptions};
