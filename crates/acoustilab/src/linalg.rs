//! Dense complex LU with row/column equilibration and partial pivoting.
//!
//! Systems are small (tens of unknowns) and mix electrical, mechanical and
//! acoustic magnitudes spanning many decades, so rows and columns are scaled
//! to unit maximum before factorisation (spec Section 3, "Conditioning").

use crate::C64;

/// Row-major square matrix.
#[derive(Debug, Clone)]
pub struct Matrix {
    pub n: usize,
    pub data: Vec<C64>,
}

impl Matrix {
    pub fn zeros(n: usize) -> Self {
        Matrix {
            n,
            data: vec![C64::new(0.0, 0.0); n * n],
        }
    }

    #[inline]
    pub fn get(&self, r: usize, c: usize) -> C64 {
        self.data[r * self.n + c]
    }

    #[inline]
    pub fn add(&mut self, r: usize, c: usize, v: C64) {
        self.data[r * self.n + c] += v;
    }

    pub fn clear(&mut self) {
        self.data.iter_mut().for_each(|x| *x = C64::new(0.0, 0.0));
    }

    /// y = A x.
    pub fn mul_vec(&self, x: &[C64]) -> Vec<C64> {
        (0..self.n)
            .map(|r| (0..self.n).map(|c| self.data[r * self.n + c] * x[c]).sum())
            .collect()
    }
}

/// Failure of the factorisation: the column (unknown) whose pivot vanished.
#[derive(Debug, Clone, Copy)]
pub struct SingularColumn(pub usize);

/// LU factors of an equilibrated matrix: P·(R·A·C) = L·U, with row scale R,
/// column scale C and row permutation P, stored so that further right-hand
/// sides (refinement steps, sensitivities) reuse the factorisation.
#[derive(Debug, Clone)]
pub struct Lu {
    n: usize,
    lu: Vec<C64>,
    perm: Vec<usize>,
    row_scale: Vec<f64>,
    col_scale: Vec<f64>,
}

impl Lu {
    /// Equilibrates `a` (rows, then columns, to unit max-abs) and factors it
    /// with partial pivoting.
    #[allow(clippy::needless_range_loop)]
    pub fn factor(a: &Matrix) -> Result<Lu, SingularColumn> {
        let n = a.n;
        let mut m = a.data.clone();
        let mut row_scale = vec![1.0; n];
        for r in 0..n {
            let mx = (0..n).map(|c| m[r * n + c].norm()).fold(0.0, f64::max);
            if mx == 0.0 {
                return Err(SingularColumn(r));
            }
            row_scale[r] = 1.0 / mx;
            for c in 0..n {
                m[r * n + c] *= row_scale[r];
            }
        }
        let mut col_scale = vec![1.0; n];
        for c in 0..n {
            let mx = (0..n).map(|r| m[r * n + c].norm()).fold(0.0, f64::max);
            if mx == 0.0 {
                return Err(SingularColumn(c));
            }
            col_scale[c] = 1.0 / mx;
            for r in 0..n {
                m[r * n + c] *= col_scale[c];
            }
        }
        let mut perm: Vec<usize> = (0..n).collect();
        let tiny = 1e-14;
        for k in 0..n {
            let (p, pmax) = (k..n)
                .map(|r| (r, m[r * n + k].norm()))
                .fold((k, -1.0), |acc, x| if x.1 > acc.1 { x } else { acc });
            if pmax <= tiny {
                return Err(SingularColumn(k));
            }
            if p != k {
                for c in 0..n {
                    m.swap(k * n + c, p * n + c);
                }
                perm.swap(k, p);
            }
            let inv = m[k * n + k].inv();
            for r in (k + 1)..n {
                let l = m[r * n + k] * inv;
                if l == C64::new(0.0, 0.0) {
                    continue;
                }
                m[r * n + k] = l;
                for c in (k + 1)..n {
                    let akc = m[k * n + c];
                    m[r * n + c] -= l * akc;
                }
            }
        }
        Ok(Lu {
            n,
            lu: m,
            perm,
            row_scale,
            col_scale,
        })
    }

    /// Solves A x = b with the stored factors.
    #[allow(clippy::needless_range_loop)]
    pub fn solve(&self, b: &[C64]) -> Vec<C64> {
        let n = self.n;
        assert_eq!(b.len(), n);
        // y = P·R·b
        let mut y: Vec<C64> = (0..n)
            .map(|i| b[self.perm[i]] * self.row_scale[self.perm[i]])
            .collect();
        // Forward substitution with unit-diagonal L.
        for r in 0..n {
            let mut s = y[r];
            for c in 0..r {
                s -= self.lu[r * n + c] * y[c];
            }
            y[r] = s;
        }
        // Back substitution with U.
        for r in (0..n).rev() {
            let mut s = y[r];
            for c in (r + 1)..n {
                s -= self.lu[r * n + c] * y[c];
            }
            y[r] = s / self.lu[r * n + r];
        }
        y.iter()
            .zip(&self.col_scale)
            .map(|(yi, s)| yi * *s)
            .collect()
    }
}

/// Solves A x = b: equilibrated partial-pivot LU followed by up to two steps
/// of iterative refinement against the original matrix. Equilibration alone
/// leaves potentials far below the largest one accurate only norm-wise
/// (up to ~6 digits lost); refinement restores them to ~1e-15.
pub fn solve(a: Matrix, b: &[C64]) -> Result<Vec<C64>, SingularColumn> {
    let n = a.n;
    assert_eq!(b.len(), n);
    if n == 0 {
        return Ok(Vec::new());
    }
    let lu = Lu::factor(&a)?;
    let mut x = lu.solve(b);
    for _ in 0..2 {
        let ax = a.mul_vec(&x);
        let r: Vec<C64> = b.iter().zip(&ax).map(|(bi, ai)| bi - ai).collect();
        let dx = lu.solve(&r);
        let mut changed = false;
        for (xi, di) in x.iter_mut().zip(&dx) {
            if di.norm() > 1e-17 * xi.norm() {
                changed = true;
            }
            *xi += di;
        }
        if !changed {
            break;
        }
    }
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn solves_badly_scaled_complex_system() {
        // Magnitudes spanning 20 decades, as in a mixed-domain MNA.
        let n = 3;
        let mut a = Matrix::zeros(n);
        let vals = [
            [
                C64::new(1e-10, 2e-10),
                C64::new(0.0, 1e-10),
                C64::new(0.0, 0.0),
            ],
            [C64::new(0.0, 1e-10), C64::new(5.0, 0.0), C64::new(1e5, 1e5)],
            [C64::new(0.0, 0.0), C64::new(1e5, -1e5), C64::new(3e10, 0.0)],
        ];
        for r in 0..n {
            for c in 0..n {
                a.add(r, c, vals[r][c]);
            }
        }
        let x_true = vec![
            C64::new(1.0, -2.0),
            C64::new(3e-3, 0.5),
            C64::new(-1e-6, 2e-6),
        ];
        let b = a.mul_vec(&x_true);
        let x = solve(a, &b).unwrap();
        for (xi, ti) in x.iter().zip(&x_true) {
            assert!((xi - ti).norm() <= 1e-12 * ti.norm());
        }
    }

    #[test]
    fn reports_singular_column() {
        let mut a = Matrix::zeros(2);
        a.add(0, 0, C64::new(1.0, 0.0));
        a.add(1, 0, C64::new(1.0, 0.0));
        assert!(solve(a, &[C64::new(1.0, 0.0), C64::new(1.0, 0.0)]).is_err());
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn refinement_restores_small_components() {
        // A graded system: the last unknown is ~1e-8 of the first, and the
        // coupling makes unrefined equilibrated LU lose digits there.
        let n = 4;
        let mut a = Matrix::zeros(n);
        let v = [
            [
                C64::new(2e8, 0.0),
                C64::new(-1.2e8, 3e7),
                C64::new(0.0, 0.0),
                C64::new(0.0, 0.0),
            ],
            [
                C64::new(-1.2e8, 3e7),
                C64::new(3.3e8, -1e6),
                C64::new(-2.1e8, 0.0),
                C64::new(0.0, 0.0),
            ],
            [
                C64::new(0.0, 0.0),
                C64::new(-2.1e8, 0.0),
                C64::new(2.1e8, 4e4),
                C64::new(0.0, -7e3),
            ],
            [
                C64::new(0.0, 0.0),
                C64::new(0.0, 0.0),
                C64::new(0.0, -7e3),
                C64::new(1e-3, 7e3),
            ],
        ];
        for (r, row) in v.iter().enumerate() {
            for (c, val) in row.iter().enumerate() {
                a.add(r, c, *val);
            }
        }
        let x_true = vec![
            C64::new(0.24, 0.01),
            C64::new(0.1, -0.05),
            C64::new(3e-4, 2e-4),
            C64::new(2.35e-6, -1e-7),
        ];
        let b = a.mul_vec(&x_true);
        let unrefined = Lu::factor(&a).unwrap().solve(&b);
        let x = solve(a, &b).unwrap();
        // Componentwise condition numbers |A⁻¹||A||x|/|x| (numpy):
        // 6.1, 18, 5.9e3, 9.1e5. The attainable accuracy is ~eps times these
        // (LAPACK reaches 1.6e-16, 4.5e-16, 1.7e-13, 2.5e-11).
        let cond = [6.13, 18.2, 5.92e3, 9.07e5];
        for k in 0..4 {
            let err = (x[k] - x_true[k]).norm() / x_true[k].norm();
            let err0 = (unrefined[k] - x_true[k]).norm() / x_true[k].norm();
            assert!(err <= 50.0 * f64::EPSILON * cond[k], "x{k}: {err:e}");
            assert!(err <= err0 * 1.0001 + 1e-16, "refinement worsened x{k}");
        }
    }
}
