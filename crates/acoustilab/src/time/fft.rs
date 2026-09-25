//! Radix-2 complex FFT and the real-signal helpers built on it.
//!
//! An iterative decimation-in-time Cooley–Tukey transform over a
//! power-of-two length. Twiddle factors are evaluated directly as
//! cos/sin(2πk/N) for every k (not by recurrence), so the transform error
//! grows only like ε·log2 N; against numpy's pocketfft it agrees to ~1e-15
//! of the largest output (`tests/time.rs`). The engine needs lengths of 2^8
//! to 2^20 (IR buffers up to 16384, cepstra padded 8×), where a radix-2
//! kernel is fast enough and keeps the engine free of dependencies on every
//! target, wasm32 included.
//!
//! Conventions: `forward` computes X_k = Σ_n x_n·e^{−j2πkn/N}; `inverse`
//! computes x_n = (1/N)·Σ_k X_k·e^{+j2πkn/N}. With the engine's e^{+jωt}
//! time convention, the spectrum of a real impulse response h[n] sampled at
//! fs is H(f_k) = Σ_n h[n]·e^{−j2πkn/N}, f_k = k·fs/N.

use crate::C64;
use std::f64::consts::PI;

/// A planned transform of one length.
#[derive(Debug, Clone)]
pub struct Fft {
    n: usize,
    /// e^{−j2πk/N}, k = 0..N/2.
    twiddles: Vec<C64>,
}

impl Fft {
    /// Plans a transform of length `n`, which must be a power of two.
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two(), "FFT length {n} is not a power of two");
        let twiddles = (0..n / 2)
            .map(|k| {
                let (s, c) = (-2.0 * PI * k as f64 / n as f64).sin_cos();
                C64::new(c, s)
            })
            .collect();
        Fft { n, twiddles }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// In-place forward transform.
    pub fn forward(&self, x: &mut [C64]) {
        self.transform(x, false);
    }

    /// In-place inverse transform, including the 1/N factor.
    pub fn inverse(&self, x: &mut [C64]) {
        self.transform(x, true);
        let k = 1.0 / self.n as f64;
        x.iter_mut().for_each(|v| *v *= k);
    }

    fn transform(&self, x: &mut [C64], inverse: bool) {
        let n = self.n;
        assert_eq!(x.len(), n, "FFT buffer length");
        if n <= 1 {
            return;
        }
        // Bit-reversal permutation.
        let bits = n.trailing_zeros();
        for i in 0..n {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if j > i {
                x.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let stride = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let w = self.twiddles[k * stride];
                    let w = if inverse { w.conj() } else { w };
                    let a = x[start + k];
                    let b = x[start + k + half] * w;
                    x[start + k] = a + b;
                    x[start + k + half] = a - b;
                }
            }
            len *= 2;
        }
    }
}

/// Forward transform of a copy of `x`.
pub fn fft(x: &[C64]) -> Vec<C64> {
    let mut y = x.to_vec();
    Fft::new(y.len()).forward(&mut y);
    y
}

/// Inverse transform (with 1/N) of a copy of `x`.
pub fn ifft(x: &[C64]) -> Vec<C64> {
    let mut y = x.to_vec();
    Fft::new(y.len()).inverse(&mut y);
    y
}

/// The full N-point spectrum of a real signal from its half spectrum
/// X_0..X_{N/2} (N/2 + 1 values), by Hermitian symmetry X_{N−k} = conj(X_k).
/// The DC and Nyquist values must already be real; their imaginary parts
/// are dropped.
pub fn hermitian_full(half: &[C64]) -> Vec<C64> {
    assert!(half.len() >= 2, "a half spectrum needs DC and Nyquist");
    let n = 2 * (half.len() - 1);
    let mut full = vec![C64::new(0.0, 0.0); n];
    full[0] = C64::new(half[0].re, 0.0);
    full[n / 2] = C64::new(half[n / 2].re, 0.0);
    for k in 1..n / 2 {
        full[k] = half[k];
        full[n - k] = half[k].conj();
    }
    full
}

/// Real signal of length N = 2·(len − 1) from a half spectrum (see
/// [`hermitian_full`]).
pub fn irfft(half: &[C64]) -> Vec<f64> {
    let mut full = hermitian_full(half);
    Fft::new(full.len()).inverse(&mut full);
    full.iter().map(|v| v.re).collect()
}

/// Half spectrum X_0..X_{N/2} of a real signal of power-of-two length N.
pub fn rfft(x: &[f64]) -> Vec<C64> {
    let mut y: Vec<C64> = x.iter().map(|&v| C64::new(v, 0.0)).collect();
    Fft::new(y.len()).forward(&mut y);
    y.truncate(x.len() / 2 + 1);
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Direct O(N²) DFT for comparison.
    fn dft(x: &[C64]) -> Vec<C64> {
        let n = x.len();
        (0..n)
            .map(|k| {
                x.iter()
                    .enumerate()
                    .map(|(m, v)| {
                        let (s, c) = (-2.0 * PI * ((k * m) % n) as f64 / n as f64).sin_cos();
                        v * C64::new(c, s)
                    })
                    .sum()
            })
            .collect()
    }

    #[test]
    fn matches_direct_dft_and_inverts() {
        for n in [1usize, 2, 4, 8, 64, 256] {
            let x: Vec<C64> = (0..n)
                .map(|i| C64::new((i as f64 * 0.37).sin() + 0.1, (i as f64 * 1.3).cos()))
                .collect();
            let y = fft(&x);
            let d = dft(&x);
            let scale = d.iter().map(|v| v.norm()).fold(0.0, f64::max).max(1.0);
            for (a, b) in y.iter().zip(&d) {
                assert!((a - b).norm() < 1e-13 * scale, "n = {n}");
            }
            let z = ifft(&y);
            for (a, b) in z.iter().zip(&x) {
                assert!((a - b).norm() < 1e-14, "n = {n}");
            }
        }
    }

    #[test]
    fn real_round_trip() {
        let x: Vec<f64> = (0..32).map(|i| ((i * i) as f64 * 0.1).sin()).collect();
        let half = rfft(&x);
        assert_eq!(half.len(), 17);
        assert!(half[0].im.abs() < 1e-14 && half[16].im.abs() < 1e-14);
        let y = irfft(&half);
        for (a, b) in x.iter().zip(&y) {
            assert!((a - b).abs() < 1e-14);
        }
    }
}
