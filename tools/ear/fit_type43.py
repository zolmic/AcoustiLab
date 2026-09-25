#!/usr/bin/env python3
"""Fit the Type 4.3 drum network to ITU-T P.57 (06/2021) Table 5-c.

Inputs
  * data/ear/type43_geometry.json   derived canal area function (p57_geometry.py)
  * private/p57_table5c.json        verbatim Table 5-c (untracked; written by
                                    p57_geometry.py from the ITU PDF)

Model (tools/ear/type43.py): P.57 canal as a chain of lossy conical segments
(48 over tip..EEP, air "standard_23C"), drum network at the DRP's axial
position, rigid-ended tip stub in parallel. The transfer impedance
p_DRP / U_ref is evaluated at the 121 Table 5-c frequencies.

Objective (least squares in log-parameter space):
  * each Table 5-c point: deviation of |Z_T|*f relative to 500 Hz from the
    nominal value, divided by the tolerance on the side it falls (upper
    tolerance for positive deviations, lower for negative);
  * the absolute level at 500 Hz: 20 log10(|Z_T| / 27.7 MPa s/m^3) / 0.2 dB
    (P.57 clause 6.4.3.3 NOTE 2: 27.7 MPa s/m^3 <-> 1.63 cm^3).

Output: data/ear/type43_drum.json (fitted element values and a fit summary;
the Table 5-c values themselves are not written).
"""

from __future__ import annotations

import json
import math
import sys
from dataclasses import asdict, fields
from pathlib import Path

import numpy as np
from scipy.optimize import least_squares

sys.path.insert(0, str(Path(__file__).resolve().parent))
import earmodels as em  # noqa: E402
from type43 import ROOT, Type43, Type43Drum  # noqa: E402

TABLE = ROOT / "private" / "p57_table5c.json"
OUT = ROOT / "data" / "ear" / "type43_drum.json"
Z500 = 27.7e6
N_SEGMENTS = 48

START = [
    Type43Drum(v_m=8.3e-8, r_o=2.0e7, m_o=380.0, v_t=1.38e-6, r_a=7.7e8, v_a=2.2e-6),
    Type43Drum(v_m=5e-8, r_o=3e7, m_o=100.0, v_t=1.5e-6, r_a=3e8, v_a=5e-6),
]


def load_table():
    if not TABLE.exists():
        sys.exit(f"{TABLE} not found: run tools/ear/p57_geometry.py first")
    rows = json.loads(TABLE.read_text())["rows"]
    f = np.array([r["f_Hz"] for r in rows])
    lv = np.array([r["level_dB"] for r in rows])
    tu = np.array([r["tol_upper_dB"] for r in rows])
    tl = np.array([r["tol_lower_dB"] for r in rows])
    return f, lv, tu, tl


def levels(drum, freqs):
    m = Type43(drum, n_segments=N_SEGMENTS)
    z500 = m.transfer_ref(500.0)
    z = np.array([m.transfer_ref(f) for f in freqs])
    return 20 * np.log10(np.abs(z) * freqs / (abs(z500) * 500.0)), z500


def main():
    f, lv0, tu, tl = load_table()
    names = [fl.name for fl in fields(Type43Drum)]

    def drum_of(p, x0):
        vals = {n: getattr(x0, n) * math.exp(v) for n, v in zip(names, p)}
        return Type43Drum(**vals)

    def make_resid(x0, power):
        def resid(p):
            lv, z500 = levels(drum_of(p, x0), f)
            d = lv - lv0
            r = np.where(d > 0, d / tu, d / -tl)
            a = 20 * math.log10(abs(z500) / Z500) / 0.2
            r = np.append(r, a)
            # |r|^(power/2) with sign: sum of squares = sum |r|^power
            return np.sign(r) * np.abs(r) ** (power / 2)

        return resid

    best = None
    for x0 in START:
        # Stage 1: least squares; stage 2: an 8-norm (close to minimax) so
        # that no point is left just outside its tolerance.
        sol = least_squares(make_resid(x0, 2), np.zeros(len(names)), method="trf", diff_step=1e-4, max_nfev=400)
        x1 = drum_of(sol.x, x0)
        sol = least_squares(make_resid(x1, 8), np.zeros(len(names)), method="trf", diff_step=1e-4, max_nfev=400)
        cand = drum_of(sol.x, x1)
        if best is None or sol.cost < best[1]:
            best = (cand, sol.cost)
    drum, cost = best
    lv, z500 = levels(drum, f)
    d = lv - lv0
    inside = (d <= tu + 1e-9) & (d >= tl - 1e-9)
    norm = np.where(d > 0, d / tu, d / -tl)
    air = em.Air.standard_23c()
    veff = em.effective_volume(z500, air, 2 * math.pi * 500.0)
    summary = {
        "points_within_tolerance": int(inside.sum()),
        "points": int(len(f)),
        "max_deviation_over_tolerance": round(float(np.abs(norm).max()), 3),
        "rms_deviation_dB": round(float(np.sqrt(np.mean(d**2))), 3),
        "transfer_impedance_500Hz_MPa_s_per_m3": round(abs(z500) / 1e6, 3),
        "effective_volume_500Hz_cm3": round(veff * 1e6, 4),
        "cost": round(float(cost), 4),
    }
    print(json.dumps(summary, indent=2))
    for fi, a, b, u, l in zip(f, lv0, lv, tu, tl):
        flag = "" if (l - 1e-9 <= b - a <= u + 1e-9) else "  OUTSIDE"
        print(f"{fi:7.0f} dev {b - a:+6.2f} dB  tol [{l:+.1f}, {u:+.1f}]{flag}")
    out = {
        "source": "Fitted by tools/ear/fit_type43.py to ITU-T P.57 (06/2021) Table 5-c "
        "(transfer impedance times f relative to 500 Hz, with tolerances) and to "
        "27.7 MPa s/m^3 at 500 Hz (clause 6.4.3.3 NOTE 2).",
        "model": "Z = Z_cav + 1/(jw C_m + 1/(R_o + jw M_o)); "
        "Z_cav = 1/(jw C_t + 1/(R_a + 1/(jw C_a))); C = V/(gamma P0)",
        "canal": "data/ear/type43_geometry.json, 48 conical segments tip..EEP, air standard_23C",
        "status": "fitted surrogate of the Type 4.3 drum simulator; the physical network "
        "of commercial Type 4.3 simulators is not published",
        "parameters": {
            "C_m_volume_cm3": drum.v_m * 1e6,
            "R_o_Pa_s_per_m3": drum.r_o,
            "M_o_kg_per_m4": drum.m_o,
            "C_t_volume_cm3": drum.v_t * 1e6,
            "R_a_Pa_s_per_m3": drum.r_a,
            "C_a_volume_cm3": drum.v_a * 1e6,
        },
        "fit": summary,
    }
    OUT.write_text(json.dumps(out, indent=2) + "\n")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
