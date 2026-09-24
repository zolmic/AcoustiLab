#!/usr/bin/env python3
"""Independent reference values for the materials, vent and leak tests.

Everything here is re-implemented from the published formulas with mpmath
(50-digit Bessel functions of complex argument) and scipy, sharing no code
with the Rust engine. Air is the spec's reference state (rho 1.204 kg/m3,
c 343 m/s, mu 1.81e-5 Pa s, gamma 1.4, Pr 0.71, P0 = rho c^2 / gamma).

Writes crates/acoustilab/tests/data/materials_reference.json.

Model definitions mirrored from the engine (see the doc comments in
crates/acoustilab/src/elements/materials.rs and ducts.rs):
* circular hole / tube: rho_eff = rho / (1 - 2 I1(z)/(z I0(z))), z = a sqrt(j w rho/mu);
  thermal K_eff = gamma P0 / (1 + (gamma-1) F(z sqrt(Pr))).
* hole end correction 8a/(3 pi) * Fok(sqrt(porosity)) per side; resistive end
  correction per_end * sqrt(mu rho w / 2) / (pi a^2) per side (Maa 0.5, Ingard 1).
* JCA/JCAL (Allard & Atalla 2009), Delany-Bazley 1970, Miki 1990 as written in
  PorousModel::equivalent_fluid.
* fill: exact isothermal-fibre cell model (Tarnow 1996) versus the single
  relaxation with tau = <theta>/nu'.
"""
import json
import os

import mpmath as mp
import numpy as np
from scipy.optimize import brentq, minimize_scalar
from scipy.special import jv, struve

mp.mp.dps = 50

RHO, C0, MU, GAMMA, PR = 1.204, 343.0, 1.81e-5, 1.4, 0.71
P0 = RHO * C0 * C0 / GAMMA
K0 = RHO * C0 * C0

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "..", "crates", "acoustilab", "tests", "data",
                   "materials_reference.json")


def cx(z):
    return [float(mp.re(z)), float(mp.im(z))]


# ---------------------------------------------------------------- ducts ---
def f_circle(z):
    z = mp.mpc(z)
    return 2 * mp.besseli(1, z) / (z * mp.besseli(0, z))


def f_slit(z):
    z = mp.mpc(z)
    return mp.tanh(z) / z


def medium(shape, s, w):
    kv = mp.sqrt(1j * w * RHO / MU)
    kt = kv * mp.sqrt(PR)
    F = f_circle if shape == "circle" else f_slit
    rho_eff = RHO / (1 - F(kv * s))
    k_eff = K0 / (1 + (GAMMA - 1) * F(kt * s))
    return rho_eff, k_eff


def lumped_tube(a, t, w):
    rho_eff, _ = medium("circle", a, w)
    return 1j * w * rho_eff * t / (mp.pi * a * a)


def line_abcd(shape, s, area, length, w):
    rho_eff, k_eff = medium(shape, s, w)
    sq = mp.sqrt(rho_eff / k_eff)
    if mp.im(sq) > 0:
        sq = -sq
    G = 1j * w * sq
    Zc = k_eff * sq / area
    gl = G * length
    return mp.matrix([[mp.cosh(gl), Zc * mp.sinh(gl)], [mp.sinh(gl) / Zc, mp.cosh(gl)]])


def series(z):
    return mp.matrix([[1, z], [0, 1]])


def fok(x):
    p = (1 - 1.40925 * x + 0.33818 * x**3 + 0.06793 * x**5 - 0.02287 * x**6
         + 0.03015 * x**7 - 0.01641 * x**8)
    return max(p, 0.0)


def rs(w):
    return mp.sqrt(MU * RHO * w / 2)


def hole(a, t, phi, per_end, interaction, w):
    s = mp.pi * a * a
    psi = fok(np.sqrt(phi)) if interaction else 1.0
    delta = 8 * a / (3 * mp.pi) * psi
    return lumped_tube(a, t, w) + 2 * per_end * rs(w) / s + 1j * w * RHO * 2 * delta / s


def maa(d, t, sigma, area, w):
    k = d * mp.sqrt(w * RHO / (4 * MU))
    r = 32 * MU * t / (sigma * RHO * C0 * d * d) * (mp.sqrt(1 + k * k / 32) + mp.sqrt(2) / 32 * k * d / t)
    wm = w * t / (sigma * C0) * (1 + 1 / mp.sqrt(9 + k * k / 2) + 0.85 * d / t)
    return (r + 1j * wm) * RHO * C0 / area


# -------------------------------------------------------------- porous ---
def jca(phi, sigma, ainf, lv, lt, k0t, w):
    j = 1j
    gv = mp.sqrt(1 + j * 4 * ainf**2 * MU * RHO * w / (sigma**2 * lv**2 * phi**2))
    rho = ainf * RHO * (1 + sigma * phi / (j * w * RHO * ainf) * gv)
    if k0t is None:
        k0t = phi * lt * lt / 8
    gt = mp.sqrt(1 + j * 4 * k0t**2 * RHO * PR * w / (MU * lt**2 * phi**2))
    K = GAMMA * P0 / (GAMMA - (GAMMA - 1) / (1 + MU * phi / (j * w * RHO * PR * k0t) * gt))
    return rho / phi, K / phi


def one_param(kind, sigma, w):
    f = w / (2 * mp.pi)
    X = 1e3 * f / sigma
    if kind == "db":
        zc = RHO * C0 * (1 + 9.08 * X**-0.75 - 1j * 11.9 * X**-0.73)
        k = w / C0 * (1 + 10.8 * X**-0.70 - 1j * 10.3 * X**-0.59)
    else:
        zc = RHO * C0 * (1 + 5.50 * X**-0.632 - 1j * 8.43 * X**-0.632)
        k = w / C0 * (1 + 7.81 * X**-0.618 - 1j * 11.41 * X**-0.618)
    return zc * k / w, zc * w / k


def surface(rho_eq, k_eq, t, area, w):
    sq = mp.sqrt(rho_eq / k_eq)
    if mp.im(sq) > 0:
        sq = -sq
    G = 1j * w * sq
    zc = k_eq * sq
    return zc / mp.tanh(G * t) / area


# ---------------------------------------------------------------- fill ---
def cell_isothermal_fraction(f, r, c):
    """Mean of (1 - normalised temperature) in the Tarnow cell: 1 at DC."""
    nu_t = MU / (RHO * PR)
    b = r / mp.sqrt(c)
    q = mp.sqrt(1j * 2 * mp.pi * f / nu_t)
    D = mp.besseli(0, q * r) * mp.besselk(1, q * b) + mp.besselk(0, q * r) * mp.besseli(1, q * b)
    num = (r / q) * (mp.besselk(1, q * r) * mp.besseli(1, q * b) - mp.besseli(1, q * r) * mp.besselk(1, q * b))
    return 2 / (b * b - r * r) * num / D


def tau_cell(r, c):
    nu_t = MU / (RHO * PR)
    b2 = r * r / c
    th = b2 * (-0.25 * np.log(c) - 3 / 8 + c / 2 - c * c / 8) / (1 - c)
    return th / nu_t


# ------------------------------------------------------ vent resonance ---
def piston_rad(a, w):
    k = w / C0
    x = 2 * k * a
    s = np.pi * a * a
    return RHO * C0 / s * ((1 - 2 * jv(1, x) / x) + 1j * 2 * struve(1, x) / x)


def vent_transfer(a, t, V, inner, per_end, f, mesh_rayl=None):
    """|p_cav / p_ext| for an external pressure acting through the vent's
    radiation impedance (engine netlist: pressure_source -> vent -> cavity)."""
    w = 2 * np.pi * f
    s = np.pi * a * a
    # cavity (volume only: wall area of a cube of the same volume)
    aw = 6 * V ** (2 / 3)
    dt = np.sqrt(2 * MU / (RHO * w)) / np.sqrt(PR)
    eps = (GAMMA - 1) * dt * aw / (2 * V)
    Cc = V / K0 * (1 + eps * (1 - 1j))
    Zcav = 1 / (1j * w * Cc)
    # tube: half of the inner end mass and resistance on each side of the line
    end_z = 1j * w * RHO * inner / s + (per_end * float(rs(w)) / s if inner > 0 else 0)
    T = series(end_z / 2) * line_abcd("circle", a, s, t, w) * series(end_z / 2)
    Zout = piston_rad(a, w) + per_end * float(rs(w)) / s
    if mesh_rayl is not None:
        Zout = Zout + mesh_rayl / s
    # start at the cavity: port 1 of T faces the cavity? The engine's tube goes
    # cavity (port 1) -> mouth (port 2); T maps [p_cav; U] at port 1 from port 2.
    # Solve backwards: at the mouth p_m, U_m; cavity side p_c = A p_m + B U_m,
    # U_c = C p_m + D U_m, with p_c = Zcav U_c (U_c flowing into the cavity is -U_c
    # in the tube's convention). Use the load approach instead:
    # impedance looking from the mouth into tube + cavity:
    A, B, Cm, D = T[0, 0], T[0, 1], T[1, 0], T[1, 1]
    # tube is reciprocal and symmetric; flow from the mouth towards the cavity:
    # p_m = A p_c + B U, U_m = C p_c + D U with U = p_c / Zcav (symmetric line).
    pc = 1
    U = pc / Zcav
    pm = A * pc + B * U
    Um = Cm * pc + D * U
    pext = pm + Um * Zout
    return complex(pc / pext)


def resonance(fun, lo, hi):
    fs = np.linspace(lo, hi, 2001)
    h = np.array([abs(fun(f)) for f in fs])
    i = int(np.argmax(h))
    r = minimize_scalar(lambda f: -abs(fun(f)), bracket=(fs[i - 1], fs[i], fs[i + 1]), tol=1e-12)
    f0 = r.x
    hm = abs(fun(f0))
    f_lo = brentq(lambda f: abs(fun(f)) - hm / np.sqrt(2), lo * 0.2, f0)
    f_hi = brentq(lambda f: abs(fun(f)) - hm / np.sqrt(2), f0, hi * 3)
    return f0, f0 / (f_hi - f_lo), hm


# ------------------------------------------------------- slit examples ---
def slit_H(gap, width, length, V, f):
    """C3 transfer H = Z_L / (Z_L + 1/(j w C)) with the lumped exact slit."""
    w = 2 * np.pi * f
    rho_eff, _ = medium("slit", gap / 2, w)
    zl = 1j * w * rho_eff * length / (width * gap)
    zc = 1 / (1j * w * V / K0)
    return complex(zl / (zl + zc))


def main():
    out = {"air": "spec_reference"}
    freqs = [10.0, 100.0, 1000.0, 5000.0, 20000.0]

    # Mesh: pores calibrated from R_s, thickness and open area.
    meshes = []
    for (r_s, area, t, phi, per_end) in [(160.0, 1e-4, 60e-6, 0.3, 0.5), (3.0, 1e-4, None, 0.44, 0.5)]:
        if t is None:  # pore diameter 285 um given instead of thickness
            a = 142.5e-6
            t = r_s * phi * a * a / (8 * MU)
        else:
            a = np.sqrt(8 * MU * t / (phi * r_s))
        n = phi * area / (np.pi * a * a)
        z = [cx(hole(a, t, phi, per_end, True, 2 * np.pi * f) / n) for f in freqs]
        meshes.append({"R_s": r_s, "area": area, "thickness": t, "open_area": phi,
                       "radius": a, "freqs": freqs, "z": z})
    out["mesh"] = meshes

    # Perforated plate: exact (interaction on and off) and Maa 1998.
    plates = []
    for (d, t, sig, area) in [(0.4e-3, 0.5e-3, 0.01, 1e-3), (1.0e-3, 1.0e-3, 0.05, 1e-3)]:
        a = d / 2
        n = sig * area / (np.pi * a * a)
        pf = [20.0, 200.0, 2000.0, 20000.0]
        plates.append({
            "d": d, "t": t, "porosity": sig, "area": area, "freqs": pf,
            "exact": [cx(hole(a, t, sig, 0.5, True, 2 * np.pi * f) / n) for f in pf],
            "exact_no_interaction": [cx(hole(a, t, sig, 0.5, False, 2 * np.pi * f) / n) for f in pf],
            "maa": [cx(maa(d, t, sig, area, 2 * np.pi * f)) for f in pf],
        })
    # Largest relative deviation of Maa's formula from the exact hole
    # (interaction off) over the perforate constant 0.1 .. 30.
    dev_r, dev_x = 0.0, 0.0
    for (d, t) in [(0.2e-3, 0.2e-3), (0.5e-3, 1e-3), (1e-3, 0.5e-3), (0.3e-3, 3e-3)]:
        a = d / 2
        for k in np.geomspace(0.1, 30, 60):
            w = (k / d) ** 2 * 4 * MU / RHO
            ze = hole(a, t, 0.01, 0.5, False, w) / (0.01 * 1.0 / (np.pi * a * a))
            zm = maa(d, t, 0.01, 1.0, w)
            dev_r = max(dev_r, abs(float(mp.re(zm) / mp.re(ze)) - 1))
            dev_x = max(dev_x, abs(float(mp.im(zm) / mp.im(ze)) - 1))
    out["perforate"] = {"cases": plates, "maa_max_rel_dev_r": dev_r, "maa_max_rel_dev_x": dev_x}

    # Porous: equivalent fluid and rigid-backed surface impedance.
    pf = [50.0, 200.0, 1000.0, 5000.0, 20000.0]
    foam = dict(phi=0.85, sigma=34000.0, ainf=1.18, lv=60e-6, lt=87e-6)
    porous = {"freqs": pf, "thickness": 0.025, "area": 1e-3}
    rows = {}
    for name, fn in [
        ("jca_foam", lambda w: jca(foam["phi"], foam["sigma"], foam["ainf"], foam["lv"], foam["lt"], None, w)),
        ("jcal_foam", lambda w: jca(foam["phi"], foam["sigma"], foam["ainf"], foam["lv"], foam["lt"], 2.0e-10, w)),
        ("db_20k", lambda w: one_param("db", 20000.0, w)),
        ("miki_20k", lambda w: one_param("miki", 20000.0, w)),
    ]:
        vals = [fn(2 * np.pi * f) for f in pf]
        rows[name] = {
            "rho_eq": [cx(v[0]) for v in vals],
            "k_eq": [cx(v[1]) for v in vals],
            "zs": [cx(surface(v[0], v[1], 0.025, 1e-3, 2 * np.pi * f)) for v, f in zip(vals, pf)],
        }
    porous["models"] = rows
    out["porous"] = porous

    # Fill: exact cell model against the single relaxation.
    fills = []
    for (d, bulk, solid) in [(6e-6, 14.0, 2500.0), (20e-6, 10.0, 1380.0)]:
        r, c = d / 2, bulk / solid
        tau = tau_cell(r, c)
        fr = 1 / (2 * np.pi * tau)
        ratios = [0.01, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0]
        fills.append({"fibre_diameter": d, "bulk_density": bulk, "fibre_density": solid,
                      "tau": tau, "f_relax": fr, "ratios": ratios,
                      "exact": [cx(cell_isothermal_fraction(x * fr, r, c)) for x in ratios]})
    out["fill"] = fills

    # Vents: resonance and Q of |p_cav/p_ext| (pressure drive through the
    # radiation impedance). inner: end correction of the inner end (m).
    vents = []
    for (label, a, t, V, inner_kind, per_end, mesh) in [
        ("3mm_2mm_100cc_piston", 1.5e-3, 2e-3, 100e-6, "piston", 0.0, None),
        ("2mm_1.5mm_60cc_flanged", 1e-3, 1.5e-3, 60e-6, "flanged", 0.0, None),
        ("2mm_1.5mm_60cc_flanged_maa", 1e-3, 1.5e-3, 60e-6, "flanged", 0.5, None),
        ("2mm_1.5mm_60cc_flanged_ingard", 1e-3, 1.5e-3, 60e-6, "flanged", 1.0, None),
    ]:
        inner = {"piston": 8 * a / (3 * np.pi), "flanged": 0.8216 * a}[inner_kind]
        f0, q, hm = resonance(lambda f: vent_transfer(a, t, V, inner, per_end, f, mesh), 120, 400)
        vents.append({"label": label, "radius": a, "length": t, "volume": V,
                      "inlet": inner_kind, "per_end": per_end, "f0": f0, "Q": q, "peak": hm})
    out["vent"] = vents

    # Slit worked examples (App. C3 transfer, lumped exact slit, lossless C).
    slits = []
    for gap in [1e-3, 0.2e-3]:
        fun = lambda f: slit_H(gap, 30e-3, 10e-3, 30e-6, f)
        fs = np.geomspace(5, 5000, 4000)
        h = np.array([abs(fun(f)) for f in fs])
        i = int(np.argmax(h))
        entry = {"gap": gap, "max_gain_db": 20 * np.log10(h.max()), "f_max": fs[i]}
        if gap == 1e-3:
            f0, q, hm = resonance(fun, 200, 1000)
            entry.update({"f0": f0, "Q": q, "peak": hm})
            r_p = 12 * MU * 10e-3 / (30e-3 * gap**3)
            m_p = 6 * RHO * 10e-3 / (5 * 30e-3 * gap)
            cc = 30e-6 / K0
            entry["Q_poiseuille"] = np.sqrt(m_p / cc) / r_p
        else:
            f3 = brentq(lambda f: abs(fun(f)) - 1 / np.sqrt(2), 5, 500)
            entry["f_minus3db"] = f3
            r_p = 12 * MU * 10e-3 / (30e-3 * gap**3)
            entry["f_rc"] = 1 / (2 * np.pi * r_p * 30e-6 / K0)
        slits.append(entry)
    out["slit_examples"] = slits

    with open(OUT, "w") as fh:
        json.dump(out, fh, indent=1)
    print(json.dumps({k: out[k] for k in ["vent", "slit_examples"]}, indent=1))
    print("maa deviations", dev_r, dev_x)
    print("wrote", os.path.normpath(OUT))


if __name__ == "__main__":
    main()
