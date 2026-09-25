#!/usr/bin/env python3
"""Reference data for the modal-cavity tests (crates/acoustilab/tests/cavity.rs).

Writes crates/acoustilab/tests/data/cavity_reference.json. Everything here is
computed independently of the Rust code in crates/acoustilab/src/modes.rs and
elements/cavity.rs:

* eigenfrequencies from closed forms and scipy / mpmath Bessel-derivative roots;
* Bessel values and roots with mpmath at 30 digits;
* footprint means of mode shapes by direct 2-D Gauss-Legendre quadrature of the
  mode shape (not the mean-value theorem, sinc products or angular-spectrum sums
  the Rust code uses), and the disk mean of J_m^2 by quadrature;
* wall integrals (integral of phi^2 and of |grad_t phi|^2 over all walls) by
  quadrature face by face, with analytic derivatives of the mode shape;
* lossless port impedance matrices from the *mixed* (waveguide) representation:
  the sum along the wall normal in closed form (cot / csc, or the radial Bessel
  ratio for the cylinder side), the transverse sum to a cutoff 10-20x the
  element's, plus the continuum tail beyond it; pairs of ports on walls with
  different normals from a plain 3-D modal sum with 4x the element's cutoff;
* modal Q factors (Morse-Ingard boundary-layer perturbation) from the
  quadrature wall integrals.

Air is the engine's "spec_reference" preset (rho 1.204, c 343, mu 1.81e-5,
gamma 1.4, Pr 0.71, P0 = rho c^2 / gamma).

Usage: python3 tools/cavity/generate.py   (about two minutes)
"""
import json
import math
import os
import sys

import mpmath as mp
import numpy as np
from numpy.polynomial.legendre import leggauss
from scipy import optimize, special

RHO, C, MU, GAMMA, PR = 1.204, 343.0, 1.81e-5, 1.4, 0.71
OUT = os.path.join(os.path.dirname(__file__), "..", "..", "crates", "acoustilab", "tests",
                   "data", "cavity_reference.json")


def eps(n):
    return 1.0 if n == 0 else 2.0


def jinc(x):
    x = np.asarray(x, float)
    out = np.ones_like(x)
    nz = np.abs(x) > 1e-12
    out[nz] = 2 * special.j1(x[nz]) / x[nz]
    return out


def sinc(x):
    return np.sinc(np.asarray(x, float) / np.pi)


def gl(a, b, n, panels=1):
    x, w = leggauss(n)
    edges = np.linspace(a, b, panels + 1)
    xs, ws = [], []
    for lo, hi in zip(edges[:-1], edges[1:]):
        xs.append(0.5 * (hi - lo) * x + 0.5 * (hi + lo))
        ws.append(0.5 * (hi - lo) * w)
    return np.concatenate(xs), np.concatenate(ws)


# ----- Bessel ---------------------------------------------------------------

def bessel_fixture():
    mp.mp.dps = 30
    vals = []
    for m in [0, 1, 2, 5, 10, 30, 60, 100]:
        for x in [0.1, 1.0, 5.0, 10.0, 24.9, 25.1, 50.0, 100.0, 300.0]:
            vals.append([m, x, float(mp.besselj(m, x))])
    roots = []
    for m in [0, 1, 2, 3, 5, 10, 20, 40]:
        rs = [float(mp.besseljzero(m, s, derivative=1)) for s in range(1, 6)]
        if m == 0:
            rs = rs[1:] if rs[0] == 0.0 else rs
        roots.append([m, rs])
    return {"j": vals, "jp_roots": roots}


def jp_roots_all(x_max, step=0.05):
    """Positive roots of J'_m below x_max for all m (m = 0 without the root 0),
    by scanning scipy.special.jvp and refining with brentq."""
    xs = np.arange(step, x_max + step, step)
    out = []
    m = 0
    while True:
        # Every positive root of J'_m exceeds m (DLMF 10.21(i)); below that,
        # J'_m underflows and its sign is noise.
        d = np.where(xs > max(m - 1.0, 0.0), special.jvp(m, xs), 1.0 if m > 0 else -1.0)
        s = np.sign(d)
        idx = np.nonzero(s[:-1] * s[1:] < 0)[0]
        roots = []
        for i in idx:
            r = optimize.brentq(lambda t: special.jvp(m, t), xs[i], xs[i + 1], xtol=1e-15, rtol=1e-15)
            if r < x_max:
                roots.append(r)
        if not roots and m > x_max:
            break
        out.append(roots)
        m += 1
    return out


# ----- Mode lists -------------------------------------------------------------

def box_modes(lx, ly, lz, kmax):
    kk = np.pi / np.array([lx, ly, lz])
    n = [int(kmax / k) + 1 for k in kk]
    nx, ny, nz = np.meshgrid(np.arange(n[0] + 1), np.arange(n[1] + 1), np.arange(n[2] + 1), indexing="ij")
    k = np.sqrt((nx * kk[0]) ** 2 + (ny * kk[1]) ** 2 + (nz * kk[2]) ** 2)
    m = k < kmax
    order = np.lexsort((nz[m], ny[m], nx[m], k[m]))
    return np.stack([nx[m], ny[m], nz[m]], 1)[order], k[m][order]


def cyl_modes(a, d, kmax, roots=None):
    """List of (m, q, l, sine, k, kr)."""
    if roots is None:
        roots = jp_roots_all(kmax * a)
    out = []
    for m, rs in enumerate(roots):
        rl = ([0.0] if m == 0 else []) + list(rs)
        for q, j in enumerate(rl):
            kr = j / a
            if kr >= kmax:
                break
            l = 0
            while True:
                k = math.hypot(kr, l * math.pi / d)
                if k >= kmax:
                    break
                out.append((m, q, l, False, k, kr))
                if m > 0:
                    out.append((m, q, l, True, k, kr))
                l += 1
    out.sort(key=lambda t: (t[4], t[0], t[1], t[2], t[3]))
    return out


def mode_lists():
    lx, ly, lz = 0.06, 0.045, 0.02
    kmax = 2 * np.pi * 10000.0 / C
    idx, k = box_modes(lx, ly, lz, kmax)
    box = [[int(a), int(b), int(c_), float(kk * C / (2 * np.pi))] for (a, b, c_), kk in zip(idx, k)]
    counts = {}
    for fmax in [20000.0, 40000.0]:
        K = 3 * 2 * np.pi * fmax / C
        _, kk = box_modes(lx, ly, lz, K * 1.001)
        counts[str(int(fmax))] = int(np.sum(kk < K))
        gap = float(np.min(np.abs(kk - K)) / K)
        counts["gap_" + str(int(fmax))] = gap
    a, d = 0.025, 0.025
    cm = cyl_modes(a, d, 2 * np.pi * 10000.0 / C)
    cyl = [[m, q, l, s, k * C / (2 * np.pi)] for (m, q, l, s, k, kr) in cm]
    ccounts = {}
    for fmax in [20000.0]:
        K = 3 * 2 * np.pi * fmax / C
        allm = cyl_modes(a, d, K * 1.001)
        ks = np.array([t[4] for t in allm])
        ccounts[str(int(fmax))] = int(np.sum(ks < K))
        ccounts["gap_" + str(int(fmax))] = float(np.min(np.abs(ks - K)) / K)
    return {"box": {"dims": [lx, ly, lz], "modes_below_10k": box, "counts_3x": counts},
            "cylinder": {"radius": a, "depth": d, "modes_below_10k": cyl, "counts_3x": ccounts}}


# ----- Mode shapes, footprint means, wall integrals -------------------------

def disk_mean_j2(m, j):
    if j == 0.0:
        return 1.0
    val, _ = __import__("scipy").integrate.quad(lambda r: special.jv(m, j * r) ** 2 * r, 0, 1,
                                                 limit=400, epsabs=1e-15, epsrel=1e-13)
    return 2 * val


class BoxShape:
    def __init__(self, lx, ly, lz):
        self.L = np.array([lx, ly, lz])

    def phi(self, n, x, y, z):
        amp = math.sqrt(eps(n[0]) * eps(n[1]) * eps(n[2]))
        c = [x + self.L[0] / 2, y + self.L[1] / 2, z]
        out = amp
        for i in range(3):
            out = out * np.cos(n[i] * np.pi * c[i] / self.L[i])
        return out

    def grad(self, n, x, y, z):
        amp = math.sqrt(eps(n[0]) * eps(n[1]) * eps(n[2]))
        c = [x + self.L[0] / 2, y + self.L[1] / 2, z]
        k = [n[i] * np.pi / self.L[i] for i in range(3)]
        cs = [np.cos(k[i] * c[i]) for i in range(3)]
        sn = [-k[i] * np.sin(k[i] * c[i]) for i in range(3)]
        return [amp * sn[0] * cs[1] * cs[2], amp * cs[0] * sn[1] * cs[2], amp * cs[0] * cs[1] * sn[2]]

    def face_point(self, face, u, v):
        axis = {"x": 0, "y": 1, "z": 2}[face[0]]
        far = face[1] == "1"
        ua, va = {0: (1, 2), 1: (0, 2), 2: (0, 1)}[axis]
        p = [None, None, None]
        p[ua] = u
        p[va] = v
        if axis == 2:
            p[2] = self.L[2] if far else 0.0
        else:
            p[axis] = self.L[axis] / 2 if far else -self.L[axis] / 2
        return p

    def wall_integrals(self, n):
        I = J = 0.0
        lx, ly, lz = self.L
        faces = {"x": ((-ly / 2, ly / 2), (0, lz)), "y": ((-lx / 2, lx / 2), (0, lz)),
                 "z": ((-lx / 2, lx / 2), (-ly / 2, ly / 2))}
        for f, ((u0, u1), (v0, v1)) in faces.items():
            axis = {"x": 0, "y": 1, "z": 2}[f]
            ua, va = {0: (1, 2), 1: (0, 2), 2: (0, 1)}[axis]
            uu, wu = gl(u0, u1, 40, 4)
            vv, wv = gl(v0, v1, 40, 4)
            U, Vv = np.meshgrid(uu, vv, indexing="ij")
            W = np.outer(wu, wv)
            for side in "01":
                p = self.face_point(f + side, U, Vv)
                ph = self.phi(n, *p)
                g = self.grad(n, *p)
                I += np.sum(W * ph ** 2)
                J += np.sum(W * (g[ua] ** 2 + g[va] ** 2))
        return I, J


class CylShape:
    def __init__(self, a, d):
        self.a, self.d = a, d

    def amp(self, m, q_root, l):
        return math.sqrt(eps(m) * eps(l) / disk_mean_j2(m, q_root))

    def phi(self, md, r, th, z):
        m, j, l, sine = md
        kr = j / self.a
        trig = np.sin(m * th) if sine else np.cos(m * th)
        return self.amp(m, j, l) * special.jv(m, kr * r) * trig * np.cos(l * np.pi * z / self.d)

    def wall_integrals(self, md):
        m, j, l, sine = md
        a, d = self.a, self.d
        A = self.amp(m, j, l)
        kr = j / a
        kz = l * np.pi / d
        rr, wr = gl(0, a, 60, 2)
        tt, wt = gl(0, 2 * np.pi, 60, 4)
        R, T = np.meshgrid(rr, tt, indexing="ij")
        W = np.outer(wr, wt) * R
        trig = np.sin(m * T) if sine else np.cos(m * T)
        dtrig = m * (np.cos(m * T) if sine else -np.sin(m * T))
        I = J = 0.0
        for zc in (0.0, d):
            ax = np.cos(kz * zc)
            ph = A * special.jv(m, kr * R) * trig * ax
            dr = A * kr * special.jvp(m, kr * R) * trig * ax
            dt = A * special.jv(m, kr * R) * dtrig * ax / R
            I += np.sum(W * ph ** 2)
            J += np.sum(W * (dr ** 2 + dt ** 2))
        zz, wz = gl(0, d, 60, 2)
        T2, Z2 = np.meshgrid(tt, zz, indexing="ij")
        W2 = np.outer(wt, wz) * a
        trig = np.sin(m * T2) if sine else np.cos(m * T2)
        dtrig = m * (np.cos(m * T2) if sine else -np.sin(m * T2))
        jm = special.jv(m, j)
        ph = A * jm * trig * np.cos(kz * Z2)
        dth = A * jm * dtrig * np.cos(kz * Z2) / a
        dz = -A * jm * trig * kz * np.sin(kz * Z2)
        I += np.sum(W2 * ph ** 2)
        J += np.sum(W2 * (dth ** 2 + dz ** 2))
        return I, J


def footprint_nodes(fp, n_r=48, n_t=96):
    """Quadrature nodes (du, dv, weight) over a footprint, weights summing to its area."""
    if fp[0] == "disk":
        b = fp[1]
        rr, wr = gl(0, b, n_r, 2)
        tt, wt = gl(0, 2 * np.pi, n_t, 4)
        R, T = np.meshgrid(rr, tt, indexing="ij")
        return R * np.cos(T), R * np.sin(T), np.outer(wr, wt) * R
    du, dv = fp[1], fp[2]
    uu, wu = gl(-du / 2, du / 2, n_r, 2)
    vv, wv = gl(-dv / 2, dv / 2, n_r, 2)
    U, V = np.meshgrid(uu, vv, indexing="ij")
    return U, V, np.outer(wu, wv)


def box_patch_mean(shape, n, patch):
    face, u, v, fp = patch
    du, dv, w = footprint_nodes(fp)
    p = shape.face_point(face, u + du, v + dv)
    return float(np.sum(w * shape.phi(n, *p)) / np.sum(w))


def cyl_patch_mean(shape, md, patch):
    face, u, v, fp = patch
    du, dv, w = footprint_nodes(fp)
    if face in ("z0", "z1"):
        x, y = u + du, v + dv
        z = 0.0 if face == "z0" else shape.d
        val = shape.phi(md, np.hypot(x, y), np.arctan2(y, x), z)
    else:  # side: u is the angle in degrees, footprint in (arc length, z)
        th = np.deg2rad(u) + du / shape.a
        val = shape.phi(md, shape.a, th, v + dv)
    return float(np.sum(w * val) / np.sum(w))


BOX = BoxShape(0.06, 0.045, 0.02)
BOX_PATCHES = [
    ("z0", -0.020, -0.014, ("disk", 0.003)),
    ("z0", 0.005, 0.003, ("disk", 0.015)),
    ("z1", 0.018, 0.010, ("disk", 0.004)),
    ("z1", -0.010, 0.018, ("rect", 0.010, 0.0005)),
    ("x1", 0.005, 0.010, ("disk", 0.003)),
    ("y0", 0.010, 0.012, ("rect", 0.008, 0.004)),
]
BOX_MODES = [(0, 0, 0), (1, 0, 0), (0, 1, 1), (3, 2, 1), (7, 4, 0), (0, 0, 5), (12, 9, 3), (5, 11, 2)]

CYL = CylShape(0.025, 0.020)
CYL_PATCHES = [
    ("z0", 0.003, -0.002, ("disk", 0.012)),
    ("z1", -0.010, 0.008, ("disk", 0.003)),
    ("z1", 0.012, -0.012, ("rect", 0.006, 0.001)),
    ("side", 40.0, 0.010, ("disk", 0.0025)),
    ("side", 200.0, 0.003, ("rect", 0.008, 0.001)),
]
CYL_MODES = [(0, 0, 0, False), (1, 0, 0, False), (1, 0, 0, True), (2, 1, 1, False), (0, 2, 0, False),
             (5, 1, 2, True), (7, 0, 1, False), (3, 3, 0, True)]


def patch_fixture():
    box = []
    for n in BOX_MODES:
        box.append({"n": list(n), "means": [box_patch_mean(BOX, n, p) for p in BOX_PATCHES],
                    "wall": list(BOX.wall_integrals(n))})
    roots = jp_roots_all(40.0)
    cyl = []
    for (m, q, l, s) in CYL_MODES:
        j = 0.0 if (m == 0 and q == 0) else ([0.0] + roots[0] if m == 0 else roots[m])[q]
        md = (m, j, l, s)
        cyl.append({"index": [m, q, l, s], "root": j, "disk_mean_j2": disk_mean_j2(m, j),
                    "means": [cyl_patch_mean(CYL, md, p) for p in CYL_PATCHES],
                    "wall": list(CYL.wall_integrals(md))})
    return {"box": {"dims": list(BOX.L), "patches": [list(p[:3]) + [list(p[3])] for p in BOX_PATCHES],
                    "modes": box},
            "cylinder": {"radius": CYL.a, "depth": CYL.d,
                         "patches": [list(p[:3]) + [list(p[3])] for p in CYL_PATCHES], "modes": cyl}}


# ----- Quality factors ---------------------------------------------------------

def layers(omega):
    dv = math.sqrt(2 * MU / (RHO * omega))
    return dv, dv / math.sqrt(PR)


def q_fixture():
    out = {"box": [], "cylinder": []}
    V = float(np.prod(BOX.L))
    idx, k = box_modes(*BOX.L, 2 * np.pi * 9000 / C)
    for n, kk in list(zip(idx, k))[1:9]:
        I, J = BOX.wall_integrals(tuple(int(t) for t in n))
        dv, dt = layers(kk * C)
        et = (GAMMA - 1) * kk * kk * dt * I / (2 * V)
        ev = dv * J / (2 * V)
        out["box"].append({"n": [int(t) for t in n], "f": kk * C / (2 * np.pi), "Q": kk * kk / (et + ev),
                           "Q_thermal": kk * kk / et})
    a, d = 0.025, 0.025
    cs = CylShape(a, d)
    V = math.pi * a * a * d
    for (m, q, l, s, kk, kr) in cyl_modes(a, d, 2 * np.pi * 9000 / C)[1:9]:
        I, J = cs.wall_integrals((m, kr * a, l, s))
        dv, dt = layers(kk * C)
        et = (GAMMA - 1) * kk * kk * dt * I / (2 * V)
        ev = dv * J / (2 * V)
        out["cylinder"].append({"index": [m, q, l, s], "f": kk * C / (2 * np.pi), "Q": kk * kk / (et + ev),
                                "Q_thermal": kk * kk / et})
    return out


# ----- Impedance references (lossless) --------------------------------------

def rect_self_potential(w, h):
    dd = math.hypot(w, h)
    return (2 * w * w * h * math.asinh(h / w) + 2 * w * h * h * math.asinh(w / h)
            + 2 / 3 * (w ** 3 + h ** 3) - 2 / 3 * dd ** 3)


def ang_f2(fp, kap):
    kap = np.atleast_1d(kap)
    if fp[0] == "disk":
        return 2 * np.pi * jinc(kap * fp[1]) ** 2
    du, dv = fp[1], fp[2]
    out = np.empty_like(kap)
    for i, kk in enumerate(kap):
        panels = int(min(kk * max(du, dv) / np.pi + 2, 400))
        th, wt = gl(0, np.pi / 2, 16, panels)
        out[i] = 4 * np.sum(wt * (sinc(0.5 * kk * du * np.cos(th)) * sinc(0.5 * kk * dv * np.sin(th))) ** 2)
    return out


def tails(fp, kt):
    """(int_{|k|>kt} F^2/k d2k, int_{|k|>kt} F^2/(2k^3) d2k) in the half-space limit."""
    size = 2 * fp[1] if fp[0] == "disk" else max(fp[1], fp[2])
    full = 32 / (3 * fp[1]) if fp[0] == "disk" else \
        2 * np.pi / (fp[1] * fp[2]) ** 2 * rect_self_potential(fp[1], fp[2])
    kk, wk = gl(0, kt, 16, int(kt * size / np.pi) + 4)
    t1 = full - np.sum(wk * ang_f2(fp, kk))
    tt, wt = gl(0, 1, 16, 64)
    t2 = np.sum(wt * ang_f2(fp, kt / tt)) / (2 * kt)
    return t1, t2


def axial_G(kap2, L):
    """Sum over the axial order of eps_l c_l c'_l / (kap2 + (l pi/L)^2), same side and opposite
    sides, for complex-safe kap2 = kappa^2 - k^2 (including the uniform mode)."""
    kap2 = np.asarray(kap2, float)
    gs = np.empty(kap2.shape); go = np.empty(kap2.shape)
    pos = kap2 > 0
    kp = np.sqrt(kap2[pos]); x = kp * L
    with np.errstate(over="ignore"):
        gs[pos] = L / kp / np.tanh(x)
        go[pos] = np.where(x > 700, 0.0, L / kp / np.sinh(np.minimum(x, 700)))
    q = np.sqrt(-kap2[~pos]); x = q * L
    gs[~pos] = -L / q / np.tan(x)
    go[~pos] = -L / q / np.sin(x)
    return gs, go


def box_family_S(dims, axis, ports, k, kref):
    """Mixed representation along the normal `axis`: S_ij such that Z = (j w rho / V) S."""
    ua, va = {0: (1, 2), 1: (0, 2), 2: (0, 1)}[axis]
    Lu, Lv, L = dims[ua], dims[va], dims[axis]
    off = lambda ax: 0.0 if ax == 2 else dims[ax] / 2
    P = int(kref * Lu / np.pi) + 1
    Q = int(kref * Lv / np.pi) + 1
    p, q = np.meshgrid(np.arange(P + 1), np.arange(Q + 1), indexing="ij")
    kp = p * np.pi / Lu; kq = q * np.pi / Lv
    m = np.hypot(kp, kq) < kref
    kp, kq, p, q = kp[m], kq[m], p[m], q[m]
    kap = np.hypot(kp, kq)
    T = []
    for (face, u, v, fp) in ports:
        U, V = u + off(ua), v + off(va)
        base = np.sqrt(np.where(p > 0, 2.0, 1.0) * np.where(q > 0, 2.0, 1.0)) * np.cos(kp * U) * np.cos(kq * V)
        if fp[0] == "disk":
            T.append(base * jinc(kap * fp[1]))
        else:
            T.append(base * sinc(kp * fp[1] / 2) * sinc(kq * fp[2] / 2))
    gs, go = axial_G(kap ** 2 - k * k, L)
    n = len(ports)
    S = np.zeros((n, n))
    for i in range(n):
        for j in range(n):
            G = gs if ports[i][0][1] == ports[j][0][1] else go
            S[i, j] = np.sum(T[i] * T[j] * G)
    Vol = dims[0] * dims[1] * dims[2]
    for i in range(n):
        t1, t2 = tails(ports[i][3], kref)
        S[i, i] += Vol / (4 * np.pi ** 2) * (t1 + k * k * t2)
    return S


def rect_nodes(du, dv, nu, nv):
    uu, wu = gl(-du / 2, du / 2, nu, 1)
    vv, wv = gl(-dv / 2, dv / 2, nv, 1)
    U, V = np.meshgrid(uu, vv, indexing="ij")
    return U, V, np.outer(wu, wv)


def cyl_end_T(a, roots, ports, kref):
    """Transverse disk modes (m, q, sine) below kref and their normalised means over end
    ports: returns kr and T[port]. Disks by the closed form checked in patch_fixture,
    rectangles by brute-force quadrature."""
    krs, Ts = [], [[] for _ in ports]
    meta = []
    for m, rs in enumerate(roots):
        rl = ([0.0] if m == 0 else []) + list(rs)
        for j in rl:
            kr = j / a
            if kr >= kref:
                break
            jmw = 1.0 if j == 0 else special.jv(m, j)
            nt = math.sqrt(eps(m) / (1.0 if j == 0 else (1 - m * m / (j * j)) * jmw ** 2))
            vals = []
            for (face, u, v, fp) in ports:
                if fp[0] == "disk":
                    r0, th0 = math.hypot(u, v), math.atan2(v, u)
                    g = special.jv(m, kr * r0) * jinc(np.array([kr * fp[1]]))[0]
                    vals.append((g * math.cos(m * th0), g * math.sin(m * th0)))
                else:
                    du, dv, w = rect_nodes(fp[1], fp[2], 48, 16)
                    x, y = u + du, v + dv
                    th = np.arctan2(y, x)
                    jj = special.jv(m, kr * np.hypot(x, y))
                    sw = np.sum(w)
                    vals.append((np.sum(w * jj * np.cos(m * th)) / sw, np.sum(w * jj * np.sin(m * th)) / sw))
            for sine in ([False, True] if m > 0 else [False]):
                krs.append(kr)
                meta.append((m, sine, nt, jmw))
                for pi_, val in enumerate(vals):
                    Ts[pi_].append(nt * val[1 if sine else 0])
    return np.array(krs), [np.array(t) for t in Ts], meta


def cyl_end_S(a, d, ports, k, kref, kr, T):
    gs, go = axial_G(kr ** 2 - k * k, d)
    n = len(ports)
    S = np.zeros((n, n))
    for i in range(n):
        for j in range(n):
            G = gs if ports[i][0] == ports[j][0] else go
            S[i, j] = np.sum(T[i] * T[j] * G)
    Vol = math.pi * a * a * d
    for i in range(n):
        t1, t2 = tails(ports[i][3], kref)
        S[i, i] += Vol / (4 * np.pi ** 2) * (t1 + k * k * t2)
    return S


def i_ratios(mmax, x):
    """r_m = I_{m+1}(x)/I_m(x), m = 0..mmax, complex x, by backward recurrence
    r_{m-1} = 1/(2m/x + r_m) started far above max(mmax, |x|)."""
    M = int(max(mmax, abs(x))) + 80
    r = x / (2 * (M + 1))
    out = np.empty(mmax + 1, complex)
    for m in range(M, 0, -1):
        r = 1 / (2 * m / x + r)
        if m - 1 <= mmax:
            out[m - 1] = r
    return out


def cyl_side_S(a, d, ports, k, kref):
    """Mixed representation along the radius for side-wall ports."""
    n = len(ports)
    S = np.zeros((n, n))
    l = 0
    while True:
        kz = l * np.pi / d
        if kz >= kref:
            break
        mmax = int(a * math.sqrt(kref ** 2 - kz ** 2))
        ms = np.arange(mmax + 1)
        kap = np.hypot(ms / a, kz)
        keep = kap < kref
        ms, kap = ms[keep], kap[keep]
        s = np.sqrt(complex(kz * kz - k * k))
        x = s * a
        r = i_ratios(int(ms[-1]), x)[ms]
        G = np.real((a * a / 2) / (x * r + ms))
        e = np.sqrt(np.where(ms > 0, 2.0, 1.0) * eps(l))
        tc, ts = [], []
        for (face, ang, z0, fp) in ports:
            th0 = math.radians(ang)
            if fp[0] == "disk":
                c_ = math.cos(kz * z0) * jinc(kap * fp[1])
            else:
                c_ = sinc(ms / a * fp[1] / 2) * math.cos(kz * z0) * sinc(kz * fp[2] / 2)
            tc.append(e * c_ * np.cos(ms * th0))
            ts.append(e * c_ * np.sin(ms * th0))
        for i in range(n):
            for j in range(n):
                S[i, j] += np.sum(G * (tc[i] * tc[j] + ts[i] * ts[j]))
        l += 1
    Vol = math.pi * a * a * d
    for i in range(n):
        t1, t2 = tails(ports[i][3], kref)
        S[i, i] += Vol / (4 * np.pi ** 2) * (t1 + k * k * t2)
    return S


def axial_profile(s, L, z, far):
    """Sum over l of eps_l c_l cos(l pi z/L)/(s^2 + (l pi/L)^2) for a port on the z = 0
    (far = False, c_l = 1) or z = L face (c_l = (-1)^l): (L/s) cosh(s(L - z'))/sinh(sL) with
    z' = z or L - z, in exponential form; s may be imaginary (propagating)."""
    zz = (L - z) if far else z
    return (L / s) * (np.exp(-s * zz) + np.exp(-s * (2 * L - zz))) / (1 - np.exp(-2 * s * L))


def box_cross_S(dims, zports, xport, k, kref):
    """Mixed representation along z between z-face ports and one port on the x1 face,
    whose footprint (in y, z) is integrated by quadrature. Converges exponentially:
    the axial profile decays like exp(-kappa z_min) away from each z face."""
    lx, ly, L = dims
    P = int(kref * lx / np.pi) + 1
    Q = int(kref * ly / np.pi) + 1
    p, q = np.meshgrid(np.arange(P + 1), np.arange(Q + 1), indexing="ij")
    kp = p * np.pi / lx; kq = q * np.pi / ly
    m = np.hypot(kp, kq) < kref
    kp, kq, p, q = kp[m], kq[m], p[m], q[m]
    kap = np.hypot(kp, kq)
    e = np.sqrt(np.where(p > 0, 2.0, 1.0) * np.where(q > 0, 2.0, 1.0))
    s = np.sqrt((kap ** 2 - k * k).astype(complex))
    face, yj, zj, fp = xport
    dy, dz, w = footprint_nodes(fp, 16, 32)
    Y = (yj + dy + ly / 2).ravel(); Z = (zj + dz).ravel(); w = w.ravel() / np.sum(w)
    sign_x = np.where(p % 2 == 1, -1.0, 1.0) if face == "x1" else 1.0
    out = []
    for (fz, u, v, fpi) in zports:
        far = fz == "z1"
        U, V = u + lx / 2, v + ly / 2
        if fpi[0] == "disk":
            Ti = e * np.cos(kp * U) * np.cos(kq * V) * jinc(kap * fpi[1])
        else:
            Ti = e * np.cos(kp * U) * sinc(kp * fpi[1] / 2) * np.cos(kq * V) * sinc(kq * fpi[2] / 2)
        acc = 0.0
        for c0 in range(0, len(kp), 2000):
            sl = slice(c0, c0 + 2000)
            prof = axial_profile(s[sl, None], L, Z[None, :], far)
            avg = np.real(np.sum(w[None, :] * np.cos(kq[sl, None] * Y[None, :]) * prof, axis=1))
            acc += np.sum(Ti[sl] * e[sl] * (sign_x[sl] if np.ndim(sign_x) else sign_x) * avg)
        out.append(acc)
    return np.array(out)


def disk_nodes_periodic(b, n_r=24, n_t=96):
    """Polar nodes over a disk: Gauss-Legendre in r, uniform (trapezoidal) in angle."""
    rr, wr = gl(0, b, n_r, 1)
    tt = 2 * np.pi * np.arange(n_t) / n_t
    R, T = np.meshgrid(rr, tt, indexing="ij")
    return R * np.cos(T), R * np.sin(T), np.outer(wr, np.full(n_t, 2 * np.pi / n_t)) * R


def cyl_cross_S(a, d, eports, sports, k, kr, T, meta):
    """Mixed representation along z between end ports (transverse means T over the disk
    modes, from cyl_end_T) and side ports integrated by quadrature over their unrolled
    footprints. The axial profile decays like exp(-k_r z_min) away from each end, so terms
    with k_r z_min > 36 (below 2e-16) are skipped."""
    m = np.array([t[0] for t in meta], float)
    sine = np.array([t[1] for t in meta])
    nt = np.array([t[2] for t in meta])
    jmw = np.array([t[3] for t in meta])
    s = np.sqrt((kr ** 2 - k * k).astype(complex))
    out = np.zeros((len(eports), len(sports)))
    for jj, (face, ang, z0, fp) in enumerate(sports):
        if fp[0] == "disk":
            dsv, dz, w = disk_nodes_periodic(fp[1])
            half_z = fp[1]
        else:
            dsv, dz, w = rect_nodes(fp[1], fp[2], 48, 12)
            half_z = fp[2] / 2
        th = (math.radians(ang) + dsv / a).ravel()
        Z = (z0 + dz).ravel()
        w = w.ravel() / np.sum(w)
        for far in (False, True):
            users = [ii for ii, e in enumerate(eports) if (e[0] == "z1") == far]
            if not users:
                continue
            zmin = (d - z0 - half_z) if far else (z0 - half_z)
            keep = np.nonzero(kr * zmin < 36.0)[0]
            avg = np.zeros(len(kr))
            for c0 in range(0, len(keep), 500):
                sl = keep[c0:c0 + 500]
                mm = m[sl, None]
                trig = np.where(sine[sl, None], np.sin(mm * th[None, :]), np.cos(mm * th[None, :]))
                prof = axial_profile(s[sl, None], d, Z[None, :], far)
                avg[sl] = np.real(np.sum(w[None, :] * trig * prof, axis=1))
            for ii in users:
                out[ii, jj] = np.sum(T[ii] * nt * jmw * avg)
    return out


FREQS = [310.0, 2130.0, 5070.0, 9110.0, 13170.0, 17930.0]
F_MAX = 20000.0


def z_fixture():
    K = 3 * 2 * np.pi * F_MAX / C
    out = {"f_max": F_MAX, "freqs": FREQS}
    # Box: z-family ports 0-3, x-family port 4, cross pairs (4, 0-3).
    dims = list(BOX.L)
    zp = BOX_PATCHES[:4]
    xp = [BOX_PATCHES[4]]
    Vol = float(np.prod(dims))
    box = []
    for f in FREQS:
        k = 2 * np.pi * f / C
        pre = RHO * 2 * np.pi * f / Vol  # Z = j * pre * S
        Sz = box_family_S(dims, 2, zp, k, 20 * K)
        Sx = box_family_S(dims, 0, xp, k, 20 * K)
        # Port 4 spans z in [7, 13] mm: exp(-kappa * 7 mm) < 1e-17 beyond 40/7 mm.
        Sc = box_cross_S(dims, zp, xp[0], k, 40 / 0.007)
        Sc2 = box_cross_S(dims, zp, xp[0], k, 30 / 0.007)
        n = 5
        S = np.zeros((n, n))
        S[:4, :4] = Sz
        S[4, 4] = Sx[0, 0]
        S[4, :4] = Sc
        S[:4, 4] = Sc
        box.append({"f": f, "im_Z": (pre * S).tolist(),
                    "cross_cutoff_change": float(np.max(np.abs(Sc2 - Sc) / np.abs(Sc)))})
        print(f"box f={f}", file=sys.stderr)
    out["box"] = {"ports": [list(p[:3]) + [list(p[3])] for p in BOX_PATCHES[:5]], "Z": box}
    # Cylinder: end family 0-2, side family 3-4, cross end-side from 3-D sums.
    a, d = CYL.a, CYL.d
    ep = CYL_PATCHES[:3]
    sp = CYL_PATCHES[3:]
    kr_end, T_end, meta = cyl_end_T(a, jp_roots_all(10 * K * a), ep, 10 * K)
    print(f"cylinder end transverse modes: {len(kr_end)}", file=sys.stderr)
    # Nearest side footprint edge to an end: port 4, z in [2.5, 3.5] mm;
    # exp(-k_r * 2.5 mm) at k_r = 10K = 1.1e4 rad/m is 1e-12.
    Vol = math.pi * a * a * d
    cyl = []
    for f in FREQS:
        k = 2 * np.pi * f / C
        pre = RHO * 2 * np.pi * f / Vol
        Se = cyl_end_S(a, d, ep, k, 10 * K, kr_end, T_end)
        Ss = cyl_side_S(a, d, sp, k, 20 * K)
        Sc = cyl_cross_S(a, d, ep, sp, k, kr_end, T_end, meta)
        n = 5
        S = np.zeros((n, n))
        S[:3, :3] = Se
        S[3:, 3:] = Ss
        S[:3, 3:] = Sc
        S[3:, :3] = Sc.T
        cyl.append({"f": f, "im_Z": (pre * S).tolist()})
        print(f"cyl f={f}", file=sys.stderr)
    out["cylinder"] = {"ports": [list(p[:3]) + [list(p[3])] for p in CYL_PATCHES], "Z": cyl}
    return out


def main():
    data = {
        "generator": "tools/cavity/generate.py",
        "air": {"rho": RHO, "c": C, "mu": MU, "gamma": GAMMA, "Pr": PR},
        "bessel": bessel_fixture(),
        "modes": mode_lists(),
        "patches": patch_fixture(),
        "q": q_fixture(),
        "z": z_fixture(),
    }
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w") as fh:
        json.dump(data, fh, indent=1)
    print("wrote", os.path.normpath(OUT), file=sys.stderr)


if __name__ == "__main__":
    main()
