//! Dense real linear algebra for vector fitting: least squares by
//! Householder QR, and eigenvalues of a general real matrix.
//!
//! * [`Qr`]: Householder QR of an m×n matrix (m ≥ n) applied to right-hand
//!   sides at the same time, so a caller can keep R and Qᵀb (the fast
//!   vector-fitting reduction needs the trailing block of R).
//! * [`lstsq`]: min ‖Ax − b‖ with columns scaled to unit norm first (the
//!   vector-fitting columns differ by many orders of magnitude).
//! * [`eigenvalues`]: balancing, reduction to upper Hessenberg form by
//!   stabilised elementary similarity transforms, and the Francis
//!   double-shift QR iteration (EISPACK `balanc`/`elmhes`/`hqr`, in the form
//!   of Press et al., Numerical Recipes, 2nd ed., §11.5–11.6). Complex
//!   eigenvalues come out as exact conjugate pairs.
//!
//! Matrices are row-major `Vec<f64>`.

use crate::C64;

/// Householder QR of a row-major m×n matrix, in place.
pub struct Qr {
    pub m: usize,
    pub n: usize,
    /// Row-major m×n; after [`Qr::factor`] its upper triangle holds R.
    pub a: Vec<f64>,
}

impl Qr {
    /// Factors `a` (m×n, m ≥ n) and applies Qᵀ to every vector in `rhs`.
    pub fn factor(m: usize, n: usize, mut a: Vec<f64>, rhs: &mut [&mut Vec<f64>]) -> Qr {
        assert!(m >= n && a.len() == m * n);
        let mut v = vec![0.0; m];
        for j in 0..n {
            let norm = (j..m).map(|i| a[i * n + j].powi(2)).sum::<f64>().sqrt();
            if norm == 0.0 {
                continue;
            }
            let x0 = a[j * n + j];
            let alpha = if x0 >= 0.0 { -norm } else { norm };
            for i in j..m {
                v[i] = a[i * n + j];
            }
            v[j] -= alpha;
            let vnorm2: f64 = (j..m).map(|i| v[i] * v[i]).sum();
            if vnorm2 == 0.0 {
                continue;
            }
            let beta = 2.0 / vnorm2;
            for c in j..n {
                let s: f64 = (j..m).map(|i| v[i] * a[i * n + c]).sum();
                let s = s * beta;
                for i in j..m {
                    a[i * n + c] -= s * v[i];
                }
            }
            for b in rhs.iter_mut() {
                let s: f64 = (j..m).map(|i| v[i] * b[i]).sum();
                let s = s * beta;
                for i in j..m {
                    b[i] -= s * v[i];
                }
            }
            a[j * n + j] = alpha;
            for i in j + 1..m {
                a[i * n + j] = 0.0;
            }
        }
        Qr { m, n, a }
    }

    pub fn r(&self, i: usize, j: usize) -> f64 {
        self.a[i * self.n + j]
    }

    /// Solves R x = y[0..n] by back substitution. A diagonal entry below
    /// `1e-14` of the largest one is treated as a rank deficiency: its
    /// unknown is set to zero (the basic least-squares solution).
    pub fn back_substitute(&self, y: &[f64]) -> Vec<f64> {
        let n = self.n;
        let dmax = (0..n).map(|i| self.r(i, i).abs()).fold(0.0, f64::max);
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            let d = self.r(i, i);
            if d.abs() <= 1e-14 * dmax {
                continue;
            }
            let s: f64 = (i + 1..n).map(|j| self.r(i, j) * x[j]).sum();
            x[i] = (y[i] - s) / d;
        }
        x
    }
}

/// Least-squares solution of the row-major m×n system A x ≈ b, with the
/// columns first scaled to unit Euclidean norm (and the solution unscaled).
pub fn lstsq(m: usize, n: usize, mut a: Vec<f64>, b: &[f64]) -> Vec<f64> {
    let scale: Vec<f64> = (0..n)
        .map(|j| {
            let s = (0..m).map(|i| a[i * n + j].powi(2)).sum::<f64>().sqrt();
            if s > 0.0 {
                1.0 / s
            } else {
                1.0
            }
        })
        .collect();
    for i in 0..m {
        for j in 0..n {
            a[i * n + j] *= scale[j];
        }
    }
    let mut y = b.to_vec();
    let qr = Qr::factor(m, n, a, &mut [&mut y]);
    let x = qr.back_substitute(&y);
    x.iter().zip(&scale).map(|(x, s)| x * s).collect()
}

/// Eigenvalues of a real n×n row-major matrix.
pub fn eigenvalues(mut a: Vec<f64>, n: usize) -> Result<Vec<C64>, String> {
    assert_eq!(a.len(), n * n);
    if n == 0 {
        return Ok(Vec::new());
    }
    if a.iter().any(|x| !x.is_finite()) {
        return Err("eigenvalues: the matrix has non-finite entries".into());
    }
    balance(&mut a, n);
    hessenberg(&mut a, n);
    hqr(&mut a, n)
}

/// Parlett–Reinsch balancing by powers of 2 (similarity; exact in binary).
fn balance(a: &mut [f64], n: usize) {
    const RADIX: f64 = 2.0;
    let sqrdx = RADIX * RADIX;
    let mut done = false;
    while !done {
        done = true;
        for i in 0..n {
            let (mut r, mut c) = (0.0, 0.0);
            for j in 0..n {
                if j != i {
                    c += a[j * n + i].abs();
                    r += a[i * n + j].abs();
                }
            }
            if c != 0.0 && r != 0.0 {
                let mut g = r / RADIX;
                let mut f = 1.0;
                let s = c + r;
                while c < g {
                    f *= RADIX;
                    c *= sqrdx;
                }
                g = r * RADIX;
                while c > g {
                    f /= RADIX;
                    c /= sqrdx;
                }
                if (c + r) / f < 0.95 * s {
                    done = false;
                    let g = 1.0 / f;
                    for j in 0..n {
                        a[i * n + j] *= g;
                    }
                    for j in 0..n {
                        a[j * n + i] *= f;
                    }
                }
            }
        }
    }
}

/// Reduction to upper Hessenberg form by Gaussian elimination with
/// pivoting (similarity transforms); entries below the subdiagonal are
/// cleared afterwards.
fn hessenberg(a: &mut [f64], n: usize) {
    for m in 1..n.saturating_sub(1) {
        let mut x = 0.0f64;
        let mut piv = m;
        for j in m..n {
            if a[j * n + m - 1].abs() > x.abs() {
                x = a[j * n + m - 1];
                piv = j;
            }
        }
        if piv != m {
            for j in m - 1..n {
                a.swap(piv * n + j, m * n + j);
            }
            for j in 0..n {
                a.swap(j * n + piv, j * n + m);
            }
        }
        if x != 0.0 {
            for i in m + 1..n {
                let mut y = a[i * n + m - 1];
                if y != 0.0 {
                    y /= x;
                    a[i * n + m - 1] = y;
                    for j in m..n {
                        a[i * n + j] -= y * a[m * n + j];
                    }
                    for j in 0..n {
                        a[j * n + m] += y * a[j * n + i];
                    }
                }
            }
        }
    }
    for i in 2..n {
        for j in 0..i - 1 {
            a[i * n + j] = 0.0;
        }
    }
}

fn sign(a: f64, b: f64) -> f64 {
    if b >= 0.0 {
        a.abs()
    } else {
        -a.abs()
    }
}

/// Francis double-shift QR on an upper Hessenberg matrix (Numerical
/// Recipes `hqr`, written with 1-based indices to follow the reference).
#[allow(clippy::many_single_char_names)]
fn hqr(h: &mut [f64], n: usize) -> Result<Vec<C64>, String> {
    let ni = n as isize;
    let at = |i: isize, j: isize| ((i - 1) * ni + (j - 1)) as usize;
    let mut wr = vec![0.0; n + 1];
    let mut wi = vec![0.0; n + 1];
    let mut anorm = 0.0;
    for i in 1..=ni {
        for j in (i - 1).max(1)..=ni {
            anorm += h[at(i, j)].abs();
        }
    }
    let mut nn = ni;
    let mut t = 0.0;
    let (mut p, mut q, mut r, mut s, mut w, mut x, mut y, mut z);
    while nn >= 1 {
        let mut its = 0;
        loop {
            let mut l = nn;
            while l >= 2 {
                s = h[at(l - 1, l - 1)].abs() + h[at(l, l)].abs();
                if s == 0.0 {
                    s = anorm;
                }
                if h[at(l, l - 1)].abs() + s == s {
                    h[at(l, l - 1)] = 0.0;
                    break;
                }
                l -= 1;
            }
            x = h[at(nn, nn)];
            if l == nn {
                wr[nn as usize] = x + t;
                wi[nn as usize] = 0.0;
                nn -= 1;
            } else {
                y = h[at(nn - 1, nn - 1)];
                w = h[at(nn, nn - 1)] * h[at(nn - 1, nn)];
                if l == nn - 1 {
                    p = 0.5 * (y - x);
                    q = p * p + w;
                    z = q.abs().sqrt();
                    x += t;
                    let (a, b) = ((nn - 1) as usize, nn as usize);
                    if q >= 0.0 {
                        z = p + sign(z, p);
                        wr[a] = x + z;
                        wr[b] = x + z;
                        if z != 0.0 {
                            wr[b] = x - w / z;
                        }
                        wi[a] = 0.0;
                        wi[b] = 0.0;
                    } else {
                        wr[a] = x + p;
                        wr[b] = x + p;
                        wi[a] = -z;
                        wi[b] = z;
                    }
                    nn -= 2;
                } else {
                    if its == 60 {
                        return Err("eigenvalues: QR iteration did not converge".into());
                    }
                    if its == 10 || its == 20 || its == 40 {
                        // Exceptional shift.
                        t += x;
                        for i in 1..=nn {
                            h[at(i, i)] -= x;
                        }
                        s = h[at(nn, nn - 1)].abs() + h[at(nn - 1, nn - 2)].abs();
                        x = 0.75 * s;
                        y = x;
                        w = -0.4375 * s * s;
                    }
                    its += 1;
                    let mut m = nn - 2;
                    loop {
                        z = h[at(m, m)];
                        r = x - z;
                        s = y - z;
                        p = (r * s - w) / h[at(m + 1, m)] + h[at(m, m + 1)];
                        q = h[at(m + 1, m + 1)] - z - r - s;
                        r = h[at(m + 2, m + 1)];
                        s = p.abs() + q.abs() + r.abs();
                        p /= s;
                        q /= s;
                        r /= s;
                        if m == l {
                            break;
                        }
                        let u = h[at(m, m - 1)].abs() * (q.abs() + r.abs());
                        let v = p.abs()
                            * (h[at(m - 1, m - 1)].abs() + z.abs() + h[at(m + 1, m + 1)].abs());
                        if u + v == v {
                            break;
                        }
                        m -= 1;
                    }
                    for i in m + 2..=nn {
                        h[at(i, i - 2)] = 0.0;
                        if i != m + 2 {
                            h[at(i, i - 3)] = 0.0;
                        }
                    }
                    let mut k = m;
                    while k < nn {
                        if k != m {
                            p = h[at(k, k - 1)];
                            q = h[at(k + 1, k - 1)];
                            r = 0.0;
                            if k != nn - 1 {
                                r = h[at(k + 2, k - 1)];
                            }
                            x = p.abs() + q.abs() + r.abs();
                            if x != 0.0 {
                                p /= x;
                                q /= x;
                                r /= x;
                            }
                        }
                        s = sign((p * p + q * q + r * r).sqrt(), p);
                        if s != 0.0 {
                            if k == m {
                                if l != m {
                                    h[at(k, k - 1)] = -h[at(k, k - 1)];
                                }
                            } else {
                                h[at(k, k - 1)] = -s * x;
                            }
                            p += s;
                            x = p / s;
                            y = q / s;
                            z = r / s;
                            q /= p;
                            r /= p;
                            for j in k..=nn {
                                p = h[at(k, j)] + q * h[at(k + 1, j)];
                                if k != nn - 1 {
                                    p += r * h[at(k + 2, j)];
                                    h[at(k + 2, j)] -= p * z;
                                }
                                h[at(k + 1, j)] -= p * y;
                                h[at(k, j)] -= p * x;
                            }
                            let mmin = if nn < k + 3 { nn } else { k + 3 };
                            for i in l..=mmin {
                                p = x * h[at(i, k)] + y * h[at(i, k + 1)];
                                if k != nn - 1 {
                                    p += z * h[at(i, k + 2)];
                                    h[at(i, k + 2)] -= p * r;
                                }
                                h[at(i, k + 1)] -= p * q;
                                h[at(i, k)] -= p;
                            }
                        }
                        k += 1;
                    }
                }
            }
            if l >= nn - 1 {
                break;
            }
        }
    }
    Ok((1..=n).map(|i| C64::new(wr[i], wi[i])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<C64>) -> Vec<C64> {
        v.sort_by(|a, b| a.re.total_cmp(&b.re).then(a.im.total_cmp(&b.im)));
        v
    }

    #[test]
    fn eigenvalues_of_known_matrices() {
        // Companion matrix of (x − 1)(x − 2)(x² + 2x + 5): roots 1, 2, −1 ± 2j.
        // x⁴ − x³ + x² − 11x + 10 … expanded: (x² − 3x + 2)(x² + 2x + 5)
        // = x⁴ − x³ + x² − 11x + 10.
        let a = vec![
            1.0, -1.0, 11.0, -10.0, //
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0,
        ];
        let e = sorted(eigenvalues(a, 4).unwrap());
        let want = [
            C64::new(-1.0, -2.0),
            C64::new(-1.0, 2.0),
            C64::new(1.0, 0.0),
            C64::new(2.0, 0.0),
        ];
        for (a, b) in e.iter().zip(&want) {
            assert!((a - b).norm() < 1e-12, "{a} vs {b}");
        }
        // Real block form of a complex pair and a real pole.
        let a = vec![-3.0, 40.0, 0.0, -40.0, -3.0, 0.0, 0.0, 0.0, -7.0];
        let e = sorted(eigenvalues(a, 3).unwrap());
        assert!((e[0] - C64::new(-7.0, 0.0)).norm() < 1e-12);
        assert!((e[1] - C64::new(-3.0, -40.0)).norm() < 1e-12);
        assert!((e[2] - C64::new(-3.0, 40.0)).norm() < 1e-12);
    }

    #[test]
    fn least_squares_recovers_exact_solution() {
        // Overdetermined, consistent, badly scaled columns.
        let (m, n) = (6, 3);
        let x_true = [2.0, -1e-5, 3e4];
        let mut a = vec![0.0; m * n];
        for i in 0..m {
            let t = i as f64 + 1.0;
            a[i * n] = 1.0;
            a[i * n + 1] = 1e5 * t;
            a[i * n + 2] = 1e-4 / t;
        }
        let b: Vec<f64> = (0..m)
            .map(|i| (0..n).map(|j| a[i * n + j] * x_true[j]).sum())
            .collect();
        let x = lstsq(m, n, a, &b);
        for (a, b) in x.iter().zip(&x_true) {
            assert!((a - b).abs() <= 1e-10 * b.abs(), "{a} vs {b}");
        }
    }
}
