#!/usr/bin/env python3
"""Literature model of the IEC 60318-4 occluded-ear simulator: one-parameter fit.

Topology (Nielsen et al., AES 116th Conv. 2004, paper 6162; Luan et al., Acta
Acustica united with Acustica 105(6) 1258-1268, 2019, Fig. 2 and Eq. 6):
three sections of the main cylindrical cavity in cascade with two shunt
Helmholtz resonators (narrow slit + annular side cavity), terminated by the
microphone.

Geometry: mean micro-CT dimensions of a G.R.A.S. RA0045 from Luan et al.
(2019) Table 1 (accepted manuscript, https://espace2.etsmtl.ca/20020/).
Microphone: B&K Type 4192 acoustic impedance (C = 0.62e-13 m^5/N,
R = 119e6 N s/m^5, L = 710 kg/m^4) as used in the COMSOL "Generic 711
Coupler" model documentation, after the B&K Microphone Handbook (1995).

Only publicly stated facts are used as targets:
  * effective volume 1260 mm^3 at 500 Hz (GRAS RA0045 product data; the
    widely quoted IEC 60318-4 value),
  * half-wave resonance of the main cavity at about 13.5 kHz (COMSOL Generic
    711 Coupler documentation; the model is checked, not fitted, against it).

With the Luan et al. dimensions the model gives about 1068 mm^3 at 500 Hz.
No slit height within the stated micro-CT uncertainty reaches 1260 mm^3,
so the one fitted parameter is a common scale factor on the two side-cavity
volumes (applied to their thicknesses d1, d2). The result is written to
data/ear/iec60318_4.json. This is a literature model, not verified against
the IEC 60318-4 Table 1.
"""

from __future__ import annotations

import json
import math
import sys
from dataclasses import asdict
from pathlib import Path

import numpy as np
from scipy.optimize import brentq

sys.path.insert(0, str(Path(__file__).resolve().parent))
import earmodels as em  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "data" / "ear" / "iec60318_4.json"
V_TARGET = 1260e-9


def model(scale: float) -> em.Iec711:
    base = em.Iec711()
    return em.Iec711(d1=base.d1 * scale, d2=base.d2 * scale)


def half_wave(m: em.Iec711, air: em.Air) -> float:
    fs = np.linspace(9000.0, 18000.0, 9001)
    zz = np.array([abs(m.transfer(air, 2 * math.pi * f)) for f in fs])
    return float(fs[np.argmax(zz)])


def main():
    air = em.Air.standard_23c()
    w = 2 * math.pi * 500.0

    def veff(s):
        return em.effective_volume(model(s).transfer(air, w), air, w)

    v0 = veff(1.0)
    s = brentq(lambda s: veff(s) - V_TARGET, 0.8, 3.0, xtol=1e-14)
    m = model(s)
    fr = half_wave(m, air)
    fr0 = half_wave(model(1.0), air)
    base = em.Iec711()
    out = {
        "source": "Geometry: Luan, Sgard, Benacchio, Nelisse and Doutres, 'A transfer matrix model of "
        "the IEC 60318-4 ear simulator', Acta Acustica united with Acustica 105(6) 1258-1268 (2019), "
        "Table 1 mean values (G.R.A.S. RA0045, micro-CT). Topology after Nielsen, Schuhmacher, Liu "
        "and Jonsson, AES 116th Convention (2004) paper 6162, and Luan et al. Fig. 2 / Eq. 6.",
        "status": "literature model, not verified against the IEC 60318-4 Table 1",
        "geometry_mm": {
            k: round(v * 1e3, 9)
            for k, v in {
                "R0": base.r0, "L1": base.l1, "L3": base.l3, "L5": base.l5,
                "a2": base.a2, "b2": base.b2, "h2": base.h2,
                "r2": base.r2, "R2": base.big_r2, "d1": base.d1,
                "r4": base.r4, "h4": base.h4, "R4": base.big_r4, "d2": base.d2,
            }.items()
        },
        "annular_slit": {"parts": base.parts4, "part_angle_deg": base.alpha4_deg},
        "microphone_bk4192": {
            "source": "COMSOL Generic 711 Coupler model documentation (B&K Microphone Handbook, "
            "Falcon range, 1995, pp. 6-18)",
            "C_m3_per_Pa": 0.62e-13, "R_Pa_s_per_m3": 119e6, "M_kg_per_m4": 710.0,
        },
        "fit": {
            "parameter": "side_volume_scale (multiplies both side-cavity thicknesses d1, d2)",
            "side_volume_scale": s,
            "target": "effective volume 1260 mm^3 at 500 Hz (GRAS RA0045 product data)",
            "air": "standard_23C",
            "unfitted_effective_volume_500Hz_mm3": v0 * 1e9,
            "half_wave_resonance_Hz": fr,
            "half_wave_resonance_unfitted_Hz": fr0,
        },
    }
    OUT.write_text(json.dumps(out, indent=2) + "\n")
    print(json.dumps(out["fit"], indent=2))


if __name__ == "__main__":
    main()
