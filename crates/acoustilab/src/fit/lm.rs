//! Levenberg–Marquardt least squares with box bounds.
//!
//! Minimises the cost Σ r_i(u)² over the fitting variables u (the natural
//! logarithms of positive parameters, or scaled parameters on a linear
//! scale) within bounds a ≤ u ≤ b ([`Bound`]).
//!
//! Each iteration takes the Jacobian J = ∂r/∂u, its singular value
//! decomposition J = U·Σ·Vᵀ, and solves the Marquardt-damped normal
//! equations (JᵀJ + μ·D)·δu = −Jᵀr, D = diag(JᵀJ), within the span of the
//! right singular vectors whose singular value exceeds
//! [`LmOptions::null_tolerance`] times the largest and the absolute floor
//! [`LmOptions::min_sigma`]: with δu = V_r·y,
//! (Σ_r² + μ·V_rᵀ·D·V_r)·y = −Σ_r·U_rᵀ·r. Directions the data do not
//! constrain (a numerically null or statistically flat singular value)
//! are thus not stepped along, so parameters that only move along them
//! stay where they are instead of wandering on noise. With every
//! direction kept this is the ordinary Marquardt step.
//!
//! Flat directions are frozen only while the fit is statistically
//! acceptable: if the iteration would stop with some left out and a cost
//! per degree of freedom above [`LmOptions::unfreeze_above`], it continues
//! with every direction that is not numerically null. A weakly sensitive
//! parameter that starts far from its optimum (a leak that starts nearly
//! closed) thus still gets there, where freezing it would stop at once
//! with a gross misfit.
//!
//! Bounds are kept by the fraction-to-the-boundary rule of interior-point
//! methods, per variable: a component of the step that would cross a bound
//! goes 90 % of the way to it instead ([`Bound::step`]). Trial points
//! therefore never reach a bound, so the model is never evaluated on it
//! and a variable is never pinned there, unlike projection onto the box;
//! a variable whose optimum lies beyond a bound approaches it geometrically
//! while the cost keeps falling, and the fit reports it as at the bound.
//! A start on a bound is allowed: steps away from it are taken in full.
//! (A smooth change of variable, such as a logistic map, was not used: its
//! derivative vanishes at the bounds, which freezes a variable that starts
//! on one and distorts the flatness test near them.)
//!
//! A trial point that lowers the cost is accepted and μ divided by 3;
//! otherwise μ is multiplied by 4 (by 10 when the damped system is
//! singular). A trial point where the model cannot be evaluated, such as a
//! singular network, counts as a rejected step.

use super::dense::{self, Mat};
use super::jacobian;
use serde::Serialize;

/// Fraction of the distance to a bound that a crossing step component
/// covers.
pub const TO_BOUNDARY: f64 = 0.9;

/// Bound of one fitting variable u (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bound {
    Free,
    Lower(f64),
    Upper(f64),
    /// Lower and upper bound, lower < upper.
    Both(f64, f64),
}

impl Bound {
    /// From optional bounds.
    pub fn new(lo: Option<f64>, hi: Option<f64>) -> Bound {
        match (lo, hi) {
            (Some(a), Some(b)) => Bound::Both(a, b),
            (Some(a), None) => Bound::Lower(a),
            (None, Some(b)) => Bound::Upper(b),
            (None, None) => Bound::Free,
        }
    }

    fn lower(&self) -> Option<f64> {
        match *self {
            Bound::Lower(a) | Bound::Both(a, _) => Some(a),
            _ => None,
        }
    }

    fn upper(&self) -> Option<f64> {
        match *self {
            Bound::Upper(b) | Bound::Both(_, b) => Some(b),
            _ => None,
        }
    }

    /// True if u satisfies the bounds.
    pub fn contains(&self, u: f64) -> bool {
        self.lower().is_none_or(|a| u >= a) && self.upper().is_none_or(|b| u <= b)
    }

    /// u clamped into the bounds.
    pub fn clamp(&self, u: f64) -> f64 {
        let u = self.lower().map_or(u, |a| u.max(a));
        self.upper().map_or(u, |b| u.min(b))
    }

    /// u + δ, or [`TO_BOUNDARY`] of the way to the bound that u + δ would
    /// reach or cross.
    pub fn step(&self, u: f64, delta: f64) -> f64 {
        let t = u + delta;
        match (self.lower(), self.upper()) {
            (Some(a), _) if t <= a => u + TO_BOUNDARY * (a - u),
            (_, Some(b)) if t >= b => u + TO_BOUNDARY * (b - u),
            _ => t,
        }
    }

    /// True if u lies within `tol` of a bound (in u).
    pub fn near_bound(&self, u: f64, tol: f64) -> bool {
        self.lower().is_some_and(|a| u - a <= tol) || self.upper().is_some_and(|b| b - u <= tol)
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
    /// No step lowers the cost along the directions left, and those left
    /// out are statistically flat ([`LmOptions::min_sigma`]) at an
    /// acceptable misfit ([`LmOptions::unfreeze_above`]).
    Flat,
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
            Stop::Flat => {
                "converged: no step lowers the cost along the directions the data determine; along the others (moving e-fold changes chi-square by less than 1) the parameters stay where they are"
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
    /// the step while [`LmOptions::unfreeze_above`] allows. For residuals
    /// weighted by their standard uncertainties, σ < 1 means that moving
    /// one unit along the direction changes χ² by less than 1: the data
    /// barely determine it, and stepping along it near the optimum only
    /// chases noise (the fit then leaves that combination where it is).
    /// 0 disables it.
    pub min_sigma: f64,
    /// Cost per degree of freedom, Σr²/(m − n), above which a stop with
    /// directions left out by `min_sigma` is not accepted: the iteration
    /// continues with them. For weighted residuals this is a reduced χ²; a
    /// value far above 1 is a gross misfit, not noise.
    pub unfreeze_above: f64,
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
            unfreeze_above: f64::INFINITY,
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
    let mut u: Vec<f64> = u0.iter().zip(bounds).map(|(u, b)| b.clamp(*u)).collect();
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
    let dof = (r.len() as f64 - n as f64).max(1.0);
    let mut frozen = o.min_sigma > 0.0;
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
        let resolved: Vec<usize> = (0..n)
            .filter(|&i| d.s[i] > 0.0 && d.s[i] > o.null_tolerance * smax)
            .collect();
        let keep: Vec<usize> = resolved
            .iter()
            .copied()
            .filter(|&i| !frozen || d.s[i] > o.min_sigma)
            .collect();
        let left_out = keep.len() < resolved.len();
        if keep.is_empty() {
            if resolved.is_empty() {
                break Stop::NoFurtherReduction;
            }
            if c / dof > o.unfreeze_above {
                frozen = false;
                continue;
            }
            break Stop::Flat;
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
        let mut converged = None;
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
            let ut: Vec<f64> = u
                .iter()
                .zip(&delta)
                .zip(bounds)
                .map(|((u, d), b)| b.step(*u, *d))
                .collect();
            evaluations += 1;
            match problem.residuals(&ut) {
                Ok(rt) => {
                    let ct = cost(&rt);
                    if ct.is_finite() && ct < c {
                        let rel = (c - ct) / c.max(1e-300);
                        let small = delta.iter().all(|d| d.abs() < o.xtol);
                        u = ut;
                        r = rt;
                        c = ct;
                        mu = (mu / 3.0).max(1e-15);
                        accepted = true;
                        if c <= o.cost_floor {
                            break 'outer Stop::ExactFit;
                        }
                        history.push(c);
                        let (w, atol) = o.stall;
                        let stalled =
                            w > 0 && history.len() > w && history[history.len() - 1 - w] - c < atol;
                        if rel < o.ftol || stalled {
                            converged = Some(Stop::SmallReduction);
                        } else if small {
                            converged = Some(Stop::SmallStep);
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
            converged = Some(if left_out {
                Stop::Flat
            } else {
                Stop::NoFurtherReduction
            });
        }
        if let Some(stop) = converged {
            if left_out && c / dof > o.unfreeze_above {
                frozen = false;
                mu = o.mu0;
                history.clear();
                history.push(c);
                continue;
            }
            break stop;
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
    fn steps_stop_short_of_the_bounds() {
        let b = Bound::Both(-1.0, 2.0);
        assert_eq!(b.step(0.0, 0.5), 0.5);
        assert!((b.step(0.0, 5.0) - 1.8).abs() < 1e-15);
        assert!((b.step(0.0, -5.0) + 0.9).abs() < 1e-15);
        assert_eq!(b.step(-1.0, 0.3), -0.7, "away from a bound: in full");
        assert_eq!(b.step(-1.0, -0.3), -1.0, "into a bound it sits on: no move");
        assert_eq!(Bound::Lower(1.0).step(1.5, -2.0), 1.05);
        assert_eq!(Bound::Upper(1.0).step(0.0, 3.0), 0.9);
        assert_eq!(Bound::Free.step(0.0, 1e9), 1e9);
        assert_eq!(b.clamp(7.0), 2.0);
        assert!(b.near_bound(1.9995, 1e-3) && !b.near_bound(0.0, 1e-3));
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
    fn flat_directions_are_frozen_only_at_an_acceptable_misfit() {
        // 50 residuals 0.1·(u − a): σ = 0.1·√50 = 0.71, below min_sigma.
        let run = |a: f64, unfreeze_above: f64| {
            let mut p = FnProblem {
                f: move |u: &[f64]| -> Result<Vec<f64>, String> { Ok(vec![0.1 * (u[0] - a); 50]) },
                steps: vec![1e-6],
            };
            let o = LmOptions {
                min_sigma: 1.0,
                unfreeze_above,
                ..LmOptions::default()
            };
            minimize(&mut p, &[0.0], &[Bound::Free], &o).unwrap()
        };
        // χ²/(m − n) = 50·0.01·a²/49 at the start: 4.1 for a = 20 stays
        // frozen, 16.3 for a = 40 (a gross misfit) does not.
        let r = run(20.0, 10.0);
        assert_eq!((r.stop, r.u[0]), (Stop::Flat, 0.0));
        let r = run(40.0, 10.0);
        assert!(r.stop.converged() && (r.u[0] - 40.0).abs() < 1e-6, "{r:?}");
        let r = run(40.0, f64::INFINITY);
        assert_eq!((r.stop, r.u[0]), (Stop::Flat, 0.0));
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
