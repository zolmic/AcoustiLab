//! One-dimensional searches on exact re-solves: refine a grid maximum and a
//! level crossing between grid points (readouts are never interpolated).

use crate::error::Result;

/// Maximum of `f` on [a, b] by Brent's method (golden section with
/// parabolic steps; R. P. Brent, "Algorithms for Minimization without
/// Derivatives", 1973, procedure `localmin`). Stops when the bracket is
/// below `tol` (absolute, in the units of x) plus √ε·|x|. Returns (x, f(x)),
/// the best point evaluated.
pub fn brent_max(
    f: &mut dyn FnMut(f64) -> Result<f64>,
    a: f64,
    b: f64,
    tol: f64,
) -> Result<(f64, f64)> {
    let golden = 0.5 * (3.0 - 5f64.sqrt());
    let sqrt_eps = f64::EPSILON.sqrt();
    let (mut a, mut b) = (a.min(b), a.max(b));
    let mut x = a + golden * (b - a);
    let (mut w, mut v) = (x, x);
    // Minimise g = −f.
    let mut fx = -f(x)?;
    let (mut fw, mut fv) = (fx, fx);
    let (mut d, mut e) = (0.0f64, 0.0f64);
    for _ in 0..200 {
        let xm = 0.5 * (a + b);
        let tol1 = sqrt_eps * x.abs() + tol / 3.0;
        let tol2 = 2.0 * tol1;
        if (x - xm).abs() <= tol2 - 0.5 * (b - a) {
            break;
        }
        let mut golden_step = true;
        if e.abs() > tol1 {
            let r = (x - w) * (fx - fv);
            let mut q = (x - v) * (fx - fw);
            let mut p = (x - v) * q - (x - w) * r;
            q = 2.0 * (q - r);
            if q > 0.0 {
                p = -p;
            }
            q = q.abs();
            let e_prev = e;
            if p.abs() < (0.5 * q * e_prev).abs() && p > q * (a - x) && p < q * (b - x) {
                e = d;
                d = p / q;
                let u = x + d;
                if u - a < tol2 || b - u < tol2 {
                    d = tol1.copysign(xm - x);
                }
                golden_step = false;
            }
        }
        if golden_step {
            e = if x >= xm { a - x } else { b - x };
            d = golden * e;
        }
        let u = if d.abs() >= tol1 {
            x + d
        } else {
            x + tol1.copysign(d)
        };
        let fu = -f(u)?;
        if fu <= fx {
            if u >= x {
                a = x;
            } else {
                b = x;
            }
            v = w;
            fv = fw;
            w = x;
            fw = fx;
            x = u;
            fx = fu;
        } else {
            if u < x {
                a = u;
            } else {
                b = u;
            }
            if fu <= fw || w == x {
                v = w;
                fv = fw;
                w = u;
                fw = fu;
            } else if fu <= fv || v == x || v == w {
                v = u;
                fv = fu;
            }
        }
    }
    Ok((x, -fx))
}

/// Root of `g` in [a, b], where g(a) and g(b) differ in sign, by the
/// Illinois variant of regula falsi (Dowell and Jarratt, BIT 11, 1971),
/// which keeps the root bracketed and converges superlinearly. Stops when
/// the bracket is below `tol` (absolute) or g vanishes.
pub fn illinois_root(
    g: &mut dyn FnMut(f64) -> Result<f64>,
    a: f64,
    b: f64,
    ga: f64,
    gb: f64,
    tol: f64,
) -> Result<f64> {
    let (mut a, mut b, mut ga, mut gb) = (a, b, ga, gb);
    if ga == 0.0 {
        return Ok(a);
    }
    if gb == 0.0 {
        return Ok(b);
    }
    let mut side = 0i8;
    for _ in 0..100 {
        if (b - a).abs() <= tol {
            break;
        }
        let mut c = (a * gb - b * ga) / (gb - ga);
        if !c.is_finite() || c <= a.min(b) || c >= a.max(b) {
            c = 0.5 * (a + b);
        }
        let gc = g(c)?;
        if gc == 0.0 {
            return Ok(c);
        }
        if (gc > 0.0) == (gb > 0.0) {
            b = c;
            gb = gc;
            if side == -1 {
                ga *= 0.5;
            }
            side = -1;
        } else {
            a = c;
            ga = gc;
            if side == 1 {
                gb *= 0.5;
            }
            side = 1;
        }
    }
    Ok((a * gb - b * ga) / (gb - ga))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brent_finds_a_smooth_maximum() {
        let mut n = 0;
        let mut f = |x: f64| {
            n += 1;
            Ok(-(x - 0.3).powi(2) + 0.1 * (x - 0.3).powi(3) + 2.0)
        };
        let (x, fx) = brent_max(&mut f, 0.0, 1.0, 1e-10).unwrap();
        assert!((x - 0.3).abs() < 1e-8, "{x}");
        assert!((fx - 2.0).abs() < 1e-15);
        assert!(n < 40, "{n} evaluations");
    }

    #[test]
    fn illinois_finds_a_root_quickly() {
        let mut n = 0;
        let mut g = |x: f64| {
            n += 1;
            Ok(x.exp() - 2.0)
        };
        let r = illinois_root(&mut g, 0.0, 1.0, -1.0, std::f64::consts::E - 2.0, 1e-13).unwrap();
        assert!((r - 2f64.ln()).abs() < 1e-12, "{r}");
        assert!(n < 20, "{n} evaluations");
    }
}
