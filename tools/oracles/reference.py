#!/usr/bin/env python3
"""Independent reference values for crates/acoustilab/tests/oracles.rs.

Writes crates/acoustilab/tests/data/oracles_reference.json.

Everything here is recomputed from first principles with mpmath at 30
significant digits, independently of the Rust engine:

* Air: the spec's analytical-check constants (spec Section 17 / App. C):
  rho = 1.204 kg/m^3, c = 343 m/s, mu = 1.81e-5 Pa.s, gamma = 1.4, Pr = 0.71,
  P0 = rho c^2 / gamma (docs/conventions.md, "Air").
* Thermoviscous ducts (Zwikker-Kosten / Stinson low-reduced-frequency model,
  docs/conventions.md "Thermoviscous functions", errata E1/E2): with
  k_v = sqrt(j w rho / mu) and k_t = k_v sqrt(Pr),
    slit   F(z) = tanh(z)/z,             z = k h/2   (h the gap)
    circle F(z) = 2 I1(z) / (z I0(z)),   z = k a     (a the radius)
    rho_eff = rho / (1 - F(k_v s)),  K_eff = gamma P0 / (1 + (gamma-1) F(k_t s)).
  L0 (lumped): Z = j w rho_eff l / S (+ j w rho delta / S for end corrections).
  L1 (distributed): T = [[cosh G l, Zc sinh G l], [sinh G l / Zc, cosh G l]],
  G = j w sqrt(rho_eff/K_eff) (Re G >= 0), Zc = sqrt(rho_eff K_eff)/S, with half
  of the end-correction inertance as a series element at each end.
* Leak transfer relative to the sealed cavity at constant volume velocity
  (spec App. C3, errata E9): H = Z_L / (Z_L + 1/(j w C)), C = V/(rho c^2).
  At L1 the leak's far end is at ambient, so Z_L = B/D of its ABCD matrix.

Run from anywhere:  python3 tools/oracles/reference.py
"""

import json
from pathlib import Path

import mpmath as mp

mp.mp.dps = 30

RHO = mp.mpf("1.204")
C0 = mp.mpf("343")
MU = mp.mpf("1.81e-5")
GAMMA = mp.mpf("1.4")
PR = mp.mpf("0.71")
K0 = RHO * C0 * C0  # = gamma P0 by construction
J = mp.mpc(0, 1)


def shape(kind, z):
    if kind == "slit":
        return mp.tanh(z) / z
    return 2 * mp.besseli(1, z) / (z * mp.besseli(0, z))


def medium(kind, s, w):
    kv = mp.sqrt(J * w * RHO / MU)
    kt = kv * mp.sqrt(PR)
    return RHO / (1 - shape(kind, kv * s)), K0 / (1 + (GAMMA - 1) * shape(kind, kt * s))


def z_lumped(kind, s, area, length, w, end=0):
    rho_eff, _ = medium(kind, s, w)
    return J * w * rho_eff * length / area + J * w * RHO * end / area


def abcd(kind, s, area, length, w, end=0):
    rho_eff, k_eff = medium(kind, s, w)
    g = mp.sqrt(rho_eff / k_eff)
    if mp.im(g) > 0:
        g = -g
    gamma = J * w * g
    zc = k_eff * g / area
    ch, sh = mp.cosh(gamma * length), mp.sinh(gamma * length)
    t = mp.matrix([[ch, zc * sh], [sh / zc, ch]])
    if end:
        half = mp.matrix([[1, J * w * RHO * end / 2 / area], [0, 1]])
        t = half * t * half
    return t


def z_leak(kind, s, area, length, w, level, end=0):
    if level == 0:
        return z_lumped(kind, s, area, length, w, end)
    t = abcd(kind, s, area, length, w, end)
    return t[0, 1] / t[1, 1]


def f17(x):
    return float(mp.nstr(x, 17))


def golden_max(fun, lo, hi, tol=mp.mpf("1e-14")):
    """Maximum of a unimodal function on [lo, hi] (golden-section)."""
    g = (mp.sqrt(5) - 1) / 2
    a, b = mp.mpf(lo), mp.mpf(hi)
    c, d = b - g * (b - a), a + g * (b - a)
    fc, fd = fun(c), fun(d)
    while b - a > tol * (a + b):
        if fc > fd:
            b, d, fd = d, c, fc
            c = b - g * (b - a)
            fc = fun(c)
        else:
            a, c, fc = c, d, fd
            d = a + g * (b - a)
            fd = fun(d)
    return (a + b) / 2


def bisect(fun, lo, hi):
    return mp.findroot(fun, (mp.mpf(lo), mp.mpf(hi)), solver="bisect")


# ----- Slit leak worked examples (spec Section 17, App. C3; errata E45) ----

SLIT_FREQS = [10, 20, 50, 73, 83, 100, 200, 348, 500, 522, 1000, 2000, 5000, 10000]


def slit_examples():
    v = mp.mpf("30e-6")
    cap = v / K0
    width, length = mp.mpf("30e-3"), mp.mpf("10e-3")
    out = []
    for gap_mm in ["1", "0.2"]:
        gap = mp.mpf(gap_mm) * mp.mpf("1e-3")
        area = gap * width
        r_pois = 12 * MU * length / (width * gap**3)
        m_pois = 6 * RHO * length / (5 * width * gap)
        m_geo = RHO * length / (width * gap)
        rec = {
            "gap_mm": float(gap_mm),
            "width_mm": 30.0,
            "length_mm": 10.0,
            "volume_cm3": 30.0,
            "R_pois": f17(r_pois),
            "M_pois": f17(m_pois),
            "rc_corner_Hz": f17(1 / (2 * mp.pi * r_pois * cap)),
            "f0_geometric_mass_Hz": f17(1 / (2 * mp.pi * mp.sqrt(m_geo * cap))),
            "Q_poiseuille_estimate": f17(mp.sqrt(m_geo / cap) / r_pois),
            "levels": [],
        }
        for level in (0, 1):

            def h(f, level=level):
                w = 2 * mp.pi * f
                zl = z_leak("slit", gap / 2, area, length, w, level)
                return zl / (zl + 1 / (J * w * cap))

            mag = lambda f: abs(h(f))
            lv = {
                "level": level,
                "H": [[f, f17(mp.re(h(f))), f17(mp.im(h(f)))] for f in SLIT_FREQS],
            }
            if gap_mm == "1":
                # Frequency where the leak turns inertive: Im Z_L = Re Z_L.
                def react_minus_res(f, level=level):
                    zl = z_leak("slit", gap / 2, area, length, 2 * mp.pi * f, level)
                    return mp.im(zl) - mp.re(zl)

                lv["inertive_above_Hz"] = f17(bisect(react_minus_res, 5, 200))
                fp = golden_max(mag, 400, 650)
                pk = mag(fp)
                f1 = bisect(lambda f: mag(f) - pk / mp.sqrt(2), 300, fp)
                f2 = bisect(lambda f: mag(f) - pk / mp.sqrt(2), fp, 900)
                lv.update(
                    peak_Hz=f17(fp),
                    peak_dB=f17(20 * mp.log10(pk)),
                    half_power_Hz=[f17(f1), f17(f2)],
                    Q_half_power=f17(fp / (f2 - f1)),
                )
            else:
                fp = golden_max(mag, 150, 1000)
                lv.update(
                    peak_Hz=f17(fp),
                    peak_dB=f17(20 * mp.log10(mag(fp))),
                    minus3dB_Hz=f17(bisect(lambda f: mag(f) - 1 / mp.sqrt(2), 30, 120)),
                )
            rec["levels"].append(lv)
        out.append(rec)
    return out


# ----- Helmholtz vent (spec Section 17 / App. C4; errata E22, E23) --------


def helmholtz_vent():
    a, wall, v = mp.mpf("1.5e-3"), mp.mpf("2e-3"), mp.mpf("100e-6")
    area = mp.pi * a * a
    cap = v / K0
    piston_end = 8 * a / (3 * mp.pi)  # low-frequency baffled-piston end correction
    rec = {
        "radius_mm": 1.5,
        "wall_mm": 2.0,
        "volume_cm3": 100.0,
        "lossless_piston_ends_Hz": f17(C0 / (2 * mp.pi) * mp.sqrt(area / (v * (wall + 2 * piston_end)))),
        "lossless_no_ends_Hz": f17(C0 / (2 * mp.pi) * mp.sqrt(area / (v * wall))),
        "thermoviscous": [],
    }
    for ends in ("piston", "none"):
        end = 2 * piston_end if ends == "piston" else 0
        for level in (0, 1):

            def im_y(f, level=level, end=end):
                w = 2 * mp.pi * f
                return mp.im(J * w * cap + 1 / z_leak("circle", a, area, wall, w, level, end))

            lo, hi = (150, 260) if ends == "piston" else (250, 400)
            rec["thermoviscous"].append(
                {"ends": ends, "level": level, "resonance_Hz": f17(bisect(im_y, lo, hi))}
            )
    return rec


# ----- Thermoviscous low-frequency limits (Section 17; App. D) -------------

LIMIT_FREQ = 0.1  # Hz; low enough that the O(s^4) terms are < 2e-6 relative


def duct_limits():
    cases = [
        ("slit", {"gap_mm": 0.2, "width_mm": 20.0, "length_mm": 5.0}),
        ("slit", {"gap_mm": 1.0, "width_mm": 30.0, "length_mm": 10.0}),
        ("tube", {"radius_mm": 0.5, "length_mm": 5.0}),
        ("tube", {"radius_mm": 1.5, "length_mm": 2.0}),
    ]
    out = []
    w = 2 * mp.pi * LIMIT_FREQ
    for kind, geo in cases:
        length = mp.mpf(geo["length_mm"]) / 1000
        if kind == "slit":
            h = mp.mpf(geo["gap_mm"]) / 1000
            wd = mp.mpf(geo["width_mm"]) / 1000
            s, area = h / 2, h * wd
            r0 = 12 * MU * length / (wd * h**3)
            m0 = 6 * RHO * length / (5 * wd * h)
            shape_kind = "slit"
        else:
            a = mp.mpf(geo["radius_mm"]) / 1000
            s, area = a, mp.pi * a * a
            r0 = 8 * MU * length / (mp.pi * a**4)
            m0 = 4 * RHO * length / (3 * mp.pi * a * a)
            shape_kind = "circle"
        z = z_lumped(shape_kind, s, area, length, w)
        out.append(
            {
                "type": kind,
                **geo,
                "f_Hz": LIMIT_FREQ,
                "R_poiseuille": f17(r0),
                "M_poiseuille": f17(m0),
                # Exact series impedance at f (= Gamma Zc l at L1): R and X/w.
                "R_exact": f17(mp.re(z)),
                "M_exact": f17(mp.im(z) / w),
            }
        )
    return out


def high_frequency_resistance():
    a, length = mp.mpf("2e-3"), mp.mpf("10e-3")
    area = mp.pi * a * a
    r = {f: mp.re(z_lumped("circle", a, area, length, 2 * mp.pi * f)) for f in (10000, 40000)}
    return {
        "radius_mm": 2.0,
        "length_mm": 10.0,
        "R_10kHz": f17(r[10000]),
        "R_40kHz": f17(r[40000]),
        "ratio": f17(r[40000] / r[10000]),
    }


def main():
    out = {
        "generator": "tools/oracles/reference.py (mpmath, 30 digits)",
        "air": {"rho": 1.204, "c": 343.0, "mu": 1.81e-5, "gamma": 1.4, "Pr": 0.71},
        "slit_examples": slit_examples(),
        "helmholtz_vent": helmholtz_vent(),
        "duct_limits": duct_limits(),
        "hf_resistance": high_frequency_resistance(),
    }
    path = Path(__file__).resolve().parents[2] / "crates/acoustilab/tests/data/oracles_reference.json"
    path.write_text(json.dumps(out, indent=1) + "\n")
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
