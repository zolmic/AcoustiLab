//! Small dense real linear algebra for least-squares fitting: a row-major
//! matrix, the singular value decomposition by one-sided Jacobi rotations,
//! and Gaussian elimination for the normal equations.
//!
//! Fitting problems have a few to a few tens of parameters and a few hundred
//! to a few thousand residuals, so O(m·n²) algorithms are cheap. One-sided
//! Jacobi (Hestenes 1958; Demmel and Veselić, SIAM J. Matrix Anal. Appl.
//! 13(4), 1992) is used because it computes small singular values to high
//! relative accuracy, which is what an identifiability analysis needs: the
//! smallest singular value of a weighted Jacobian decides whether a
//! parameter combination is determined at all.

/// Row-major dense real matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
}

impl Mat {
    pub fn zeros(rows: usize, cols: usize) -> Mat {
        Mat {
            rows,
            cols,
            data: vec![0.0; rows * cols],
        }
    }

    pub fn identity(n: usize) -> Mat {
        let mut m = Mat::zeros(n, n);
        for i in 0..n {
            m.set(i, i, 1.0);
        }
        m
    }

    /// From a list of equal-length rows.
    pub fn from_rows(rows: &[Vec<f64>]) -> Mat {
        let cols = rows.first().map_or(0, Vec::len);
        assert!(rows.iter().all(|r| r.len() == cols), "ragged rows");
        Mat {
            rows: rows.len(),
            cols,
            data: rows.iter().flatten().copied().collect(),
        }
    }

    #[inline]
    pub fn get(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.cols + c]
    }

    #[inline]
    pub fn set(&mut self, r: usize, c: usize, v: f64) {
        self.data[r * self.cols + c] = v;
    }

    pub fn row(&self, r: usize) -> &[f64] {
        &self.data[r * self.cols..(r + 1) * self.cols]
    }

    pub fn col(&self, c: usize) -> Vec<f64> {
        (0..self.rows).map(|r| self.get(r, c)).collect()
    }

    pub fn transpose(&self) -> Mat {
        let mut t = Mat::zeros(self.cols, self.rows);
        for r in 0..self.rows {
            for c in 0..self.cols {
                t.set(c, r, self.get(r, c));
            }
        }
        t
    }

    pub fn mul(&self, b: &Mat) -> Mat {
        assert_eq!(self.cols, b.rows, "dimension mismatch");
        let mut out = Mat::zeros(self.rows, b.cols);
        for i in 0..self.rows {
            for k in 0..self.cols {
                let a = self.get(i, k);
                if a != 0.0 {
                    for j in 0..b.cols {
                        out.data[i * b.cols + j] += a * b.get(k, j);
                    }
                }
            }
        }
        out
    }

    /// Aᵀ·v.
    pub fn tmul_vec(&self, v: &[f64]) -> Vec<f64> {
        assert_eq!(self.rows, v.len(), "dimension mismatch");
        let mut out = vec![0.0; self.cols];
        for (r, &x) in v.iter().enumerate() {
            if x != 0.0 {
                for (o, a) in out.iter_mut().zip(self.row(r)) {
                    *o += a * x;
                }
            }
        }
        out
    }

    /// Multiplies column `c` by `s`.
    pub fn scale_col(&mut self, c: usize, s: f64) {
        for r in 0..self.rows {
            self.data[r * self.cols + c] *= s;
        }
    }

    /// Multiplies row `r` by `s`.
    pub fn scale_row(&mut self, r: usize, s: f64) {
        for x in &mut self.data[r * self.cols..(r + 1) * self.cols] {
            *x *= s;
        }
    }
}

/// Thin singular value decomposition A = U·diag(s)·Vᵀ of an m×n matrix:
/// U is m×n with orthonormal columns (a column is zero where s is zero),
/// s holds the n singular values in decreasing order, V is n×n orthogonal.
#[derive(Debug, Clone)]
pub struct Svd {
    pub u: Mat,
    pub s: Vec<f64>,
    pub v: Mat,
}

impl Svd {
    /// Column `i` of V: the right singular vector of s[i], a direction in
    /// parameter space.
    pub fn direction(&self, i: usize) -> Vec<f64> {
        self.v.col(i)
    }
}

/// SVD by one-sided Jacobi rotations on the columns of A.
///
/// Each rotation of a column pair (a_p, a_q) with α = |a_p|², β = |a_q|²,
/// γ = a_p·a_q makes them orthogonal: ζ = (β − α)/(2γ),
/// t = sign(ζ)/(|ζ| + sqrt(1 + ζ²)), c = 1/sqrt(1 + t²), s = c·t, then
/// a_p ← c·a_p − s·a_q and a_q ← s·a_p + c·a_q; the same rotation applied
/// to V accumulates the right singular vectors. Sweeps stop when every pair
/// satisfies |γ| ≤ 1e-15·sqrt(αβ). The column norms are then the singular
/// values. A matrix with fewer rows than columns is padded with zero rows.
pub fn svd(a: &Mat) -> Svd {
    let n = a.cols;
    let m = a.rows.max(n);
    // Work column-major: w[j] is column j.
    let mut w: Vec<Vec<f64>> = (0..n)
        .map(|j| {
            let mut c = a.col(j);
            c.resize(m, 0.0);
            c
        })
        .collect();
    let mut v: Vec<Vec<f64>> = (0..n)
        .map(|j| {
            let mut c = vec![0.0; n];
            c[j] = 1.0;
            c
        })
        .collect();
    let dot = |x: &[f64], y: &[f64]| x.iter().zip(y).map(|(a, b)| a * b).sum::<f64>();
    for _sweep in 0..80 {
        let mut rotated = false;
        for p in 0..n {
            for q in p + 1..n {
                let alpha = dot(&w[p], &w[p]);
                let beta = dot(&w[q], &w[q]);
                let gamma = dot(&w[p], &w[q]);
                if gamma == 0.0 || gamma.abs() <= 1e-15 * (alpha * beta).sqrt() {
                    continue;
                }
                rotated = true;
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                let (lo, hi) = w.split_at_mut(q);
                for (x, y) in lo[p].iter_mut().zip(hi[0].iter_mut()) {
                    let (xp, xq) = (*x, *y);
                    *x = c * xp - s * xq;
                    *y = s * xp + c * xq;
                }
                let (lo, hi) = v.split_at_mut(q);
                for (x, y) in lo[p].iter_mut().zip(hi[0].iter_mut()) {
                    let (xp, xq) = (*x, *y);
                    *x = c * xp - s * xq;
                    *y = s * xp + c * xq;
                }
            }
        }
        if !rotated {
            break;
        }
    }
    let norms: Vec<f64> = w.iter().map(|c| dot(c, c).sqrt()).collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| norms[j].total_cmp(&norms[i]));
    let mut u = Mat::zeros(a.rows, n);
    let mut vm = Mat::zeros(n, n);
    let mut s = Vec::with_capacity(n);
    for (k, &j) in order.iter().enumerate() {
        let sj = norms[j];
        s.push(sj);
        if sj > 0.0 {
            for (r, x) in w[j].iter().take(a.rows).enumerate() {
                u.set(r, k, x / sj);
            }
        }
        for (r, x) in v[j].iter().enumerate() {
            vm.set(r, k, *x);
        }
    }
    Svd { u, s, v: vm }
}

/// Gaussian elimination with partial pivoting for a small square system;
/// `None` if the matrix is singular or the solution is not finite.
pub fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col] == 0.0 || !a[piv][col].is_finite() {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let (upper, lower) = a.split_at_mut(col + 1);
        let pivot = &upper[col];
        for (i, row) in lower.iter_mut().enumerate() {
            let f = row[col] / pivot[col];
            if f != 0.0 {
                for (x, y) in row[col..].iter_mut().zip(&pivot[col..]) {
                    *x -= f * y;
                }
                b[col + 1 + i] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_solver() {
        let x = solve(vec![vec![2.0, 1.0], vec![1.0, 3.0]], vec![3.0, 5.0]).unwrap();
        assert!((x[0] - 0.8).abs() < 1e-15 && (x[1] - 1.4).abs() < 1e-15);
        assert!(solve(vec![vec![1.0, 2.0], vec![2.0, 4.0]], vec![1.0, 2.0]).is_none());
    }

    #[test]
    fn svd_reconstructs_and_is_orthogonal() {
        let a = Mat::from_rows(&[
            vec![4.0, 1.0, -2.0],
            vec![1.0, 3.0, 0.5],
            vec![0.0, -1.0, 2.0],
            vec![2.0, 2.0, 2.0],
        ]);
        let d = svd(&a);
        assert!(d.s.windows(2).all(|w| w[0] >= w[1]));
        let mut us = d.u.clone();
        for (k, &s) in d.s.iter().enumerate() {
            us.scale_col(k, s);
        }
        let back = us.mul(&d.v.transpose());
        for (x, y) in back.data.iter().zip(&a.data) {
            assert!((x - y).abs() < 1e-13, "{x} vs {y}");
        }
        let vtv = d.v.transpose().mul(&d.v);
        let utu = d.u.transpose().mul(&d.u);
        for i in 0..3 {
            for j in 0..3 {
                let e = if i == j { 1.0 } else { 0.0 };
                assert!((vtv.get(i, j) - e).abs() < 1e-14);
                assert!((utu.get(i, j) - e).abs() < 1e-14);
            }
        }
    }

    #[test]
    fn svd_finds_an_exact_null_direction() {
        // Column 2 = 2·column 0 − column 1: the null vector is (2, −1, −1)/√6.
        let rows: Vec<Vec<f64>> = (0..6)
            .map(|i| {
                let x = i as f64;
                let (a, b) = (1.0 + x, x * x - 3.0);
                vec![a, b, 2.0 * a - b]
            })
            .collect();
        let d = svd(&Mat::from_rows(&rows));
        assert!(d.s[2] < 1e-13 * d.s[0], "{:?}", d.s);
        let v = d.direction(2);
        let sign = v[0].signum();
        let want = [2.0, -1.0, -1.0].map(|x: f64| x / 6f64.sqrt());
        for (x, y) in v.iter().zip(want) {
            assert!((sign * x - y).abs() < 1e-12, "{v:?}");
        }
    }
}
