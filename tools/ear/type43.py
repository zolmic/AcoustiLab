"""Type 4.3 ear-simulator model (Python reference) built from data/ear/type43_geometry.json.

Layout along the curved centre line (positions from the canal tip, P.57 Table 6):

    EEP (30.68 mm) --outer canal--> reference plane (17.33 mm) --inner canal--> DRP (4.02 mm)
                                                                             |  drum impedance
                                                                             |  tip stub (DRP -> 0.5 mm, rigid end)

The drum impedance acts at the DRP's axial position (the inclined drum is
lumped at the microphone plane); the wedge between the DRP plane and the tip
is a rigid-ended conical stub in parallel with it.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass
from pathlib import Path

import numpy as np

import earmodels as em

ROOT = Path(__file__).resolve().parents[2]
GEOM = json.loads((ROOT / "data" / "ear" / "type43_geometry.json").read_text())


def knots():
    xs = np.array([s["position_mm"] for s in GEOM["sections"]]) * 1e-3
    areas = np.array([s["area_mm2"] for s in GEOM["sections"]]) * 1e-6
    return xs, areas


def area_at(x):
    """Area at axial position x (m), radius interpolated linearly between knots."""
    xs, areas = knots()
    r = np.sqrt(areas / math.pi)
    return math.pi * float(np.interp(x, xs, r)) ** 2


def sub_profile(x_from, x_to):
    """Knots from x_from to x_to (either direction), ends interpolated."""
    xs, areas = knots()
    lo, hi = min(x_from, x_to), max(x_from, x_to)
    inner = [(x, a) for x, a in zip(xs, areas) if lo + 1e-12 < x < hi - 1e-12]
    pts = [(lo, area_at(lo))] + inner + [(hi, area_at(hi))]
    if x_from > x_to:
        pts = pts[::-1]
    return [p[0] for p in pts], [p[1] for p in pts]


def round_half_up(x: float) -> int:
    return int(math.floor(x + 0.5))


@dataclass
class Type43:
    drum: object
    n_segments: int = 48
    air: em.Air = None
    x_drp: float = None

    def __post_init__(self):
        self.air = self.air or em.Air.standard_23c()
        self.x_tip = GEOM["sections"][0]["position_mm"] * 1e-3
        self.x_ref = GEOM["ref_plane_mm"] * 1e-3
        self.x_eep = GEOM["eep_mm"] * 1e-3
        if self.x_drp is None:
            self.x_drp = GEOM["drp_axial_mm"] * 1e-3

    def _n(self, length):
        total = self.x_eep - self.x_tip
        return max(1, round_half_up(self.n_segments * length / total))

    def canal(self, x_from, x_to, omega):
        px, pa = sub_profile(x_from, x_to)
        return em.canal_abcd(px, pa, self._n(abs(x_to - x_from)), self.air, omega)

    def load_at_drp(self, omega):
        t_tip = self.canal(self.x_drp, self.x_tip, omega)
        y_tip = t_tip[1, 0] / t_tip[0, 0]  # rigid end: Z_in = A / C
        y_drum = 1.0 / self.drum.impedance(omega, self.air.bulk)
        return 1.0 / (y_tip + y_drum)

    def transfer_ref(self, f):
        """p_DRP / U_ref for a volume velocity injected at the reference plane."""
        w = 2 * math.pi * f
        return em.transfer_impedance(self.canal(self.x_ref, self.x_drp, w), self.load_at_drp(w))

    def input_ref(self, f):
        w = 2 * math.pi * f
        return em.z_in(self.canal(self.x_ref, self.x_drp, w), self.load_at_drp(w))

    def transfer_eep(self, f):
        """p_DRP / U_eep for a volume velocity injected at the EEP."""
        w = 2 * math.pi * f
        t = em.chain([self.canal(self.x_eep, self.x_ref, w), self.canal(self.x_ref, self.x_drp, w)])
        return em.transfer_impedance(t, self.load_at_drp(w))


@dataclass
class Type43Drum:
    """Drum network fitted to P.57 Table 5-c (see tools/ear/fit_type43.py).

      Z = Z_cav + 1 / (jw C_m + 1 / (R_o + jw M_o))
      Z_cav = 1 / (jw C_t + 1 / (R_a + 1 / (jw C_a)))

    C_m: drum-membrane compliance; R_o, M_o: resistance and inertance of the
    path through which the drum couples to the middle-ear cavity; C_t: the
    exposed middle-ear (tympanic) cavity; R_a, C_a: aditus resistance and
    antrum volume.
    Compliances are equivalent air volumes, C = V / (gamma P0).
    """

    v_m: float
    r_o: float
    m_o: float
    v_t: float
    r_a: float
    v_a: float

    def impedance(self, omega, gamma_p0):
        jw = 1j * omega
        z_cav = 1.0 / (jw * self.v_t / gamma_p0 + 1.0 / (self.r_a + gamma_p0 / (jw * self.v_a)))
        return z_cav + 1.0 / (jw * self.v_m / gamma_p0 + 1.0 / (self.r_o + jw * self.m_o))
