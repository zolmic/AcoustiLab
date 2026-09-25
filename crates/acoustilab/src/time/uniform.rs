//! The network re-solved on the linear FFT grid (spec Section 16, "From
//! response to impulse response"; Section 3, frequency grid policy).
//!
//! Bins are f_k = k·fs/N, k = 0..N/2. Every bin in the solved band
//! [f_lo, f_hi] is an exact solve of the network at that frequency, never
//! an interpolation of the log grid. By default f_lo is the first bin
//! (fs/N) and f_hi = min(netlist sweep maximum, fs/2). Outside the band:
//!
//! * **Below f_lo** (only when `f_min_Hz` raises it above the first bin):
//!   the power law |H| ∝ f^n with the phase of the lowest solved bin, n
//!   being the log-log slope between the two lowest solved bins.
//! * **DC.** The network is often singular at 0 Hz (floating acoustic
//!   nodes, inductive loops), so DC is never solved. Its value comes from
//!   the model's own low-frequency asymptote, probed by two extra solves at
//!   f_a = fs/(1024·N) and 2·f_a (5.7 and 11.4 mHz at 48 kHz, N = 8192), far
//!   below the first bin, so that a leak corner below the bin spacing is
//!   seen. With n the log-log slope of |H| between them: n > 1/2 (the
//!   response falls towards DC, as through a leak) gives H(0) = 0;
//!   |n| ≤ 1/2 (flat, as in a sealed pressure chamber) gives the real part
//!   of the linear extrapolation 2·H(f_a) − H(2f_a); n < −1/2 (the response
//!   grows towards DC, e.g. the impedance of a compliance) has no finite DC
//!   value: the same extrapolation is used and a warning says the impulse
//!   response does not decay. DC is real in every case. If either probe
//!   solve fails (a network singular that close to DC), the two lowest
//!   solved bins stand in for them.
//! * **DC order and corner.** m = round(n) when |n| > 1/2 is the order of
//!   the zero (or pole) at DC, and the corner f_c = f_a·(|H(f_lo)|/|H(f_a)|)^(1/m),
//!   clamped to [f_a, f_lo], is where that asymptote meets the level at the
//!   lowest bin. The minimum-phase step divides (s/(s + 2πf_c))^m out.
//! * **Above f_hi**: the model's high-frequency asymptote, continued from
//!   the top 1/6 octave of the solved band: |H| ∝ f^n and phase continued
//!   linearly in f (constant group delay). n is clamped to [−4, 1]: a
//!   passive network's response cannot rise faster than 6 dB/octave
//!   asymptotically (an impedance's inductive or mass rise), and a local
//!   slope outside that range at the band edge comes from a nearby
//!   resonance, not from the asymptote.
//!   This continuation is multiplied by a half-cosine taper,
//!   ½·(1 + cos(π·(f − f_hi)/(fs/2 − f_hi))), which is 1 at f_hi and 0 at
//!   Nyquist. When the band reaches Nyquist there is no taper and the
//!   Nyquist bin is the real part of the solved value.
//!
//! The half spectrum then defines a real signal by Hermitian symmetry (real
//! DC and Nyquist bins).

use crate::circuit::{Circuit, Probe};
use crate::drive::DriveInfo;
use crate::error::{Error, Result};
use crate::solve::Scale;
use crate::validity::Shading;
use crate::C64;
use serde::Serialize;
use std::f64::consts::PI;

/// Supported FFT lengths.
pub const N_MIN: usize = 256;
pub const N_MAX: usize = 65536;
/// Default FFT length and sample rate (spec Section 16).
pub const N_DEFAULT: usize = 8192;
pub const FS_DEFAULT: f64 = 48_000.0;
/// Local log-log slope above which the response is taken to vanish at DC
/// (and below whose negative it grows without bound).
pub const DC_SLOPE_LIMIT: f64 = 0.5;
/// The low-frequency asymptote is probed at the bin spacing divided by this.
pub const DC_PROBE_DIVISOR: f64 = 1024.0;
/// Width of the band at the top of the solved range from which the
/// high-frequency asymptote is taken, octaves.
pub const HF_BASELINE_OCTAVES: f64 = 1.0 / 6.0;
/// Clamp on the high-frequency asymptotic exponent.
pub const HF_SLOPE_RANGE: (f64, f64) = (-4.0, 1.0);

#[derive(Debug, Clone)]
pub struct UniformOptions {
    pub fs_hz: f64,
    /// FFT length, a power of two in [`N_MIN`, `N_MAX`].
    pub n: usize,
    /// Lowest directly solved frequency (default: the first bin, fs/N).
    pub f_min_hz: Option<f64>,
    /// Highest directly solved frequency (default: the netlist sweep's
    /// maximum, capped at Nyquist).
    pub f_max_hz: Option<f64>,
}

impl Default for UniformOptions {
    fn default() -> Self {
        UniformOptions {
            fs_hz: FS_DEFAULT,
            n: N_DEFAULT,
            f_min_hz: None,
            f_max_hz: None,
        }
    }
}

impl UniformOptions {
    pub fn check(&self) -> Result<()> {
        let bad = |m: String| Err(Error::Netlist(format!("uniform grid: {m}")));
        if !(self.n.is_power_of_two() && (N_MIN..=N_MAX).contains(&self.n)) {
            return bad(format!(
                "n = {} must be a power of two from {N_MIN} to {N_MAX}",
                self.n
            ));
        }
        if !(self.fs_hz.is_finite() && (1000.0..=768_000.0).contains(&self.fs_hz)) {
            return bad(format!(
                "fs = {} Hz must be between 1 kHz and 768 kHz",
                self.fs_hz
            ));
        }
        for (name, v) in [("f_min", self.f_min_hz), ("f_max", self.f_max_hz)] {
            if v.is_some_and(|f| !(f.is_finite() && f > 0.0)) {
                return bad(format!("{name} must be positive"));
            }
        }
        Ok(())
    }

    pub fn df(&self) -> f64 {
        self.fs_hz / self.n as f64
    }
}

/// How the bins outside the solved band were filled.
#[derive(Debug, Clone, Serialize)]
pub struct Extrapolation {
    /// Lowest and highest directly solved bin frequencies, Hz.
    #[serde(rename = "solved_Hz")]
    pub solved_hz: (f64, f64),
    /// Frequency f_a of the low-frequency probe; `None` when the probe
    /// solves failed and the lowest bins were used instead.
    #[serde(rename = "dc_probe_Hz")]
    pub dc_probe_hz: Option<f64>,
    /// H(f_a) at the probe (re, im).
    pub dc_probe_value: Option<(f64, f64)>,
    /// Log-log slope of |H| at the low-frequency probe.
    pub dc_slope: f64,
    /// `zero`, `extrapolated` or `singular` (see the module docs).
    pub dc_rule: &'static str,
    /// Order m of the zero (m > 0) or pole (m < 0) at DC; 0 when flat.
    pub dc_order: i32,
    /// Corner f_c of the DC asymptote, Hz (when m ≠ 0).
    #[serde(rename = "dc_corner_Hz")]
    pub dc_corner_hz: Option<f64>,
    /// Asymptotic exponent n of |H| ∝ f^n used above the solved band.
    pub hf_slope: f64,
    /// Group delay used to continue the phase above the band, s.
    #[serde(rename = "hf_group_delay_s")]
    pub hf_group_delay_s: f64,
    /// Taper band (start, end), Hz; `None` when the band reaches Nyquist.
    #[serde(rename = "taper_Hz")]
    pub taper_hz: Option<(f64, f64)>,
}

/// One probe re-solved on the uniform grid.
#[derive(Debug, Clone)]
pub struct UniformResponse {
    pub probe: String,
    pub quantity: String,
    pub unit: String,
    pub fs_hz: f64,
    pub n: usize,
    /// H(f_k), k = 0..=N/2; bins 0 and N/2 are real.
    pub values: Vec<C64>,
    /// Bin range solved directly (inclusive).
    pub solved: (usize, usize),
    pub extrapolation: Extrapolation,
    /// The drive the values are stated at (as in a solve).
    pub drive: DriveInfo,
    /// Validity shading of the network.
    pub shading: Shading,
    pub warnings: Vec<String>,
}

impl UniformResponse {
    pub fn df(&self) -> f64 {
        self.fs_hz / self.n as f64
    }

    pub fn freq(&self, k: usize) -> f64 {
        k as f64 * self.df()
    }

    pub fn freqs(&self) -> Vec<f64> {
        (0..self.values.len()).map(|k| self.freq(k)).collect()
    }

    /// The real impulse response h[n] = IDFT(H), n = 0..N−1, in the
    /// probe's unit per unit of drive per sample (circular; negative
    /// times sit at the end of the buffer).
    pub fn impulse(&self) -> Vec<f64> {
        super::fft::irfft(&self.values)
    }

    /// Bins that are directly solved and unshaded (validity band 0).
    pub fn trusted(&self) -> Vec<bool> {
        (0..self.values.len())
            .map(|k| {
                k >= self.solved.0 && k <= self.solved.1 && self.shading.band(self.freq(k)) == 0
            })
            .collect()
    }
}

impl Scale {
    /// Factor that turns a solution at the netlist's source values into the
    /// stated drive at `f` (see `Circuit::solve`).
    pub(crate) fn factor(&self, circuit: &Circuit, f: f64, x: &[C64]) -> Result<C64> {
        Ok(match *self {
            Scale::Constant(k) => C64::new(k, 0.0),
            Scale::Current { amps, source } => {
                let cx = circuit.cx(f);
                let i = circuit.elements[source]
                    .port_flow(&cx, x, &circuit.branches(source), 0)
                    .unwrap_or_default();
                if i.norm() == 0.0 || !i.norm().is_finite() {
                    return Err(Error::Netlist(format!(
                        "drive: the source current is zero at {f} Hz; cannot hold it constant"
                    )));
                }
                C64::new(amps, 0.0) / i
            }
        })
    }
}

fn find_probe<'a>(circuit: &'a Circuit, id: &str) -> Result<&'a Probe> {
    circuit
        .probes
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::Probe {
            id: id.to_string(),
            msg: "no such probe".into(),
        })
}

/// Solves at `f` and reads the probes, at the stated drive.
fn solve_probes(circuit: &Circuit, scale: &Scale, probes: &[&Probe], f: f64) -> Result<Vec<C64>> {
    let mut x = circuit.solve_at(f)?;
    let s = scale.factor(circuit, f, &x)?;
    if s != C64::new(1.0, 0.0) {
        x.iter_mut().for_each(|v| *v *= s);
    }
    probes
        .iter()
        .map(|p| circuit.probe_value(p, f, &x))
        .collect()
}

/// Re-solves the network on the uniform grid for one probe.
pub fn uniform_response(
    circuit: &Circuit,
    probe: &str,
    opts: &UniformOptions,
) -> Result<UniformResponse> {
    Ok(uniform_responses(circuit, &[probe], opts)?.remove(0))
}

/// Re-solves the network on the uniform grid, one solve per bin for all
/// the listed probes.
pub fn uniform_responses(
    circuit: &Circuit,
    probes: &[&str],
    opts: &UniformOptions,
) -> Result<Vec<UniformResponse>> {
    opts.check()?;
    let probes: Vec<&Probe> = probes
        .iter()
        .map(|id| find_probe(circuit, id))
        .collect::<Result<_>>()?;
    let n = opts.n;
    let half = n / 2;
    let df = opts.df();
    let nyquist = opts.fs_hz / 2.0;
    let sweep_max = circuit.freqs.iter().copied().fold(0.0, f64::max);
    let f_hi = opts.f_max_hz.unwrap_or(sweep_max).min(nyquist);
    let f_lo = opts.f_min_hz.unwrap_or(df).max(df);
    let k_lo = (f_lo / df - 1e-9).ceil().max(1.0) as usize;
    let k_hi = ((f_hi / df + 1e-9).floor() as usize).min(half);
    if k_hi < k_lo + 2 {
        return Err(Error::Netlist(format!(
            "uniform grid: the solved band {f_lo} to {f_hi} Hz holds fewer than 3 bins of {df} Hz"
        )));
    }
    let (scale, drive) = circuit.drive_scale()?;
    let mut vals: Vec<Vec<C64>> = vec![vec![C64::new(0.0, 0.0); half + 1]; probes.len()];
    for k in k_lo..=k_hi {
        for (v, x) in vals
            .iter_mut()
            .zip(solve_probes(circuit, &scale, &probes, k as f64 * df)?)
        {
            v[k] = x;
        }
    }
    // The model's low-frequency asymptote (see the module docs).
    let f_a = df / DC_PROBE_DIVISOR;
    let probe_lf = |f: f64| {
        solve_probes(circuit, &scale, &probes, f)
            .ok()
            .filter(|v| v.iter().all(|x| x.is_finite()))
    };
    let lf = probe_lf(f_a).zip(probe_lf(2.0 * f_a));
    let shading = circuit.shading();
    Ok(probes
        .iter()
        .zip(vals)
        .enumerate()
        .map(|(i, (p, mut v))| {
            let mut warnings = Vec::new();
            let lf_i = lf.as_ref().map(|(a, b)| (f_a, a[i], b[i]));
            let extrapolation = fill(&mut v, k_lo, k_hi, df, lf_i, &mut warnings);
            UniformResponse {
                probe: p.id.clone(),
                quantity: p.quantity.clone(),
                unit: p.unit.to_string(),
                fs_hz: opts.fs_hz,
                n,
                values: v,
                solved: (k_lo, k_hi),
                extrapolation,
                drive: drive.clone(),
                shading,
                warnings,
            }
        })
        .collect())
}

/// Log-log slope of |H| between two frequencies (0 when either vanishes).
fn loglog(a: C64, b: C64, fa: f64, fb: f64) -> f64 {
    if a.norm() == 0.0 || b.norm() == 0.0 {
        return 0.0;
    }
    (b.norm() / a.norm()).ln() / (fb / fa).ln()
}

/// Fills the bins outside [k_lo, k_hi] (see the module docs). `lf` holds
/// the low-frequency probe (f_a, H(f_a), H(2f_a)).
fn fill(
    h: &mut [C64],
    k_lo: usize,
    k_hi: usize,
    df: f64,
    lf: Option<(f64, C64, C64)>,
    warnings: &mut Vec<String>,
) -> Extrapolation {
    let half = h.len() - 1;
    let (f1, f2) = (k_lo as f64 * df, (k_lo + 1) as f64 * df);
    // Low side below the solved band.
    let n_lo = loglog(h[k_lo], h[k_lo + 1], f1, f2);
    for k in 1..k_lo {
        h[k] = h[k_lo] * (k as f64 / k_lo as f64).powf(n_lo);
    }
    // DC from the low-frequency asymptote.
    let (fa, ha, fb, hb) = match lf {
        Some((fa, ha, hb)) => (fa, ha, 2.0 * fa, hb),
        None => (f1, h[k_lo], f2, h[k_lo + 1]),
    };
    let n0 = loglog(ha, hb, fa, fb);
    let linear = ha - (hb - ha) * (fa / (fb - fa));
    let dc_rule = if ha.norm() == 0.0 || n0 > DC_SLOPE_LIMIT {
        h[0] = C64::new(0.0, 0.0);
        "zero"
    } else if n0 >= -DC_SLOPE_LIMIT {
        h[0] = C64::new(linear.re, 0.0);
        "extrapolated"
    } else {
        h[0] = C64::new(linear.re, 0.0);
        warnings.push(format!(
            "the response grows towards 0 Hz (slope {n0:.2}); it has no finite DC value and its impulse response does not decay. DC was set to the real part of the linear extrapolation"
        ));
        "singular"
    };
    let dc_order = if n0.abs() > DC_SLOPE_LIMIT {
        (n0.round() as i32).clamp(-4, 4)
    } else {
        0
    };
    let dc_corner = (dc_order != 0).then(|| {
        let r = h[k_lo].norm() / ha.norm();
        let fc = if r.is_finite() && r > 0.0 {
            fa * r.powf(1.0 / dc_order as f64)
        } else {
            f1
        };
        fc.clamp(fa, f1)
    });
    // High side.
    let f_hi = k_hi as f64 * df;
    let mut hf_slope = 0.0;
    let mut tau = 0.0;
    let mut taper = None;
    if k_hi < half {
        let k_b = (((k_hi as f64) * 2f64.powf(-HF_BASELINE_OCTAVES)).round() as usize)
            .clamp(k_lo, k_hi - 1);
        hf_slope = loglog(h[k_b], h[k_hi], k_b as f64, k_hi as f64)
            .clamp(HF_SLOPE_RANGE.0, HF_SLOPE_RANGE.1);
        let mut dphi = 0.0;
        for k in k_b..k_hi {
            if h[k].norm() > 0.0 && h[k + 1].norm() > 0.0 {
                dphi += (h[k + 1] / h[k]).arg();
            }
        }
        let dphi_df = dphi / ((k_hi - k_b) as f64 * df);
        tau = -dphi_df / (2.0 * PI);
        let nyquist = half as f64 * df;
        let (m0, p0) = (h[k_hi].norm(), h[k_hi].arg());
        for (k, v) in h.iter_mut().enumerate().skip(k_hi + 1) {
            let f = k as f64 * df;
            let w = 0.5 * (1.0 + (PI * (f - f_hi) / (nyquist - f_hi)).cos());
            let mag = m0 * (f / f_hi).powf(hf_slope) * w;
            *v = C64::from_polar(mag, p0 + dphi_df * (f - f_hi));
        }
        taper = Some((f_hi, nyquist));
    }
    h[half] = C64::new(h[half].re, 0.0);
    Extrapolation {
        solved_hz: (f1, f_hi),
        dc_probe_hz: lf.map(|l| l.0),
        dc_probe_value: lf.map(|l| (l.1.re, l.1.im)),
        dc_slope: n0,
        dc_rule,
        dc_order,
        dc_corner_hz: dc_corner,
        hf_slope,
        hf_group_delay_s: tau,
        taper_hz: taper,
    }
}
