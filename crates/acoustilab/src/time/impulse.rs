//! Impulse response, step response, energy-time curve and the group delay
//! of the uniform re-solve.
//!
//! **Scaling and time axis.** h[n] = IDFT(H)[n] is the response to a
//! one-sample unit pulse of the drive: in the probe's unit per unit of
//! drive per sample, h[n] ≈ h(t_n)/fs for a continuous response h(t). The
//! DFT buffer is circular; the returned buffers are rotated so that the
//! first `pre` samples hold negative times (the end of the circular
//! buffer): sample m is at t_m = (m − pre)/fs. No other delay is applied.
//!
//! **Step response**: s[m] = Σ_{i ≤ m} h[i] over the rotated buffer, the
//! response to a unit step of the drive starting at the buffer's first
//! sample (in the probe's unit per unit of drive).
//!
//! **Energy-time curve**: 10·log10(|h_a|²/max|h_a|²), where h_a = h + j·Ĥh is
//! the analytic signal (spectrum doubled at positive frequencies and zeroed
//! at negative ones), floored at −300 dB.
//!
//! **Late energy** (the causality measure of spec Section 17 and erratum
//! E46): the energy in the second half of the circular buffer, relative to
//! the total. That half holds negative times, so it collects any
//! non-causal response plus whatever of the causal tail has not decayed by
//! N/(2·fs); E46 sizes N so that it stays below −80 dB.
//!
//! **Dense group delay**: τ_k = Re{DFT(n·h[n])_k / H_k}/fs with signed n
//! (n − N for n ≥ N/2), the exact derivative −dφ/dω of the trigonometric
//! interpolant of the uniform re-solve. No phase unwrapping is involved.

use super::fft::{hermitian_full, Fft};
use crate::C64;
use serde::Serialize;

/// Summary of an impulse response.
#[derive(Debug, Clone, Serialize)]
pub struct Impulse {
    #[serde(rename = "fs_Hz")]
    pub fs_hz: f64,
    pub n: usize,
    /// Samples before t = 0 at the start of the buffers.
    pub pre: usize,
    /// Time of the first sample, s (−pre/fs).
    #[serde(rename = "t0_s")]
    pub t0_s: f64,
    /// Rotated impulse response.
    pub h: Vec<f64>,
    /// Step response over the rotated buffer.
    pub step: Vec<f64>,
    /// Energy-time curve over the rotated buffer, dB re its maximum.
    #[serde(rename = "etc_dB")]
    pub etc_db: Vec<f64>,
    /// Energy at negative times (second half of the circular buffer)
    /// relative to the total, dB.
    #[serde(rename = "late_energy_dB")]
    pub late_energy_db: f64,
    /// Time of the largest |h|, s.
    #[serde(rename = "peak_s")]
    pub peak_s: f64,
}

/// Default number of pre-samples: N/32 (5.3 ms at N = 8192, 48 kHz).
pub fn default_pre(n: usize) -> usize {
    n / 32
}

/// Impulse, step and energy-time curve from a half spectrum H_0..H_{N/2}
/// sampled at fs/N.
pub fn impulse(half: &[C64], fs_hz: f64, pre: usize) -> Impulse {
    let mut full = hermitian_full(half);
    let n = full.len();
    let pre = pre.min(n / 2);
    let fft = Fft::new(n);
    // Analytic signal first (it needs the spectrum).
    let mut analytic = vec![C64::new(0.0, 0.0); n];
    analytic[0] = full[0];
    analytic[n / 2] = full[n / 2];
    for k in 1..n / 2 {
        analytic[k] = full[k] * 2.0;
    }
    fft.inverse(&mut analytic);
    fft.inverse(&mut full);
    let raw: Vec<f64> = full.iter().map(|v| v.re).collect();
    let total: f64 = raw.iter().map(|v| v * v).sum();
    let late: f64 = raw[n / 2..].iter().map(|v| v * v).sum();
    let rot = |m: usize| (m + n - pre) % n;
    let h: Vec<f64> = (0..n).map(|m| raw[rot(m)]).collect();
    let mut acc = 0.0;
    let step: Vec<f64> = h
        .iter()
        .map(|v| {
            acc += v;
            acc
        })
        .collect();
    let env: Vec<f64> = (0..n).map(|m| analytic[rot(m)].norm_sqr()).collect();
    let emax = env.iter().copied().fold(0.0, f64::max);
    let etc_db = env
        .iter()
        .map(|e| {
            if emax > 0.0 {
                (10.0 * (e / emax).log10()).max(-300.0)
            } else {
                -300.0
            }
        })
        .collect();
    let (ipeak, _) =
        h.iter().enumerate().fold(
            (0, 0.0),
            |(im, vm), (i, v)| if v.abs() > vm { (i, v.abs()) } else { (im, vm) },
        );
    Impulse {
        fs_hz,
        n,
        pre,
        t0_s: -(pre as f64) / fs_hz,
        h,
        step,
        etc_db,
        late_energy_db: if total > 0.0 && late > 0.0 {
            10.0 * (late / total).log10()
        } else {
            -300.0
        },
        peak_s: (ipeak as f64 - pre as f64) / fs_hz,
    }
}

/// Group delay of the uniform re-solve (see the module docs), s, at bins
/// 0..=N/2. Bins where H vanishes read 0.
pub fn group_delay_dense(half: &[C64], fs_hz: f64) -> Vec<f64> {
    let mut full = hermitian_full(half);
    let n = full.len();
    let fft = Fft::new(n);
    fft.inverse(&mut full);
    let mut nh: Vec<C64> = full
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let t = if i < n / 2 {
                i as f64
            } else {
                i as f64 - n as f64
            };
            C64::new(v.re * t, 0.0)
        })
        .collect();
    fft.forward(&mut nh);
    half.iter()
        .zip(&nh)
        .map(|(h, x)| {
            if h.norm() > 0.0 {
                (x / h).re / fs_hz
            } else {
                0.0
            }
        })
        .collect()
}
