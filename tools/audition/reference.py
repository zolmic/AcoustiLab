#!/usr/bin/env python3
"""Independent reference values for crates/acoustilab/tests/audition.rs.

Re-implements from the definitions in docs/auralization.md, sharing no code
with the Rust engine:

* the regularised inversion of a magnitude-only baseline: a working grid of
  96 points per octave over the curve's range with the inversion-band edges
  inserted; 1/6-octave power smoothing and the one-octave envelope (the
  smoothing of tools/targets/reference.py: scipy.integrate.quad over the
  piecewise-linear power, not the engine's exact trapezoid sums); the notch
  rule (maximal runs below the envelope, replaced by it when deeper than
  15 dB); the reference level (mean dB of the working-grid points from
  500 Hz to 2 kHz); and the
  Kirkeby-Nelson inverse c = b/(b^2 + beta), beta = 1/(4*10^(12/10));
* the analytic audition filter of a closed-form candidate (a series-RLC
  low-pass, V(C)/V = 1/(1 + sRC + s^2 LC)) against such a baseline: the
  check grid (48 points per octave from 20 Hz to 20 kHz with the band,
  inversion-band and anchor edges inserted), the band-edge hold with a
  continuous slope, and the 500 Hz-2 kHz anchor (log-interval weighted mean).

The synthetic baselines are tabulated at 24 points per octave from 20 Hz to
20 kHz:

* ``cap``: a broad dip of -24 dB at 5 kHz (Gaussian in log2 f, sigma 0.4
  octave), outside the 500 Hz-2 kHz reference band: the Kirkeby cap bites
  (12.0 dB).
* ``notches``: a 30 dB notch at 3 kHz and an 8 dB notch at 300 Hz (sigma
  0.08 octave each) on a +3 dB bass bump: after smoothing the first is
  deeper than 15 dB (not inverted), the second shallower (inverted).
* ``smooth``: a headphone-target-like curve (bass bump, ear gain, treble
  dip).

Writes crates/acoustilab/tests/data/audition_reference.json.
Usage: python3 tools/audition/reference.py
"""

from __future__ import annotations

import importlib.util
import sys
import json
import math
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "acoustilab" / "tests" / "data" / "audition_reference.json"

# Smoothing from the targets reference (quad integration of the power).
sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location("targets_reference", ROOT / "tools" / "targets" / "reference.py")
_tref = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_tref)
smooth_power = _tref.smooth_power

BAND = (20.0, 10000.0)
CAP_DB = 12.0
NOTCH_DB = 15.0
PPO_WORK = 96.0
PPO_CHECK = 48.0
ANCHOR = (500.0, 2000.0)
EXCURSION_DB = 6.0


def log_grid(lo, hi, ppo):
    n = max(1, math.ceil(math.log2(hi / lo) * ppo))
    return [lo * (hi / lo) ** (i / n) for i in range(n + 1)]


def insert(grid, extra, lo=None, hi=None):
    g = list(grid)
    for f in extra:
        if lo is not None and not (lo < f < hi):
            continue
        if not any(abs(x / f - 1) < 1e-9 for x in g):
            g.append(f)
    return sorted(g)


def interp_db(f, db, x):
    return float(np.interp(math.log(x), np.log(f), db))


def gauss(f, fc, sigma, gain):
    return gain * math.exp(-0.5 * (math.log2(f / fc) / sigma) ** 2)


CURVES = {
    "cap": lambda f: gauss(f, 5000.0, 0.4, -24.0),
    "notches": lambda f: gauss(f, 3000.0, 0.08, -30.0) + gauss(f, 300.0, 0.08, -8.0) + gauss(f, 60.0, 1.0, 3.0),
    "smooth": lambda f: gauss(f, 50.0, 1.2, 6.0) + gauss(f, 2800.0, 0.6, 9.0) + gauss(f, 9000.0, 0.4, -4.0),
}


def invert(f_in, db_in):
    c_lo, c_hi = f_in[0], f_in[-1]
    lo, hi = max(BAND[0], c_lo), min(BAND[1], c_hi)
    grid = insert(log_grid(c_lo, c_hi, PPO_WORK), [lo, hi])
    raw = np.array([interp_db(f_in, db_in, x) for x in grid])
    b6 = smooth_power(grid, raw, 6)
    env = smooth_power(grid, b6, 1)
    # End points keep their levels (no window there).
    b6[0], b6[-1] = raw[0], raw[-1]
    env[0], env[-1] = b6[0], b6[-1]
    b6 = b6.copy()
    notches = []
    i, n = 0, len(grid)
    while i < n:
        if b6[i] >= env[i]:
            i += 1
            continue
        start = i
        while i < n and b6[i] < env[i]:
            i += 1
        depth = env[start:i] - b6[start:i]
        j = int(np.argmax(depth))
        d, at = float(depth[j]), grid[start + j]
        if not (lo * (1 - 1e-12) <= at <= hi * (1 + 1e-12)) or d < 1e-9:
            continue
        inverted = d <= NOTCH_DB
        if not inverted:
            b6[start:i] = env[start:i]
        if d >= 3.0 or not inverted:
            notches.append({"from_Hz": grid[start], "to_Hz": grid[i - 1], "depth_dB": d, "at_Hz": at, "inverted": inverted})
    band = [k for k, x in enumerate(grid) if lo * (1 - 1e-12) <= x <= hi * (1 + 1e-12)]
    refs = [k for k, x in enumerate(grid) if 500 * (1 - 1e-12) <= x <= 2000 * (1 + 1e-12)]
    ref = float(np.mean(b6[refs]))
    beta = 0.25 * 10 ** (-CAP_DB / 10)
    b = 10 ** ((b6[band] - ref) / 20)
    inv = 20 * np.log10(b / (b * b + beta))
    return {
        "grid_Hz": [grid[k] for k in band],
        "smoothed_dB": list(b6[band] - ref),
        "inverse_dB": list(inv),
        "reference_dB": ref,
        "band_Hz": [lo, hi],
        "notches": notches,
    }


def inverse_at(inv, f):
    g, v = inv["grid_Hz"], inv["inverse_dB"]
    if f <= g[0]:
        return v[0]
    if f >= g[-1]:
        return v[-1]
    return interp_db(g, v, f)


def edge_hold(level, slope, x):
    w = 1.0 if slope == 0 else min(1.0, 2 * EXCURSION_DB / abs(slope))
    x = min(x, w)
    return level + slope * (x - x * x / (2 * w))


def anchor_mean(grid, db):
    idx = [i for i, f in enumerate(grid) if ANCHOR[0] * (1 - 1e-9) <= f <= ANCHOR[1] * (1 + 1e-9)]
    sw = s = 0.0
    for j, i in enumerate(idx):
        a = grid[i] if j == 0 else math.sqrt(grid[idx[j - 1]] * grid[i])
        b = grid[i] if j == len(idx) - 1 else math.sqrt(grid[i] * grid[idx[j + 1]])
        w = math.log(b / a)
        sw += w
        s += w * db[i]
    return s / sw


def rlc(f0, q, c):
    w0 = 2 * math.pi * f0
    l = 1 / (w0 * w0 * c)
    return {"R_ohm": math.sqrt(l / c) / q, "L_H": l, "C_F": c}


def h_lp(p, f):
    s = 2j * math.pi * f
    return 1 / (1 + s * p["R_ohm"] * p["C_F"] + s * s * p["L_H"] * p["C_F"])


def target_design(p, inv):
    band = tuple(inv["band_Hz"])
    grid = insert(log_grid(20.0, 20000.0, PPO_CHECK), [band[0], band[1], BAND[0], BAND[1], ANCHOR[0], ANCHOR[1]], 20.0, 20000.0)
    raw = [20 * math.log10(abs(h_lp(p, f))) + inverse_at(inv, f) for f in grid]
    i_lo = next(i for i, f in enumerate(grid) if abs(f / band[0] - 1) < 1e-9)
    i_hi = next(i for i, f in enumerate(grid) if abs(f / band[1] - 1) < 1e-9)
    s_lo = (raw[i_lo + 1] - raw[i_lo]) / math.log2(grid[i_lo + 1] / grid[i_lo])
    s_hi = (raw[i_hi] - raw[i_hi - 1]) / math.log2(grid[i_hi] / grid[i_hi - 1])
    un = []
    for i, f in enumerate(grid):
        if f < band[0] * (1 - 1e-12):
            un.append(edge_hold(raw[i_lo], -s_lo, math.log2(band[0] / f)))
        elif f > band[1] * (1 + 1e-12):
            un.append(edge_hold(raw[i_hi], s_hi, math.log2(f / band[1])))
        else:
            un.append(raw[i])
    a = anchor_mean(grid, un)
    return {"grid_Hz": grid, "design_dB": [v - a for v in un], "anchor_dB": a}


def main():
    f_in = log_grid(20.0, 20000.0, 24.0)
    cases = {}
    for name, fn in CURVES.items():
        db = [fn(f) for f in f_in]
        cases[name] = {"frequencies_Hz": f_in, "dB": db, **invert(f_in, db)}
    p = rlc(1000.0, 2.0, 1e-6)
    out = {
        "about": "tools/audition/reference.py: regularised inversion and target-baseline filter design, independent of the engine",
        "cases": cases,
        "candidate_rlc": p,
        "target_design": {name: target_design(p, cases[name]) for name in ("smooth", "notches")},
    }
    OUT.write_text(json.dumps(out, indent=1) + "\n")
    for name, c in cases.items():
        print(name, "ref", round(c["reference_dB"], 4), "max inverse", round(max(c["inverse_dB"]), 4),
              "notches", [(round(n["at_Hz"]), round(n["depth_dB"], 2), n["inverted"]) for n in c["notches"]])
    print("wrote", OUT)


if __name__ == "__main__":
    main()
