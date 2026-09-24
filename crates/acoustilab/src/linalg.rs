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

/// Solves A x = b. `a` is consumed as workspace. Returns x.
#[allow(clippy::needless_range_loop)]
pub fn solve(mut a: Matrix, b: &[C64]) -> Result<Vec<C64>, SingularColumn> {
    let n = a.n;
    assert_eq!(b.len(), n);
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut b = b.to_vec();

    // Equilibrate: rows, then columns, to unit max-abs.
    let mut row_scale = vec![1.0; n];
    for (r, s) in row_scale.iter_mut().enumerate() {
        let m = (0..n).map(|c| a.get(r, c).norm()).fold(0.0, f64::max);
        if m == 0.0 {
            return Err(SingularColumn(r));
        }
        *s = 1.0 / m;
        for c in 0..n {
            a.data[r * n + c] *= *s;
        }
        b[r] *= *s;
    }
    let mut col_scale = vec![1.0; n];
    for (c, s) in col_scale.iter_mut().enumerate() {
        let m = (0..n).map(|r| a.get(r, c).norm()).fold(0.0, f64::max);
        if m == 0.0 {
            return Err(SingularColumn(c));
        }
        *s = 1.0 / m;
        for r in 0..n {
            a.data[r * n + c] *= *s;
        }
    }

    // LU with partial pivoting, in place; perm tracks row swaps.
    let tiny = 1e-14;
    for k in 0..n {
        let (p, pmax) = (k..n)
            .map(|r| (r, a.get(r, k).norm()))
            .fold((k, -1.0), |acc, x| if x.1 > acc.1 { x } else { acc });
        if pmax <= tiny {
            return Err(SingularColumn(k));
        }
        if p != k {
            for c in 0..n {
                a.data.swap(k * n + c, p * n + c);
            }
            b.swap(k, p);
        }
        let pivot = a.get(k, k);
        let inv = pivot.inv();
        for r in (k + 1)..n {
            let l = a.data[r * n + k] * inv;
            if l == C64::new(0.0, 0.0) {
                continue;
            }
            a.data[r * n + k] = l;
            for c in (k + 1)..n {
                let akc = a.data[k * n + c];
                a.data[r * n + c] -= l * akc;
            }
            let bk = b[k];
            b[r] -= l * bk;
        }
    }
    // Back substitution.
    let mut y = vec![C64::new(0.0, 0.0); n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for c in (r + 1)..n {
            s -= a.data[r * n + c] * y[c];
        }
        y[r] = s / a.data[r * n + r];
    }
    Ok(y.iter().zip(&col_scale).map(|(yi, s)| yi * *s).collect())
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
}
