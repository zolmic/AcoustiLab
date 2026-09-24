#!/usr/bin/env python3
"""mpmath references for crates/acoustilab/src/special.rs.

Writes, under crates/acoustilab/tests/data/:

* special_bessel_ratio.json: I_{nu+1}(z)/I_nu(z) for nu = 0, 1 and complex
  z = r e^{i theta}, r from 1e-4 to 1e4 (log spaced, plus points around the
  algorithm switches), theta in {0, pi/8, pi/4, 3pi/8, 0.49 pi}.
* special_shape.json: slit F = tanh(z)/z and circle F = 2 I1(z)/(z I0(z)),
  and 1 - F (= I2/I0 for the circle), at z = sqrt(j) x for x from 1e-4 to
  1e4. Viscous usage needs 1 - F(k_v s), thermal usage F(k_t s); both have
  arguments of this form, k_t = k_v sqrt(Pr).
* special_piston.json: J1(x), H1(x) (Struve), R1(x) = 1 - 2 J1(x)/x and
  X1(x) = 2 H1(x)/x for x in [0, 500] (step 0.25, plus points within 1e-9
  relative of every zero of J1), with x J1'(x) and x H1'(x).

All inputs are doubles; mpmath evaluates at their exact binary values with
40 and 60 significant digits and the two must agree to 1e-30.

Usage: python3 tools/refgen/special_refs.py
"""

import math

import mpmath as mp
import numpy as np

from common import mpc_of, s, stable, write

GEN = "tools/refgen/special_refs.py"


def radii():
    r = list(np.logspace(-4, 4, 8 * 9 + 1))
    # Around the switches of special.rs (continued fraction / asymptotics).
    for c in (12.0, 17.0, 18.0, 20.0, 25.0, 30.0, 60.0):
        r += [c * (1 - 1e-9), c, c * (1 + 1e-9)]
    return sorted(set(float(x) for x in r))


def bessel_ratio_rows():
    rows = []
    thetas = [0.0, math.pi / 8, math.pi / 4, 3 * math.pi / 8, 0.49 * math.pi]
    for nu in (0, 1):
        for th in thetas:
            for r in radii():
                zr, zi = r * math.cos(th), r * math.sin(th)
                if th == 0.0:
                    zi = 0.0

                def f():
                    z = mpc_of(zr, zi)
                    return mp.besseli(nu + 1, z) / mp.besseli(nu, z)

                v = stable(f)
                rows.append([str(nu), s(zr), s(zi), s(v.real), s(v.imag)])
    return rows


def shape_rows():
    rows = []
    xs = sorted(set(float(x) for x in np.logspace(-4, 4, 8 * 12 + 1)))
    xs += [0.2 * (1 - 1e-9), 0.2 * (1 + 1e-9), 0.25, 0.5, 1.0, 2.0, 3.0]
    for x in sorted(set(xs)):
        h = x * math.sqrt(0.5)

        def f():
            z = mpc_of(h, h)
            slit = mp.tanh(z) / z
            i0, i1, i2 = mp.besseli(0, z), mp.besseli(1, z), mp.besseli(2, z)
            circ = 2 * i1 / (z * i0)
            return [slit, 1 - slit, circ, i2 / i0]

        v = stable(f)
        rows.append([s(x), s(h), s(h)] + [t for c in v for t in (s(c.real), s(c.imag))])
    return rows


def piston_rows():
    xs = [0.0] + list(np.logspace(-4, 0, 17)) + list(np.arange(0.25, 500.0001, 0.25))
    xs += [2.0 * (1 - 1e-12), 2.0 * (1 + 1e-12), 25.0 * (1 - 1e-12), 25.0 * (1 + 1e-12)]
    # Just beside every zero of J1 below 500, where only an algorithm with
    # accurate argument reduction keeps relative accuracy.
    k = 1
    while True:
        z = float(mp.besseljzero(1, k))
        if z > 500:
            break
        xs += [z * (1 - 1e-9), z * (1 + 1e-9)]
        k += 1
    rows = []
    for x in sorted(set(float(t) for t in xs)):
        if x == 0.0:
            rows.append(["0.0", "0.0", "0.0", "0.0", "0.0", "0.0", "0.0"])
            continue

        def f():
            xm = mp.mpf(x)
            j1 = mp.besselj(1, xm)
            h1 = mp.struveh(1, xm)
            # x·J1'(x) and x·H1'(x) give each function's condition number.
            dj1 = xm * (mp.besselj(0, xm) - j1 / xm)
            dh1 = xm * (mp.struveh(0, xm) - h1 / xm)
            return [j1, h1, 1 - 2 * j1 / xm, 2 * h1 / xm, dj1, dh1]

        v = stable(f)
        rows.append([s(x)] + [s(t) for t in v])
    return rows


def main():
    meta = {
        "generator": GEN,
        "mpmath": mp.__version__,
        "precision": "mpmath at 40 and 60 digits, agreeing to 1e-30",
    }
    write(
        "special_bessel_ratio.json",
        dict(meta, description="I_{nu+1}(z)/I_nu(z), complex z"),
        ["nu", "z_re", "z_im", "ratio_re", "ratio_im"],
        bessel_ratio_rows(),
    )
    write(
        "special_shape.json",
        dict(
            meta,
            description="shape functions at z = sqrt(j) x: slit tanh(z)/z, "
            "circle 2 I1/(z I0), and their complements (1 - F_circle = I2/I0)",
        ),
        [
            "x", "z_re", "z_im",
            "slit_f_re", "slit_f_im", "slit_om_re", "slit_om_im",
            "circle_f_re", "circle_f_im", "circle_om_re", "circle_om_im",
        ],
        shape_rows(),
    )
    write(
        "special_piston.json",
        dict(
            meta,
            description="J1, Struve H1, R1 = 1 - 2 J1(x)/x, X1 = 2 H1(x)/x, "
            "and x J1'(x), x H1'(x) (condition numbers) on [0, 500]",
        ),
        ["x", "j1", "h1", "r1", "x1", "x_dj1", "x_dh1"],
        piston_rows(),
    )


if __name__ == "__main__":
    main()
