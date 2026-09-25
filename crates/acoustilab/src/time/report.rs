//! JSON reports of the time-domain and rational-fit outputs, shared by the
//! command-line runner and the WebAssembly wrapper.

use super::impulse::{default_pre, group_delay_dense, impulse, Impulse};
use super::minphase::{
    excess_phase, min_phase, min_phase_fir, phase_decision, ExcessPhase, MinPhase, MinPhaseOptions,
    PhaseDecision, EXCESS_GD_THRESHOLD_S,
};
use super::poles::{
    attribute_poles, fit_probe, ir_length_check, IrLengthCheck, PoleFit, PoleFitOptions,
    SensitivityOptions,
};
use super::uniform::{uniform_response, UniformOptions, UniformResponse};
use crate::circuit::Circuit;
use crate::error::{Error, Result};
use crate::params::{Overrides, Parametric};
use serde_json::{json, Value};
use std::f64::consts::PI;

/// The probe a report uses when none is named: the netlist's
/// `ui.primary_probe` if it exists, else the first acoustic pressure probe,
/// else the first probe.
pub fn default_probe(p: &Parametric, circuit: &Circuit) -> Result<String> {
    let ui = p
        .doc
        .get("ui")
        .and_then(|u| u.get("primary_probe"))
        .and_then(Value::as_str);
    if let Some(id) = ui.filter(|id| circuit.probes.iter().any(|q| q.id == *id)) {
        return Ok(id.to_string());
    }
    circuit
        .probes
        .iter()
        .find(|q| q.is_pressure)
        .or_else(|| circuit.probes.first())
        .map(|q| q.id.clone())
        .ok_or_else(|| Error::Netlist("the netlist has no probe".into()))
}

/// What an impulse report computes.
#[derive(Debug, Clone)]
pub struct ImpulseRequest {
    pub probe: Option<String>,
    pub uniform: UniformOptions,
    /// Samples before t = 0 in the returned buffers (default N/32).
    pub pre: Option<usize>,
    pub min_phase: MinPhaseOptions,
    pub threshold_s: f64,
    /// Run the rational fit for the E46 impulse-length check.
    pub length_check: bool,
    pub fit: PoleFitOptions,
}

impl Default for ImpulseRequest {
    fn default() -> Self {
        ImpulseRequest {
            probe: None,
            uniform: UniformOptions::default(),
            pre: None,
            min_phase: MinPhaseOptions::default(),
            threshold_s: EXCESS_GD_THRESHOLD_S,
            length_check: true,
            fit: PoleFitOptions::default(),
        }
    }
}

/// Mixed- and minimum-phase impulse responses of one probe with their
/// analysis.
#[derive(Debug, Clone)]
pub struct Impulses {
    pub response: UniformResponse,
    pub mixed: Impulse,
    /// Impulse response of the minimum-phase filter.
    pub minimum: Impulse,
    /// The minimum-phase filter's half spectrum.
    pub fir: Vec<crate::C64>,
    /// The analysis counterpart (excess phase and decision).
    pub min_phase: MinPhase,
    pub excess: ExcessPhase,
    pub decision: PhaseDecision,
    /// Group delay of the uniform re-solve, s.
    pub group_delay: Vec<f64>,
    pub ir_length: Option<IrLengthCheck>,
    pub fit_error: Option<String>,
}

pub fn impulses(circuit: &Circuit, probe: &str, req: &ImpulseRequest) -> Result<Impulses> {
    let response = uniform_response(circuit, probe, &req.uniform)?;
    let pre = req.pre.unwrap_or_else(|| default_pre(response.n));
    let mixed = impulse(&response.values, response.fs_hz, pre);
    let mp = min_phase(&response, &req.min_phase);
    let fir = min_phase_fir(&response, req.min_phase.refine, mp.polarity);
    let minimum = impulse(&fir, response.fs_hz, pre);
    let excess = excess_phase(
        &response.values,
        &mp.values,
        response.df(),
        response.solved.0,
    );
    let decision = phase_decision(
        &response.values,
        &excess,
        &response.trusted(),
        response.df(),
        mp.polarity,
        req.threshold_s,
    );
    let group_delay = group_delay_dense(&response.values, response.fs_hz);
    let (ir_length, fit_error) = if req.length_check {
        match fit_probe(circuit, probe, &req.fit) {
            Ok(fit) => (
                Some(ir_length_check(&fit, response.fs_hz, response.n)),
                None,
            ),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };
    Ok(Impulses {
        response,
        mixed,
        minimum,
        fir,
        min_phase: mp,
        excess,
        decision,
        group_delay,
        ir_length,
        fit_error,
    })
}

impl Impulses {
    /// Energy of the spectrum above the solved band relative to the whole,
    /// dB: the zero-phase taper there band-limits the response, so the
    /// mixed-phase IR's energy at negative times is of this order however
    /// long N is.
    pub fn band_edge_energy_db(&self) -> f64 {
        let v = &self.response.values;
        let half = v.len() - 1;
        let w = |k: usize| if k == 0 || k == half { 1.0 } else { 2.0 };
        let total: f64 = (0..=half).map(|k| w(k) * v[k].norm_sqr()).sum();
        let edge: f64 = (self.response.solved.1 + 1..=half)
            .map(|k| w(k) * v[k].norm_sqr())
            .sum();
        if edge > 0.0 && total > 0.0 {
            10.0 * (edge / total).log10()
        } else {
            -300.0
        }
    }

    /// The report; `spectrum` adds the uniform spectrum and the phase
    /// curves at every bin.
    pub fn to_json(&self, spectrum: bool) -> Value {
        let r = &self.response;
        let mut o = json!({
            "probe": r.probe,
            "quantity": r.quantity,
            "unit": r.unit,
            "fs_Hz": r.fs_hz,
            "n": r.n,
            "df_Hz": r.df(),
            "t0_s": self.mixed.t0_s,
            "dt_s": 1.0 / r.fs_hz,
            "pre_samples": self.mixed.pre,
            "ir": self.mixed.h,
            "ir_min_phase": self.minimum.h,
            "step": self.mixed.step,
            "step_min_phase": self.minimum.step,
            "etc_dB": self.mixed.etc_db,
            "etc_min_phase_dB": self.minimum.etc_db,
            "peak_s": self.mixed.peak_s,
            "late_energy_dB": self.mixed.late_energy_db,
            "late_energy_min_phase_dB": self.minimum.late_energy_db,
            "band_edge_energy_dB": self.band_edge_energy_db(),
            "causal_to_80dB": self.mixed.late_energy_db <= -80.0,
            "decision": self.decision,
            "min_phase": self.min_phase,
            "extrapolation": r.extrapolation,
            "ir_length": self.ir_length,
            "ir_length_error": self.fit_error,
            "drive": r.drive,
            "shading": r.shading,
            "warnings": r.warnings,
            "time_axis": "sample m is at t = t0_s + m*dt_s; the buffers are the circular IDFT rotated by pre_samples; no other delay is applied",
            "scaling": "h[n] = IDFT(H): the response to a one-sample unit pulse of the drive, in the probe's unit per unit of drive per sample",
        });
        if spectrum {
            let v = &r.values;
            o["frequencies_Hz"] = json!(r.freqs());
            o["spectrum"] = json!({
                "re": v.iter().map(|x| x.re).collect::<Vec<_>>(),
                "im": v.iter().map(|x| x.im).collect::<Vec<_>>(),
            });
            o["excess_phase_deg"] = json!(self
                .excess
                .phase_rad
                .iter()
                .map(|x| x.to_degrees())
                .collect::<Vec<_>>());
            o["excess_group_delay_s"] = json!(self.excess.group_delay_s);
            o["group_delay_s"] = json!(self.group_delay);
            o["trusted"] = json!(r.trusted());
        }
        o
    }
}

/// What a pole report computes.
#[derive(Debug, Clone)]
pub struct PoleRequest {
    pub probe: Option<String>,
    pub fit: PoleFitOptions,
    /// Attribute resonances to parameters by re-fits.
    pub attribute: bool,
    pub sensitivity: SensitivityOptions,
    /// FFT length and rate of the impulse-length check.
    pub fs_hz: f64,
    pub n: usize,
    pub state_space: bool,
}

impl Default for PoleRequest {
    fn default() -> Self {
        PoleRequest {
            probe: None,
            fit: PoleFitOptions::default(),
            attribute: false,
            sensitivity: SensitivityOptions::default(),
            fs_hz: super::uniform::FS_DEFAULT,
            n: super::uniform::N_DEFAULT,
            state_space: false,
        }
    }
}

/// Fits a probe and reports poles, zeros, fit error, group delay, the
/// impulse-length check and (optionally) the attribution and state space.
pub fn poles_report(
    p: &Parametric,
    overrides: &Overrides,
    circuit: &Circuit,
    probe: &str,
    req: &PoleRequest,
) -> Result<(PoleFit, Value)> {
    let fit = fit_probe(circuit, probe, &req.fit)?;
    let attribution = if req.attribute {
        Some(attribute_poles(
            p,
            overrides,
            &fit,
            &req.fit,
            &req.sensitivity,
        )?)
    } else {
        None
    };
    let fitted: Vec<_> = fit
        .freqs_hz
        .iter()
        .map(|&f| fit.model.eval(0, crate::C64::new(0.0, 2.0 * PI * f)))
        .collect();
    let report = &fit.report;
    let v = json!({
        "probe": fit.probe,
        "quantity": fit.quantity,
        "unit": fit.unit,
        "band_Hz": fit.band(),
        "order": report.order,
        "asymptote": req.fit.vf.asymptote,
        "weighting": req.fit.vf.weighting,
        "fit": {
            "rms_relative": report.error.rms_relative,
            "rms_dB": report.error.rms_db,
            "max_dB": report.error.max_db,
            "max_deg": report.error.max_deg,
            "iterations": report.iterations,
            "history": report.history,
            "flipped": report.flipped,
        },
        "notes": fit.notes,
        "poles": fit.poles,
        "zeros": fit.zeros,
        "min_phase_in_band": fit.min_phase_in_band(),
        "q_min": req.fit.q_min,
        "weight_min_dB": req.fit.weight_min_db,
        "frequencies_Hz": fit.freqs_hz,
        "response": {
            "re": fit.values.iter().map(|x| x.re).collect::<Vec<_>>(),
            "im": fit.values.iter().map(|x| x.im).collect::<Vec<_>>(),
        },
        "fitted": {
            "re": fitted.iter().map(|x| x.re).collect::<Vec<_>>(),
            "im": fitted.iter().map(|x| x.im).collect::<Vec<_>>(),
        },
        "group_delay_s": fit.freqs_hz.iter().map(|&f| fit.group_delay(f)).collect::<Vec<_>>(),
        "ir_length": ir_length_check(&fit, req.fs_hz, req.n),
        "attribution": attribution,
        "state_space": req.state_space.then(|| fit.model.state_space(0)),
        "drive": fit.drive,
    });
    Ok((fit, v))
}
