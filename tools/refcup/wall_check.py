#!/usr/bin/env python3
"""Margins of the open reference cup's walls against its cavities and leaks.

The model treats every printed wall as rigid. This script checks that the
walls are stiff enough for that, from validation/reference_cup/dimensions.json
(geometry and material data), with closed forms:

* back plate of the rear chamber, a circular plate clamped at its rim
  (radius a, thickness h, D = E h^3 / (12 (1 - nu^2))): volume compliance
  C = pi a^6 / (192 D) under uniform pressure, first resonance
  f1 = 10.2158 / (2 pi a^2) * sqrt(D / (rho h))  (Leissa, Vibration of
  Plates, NASA SP-160, 1969, Table 2.1: lambda^2 = 10.2158);
* side wall of the rear shell, a thin cylinder: dV/V = 2 r p / (E t);
* against the cavity's air compliance V / (rho c^2) and the impedance of the
  rear vent's mesh, R_s / A (the smallest vent resistance);
* the gasket's own compliance, bounded by its gas volume at P0 (isothermal),
  as if all of it were exposed to the cavity pressure.

    python3 tools/refcup/wall_check.py

Standard library only. Air: 23 C, 101.325 kPa (rho c^2 = 1.4 * P0).
"""
import json
import math
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
P0 = 101325.0
RHO_C2 = 1.4 * P0  # adiabatic bulk modulus of air, Pa
MESH_RAYL = 260.0  # SAATI Acoustex 260, data/materials/meshes.json


def main():
    d = json.loads((ROOT / "validation/reference_cup/dimensions.json").read_text())
    dim = {k: v["value"] for k, v in d["dimensions"].items()}
    mat = d["materials"]["print"]
    E = mat["E_GPa"] * 1e9
    nu = mat["poisson"]
    rho = mat["density_kg_per_m3"]
    mm = 1e-3
    a = dim["cup_inner_diameter_mm"] / 2 * mm
    h = dim["back_plate_mm"] * mm
    t = dim["shell_wall_mm"] * mm
    depth = dim["rear_depth_mm"] * mm
    v_rear = math.pi * a * a * depth
    c_air = v_rear / RHO_C2

    D = E * h**3 / (12 * (1 - nu * nu))
    c_plate = math.pi * a**6 / (192 * D)
    f1 = 10.2158 / (2 * math.pi * a * a) * math.sqrt(D / (rho * h))
    c_wall = v_rear * 2 * a / (E * t)
    r_mesh = MESH_RAYL / (math.pi * (dim["rear_mesh_hole_diameter_mm"] * mm / 2) ** 2)

    print(f"rear chamber: {v_rear * 1e6:.1f} cm3, air compliance {c_air:.3e} m3/Pa")
    print(f"back plate ({h / mm:.0f} mm, clamped, radius {a / mm:.0f} mm): compliance {c_plate:.3e} m3/Pa"
          f" = {100 * c_plate / c_air:.3f} % of the air's; first resonance {f1:.0f} Hz")
    print(f"side wall ({t / mm:.0f} mm): compliance {c_wall:.3e} m3/Pa = {100 * c_wall / c_air:.3f} % of the air's")
    for f in (20.0, 100.0, 1000.0):
        w = 2 * math.pi * f
        z_plate = 1 / (w * c_plate)
        print(f"  at {f:5.0f} Hz the back plate passes {100 * r_mesh / z_plate:.4f} % of the flow of the meshed rear vent"
              f" (|Z| {z_plate:.2e} against {r_mesh:.2e} Pa s/m3)")
    ms = rho * h
    m_ac = ms / (math.pi * a * a)
    w = 2 * math.pi * 10000
    print(f"  above {f1:.0f} Hz it is mass-controlled: at 10 kHz |Z| = {w * m_ac:.2e} Pa s/m3,"
          f" {100 * r_mesh / (w * m_ac):.2f} % of the vent's flow")

    g = d["materials"]["gasket"]
    ri, ro = a, a + dim["pad_ring_width_mm"] * mm
    v_gasket = math.pi * (ro * ro - ri * ri) * dim["gasket_compressed_mm"] * mm
    c_gasket = g["void_fraction"] * v_gasket / P0
    v_front = math.pi * ri * ri * (dim["pad_ring_height_mm"] + dim["gasket_compressed_mm"]) * mm
    print(f"gasket: at most {c_gasket:.3e} m3/Pa, {100 * c_gasket / (v_front / RHO_C2):.1f} % of the front cavity's"
          f" compliance (all its gas exposed; the real share is smaller and the acceptance fit absorbs it in the front volume)")


if __name__ == "__main__":
    main()
