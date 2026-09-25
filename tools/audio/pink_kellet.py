#!/usr/bin/env python3
"""Refits Paul Kellet's "refined" pink-noise filter for other sample rates.

Source of the structure and of the 44.1 kHz coefficients: Paul Kellet's
post to the music-dsp mailing list of 17 October 1999, as quoted on Robin
Whittle's page "DSP generation of Pink (1/f) Noise",
https://www.firstpr.com.au/dsp/pink-noise/ (retrieved 2026-09-25):
"This is an approximation to a -10dB/decade filter using a weighted sum of
first order filters. It is accurate to within +/-0.05dB above 9.2Hz (44100Hz
sampling rate). Unity gain is at Nyquist":

    b0 = 0.99886 * b0 + white * 0.0555179;
    b1 = 0.99332 * b1 + white * 0.0750759;
    b2 = 0.96900 * b2 + white * 0.1538520;
    b3 = 0.86650 * b3 + white * 0.3104856;
    b4 = 0.55000 * b4 + white * 0.5329522;
    b5 = -0.7616 * b5 - white * 0.0168980;
    pink = b0 + b1 + b2 + b3 + b4 + b5 + b6 + white * 0.5362;
    b6 = white * 0.115926;

i.e. H(z) = sum_i g_i/(1 - p_i z^-1) + d + e z^-1 (b6 is the previous
sample's white term). The spec (Section 16) asks for coefficients refitted
for 48 and 96 kHz rather than the 44.1 kHz ones reused.

Method, per rate fs:
1. Start: the positive poles keep their analogue corner frequencies,
   p' = p^(44100/fs) (matched z); the negative pole and every gain are
   Kellet's.
2. Fit all 14 numbers by scipy.optimize.least_squares on the dB error
   against the ideal 10*log10(C/f) (C free) at 64 points per octave from
   9.2*fs/44100 Hz (Kellet's lower limit, scaled) to 0.9 of Nyquist, then
   refine towards a minimax fit by iteratively reweighted least squares
   (weights grown where the error is largest). The poles are bounded
   to [-POLE_LIMIT_NEGATIVE, POLE_LIMIT_POSITIVE]: the dB error sees
   only |H|, and an unbounded fit put the negative pole at -1.19 at 88.2
   and 96 kHz, whose magnitude matches but whose recursion diverges (the
   programme was all NaN).
3. Scale so that |H| = 1 at 1 kHz, and check every pole is inside the
   unit circle.

In normalised frequency f/fs the fit band is the same at every rate and
the target 1/f has no scale, so the problem is the same at every rate and
the fits converge to one filter up to gain (the 48 and 88.2 kHz poles
agree to 8 digits). What the rate changes is where 20 Hz falls in the band.

The script also checks Kellet's own claim at 44.1 kHz. It writes
web/src/audio/pink-coefficients.ts (and prints each rate's largest error
over its fit band and over 20 Hz-20 kHz).

Usage: python3 tools/audio/pink_kellet.py
"""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
from scipy.optimize import least_squares

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "web" / "src" / "audio" / "pink-coefficients.ts"

KELLET_POLES = [0.99886, 0.99332, 0.96900, 0.86650, 0.55000, -0.7616]
KELLET_GAINS = [0.0555179, 0.0750759, 0.1538520, 0.3104856, 0.5329522, -0.0168980]
KELLET_D = 0.5362
KELLET_E = 0.115926
RATES = [44100.0, 48000.0, 88200.0, 96000.0]
# Pole bounds for the fit. The slowest pole (Kellet's 0.99886 at 44.1 kHz,
# 0.99948 at 96 kHz by matched z) stays well below the positive bound; a
# negative pole at the negative bound decays by 60 dB in about 690 samples.
POLE_LIMIT_POSITIVE = 0.99999
POLE_LIMIT_NEGATIVE = 0.99


def response(params, f, fs):
    poles, gains, d, e = params[:6], params[6:12], params[12], params[13]
    z1 = np.exp(-2j * np.pi * f / fs)  # z^-1
    h = d + e * z1
    for p, g in zip(poles, gains):
        h = h + g / (1 - p * z1)
    return h


def db_error(params, f, fs):
    """dB error against 10 log10(C/f) with the best C (mean removed)."""
    m = 20 * np.log10(np.abs(response(params, f, fs)))
    ideal = -10 * np.log10(f)
    e = m - ideal
    return e - np.mean(e)


def grid(lo, hi, ppo=64):
    n = int(math.ceil(math.log2(hi / lo) * ppo))
    return lo * (hi / lo) ** (np.arange(n + 1) / n)


def fit(fs):
    lo, hi = 9.2 * fs / 44100.0, 0.9 * fs / 2
    f = grid(lo, hi)
    start = np.array(
        [p ** (44100.0 / fs) if p > 0 else p for p in KELLET_POLES] + KELLET_GAINS + [KELLET_D, KELLET_E]
    )
    if fs == 44100.0:
        start = np.array(KELLET_POLES + KELLET_GAINS + [KELLET_D, KELLET_E])
    lb = np.full(14, -np.inf)
    ub = np.full(14, np.inf)
    lb[:6], ub[:6] = -POLE_LIMIT_NEGATIVE, POLE_LIMIT_POSITIVE
    start[:6] = np.clip(start[:6], lb[:6], ub[:6])
    w = np.ones_like(f)
    x = start
    for _ in range(12):
        res = least_squares(
            lambda q: w * db_error(q, f, fs), x, bounds=(lb, ub), xtol=1e-15, ftol=1e-15, gtol=1e-15, max_nfev=4000
        )
        x = res.x
        e = np.abs(db_error(x, f, fs))
        w = w * (1 + e / e.max())
        w = w / w.mean()
    # Scale: |H(1 kHz)| = 1.
    k = 1 / abs(response(x, np.array([1000.0]), fs)[0])
    x = x.copy()
    x[6:14] *= k
    return x, f


def report(x, fs, lo, hi):
    f = grid(lo, hi, 128)
    e = db_error(x, f, fs)
    return float(np.max(np.abs(e)))


def main():
    out = {}
    # Kellet's own filter at 44.1 kHz, as published.
    kx = np.array(KELLET_POLES + KELLET_GAINS + [KELLET_D, KELLET_E])
    kellet_err = report(kx, 44100.0, 9.2, 0.9 * 22050)
    print(f"Kellet 44.1 kHz as published: max |error| {kellet_err:.4f} dB over 9.2 Hz-19.8 kHz")
    for fs in RATES:
        if fs == 44100.0:
            x = kx.copy()
            x[6:14] *= 1 / abs(response(x, np.array([1000.0]), fs)[0])
            kind = "published (Kellet 1999), scaled to unit gain at 1 kHz"
        else:
            x, _ = fit(fs)
            kind = "refitted (tools/audio/pink_kellet.py)"
        assert np.all(np.abs(x[:6]) < 1), f"unstable pole at {fs} Hz: {x[:6]}"
        fit_lo, fit_hi = 9.2 * fs / 44100.0, 0.9 * fs / 2
        err_fit = report(x, fs, fit_lo, fit_hi)
        err_audio = report(x, fs, 20.0, min(20000.0, 0.9 * fs / 2))
        out[int(fs)] = {
            "poles": [float(v) for v in x[:6]],
            "gains": [float(v) for v in x[6:12]],
            "direct": float(x[12]),
            "delayed": float(x[13]),
            "kind": kind,
            "fit_band_Hz": [fit_lo, fit_hi],
            "max_error_dB": err_fit,
            "max_error_20Hz_20kHz_dB": err_audio,
        }
        print(f"{fs:8.0f} Hz: {kind}; max |error| {err_fit:.4f} dB over {fit_lo:.1f} Hz-{fit_hi:.0f} Hz, {err_audio:.4f} dB over 20 Hz-20 kHz")
    lines = [
        "// Generated by tools/audio/pink_kellet.py; do not edit by hand.",
        "//",
        "// Paul Kellet's \"refined\" pink-noise filter (music-dsp mailing list,",
        "// 17 October 1999, quoted at https://www.firstpr.com.au/dsp/pink-noise/,",
        "// retrieved 2026-09-25): H(z) = Σ_i g_i/(1 − p_i·z⁻¹) + d + e·z⁻¹, driven",
        "// by white noise. 44.1 kHz: the published coefficients; other rates:",
        "// refitted to 10·log10(C/f) (poles and gains free, least squares on the",
        "// dB error, reweighted towards minimax), see the script's docstring. All",
        "// are scaled to |H| = 1 at 1 kHz. maxErrorDb is the largest deviation",
        "// from −3.01 dB/octave over the fit band (9.2 Hz·fs/44.1 kHz to 0.9 of",
        "// Nyquist), maxErrorAudioDb over 20 Hz–20 kHz.",
        "",
        "export interface PinkCoefficients {",
        "  poles: number[];",
        "  gains: number[];",
        "  direct: number;",
        "  delayed: number;",
        "  kind: string;",
        "  fitBandHz: [number, number];",
        "  maxErrorDb: number;",
        "  maxErrorAudioDb: number;",
        "}",
        "",
        "export const PINK_KELLET: Record<number, PinkCoefficients> = {",
    ]
    for fs, c in out.items():
        lines.append(f"  {fs}: {{")
        lines.append(f"    poles: {json.dumps(c['poles'])},")
        lines.append(f"    gains: {json.dumps(c['gains'])},")
        lines.append(f"    direct: {c['direct']!r},")
        lines.append(f"    delayed: {c['delayed']!r},")
        lines.append(f"    kind: {json.dumps(c['kind'])},")
        lines.append(f"    fitBandHz: [{c['fit_band_Hz'][0]!r}, {c['fit_band_Hz'][1]!r}],")
        lines.append(f"    maxErrorDb: {c['max_error_dB']!r},")
        lines.append(f"    maxErrorAudioDb: {c['max_error_20Hz_20kHz_dB']!r},")
        lines.append("  },")
    lines.append("};")
    OUT.write_text("\n".join(lines) + "\n")
    print("wrote", OUT)


if __name__ == "__main__":
    main()
