"""Reference data for the fitting tests (crates/acoustilab/tests/fit.rs).

An independent implementation of the two numerical cores of
`acoustilab::fit` with numpy and scipy:

1. Weighted nonlinear least squares with a scaled covariance. The model is
   the level of a second-order resonance,
       L(f) = 20 log10( A / sqrt((1 - x^2)^2 + (x/Q)^2) ),  x = f/f0,
   fitted to noisy levels with a stated standard uncertainty sigma (dB) by
   scipy.optimize.curve_fit (method "lm", analytic Jacobian,
   absolute_sigma=False), so pcov = (J^T W J)^-1 * chi2/(m - n). The noise
   is drawn larger than sigma, so chi2/(m - n) > 1 and the engine's
   max(chi2_nu, 1) scale equals scipy's. The Rust test fits the same data
   in ln-parameter space and compares the optimum and the covariance of
   ln(A), ln(f0), ln(Q): cov_ln[i][j] = pcov[i][j] / (p_i p_j).

2. The singular value decomposition of a fixed matrix with a nearly null
   direction, by numpy.linalg.svd (LAPACK gesdd).

Numbers are stored as repr() strings, which round-trip exactly; the Rust
tests parse them with str::parse::<f64>.

    python3 tools/fit/reference.py
"""

import json
import os

import numpy as np
import scipy
from scipy.optimize import curve_fit

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(REPO, "crates", "acoustilab", "tests", "data", "fit", "reference.json")


def s(x):
    return repr(float(x))


def level(f, a, f0, q):
    x = f / f0
    return 20.0 * np.log10(a / np.sqrt((1.0 - x * x) ** 2 + (x / q) ** 2))


def level_jac(f, a, f0, q):
    x = f / f0
    d = (1.0 - x * x) ** 2 + (x / q) ** 2
    k = 20.0 / np.log(10.0)
    # dL/da, dL/df0, dL/dq
    da = k / a
    dd_dx = 2.0 * (1.0 - x * x) * (-2.0 * x) + 2.0 * x / (q * q)
    dx_df0 = -x / f0
    df0 = -0.5 * k / d * dd_dx * dx_df0
    dd_dq = -2.0 * x * x / q ** 3
    dq = -0.5 * k / d * dd_dq
    return np.stack([np.full_like(f, da), df0, dq], axis=1)


def least_squares_case():
    rng = np.random.default_rng(20260925)
    f = np.geomspace(10.0, 1000.0, 25)
    truth = (2.0, 100.0, 3.0)
    sigma = 0.2
    y = level(f, *truth) + rng.normal(0.0, 0.35, f.size)
    p0 = (1.5, 80.0, 2.0)
    popt, pcov = curve_fit(
        level,
        f,
        y,
        p0=p0,
        sigma=np.full_like(f, sigma),
        absolute_sigma=False,
        jac=level_jac,
        method="lm",
        xtol=1e-15,
        ftol=1e-15,
        gtol=1e-15,
        maxfev=100000,
    )
    r = (level(f, *popt) - y) / sigma
    chi2 = float(r @ r)
    dof = f.size - 3
    cov_ln = pcov / np.outer(popt, popt)
    return {
        "model": "L(f) = 20 log10(A / sqrt((1 - x^2)^2 + (x/Q)^2)), x = f/f0",
        "sigma_dB": s(sigma),
        "start": [s(v) for v in p0],
        "frequencies_Hz": [s(v) for v in f],
        "level_dB": [s(v) for v in y],
        "popt": [s(v) for v in popt],
        "chi2": s(chi2),
        "dof": dof,
        "cov_ln": [[s(v) for v in row] for row in cov_ln],
    }


def svd_case():
    a = np.array(
        [
            [1.0, 2.0, 3.0 + 1e-7, 0.5],
            [2.0, 1.0, 3.0, -1.0],
            [0.5, 0.25, 0.75, 2.0],
            [3.0, -1.0, 2.0, 0.0],
            [1.5, 1.5, 3.0 - 1e-7, 1.0],
            [-1.0, 4.0, 3.0, 0.3],
            [0.0, 0.1, 0.1, -0.7],
        ]
    )
    u, sv, vt = np.linalg.svd(a, full_matrices=False)
    return {
        "matrix": [[s(v) for v in row] for row in a],
        "singular_values": [s(v) for v in sv],
        "v": [[s(v) for v in row] for row in vt.T],
    }


def main():
    doc = {
        "generator": "tools/fit/reference.py",
        "numpy": np.__version__,
        "scipy": scipy.__version__,
        "least_squares": least_squares_case(),
        "svd": svd_case(),
    }
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w") as fh:
        json.dump(doc, fh, indent=1)
        fh.write("\n")
    print("wrote", OUT)


if __name__ == "__main__":
    main()
