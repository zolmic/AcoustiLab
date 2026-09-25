#!/usr/bin/env python3
"""Independent mpmath model of thermoviscous ducts (crates/acoustilab/src/
thermoviscous.rs), spec Section 6 with Appendix D corrected (errata E1, E2).

Writes, under crates/acoustilab/tests/data/:

* thermo_duct.json: circular tubes, slits and rectangles (aspect 2.5),
  Zwikker–Kosten / Stinson (1991) low-reduced-frequency model evaluated
  directly from Bessel I, tanh and the double series below, at 40 and 60
  digits (agreeing to 1e-25): rho_eff, K_eff, Gamma, Zc and the ABCD matrix,
  for shear wavenumbers s = l*sqrt(omega*rho/mu) from 0.1 to 100 and 1000
  (l = radius, half-gap or half the short side; rectangles up to 100) and
  lengths 2 mm, 30 mm and 200 mm (skipping |Re(Gamma) l| > 30).
* thermo_rect.json: the rectangular-duct shape function of Stinson (1991)
  (also Stinson & Champoux, JASA 91, 685 (1992)),
      1 - F = sum_{m,n odd} 64/(pi^4 m^2 n^2) k^2/(k^2 + (m pi/a)^2 + (n pi/b)^2),
  for aspect ratios 1 to 100 and |k| a from 1e-3 to 1e3 at arg k = pi/4.
  The n-sum is done in closed form (tanh partial fractions), the m-sum
  directly up to N >> |k| a/pi and beyond N by a binomial series with
  mpmath's Hurwitz zeta. Each value is recomputed with a and b swapped (so
  the other index is summed directly), and the two must agree; at small |k|
  the double series is also summed by brute force.

Air: the spec_reference preset (rho 1.204, c 343, mu 1.81e-5, gamma 1.4,
Pr 0.71, P0 = rho c^2 / gamma), stored in the fixture.

Usage: python3 tools/refgen/thermo_refs.py
"""

import math

import mpmath as mp

from common import mpc_of, s, stable, write

GEN = "tools/refgen/thermo_refs.py"

RHO, C, MU, GAMMA, PR = 1.204, 343.0, 1.81e-5, 1.4, 0.71
P0 = RHO * C * C / GAMMA  # same double as AirState::spec_reference()

CIRCLE, SLIT, RECT = 0, 1, 2


def shape(kind, k, d1, d2):
    """(F, 1 - F) straight from the definitions."""
    if kind == CIRCLE:
        z = k * d1
        i0, i1, i2 = mp.besseli(0, z), mp.besseli(1, z), mp.besseli(2, z)
        return 2 * i1 / (z * i0), i2 / i0
    if kind == SLIT:
        z = k * d1 / 2
        f = mp.tanh(z) / z
        return f, 1 - f
    om = rect_one_minus(k, d1, d2)
    return 1 - om, om


def duct(kind, d1, d2, omega, length):
    rho, mu, gamma, pr, p0 = (mp.mpf(v) for v in (RHO, MU, GAMMA, PR, P0))
    w = mp.mpf(omega)
    kv = mp.sqrt(mp.mpc(0, 1) * w * rho / mu)
    kt = kv * mp.sqrt(pr)
    _, omv = shape(kind, kv, d1, d2)
    ft, _ = shape(kind, kt, d1, d2)
    rho_eff = rho / omv
    k_eff = gamma * p0 / (1 + (gamma - 1) * ft)
    area = mp.pi * mp.mpf(d1) ** 2 if kind == CIRCLE else mp.mpf(d1) * mp.mpf(d2)
    g = mp.mpc(0, 1) * w * mp.sqrt(rho_eff / k_eff)
    if g.real < 0:
        g = -g
    zc = mp.sqrt(rho_eff * k_eff) / area
    # Branch of Zc consistent with Gamma: Zc = Gamma K_eff / (j omega S).
    if abs(zc - g * k_eff / (mp.mpc(0, 1) * w * area)) > abs(zc) * mp.mpf("1e-20"):
        zc = -zc
    gl = g * mp.mpf(length)
    ch, sh = mp.cosh(gl), mp.sinh(gl)
    return [rho_eff, k_eff, g, zc, ch, zc * sh, sh / zc, ch]


def rect_terms_direct(k, a, b, n_max):
    """sum_{m odd < n_max} 8/(pi^2 m^2) (k^2/kappa^2)(1 - tanh(kappa b/2)/(kappa b/2))."""
    total = mp.mpc(0)
    k2 = k * k
    for m in range(1, n_max, 2):
        kappa2 = k2 + (m * mp.pi / a) ** 2
        kappa = mp.sqrt(kappa2)
        y = kappa * b / 2
        total += 8 / (mp.pi**2 * m * m) * (k2 / kappa2) * (1 - mp.tanh(y) / y)
    return total


def rect_one_minus(k, a, b):
    a, b = mp.mpf(a), mp.mpf(b)
    # Direct terms well past the knee at m ~ |k| a / pi, and far enough that
    # exp(-n pi b/a) is below the working precision.
    n_prec = int((mp.mp.dps * math.log(10) + 20) * a / (math.pi * b)) + 1
    n = max(101, 4 * int(3 * abs(k) * a / mp.pi) + 1, n_prec)
    if n % 2 == 0:
        n += 1
    direct = rect_terms_direct(k, a, b, n)
    # Tail m >= n: tanh(kappa b/2) = 1 to within exp(-n pi b/a), so
    # the term is 8/(pi^2 m^2) [k^2/kappa^2 - 2 k^2/(b kappa^3)]; expand in
    # rho/m^2, rho = (k a/pi)^2, with sum_{m odd >= n} m^-s = 2^-s zeta(s, n/2).
    rho = (k * a / mp.pi) ** 2
    zt = lambda s_: mp.zeta(s_, mp.mpf(n) / 2) / mp.mpf(2) ** s_
    t1 = mp.mpc(0)  # sum m^-2 k^2/(k^2 + alpha^2) = (k a/pi)^2 sum_j (-rho)^j m^{-4-2j}
    t2 = mp.mpc(0)  # sum m^-2 k^2/kappa^3 = k^2 (a/pi)^3 sum_j binom(-3/2, j) rho^j m^{-5-2j}
    for j in range(0, 200):
        u = (-rho) ** j * zt(4 + 2 * j)
        v = mp.binomial(mp.mpf(-1.5), j) * rho**j * zt(5 + 2 * j)
        t1 += u
        t2 += v
        if abs(u) < mp.mpf(10) ** (-mp.mp.dps - 5) and abs(v) < mp.mpf(10) ** (-mp.mp.dps - 5):
            break
    tail = 8 / mp.pi**2 * (rho * t1 - 2 / b * k * k * (a / mp.pi) ** 3 * t2)
    return direct + tail


def rect_brute(k, a, b, n_max):
    """Truncated double series (checks the closed-form n-sum at small |k|)."""
    a, b = mp.mpf(a), mp.mpf(b)
    total = mp.mpc(0)
    for m in range(1, n_max, 2):
        for n in range(1, n_max, 2):
            total += 64 / (mp.pi**4 * m * m * n * n) * k * k / (
                k * k + (m * mp.pi / a) ** 2 + (n * mp.pi / b) ** 2
            )
    return total


def duct_rows():
    rows = []
    shear = [0.1, 0.2, 0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0, 30.0, 50.0, 100.0, 1000.0]
    for kind in (CIRCLE, SLIT, RECT):
        for ell in (1e-4, 1e-3, 1e-2):
            if kind == CIRCLE:
                d1, d2 = ell, 0.0
            elif kind == SLIT:
                d1, d2 = 2 * ell, 40 * ell
            else:
                d1, d2 = 2 * ell, 5 * ell
            for sh in shear:
                if kind == RECT and sh > 100.0:
                    continue  # covered by thermo_rect.json; keeps generation fast
                omega = sh * sh * MU / (RHO * ell * ell)
                f = omega / (2 * math.pi)
                if not 1.0 <= f <= 2e5:
                    continue
                for length in (2e-3, 3e-2, 2e-1):
                    def fn():
                        return duct(kind, d1, d2, omega, length)

                    v = stable(fn, rtol=mp.mpf("1e-25"))
                    if abs(v[2].real * length) > 30.0:
                        continue  # cosh(Gamma l) beyond 1e13: no use as a test
                    rows.append(
                        [str(kind), s(d1), s(d2), s(omega), s(length)]
                        + [t for c in v for t in (s(c.real), s(c.imag))]
                    )
    return rows


def rect_rows():
    rows = []
    kas = [10.0 ** (e / 4) for e in range(-12, 13)]  # 1e-3 .. 1e3
    # Around the algorithm switches of special::shape_rect: |rho| = 1/9
    # (|ka| = pi/3), M > 33 (|ka| = 11 pi) and Re(ka) = 42 (|ka| = 42 sqrt 2).
    for c in (math.pi / 3, 11 * math.pi, 42 * math.sqrt(2)):
        kas += [c * (1 - 1e-6), c * (1 + 1e-6)]
    for ratio in (1.0, 1.5, 3.0, 10.0, 100.0):
        a = 1e-3
        b = a * ratio
        for ka in sorted(kas):
            kr = ka / a * math.sqrt(0.5)
            if ratio == 100.0 and ka > 30.0:
                continue  # the swapped check would need ~1e5 terms

            def fn():
                k = mpc_of(kr, kr)
                om = rect_one_minus(k, a, b)
                om_swapped = rect_one_minus(k, b, a)
                if abs(om - om_swapped) > abs(om) * mp.mpf("1e-22"):
                    raise RuntimeError(f"swap check failed at ka={ka}, b/a={ratio}")
                return [1 - om, om]

            v = stable(fn, dps=(40, 50), rtol=mp.mpf("1e-22"))
            rows.append([s(a), s(b), s(kr), s(kr)] + [t for c in v for t in (s(c.real), s(c.imag))])
    return rows


def brute_force_check():
    """The closed-form inner sum against the plain double series."""
    with mp.workdps(30):
        for ratio in (1.0, 3.0):
            k = mpc_of(300.0, 300.0)  # |k| a = 0.42 for a = 1 mm
            a, b = 1e-3, 1e-3 * ratio
            ref = rect_one_minus(k, a, b)
            brute = rect_brute(k, a, b, 801)
            # Truncating both sums at 800 leaves O(1/800^3) of the leading term.
            err = abs(brute - ref) / abs(ref)
            print(f"brute-force double series vs closed-form n-sum, b/a={ratio}: {mp.nstr(err, 3)}")
            assert err < 1e-6


def main():
    brute_force_check()
    meta = {
        "generator": GEN,
        "mpmath": mp.__version__,
        "precision": "mpmath at 40 and 60 digits agreeing to 1e-25 (thermo_rect: 40 and 50 digits, 1e-22, and a/b-swapped sums agreeing to 1e-22)",
        "air": {"rho": s(RHO), "c": s(C), "mu": s(MU), "gamma": s(GAMMA), "prandtl": s(PR), "p0": s(P0)},
    }
    write(
        "thermo_duct.json",
        dict(
            meta,
            description="Thermoviscous duct model: kind 0 circle (d1 = radius), 1 slit "
            "(d1 = gap, d2 = width), 2 rectangle (d1, d2 = sides); omega in rad/s, "
            "length in m",
        ),
        ["kind", "d1", "d2", "omega", "length"]
        + [f"{n}_{p}" for n in ("rho_eff", "k_eff", "gamma", "zc", "a", "b", "c", "d") for p in ("re", "im")],
        duct_rows(),
    )
    write(
        "thermo_rect.json",
        dict(meta, description="Rectangular-duct shape function F and 1 - F for sides a, b and complex k"),
        ["a", "b", "k_re", "k_im", "f_re", "f_im", "om_re", "om_im"],
        rect_rows(),
    )


if __name__ == "__main__":
    main()
