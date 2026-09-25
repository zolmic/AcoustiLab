#!/usr/bin/env python3
"""Independent reference values for the targets tests.

Re-implements, from the definitions in docs/targets.md and with no code
shared with the Rust engine, everything crates/acoustilab/tests/targets.rs
checks:

* the 1/12-octave evaluation grid 10^(k/40), k = 52..172, and band
  membership within half a 1/12-octave step of a band on a log axis;
* linear interpolation in dB on log frequency (numpy.interp on log10 f);
* 1/N-octave power smoothing: window f*G^(+-1/2N), G = 10^0.3, shrunk
  symmetrically at the ends, mean of the piecewise-linear power over log10 f,
  integrated with scipy.integrate.quad (breakpoints passed as `points`), not
  with the engine's exact trapezoid sum; the same for complex smoothing;
* the shelving filters, evaluated with scipy.signal.freqs from their
  numerator and denominator polynomials;
* the metric set, the BS.708 mask, tracking, the preference band and the
  preference models of data/targets/preference_models.json (numpy
  statistics, scipy.stats.linregress);
* the Harman-style reconstruction.

Empirical numbers (model coefficients and bands, mask breakpoints, shelf
settings, classes, widening) are read from data/targets/*.json, as the
engine reads them.

Writes crates/acoustilab/tests/data/targets_reference.json.
Usage: python3 tools/targets/reference.py
"""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
from scipy import integrate, signal, stats

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "data" / "targets"
OUT = ROOT / "crates" / "acoustilab" / "tests" / "data" / "targets_reference.json"

MODELS = json.loads((DATA / "preference_models.json").read_text())["models"]
BS708 = json.loads((DATA / "bs708.json").read_text())
PERS = json.loads((DATA / "personalisation.json").read_text())
RAVIZZA = json.loads((DATA / "ravizza2023_5128.json").read_text())


# ------------------------------------------------------------------ curves
# A curve covers a grid point within 1.3 % of its ends (the largest gap
# between a nominal R40 frequency and the exact 10^(k/40) it names).
END_SLACK = 0.013


def grid12():
    return np.array([10.0 ** (k / 40.0) for k in range(52, 173)])


def interp(f, db, x):
    """Linear in dB on log10 f, the end value up to END_SLACK beyond an end,
    NaN further out."""
    f, db, x = np.asarray(f, float), np.asarray(db, float), np.asarray(x, float)
    y = np.interp(np.log10(np.clip(x, f[0], f[-1])), np.log10(f), db)
    out = (x < f[0] * (1 - END_SLACK)) | (x > f[-1] * (1 + END_SLACK))
    y[out] = np.nan
    return y


def band_idx(g, lo, hi):
    """Grid indices within half a 1/12-octave step of [lo, hi]."""
    h = 2 ** (1 / 24)
    return np.nonzero((g >= lo / h) & (g <= hi * h))[0]


def covers(first, last, lo, hi):
    half = 2 ** (1 / 24) * (1 + 1e-9)
    return first <= lo * half and last >= hi / half


# --------------------------------------------------------------- smoothing
def smooth_values(f, y, n):
    x = np.log10(np.asarray(f, float))
    y = np.asarray(y, float)
    h = 3.0 / (20.0 * n)
    out = np.empty_like(y)
    # Absolute tolerance relative to the data's scale: a relative one cannot
    # be met where a real or imaginary part integrates to nearly zero.
    eps = 1e-13 * float(np.max(np.abs(y)))
    for i, c in enumerate(x):
        hi = min(h, c - x[0], x[-1] - c)
        if hi <= 0:
            out[i] = y[i]
            continue
        a, b = c - hi, c + hi
        pts = x[(x > a) & (x < b)]
        fun = lambda u: np.interp(u, x, y)  # noqa: E731
        val, _ = integrate.quad(fun, a, b, points=pts if len(pts) else None,
                                limit=max(200, 4 * len(pts) + 50), epsabs=eps * (b - a), epsrel=1e-12)
        out[i] = val / (b - a)
    return out


def smooth_power(f, db, n):
    p = 10.0 ** (np.asarray(db) / 10.0)
    return 10.0 * np.log10(smooth_values(f, p, n))


def smooth_complex(f, h, n):
    return smooth_values(f, np.real(h), n) + 1j * smooth_values(f, np.imag(h), n)


# ------------------------------------------------------------------ shelves
def shelf_db(f, fc, gain, q, high):
    if gain == 0:
        return np.zeros_like(np.asarray(f, float))
    a = 10.0 ** (gain / 40.0)
    ra = math.sqrt(a)
    if high:
        b, den = [a * a, a * ra / q, a], [1.0, ra / q, a]
    else:
        b, den = [a, a * ra / q, a * a], [a, ra / q, 1.0]
    _, hresp = signal.freqs(b, den, worN=np.asarray(f, float) / fc)
    return 20.0 * np.log10(np.abs(hresp))


def shelves_db(f, s):
    return (shelf_db(f, s["bass_fc"], s["bass"], s["bass_q"], False)
            + shelf_db(f, s["treble_fc"], s["treble"], s["treble_q"], True))


SH = PERS["shelves"]


def shelves(bass=0.0, treble=0.0):
    return {"bass": bass, "treble": treble, "bass_fc": SH["bass"]["fc_Hz"],
            "treble_fc": SH["treble"]["fc_Hz"], "bass_q": SH["bass"]["Q"],
            "treble_q": SH["treble"]["Q"]}


# --------------------------------------------------------------- statistics
def offset_at(g, v, f0):
    """Level at f0 read between grid values (linear on log f)."""
    k = int(np.searchsorted(g, f0))
    if k < len(g) and abs(g[k] / f0 - 1) < 1e-12:
        return v[k]
    if k == 0 or k == len(g):
        return np.nan
    t = math.log(f0 / g[k - 1]) / math.log(g[k] / g[k - 1])
    return v[k - 1] + t * (v[k] - v[k - 1])


def offset(g, v, norm):
    if norm[0] == "at":
        return offset_at(g, v, norm[1])
    idx = band_idx(g, norm[1], norm[2])
    w = v[idx]
    w = w[~np.isnan(w)]
    return float(np.mean(w)) if len(w) else np.nan


def band_stats(g, e, lo, hi):
    idx = band_idx(g, lo, hi)
    idx = idx[~np.isnan(e[idx])]
    if len(idx) == 0:
        return None
    x, y = g[idx], e[idx]
    out = {"n": int(len(y)), "first": float(x[0]), "last": float(x[-1]),
           "partial": not covers(x[0], x[-1], lo, hi),
           "mean": float(np.mean(y)), "mae": float(np.mean(np.abs(y))),
           "rms": float(np.sqrt(np.mean(y * y))),
           "max_abs": float(np.max(np.abs(y))), "max_abs_hz": float(x[int(np.argmax(np.abs(y)))])}
    if len(y) >= 2:
        out["sd"] = float(np.std(y, ddof=1))
        out["slope"] = float(stats.linregress(np.log(x), y).slope)
    return out


# -------------------------------------------------------------------- masks
def bs708_limit(f):
    bp = BS708["mask"]["breakpoints"]
    fb = np.array([b["f_Hz"] for b in bp])
    tb = np.array([b["tolerance_dB"] for b in bp])
    f = np.asarray(f, float)
    lim = np.interp(np.log(np.clip(f, fb[0], fb[-1])), np.log(fb), tb)
    lim[(f < fb[0] * (1 - 1e-9)) | (f > fb[-1] * (1 + 1e-9))] = np.nan
    return lim


def mask_result(g, e, lo, hi, lower, upper):
    idx = band_idx(g, lo, hi)
    ok = ~np.isnan(e[idx]) & ~np.isnan(lower[idx]) & ~np.isnan(upper[idx])
    idx = idx[ok]
    if len(idx) == 0:
        return {"n": 0, "within": 0}
    ex = np.maximum(lower[idx] - e[idx], e[idx] - upper[idx])
    within = int(np.sum(ex <= 1e-12))
    out = {"n": int(len(idx)), "within": within,
           "partial": not covers(g[idx[0]], g[idx[-1]], lo, hi)}
    if within < len(idx):
        j = int(np.argmax(ex))
        out["worst_hz"], out["worst_excess"] = float(g[idx[j]]), float(ex[j])
    return out


def band_edges(g, norm):
    """Preference band relative to the reference target, normalised."""
    gains = sorted({x for c in PERS["classes"]["items"] for x in c["bass_dB"]})
    variants = []
    for gb in gains:
        v = shelf_db(g, SH["bass"]["fc_Hz"], gb, SH["bass"]["Q"], False)
        variants.append(v - offset(g, v, norm))
    lower, upper = np.min(variants, axis=0), np.max(variants, axis=0)
    w = np.zeros_like(g)
    for key in ("above_2kHz", "above_8kHz"):
        r = PERS["widening"][key]
        t = np.clip(np.log(g / r["start_Hz"]) / math.log(r["full_Hz"] / r["start_Hz"]), 0, 1)
        w += r["half_width_dB"] * t
    return lower - w, upper + w


def tracking(g, fl, ll, fr, lr, n):
    if n:
        ll, lr = smooth_power(fl, ll, n), smooth_power(fr, lr, n)
    d = interp(fl, ll, g) - interp(fr, lr, g)
    out = []
    for lim in BS708["tracking"]["limits"]:
        lo, hi = lim["band_Hz"]
        t = lim["max_difference_dB"]
        m = mask_result(g, d, lo, hi, np.full_like(g, -t), np.full_like(g, t))
        idx = band_idx(g, lo, hi)
        idx = idx[~np.isnan(d[idx])]
        j = int(np.argmax(np.abs(d[idx])))
        m["max_abs"], m["max_abs_hz"] = float(abs(d[idx][j])), float(g[idx][j])
        out.append(m)
    a = band_idx(g, *BS708["tracking"]["limits"][0]["band_Hz"])[-1] + 1
    b = band_idx(g, *BS708["tracking"]["limits"][1]["band_Hz"])[0]
    gap = np.abs(d[a:b])
    return {"difference": nan_list(d), "bands": out,
            "gap_max": float(np.max(gap)), "gap_hz": float(g[a:b][int(np.argmax(gap))])}


# ------------------------------------------------------------------- scores
def score(model, g, e500):
    val = model["intercept"]
    variables = []
    for t in model["terms"]:
        s = band_stats(g, e500, *t["band_Hz"])
        v = {"SD": s["sd"], "AS": abs(s["slope"]), "ME": s["mae"]}[t["variable"]]
        variables.append(v)
        val -= t["weight"] * v
    return {"model": model["id"], "score": val, "variables": variables}


# ------------------------------------------------------------------- report
def evaluate(case):
    g = np.asarray(case.get("grid", grid12()), float)
    norm = case["norm"]
    n = case.get("smoothing")
    fl, ll = case["left"]
    if n:
        ll = smooth_power(fl, ll, n)
    avg = interp(fl, ll, g)
    if "right" in case:
        fr, lr = case["right"]
        if n:
            lr = smooth_power(fr, lr, n)
        avg = 0.5 * (avg + interp(fr, lr, g))
    tf, tdb, (vlo, vhi) = case["target"]
    t_raw = interp(tf, tdb, g)
    t_raw[(g < vlo * (1 - END_SLACK)) | (g > vhi * (1 + END_SLACK))] = np.nan
    sh = case.get("shelves")
    t_pers = t_raw + (shelves_db(g, sh) if sh else 0.0)
    r_off, t_off = offset(g, avg, norm), offset(g, t_pers, norm)
    resp, targ = avg - r_off, t_pers - t_off
    e = resp - targ
    out = {
        "response_offset": float(r_off), "target_offset": float(t_off),
        "error": nan_list(e),
        "main": band_stats(g, e, 20, 10000),
        "above_10k": band_stats(g, e, 10000, 20000),
        "bands": [band_stats(g, e, a, b) for a, b in [(20, 200), (200, 2000), (2000, 8000), (8000, 20000)]],
    }
    lim = bs708_limit(g)
    out["bs708"] = mask_result(g, e, 100, 16000, -lim, lim)
    t_ref = t_raw - offset(g, t_raw, norm)
    lo, up = band_edges(g, norm)
    out["band_lower"], out["band_upper"] = nan_list(t_ref + lo), nan_list(t_ref + up)
    out["band"] = mask_result(g, resp - t_ref, 20, 20000, lo, up)
    e500 = (avg - offset_at(g, avg, 500.0)) - (t_raw - offset_at(g, t_raw, 500.0))
    out["scores"] = [score(m, g, e500) for m in MODELS]
    if "right" in case:
        out["tracking"] = tracking(g, fl, case["left"][1], case["right"][0], case["right"][1],
                                   case.get("tracking_smoothing", 3))
    return out


def nan_list(v):
    return [None if (x is None or np.isnan(x)) else float(x) for x in np.asarray(v, float)]


# ------------------------------------------------------------ test curves
def native(ppo=24, f0=10.0, f1=20000.0):
    k = np.arange(0, int(math.floor(ppo * math.log2(f1 / f0))) + 1)
    return f0 * 2.0 ** (k / ppo)


def lgauss(f, fc, width_oct, gain):
    return gain * np.exp(-0.5 * (np.log2(f / fc) / width_oct) ** 2)


def response_a(f):
    """A headphone-like response: bass bump, ear gain, treble notch, tilt."""
    return (95.0 + lgauss(f, 60, 0.8, 4.0) + lgauss(f, 2800, 0.45, 9.0)
            - lgauss(f, 7500, 0.15, 7.0) + lgauss(f, 12000, 0.3, 3.0)
            - 1.2 * np.log2(f / 1000) * (f > 1000))


def response_b(f):
    """The other ear: response_a with small smooth and one 3 dB high deviation."""
    return response_a(f) + 0.35 * np.sin(1.3 * np.log2(f)) + lgauss(f, 12500, 0.12, 3.0) + 0.2


def target_a(f):
    """A Harman-like target: 6 dB bass shelf, ear gain, gentle treble fall."""
    return (shelf_db(f, 105, 6.0, 0.7071067811865476, False) + lgauss(f, 3000, 0.6, 10.0)
            - 2.0 * np.clip(np.log2(f / 5000), 0, None))


def main():
    fa = native(24)
    fb = native(48, 20.0, 20000.0)
    third = np.array([10.0 ** (k / 10.0) for k in range(15, 44)])  # 31.6 Hz .. 20 kHz
    g = grid12()
    ref = {"note": "generated by tools/targets/reference.py; see its docstring"}

    ref["grid"] = [float(x) for x in g]
    ref["curves"] = {
        "a": {"f": fa.tolist(), "dB": response_a(fa).tolist()},
        "b": {"f": fb.tolist(), "dB": response_b(fb).tolist()},
        "target_third": {"f": third.tolist(), "dB": target_a(third).tolist()},
        "target_dense": {"f": fa.tolist(), "dB": target_a(fa).tolist()},
    }
    ref["interp_a_on_grid"] = nan_list(interp(fa, response_a(fa), g))
    ref["interp_third_on_grid"] = nan_list(interp(third, target_a(third), g))
    probes = [19.0, 20.0, 31.1, 31.3, 31.62, 100.0, 1234.5, 19999.0, 20001.0, 20300.0, 25000.0]
    ref["interp_third_probes"] = {"f": probes, "dB": nan_list(interp(third, target_a(third), np.array(probes)))}

    ref["band_indices"] = {f"{lo}-{hi}": [int(i) for i in band_idx(g, lo, hi)[[0, -1]]]
                           for lo, hi in [(20, 10000), (50, 10000), (40, 10000), (100, 16000),
                                          (8000, 20000), (200, 500), (10000, 16000)]}

    ref["smoothing"] = {}
    for n in (1, 3, 6, 12, 48):
        ref["smoothing"][str(n)] = smooth_power(fa, response_a(fa), n).tolist()
    ref["smoothing_third_target_3"] = smooth_power(third, target_a(third), 3).tolist()
    h = 10 ** (response_a(fa) / 20) * np.exp(-2j * np.pi * fa * 1.5e-4)
    ref["complex_smoothing"] = {}
    for n in (3, 12):
        z = smooth_complex(fa, h, n)
        ref["complex_smoothing"][str(n)] = {"re": np.real(z).tolist(), "im": np.imag(z).tolist()}

    fs = np.array([10, 30, 60, 105, 200, 500, 1000, 2500, 5000, 10000, 20000], float)
    ref["shelves"] = {
        "f": fs.tolist(),
        "low_105_+6.44": shelf_db(fs, 105, 6.44, 1 / math.sqrt(2), False).tolist(),
        "low_105_-2_q0.5": shelf_db(fs, 105, -2.0, 0.5, False).tolist(),
        "high_2500_-1.41": shelf_db(fs, 2500, -1.41, 1 / math.sqrt(2), True).tolist(),
        "high_2500_+4_q1.2": shelf_db(fs, 2500, 4.0, 1.2, True).tolist(),
    }
    ref["bs708_limit_on_grid"] = nan_list(bs708_limit(g))

    at500 = ("at", 500.0)
    cases = {
        "a_vs_third": {"left": (fa, response_a(fa)), "target": (third, target_a(third), (31.0, 20000.0)),
                       "norm": at500},
        "ab_vs_dense_bandmean_s6_shelves": {
            "left": (fa, response_a(fa)), "right": (fb, response_b(fb)),
            "target": (fa, target_a(fa), (fa[0], fa[-1])), "norm": ("band", 200.0, 500.0),
            "smoothing": 6, "shelves": shelves(2.5, -1.0)},
        "a_vs_dense": {"left": (fa, response_a(fa)), "target": (fa, target_a(fa), (fa[0], fa[-1])),
                       "norm": at500},
    }
    rv = next(c for c in RAVIZZA["curves"] if c["id"] == RAVIZZA["primary"]["curve"])
    cases["a_vs_ravizza_primary"] = {
        "left": (fa, response_a(fa)),
        "target": (np.array(RAVIZZA["band_centres_Hz"], float), np.array(rv["gain_dB"], float),
                   (RAVIZZA["band_centres_Hz"][0], 20000.0)),
        "norm": at500}
    ref["cases"] = {k: evaluate(v) for k, v in cases.items()}

    # Harman-style reconstruction of the third-octave baseline, 2015 preset.
    pre = PERS["recipe"]["presets"][0]
    # Points of the reconstruction's own 1/48-octave grid 10^(k/160).
    fq = 10.0 ** (np.array([256, 323, 416, 544, 633]) / 160.0)
    ref["reconstruction"] = {
        "preset": pre["id"], "f": fq.tolist(),
        "dB": (interp(third, target_a(third), fq) + shelves_db(fq, shelves(pre["bass_dB"], pre["treble_dB"]))).tolist(),
    }

    OUT.write_text(json.dumps(ref, indent=0, allow_nan=False) + "\n")
    print(f"wrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
