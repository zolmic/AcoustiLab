//! Levenberg–Marquardt least squares with smooth box bounds.
//!
//! Minimises the cost Σ r_i(u)² over the fitting variables u (the natural
//! logarithms of positive parameters, or scaled parameters on a linear
//! scale). The iterate lives in an unconstrained variable z with u = T(z)
//! ([`Bound`]), so every trial point satisfies the bounds:
//!
//! * two bounds a < u < b: u = a + (b − a)·σ(z), σ the logistic function;
//! * a lower bound only: u = a + ln(1 + e^z);
//! * an upper bound only: u = b − ln(1 + e^(−z)).
//!
//! Each iteration takes the Jacobian J = ∂r/∂u, its singular value
//! decomposition J = U·Σ·Vᵀ, and solves the Marquardt-damped normal
//! equations (JᵀJ + μ·D)·δu = −Jᵀr, D = diag(JᵀJ), within the span of the
//! right singular vectors whose singular value exceeds
//! [`LmOptions::null_tolerance`] times the largest and the absolute floor
//! [`LmOptions::min_sigma`]: with δu = V_r·y,
//! (Σ_r² + μ·V_rᵀ·D·V_r)·y = −Σ_r·U_rᵀ·r. Directions the data do not
//! constrain (a numerically null or statistically flat singular value)
//! are thus never stepped along, so parameters that only move along them
//! stay where they started instead of wandering on noise. With every
//! direction kept this is the ordinary Marquardt step.
//!
//! The step is carried to z to first order, δz_k = δu_k/T'(z_k), and the
//! trial point is T(z + δz): away from the bounds this is the step itself;
//! towards a bound it saturates smoothly instead of crossing it. This
//! rather than projecting onto the box: projection makes the cost
//! non-smooth at the faces and pins a variable there. Working in u rather
//! than z keeps the step and the flatness test independent of how close a
//! variable is to its bound (dT/dz vanishes there); a variable whose
//! optimum lies beyond a bound ends next to it, and the fit reports that.
//!
//! A trial point that lowers the cost is accepted and μ divided by 3;
//! otherwise μ is multiplied by 4 (by 10 when the damped system is
//! singular). A trial point where the model cannot be evaluated, such as a
//! singular network, counts as a rejected step.

use super::dense::{self, Mat};
use super::jacobian;
use serde::Serialize;

/// Bound of one fitting variable u (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bound {
    Free,
    Lower(f64),
    Upper(f64),
    /// Lower and upper bound, lower < upper.
    Both(f64, f64),
}

fn logistic(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

/// ln(1 + e^x) without overflow.
fn softplus(x: f64) -> f64 {
    if x > 30.0 {
        x + (-x).exp()
    } else {
        x.exp().ln_1p()
    }
}

/// Inverse of [`softplus`] for y > 0: ln(e^y − 1).
fn softplus_inv(y: f64) -> f64 {
    let y = y.max(1e-12);
    if y > 30.0 {
        y + (-(-y).exp()).ln_1p()
    } else {
        y.exp_m1().ln()
    }
}

impl Bound {
    /// From optional bounds; equal bounds are not a bound (fix the
    /// parameter instead).
    pub fn new(lo: Option<f64>, hi: Option<f64>) -> Bound {
        match (lo, hi) {
            (Some(a), Some(b)) => Bound::Both(a, b),
            (Some(a), None) => Bound::Lower(a),
            (None, Some(b)) => Bound::Upper(b),
            (None, None) => Bound::Free,
        }
    }

    /// u = T(z).
    pub fn u(&self, z: f64) -> f64 {
        match *self {
            Bound::Free => z,
            Bound::Lower(a) => a + softplus(z),
            Bound::Upper(b) => b - softplus(-z),
            Bound::Both(a, b) => a + (b - a) * logistic(z),
        }
    }

    /// du/dz at z.
    pub fn du_dz(&self, z: f64) -> f64 {
        match *self {
            Bound::Free => 1.0,
            Bound::Lower(_) => logistic(z),
            Bound::Upper(_) => logistic(-z),
            Bound::Both(a, b) => {
                let s = logistic(z);
                (b - a) * s * (1.0 - s)
            }
        }
    }

    /// z = T⁻¹(u); a u on or outside a bound maps to a point just inside.
    pub fn z(&self, u: f64) -> f64 {
        match *self {
            Bound::Free => u,
            Bound::Lower(a) => softplus_inv(u - a),
            Bound::Upper(b) => -softplus_inv(b - u),
            Bound::Both(a, b) => {
                let t = ((u - a) / (b - a)).clamp(1e-12, 1.0 - 1e-12);
                (t / (1.0 - t)).ln()
            }
        }
    }

    /// True if u satisfies the bounds.
    pub fn contains(&self, u: f64) -> bool {
        match *self {
            Bound::Free => true,
            Bound::Lower(a) => u >= a,
            Bound::Upper(b) => u <= b,
            Bound::Both(a, b) => u >= a && u <= b,
        }
    }

    /// True if u lies within `tol` of a bound (in u).
    pub fn near_bound(&self, u: f64, tol: f64) -> bool {
        match *self {
            Bound::Free => false,
            Bound::Lower(a) => u - a <= tol,
            Bound::Upper(b) => b - u <= tol,
            Bound::Both(a, b) => u - a <= tol || b - u <= tol,
        }
    }
}

/// Why the iteration stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// An accepted step lowered the cost by less than `ftol` (relative).
    SmallReduction,
    /// Every component of an accepted step was below `xtol`.
    SmallStep,
    /// The cost fell below `cost_floor`.
    ExactFit,
    /// No damped step lowers the cost: a (local) minimum.
    NoFurtherReduction,
    MaxIterations,
    MaxEvaluations,
}

impl Stop {
    pub fn converged(self) -> bool {
        !matches!(self, Stop::MaxIterations | Stop::MaxEvaluations)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Stop::SmallReduction => {
                "converged: the last step lowered the cost by a negligible fraction"
            }
            Stop::SmallStep => "converged: the last step was negligible",
            Stop::ExactFit => "converged: the residuals vanish",
            Stop::NoFurtherReduction => {
                "converged: no step lowers the cost further (a local minimum)"
            }
            Stop::MaxIterations => "stopped at the iteration limit before converging",
            Stop::MaxEvaluations => "stopped at the evaluation limit before converging",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LmOptions {
    pub max_iterations: usize,
    /// Cap on residual evaluations, Jacobian columns included (each is a
    /// full model evaluation for a network fit).
    pub max_evaluations: usize,
    pub ftol: f64,
    pub xtol: f64,
    pub cost_floor: f64,
    /// Relative singular value below which a direction is left out of the
    /// step (see the module documentation). 0 keeps every direction.
    pub null_tolerance: f64,
    /// Directions whose singular value is at most this are also left out of
    /// the step. For residuals weighted by their standard
    /// uncertainties, σ < 1 means that moving one unit along the direction
    /// changes χ² by less than 1: the data do not determine it, and
    /// stepping along it only chases noise (the fit then leaves that
    /// combination at its starting value). 0 disables it.
    pub min_sigma: f64,
    /// Converged when the last `stall.0` accepted steps together lowered
    /// the cost by less than `stall.1` (absolute). For residuals weighted
    /// by their standard uncertainties the cost is χ², and a change far
    /// below 1 moves no parameter by a meaningful fraction of its standard
    /// deviation; this stops the slow crawl along nearly flat directions.
    /// `(0, 0.0)` disables it.
    pub stall: (usize, f64),
    /// Initial damping.
    pub mu0: f64,
}

impl Default for LmOptions {
    fn default() -> Self {
        LmOptions {
            max_iterations: 100,
            max_evaluations: 10_000,
            ftol: 1e-10,
            xtol: 1e-10,
            cost_floor: 0.0,
            null_tolerance: 1e-9,
            min_sigma: 0.0,
            stall: (0, 0.0),
            mu0: 1e-3,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LmResult {
    /// Final point in the fitting variables.
    pub u: Vec<f64>,
    pub residuals: Vec<f64>,
    /// Σ r².
    pub cost: f64,
    pub iterations: usize,
    pub evaluations: usize,
    /// Trial points at which the model could not be evaluated.
    pub failed_evaluations: usize,
    pub stop: Stop,
}

/// A least-squares problem in the fitting variables u.
pub trait Problem {
    /// Residuals at u, or why the model cannot be evaluated there.
    fn residuals(&mut self, u: &[f64]) -> Result<Vec<f64>, String>;

    /// ∂r/∂u at u (rows are residuals) and the number of residual
    /// evaluations it used. `r` holds the residuals at u.
    fn jacobian(&mut self, u: &[f64], r: &[f64], bounds: &[Bound]) -> Result<(Mat, usize), String>;
}

/// A problem given by a residual closure, differentiated by central
/// differences with fixed steps in u.
pub struct FnProblem<F> {
    pub f: F,
    pub steps: Vec<f64>,
}

impl<F: FnMut(&[f64]) -> Result<Vec<f64>, String>> Problem for FnProblem<F> {
    fn residuals(&mut self, u: &[f64]) -> Result<Vec<f64>, String> {
        (self.f)(u)
    }

    fn jacobian(&mut self, u: &[f64], r: &[f64], bounds: &[Bound]) -> Result<(Mat, usize), String> {
        jacobian::central_differences(&mut self.f, u, r, &self.steps, &|k, x| {
            bounds[k].contains(x)
        })
    }
}

fn cost(r: &[f64]) -> f64 {
    r.iter().map(|x| x * x).sum()
}

/// Minimises Σ r(u)² from `u0` within `bounds` (see the module
/// documentation). Fails only if the model cannot be evaluated at `u0` or
/// its Jacobian cannot be formed.
pub fn minimize(
    problem: &mut dyn Problem,
    u0: &[f64],
    bounds: &[Bound],
    o: &LmOptions,
) -> Result<LmResult, String> {
    let n = u0.len();
    assert_eq!(bounds.len(), n, "one bound per variable");
    let to_u = |z: &[f64]| -> Vec<f64> { z.iter().zip(bounds).map(|(z, b)| b.u(*z)).collect() };
    let mut z: Vec<f64> = u0.iter().zip(bounds).map(|(u, b)| b.z(*u)).collect();
    let mut u = to_u(&z);
    let mut evaluations = 1;
    let mut r = problem
        .residuals(&u)
        .map_err(|e| format!("the model cannot be evaluated at the starting point: {e}"))?;
    let mut c = cost(&r);
    if !c.is_finite() {
        return Err("the residuals are not finite at the starting point".into());
    }
    let mut mu = o.mu0;
    let mut failed = 0;
    let mut iterations = 0;
    let mut history: Vec<f64> = vec![c];
    let stop = 'outer: loop {
        if c <= o.cost_floor {
            break Stop::ExactFit;
        }
        if n == 0 {
            break Stop::NoFurtherReduction;
        }
        if iterations >= o.max_iterations {
            break Stop::MaxIterations;
        }
        if evaluations >= o.max_evaluations {
            break Stop::MaxEvaluations;
        }
        iterations += 1;
        let (jac, used) = problem.jacobian(&u, &r, bounds)?;
        evaluations += used;
        let d = dense::svd(&jac);
        let smax = d.s.first().copied().unwrap_or(0.0);
        let keep: Vec<usize> = (0..n)
            .filter(|&i| d.s[i] > 0.0 && d.s[i] > o.null_tolerance * smax && d.s[i] > o.min_sigma)
            .collect();
        if keep.is_empty() {
            break Stop::NoFurtherReduction;
        }
        let diag: Vec<f64> = (0..n)
            .map(|k| (0..jac.rows).map(|i| jac.get(i, k).powi(2)).sum())
            .collect();
        let dmax = diag.iter().copied().fold(0.0, f64::max);
        let diag: Vec<f64> = diag.iter().map(|&x| x.max(1e-30 * dmax)).collect();
        let utr: Vec<f64> = keep
            .iter()
            .map(|&i| (0..jac.rows).map(|row| d.u.get(row, i) * r[row]).sum())
            .collect();
        let vdv: Vec<Vec<f64>> = keep
            .iter()
            .map(|&a| {
                keep.iter()
                    .map(|&b| {
                        (0..n)
                            .map(|k| d.v.get(k, a) * diag[k] * d.v.get(k, b))
                            .sum()
                    })
                    .collect()
            })
            .collect();
        let mut accepted = false;
        while mu < 1e14 {
            if evaluations >= o.max_evaluations {
                break 'outer Stop::MaxEvaluations;
            }
            let a: Vec<Vec<f64>> = (0..keep.len())
                .map(|x| {
                    (0..keep.len())
                        .map(|y| {
                            let s2 = if x == y { d.s[keep[x]].powi(2) } else { 0.0 };
                            s2 + mu * vdv[x][y]
                        })
                        .collect()
                })
                .collect();
            let b: Vec<f64> = keep.iter().zip(&utr).map(|(&i, g)| -d.s[i] * g).collect();
            let Some(y) = dense::solve(a, b) else {
                mu *= 10.0;
                continue;
            };
            let delta: Vec<f64> = (0..n)
                .map(|k| keep.iter().zip(&y).map(|(&i, y)| d.v.get(k, i) * y).sum())
                .collect();
            let zt: Vec<f64> = z
                .iter()
                .zip(&delta)
                .zip(bounds)
                .map(|((z, d), b)| z + (d / b.du_dz(*z).max(1e-300)).clamp(-60.0, 60.0))
                .collect();
            let ut = to_u(&zt);
            evaluations += 1;
            match problem.residuals(&ut) {
                Ok(rt) => {
                    let ct = cost(&rt);
                    if ct.is_finite() && ct < c {
                        let rel = (c - ct) / c.max(1e-300);
                        let small = delta.iter().all(|d| d.abs() < o.xtol);
                        z = zt;
                        u = ut;
                        r = rt;
                        c = ct;
                        mu = (mu / 3.0).max(1e-15);
                        accepted = true;
                        if c <= o.cost_floor {
                            break 'outer Stop::ExactFit;
                        }
                        if rel < o.ftol {
                            break 'outer Stop::SmallReduction;
                        }
                        history.push(c);
                        let (w, atol) = o.stall;
                        if w > 0 && history.len() > w && history[history.len() - 1 - w] - c < atol {
                            break 'outer Stop::SmallReduction;
                        }
                        if small {
                            break 'outer Stop::SmallStep;
                        }
                        break;
                    }
                    mu *= 4.0;
                }
                Err(_) => {
                    failed += 1;
                    mu *= 4.0;
                }
            }
        }
        if !accepted {
            break Stop::NoFurtherReduction;
        }
    };
    Ok(LmResult {
        u,
        residuals: r,
        cost: c,
        iterations,
        evaluations,
        failed_evaluations: failed,
        stop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_invert_and_stay_inside() {
        for b in [
            Bound::Free,
            Bound::Lower(-1.0),
            Bound::Upper(2.0),
            Bound::Both(-1.0, 2.0),
        ] {
            for u in [-0.9, 0.0, 0.5, 1.9] {
                let z = b.z(u);
                assert!((b.u(z) - u).abs() < 1e-12, "{b:?} {u}");
                let h = 1e-6;
                let fd = (b.u(z + h) - b.u(z - h)) / (2.0 * h);
                assert!((fd - b.du_dz(z)).abs() < 1e-8);
            }
            for z in [-50.0, -5.0, 0.0, 5.0, 50.0] {
                assert!(b.contains(b.u(z)), "{b:?} {z}");
            }
        }
    }

    #[test]
    fn fits_an_exponential_decay() {
        // y = A·exp(−k·t), exact data: recovers A and k from a poor start.
        let t: Vec<f64> = (0..20).map(|i| i as f64 * 0.25).collect();
        let (a, k) = (3.0_f64, 0.7_f64);
        let y: Vec<f64> = t.iter().map(|t| a * (-k * t).exp()).collect();
        let mut p = FnProblem {
            f: |u: &[f64]| -> Result<Vec<f64>, String> {
                Ok(t.iter()
                    .zip(&y)
                    .map(|(t, y)| u[0].exp() * (-u[1].exp() * t).exp() - y)
                    .collect())
            },
            steps: vec![1e-6; 2],
        };
        let o = LmOptions {
            cost_floor: 1e-26,
            ..LmOptions::default()
        };
        let r = minimize(&mut p, &[0.0, 1.0], &[Bound::Free; 2], &o).unwrap();
        assert!(r.stop.converged(), "{r:?}");
        assert!((r.u[0].exp() / a - 1.0).abs() < 1e-9);
        assert!((r.u[1].exp() / k - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_bound_holds_and_failing_points_are_rejected() {
        // Minimum of (u − 3)² at u = 3, but u ≤ 2; the model fails above 2.5.
        let mut p = FnProblem {
            f: |u: &[f64]| -> Result<Vec<f64>, String> {
                if u[0] > 2.5 {
                    Err("singular".into())
                } else {
                    Ok(vec![u[0] - 3.0])
                }
            },
            steps: vec![1e-6],
        };
        let r = minimize(
            &mut p,
            &[0.0],
            &[Bound::Both(-5.0, 2.0)],
            &LmOptions::default(),
        )
        .unwrap();
        assert!(r.u[0] <= 2.0 && r.u[0] > 1.99, "{r:?}");
        let mut q = FnProblem {
            f: |u: &[f64]| -> Result<Vec<f64>, String> {
                if u[0] > 2.5 {
                    Err("singular".into())
                } else {
                    Ok(vec![u[0] - 3.0])
                }
            },
            steps: vec![1e-6],
        };
        let r = minimize(&mut q, &[0.0], &[Bound::Free], &LmOptions::default()).unwrap();
        assert!(r.failed_evaluations > 0);
        assert!(r.u[0] <= 2.5 && r.u[0] > 2.4, "{r:?}");
    }

    #[test]
    fn null_directions_are_not_stepped_along() {
        // r depends on u0 + u1 only: the difference direction is null.
        let mut p = FnProblem {
            f: |u: &[f64]| -> Result<Vec<f64>, String> {
                Ok(vec![u[0] + u[1] - 1.0, 2.0 * (u[0] + u[1]) - 2.0])
            },
            steps: vec![1e-6; 2],
        };
        let r = minimize(
            &mut p,
            &[0.3, 0.1],
            &[Bound::Free; 2],
            &LmOptions::default(),
        )
        .unwrap();
        assert!((r.u[0] + r.u[1] - 1.0).abs() < 1e-9);
        // The minimum-norm correction splits the change equally.
        assert!((r.u[0] - r.u[1] - 0.2).abs() < 1e-9, "{:?}", r.u);
    }
}
