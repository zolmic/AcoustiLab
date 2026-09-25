//! Jacobians of residual vectors by finite differences.
//!
//! The fit calls [`central_differences`] through [`crate::fit::lm::Problem`],
//! so a faster method (forward sensitivities reusing the LU factors, or an
//! adjoint) can replace it without touching the optimiser.
//!
//! Central differences with step h have a truncation error of order
//! h²·r'''/6 and a rounding error of order ε_r/h, where ε_r is the absolute
//! accuracy of one residual evaluation. For full network solves in
//! log-parameter space, ε_r is about 1e-13 relative, so h = 1e-4 balances
//! the two near 1e-9 relative, far below anything a fit or an
//! identifiability test resolves (docs/fitting.md).

use super::dense::Mat;

/// Residual function: residuals at a point, or why they cannot be evaluated
/// there (a singular network, a parameter an element rejects).
pub type ResidualFn<'a> = dyn FnMut(&[f64]) -> Result<Vec<f64>, String> + 'a;

/// ∂r/∂u by central differences with per-variable steps `steps`, and the
/// number of residual evaluations used.
///
/// `inside(k, x)` says whether variable k may take the value x (its
/// bounds). Where a central difference would leave the bounds, or one side
/// cannot be evaluated, a one-sided difference against `r0` (the residuals
/// at `u`) is used instead; if neither side can be evaluated the
/// derivative is an error naming the variable.
pub fn central_differences(
    f: &mut ResidualFn,
    u: &[f64],
    r0: &[f64],
    steps: &[f64],
    inside: &dyn Fn(usize, f64) -> bool,
) -> Result<(Mat, usize), String> {
    let n = u.len();
    let m = r0.len();
    let mut jac = Mat::zeros(m, n);
    let mut evals = 0;
    let mut point = u.to_vec();
    for k in 0..n {
        let h = steps[k];
        let mut eval = |x: f64, evals: &mut usize| -> Option<Result<Vec<f64>, String>> {
            if !inside(k, x) {
                return None;
            }
            point[k] = x;
            *evals += 1;
            let r = f(&point);
            point[k] = u[k];
            Some(r.and_then(|r| {
                if r.len() == m {
                    Ok(r)
                } else {
                    Err(format!(
                        "the residual vector changed length ({} to {}) within a difference step",
                        m,
                        r.len()
                    ))
                }
            }))
        };
        let plus = eval(u[k] + h, &mut evals);
        let minus = eval(u[k] - h, &mut evals);
        let column: Vec<f64> = match (plus, minus) {
            (Some(Ok(rp)), Some(Ok(rm))) => rp
                .iter()
                .zip(&rm)
                .map(|(a, b)| (a - b) / (2.0 * h))
                .collect(),
            (Some(Ok(rp)), _) => rp.iter().zip(r0).map(|(a, b)| (a - b) / h).collect(),
            (_, Some(Ok(rm))) => r0.iter().zip(&rm).map(|(a, b)| (a - b) / h).collect(),
            (Some(Err(e)), _) | (_, Some(Err(e))) => {
                return Err(format!(
                    "cannot differentiate with respect to variable {k}: {e}"
                ))
            }
            (None, None) => {
                return Err(format!(
                    "cannot differentiate with respect to variable {k}: its bounds are narrower than the difference step"
                ))
            }
        };
        for (r, v) in column.into_iter().enumerate() {
            jac.set(r, k, v);
        }
    }
    Ok((jac, evals))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_analytic_derivatives_and_goes_one_sided_at_bounds() {
        // r(u) = [e^{u0}·u1, sin(u0) + u1²]
        let mut f = |u: &[f64]| -> Result<Vec<f64>, String> {
            Ok(vec![u[0].exp() * u[1], u[0].sin() + u[1] * u[1]])
        };
        let u = [0.3, 1.7];
        let r0 = f(&u).unwrap();
        let exact = [[u[0].exp() * u[1], u[0].exp()], [u[0].cos(), 2.0 * u[1]]];
        let (j, n) = central_differences(&mut f, &u, &r0, &[1e-4, 1e-4], &|_, _| true).unwrap();
        assert_eq!(n, 4);
        for (r, row) in exact.iter().enumerate() {
            for (c, x) in row.iter().enumerate() {
                assert!((j.get(r, c) - x).abs() < 1e-8);
            }
        }
        // Variable 0 bounded above at u0: forward step refused, backward used.
        let (j, n) =
            central_differences(&mut f, &u, &r0, &[1e-6, 1e-6], &|k, x| k != 0 || x <= 0.3)
                .unwrap();
        assert_eq!(n, 3);
        assert!((j.get(0, 0) - exact[0][0]).abs() < 1e-5);
        // A failing side falls back to the other one.
        let mut g = |u: &[f64]| -> Result<Vec<f64>, String> {
            if u[0] > 0.3 {
                Err("singular".into())
            } else {
                Ok(vec![u[0] * u[0]])
            }
        };
        let (j, _) = central_differences(&mut g, &[0.3], &[0.09], &[1e-7], &|_, _| true).unwrap();
        assert!((j.get(0, 0) - 0.6).abs() < 1e-6);
    }
}
