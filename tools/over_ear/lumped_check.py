#!/usr/bin/env python3
"""Independent lumped model of the over-ear template, sealed and vented.

Checks the explanation in docs/over-ear-template.md against the engine. The
model shares no code with the engine: numpy, Poiseuille and flanged end
corrections, and the driver's primary set.

* Driver: Re, Mms, Sd, fs, Qms, Qes (Tymphany HPD-40N16PET00-32 primary set,
  erratum E5), so Cms = 1/(ws^2 Mms), Rms = ws Mms/Qms, Bl^2 = ws Mms Re/Qes.
* Front node: the front cavity plus the IEC 60318-4 nominal effective volume
  (1.26 cm^3) as one compliance, shunted by the pad leak as a Poiseuille slit.
* Rear node: the rear cavity, shunted by the rear vent's mesh resistance.
* Baffle vents between the two nodes: the mesh resistance R_s/A in series
  with the air mass rho (L + 2 * 0.8216 a)/A (flanged ends).
* The diaphragm injects U = Sd v into the front node and draws it from the
  rear node; Zm = j w Mms + Rms + 1/(j w Cms) + Sd^2 (p_front - p_rear)/U.

The engine is run at level 0 (lumped cavities), where both models describe
the same network. Agreement: 0.3 dB in front pressure and 0.1 ohm in input
impedance from 30 Hz to 3.6 kHz for the vented default; for the sealed cup
the same away from its coupled resonance, which the simple model damps less
(no thermal wall loss or vent air mass).

Run from the repository root after `cargo build -p acoustilab-cli --release`:
    python3 tools/over_ear/lumped_check.py
"""

import csv
import math
import subprocess
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "target/release/acoustilab"
TEMPLATE = ROOT / "examples/design_over_ear.json"

# Air: the template's standard_23C preset (dry air, 23 C, 101.325 kPa).
T, P0, R_AIR, GAMMA, MU = 296.15, 101325.0, 287.05, 1.4, 1.8e-5
RHO = P0 / (R_AIR * T)
GP0 = GAMMA * P0

# Driver primary set.
RE, MMS, SD, FS, QMS, QES = 32.8, 0.3e-3, 10e-4, 81.8, 2.71, 1.01
WS = 2 * math.pi * FS
CMS = 1 / (WS**2 * MMS)
RMS = WS * MMS / QMS
BL = math.sqrt(WS * MMS * RE / QES)

# Template geometry (default parameter values).
V_FRONT = math.pi * 25e-3**2 * 15e-3
V_EAR = 1.26e-6
V_REAR = 25e-6
C_F = (V_FRONT + V_EAR) / GP0
C_R = V_REAR / GP0

# Pad leak: slit of perimeter 2 pi 25 mm, depth 15 mm, gap 0.08 mm.
R_LEAK = 12 * MU * 15e-3 / (2 * math.pi * 25e-3 * (0.08e-3) ** 3)
# Rear vent: one 3 mm hole under 160 rayl.
R_VENT = 160 / (math.pi * 1.5e-3**2)
# Baffle vents: four 4 mm holes in a 2 mm baffle under 65 rayl.
N_B, A_B, L_B, RS_B = 4, 2e-3, 2e-3, 65.0
AREA_B = N_B * math.pi * A_B**2
R_B = RS_B / AREA_B
M_B = RHO * (L_B + 2 * 0.8216 * A_B) / AREA_B

EMF = math.sqrt(1e-3 * 32.0)  # 1 mW into the 32 ohm rated impedance


def model(f, baffle):
    w = 2 * math.pi * f
    yf = 1j * w * C_F + 1 / R_LEAK
    yr = 1j * w * C_R + 1 / R_VENT
    yb = 1 / (R_B + 1j * w * M_B) if baffle else 0.0
    y = np.array([[yf + yb, -yb], [-yb, yr + yb]])
    p_front, p_rear = np.linalg.solve(y, np.array([1.0, -1.0]))
    zm = 1j * w * MMS + RMS + 1 / (1j * w * CMS) + SD**2 * (p_front - p_rear)
    zin = RE + BL**2 / zm
    u = SD * BL * (EMF / zin) / zm
    return zin, u * p_front


def engine(sets):
    cmd = [str(BIN), "solve", str(TEMPLATE), "--csv", "--set", "fidelity=0",
           "--set", "points_per_octave=12"]
    for s in sets:
        cmd += ["--set", s]
    out = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
    return list(csv.DictReader(out.splitlines()))


def main():
    print(f"Vas = {GP0 * SD**2 * CMS * 1e3:.3f} L; baffle R = {R_B:.3e} Pa s/m^3, "
          f"M = {M_B:.1f} kg/m^4, Sd^2 R = {SD**2 * R_B:.3f} N s/m")
    c_s = C_F * C_R / (C_F + C_R)
    print(f"f_H = 1/(2 pi sqrt(M C_s)) = {1 / (2 * math.pi * math.sqrt(M_B * c_s)):.0f} Hz")
    for label, sets, baffle, tol_db, tol_ohm in (
        ("vented (default)", [], True, 0.3, 0.1),
        ("sealed (baffle_vent_count=0)", ["baffle_vent_count=0"], False, 0.3, 0.1),
    ):
        worst_db = worst_ohm = 0.0
        print(label)
        for k, row in enumerate(engine(sets)):
            f = float(row["frequency_Hz"])
            if f < 30 or f > 3600:
                continue
            zin, p = model(f, baffle)
            spl = 20 * math.log10(abs(p) / 2e-5)
            d_db = spl - float(row["p_front_spl_dB"])
            d_ohm = abs(zin) - float(row["zin_mag"])
            near_peak = not baffle and 600 < f < 1400
            if not near_peak:
                worst_db = max(worst_db, abs(d_db))
                worst_ohm = max(worst_ohm, abs(d_ohm))
            if k % 6 == 0:
                print(f"  {f:7.0f} Hz  |Zin| {abs(zin):6.2f} (engine {float(row['zin_mag']):6.2f})"
                      f"  p_front {spl:6.1f} dB (engine {float(row['p_front_spl_dB']):6.1f})")
        print(f"  largest difference: {worst_db:.2f} dB, {worst_ohm:.3f} ohm"
              + (" (600-1400 Hz excluded)" if not baffle else ""))
        assert worst_db < tol_db and worst_ohm < tol_ohm, label


if __name__ == "__main__":
    main()
