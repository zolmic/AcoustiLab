#!/usr/bin/env python3
"""Generate crates/acoustilab/tests/data/ear_reference.json.

Independent Python computations used by crates/acoustilab/tests/ear.rs:

* webster: transfer matrices of horns with the thermoviscous medium evaluated
  at the LOCAL radius, by numerical integration of the lossy horn equations
  (scipy DOP853, rtol 1e-11). This is the limit the conical chain converges
  to; the chain itself never enters the reference.
* cone_lossless: the lossless truncated cone in the textbook apex-distance
  form (a different algebraic form from the engine's).
* hudde_engel: the Hudde & Engel eardrum impedance (COMSOL guide 6.4
  Eqs. 2-34/2-35, Table 2-8).
* iec60318_4, type33, type43: the ear-simulator models of earmodels.py and
  type43.py with the committed data files.

Air: "standard_23C" (23 degC, 101.325 kPa) unless stated.
"""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import earmodels as em  # noqa: E402
import type43 as t43  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "acoustilab" / "tests" / "data" / "ear_reference.json"


def cx(z):
    return [float(np.real(z)), float(np.imag(z))]


def mat(t):
    return [cx(t[0, 0]), cx(t[0, 1]), cx(t[1, 0]), cx(t[1, 1])]


def piecewise_radius(xs, rs):
    xs = np.asarray(xs, float)
    rs = np.asarray(rs, float)
    order = np.argsort(xs)

    def r(x):
        return float(np.interp(x, xs[order], rs[order]))

    return r


def main():
    air = em.Air.standard_23c()
    freqs = [20.0, 200.0, 2000.0, 10000.0, 20000.0]
    out = {"air": "standard_23C", "generator": "tools/ear/gen_fixtures.py"}

    # --- Webster references ---------------------------------------------------
    horns = []
    # A single cone, 2 mm -> 4 mm radius over 10 mm.
    cone = {"name": "cone_2_4_10", "positions_mm": [0.0, 10.0], "radii_mm": [2.0, 4.0]}
    # The P.57 Type 4.3 canal from the reference plane to the DRP.
    px, pa = t43.sub_profile(t43.GEOM["ref_plane_mm"] * 1e-3, t43.GEOM["drp_axial_mm"] * 1e-3)
    p57 = {
        "name": "p57_ref_to_drp",
        "positions_mm": [x * 1e3 for x in px],
        "radii_mm": [math.sqrt(a / math.pi) * 1e3 for a in pa],
    }
    for h in (cone, p57):
        xs = np.array(h["positions_mm"]) * 1e-3
        rs = np.array(h["radii_mm"]) * 1e-3
        rfun = piecewise_radius(xs, rs)
        entries = []
        for f in freqs:
            w = 2 * math.pi * f
            # Integrate along increasing x from positions[0] to positions[-1]:
            # map the traversal coordinate s in [0, L] to x.
            x0, x1 = xs[0], xs[-1]
            sgn = 1.0 if x1 > x0 else -1.0
            length = abs(x1 - x0)
            t = em.webster_abcd(lambda s: rfun(x0 + sgn * s), 0.0, length, air, w)
            entries.append({"f_Hz": f, "T": mat(t)})
        h["reference"] = entries
        horns.append(h)
    out["webster"] = horns

    # --- Lossless cone, textbook form -----------------------------------------------
    lossless = []
    for (r1, r2, L) in [(2e-3, 4e-3, 10e-3), (4e-3, 1.5e-3, 7e-3)]:
        for f in [100.0, 3000.0, 15000.0]:
            t = em.cone_abcd_lossless(r1, r2, L, air, 2 * math.pi * f)
            lossless.append({"r1_mm": r1 * 1e3, "r2_mm": r2 * 1e3, "length_mm": L * 1e3, "f_Hz": f, "T": mat(t)})
    out["cone_lossless"] = lossless

    # --- Hudde & Engel ----------------------------------------------------------------
    he = em.HuddeEngel()
    hf = [10.0, 50.0, 150.0, 500.0, 1000.0, 1499.0, 1501.0, 2000.0, 3000.0, 5000.0, 8000.0, 12000.0, 16000.0, 20000.0, 40000.0]
    out["hudde_engel"] = [{"f_Hz": f, "Z": cx(he.impedance(2 * math.pi * f, air.bulk))} for f in hf]

    # --- IEC 60318-4 ----------------------------------------------------------------------
    d711 = json.loads((ROOT / "data" / "ear" / "iec60318_4.json").read_text())
    s = d711["fit"]["side_volume_scale"]
    h2 = d711["slit_heights_mm"]["h2"] * 1e-3
    h4 = d711["slit_heights_mm"]["h4"] * 1e-3
    base = em.Iec711()
    m711 = em.Iec711(d1=base.d1 * s, d2=base.d2 * s, h2=h2, h4=h4)
    m711_rigid = em.Iec711(d1=base.d1 * s, d2=base.d2 * s, h2=h2, h4=h4, mic="rigid")
    cf = [20.0, 100.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 13500.0, 20000.0]
    out["iec60318_4"] = [
        {
            "f_Hz": f,
            "transfer": cx(m711.transfer(air, 2 * math.pi * f)),
            "input": cx(m711.input(air, 2 * math.pi * f)),
            "transfer_rigid_mic": cx(m711_rigid.transfer(air, 2 * math.pi * f)),
        }
        for f in cf
    ]
    # Type 3.3: 10 mm x 7.5 mm extension in front of the coupler.
    ext = []
    for f in cf:
        w = 2 * math.pi * f
        te = em.tube_abcd("circle", 3.75e-3, math.pi * 3.75e-3**2, 10e-3, air, w)
        t = em.chain([te, m711.abcd(air, w)])
        ext.append({"f_Hz": f, "transfer": cx(em.transfer_impedance(t, m711.mic_z(w)))})
    out["type33"] = ext

    # --- Type 4.3 -----------------------------------------------------------------------------
    p = json.loads((ROOT / "data" / "ear" / "type43_drum.json").read_text())["parameters"]
    drum = t43.Type43Drum(
        v_m=p["C_m_volume_cm3"] * 1e-6,
        r_o=p["R_o_Pa_s_per_m3"],
        m_o=p["M_o_kg_per_m4"],
        v_t=p["C_t_volume_cm3"] * 1e-6,
        r_a=p["R_a_Pa_s_per_m3"],
        v_a=p["C_a_volume_cm3"] * 1e-6,
    )
    m43 = t43.Type43(drum, n_segments=48)
    tf = [20.0, 100.0, 500.0, 1000.0, 3000.0, 6000.0, 10600.0, 15000.0, 20000.0]
    out["type43"] = [
        {
            "f_Hz": f,
            "transfer_ref": cx(m43.transfer_ref(f)),
            "input_ref": cx(m43.input_ref(f)),
            "transfer_eep": cx(m43.transfer_eep(f)),
            "drum": cx(drum.impedance(2 * math.pi * f, air.bulk)),
        }
        for f in tf
    ]
    m43he = t43.Type43(em.HuddeEngel(), n_segments=48)
    out["type43_hudde_engel"] = [{"f_Hz": f, "transfer_ref": cx(m43he.transfer_ref(f))} for f in tf]

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(out, indent=1) + "\n")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
