//! Thiele–Small parameter sets, their identities, the suspension creep law,
//! and identification from an impedance curve (spec Sections 5 and 17;
//! errata E5 and E18).
//!
//! Two parameter sets describe the same D0 (rigid piston) driver:
//!
//! * the **primary** set of a driver record: fs, Qms, Qes, Re, Mms, Sd
//!   (docs/conventions.md, "Driver records");
//! * the **physical** set the network stamps: Re, Bl, Mms, Cms, Rms, Sd.
//!
//! With ωs = 2π·fs they are related by
//!
//! ```text
//! Cms = 1/(ωs²·Mms)    Rms = ωs·Mms/Qms    Bl = sqrt(ωs·Mms·Re/Qes)
//! Qts = Qms·Qes/(Qms + Qes)                Vas = ρc²·Sd²·Cms
//! ```
//!
//! Identification uses the classical sqrt(R0) bandwidth method (Small,
//! "Direct-radiator loudspeaker system analysis", JAES 20(5), 1972), which
//! is exact for the lumped model without inductance (derivation at
//! [`extract_ts`]), and a Levenberg–Marquardt fit in log-parameter space for
//! models with creep and lossy inductance ([`fit_impedance`]).

use crate::air::AirState;
use crate::C64;
use serde::Serialize;
use std::f64::consts::{FRAC_PI_2, LN_10, PI};

/// Primary parameter set of a driver record (erratum E5): the values Bl, Cms
/// and Rms are derived from, whatever else a datasheet prints.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TsParams {
    /// Free-air resonance frequency, Hz.
    pub fs: f64,
    pub qms: f64,
    pub qes: f64,
    /// DC coil resistance, Ω.
    pub re: f64,
    /// Moving mass including the air load of the stated condition, kg.
    pub mms: f64,
    /// Effective piston area, m².
    pub sd: f64,
}

/// Physical (network) parameter set of the D0 piston model, SI units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PhysicalParams {
    pub re: f64,
    pub bl: f64,
    pub mms: f64,
    pub cms: f64,
    pub rms: f64,
    pub sd: f64,
}

fn check_positive(pairs: &[(&str, f64)]) -> Result<(), String> {
    for (name, v) in pairs {
        if !(v.is_finite() && *v > 0.0) {
            return Err(format!(
                "'{name}' must be a positive finite number, got {v}"
            ));
        }
    }
    Ok(())
}

impl TsParams {
    /// Checks every parameter is positive and finite.
    pub fn validate(&self) -> Result<(), String> {
        check_positive(&[
            ("fs", self.fs),
            ("Qms", self.qms),
            ("Qes", self.qes),
            ("Re", self.re),
            ("Mms", self.mms),
            ("Sd", self.sd),
        ])
    }

    pub fn omega_s(&self) -> f64 {
        2.0 * PI * self.fs
    }

    pub fn qts(&self) -> f64 {
        qts(self.qms, self.qes)
    }

    /// Derives Bl, Cms and Rms (docs/conventions.md, "Driver records").
    pub fn to_physical(&self) -> PhysicalParams {
        let ws = self.omega_s();
        PhysicalParams {
            re: self.re,
            bl: force_factor(self.qes, self.re, self.mms, self.fs),
            mms: self.mms,
            cms: 1.0 / (ws * ws * self.mms),
            rms: ws * self.mms / self.qms,
            sd: self.sd,
        }
    }

    /// Equivalent air volume ρc²·Sd²·Cms, m³.
    pub fn vas(&self, air: &AirState) -> f64 {
        vas(air, self.sd, self.to_physical().cms)
    }
}

impl PhysicalParams {
    /// Checks the parameters: Re, Bl, Mms, Cms and Sd positive, Rms ≥ 0.
    pub fn validate(&self) -> Result<(), String> {
        check_positive(&[
            ("Re", self.re),
            ("Bl", self.bl),
            ("Mms", self.mms),
            ("Cms", self.cms),
            ("Sd", self.sd),
        ])?;
        if !(self.rms.is_finite() && self.rms >= 0.0) {
            return Err(format!("'Rms' must be non-negative, got {}", self.rms));
        }
        Ok(())
    }

    pub fn fs(&self) -> f64 {
        resonance_hz(self.mms, self.cms)
    }

    pub fn qms(&self) -> f64 {
        2.0 * PI * self.fs() * self.mms / self.rms
    }

    pub fn qes(&self) -> f64 {
        electrical_q(self.bl, self.re, self.mms, self.fs())
    }

    pub fn qts(&self) -> f64 {
        qts(self.qms(), self.qes())
    }

    pub fn to_ts(&self) -> TsParams {
        TsParams {
            fs: self.fs(),
            qms: self.qms(),
            qes: self.qes(),
            re: self.re,
            mms: self.mms,
            sd: self.sd,
        }
    }

    pub fn vas(&self, air: &AirState) -> f64 {
        vas(air, self.sd, self.cms)
    }

    /// Mechanical impedance jωMms + Rms + 1/(jωCms), unloaded.
    pub fn mechanical_impedance(&self, omega: f64) -> C64 {
        let jw = C64::new(0.0, omega);
        jw * self.mms + self.rms + (jw * self.cms).inv()
    }

    /// Electrical input impedance of the unloaded D0 model,
    /// Re + Bl²/Zm (spec Appendix C5 with Z_L = 0 and no acoustic load).
    pub fn impedance(&self, omega: f64) -> C64 {
        self.re + self.bl * self.bl / self.mechanical_impedance(omega)
    }
}

/// Resonance identity fs = 1/(2π·sqrt(Mms·Cms)).
pub fn resonance_hz(mms: f64, cms: f64) -> f64 {
    1.0 / (2.0 * PI * (mms * cms).sqrt())
}

/// Electrical Q identity Qes = 2π·fs·Mms·Re/Bl² (spec p. 15).
pub fn electrical_q(bl: f64, re: f64, mms: f64, fs: f64) -> f64 {
    2.0 * PI * fs * mms * re / (bl * bl)
}

/// Force factor from the primary set, Bl = sqrt(2π·fs·Mms·Re/Qes).
pub fn force_factor(qes: f64, re: f64, mms: f64, fs: f64) -> f64 {
    (2.0 * PI * fs * mms * re / qes).sqrt()
}

/// Total Q of two parallel damping mechanisms.
pub fn qts(qms: f64, qes: f64) -> f64 {
    qms * qes / (qms + qes)
}

/// Equivalent volume Vas = ρc²·Sd²·Cms (adiabatic), m³.
pub fn vas(air: &AirState, sd: f64, cms: f64) -> f64 {
    air.rho * air.c * air.c * sd * sd * cms
}

// ----- Creep ---------------------------------------------------------------

/// Suspension creep after Knudsen and Jensen, "Low-frequency loudspeaker
/// models that include suspension creep", JAES 41(1/2), 1993, in the complex
/// form written out in Klippel application note AN49 ("Extended Creep
/// Modeling", rev. 1.1, 2022):
///
/// `C(jω) = C0·[1 − λ·log10(jω/ω0)]`
///
/// with ω0 = 2π·f0 the reference frequency (the driver's fs by default), at
/// which C0 is the real part of the compliance. Because
/// log10(jω/ω0) = log10(ω/ω0) + j·π/(2 ln 10), the imaginary part is the
/// constant −λ·π/(2 ln 10)·C0: a loss that makes the compliance causal (it
/// is analytic in the right half s-plane) and the spring passive on the jω
/// axis for λ ≥ 0, since Re{1/(jωC)} = λ·π/(2 ln 10)·C0/(ω|C|²) ≥ 0. The
/// real-only law C0·[1 − λ·log10(ω/ω0)] violates the Kramers–Kronig
/// relations (erratum E18).
///
/// Re C still falls through zero at f0·10^(1/λ) ([`Creep::zero_crossing_hz`]),
/// so λ is limited to keep Re C > 0 over the engine band [`Creep::BAND_HZ`].
/// The same point limits causality: C(s) vanishes at the real
/// s = ω0·10^(1/λ), so the stiffness 1/(s·C) has a pole there and a driver
/// with creep has one right-half-plane pole just above it (for the
/// Tymphany record, at 42.6 kHz when λ = 0.99·λmax). The λ limit keeps it
/// above the band, and its in-band share of the input impedance stays below
/// 1e-7 (a Hilbert transform of the solver's impedance agrees to 3e-8 at
/// 0.99·λmax and to 1e-13 for λ ≤ λmax/2; tests/driver.rs). This is
/// negligible in the frequency domain, but a time-domain model must replace
/// the law by a realisable approximation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Creep {
    /// Creep factor λ: relative compliance increase per decade of
    /// decreasing frequency.
    pub lambda: f64,
    /// Reference frequency f0, Hz.
    pub f0: f64,
}

impl Creep {
    /// Band over which Re C must stay positive: the engine's internal range
    /// (spec Section 2, item 6: 10 Hz to 40 kHz).
    pub const BAND_HZ: (f64, f64) = (10.0, 40_000.0);

    /// The factor C(jω)/C0 = 1 − λ·log10(jω/ω0).
    pub fn factor(&self, omega: f64) -> C64 {
        let w0 = 2.0 * PI * self.f0;
        C64::new(
            1.0 - self.lambda * (omega / w0).log10(),
            -self.lambda * FRAC_PI_2 / LN_10,
        )
    }

    /// Frequency f0·10^(1/λ) where Re C crosses zero; also where the
    /// stiffness 1/(s·C(s)) has its pole on the positive real s axis
    /// (s = 2π·this). Infinite for λ = 0.
    pub fn zero_crossing_hz(&self) -> f64 {
        if self.lambda > 0.0 {
            self.f0 * 10f64.powf(1.0 / self.lambda)
        } else {
            f64::INFINITY
        }
    }

    /// Largest λ keeping Re C > 0 up to the top of [`Creep::BAND_HZ`]
    /// (exclusive bound).
    pub fn max_lambda(f0: f64) -> f64 {
        let decades = (Self::BAND_HZ.1 / f0).log10();
        if decades > 0.0 {
            1.0 / decades
        } else {
            f64::INFINITY
        }
    }

    /// Rejects λ < 0 (active: Re Zm < 0) and λ that makes Re C ≤ 0 in the band.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.f0.is_finite() && self.f0 > 0.0) {
            return Err(format!(
                "creep reference frequency must be positive, got {}",
                self.f0
            ));
        }
        if !(self.lambda.is_finite() && self.lambda >= 0.0) {
            return Err(format!(
                "creep_lambda must be >= 0 (a negative factor makes the suspension active), got {}",
                self.lambda
            ));
        }
        let max = Self::max_lambda(self.f0);
        if self.lambda >= max {
            return Err(format!(
                "creep_lambda {} makes the real compliance negative below {} Hz; with f0 = {:.4} Hz it must be below {:.4}",
                self.lambda,
                Self::BAND_HZ.1,
                self.f0,
                max
            ));
        }
        Ok(())
    }
}

// ----- Impedance model used for identification ------------------------------

/// Free-air electrical impedance model with creep and LR-2 lossy inductance,
/// parameterised by the identifiable quantities (Mms is not identifiable
/// from impedance alone):
///
/// `Z = Re + jωLe + (R2 ∥ jωL2) + (Re/Qes) / (j·w + 1/Qms + 1/(j·w·c))`
///
/// with w = f/fs and c = 1 − λ·log10(jω/ωs) (creep referenced to fs). For
/// any Mms this equals the D1 driver element with Cms = 1/(ωs²·Mms),
/// Rms = ωs·Mms/Qms, Bl² = ωs·Mms·Re/Qes and the creep reference at fs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ImpedanceModel {
    pub re: f64,
    pub fs: f64,
    pub qms: f64,
    pub qes: f64,
    pub creep_lambda: f64,
    pub le: f64,
    /// LR-2 eddy branch; zero L2 or R2 disables it.
    pub l2: f64,
    pub r2: f64,
}

impl ImpedanceModel {
    /// The model of a D0 driver: no creep, no inductance.
    pub fn from_ts(ts: &TsParams) -> Self {
        ImpedanceModel {
            re: ts.re,
            fs: ts.fs,
            qms: ts.qms,
            qes: ts.qes,
            creep_lambda: 0.0,
            le: 0.0,
            l2: 0.0,
            r2: 0.0,
        }
    }

    pub fn impedance(&self, f: f64) -> C64 {
        let omega = 2.0 * PI * f;
        let jw = C64::new(0.0, omega);
        let w = f / self.fs;
        let jwn = C64::new(0.0, w);
        let c = Creep {
            lambda: self.creep_lambda,
            f0: self.fs,
        }
        .factor(omega);
        let zmot = (self.re / self.qes) / (jwn + 1.0 / self.qms + (jwn * c).inv());
        let mut z = self.re + jw * self.le + zmot;
        if self.l2 > 0.0 && self.r2 > 0.0 {
            let zl = jw * self.l2;
            z += zl * self.r2 / (zl + self.r2);
        }
        z
    }
}

// ----- sqrt(R0) extraction ----------------------------------------------------

/// Thiele–Small estimate from an impedance curve.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TsEstimate {
    pub re: f64,
    /// Frequency of the impedance maximum (parabolic refinement in ln f).
    pub fs: f64,
    pub z_max: f64,
    /// Zmax/Re.
    pub r0: f64,
    /// Frequencies below and above fs where |Z| = Re·sqrt(r0).
    pub f1: f64,
    pub f2: f64,
    pub qms: f64,
    pub qes: f64,
    pub qts: f64,
    /// Identifiability and model-mismatch warnings.
    pub warnings: Vec<String>,
}

impl TsEstimate {
    /// Ratio Qms/Qes (= r0 − 1). Below 0.1 the impedance hump is too small
    /// to identify parameters electrically (spec Section 5).
    pub fn q_ratio(&self) -> f64 {
        self.qms / self.qes
    }

    /// Physical parameters once the moving mass is known (e.g. from an
    /// added-mass measurement): impedance alone does not fix the scale.
    pub fn with_mass(&self, mms: f64, sd: f64) -> PhysicalParams {
        TsParams {
            fs: self.fs,
            qms: self.qms,
            qes: self.qes,
            re: self.re,
            mms,
            sd,
        }
        .to_physical()
    }
}

/// Identifiability limit on Qms/Qes (spec Section 5, "Typical ranges").
pub const MIN_Q_RATIO: f64 = 0.1;

/// Classical sqrt(R0) extraction (Small 1972) from a complex impedance curve
/// on increasing frequencies.
///
/// * Re: `re_dc` if given (an ohmmeter reading), else the low-frequency
///   limit of Re Z extrapolated in f² from the two lowest points (Re Z of
///   the lumped model rises as f² below resonance).
/// * fs and Zmax: the largest |Z|, refined by a parabola in ln f.
/// * f1, f2: where |Z| crosses Re·sqrt(r0), interpolated linearly in ln f.
/// * Qms = fs·sqrt(r0)/(f2 − f1), Qes = Qms/(r0 − 1), Qts = Qms/r0.
///
/// Exactness for the lumped model: with y = X/Rms and r0 = 1 + Bl²/(Rms·Re),
/// Z = Re·(r0 + j·y)/(1 + j·y), so |Z|² = Re²·(r0² + y²)/(1 + y²), which
/// equals Re²·r0 exactly where y = ±sqrt(r0). Since y = Qms·(ω/ωs − ωs/ω),
/// the two roots satisfy ω1·ω2 = ωs² and ω2 − ω1 = ωs·sqrt(r0)/Qms. The only
/// errors are therefore the interpolation errors of the sampled curve.
pub fn extract_ts(freqs: &[f64], z: &[C64], re_dc: Option<f64>) -> Result<TsEstimate, String> {
    let n = freqs.len();
    if n != z.len() {
        return Err("frequency and impedance arrays differ in length".into());
    }
    if n < 5 {
        return Err("need at least 5 impedance points".into());
    }
    if freqs.windows(2).any(|w| w[1].is_nan() || w[1] <= w[0]) || freqs[0] <= 0.0 {
        return Err("frequencies must be positive and strictly increasing".into());
    }
    let re = match re_dc {
        Some(r) if r > 0.0 => r,
        Some(r) => return Err(format!("Re must be positive, got {r}")),
        None => {
            let (f1, f2) = (freqs[0], freqs[1]);
            let (r1, r2) = (z[0].re, z[1].re);
            (r1 * f2 * f2 - r2 * f1 * f1) / (f2 * f2 - f1 * f1)
        }
    };
    if re.is_nan() || re <= 0.0 {
        return Err(format!(
            "low-frequency resistance estimate {re} is not positive"
        ));
    }
    let mag: Vec<f64> = z.iter().map(|v| v.norm()).collect();
    let u: Vec<f64> = freqs.iter().map(|f| f.ln()).collect();
    let i = (1..n - 1)
        .max_by(|&a, &b| mag[a].total_cmp(&mag[b]))
        .expect("n >= 5");
    if mag[i] < mag[0] || mag[i] < mag[n - 1] {
        return Err("no interior impedance maximum (resonance outside the sweep)".into());
    }
    let (u_pk, z_max) = parabola_vertex(
        (u[i - 1], mag[i - 1]),
        (u[i], mag[i]),
        (u[i + 1], mag[i + 1]),
    );
    let fs = u_pk.exp();
    let r0 = z_max / re;
    if r0.is_nan() || r0 <= 1.0 {
        return Err(format!(
            "impedance maximum {z_max:.6} does not exceed Re {re:.6}"
        ));
    }
    let level = re * r0.sqrt();
    // Crossing below the peak.
    let mut j = i;
    while j > 0 && mag[j] > level {
        j -= 1;
    }
    if mag[j] > level {
        return Err("|Z| never falls to Re*sqrt(r0) below the peak; extend the sweep down".into());
    }
    let f1 = cross(u[j], mag[j], u[j + 1], mag[j + 1], level).exp();
    let mut k = i;
    while k < n - 1 && mag[k] > level {
        k += 1;
    }
    if mag[k] > level {
        return Err("|Z| never falls to Re*sqrt(r0) above the peak; extend the sweep up".into());
    }
    let f2 = cross(u[k - 1], mag[k - 1], u[k], mag[k], level).exp();
    let qms = fs * r0.sqrt() / (f2 - f1);
    let qes = qms / (r0 - 1.0);
    let qts = qms / r0;
    let mut warnings = Vec::new();
    if qms / qes < MIN_Q_RATIO {
        warnings.push(format!(
            "Qms/Qes = {:.4} is below {MIN_Q_RATIO}: the impedance hump is too small to identify parameters electrically (spec Section 5)",
            qms / qes
        ));
    }
    let f_geo = (f1 * f2).sqrt();
    if (f_geo / fs - 1.0).abs() > 0.01 {
        warnings.push(format!(
            "peak at {fs:.4} Hz but sqrt(f1*f2) = {f_geo:.4} Hz: the curve is not a pure Thiele-Small resonance (inductance, creep or acoustic load); use fit_impedance"
        ));
    }
    if re_dc.is_none() && freqs[0] > fs / 3.0 {
        warnings.push(format!(
            "sweep starts at {:.4} Hz, above fs/3; the extrapolated Re is unreliable",
            freqs[0]
        ));
    }
    Ok(TsEstimate {
        re,
        fs,
        z_max,
        r0,
        f1,
        f2,
        qms,
        qes,
        qts,
        warnings,
    })
}

/// Vertex of the parabola through three points.
fn parabola_vertex((x0, y0): (f64, f64), (x1, y1): (f64, f64), (x2, y2): (f64, f64)) -> (f64, f64) {
    let d01 = (y1 - y0) / (x1 - x0);
    let d12 = (y2 - y1) / (x2 - x1);
    let a = (d12 - d01) / (x2 - x0);
    if a >= 0.0 {
        return (x1, y1);
    }
    let b = d01 - a * (x0 + x1);
    let c = y0 - a * x0 * x0 - b * x0;
    let xv = -b / (2.0 * a);
    (xv, c - b * b / (4.0 * a))
}

/// Linear interpolation of the abscissa where y crosses `level`.
fn cross(x0: f64, y0: f64, x1: f64, y1: f64, level: f64) -> f64 {
    x0 + (level - y0) / (y1 - y0) * (x1 - x0)
}

// ----- Levenberg–Marquardt fit ------------------------------------------------

/// Which parameters of [`ImpedanceModel`] are free in [`fit_impedance`].
/// Re, fs, Qms and Qes are always free.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitOptions {
    pub creep: bool,
    pub le: bool,
    pub lr2: bool,
    pub max_iterations: usize,
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions {
            creep: true,
            le: true,
            lr2: true,
            max_iterations: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FitResult {
    pub model: ImpedanceModel,
    /// RMS of the relative complex error |Z_model − Z|/|Z| over the points.
    pub rms_relative_error: f64,
    pub iterations: usize,
    pub converged: bool,
}

/// Starting point for [`fit_impedance`]: the sqrt(R0) estimate plus rough
/// creep and inductance guesses from the ends of the curve.
pub fn initial_model(
    freqs: &[f64],
    z: &[C64],
    opts: &FitOptions,
) -> Result<ImpedanceModel, String> {
    let est = extract_ts(freqs, z, None)?;
    let mut m = ImpedanceModel {
        re: est.re,
        fs: est.fs,
        qms: est.qms,
        qes: est.qes,
        creep_lambda: 0.0,
        le: 0.0,
        l2: 0.0,
        r2: 0.0,
    };
    if opts.creep {
        m.creep_lambda = 0.01;
    }
    let (fh, zh) = (freqs[freqs.len() - 1], z[z.len() - 1]);
    let wh = 2.0 * PI * fh;
    // Excess reactance at the top of the sweep, split between Le and L2.
    let l_total = (zh.im / wh).max(1e-9);
    if opts.le {
        m.le = if opts.lr2 { 0.5 * l_total } else { l_total };
    }
    if opts.lr2 {
        m.l2 = 0.5 * l_total;
        m.r2 = (zh.re - est.re).max(0.1 * est.re);
    }
    Ok(m)
}

/// Fits [`ImpedanceModel`] to a complex impedance curve by Levenberg–
/// Marquardt in log-parameter space (every free parameter stays positive;
/// spec Section 3, "Fitting and optimisation"), using the optimiser of
/// [`crate::fit::lm`]. Residuals are the real and imaginary parts of
/// (Z_model − Z)/|Z|; the Jacobian is by central differences in the log
/// parameters.
pub fn fit_impedance(
    freqs: &[f64],
    z: &[C64],
    initial: ImpedanceModel,
    opts: &FitOptions,
) -> Result<FitResult, String> {
    use crate::fit::lm::{self, Bound, FnProblem, LmOptions};
    if freqs.len() != z.len() || freqs.is_empty() {
        return Err("frequency and impedance arrays must be non-empty and equal in length".into());
    }
    let free = free_indices(opts);
    let p0 = to_vec(&initial);
    for &k in &free {
        if p0[k].is_nan() || p0[k] <= 0.0 {
            return Err(format!(
                "initial value of free parameter {} must be positive",
                PARAM_NAMES[k]
            ));
        }
    }
    let theta: Vec<f64> = free.iter().map(|&k| p0[k].ln()).collect();
    let with = |theta: &[f64]| -> [f64; 8] {
        let mut p = p0;
        for (t, &k) in theta.iter().zip(&free) {
            p[k] = t.exp();
        }
        p
    };
    let mut problem = FnProblem {
        f: |theta: &[f64]| -> Result<Vec<f64>, String> {
            let m = from_vec(&with(theta));
            let mut r = Vec::with_capacity(2 * z.len());
            for (f, zd) in freqs.iter().zip(z) {
                let e = (m.impedance(*f) - zd) / zd.norm();
                r.push(e.re);
                r.push(e.im);
            }
            Ok(r)
        },
        steps: vec![1e-6; free.len()],
    };
    let o = LmOptions {
        max_iterations: opts.max_iterations,
        max_evaluations: usize::MAX,
        ftol: 1e-14,
        xtol: 1e-12,
        cost_floor: 1e-28,
        null_tolerance: 0.0,
        min_sigma: 0.0,
        stall: (0, 0.0),
        mu0: 1e-3,
    };
    let r = lm::minimize(&mut problem, &theta, &vec![Bound::Free; free.len()], &o)?;
    Ok(FitResult {
        model: from_vec(&with(&r.u)),
        rms_relative_error: (r.cost / freqs.len() as f64).sqrt(),
        iterations: r.iterations,
        converged: r.stop.converged(),
    })
}

const PARAM_NAMES: [&str; 8] = ["Re", "fs", "Qms", "Qes", "creep_lambda", "Le", "L2", "R2"];

fn free_indices(o: &FitOptions) -> Vec<usize> {
    let mut v = vec![0, 1, 2, 3];
    if o.creep {
        v.push(4);
    }
    if o.le {
        v.push(5);
    }
    if o.lr2 {
        v.extend([6, 7]);
    }
    v
}

fn to_vec(m: &ImpedanceModel) -> [f64; 8] {
    [m.re, m.fs, m.qms, m.qes, m.creep_lambda, m.le, m.l2, m.r2]
}

fn from_vec(p: &[f64; 8]) -> ImpedanceModel {
    ImpedanceModel {
        re: p[0],
        fs: p[1],
        qms: p[2],
        qes: p[3],
        creep_lambda: p[4],
        le: p[5],
        l2: p[6],
        r2: p[7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_round_trip() {
        let ts = TsParams {
            fs: 81.8,
            qms: 2.71,
            qes: 1.01,
            re: 32.8,
            mms: 0.3e-3,
            sd: 10e-4,
        };
        let p = ts.to_physical();
        let back = p.to_ts();
        for (a, b) in [
            (ts.fs, back.fs),
            (ts.qms, back.qms),
            (ts.qes, back.qes),
            (ts.re, back.re),
            (ts.mms, back.mms),
        ] {
            assert!((a / b - 1.0).abs() < 1e-14);
        }
        // Erratum E5: the primary set derives Bl = 2.24 T·m.
        assert!((p.bl - 2.2377).abs() < 5e-4, "{}", p.bl);
    }

    #[test]
    fn creep_factor_matches_definition() {
        let c = Creep {
            lambda: 0.1,
            f0: 100.0,
        };
        // At f0 the real part is 1; one decade below it is 1 + λ.
        let w0 = 2.0 * PI * 100.0;
        assert!((c.factor(w0).re - 1.0).abs() < 1e-15);
        assert!((c.factor(w0 / 10.0).re - 1.1).abs() < 1e-14);
        assert!((c.factor(w0).im + 0.1 * 0.682_188_176_920_920_6).abs() < 1e-14);
        assert!(Creep {
            lambda: -0.01,
            f0: 100.0
        }
        .validate()
        .is_err());
        let max = Creep::max_lambda(100.0);
        assert!(Creep {
            lambda: 0.99 * max,
            f0: 100.0
        }
        .validate()
        .is_ok());
        assert!(Creep {
            lambda: max,
            f0: 100.0
        }
        .validate()
        .is_err());
    }
}
