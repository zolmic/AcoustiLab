#!/usr/bin/env python3
"""Reference impulse responses for crates/acoustilab/tests/time.rs.

Writes crates/acoustilab/tests/data/time_ir.json.

Networks (ideal electrical elements, driven by 1 V; exact transfer
functions):

* ``hp``: C then R to ground, V(out)/V = sRC/(1 + sRC), 100 Hz corner.
* ``lp``: series R-L, C to ground, V(C)/V = 1/(1 + sRC + s^2 LC),
  f0 = 1 kHz, Q = 5.
* ``lp_q20``: the same with f0 = 100 Hz, Q = 20 (erratum E46 study).

The engine's uniform grid is reproduced independently with numpy
(fs = 48 kHz): bins k = 1..N/2 are H(j 2 pi k fs/N), the solved band
reaching Nyquist (Nyquist bin: real part); DC from the low-frequency probe
at f_a = fs/(1024 N) and 2 f_a: zero when the log-log slope exceeds 1/2,
else Re(2 H(f_a) - H(2 f_a)). h = irfft(half spectrum) (numpy's pocketfft).

For ``lp`` the continuous analytic response h(t) = sum_i r_i exp(p_i t),
time-aliased with period T = N/fs, h_T(t) = sum_i r_i exp(p_i t)/(1 -
exp(p_i T)), is also given, sampled as h_T(n/fs)/fs, together with the
bound on its difference from the band-limited IR:
(1/N)(sum over bins outside [-N/2, N/2] of |H| + |Re H(Nyquist)|
+ |DC rule - H(0)|): the Nyquist bin carries Re H for both k = +-N/2,
whose exact sum is 2 Re H. The bins beyond Nyquist are summed to 32 fs and the
tail beyond that integrated for the w0^2/w^2 asymptote.

E46 study (``lp_q20``): the energy in the second half of the circular
buffer relative to the total, for several N, including buffers whose
causal half equals 1.1 Q/f (T60/2), 2.2 Q/f (T60) and 2.9 Q/f (E46).

Run from anywhere:  python3 tools/time/ir_refs.py
"""

import json
from math import pi
from pathlib import Path

import numpy as np

OUT = Path(__file__).resolve().parents[2] / "crates/acoustilab/tests/data/time_ir.json"
FS = 48000.0


def lp_values(f0, q, c=None):
    c = c if c is not None else (1e-6 if f0 == 1000.0 else 1e-5)
    w0 = 2 * pi * f0
    l = 1.0 / (w0 * w0 * c)
    r = np.sqrt(l / c) / q
    return {"R_ohm": r, "L_H": l, "C_F": c, "f0_Hz": f0, "Q": q}


HP = {"R_ohm": 1000.0, "C_F": 1.0 / (2 * pi * 100.0 * 1000.0), "fc_Hz": 100.0}
LP = lp_values(1000.0, 5.0, 1e-6)
LPQ = lp_values(100.0, 20.0, 1e-5)


def h_hp(s):
    t = HP["R_ohm"] * HP["C_F"]
    return s * t / (1 + s * t)


def h_lp(p):
    return lambda s: 1.0 / (1 + s * p["R_ohm"] * p["C_F"] + s * s * p["L_H"] * p["C_F"])


def half_spectrum(h, n):
    df = FS / n
    k = np.arange(n // 2 + 1)
    s = 2j * pi * k * df
    with np.errstate(divide="ignore", invalid="ignore"):
        v = h(s).astype(complex)
    fa = df / 1024.0
    ha, hb = h(2j * pi * fa), h(2j * pi * 2 * fa)
    slope = np.log(abs(hb) / abs(ha)) / np.log(2.0)
    v[0] = 0.0 if slope > 0.5 else (2 * ha - hb).real
    v[-1] = v[-1].real
    return v, slope


def late_energy_db(h):
    n = len(h)
    return float(10 * np.log10(np.sum(h[n // 2:] ** 2) / np.sum(h ** 2)))


def sample_indices(n):
    return list(range(0, 512)) + list(range(512, n, 8))


def lp_continuous(p, n):
    w0 = 2 * pi * p["f0_Hz"]
    sig = w0 / (2 * p["Q"])
    wd = w0 * np.sqrt(1 - 1 / (4 * p["Q"] ** 2))
    p1, p2 = -sig + 1j * wd, -sig - 1j * wd
    r1 = w0 * w0 / (p1 - p2)
    r2 = np.conj(r1)
    t = np.arange(n) / FS
    tt = n / FS
    hT = r1 * np.exp(p1 * t) / (1 - np.exp(p1 * tt)) + r2 * np.exp(p2 * t) / (1 - np.exp(p2 * tt))
    return (hT.real / FS), (p1, p2, r1)


def lp_bound(p, n, h, dc):
    df = FS / n
    half = n // 2
    kmax = half * 64
    k = np.arange(half + 1, kmax + 1, dtype=float)
    out = 2 * np.sum(np.abs(h(2j * pi * k * df)))
    # tail beyond kmax: |H| ~ w0^2/w^2, two sides
    w0 = 2 * pi * p["f0_Hz"]
    out += 2 * (w0 / (2 * pi * df)) ** 2 / kmax
    nyq = abs(h(2j * pi * half * df).real)
    return float((out + nyq + abs(dc - 1.0)) / n)


def case(name, h, n, params, continuous=False):
    v, slope = half_spectrum(h, n)
    ir = np.fft.irfft(v, n)
    idx = sample_indices(n)
    rec = {
        "name": name,
        "params": params,
        "fs_Hz": FS,
        "n": n,
        "dc_slope": float(slope),
        "dc": float(v[0].real),
        "indices": idx,
        "h": [float(ir[i]) for i in idx],
        "energy": float(np.sum(ir ** 2)),
        "late_energy_dB": late_energy_db(ir),
    }
    if continuous:
        hc, _ = lp_continuous(params, n)
        rec["h_continuous"] = [float(hc[i]) for i in idx]
        rec["bound"] = lp_bound(params, n, h, v[0].real)
        rec["max_deviation"] = float(np.max(np.abs(ir - hc)))
    return rec


def e46_study():
    h = h_lp(LPQ)
    q, f0 = LPQ["Q"], LPQ["f0_Hz"]
    rows = []
    for label, half_s in [("T60/2", 1.1 * q / f0), ("T60", 2.2 * q / f0), ("E46", 2.9 * q / f0)]:
        n = int(2 * round(half_s * FS))
        v, _ = half_spectrum(h, n)
        rows.append({"label": label, "half_length_s": n / (2 * FS), "n": n,
                     "late_energy_dB": late_energy_db(np.fft.irfft(v, n))})
    pow2 = []
    for n in [8192, 16384, 32768, 65536]:
        v, _ = half_spectrum(h, n)
        pow2.append({"n": n, "late_energy_dB": late_energy_db(np.fft.irfft(v, n))})
    return {"params": LPQ, "needed_s": 2.9 * q / f0, "study": rows, "pow2": pow2}


def main():
    data = {
        "generator": "tools/time/ir_refs.py",
        "numpy": np.__version__,
        "cases": [
            case("hp", h_hp, 8192, HP),
            case("lp", h_lp(LP), 8192, LP, continuous=True),
        ],
        "e46": e46_study(),
    }
    OUT.write_text(json.dumps(data, indent=1) + "\n")
    for c in data["cases"]:
        extra = f", bound {c.get('bound', 0):.3e}, max dev {c.get('max_deviation', 0):.3e}" if "bound" in c else ""
        print(f"{c['name']}: dc {c['dc']:.3e} slope {c['dc_slope']:.4f} late {c['late_energy_dB']:.1f} dB{extra}")
    for r in data["e46"]["study"] + data["e46"]["pow2"]:
        print(r)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
