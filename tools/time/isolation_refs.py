#!/usr/bin/env python3
"""Leak-limited isolation of a sealed rigid cup, for crates/acoustilab/tests/isolation.rs.

Writes crates/acoustilab/tests/data/isolation_reference.json.

A rigid cup of volume V (a lossless compliance C = V/(rho c^2)) sealed
except for one thermoviscous slit to the outside air (gap h, width w,
length l). With 1 Pa outside, the pressure inside is the divider of the
leak and the cup compliance:

* L0 (lumped leak): p_in = Z_C/(Z_L + Z_C), Z_L = j w rho_eff l/(w h),
  Z_C = 1/(j w C);
* L1 (transmission line): p_in = Z_C/(A Z_C + B) with the slit's ABCD
  matrix (A = cosh G l, B = Zc sinh G l).

The insertion loss at the cup (the drum point, with the open "ear" the
cup node itself) is IL = -20 log10 |p_in|. Thermoviscous slit functions
(docs/conventions.md; errata E1/E2): k_v = sqrt(j w rho/mu),
k_t = k_v sqrt(Pr), F(z) = tanh(z)/z with z = k h/2,
rho_eff = rho/(1 - F(k_v h/2)), K_eff = gamma P0/(1 + (gamma - 1) F(k_t h/2)),
G = j w sqrt(rho_eff/K_eff) (Re G >= 0), Zc = sqrt(rho_eff K_eff)/(w h).
Air: the spec's analytical set (rho 1.204, c 343, mu 1.81e-5, gamma 1.4,
Pr 0.71, P0 = rho c^2/gamma). mpmath at 30 digits, independent of the
engine.

Run from anywhere:  python3 tools/time/isolation_refs.py
"""

import json
from pathlib import Path

import mpmath as mp

mp.mp.dps = 30
OUT = Path(__file__).resolve().parents[2] / "crates/acoustilab/tests/data/isolation_reference.json"

RHO, C0, MU = mp.mpf("1.204"), mp.mpf("343"), mp.mpf("1.81e-5")
GAMMA, PR = mp.mpf("1.4"), mp.mpf("0.71")
K0 = RHO * C0 * C0
J = mp.mpc(0, 1)

VOLUME = mp.mpf("25e-6")
GAP, WIDTH, LENGTH = mp.mpf("0.1e-3"), mp.mpf("20e-3"), mp.mpf("5e-3")
FREQS = [10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]


def medium(w):
    kv = mp.sqrt(J * w * RHO / MU)
    kt = kv * mp.sqrt(PR)
    f = lambda z: mp.tanh(z) / z
    return RHO / (1 - f(kv * GAP / 2)), K0 / (1 + (GAMMA - 1) * f(kt * GAP / 2))


def p_inside(f, level):
    w = 2 * mp.pi * f
    area = GAP * WIDTH
    zc_cup = 1 / (J * w * VOLUME / K0)
    rho_eff, k_eff = medium(w)
    if level == 0:
        zl = J * w * rho_eff * LENGTH / area
        return zc_cup / (zl + zc_cup)
    g = mp.sqrt(rho_eff / k_eff)
    if mp.im(g) > 0:
        g = -g
    gamma = J * w * g
    zc = k_eff * g / area
    a, b = mp.cosh(gamma * LENGTH), zc * mp.sinh(gamma * LENGTH)
    return zc_cup / (a * zc_cup + b)


def main():
    cases = []
    for level in (0, 1):
        rows = []
        for f in FREQS:
            p = p_inside(mp.mpf(f), level)
            rows.append({
                "f_Hz": f,
                "re": float(mp.re(p)),
                "im": float(mp.im(p)),
                "insertion_loss_dB": float(-20 * mp.log10(abs(p))),
            })
        cases.append({"level": level, "rows": rows})
    r_pois = 12 * MU * LENGTH / (WIDTH * GAP ** 3)
    data = {
        "generator": "tools/time/isolation_refs.py",
        "volume_cm3": 25.0,
        "gap_mm": 0.1,
        "width_mm": 20.0,
        "length_mm": 5.0,
        "corner_Hz": float(1 / (2 * mp.pi * r_pois * VOLUME / K0)),
        "cases": cases,
    }
    OUT.write_text(json.dumps(data, indent=1) + "\n")
    print(f"corner {data['corner_Hz']:.2f} Hz")
    for c in cases:
        print(c["level"], [round(r["insertion_loss_dB"], 3) for r in c["rows"]])
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
