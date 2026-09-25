"""Reference (Python) implementations of the AcoustiLab ear loads.

These are written independently of the Rust engine so that the fixtures they
generate are a genuine cross-check. Conventions follow docs/conventions.md:
time dependence e^{+j omega t}, RMS phasors, SI units, pressure / volume
velocity as across / through.

Contents
  * Air           - the two presets of crates/acoustilab/src/air.rs.
  * medium()      - Zwikker-Kosten / Stinson low-reduced-frequency medium for
                    circular ducts and slits (modified Bessel form, erratum E1).
  * cone_abcd()   - exact lossless truncated-cone transfer matrix with the
                    thermoviscous medium of one radius substituted.
  * webster_abcd()- independent reference: numerical integration of the lossy
                    horn equations with the medium evaluated at the local radius.
  * hudde_engel() - the Hudde & Engel (1998) eardrum impedance, as documented
                    in the COMSOL Acoustics Module User's Guide 6.4, Eqs. 2-34
                    and 2-35 and Table 2-8.
  * iec711()      - literature transfer-matrix model of the IEC 60318-4
                    occluded-ear simulator (geometry of Luan et al. 2019).
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field, replace

import numpy as np
from scipy import integrate, special

J = 1j


# ----- Air ---------------------------------------------------------------------


@dataclass(frozen=True)
class Air:
    rho: float
    c: float
    mu: float
    gamma: float
    prandtl: float
    p0: float

    @property
    def bulk(self) -> float:
        return self.gamma * self.p0

    @staticmethod
    def spec_reference() -> "Air":
        rho, c, gamma = 1.204, 343.0, 1.4
        return Air(rho, c, 1.81e-5, gamma, 0.71, rho * c * c / gamma)

    @staticmethod
    def conditions(t_k: float, p0: float) -> "Air":
        r_air, cp = 287.05, 1005.0
        gamma = 1.4
        rho = p0 / (r_air * t_k)
        c = math.sqrt(gamma * r_air * t_k)
        mu = 1.458e-6 * t_k**1.5 / (t_k + 110.4)
        kappa = 2.646e-3 * t_k**1.5 / (t_k + 245.4 * 10 ** (-12.0 / t_k))
        return Air(rho, c, mu, gamma, mu * cp / kappa, p0)

    @staticmethod
    def standard_23c() -> "Air":
        return Air.conditions(296.15, 101_325.0)

    def thermal_layer(self, omega: float) -> float:
        return math.sqrt(2.0 * self.mu / (self.rho * omega)) / math.sqrt(self.prandtl)


# ----- Thermoviscous medium ---------------------------------------------------------


def _circle_f(z):
    """2 I1(z) / (z I0(z)) and 1 - F = I2/I0, exponentially scaled for stability."""
    z = np.asarray(z, dtype=complex)
    r1 = special.ive(1, z) / special.ive(0, z)
    r2 = special.ive(2, z) / special.ive(0, z)
    return 2.0 * r1 / z, r2


def _slit_f(z):
    z = np.asarray(z, dtype=complex)
    f = np.tanh(z) / z
    return f, 1.0 - f


def medium(shape: str, s: float, air: Air, omega: float):
    """(rho_eff, K_eff) for a circle of radius s or a slit of half-gap s."""
    kv = np.sqrt(J * omega * air.rho / air.mu)
    kt = kv * math.sqrt(air.prandtl)
    fn = _circle_f if shape == "circle" else _slit_f
    _, om_v = fn(kv * s)
    f_t, _ = fn(kt * s)
    rho_eff = air.rho / om_v
    k_eff = air.bulk / (1.0 + (air.gamma - 1.0) * f_t)
    return complex(rho_eff), complex(k_eff)


def propagation(rho_eff: complex, k_eff: complex, omega: float):
    """Gamma (Re >= 0) and the specific characteristic impedance sqrt(rho K)."""
    s = np.sqrt(rho_eff / k_eff)
    if s.imag > 0:
        s = -s
    return J * omega * s, k_eff * s


# ----- Ducts -------------------------------------------------------------------------


def tube_abcd(shape: str, s: float, area: float, length: float, air: Air, omega: float):
    rho, k = medium(shape, s, air, omega)
    g, z0 = propagation(rho, k, omega)
    zc = z0 / area
    gl = g * length
    return np.array([[np.cosh(gl), zc * np.sinh(gl)], [np.sinh(gl) / zc, np.cosh(gl)]])


def cone_abcd(r1: float, r2: float, length: float, air: Air, omega: float, r_loss=None):
    """Truncated cone, radius r1 -> r2 over `length`, medium of radius r_loss.

    Exact for a cone filled with a uniform effective medium (the horn equation
    with S ~ x^2 reduces to q'' = Gamma^2 q for q = x p). Written with the
    inverse apex distances so that r1 == r2 (cylinder) needs no special case.
    """
    if r_loss is None:
        r_loss = 0.5 * (r1 + r2)
    rho, k = medium("circle", r_loss, air, omega)
    g, z0 = propagation(rho, k, omega)
    ix1 = (r2 - r1) / (r1 * length)
    ix2 = (r2 - r1) / (r2 * length)
    gl = g * length
    ch, sh = np.cosh(gl), np.sinh(gl)
    s12 = math.pi * r1 * r2
    a = (r2 / r1) * ch - ix1 * sh / g
    d = (r1 / r2) * ch + ix2 * sh / g
    b = z0 * sh / s12
    c = (s12 / z0) * ((1.0 - ix1 * ix2 / g**2) * sh + length * ix1 * ix2 * ch / g)
    return np.array([[a, b], [c, d]])


def webster_abcd(radius_fn, x1: float, x2: float, air: Air, omega: float, rtol=1e-11):
    """Reference transfer matrix of a lossy horn by numerical integration.

    Integrates dp/dx = -jw rho_eff(r(x))/S(x) U and dU/dx = -jw S(x)/K_eff(r(x)) p
    from x2 back to x1 for the two unit end states, which gives the columns of
    T with [p1, U1] = T [p2, U2].
    """

    def rhs(x, y):
        r = radius_fn(x)
        s = math.pi * r * r
        rho, k = medium("circle", r, air, omega)
        p = y[0] + J * y[1]
        u = y[2] + J * y[3]
        dp = -J * omega * rho / s * u
        du = -J * omega * s / k * p
        return [dp.real, dp.imag, du.real, du.imag]

    cols = []
    for y0 in ([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]):
        sol = integrate.solve_ivp(
            rhs, (x2, x1), y0, method="DOP853", rtol=rtol, atol=1e-30
        )
        y = sol.y[:, -1]
        cols.append((y[0] + J * y[1], y[2] + J * y[3]))
    return np.array([[cols[0][0], cols[1][0]], [cols[0][1], cols[1][1]]])


def chain(mats):
    t = np.eye(2, dtype=complex)
    for m in mats:
        t = t @ m
    return t


def z_in(t, z_load):
    """Input impedance of a two-port loaded by z_load (np.inf for rigid)."""
    if np.isinf(z_load):
        return t[0, 0] / t[1, 0]
    return (t[0, 0] * z_load + t[0, 1]) / (t[1, 0] * z_load + t[1, 1])


def transfer_impedance(t, z_load):
    """p2 / U1 for a two-port loaded by z_load."""
    if np.isinf(z_load):
        return 1.0 / t[1, 0]
    return z_load / (t[1, 0] * z_load + t[1, 1])


# ----- Canal from an area function ---------------------------------------------------


def canal_segments(positions, areas, n_segments: int):
    """Breakpoints and radii: every knot is kept, each interval subdivided evenly.

    The radius is piecewise linear in x between knots (each piece a cone).
    Positions may run either way; distances are |dx|. Interval i is split
    into ceil(|span_i| / (L/N) - 1e-9) equal parts (at least one), where L is
    the total length and N the requested segment count.
    """
    positions = np.asarray(positions, float)
    radii = np.sqrt(np.asarray(areas, float) / math.pi)
    total = abs(positions[-1] - positions[0])
    dx = total / n_segments
    xs = [positions[0]]
    rs = [radii[0]]
    for i in range(len(positions) - 1):
        span = positions[i + 1] - positions[i]
        m = max(1, int(math.ceil(abs(span) / dx - 1e-9)))
        for k in range(1, m + 1):
            t = k / m
            xs.append(positions[i] + t * span)
            rs.append(radii[i] + t * (radii[i + 1] - radii[i]))
    return np.array(xs), np.array(rs)


def canal_abcd(positions, areas, n_segments, air, omega, lossless=False):
    """Chain of conical segments from positions[0] to positions[-1]."""
    xs, rs = canal_segments(positions, areas, n_segments)
    mats = []
    for i in range(len(xs) - 1):
        length = abs(xs[i + 1] - xs[i])
        if lossless:
            mats.append(cone_abcd_lossless(rs[i], rs[i + 1], length, air, omega))
        else:
            mats.append(cone_abcd(rs[i], rs[i + 1], length, air, omega))
    return chain(mats)


def cone_abcd_lossless(r1, r2, length, air, omega):
    """Lossless truncated cone, written in the textbook apex-distance form
    (e.g. Mapes-Riordan, JAES 41(6) 1993; Chaigne & Kergomard 2016 Ch. 7):
    x1, x2 are signed distances of the two ends from the apex."""
    k = omega / air.c
    rc = air.rho * air.c
    if r1 == r2:
        return tube_lossless(math.pi * r1 * r1, length, air, omega)
    x1 = r1 * length / (r2 - r1)
    x2 = x1 + length
    s1, s2 = math.pi * r1 * r1, math.pi * r2 * r2
    kl = k * length
    a = (x2 / x1) * math.cos(kl) - math.sin(kl) / (k * x1)
    b = 1j * rc / math.sqrt(s1 * s2) * math.sin(kl)
    c = 1j * math.sqrt(s1 * s2) / rc * (
        (1.0 + 1.0 / (k * k * x1 * x2)) * math.sin(kl) - length / (k * x1 * x2) * math.cos(kl)
    )
    d = (x1 / x2) * math.cos(kl) + math.sin(kl) / (k * x2)
    return np.array([[a, b], [c, d]], dtype=complex)


def tube_lossless(area, length, air, omega):
    k = omega / air.c
    zc = air.rho * air.c / area
    return np.array(
        [[math.cos(k * length), 1j * zc * math.sin(k * length)],
         [1j * math.sin(k * length) / zc, math.cos(k * length)]],
        dtype=complex,
    )


# ----- Hudde & Engel eardrum --------------------------------------------------------


@dataclass
class HuddeEngel:
    """Parameters of COMSOL Acoustics Module User's Guide 6.4, Table 2-8.

    Units: acoustic elements in N s m^-5, kg m^-4, m^5/N; mechanical in
    N s/m, kg, m/N; areas in m^2; volumes in m^3.
    """

    r_tcav: float = 2e6
    v_tcav: float = 0.5e-6
    r_ada: float = 1.7e6
    l_ada: float = 880.0
    v_ant: float = 0.8e-6
    q_mac: float = 0.4
    v_mac: float = 8e-6
    f_mac: float = 3500.0
    r_ac: float = 4e7
    c_ac: float = 5e-12
    l_ac0: float = 2.4e3
    f_lac: float = 1900.0
    f_yph: float = 8000.0
    s_yph: float = 1.4
    a0: float = 38e-6
    a_inf: float = 2e-6
    f_a: float = 2200.0
    q_a: float = 1.3
    s_aph: float = -1.2
    f_aph: float = 1500.0
    phi_a: float = -0.8038
    r_mi: float = 1.0
    c_mi: float = 0.04e-3
    c_oss: float = 3e-3
    l_oss: float = 7e-6
    c_cpl: float = 0.5e-3
    r_cpl: float = 0.08
    r_free: float = 0.02
    l_free: float = 12e-6
    r_st: float = 18e-3
    l_st: float = 3e-6
    c_st: float = 1.2e-3
    r_c_mech: float = 70e-3  # R_c * A_F^2
    l_c_mech: float = 10e-6  # L_c * A_F^2
    c_c_mech: float = 11e-3  # C_c / A_F^2
    log: str = "ln"  # base of the phase laws; see docs/ear-loads.md

    def scaled(self, r=1.0, m=1.0, c=1.0, cav=1.0) -> "HuddeEngel":
        """Scale every resistance, mass/inertance and compliance of the drum,
        ossicles and cochlea; `cav` scales the three cavity volumes."""
        return replace(
            self,
            r_ac=self.r_ac * r,
            r_mi=self.r_mi * r,
            r_cpl=self.r_cpl * r,
            r_free=self.r_free * r,
            r_st=self.r_st * r,
            r_c_mech=self.r_c_mech * r,
            l_ac0=self.l_ac0 * m,
            l_oss=self.l_oss * m,
            l_free=self.l_free * m,
            l_st=self.l_st * m,
            l_c_mech=self.l_c_mech * m,
            c_ac=self.c_ac * c,
            c_mi=self.c_mi * c,
            c_oss=self.c_oss * c,
            c_cpl=self.c_cpl * c,
            c_st=self.c_st * c,
            c_c_mech=self.c_c_mech * c,
            v_tcav=self.v_tcav * cav,
            v_ant=self.v_ant * cav,
            v_mac=self.v_mac * cav,
        )

    def _log(self, x):
        return np.log(x) if self.log == "ln" else np.log10(x)

    def cavity(self, omega, gamma_p0):
        jw = J * omega
        c_t = self.v_tcav / gamma_p0
        c_ant = self.v_ant / gamma_p0
        c_mac = self.v_mac / gamma_p0
        w_mac = 2 * math.pi * self.f_mac
        y_t = 1.0 / (self.r_tcav + 1.0 / (jw * c_t))
        z_ada = self.r_ada + jw * self.l_ada
        y_ant = jw * c_ant + jw * c_mac / (
            1.0 - (omega / w_mac) ** 2 + jw / (self.q_mac * w_mac)
        )
        return 1.0 / (y_t + 1.0 / (z_ada + 1.0 / y_ant))

    def drum(self, omega):
        """Drum + ossicles + cochlea (Z_eardrum - Z_cav)."""
        jw = J * omega
        w_a = 2 * math.pi * self.f_a
        w_aph = 2 * math.pi * self.f_aph
        a = (self.a0 + self.a_inf) / (1.0 - (omega / w_a) ** 2 + jw / (self.q_a * w_a)) - self.a_inf
        if omega < w_aph:
            a_d = a
        else:
            a_d = abs(a) * np.exp(J * (self.s_aph * self._log(omega / w_aph) + self.phi_a))
        l_ac = self.l_ac0 * (1.0 + math.sqrt(omega / (2 * math.pi * self.f_lac)))
        phi_y = self.s_yph * self._log(1.0 + omega / (2 * math.pi * self.f_yph))
        y_ac = np.exp(J * phi_y) / (self.r_ac + jw * l_ac + 1.0 / (jw * self.c_ac))
        y_mi = 1.0 / (self.r_mi + 1.0 / (jw * self.c_mi))
        y_cpl = 1.0 / (self.r_cpl + 1.0 / (jw * self.c_cpl))
        y_free = 1.0 / (self.r_free + jw * self.l_free)
        z_dmi = 1.0 / (jw * self.c_oss) + jw * self.l_oss + 1.0 / (y_cpl + y_free)
        z_dm = 2.0 / 3.0 * z_dmi
        z_inc = 1.0 / 3.0 * z_dmi
        z_st = self.r_st + jw * self.l_st + 1.0 / (jw * self.c_st)
        z_sc = z_st + self.r_c_mech + jw * self.l_c_mech + 1.0 / (jw * self.c_c_mech)
        k11 = (1.0 + y_mi * z_dm) / a_d
        k12 = z_dmi / a_d * (1.0 + y_mi * z_inc * z_dm / z_dmi)
        k21 = y_ac / a_d * (1.0 + y_mi * z_dm) + a_d * y_mi
        k22 = y_ac * z_dmi / a_d * (1.0 + y_mi * z_inc * z_dm / z_dmi) + a_d * (1.0 + y_mi * z_inc)
        return (k11 * z_sc + k12) / (k21 * z_sc + k22)

    def impedance(self, omega, gamma_p0):
        return self.cavity(omega, gamma_p0) + self.drum(omega)


# ----- Radial and end-correction helpers ------------------------------------------------


def rect_end_correction(h: float, b: float) -> float:
    """End correction of a baffled rectangular opening h x b (Munjal et al.,
    Formulas of Acoustics (2008) p. 319, as used by Luan et al. 2019 Eq. A.7)."""
    beta = h / b
    eps = 1.0 + beta * beta
    t1 = (beta + (1.0 - eps**1.5) / beta**2) / (3.0 * math.pi)
    t2 = (math.log(beta + math.sqrt(eps)) / beta + math.log((1.0 + math.sqrt(eps)) / beta)) / math.pi
    return h * (t1 + t2)


def radial_slit_abcd(r_in, r_out, gap, angle, air, omega, n=32):
    """Radial flow through a thin gap between radii r_in < r_out over a total
    opening angle (rad): stepped chain of slit segments (area = angle*r*gap)."""
    edges = np.linspace(r_in, r_out, n + 1)
    mats = []
    for a, b in zip(edges[:-1], edges[1:]):
        rm = 0.5 * (a + b)
        area = angle * rm * gap
        mats.append(tube_abcd("slit", 0.5 * gap, area, b - a, air, omega))
    return chain(mats)


def lumped_cavity_z(volume, wall_area, air, omega):
    """Compliance with the thermal wall-layer correction (cavity.rs, L0)."""
    eps = (air.gamma - 1.0) * air.thermal_layer(omega) * wall_area / (2.0 * volume)
    c = volume / air.bulk * (1.0 + eps * (1.0 - 1j))
    return 1.0 / (J * omega * c)


# ----- IEC 60318-4 literature model -----------------------------------------------------


@dataclass
class Iec711:
    """Geometry of the G.R.A.S. RA0045 from Luan et al. (2019) Table 1 (mean values)."""

    r0: float = 3.77e-3
    l1: float = 3.12e-3
    l3: float = 4.75e-3
    l5: float = 4.69e-3
    a2: float = 2.53e-3
    b2: float = 2.35e-3
    h2: float = 0.16e-3
    r2: float = 6.30e-3
    big_r2: float = 9.01e-3
    d1: float = 1.91e-3
    r4: float = 4.66e-3
    alpha4_deg: float = 95.33
    parts4: int = 3
    h4: float = 0.05e-3
    big_r4: float = 9.01e-3
    d2: float = 1.40e-3
    mic: str = "bk4192"  # or "rigid"

    def mic_z(self, omega):
        if self.mic == "rigid":
            return np.inf
        # B&K 4192 (COMSOL Generic 711 Coupler doc, after the B&K Microphone Handbook):
        c, r, l = 0.62e-13, 119e6, 710.0
        return r + J * omega * l + 1.0 / (J * omega * c)

    def hr2_z(self, air, omega):
        dl = rect_end_correction(self.h2, self.b2)
        area = self.b2 * self.h2
        neck = tube_abcd("slit", 0.5 * self.h2, area, self.a2 + 2 * dl, air, omega)
        vol = math.pi * (self.big_r2**2 - self.r2**2) * self.d1
        wall = 2 * math.pi * (self.big_r2**2 - self.r2**2) + 2 * math.pi * (self.big_r2 + self.r2) * self.d1
        zc = lumped_cavity_z(vol, wall, air, omega)
        return z_in(neck, zc)

    def hr4_z(self, air, omega):
        angle_part = math.radians(self.alpha4_deg)
        angle = self.parts4 * angle_part
        dl_in = rect_end_correction(self.h4, angle_part * self.r0)
        dl_out = rect_end_correction(self.h4, angle_part * self.r4)
        neck = radial_slit_abcd(self.r0 - dl_in, self.r4 + dl_out, self.h4, angle, air, omega)
        vol = math.pi * (self.big_r4**2 - self.r4**2) * self.d2
        wall = 2 * math.pi * (self.big_r4**2 - self.r4**2) + 2 * math.pi * (self.big_r4 + self.r4) * self.d2
        zc = lumped_cavity_z(vol, wall, air, omega)
        return z_in(neck, zc)

    def abcd(self, air, omega):
        s0 = math.pi * self.r0**2
        t1 = tube_abcd("circle", self.r0, s0, self.l1, air, omega)
        t3 = tube_abcd("circle", self.r0, s0, self.l3, air, omega)
        t5 = tube_abcd("circle", self.r0, s0, self.l5, air, omega)
        y2 = 1.0 / self.hr2_z(air, omega)
        y4 = 1.0 / self.hr4_z(air, omega)
        sh2 = np.array([[1, 0], [y2, 1]])
        sh4 = np.array([[1, 0], [y4, 1]])
        return chain([t1, sh2, t3, sh4, t5])

    def transfer(self, air, omega):
        return transfer_impedance(self.abcd(air, omega), self.mic_z(omega))

    def input(self, air, omega):
        return z_in(self.abcd(air, omega), self.mic_z(omega))


def effective_volume(z, air, omega):
    return air.bulk / (omega * abs(z))
