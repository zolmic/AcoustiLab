//! Rational approximation by vector fitting (Gustavsen and Semlyen 1999),
//! in its relaxed form with the fast QR reduction for several responses
//! sharing one set of poles.
//!
//! Model, for each response m (s = jω, rad/s):
//!
//!   H_m(s) = Σ_n c_{m,n}/(s − a_n) + d_m + s·e_m
//!
//! with real poles and complex-conjugate pairs; a pair's conjugate residue
//! is implied, so every H_m is real in the time domain.
//!
//! One pole-relocation step (Gustavsen 2006, "Improving the pole relocating
//! properties of vector fitting", IEEE Trans. Power Delivery 21(3)) solves,
//! in the least-squares sense over the samples s_k,
//!
//!   Σ_n c_n/(s_k − a_n) + d + s_k·e − H(s_k)·σ(s_k) = 0,
//!   σ(s) = Σ_n c̃_n/(s − a_n) + d̃,
//!
//! with the relaxation Re Σ_k σ(s_k) = K replacing the original σ(∞) = 1.
//! The zeros of σ, the eigenvalues of Λ − b·c̃ᵀ/d̃, are the new poles.
//! Unstable ones are reflected into the left half-plane. Complex pairs use
//! the real basis 1/(s − a) + 1/(s − ā) and j/(s − a) − j/(s − ā), so
//! every unknown is real and Λ is block-diagonal with blocks
//! [[a′, a″], [−a″, a′]] and b = [2, 0]ᵀ per pair (Gustavsen's `vectfit3`).
//! For several responses, each response's rows are QR-reduced separately
//! and only the rows of R that couple to σ are stacked (Deschrijver et al.,
//! "Macromodeling of multiport systems using a fast implementation of the
//! vector fitting method", IEEE MWCL 18(6), 2008).
//!
//! After the last relocation the residues, d and e are identified by
//! linear least squares with the poles fixed. Samples are weighted by
//! 1/|H| by default, so the fit error is relative (a dB-and-degree
//! criterion) across the dynamic range of an acoustic response.
//!
//! Frequencies are normalised by the highest fitted angular frequency
//! internally; poles and residues are returned in rad/s.
//!
//! Passivity is not enforced (a fitted impedance can have small negative
//! real parts outside the fitted band); that is outside this module.

use super::dense::{eigenvalues, lstsq, Qr};
use crate::C64;
use serde::Serialize;
use std::f64::consts::PI;

/// A pole of a real rational model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pole {
    Real(f64),
    /// The member of a complex-conjugate pair with positive imaginary part.
    Pair(C64),
}

impl Pole {
    pub fn value(&self) -> C64 {
        match *self {
            Pole::Real(a) => C64::new(a, 0.0),
            Pole::Pair(a) => a,
        }
    }

    /// Number of real unknowns (and states) it contributes.
    pub fn width(&self) -> usize {
        match self {
            Pole::Real(_) => 1,
            Pole::Pair(_) => 2,
        }
    }

    /// Natural frequency |a|/(2π), Hz.
    pub fn f_hz(&self) -> f64 {
        self.value().norm() / (2.0 * PI)
    }

    /// Quality factor |a|/(−2·Re a) of a pair; `None` for a real pole.
    pub fn q(&self) -> Option<f64> {
        match *self {
            Pole::Real(_) => None,
            Pole::Pair(a) => Some(a.norm() / (-2.0 * a.re)),
        }
    }

    fn scaled(&self, k: f64) -> Pole {
        match *self {
            Pole::Real(a) => Pole::Real(a * k),
            Pole::Pair(a) => Pole::Pair(a * k),
        }
    }
}

/// Form of the model at high frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Asymptote {
    /// Strictly proper: H → 0.
    Zero,
    /// A constant term d.
    Constant,
    /// d + s·e (e.g. an impedance with a series inductance).
    Linear,
}

impl Asymptote {
    fn terms(self) -> usize {
        match self {
            Asymptote::Zero => 0,
            Asymptote::Constant => 1,
            Asymptote::Linear => 2,
        }
    }

    pub fn parse(s: &str) -> Option<Asymptote> {
        match s {
            "zero" => Some(Asymptote::Zero),
            "constant" => Some(Asymptote::Constant),
            "linear" => Some(Asymptote::Linear),
            _ => None,
        }
    }
}

/// Sample weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Weighting {
    /// 1/|H(s_k)|: relative error (default).
    Relative,
    /// Unit weights: absolute error.
    Uniform,
}

#[derive(Debug, Clone)]
pub struct VfOptions {
    /// Number of poles (states), counting each pair as two.
    pub order: usize,
    /// Pole-relocation iterations.
    pub iterations: usize,
    pub asymptote: Asymptote,
    pub weighting: Weighting,
    /// Starting poles (rad/s); default: pairs log-spaced over the band with
    /// Re a = −Im a/100 (Gustavsen and Semlyen 1999, Section 5).
    pub initial_poles: Option<Vec<Pole>>,
}

impl Default for VfOptions {
    fn default() -> Self {
        VfOptions {
            order: 30,
            iterations: 12,
            asymptote: Asymptote::Constant,
            weighting: Weighting::Relative,
            initial_poles: None,
        }
    }
}

/// A fitted rational model with common poles.
#[derive(Debug, Clone)]
pub struct RationalModel {
    pub poles: Vec<Pole>,
    /// residues[m][n]: residue of response m at pole n (for a pair, of the
    /// member with positive imaginary part; real poles have real residues).
    pub residues: Vec<Vec<C64>>,
    pub d: Vec<f64>,
    pub e: Vec<f64>,
}

/// Fit quality over the fitted samples.
#[derive(Debug, Clone, Serialize)]
pub struct FitError {
    /// RMS of |H_fit − H|/|H| over all samples and responses.
    pub rms_relative: f64,
    /// Largest |20·log10|H_fit/H||, dB.
    #[serde(rename = "max_dB")]
    pub max_db: f64,
    /// Largest |arg(H_fit/H)|, degrees.
    pub max_deg: f64,
    /// RMS of 20·log10|H_fit/H|, dB.
    #[serde(rename = "rms_dB")]
    pub rms_db: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FitReport {
    pub order: usize,
    pub iterations: usize,
    /// Error after each relocation iteration (weighted RMS), for monitoring.
    pub history: Vec<f64>,
    pub error: FitError,
    /// Poles flipped from the right half-plane during relocation.
    pub flipped: usize,
    /// Why relocation stopped early (the best model so far is kept).
    pub stopped: Option<String>,
}

impl RationalModel {
    /// Value of response `m` at complex frequency `s` (rad/s).
    pub fn eval(&self, m: usize, s: C64) -> C64 {
        let mut h = C64::new(self.d[m], 0.0) + s * self.e[m];
        for (p, c) in self.poles.iter().zip(&self.residues[m]) {
            h += pole_term(*p, *c, s);
        }
        h
    }

    /// dH_m/ds at `s`.
    pub fn derivative(&self, m: usize, s: C64) -> C64 {
        let mut h = C64::new(self.e[m], 0.0);
        for (p, c) in self.poles.iter().zip(&self.residues[m]) {
            match *p {
                Pole::Real(a) => h -= c.re / (s - a).powi(2),
                Pole::Pair(a) => h -= c / (s - a).powi(2) + c.conj() / (s - a.conj()).powi(2),
            }
        }
        h
    }

    /// Group delay −dφ/dω = −Re(H′(jω)/H(jω)) of response `m` at `f` Hz,
    /// in seconds (exact for the rational model).
    pub fn group_delay(&self, m: usize, f_hz: f64) -> f64 {
        let s = C64::new(0.0, 2.0 * PI * f_hz);
        -(self.derivative(m, s) / self.eval(m, s)).re
    }

    /// Number of states (poles counted with their conjugates).
    pub fn order(&self) -> usize {
        self.poles.iter().map(Pole::width).sum()
    }

    /// Real state-space realisation (A, B, C, D, E) of response `m`, with
    /// H(s) = C(sI − A)⁻¹B + D + sE. A is block-diagonal: [a] for a real
    /// pole, [[a′, a″], [−a″, a′]] for a pair; B holds 1 or [2, 0]; C the
    /// residue (c′, c″).
    pub fn state_space(&self, m: usize) -> StateSpace {
        let n = self.order();
        let mut a = vec![vec![0.0; n]; n];
        let mut b = vec![0.0; n];
        let mut c = vec![0.0; n];
        let mut i = 0;
        for (p, r) in self.poles.iter().zip(&self.residues[m]) {
            match *p {
                Pole::Real(x) => {
                    a[i][i] = x;
                    b[i] = 1.0;
                    c[i] = r.re;
                }
                Pole::Pair(x) => {
                    a[i][i] = x.re;
                    a[i][i + 1] = x.im;
                    a[i + 1][i] = -x.im;
                    a[i + 1][i + 1] = x.re;
                    b[i] = 2.0;
                    c[i] = r.re;
                    c[i + 1] = r.im;
                }
            }
            i += p.width();
        }
        StateSpace {
            a,
            b,
            c,
            d: self.d[m],
            e: self.e[m],
        }
    }

    /// Finite zeros of response `m`.
    ///
    /// With D ≠ 0 they are the eigenvalues of A − B·C/D. For a strictly
    /// proper model (D = 0, CB ≠ 0) the output-zeroing input is
    /// u = −(CB)⁻¹CAx, so the zeros are the eigenvalues of
    /// (I − B(CB)⁻¹C)·A restricted to ker C, computed on an orthonormal
    /// basis of ker C. A model that is only nearly strictly proper returns
    /// one very large zero in addition; callers classify zeros by
    /// frequency. D counts as zero below 1e-6 of the model's peak |H| on the
    /// fitted samples `peak`: a smaller D only adds a zero near −CB/D, far
    /// outside the band, and makes A − B·C/D so large (‖BC/D‖ ≫ ‖A‖) that
    /// its other eigenvalues lose accuracy.
    pub fn zeros(&self, m: usize, peak: f64) -> Result<Vec<C64>, String> {
        let ss = self.state_space(m);
        let n = ss.b.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let flat = |mat: &Vec<Vec<f64>>| mat.iter().flatten().copied().collect::<Vec<f64>>();
        if ss.d.abs() > 1e-6 * peak {
            let mut z = flat(&ss.a);
            for i in 0..n {
                for j in 0..n {
                    z[i * n + j] -= ss.b[i] * ss.c[j] / ss.d;
                }
            }
            return eigenvalues(z, n);
        }
        let cb: f64 = ss.c.iter().zip(&ss.b).map(|(c, b)| c * b).sum();
        if cb == 0.0 || n == 1 {
            return Ok(Vec::new());
        }
        // N = I − B·C/(CB); M = N·A.
        let mut nm = flat(&ss.a);
        let ca: Vec<f64> = (0..n)
            .map(|j| (0..n).map(|i| ss.c[i] * ss.a[i][j]).sum())
            .collect();
        for i in 0..n {
            for j in 0..n {
                nm[i * n + j] -= ss.b[i] * ca[j] / cb;
            }
        }
        // Orthonormal basis of ker C: columns 2..n of the Householder
        // reflector H = I − 2vvᵀ/vᵀv that maps Cᵀ onto a multiple of e1.
        let norm = ss.c.iter().map(|x| x * x).sum::<f64>().sqrt();
        let mut v = ss.c.clone();
        v[0] += if v[0] >= 0.0 { norm } else { -norm };
        let vv: f64 = v.iter().map(|x| x * x).sum();
        let hcol = |j: usize| -> Vec<f64> {
            (0..n)
                .map(|i| f64::from(u8::from(i == j)) - 2.0 * v[i] * v[j] / vv)
                .collect()
        };
        let q: Vec<Vec<f64>> = (1..n).map(hcol).collect();
        let k = n - 1;
        let mut z = vec![0.0; k * k];
        for (r, qr) in q.iter().enumerate() {
            for (c, qc) in q.iter().enumerate() {
                let mut s = 0.0;
                for i in 0..n {
                    if qr[i] == 0.0 {
                        continue;
                    }
                    let mq: f64 = (0..n).map(|j| nm[i * n + j] * qc[j]).sum();
                    s += qr[i] * mq;
                }
                z[r * k + c] = s;
            }
        }
        eigenvalues(z, k)
    }
}

/// Real state-space realisation of one response.
#[derive(Debug, Clone, Serialize)]
pub struct StateSpace {
    #[serde(rename = "A")]
    pub a: Vec<Vec<f64>>,
    #[serde(rename = "B")]
    pub b: Vec<f64>,
    #[serde(rename = "C")]
    pub c: Vec<f64>,
    #[serde(rename = "D")]
    pub d: f64,
    #[serde(rename = "E")]
    pub e: f64,
}

/// Contribution of one pole (with its conjugate for a pair) at `s`.
pub fn pole_term(p: Pole, c: C64, s: C64) -> C64 {
    match p {
        Pole::Real(a) => c.re / (s - a),
        Pole::Pair(a) => c / (s - a) + c.conj() / (s - a.conj()),
    }
}

/// Default starting poles over [w_min, w_max] (rad/s): `order/2` pairs with
/// log-spaced imaginary parts and Re a = −Im a/100, plus one real pole at
/// −sqrt(w_min·w_max) when the order is odd.
pub fn initial_poles(order: usize, w_min: f64, w_max: f64) -> Vec<Pole> {
    let pairs = order / 2;
    let mut v: Vec<Pole> = (0..pairs)
        .map(|i| {
            let t = if pairs > 1 {
                i as f64 / (pairs - 1) as f64
            } else {
                0.5
            };
            let b = w_min * (w_max / w_min).powf(t);
            Pole::Pair(C64::new(-b / 100.0, b))
        })
        .collect();
    if order % 2 == 1 {
        v.push(Pole::Real(-(w_min * w_max).sqrt()));
    }
    v
}

/// Basis values of every pole at `s` (real unknowns, see module docs).
fn basis(poles: &[Pole], s: C64, out: &mut Vec<C64>) {
    out.clear();
    for p in poles {
        match *p {
            Pole::Real(a) => out.push((s - a).inv()),
            Pole::Pair(a) => {
                let (u, v) = ((s - a).inv(), (s - a.conj()).inv());
                out.push(u + v);
                out.push(C64::new(0.0, 1.0) * (u - v));
            }
        }
    }
}

struct Problem<'a> {
    /// Normalised s_k = jω_k/ω_ref.
    s: Vec<C64>,
    h: &'a [Vec<C64>],
    w: Vec<Vec<f64>>,
    asym: Asymptote,
}

impl Problem<'_> {
    /// Residues, d and e for fixed (normalised) poles; returns the model
    /// and the weighted RMS error.
    fn residues(&self, poles: &[Pole]) -> (RationalModel, f64) {
        let k = self.s.len();
        let nb: usize = poles.iter().map(Pole::width).sum();
        let nt = self.asym.terms();
        let cols = nb + nt;
        let mut model = RationalModel {
            poles: poles.to_vec(),
            residues: Vec::new(),
            d: Vec::new(),
            e: Vec::new(),
        };
        let mut phi = Vec::with_capacity(nb);
        let mut sq = 0.0;
        let mut count = 0usize;
        for (hm, wm) in self.h.iter().zip(&self.w) {
            let mut a = vec![0.0; 2 * k * cols];
            let mut b = vec![0.0; 2 * k];
            for (i, &s) in self.s.iter().enumerate() {
                basis(poles, s, &mut phi);
                let w = wm[i];
                let (re, im) = (i, k + i);
                for (j, v) in phi.iter().enumerate() {
                    a[re * cols + j] = w * v.re;
                    a[im * cols + j] = w * v.im;
                }
                if nt >= 1 {
                    a[re * cols + nb] = w;
                }
                if nt == 2 {
                    a[re * cols + nb + 1] = w * s.re;
                    a[im * cols + nb + 1] = w * s.im;
                }
                b[re] = w * hm[i].re;
                b[im] = w * hm[i].im;
            }
            let x = lstsq(2 * k, cols, a, &b);
            let mut res = Vec::with_capacity(poles.len());
            let mut j = 0;
            for p in poles {
                match p {
                    Pole::Real(_) => res.push(C64::new(x[j], 0.0)),
                    Pole::Pair(_) => res.push(C64::new(x[j], x[j + 1])),
                }
                j += p.width();
            }
            model.residues.push(res);
            model.d.push(if nt >= 1 { x[nb] } else { 0.0 });
            model.e.push(if nt == 2 { x[nb + 1] } else { 0.0 });
            let m = model.residues.len() - 1;
            for (i, &s) in self.s.iter().enumerate() {
                sq += ((model.eval(m, s) - hm[i]) * wm[i]).norm_sqr();
                count += 1;
            }
        }
        (model, (sq / count as f64).sqrt())
    }

    /// One relaxed pole relocation; returns the new (normalised) poles and
    /// the number of poles reflected into the left half-plane.
    fn relocate(&self, poles: &[Pole]) -> Result<(Vec<Pole>, usize), String> {
        let k = self.s.len();
        let nb: usize = poles.iter().map(Pole::width).sum();
        let nt = self.asym.terms();
        let n1 = nb + nt; // columns of the model part
        let n2 = nb + 1; // columns of σ (c̃, d̃)
        let cols = n1 + n2;
        let nresp = self.h.len();
        // Relaxation-row scale (vectfit3): ‖w·H‖ over all responses / K.
        let scale = self
            .h
            .iter()
            .zip(&self.w)
            .map(|(h, w)| {
                h.iter()
                    .zip(w)
                    .map(|(h, w)| (h * w).norm_sqr())
                    .sum::<f64>()
            })
            .sum::<f64>()
            .sqrt()
            / k as f64;
        let mut phi = Vec::with_capacity(nb);
        let mut sum_phi = vec![0.0; nb];
        for &s in &self.s {
            basis(poles, s, &mut phi);
            for (acc, v) in sum_phi.iter_mut().zip(&phi) {
                *acc += v.re;
            }
        }
        let mut aa = Vec::with_capacity(nresp * n2 * n2);
        let mut bb = Vec::with_capacity(nresp * n2);
        for (m, (hm, wm)) in self.h.iter().zip(&self.w).enumerate() {
            let last = m + 1 == nresp;
            let rows = 2 * k + usize::from(last);
            let mut a = vec![0.0; rows * cols];
            let mut rhs = vec![0.0; rows];
            for (i, &s) in self.s.iter().enumerate() {
                basis(poles, s, &mut phi);
                let w = wm[i];
                let (re, im) = (i, k + i);
                for (j, v) in phi.iter().enumerate() {
                    let wv = *v * w;
                    a[re * cols + j] = wv.re;
                    a[im * cols + j] = wv.im;
                    let t = -wv * hm[i];
                    a[re * cols + n1 + j] = t.re;
                    a[im * cols + n1 + j] = t.im;
                }
                if nt >= 1 {
                    a[re * cols + nb] = w;
                }
                if nt == 2 {
                    a[re * cols + nb + 1] = w * s.re;
                    a[im * cols + nb + 1] = w * s.im;
                }
                let t = -hm[i] * w;
                a[re * cols + n1 + nb] = t.re;
                a[im * cols + n1 + nb] = t.im;
            }
            if last {
                let r = 2 * k;
                for j in 0..nb {
                    a[r * cols + n1 + j] = scale * sum_phi[j];
                }
                a[r * cols + n1 + nb] = scale * k as f64;
                rhs[r] = scale * k as f64;
            }
            if rows < cols {
                return Err(format!(
                    "vector fitting: {k} samples are too few for order {nb}"
                ));
            }
            let qr = Qr::factor(rows, cols, a, &mut [&mut rhs]);
            for (i, r) in rhs.iter().enumerate().take(cols).skip(n1) {
                for j in n1..cols {
                    aa.push(qr.r(i, j));
                }
                bb.push(*r);
            }
        }
        let x = lstsq(nresp * n2, n2, aa, &bb);
        let mut ct = x[..nb].to_vec();
        let mut dt = x[nb];
        // Relaxation guard (vectfit3): a vanishing d̃ makes Λ − b·c̃ᵀ/d̃
        // meaningless; refit with d̃ held at a small value instead.
        const TOL: f64 = 1e-8;
        if dt.abs() < TOL {
            dt = if dt < 0.0 { -TOL } else { TOL };
            ct = self.fixed_dt(poles, dt)?;
        }
        // Λ − b·c̃ᵀ/d̃ in the real block form.
        let mut lam = vec![0.0; nb * nb];
        let mut bvec = vec![0.0; nb];
        let mut i = 0;
        for p in poles {
            match *p {
                Pole::Real(a) => {
                    lam[i * nb + i] = a;
                    bvec[i] = 1.0;
                }
                Pole::Pair(a) => {
                    lam[i * nb + i] = a.re;
                    lam[i * nb + i + 1] = a.im;
                    lam[(i + 1) * nb + i] = -a.im;
                    lam[(i + 1) * nb + i + 1] = a.re;
                    bvec[i] = 2.0;
                }
            }
            i += p.width();
        }
        for r in 0..nb {
            for c in 0..nb {
                lam[r * nb + c] -= bvec[r] * ct[c] / dt;
            }
        }
        let z = eigenvalues(lam, nb)?;
        Ok(classify(&z))
    }

    /// σ residues with d̃ held fixed (non-relaxed step).
    fn fixed_dt(&self, poles: &[Pole], dt: f64) -> Result<Vec<f64>, String> {
        let k = self.s.len();
        let nb: usize = poles.iter().map(Pole::width).sum();
        let nt = self.asym.terms();
        let n1 = nb + nt;
        let cols = n1 + nb;
        let mut aa = Vec::new();
        let mut bb = Vec::new();
        let mut phi = Vec::with_capacity(nb);
        for (hm, wm) in self.h.iter().zip(&self.w) {
            let rows = 2 * k;
            let mut a = vec![0.0; rows * cols];
            let mut rhs = vec![0.0; rows];
            for (i, &s) in self.s.iter().enumerate() {
                basis(poles, s, &mut phi);
                let w = wm[i];
                let (re, im) = (i, k + i);
                for (j, v) in phi.iter().enumerate() {
                    let wv = *v * w;
                    a[re * cols + j] = wv.re;
                    a[im * cols + j] = wv.im;
                    let t = -wv * hm[i];
                    a[re * cols + n1 + j] = t.re;
                    a[im * cols + n1 + j] = t.im;
                }
                if nt >= 1 {
                    a[re * cols + nb] = w;
                }
                if nt == 2 {
                    a[re * cols + nb + 1] = w * s.re;
                    a[im * cols + nb + 1] = w * s.im;
                }
                let t = hm[i] * w * dt;
                rhs[re] = t.re;
                rhs[im] = t.im;
            }
            let qr = Qr::factor(rows, cols, a, &mut [&mut rhs]);
            for (i, r) in rhs.iter().enumerate().take(cols).skip(n1) {
                for j in n1..cols {
                    aa.push(qr.r(i, j));
                }
                bb.push(*r);
            }
        }
        Ok(lstsq(self.h.len() * nb, nb, aa, &bb))
    }
}

/// Sorts eigenvalues into real poles and pairs, reflecting unstable ones.
fn classify(z: &[C64]) -> (Vec<Pole>, usize) {
    let mut flipped = 0;
    let mut out = Vec::new();
    for &v in z {
        let mut v = v;
        if v.re > 0.0 {
            v.re = -v.re;
            flipped += 1;
        }
        if v.re == 0.0 {
            // A pole on the axis would make a basis function singular.
            v.re = -1e-12 * v.norm().max(1e-300);
        }
        if v.im == 0.0 {
            out.push(Pole::Real(v.re));
        } else if v.im > 0.0 {
            out.push(Pole::Pair(v));
        }
    }
    out.sort_by(|a, b| a.value().norm().total_cmp(&b.value().norm()));
    (out, flipped)
}

/// Fits `responses` (each sampled at `freqs_hz`) with common poles.
pub fn vector_fit(
    freqs_hz: &[f64],
    responses: &[Vec<C64>],
    opts: &VfOptions,
) -> Result<(RationalModel, FitReport), String> {
    if responses.is_empty() {
        return Err("vector fitting: no response to fit".into());
    }
    let k = freqs_hz.len();
    if responses.iter().any(|r| r.len() != k) {
        return Err("vector fitting: every response needs one value per frequency".into());
    }
    if freqs_hz.iter().any(|f| !(f.is_finite() && *f > 0.0)) {
        return Err("vector fitting: frequencies must be positive".into());
    }
    if responses.iter().flatten().any(|h| !h.is_finite()) {
        return Err("vector fitting: the response has non-finite values".into());
    }
    if opts.order == 0 {
        return Err("vector fitting: the order must be at least 1".into());
    }
    if 2 * k < 2 * opts.order + opts.asymptote.terms() + 1 {
        return Err(format!(
            "vector fitting: {k} samples are too few for order {}",
            opts.order
        ));
    }
    let w_max = freqs_hz.iter().fold(0.0f64, |m, f| m.max(*f)) * 2.0 * PI;
    let w_min = freqs_hz.iter().fold(f64::INFINITY, |m, f| m.min(*f)) * 2.0 * PI;
    let s: Vec<C64> = freqs_hz
        .iter()
        .map(|f| C64::new(0.0, 2.0 * PI * f / w_max))
        .collect();
    let w: Vec<Vec<f64>> = responses
        .iter()
        .map(|h| {
            h.iter()
                .map(|v| match opts.weighting {
                    Weighting::Relative => {
                        let a = v.norm();
                        if a > 0.0 {
                            1.0 / a
                        } else {
                            0.0
                        }
                    }
                    Weighting::Uniform => 1.0,
                })
                .collect()
        })
        .collect();
    let prob = Problem {
        s,
        h: responses,
        w,
        asym: opts.asymptote,
    };
    let mut poles: Vec<Pole> = match &opts.initial_poles {
        Some(p) => p.iter().map(|p| p.scaled(1.0 / w_max)).collect(),
        None => initial_poles(opts.order, w_min / w_max, 1.0),
    };
    let order: usize = poles.iter().map(Pole::width).sum();
    if order != opts.order && opts.initial_poles.is_none() {
        return Err("vector fitting: internal order mismatch".into());
    }
    let mut history = Vec::new();
    let mut flipped = 0;
    let mut best: Option<(RationalModel, f64)> = None;
    let mut stopped = None;
    for _ in 0..opts.iterations {
        let (next, fl) = match prob.relocate(&poles) {
            Ok(r) => r,
            Err(e) => {
                stopped = Some(e);
                break;
            }
        };
        flipped += fl;
        poles = next;
        let (model, err) = prob.residues(&poles);
        history.push(err);
        if best.as_ref().is_none_or(|(_, e)| err < *e) {
            best = Some((model, err));
        }
    }
    let (mut model, _) = match best {
        Some(b) => b,
        None => prob.residues(&poles),
    };
    // Back to rad/s: c/(s − a) = (c̃·ω)/(s − ã·ω); e·s = (ẽ/ω)·s.
    model.poles = model.poles.iter().map(|p| p.scaled(w_max)).collect();
    for r in &mut model.residues {
        r.iter_mut().for_each(|c| *c *= w_max);
    }
    model.e.iter_mut().for_each(|e| *e /= w_max);
    let error = fit_error(&model, freqs_hz, responses);
    Ok((
        model,
        FitReport {
            order: opts.order,
            iterations: opts.iterations,
            history,
            error,
            flipped,
            stopped,
        },
    ))
}

/// Error of a model against sampled responses.
pub fn fit_error(model: &RationalModel, freqs_hz: &[f64], responses: &[Vec<C64>]) -> FitError {
    let (mut sq, mut sq_db, mut n) = (0.0, 0.0, 0usize);
    let (mut max_db, mut max_deg) = (0.0f64, 0.0f64);
    for (m, h) in responses.iter().enumerate() {
        for (&f, &v) in freqs_hz.iter().zip(h) {
            let fit = model.eval(m, C64::new(0.0, 2.0 * PI * f));
            if v.norm() == 0.0 {
                continue;
            }
            let ratio = fit / v;
            sq += (ratio - 1.0).norm_sqr();
            let db = 20.0 * ratio.norm().log10();
            sq_db += db * db;
            max_db = max_db.max(db.abs());
            max_deg = max_deg.max(ratio.arg().to_degrees().abs());
            n += 1;
        }
    }
    let n = n.max(1) as f64;
    FitError {
        rms_relative: (sq / n).sqrt(),
        max_db,
        max_deg,
        rms_db: (sq_db / n).sqrt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_a_rational_function() {
        let poles = vec![
            Pole::Real(-2.0 * PI * 30.0),
            Pole::Pair(C64::new(-2.0 * PI * 10.0, 2.0 * PI * 200.0)),
            Pole::Pair(C64::new(-2.0 * PI * 300.0, 2.0 * PI * 3000.0)),
        ];
        let truth = RationalModel {
            poles: poles.clone(),
            residues: vec![vec![
                C64::new(1e3, 0.0),
                C64::new(50.0, -300.0),
                C64::new(2e3, 1e3),
            ]],
            d: vec![0.5],
            e: vec![0.0],
        };
        let f = crate::grid::log_grid(10.0, 20_000.0, 24.0);
        let h: Vec<C64> = f
            .iter()
            .map(|&f| truth.eval(0, C64::new(0.0, 2.0 * PI * f)))
            .collect();
        let (m, rep) = vector_fit(
            &f,
            &[h],
            &VfOptions {
                order: 5,
                iterations: 8,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rep.error.rms_relative < 1e-12, "{:?}", rep.error);
        for (p, q) in m.poles.iter().zip(&poles) {
            assert!((p.value() - q.value()).norm() < 1e-8 * q.value().norm());
        }
    }
}
