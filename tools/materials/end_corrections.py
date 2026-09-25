#!/usr/bin/env python3
"""Static (low-frequency) end corrections of circular apertures, computed
independently of the engine by a variational (Galerkin mode-matching) method.

Two geometries, both axisymmetric, incompressible (static) limit:

* step:    a duct of radius a (z < 0) joined coaxially to a duct of radius b
           (z > 0), alpha = a/b. The excess inertance of the evanescent fields
           on both sides is rho*delta/(pi a^2); `delta_step/a` is tabulated.
* orifice: a zero-thickness plate with a hole of radius a in a duct of radius
           b (both sides are duct b). By symmetry each side carries half the
           excess mass; `delta_side/a` is tabulated. For alpha -> 0 this tends
           to Rayleigh's pi/4 per side, and its ratio to pi/4 is the Fok
           interaction function.

Method: the axial velocity over the aperture is expanded in
f_m(rho) = (1 - rho^2)^(m - 1/2), m = 0..M-1 (the m = 0 term carries the
edge singularity). Each side's evanescent field is expanded in the duct modes
J0(g_n r/R), g_n the zeros of J1, and its kinetic energy is a quadratic form in
the aperture coefficients. Kelvin's minimum-energy theorem gives the excess
mass as min E / U^2 at fixed flux U, i.e. delta = pi a^2 / (F^T G^-1 F).
The projections use Sonine's integral
  int_0^1 (1-x^2)^mu J0(c x) x dx = 2^mu Gamma(mu+1) J_{mu+1}(c) / c^{mu+1}.

Convergence (M = 12..20 basis functions, 2e4..6e4 duct modes) is better than
1e-5 in delta/a, see the `--check` option.

Writes crates/acoustilab/tests/data/materials_end_corrections.json and prints
the polynomial fit used in crates/acoustilab/src/elements/ducts.rs.
"""
import json
import os
import sys

import numpy as np
from scipy.special import gamma, jn_zeros, jv
from scipy.special import j0 as J0

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "..", "crates", "acoustilab", "tests", "data",
                   "materials_end_corrections.json")


def proj(M, c):
    """int over the aperture of f_m(r/a) J0(c r/a) dA, divided by a^2."""
    rows = []
    for m in range(M):
        mu = m - 0.5
        rows.append(2 * np.pi * 2**mu * gamma(mu + 1) * jv(mu + 1, c) / c**(mu + 1))
    return np.array(rows)


def energy_matrix(M, a, R, N):
    """Kinetic-energy quadratic form of the evanescent field in a duct of
    radius R driven by the aperture velocity sum_m d_m f_m(r/a)."""
    g = jn_zeros(1, N)
    P = proj(M, g * a / R) * a * a
    norm = np.pi * R * R * J0(g) ** 2
    w = R / (g * norm)
    return (P * w) @ P.T


def delta(alpha, M=16, N=None, geometry="step"):
    a, b = alpha, 1.0
    if N is None:
        N = int(max(40000, 800 / max(alpha, 1e-3)))
    G = energy_matrix(M, a, b, N)
    if geometry == "step":
        G = G + energy_matrix(M, a, a, N)
    else:
        G = 2 * G
    F = np.array([np.pi * a * a / (m + 0.5) for m in range(M)])
    x = np.linalg.solve(G, F)
    d = np.pi * a * a / (F @ x) / a
    return d if geometry == "step" else d / 2


def fok(x):
    return (1 - 1.40925 * x + 0.33818 * x**3 + 0.06793 * x**5 - 0.02287 * x**6
            + 0.03015 * x**7 - 0.01641 * x**8)


def main():
    if "--check" in sys.argv:
        for al in [0.1, 0.5, 0.9]:
            for M in [12, 16, 20]:
                for N in [20000, 60000]:
                    print(al, M, N, delta(al, M, N), delta(al, M, N, "orifice"))
        return
    alphas = [round(0.025 * i, 3) for i in range(1, 39)]
    step = [delta(al) for al in alphas]
    orifice = [delta(al, geometry="orifice") for al in alphas]

    # Constrained least-squares fit of delta_step/a = 0.82159*(1-al)*(1 + sum c_k al^k)
    # (value at alpha = 0 from Norris & Sheng 1989, zero at alpha = 1).
    al = np.array(alphas)
    y = np.array(step) / (0.82159 * (1 - al)) - 1
    K = 6
    A = np.vstack([al**k for k in range(1, K + 1)]).T
    coef, *_ = np.linalg.lstsq(A, y, rcond=None)
    fit = 0.82159 * (1 - al) * (1 + A @ coef)
    err = np.max(np.abs(fit - np.array(step)))
    print("step fit coefficients c1..c6:", ", ".join(f"{c:.6f}" for c in coef))
    print(f"max |fit - exact| = {err:.2e} (delta/a)")
    for a_, s, o in zip(alphas, step, orifice):
        print(f"alpha {a_:.3f}  step {s:.6f}  orifice/side {o:.6f}  "
              f"ratio {o / (np.pi / 4):.6f}  fok {fok(a_):.6f}")
    data = {
        "description": "Static end corrections from tools/materials/end_corrections.py "
                       "(variational mode matching). delta_step_over_a: coaxial step "
                       "from radius a to radius b = a/alpha, both sides' evanescent "
                       "fields. delta_orifice_side_over_a: one side of a thin orifice "
                       "of radius a in a duct of radius b.",
        "alpha": alphas,
        "delta_step_over_a": step,
        "delta_orifice_side_over_a": orifice,
        "step_fit_coefficients": list(coef),
    }
    with open(OUT, "w") as f:
        json.dump(data, f, indent=1)
    print("wrote", os.path.normpath(OUT))


if __name__ == "__main__":
    main()
