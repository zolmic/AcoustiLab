#!/usr/bin/env python3
"""Independent reference values for the driver tests (tests/driver.rs).

Nothing here shares code with the Rust engine. The driver is written as
Newton's second law per moving mass plus Kirchhoff's voltage law on the coil
and volume-velocity continuity at each acoustic node, and solved as a small
dense system in mpmath (40 digits). The engine instead stamps an
across/through MNA network of motor transformer, piston gyrators, one-ports
and internal nodes, so agreement checks the stamping, the port wiring and
the D2 split.

Model definitions (see crates/acoustilab/src/elements/driver.rs and ts.rs):
* primary set -> Cms = 1/(ws^2 Mms), Rms = ws Mms/Qms, Bl = sqrt(ws Mms Re/Qes);
* creep factor c(jw) = 1 - lambda log10(j w/w0), w0 = 2 pi fs by default;
* coil Z = Re + jwLe + (R2 || jwL2);
* D2 split: C_outer = Cms - 1/Kbend, r = C_outer/Cms, M1 = Mms - r^2 Msur,
  S1 = Sd - r Ssur, R_dome = Rms - (1 - r)^2 Rbend; both springs carry c(jw).

Air is the spec reference (rho 1.204 kg/m3, c 343 m/s), used only for the
lumped front and rear compliances V/(rho c^2).

Also computes the Tymphany HPD-40N16PET00-32 identity values quoted in
erratum E5 from the datasheet numbers.

Writes crates/acoustilab/tests/data/driver_reference.json.
"""
import json
import os

import mpmath as mp

mp.mp.dps = 40

RHO, C0 = mp.mpf("1.204"), mp.mpf(343)
K0 = RHO * C0 * C0

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "..", "crates", "acoustilab", "tests", "data",
                   "driver_reference.json")

J = mp.mpc(0, 1)


def primary_to_physical(p):
    ws = 2 * mp.pi * p["fs_Hz"]
    mms = p["Mms_kg"]
    return dict(
        re=mp.mpf(p["Re_ohm"]),
        bl=mp.sqrt(ws * mms * p["Re_ohm"] / p["Qes"]),
        mms=mp.mpf(mms),
        cms=1 / (ws * ws * mms),
        rms=ws * mms / p["Qms"],
        sd=mp.mpf(p["Sd_m2"]),
        fs=mp.mpf(p["fs_Hz"]),
    )


def creep(lam, f0, w):
    if lam == 0:
        return mp.mpf(1)
    return 1 - lam * mp.log10(J * w / (2 * mp.pi * f0))


def coil(re, le, l2, r2, w):
    z = re + J * w * le
    if l2 > 0 and r2 > 0:
        zl = J * w * l2
        z += zl * r2 / (zl + r2)
    return z


def solve_case(case, f):
    """Unknowns: coil current i, dome velocity v1, surround velocity v2,
    front pressure pf, rear pressure pr."""
    w = 2 * mp.pi * mp.mpf(f)
    jw = J * w
    ph = primary_to_physical(case["primary"])
    c = creep(case.get("creep_lambda", 0), case.get("creep_f0_Hz", ph["fs"]), w)
    zc = coil(ph["re"], case.get("Le_H", 0), case.get("L2_H", 0), case.get("R2_ohm", 0), w)
    zs = mp.mpf(case.get("Zs_ohm", 0))
    V = mp.mpf(case.get("V_V", 1))
    bl = ph["bl"]
    d2 = case.get("d2")
    if d2:
        cb = 1 / mp.mpf(d2["Kbend_N_per_m"])
        co = ph["cms"] - cb
        r = co / ph["cms"]
        m1 = ph["mms"] - r * r * d2["Msur_kg"]
        m2 = mp.mpf(d2["Msur_kg"])
        s1 = ph["sd"] - r * d2["Ssur_m2"]
        s2 = mp.mpf(d2["Ssur_m2"])
        rd = ph["rms"] - (1 - r) ** 2 * d2["Rbend_Ns_per_m"]
        yb = d2["Rbend_Ns_per_m"] + 1 / (jw * cb * c)
        ys = jw * m2 + 1 / (jw * co * c)
        z1 = jw * m1 + rd
    else:
        s1, s2 = ph["sd"], mp.mpf(0)
        z1 = jw * ph["mms"] + ph["rms"] + 1 / (jw * ph["cms"] * c)
        yb = None
    # Front and rear acoustic admittances to ambient (None: node is ambient).
    front = case.get("front")
    rear = case.get("rear")

    def yac(spec):
        y = 0
        if "volume_m3" in spec:
            y += jw * spec["volume_m3"] / K0
        if "R_Pa_s_per_m3" in spec:
            y += 1 / mp.mpf(spec["R_Pa_s_per_m3"])
        return y

    n = 5
    A = mp.matrix(n, n)
    b = mp.matrix(n, 1)
    I, V1, V2, PF, PR = range(5)
    # KVL: V = (Zs + Zc) i + Bl v1
    A[0, I] = zs + zc
    A[0, V1] = bl
    b[0] = V
    # Dome: Bl i = z1 v1 + yb (v1 - v2) + S1 (pf - pr)
    A[1, I] = -bl
    A[1, V1] = z1
    A[1, PF] = s1
    A[1, PR] = -s1
    if d2:
        A[1, V1] += yb
        A[1, V2] = -yb
        # Surround: 0 = ys v2 + yb (v2 - v1) + S2 (pf - pr)
        A[2, V2] = ys + yb
        A[2, V1] = -yb
        A[2, PF] = s2
        A[2, PR] = -s2
    else:
        A[2, V2] = 1  # v2 = v1 (rigid)
        A[2, V1] = -1
    # Front: S1 v1 + S2 v2 = Yf pf  (or pf = 0 at ambient)
    if front:
        A[3, PF] = yac(front)
        A[3, V1] = -s1
        A[3, V2] = -s2
    else:
        A[3, PF] = 1
    # Rear: -(S1 v1 + S2 v2) = Yr pr
    if rear:
        A[4, PR] = yac(rear)
        A[4, V1] = s1
        A[4, V2] = s2
    else:
        A[4, PR] = 1
    x = mp.lu_solve(A, b)
    i, v1, v2, pf, pr = (x[k] for k in range(5))
    vt = V - zs * i  # terminal voltage
    return dict(
        zin=vt / i,
        p_front=pf,
        p_rear=pr,
        v_dome=v1,
        v_surround=v2,
        u=s1 * v1 + s2 * v2,
        s_eff=(s1 * v1 + s2 * v2) / v1,
    )


def cplx(z):
    z = mp.mpc(z)
    return [float(z.real), float(z.imag)]


def log_freqs(f0, f1, n):
    return [float(f0 * (f1 / f0) ** (k / (n - 1))) for k in range(n)]


TYMPHANY = {"fs_Hz": 81.8, "Qms": 2.71, "Qes": 1.01, "Re_ohm": 32.8,
            "Mms_kg": 0.3e-3, "Sd_m2": 10e-4}

CASES = {
    # D1 with creep and LR-2 in a sealed front cup with a vented rear,
    # through a 10 ohm source.
    "d1_sealed": {
        "primary": TYMPHANY,
        "model": "D1",
        "Le_H": 20e-6, "L2_H": 60e-6, "R2_ohm": 15.0,
        "creep_lambda": 0.05,
        "Zs_ohm": 10.0, "V_V": 1.0,
        "front": {"volume_m3": 30e-6},
        "rear": {"volume_m3": 20e-6, "R_Pa_s_per_m3": 5e6},
    },
    # D2 (illustrative surround values, not a real driver) in the same cup.
    "d2_sealed": {
        "primary": TYMPHANY,
        "model": "D2",
        "creep_lambda": 0.05,
        "d2": {"Msur_kg": 2e-5, "Ssur_m2": 3e-4, "Kbend_N_per_m": 2000.0,
               "Rbend_Ns_per_m": 0.005},
        "Zs_ohm": 0.0, "V_V": 1.0,
        "front": {"volume_m3": 30e-6, "R_Pa_s_per_m3": 2e7},
        "rear": {"volume_m3": 20e-6, "R_Pa_s_per_m3": 5e6},
    },
    # D2 unloaded (both faces at ambient): complex effective area.
    "d2_free": {
        "primary": TYMPHANY,
        "model": "D2",
        "d2": {"Msur_kg": 2e-5, "Ssur_m2": 3e-4, "Kbend_N_per_m": 2000.0,
               "Rbend_Ns_per_m": 0.005},
        "Zs_ohm": 0.0, "V_V": 1.0,
    },
}


def tymphany_identities():
    fs, qms, qes, re, mms, sd = (mp.mpf(x) for x in ("81.8", "2.71", "1.01", "32.8", "0.3e-3", "10e-4"))
    bl_ds, cms_um, vas_ds = mp.mpf("2.42"), mp.mpf("11.7e-6"), mp.mpf("1.64e-3")
    ws = 2 * mp.pi * fs
    qes_bl = ws * mms * re / bl_ds ** 2
    out = {
        "fs_from_printed_cms_Hz": 1 / (2 * mp.pi * mp.sqrt(mms * cms_um)),
        "fs_from_cms_mm_per_N_Hz": 1 / (2 * mp.pi * mp.sqrt(mms * cms_um * 1000)),
        "qes_from_bl": qes_bl,
        "qts_primary": qms * qes / (qms + qes),
        "qts_from_bl": qms * qes_bl / (qms + qes_bl),
        "bl_derived_Tm": mp.sqrt(ws * mms * re / qes),
        "cms_derived_m_per_N": 1 / (ws * ws * mms),
        "rms_derived_Ns_per_m": ws * mms / qms,
        "vas_from_printed_cms_m3": K0 * sd * sd * cms_um,
        "vas_from_cms_mm_per_N_m3": K0 * sd * sd * cms_um * 1000,
        "vas_printed_m3": vas_ds,
        "sensitivity_implied_ohm": mp.mpf("2.83") ** 2 * mp.power(10, (mp.mpf("80.4") - mp.mpf("74.18")) / 10),
    }
    return {k: float(v) for k, v in out.items()}


def main():
    freqs = sorted(set(log_freqs(10.0, 40000.0, 37) + [81.8, 1000.0, 1500.0, 2000.0, 3000.0]))
    data = {
        "description": "Independent driver references from tools/driver/reference.py (mpmath, 40 digits). "
                       "Complex values are [re, im] RMS phasors; SI units.",
        "air": {"rho_kg_per_m3": 1.204, "c_m_per_s": 343.0},
        "tymphany_identities": tymphany_identities(),
        "cases": {},
    }
    for name, case in CASES.items():
        rows = {k: [] for k in ("zin", "p_front", "p_rear", "v_dome", "v_surround", "s_eff")}
        for f in freqs:
            r = solve_case(case, f)
            for k in rows:
                rows[k].append(cplx(r[k]))
        data["cases"][name] = {"params": case, "frequencies_Hz": freqs, **rows}
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w") as fh:
        json.dump(data, fh, indent=1)
        fh.write("\n")
    print("wrote", os.path.relpath(OUT))


if __name__ == "__main__":
    main()
