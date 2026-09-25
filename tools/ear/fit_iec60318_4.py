#!/usr/bin/env python3
"""Literature model of the IEC 60318-4 occluded-ear simulator: one-parameter fit.

Topology (Nielsen et al., AES 116th Conv. 2004, paper 6162; Luan et al., Acta
Acustica united with Acustica 105(6) 1258-1268, 2019, Fig. 2 and Eq. 6):
three sections of the main cylindrical cavity in cascade with two shunt
Helmholtz resonators (narrow slit + annular side cavity), terminated by the
microphone.

Geometry: mean micro-CT dimensions of a G.R.A.S. RA0045 from Luan et al.
(2019) Table 1 (accepted manuscript, https://espace2.etsmtl.ca/20020/),
except the two slit heights. The spec (p. 25) asks for element values "fitted
to the standard's Table 1 rather than trusted from slit heights whose cube is
uncertain", and Luan's heights (0.16 +- 0.06 and 0.05 +- 0.02 mm) are the
least certain dimensions. The heights used are those of the COMSOL "Generic
711 Coupler" model documentation ("The slit heights are h1 = 69 um and
h2 = 170 um": the 170 um rectangular duct and the 69 um annular slit), a
model that documentation shows complying with the IEC standard curve. Both
lie inside Luan's stated ranges.
Microphone: B&K Type 4192 acoustic impedance (C = 0.62e-13 m^5/N,
R = 119e6 N s/m^5, L = 710 kg/m^4) from the same COMSOL documentation, after
the B&K Microphone Handbook (1995).

Only publicly stated facts are used as targets:
  * effective volume 1260 mm^3 at 500 Hz (GRAS RA0045 product data; the
    widely quoted IEC 60318-4 value) -- the one fitted parameter, a common
    scale on the two side-cavity volumes (their thicknesses d1, d2), is set
    by it;
  * half-wave resonance of the main cavity at about 13.5 kHz (COMSOL Generic
    711 Coupler documentation; checked, not fitted).

Check against an openly published curve: if private/iec60318_4_comsol_fig3.json
exists (tools/ear/digitize_comsol_711.py, digitised from the COMSOL
documentation's Fig. 3, a plot of the IEC 60318-4 Table 1 curve with its
tolerances; untracked), the model's transfer impedance is compared with it at
the 21 third-octave frequencies and a summary (no table values) is recorded.

Output: data/ear/iec60318_4.json.
"""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import numpy as np
from scipy.optimize import brentq

sys.path.insert(0, str(Path(__file__).resolve().parent))
import earmodels as em  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "data" / "ear" / "iec60318_4.json"
CURVE = ROOT / "private" / "iec60318_4_comsol_fig3.json"
V_TARGET = 1260e-9
# COMSOL Generic 711 Coupler documentation: 170 um rectangular slit (branch 2),
# 69 um annular slit (branch 4).
H2_COMSOL = 0.170e-3
H4_COMSOL = 0.069e-3
# Digitising accuracy of the published curve (one pixel is 0.093 dB).
READING_DB = 0.1


def model(scale: float, h2: float = H2_COMSOL, h4: float = H4_COMSOL, mic: str = "bk4192") -> em.Iec711:
    base = em.Iec711()
    return em.Iec711(d1=base.d1 * scale, d2=base.d2 * scale, h2=h2, h4=h4, mic=mic)


def half_wave(m: em.Iec711, air: em.Air) -> float:
    fs = np.linspace(9000.0, 18000.0, 9001)
    zz = np.array([abs(m.transfer(air, 2 * math.pi * f)) for f in fs])
    return float(fs[np.argmax(zz)])


def fit_scale(h2: float, h4: float, air: em.Air) -> tuple[float, float]:
    w = 2 * math.pi * 500.0

    def veff(s):
        return em.effective_volume(model(s, h2, h4).transfer(air, w), air, w)

    return brentq(lambda s: veff(s) - V_TARGET, 0.8, 3.0, xtol=1e-14), veff(1.0)


def check_curve(m: em.Iec711, air: em.Air) -> dict | None:
    """Deviation from the digitised published curve, as a summary."""
    if not CURVE.exists():
        return None
    rows = json.loads(CURVE.read_text())["rows"]
    worst, inside, lines = 0.0, 0, []
    for r in rows:
        f = r["f_Hz"]
        lv = 20 * math.log10(abs(m.transfer(air, 2 * math.pi * f)) / 1e6)
        d = lv - r["nominal_dB"]
        half = (r["upper_dB"] - r["nominal_dB"]) if d > 0 else (r["nominal_dB"] - r["lower_dB"])
        worst = max(worst, abs(d) / half)
        ok = r["lower_dB"] - READING_DB <= lv <= r["upper_dB"] + READING_DB
        inside += ok
        lines.append(f"{f:6.0f} Hz  dev {d:+5.2f} dB  band [{r['lower_dB'] - r['nominal_dB']:+.2f}, "
                     f"{r['upper_dB'] - r['nominal_dB']:+.2f}]{'' if ok else '  OUTSIDE'}")
    return {"points_within_tolerance": inside, "points": len(rows),
            "max_deviation_over_tolerance": round(worst, 2), "lines": lines}


def main():
    air = em.Air.standard_23c()
    s, v0 = fit_scale(H2_COMSOL, H4_COMSOL, air)
    m = model(s)
    fr = half_wave(m, air)
    base = em.Iec711()
    # For the record: the one-parameter fit with Luan's own slit heights (the
    # previous version of this model) and how it compares with the curve.
    s_luan, v0_luan = fit_scale(base.h2, base.h4, air)
    check = check_curve(m, air)
    check_luan = check_curve(model(s_luan, base.h2, base.h4), air)
    for name, c in (("COMSOL slit heights", check), ("Luan slit heights", check_luan)):
        if c:
            print(f"{name}: {c['points_within_tolerance']}/{c['points']} within tolerance, "
                  f"max |dev|/tol {c['max_deviation_over_tolerance']}")
            print("\n".join("   " + ln for ln in c["lines"]))
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
        "slit_heights_mm": {
            "source": "COMSOL 'Generic 711 Coupler -- An Occluded Ear-Canal Simulator' model "
            "documentation (6.0): 'The slit heights are h1 = 69 um and h2 = 170 um' (170 um "
            "rectangular duct, 69 um annular slit). They replace Luan's h2 = 0.16 +- 0.06 mm and "
            "h4 = 0.05 +- 0.02 mm, inside those ranges (spec p. 25: slit heights whose cube is "
            "uncertain are not trusted).",
            "h2": round(H2_COMSOL * 1e3, 9),
            "h4": round(H4_COMSOL * 1e3, 9),
        },
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
            "side_volume_scale_with_luan_slit_heights": s_luan,
            "unfitted_effective_volume_500Hz_with_luan_slit_heights_mm3": v0_luan * 1e9,
        },
    }
    if check:
        out["check_against_published_curve"] = {
            "curve": "IEC 60318-4 standard curve as plotted in the COMSOL Generic 711 Coupler "
            "documentation Fig. 3, digitised by tools/ear/digitize_comsol_711.py (private/)",
            "reading_margin_dB": READING_DB,
            "points_within_tolerance": check["points_within_tolerance"],
            "points": check["points"],
            "max_deviation_over_tolerance": check["max_deviation_over_tolerance"],
            "with_luan_slit_heights": {
                "points_within_tolerance": check_luan["points_within_tolerance"],
                "max_deviation_over_tolerance": check_luan["max_deviation_over_tolerance"],
            },
        }
    OUT.write_text(json.dumps(out, indent=2) + "\n")
    print(json.dumps(out["fit"], indent=2))


if __name__ == "__main__":
    main()
