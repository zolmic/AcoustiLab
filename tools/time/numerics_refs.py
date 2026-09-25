#!/usr/bin/env python3
"""Reference values for the time-domain numerics (crates/acoustilab/tests/time.rs).

Writes crates/acoustilab/tests/data/time_numerics.json.

* FFT: numpy.fft.fft (pocketfft) of the deterministic complex sequence
  x_n = cos(0.001 n^2 + 0.3 n) + j sin(0.7 n) exp(-n/3000), for N = 256
  (every bin) and N = 8192 (every 16th bin). The Rust test rebuilds the
  same input and compares its radix-2 transform, forward and inverse.
* Eigenvalues: numpy.linalg.eigvals (LAPACK dgeev) of two real matrices:
  a dense 30 x 30 matrix A_ij = sin(7i + 3j + 1) + i*[i == j]/10, and the
  pole-relocation matrix of vector fitting, Lambda - b c^T / d, for 12
  pairs log-spaced from 20 Hz to 20 kHz (Re = -Im/100) with
  c = (sin(k), cos(2k)) per pair and d = 0.7.

Numbers are written with repr(), which round-trips to the same double.

Run from anywhere:  python3 tools/time/numerics_refs.py
"""

import json
from pathlib import Path

import numpy as np

OUT = Path(__file__).resolve().parents[2] / "crates/acoustilab/tests/data/time_numerics.json"


def sequence(n):
    k = np.arange(n, dtype=float)
    return np.cos(0.001 * k * k + 0.3 * k) + 1j * np.sin(0.7 * k) * np.exp(-k / 3000.0)


def fft_case(n, stride):
    x = sequence(n)
    y = np.fft.fft(x)
    bins = list(range(0, n, stride))
    return {
        "n": n,
        "bins": bins,
        "re": [float(y[b].real) for b in bins],
        "im": [float(y[b].imag) for b in bins],
    }


def dense_matrix():
    n = 30
    a = np.empty((n, n))
    for i in range(n):
        for j in range(n):
            a[i, j] = np.sin(7 * i + 3 * j + 1) + (i / 10.0 if i == j else 0.0)
    return a


def vf_matrix():
    pairs = 12
    beta = 2 * np.pi * 20.0 * (1000.0 ** (np.arange(pairs) / (pairs - 1)))
    n = 2 * pairs
    lam = np.zeros((n, n))
    b = np.zeros(n)
    c = np.zeros(n)
    for k in range(pairs):
        re, im = -beta[k] / 100.0, beta[k]
        i = 2 * k
        lam[i, i], lam[i, i + 1] = re, im
        lam[i + 1, i], lam[i + 1, i + 1] = -im, re
        b[i] = 2.0
        c[i], c[i + 1] = np.sin(k + 1.0), np.cos(2.0 * (k + 1.0))
    return lam - np.outer(b, c) * beta.max() / 0.7


def eig_case(a):
    w = np.linalg.eigvals(a)
    w = sorted(w, key=lambda z: (round(z.real, 6), z.imag))
    return {
        "n": a.shape[0],
        "a": [float(v) for v in a.ravel()],
        "re": [float(z.real) for z in w],
        "im": [float(z.imag) for z in w],
    }


def main():
    data = {
        "generator": "tools/time/numerics_refs.py",
        "numpy": np.__version__,
        "fft": [fft_case(256, 1), fft_case(8192, 16)],
        "eig": [eig_case(dense_matrix()), eig_case(vf_matrix())],
    }
    OUT.write_text(json.dumps(data, indent=1) + "\n")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
