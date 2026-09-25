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
erratum E5 from the datasheet numbers, and the values the driver records'
`plotted` checks compare with the 2016 sheet's chart. For those, the
impedance maximum is found by a numerical search of |Z(f)|, not from the
closed form Re (1 + Qms/Qes), and the half-space band level is an adaptive
quadrature over ln f of the on-axis level |p| = rho w |U|/(2 pi r) (the
engine uses a trapezoid rule).

Writes crates/acoustilab/tests/data/driver_reference.json.
"""
import json
import os

import mpmath as mp

mp.mp.dps = 40

RHO, C0 = mp.mpf("1.204"), mp.mpf(343)
K0 = RHO * C0 * C0
P_REF = mp.mpf("20e-6")

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


def tymphany_sensitivity_routes():
    """Half-space level at 2.83 V, 1 m in the mass-controlled range implied
    by the printed values, three ways (erratum E5):
    * from the reference efficiency eta0 = 4 pi^2 fs^3 Vas/(c^3 Qes) with the
      printed Vas, p^2 = rho c eta0 (V^2/Re)/(2 pi r^2);
    * from the mass line p = rho Sd Bl V/(2 pi r Mms Re) with the printed Bl;
    * from the same mass line with Bl derived from the primary set.
    Also the impedance implied by the 2016 sheet's pair of sensitivities."""
    fs, qes, re, mms, sd = (mp.mpf(x) for x in ("81.8", "1.01", "32.8", "0.3e-3", "10e-4"))
    v, r = mp.mpf("2.83"), mp.mpf(1)
    eta0 = 4 * mp.pi ** 2 * fs ** 3 * mp.mpf("1.64e-3") / (C0 ** 3 * qes)
    p_vas = mp.sqrt(RHO * C0 * eta0 * v * v / re / (2 * mp.pi * r * r))

    def mass_line(bl):
        return RHO * sd * bl * v / (2 * mp.pi * r * mms * re)

    bl_primary = mp.sqrt(2 * mp.pi * fs * mms * re / qes)
    out = {
        "vas_route_2p83V_dB": 20 * mp.log10(p_vas / P_REF),
        "printed_bl_route_2p83V_dB": 20 * mp.log10(mass_line(mp.mpf("2.42")) / P_REF),
        "primary_route_2p83V_dB": 20 * mp.log10(mass_line(bl_primary) / P_REF),
        "sensitivity_implied_2016_ohm": mp.mpf("2.83") ** 2 * mp.power(10, (mp.mpf("89.6") - mp.mpf("83.5")) / 10),
    }
    return {k: float(v) for k, v in out.items()}


# Primary sets of the embedded records (data/drivers/), and the values
# their `plotted` blocks quote from the chart of the 2016 revision of the
# sheet (digitised by tools/driver/digitize_hpd40_2016.py).
RECORDS = {
    "tymphany_hpd_40n16pet00_32": TYMPHANY,
    "tymphany_hpd_40n16pet00_32_curves_2016": {
        "fs_Hz": 111.3, "Qms": 1.15, "Qes": 1.04, "Re_ohm": 32.1,
        "Mms_kg": 0.075e-3, "Sd_m2": 10e-4},
}
PLOTTED_2016 = {"max_ohm": 66.1, "f_max_Hz": 109.0, "drive_V": 2.83,
                "distance_m": 1.0, "f1_Hz": 300.0, "f2_Hz": 1000.0,
                "level_dB": 82.55}


def plotted_checks(primary, plotted):
    """The unloaded D0 driver, solved from Kirchhoff's voltage law on the coil
    (Re i + Bl v = V) and Newton's law on the diaphragm (Bl i = Zm v)."""
    ph = primary_to_physical(primary)
    drive, r = mp.mpf(plotted["drive_V"]), mp.mpf(plotted["distance_m"])

    def solve(f):
        w = 2 * mp.pi * f
        zm = J * w * ph["mms"] + ph["rms"] + 1 / (J * w * ph["cms"])
        det = ph["re"] * zm + ph["bl"] ** 2
        return w, zm / det, ph["bl"] / det  # current and velocity per volt

    def zabs(f):
        _, i, _ = solve(f)
        return abs(1 / i)

    # Golden-section search for the |Z| maximum within an octave of fs.
    g = (mp.sqrt(5) - 1) / 2
    a, b = ph["fs"] / mp.sqrt(2), ph["fs"] * mp.sqrt(2)
    for _ in range(200):
        c, d = b - g * (b - a), a + g * (b - a)
        if zabs(c) > zabs(d):
            b = d
        else:
            a = c
    f_peak = (a + b) / 2

    def level(ln_f):
        w, _, v = solve(mp.e ** ln_f)
        p = RHO * w * ph["sd"] * abs(v) * drive / (2 * mp.pi * r)
        return 20 * mp.log10(p / P_REF)

    lo, hi = mp.log(plotted["f1_Hz"]), mp.log(plotted["f2_Hz"])
    band = mp.quad(level, [lo, hi]) / (hi - lo)
    mass_line = RHO * ph["sd"] * ph["bl"] * drive / (2 * mp.pi * r * ph["mms"] * ph["re"])
    out = {
        "f_peak_Hz": f_peak,
        "peak_ohm": zabs(f_peak),
        "resonance_deviation": ph["fs"] / plotted["f_max_Hz"] - 1,
        "peak_deviation": zabs(f_peak) / plotted["max_ohm"] - 1,
        "band_level_dB": band,
        "band_level_deviation_dB": band - plotted["level_dB"],
        "mass_line_dB": 20 * mp.log10(mass_line / P_REF),
        "level_1kHz_dB": level(mp.log(1000)),
    }
    return {k: float(v) for k, v in out.items()}


def main():
    freqs = sorted(set(log_freqs(10.0, 40000.0, 37) + [81.8, 1000.0, 1500.0, 2000.0, 3000.0]))
    data = {
        "description": "Independent driver references from tools/driver/reference.py (mpmath, 40 digits). "
                       "Complex values are [re, im] RMS phasors; SI units.",
        "air": {"rho_kg_per_m3": 1.204, "c_m_per_s": 343.0},
        "tymphany_identities": tymphany_identities(),
        "tymphany_sensitivity": tymphany_sensitivity_routes(),
        "plotted_2016": {"plotted": PLOTTED_2016,
                         **{name: plotted_checks(p, PLOTTED_2016) for name, p in RECORDS.items()}},
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
