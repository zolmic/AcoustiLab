//! Minimum phase by the real cepstrum, excess phase, excess group delay
//! and the phase-mode decision (spec Section 16).
//!
//! Two minimum-phase products, both from the real cepstrum:
//!
//! * [`min_phase`], the **analysis counterpart**: the continuous-time
//!   minimum-phase function with the model's magnitude (the solved band,
//!   continued above it by the model's asymptotic power law, untapered).
//!   The excess phase and the phase-mode decision are measured against it.
//! * [`min_phase_fir`], the **minimum-phase filter**: the discrete
//!   minimum-phase sequence with the magnitude actually served on the grid,
//!   taper included. It is causal by construction; its phase differs from
//!   the analysis counterpart near the band edge (the taper's zero at
//!   Nyquist adds about a sample of delay, 0.9° at 1 kHz at 48 kHz).
//!
//! **Method.** A discrete real cepstrum approximates the continuous
//! Hilbert transform only well below its Nyquist frequency, and a zero or
//! pole at DC puts a log singularity on the grid. Both are removed
//! analytically first:
//!
//! 1. H = D(s)^m·(1 + s/ω_t)^n·X, with D(s) = s/(s + ω_c), m the order of
//!    the zero (or pole) at DC and f_c = ω_c/2π its corner from the uniform
//!    grid's low-frequency probe (see [`super::uniform`]), and ω_t = 2π·f_hi
//!    (the top of the solved band). D^m and (1 + s/ω_t)^n are
//!    minimum-phase with known phase. The remainder X is finite at DC, its
//!    value there taken from the probe, X(0) = H(f_a)/D(f_a)^m (the value
//!    at the lowest bin would be wrong wherever X still varies below it),
//!    and flat above f_hi. The filter divides out D^m only.
//! 2. **Refinement** (zero padding): |X| on a grid `refine` times finer,
//!    from X's impulse response zero-padded to `refine`·N samples (split at
//!    N/2 so negative times stay negative); the original bins keep their
//!    exact values. Dividing out D first matters: a leak corner below the
//!    bin spacing gives H an impulse response far longer than N, whose
//!    zero-padded spectrum would be wrong near DC, while X's is short.
//! 3. **Extension** (analysis only): the grid continues to `extend`·fs/2
//!    with X flat there, so the discrete (cot-kernel) Hilbert transform
//!    approaches the continuous one over the band.
//! 4. Real cepstrum of ln|X| (floored at 1e-15 of its peak), folded (c₀,
//!    2c_n for 0 < n < M/2, c_{M/2}), exponentiated: X_min = exp(FFT(folded)),
//!    and the analytic factors multiplied back on the original bins.
//!
//! With the defaults (refine 4, extend 8: a cepstrum of 32·N points) the
//! analysis counterpart of a known minimum-phase network agrees with its
//! analytic phase to 0.01° for a flat-topped response and to 1° at 19 kHz
//! for one still falling 12 dB/octave at 20 kHz (0.05° below 1 kHz); the
//! filter keeps its energy at negative times below −89 dB for the same
//! networks (`tests/time.rs`; numpy study: `tools/time/minphase_study.py`). Both
//! spectra have the magnitude of H at every bin.
//!
//! **Limit.** The cepstrum knows |H| only on the grid. A notch whose zeros
//! lie closer to the jω axis than about a bin spacing (Q_z = 1000 at 1 kHz
//! against 5.86 Hz bins) is not resolved: the phase errs by degrees next
//! to it and the excess group delay spikes to milliseconds, so the
//! decision reads `mixed` for a minimum-phase network. The impulse report
//! lists the rational fit's right-half-plane zeros beside the decision for
//! that reason (`docs/time-domain.md`).
//!
//! **Polarity.** The excess phase is referred to the all-pass with unit DC
//! gain: when the excess phase at the lowest solved bin is nearer ±π than
//! 0 the counterpart's sign is inverted (`polarity` −1), i.e. the network
//! inverts polarity.

use super::fft::{hermitian_full, irfft, Fft};
use super::uniform::{UniformResponse, HF_BASELINE_OCTAVES, HF_SLOPE_RANGE};
use crate::C64;
use serde::Serialize;
use std::f64::consts::PI;

/// Default grid refinement of the cepstrum (zero padding of the IR).
pub const REFINE_DEFAULT: usize = 4;
/// Default frequency-axis extension of the analysis cepstrum.
pub const EXTEND_DEFAULT: usize = 8;
/// Largest cepstrum length accepted (2^22 points).
pub const CEPSTRUM_MAX: usize = 1 << 22;
/// Floor of |X| relative to its peak before the logarithm.
pub const LOG_FLOOR: f64 = 1e-15;
/// Excess group delay below which minimum phase may be the default (spec
/// Section 16: "about 0.5 ms", set below the 1–2 ms audibility figures
/// attributed to Blauert and Laws 1978; to be confirmed from that paper).
pub const EXCESS_GD_THRESHOLD_S: f64 = 0.5e-3;
/// Bins more than this far below the trusted band's peak magnitude are
/// left out of the decision (the excess phase is still reported there).
pub const DECISION_FLOOR_DB: f64 = 60.0;

#[derive(Debug, Clone)]
pub struct MinPhaseOptions {
    pub refine: usize,
    pub extend: usize,
}

impl Default for MinPhaseOptions {
    fn default() -> Self {
        MinPhaseOptions {
            refine: REFINE_DEFAULT,
            extend: EXTEND_DEFAULT,
        }
    }
}

/// The low-frequency asymptote H ≈ K·s^m below the corner f_c, with the
/// probe (f_a, H(f_a)) that fixes X(0).
#[derive(Debug, Clone, Copy)]
pub struct DcAsymptote {
    pub order: i32,
    pub corner_hz: f64,
    pub probe: Option<(f64, C64)>,
}

impl DcAsymptote {
    /// From a uniform re-solve's extrapolation record.
    pub fn of(resp: &UniformResponse) -> DcAsymptote {
        let e = &resp.extrapolation;
        DcAsymptote {
            order: e.dc_order,
            corner_hz: e.dc_corner_hz.unwrap_or(resp.freq(resp.solved.0)),
            probe: e
                .dc_probe_hz
                .zip(e.dc_probe_value)
                .map(|(f, (re, im))| (f, C64::new(re, im))),
        }
    }

    /// Estimated from the two lowest solved bins: m the rounded log-log
    /// slope when it exceeds 1/2 in size, f_c the lowest bin, no probe.
    fn from_bins(h: &[C64], k_lo: usize, df: f64) -> DcAsymptote {
        let (a, b) = (h[k_lo].norm(), h[k_lo + 1].norm());
        let n = if a > 0.0 && b > 0.0 {
            (b / a).ln() / ((k_lo + 1) as f64 / k_lo as f64).ln()
        } else {
            0.0
        };
        DcAsymptote {
            order: if n.abs() > 0.5 {
                (n.round() as i32).clamp(-4, 4)
            } else {
                0
            },
            corner_hz: k_lo as f64 * df,
            probe: None,
        }
    }
}

/// The analysis counterpart on the uniform grid.
#[derive(Debug, Clone, Serialize)]
pub struct MinPhase {
    /// H_min at bins 0..=N/2 (same magnitude as H).
    #[serde(skip)]
    pub values: Vec<C64>,
    /// Order m of the DC factor divided out (positive: zeros at DC).
    pub dc_order: i32,
    /// Its corner f_c, Hz.
    #[serde(rename = "dc_corner_Hz")]
    pub dc_corner_hz: f64,
    /// Asymptotic exponent n above the solved band.
    pub hf_slope: f64,
    /// +1, or −1 when the network inverts polarity.
    pub polarity: f64,
    /// Length of the cepstrum, refine·extend·N.
    pub cepstrum_len: usize,
}

/// Analysis counterpart of a uniform re-solve.
pub fn min_phase(resp: &UniformResponse, opts: &MinPhaseOptions) -> MinPhase {
    min_phase_spectrum(
        &resp.values,
        resp.solved,
        resp.df(),
        Some(DcAsymptote::of(resp)),
        opts,
    )
}

/// Minimum-phase filter of a uniform re-solve (see the module docs), with
/// the analysis counterpart's polarity.
pub fn min_phase_fir(resp: &UniformResponse, refine: usize, polarity: f64) -> Vec<C64> {
    min_phase_fir_spectrum(
        &resp.values,
        resp.solved,
        resp.df(),
        Some(DcAsymptote::of(resp)),
        refine,
        polarity,
    )
}

/// Powers of two not exceeding the cepstrum limit.
fn lengths(n: usize, refine: usize, extend: usize) -> (usize, usize) {
    let mut refine = refine.max(1).next_power_of_two();
    let mut extend = extend.max(1).next_power_of_two();
    while n * refine * extend > CEPSTRUM_MAX && extend > 1 {
        extend /= 2;
    }
    while n * refine * extend > CEPSTRUM_MAX && refine > 1 {
        refine /= 2;
    }
    (refine, extend)
}

/// The remainder X = H/factor at the bins, with X(0) from the probe (or
/// the lowest solved bin), and |X| on the grid `refine` times finer up to
/// Nyquist (exact at the original bins).
fn remainder(
    h: &[C64],
    k_lo: usize,
    df: f64,
    factor: &dyn Fn(f64) -> C64,
    probe: Option<(f64, C64)>,
    refine: usize,
) -> (Vec<C64>, Vec<f64>) {
    let half = h.len() - 1;
    let n = 2 * half;
    let mut x: Vec<C64> = (0..=half)
        .map(|k| {
            if k == 0 {
                C64::new(0.0, 0.0)
            } else {
                h[k] / factor(k as f64 * df)
            }
        })
        .collect();
    let x0 = match probe {
        Some((fa, ha)) => ha / factor(fa),
        None => x[k_lo],
    };
    x[0] = C64::new(x0.norm() * x0.re.signum(), 0.0);
    x[half] = C64::new(x[half].re, 0.0);
    let nr = n * refine;
    let mut fine: Vec<f64> = if refine > 1 {
        let ir = irfft(&x);
        let mut buf = vec![C64::new(0.0, 0.0); nr];
        for i in 0..n / 2 {
            buf[i] = C64::new(ir[i], 0.0);
            buf[nr - n / 2 + i] = C64::new(ir[n / 2 + i], 0.0);
        }
        Fft::new(nr).forward(&mut buf);
        buf[..=nr / 2].iter().map(|v| v.norm()).collect()
    } else {
        x.iter().map(|v| v.norm()).collect()
    };
    for (k, v) in x.iter().enumerate() {
        fine[k * refine] = v.norm();
    }
    (x, fine)
}

/// exp of the folded real cepstrum of ln(mag) (half spectrum of M/2 + 1
/// points, floored at [`LOG_FLOOR`] of its peak): the minimum-phase
/// spectrum with magnitude `mag`, full length M.
fn cepstral_min_phase(mag: &[f64]) -> Vec<C64> {
    let m_len = 2 * (mag.len() - 1);
    let peak = mag
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0, f64::max);
    let floor = peak * LOG_FLOOR;
    let half_log: Vec<C64> = mag
        .iter()
        .map(|&r| {
            let r = if r.is_finite() { r.max(floor) } else { peak };
            C64::new(r.ln(), 0.0)
        })
        .collect();
    let mut cep = hermitian_full(&half_log);
    let fft = Fft::new(m_len);
    fft.inverse(&mut cep);
    for v in cep.iter_mut().take(m_len / 2).skip(1) {
        *v = C64::new(2.0 * v.re, 0.0);
    }
    cep[0] = C64::new(cep[0].re, 0.0);
    cep[m_len / 2] = C64::new(cep[m_len / 2].re, 0.0);
    for v in cep.iter_mut().skip(m_len / 2 + 1) {
        *v = C64::new(0.0, 0.0);
    }
    fft.forward(&mut cep);
    cep.iter_mut().for_each(|v| *v = v.exp());
    cep
}

/// Analysis counterpart of a half spectrum `h` (bins 0..=N/2, spacing `df`)
/// whose bins `solved.0..=solved.1` are the model's own values. Without
/// `dc` the DC asymptote is estimated from the two lowest solved bins.
pub fn min_phase_spectrum(
    h: &[C64],
    solved: (usize, usize),
    df: f64,
    dc: Option<DcAsymptote>,
    opts: &MinPhaseOptions,
) -> MinPhase {
    let half = h.len() - 1;
    let n = 2 * half;
    let (refine, extend) = lengths(n, opts.refine, opts.extend);
    let m_len = n * refine * extend;
    // The Nyquist bin holds only Re H (Hermitian symmetry), so a band
    // solved up to Nyquist is continued from the bin below it.
    let (k_lo, k_hi) = (solved.0, solved.1.min(half - 1));
    let dc = dc.unwrap_or_else(|| DcAsymptote::from_bins(h, k_lo, df));
    let slope = |k1: usize, k2: usize| {
        let (a, b) = (h[k1].norm(), h[k2].norm());
        if a > 0.0 && b > 0.0 {
            (b / a).ln() / (k2 as f64 / k1 as f64).ln()
        } else {
            0.0
        }
    };
    let k_b = (((k_hi as f64) * 2f64.powf(-HF_BASELINE_OCTAVES)).round() as usize)
        .clamp(k_lo, k_hi.saturating_sub(1));
    let n_hi = slope(k_b, k_hi).clamp(HF_SLOPE_RANGE.0, HF_SLOPE_RANGE.1);
    let w_c = 2.0 * PI * dc.corner_hz;
    let f_hi = k_hi as f64 * df;
    let w_t = 2.0 * PI * f_hi;
    let order = dc.order;
    let factor = move |f: f64| -> C64 {
        let s = C64::new(0.0, 2.0 * PI * f);
        (s / (s + w_c)).powi(order) * (C64::new(1.0, 2.0 * PI * f / w_t)).powf(n_hi)
    };
    let (x, fine) = remainder(h, k_lo, df, &factor, dc.probe, refine);
    // |X| on the extended grid: the refined values up to f_hi, then the
    // power law divided by the factor (nearly flat).
    let dfine = df / refine as f64;
    let jhi = k_hi * refine;
    let mh = h[k_hi].norm();
    let mut rem: Vec<f64> = (0..=m_len / 2)
        .map(|j| {
            if j <= jhi {
                fine[j]
            } else {
                let f = j as f64 * dfine;
                mh * (f / f_hi).powf(n_hi) / factor(f).norm()
            }
        })
        .collect();
    rem[0] = x[0].norm();
    let xmin = cepstral_min_phase(&rem);
    // Original bin k sits at index k·refine of the extended grid.
    let mut values: Vec<C64> = (0..=half)
        .map(|k| {
            let phase = if k == 0 {
                0.0
            } else {
                (xmin[k * refine] * factor(k as f64 * df)).arg()
            };
            C64::from_polar(h[k].norm(), phase)
        })
        .collect();
    let ex = (h[k_lo] / values[k_lo]).arg();
    let polarity = if ex.cos() < 0.0 { -1.0 } else { 1.0 };
    finish(&mut values, h, polarity);
    MinPhase {
        values,
        dc_order: dc.order,
        dc_corner_hz: dc.corner_hz,
        hf_slope: n_hi,
        polarity,
        cepstrum_len: m_len,
    }
}

/// Minimum-phase filter spectrum of a half spectrum `h` (see
/// [`min_phase_spectrum`] for the arguments): the discrete minimum phase of
/// |h| as served, with the DC factor divided out analytically.
pub fn min_phase_fir_spectrum(
    h: &[C64],
    solved: (usize, usize),
    df: f64,
    dc: Option<DcAsymptote>,
    refine: usize,
    polarity: f64,
) -> Vec<C64> {
    let half = h.len() - 1;
    let (refine, _) = lengths(2 * half, refine, 1);
    let dc = dc.unwrap_or_else(|| DcAsymptote::from_bins(h, solved.0, df));
    let w_c = 2.0 * PI * dc.corner_hz;
    let order = dc.order;
    let factor = move |f: f64| -> C64 {
        let s = C64::new(0.0, 2.0 * PI * f);
        (s / (s + w_c)).powi(order)
    };
    let (x, mut fine) = remainder(h, solved.0, df, &factor, dc.probe, refine);
    fine[0] = x[0].norm();
    let xmin = cepstral_min_phase(&fine);
    let mut values: Vec<C64> = (0..=half)
        .map(|k| {
            let phase = if k == 0 {
                0.0
            } else {
                (xmin[k * refine] * factor(k as f64 * df)).arg()
            };
            C64::from_polar(h[k].norm(), phase)
        })
        .collect();
    finish(&mut values, h, polarity);
    values
}

/// Applies the polarity and makes DC and Nyquist real with |H|.
fn finish(values: &mut [C64], h: &[C64], polarity: f64) {
    let half = values.len() - 1;
    if polarity < 0.0 {
        values.iter_mut().for_each(|v| *v = -*v);
    }
    values[0] = C64::new(polarity * h[0].norm(), 0.0);
    values[half] = C64::new(h[half].norm() * values[half].re.signum(), 0.0);
}

/// Excess phase φ_ex = unwrap(arg(H/H_min)) and excess group delay
/// −dφ_ex/dω (central differences of the unwrapped excess phase; one-sided
/// at the ends), at bins 0..=N/2. The unwrap starts at bin `k0` (the lowest
/// solved bin) from its principal value.
#[derive(Debug, Clone, Serialize)]
pub struct ExcessPhase {
    #[serde(rename = "phase_rad")]
    pub phase_rad: Vec<f64>,
    #[serde(rename = "group_delay_s")]
    pub group_delay_s: Vec<f64>,
}

pub fn excess_phase(h: &[C64], h_min: &[C64], df: f64, k0: usize) -> ExcessPhase {
    let n = h.len();
    let raw: Vec<f64> = h
        .iter()
        .zip(h_min)
        .map(|(a, b)| {
            if a.norm() > 0.0 && b.norm() > 0.0 {
                (a / b).arg()
            } else {
                0.0
            }
        })
        .collect();
    let mut ph = raw.clone();
    for k in k0 + 1..n {
        let mut d = raw[k] - raw[k - 1];
        d -= 2.0 * PI * (d / (2.0 * PI)).round();
        ph[k] = ph[k - 1] + d;
    }
    for k in (0..k0).rev() {
        let mut d = raw[k] - raw[k + 1];
        d -= 2.0 * PI * (d / (2.0 * PI)).round();
        ph[k] = ph[k + 1] + d;
    }
    let dw = 2.0 * PI * df;
    let gd = (0..n)
        .map(|k| {
            let (a, b) = match k {
                0 => (0, 1.min(n - 1)),
                _ if k == n - 1 => (k - 1, k),
                _ => (k - 1, k + 1),
            };
            if a == b {
                0.0
            } else {
                -(ph[b] - ph[a]) / ((b - a) as f64 * dw)
            }
        })
        .collect();
    ExcessPhase {
        phase_rad: ph,
        group_delay_s: gd,
    }
}

/// The Section 16 phase-mode decision.
#[derive(Debug, Clone, Serialize)]
pub struct PhaseDecision {
    #[serde(rename = "threshold_s")]
    pub threshold_s: f64,
    /// Trusted band used: solved, unshaded, within [`DECISION_FLOOR_DB`] of
    /// its peak magnitude.
    #[serde(rename = "band_Hz")]
    pub band_hz: Option<(f64, f64)>,
    /// Bins counted.
    pub bins: usize,
    /// Largest |excess group delay| in the band, s, and where.
    #[serde(rename = "max_excess_group_delay_s")]
    pub max_excess_gd_s: f64,
    #[serde(rename = "at_Hz")]
    pub at_hz: Option<f64>,
    /// Pure-delay estimate: the smallest excess group delay in the band
    /// (clamped at 0). An all-pass factor's group delay is positive at
    /// every frequency, so this bounds the pure delay from above and equals
    /// it once the all-pass delay has died out at the top of the band.
    #[serde(rename = "pure_delay_s")]
    pub pure_delay_s: f64,
    /// Largest excess group delay after removing the pure delay, s.
    #[serde(rename = "max_excess_group_delay_less_delay_s")]
    pub max_excess_gd_less_delay_s: f64,
    /// −1 when the network inverts polarity.
    pub polarity: f64,
    /// True when minimum phase may be the default phase mode.
    pub min_phase_default: bool,
    /// The phase mode to use: `minimum` or `mixed`.
    pub mode: &'static str,
}

/// Applies the decision rule to the excess group delay over `trusted` bins.
pub fn phase_decision(
    h: &[C64],
    excess: &ExcessPhase,
    trusted: &[bool],
    df: f64,
    polarity: f64,
    threshold_s: f64,
) -> PhaseDecision {
    let peak = h
        .iter()
        .zip(trusted)
        .filter(|(_, t)| **t)
        .map(|(v, _)| v.norm())
        .fold(0.0, f64::max);
    let floor = peak * 10f64.powf(-DECISION_FLOOR_DB / 20.0);
    let bins: Vec<usize> = (0..h.len())
        .filter(|&k| trusted[k] && k > 0 && h[k].norm() >= floor && peak > 0.0)
        .collect();
    let (mut max_abs, mut at, mut min_gd, mut max_gd) =
        (0.0f64, None, f64::INFINITY, f64::NEG_INFINITY);
    for &k in &bins {
        let g = excess.group_delay_s[k];
        if g.abs() > max_abs {
            max_abs = g.abs();
            at = Some(k as f64 * df);
        }
        min_gd = min_gd.min(g);
        max_gd = max_gd.max(g);
    }
    let pure = if bins.is_empty() {
        0.0
    } else {
        min_gd.max(0.0)
    };
    let ok = !bins.is_empty() && max_abs < threshold_s;
    PhaseDecision {
        threshold_s,
        band_hz: match (bins.first(), bins.last()) {
            (Some(a), Some(b)) => Some((*a as f64 * df, *b as f64 * df)),
            _ => None,
        },
        bins: bins.len(),
        max_excess_gd_s: max_abs,
        at_hz: at,
        pure_delay_s: pure,
        max_excess_gd_less_delay_s: if bins.is_empty() {
            0.0
        } else {
            (max_gd - pure).max(pure - min_gd)
        },
        polarity,
        min_phase_default: ok,
        mode: if ok { "minimum" } else { "mixed" },
    }
}
