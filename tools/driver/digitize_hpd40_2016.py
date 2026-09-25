#!/usr/bin/env python3
"""Digitise the curves of the 2016 Tymphany HPD-40N16PET00-32 sheet.

Source: "Driver Specification Sheet, Model No: HPD-40N16PET00-32, Rev 1,
Last Update: 2016-12-09 01:00:59", Peerless by Tymphany,
https://www.toutlehautparleur.com/media/catalog/product/datasheet/peerless/HPD-40N16PET00-32.pdf
It prints the same Thiele-Small values as the 2018-07-11 sheet that the
driver record quotes. Unlike that sheet, it has a filled "Frequency and
Impedance Response" chart, with an impedance curve and an on-axis SPL curve
"@ 2.83V/1m" (docs/spec-errata.md E5).

The PDF and every digitised curve stay in the untracked private/ directory
(CLAUDE.md: record derived values with citations, not verbatim tables). The
committed record quotes only the summary values this script prints.

Method:
* The chart is the page's embedded 800 x 400 RGB image, a lossless
  Highcharts export. Canvas coordinates put pixel centres at index + 0.5.
* Axes. The left axis runs 60-110 dB and the right 0-200 ohm, linear, on
  the same gridlines: 60 dB = 0 ohm at y = 305.0 and 110 dB = 200 ohm at
  y = 10.0 (29.5 px per 5 dB, per 20 ohm). The x axis is logarithmic, with
  20 Hz at x = 71.0 and 20 kHz at x = 729.0.
  - The script checks the calibration against the image. The crisp
    gridlines must lie on the rows round(305 - 29.5 k), k = 1..9 (k = 0 is
    the x axis), and the decade lines (100 Hz, 1 kHz, 10 kHz) within 0.5 px
    of the model.
* Curves. The SPL line is (170, 70, 67) and the impedance line (0, 0, 0),
  both anti-aliased over white or grey. Each pixel is unmixed into
  coverages from its red and green channels:
  - a_red = (R - G)/100;
  - a_black = (255 - R - 85 a_red)/255.
  The curve's position in each column is the coverage-weighted centroid of
  the heaviest run of pixels.
* Accuracy. The fit of the gridlines leaves less than 0.5 px in y and 0.35 px
  in x. The centroid adds about 0.2 px. That is ±0.1 dB, ±0.5 ohm and
  ±0.4 % in frequency, well inside the sheet's own tolerances.
* Limits of the curves.
  - The SPL curve runs along the chart's floor below about 40 Hz, so it
    starts after its last point under 62 dB.
  - Where the two curves touch (90-110 Hz), the unmixing still separates
    them. This was checked by overlaying the trace on the image.

Outputs, in private/drivers/hpd_40n16pet00_32_2016/:
* impedance.csv and spl.csv: at 12 points per octave, each with a sidecar
  (FILE.sidecar.json, docs/fitting.md);
* fit_spec.json: the joint fit for tools/driver/datasheet_bench.json:
  - the impedance from 20 Hz to 2 kHz;
  - the SPL from 250 Hz to 1.2 kHz, where the diaphragm is mass-controlled,
    read as half space with the free-air moving mass (the notes of record
    tymphany_hpd_40n16pet00_32_curves_2016 compare fits with a baffle's air
    load added through the bench's `baffle_air_mg`);
* summary.json: the values quoted in the record's `plotted` block.

Run from the repository root (needs numpy and PyMuPDF):
    python3 tools/driver/digitize_hpd40_2016.py
    cargo run -p acoustilab-cli --release -- fit tools/driver/datasheet_bench.json \\
        --spec private/drivers/hpd_40n16pet00_32_2016/fit_spec.json \\
        --out private/drivers/hpd_40n16pet00_32_2016/fit_report.json
"""

import csv
import json
import math
import os
import sys
import urllib.request

import numpy as np
import pymupdf

URL = ("https://www.toutlehautparleur.com/media/catalog/product/datasheet/"
       "peerless/HPD-40N16PET00-32.pdf")
SOURCE = ("Peerless by Tymphany, Driver Specification Sheet, Model No: "
          "HPD-40N16PET00-32, Rev 1, Last Update 2016-12-09 01:00:59, chart "
          "'Frequency and Impedance Response'")
LICENCE = ("Public manufacturer datasheet. The curves are digitised for "
           "analysis and kept out of the repository; the document is not "
           "redistributed.")
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
PRIVATE = os.path.join(ROOT, "private")
OUT = os.path.join(PRIVATE, "drivers", "hpd_40n16pet00_32_2016")
PDF = os.path.join(PRIVATE, "HPD-40N16PET00-32_2016-12-09.pdf")

# Calibration (canvas coordinates).
Y_AXIS, Y_TOP = 305.0, 10.0          # 60 dB = 0 ohm, 110 dB = 200 ohm
X_20, X_20K = 71.0, 729.0            # 20 Hz, 20 kHz
PX_PER_DB = (Y_AXIS - Y_TOP) / 50.0
PX_PER_OHM = (Y_AXIS - Y_TOP) / 200.0
PX_PER_DECADE = (X_20K - X_20) / 3.0

LINE_SPL = (170, 70, 67)


def freq(x):
    return 20.0 * 10 ** ((x - X_20) / PX_PER_DECADE)


def x_of(f):
    return X_20 + PX_PER_DECADE * math.log10(f / 20.0)


def fetch():
    if not os.path.exists(PDF):
        os.makedirs(PRIVATE, exist_ok=True)
        req = urllib.request.Request(URL, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=60) as r, open(PDF, "wb") as fh:
            fh.write(r.read())
    doc = pymupdf.open(PDF)
    text = doc[0].get_text()
    if "2016-12-09" not in text or "HPD-40N16PET00-32" not in text:
        sys.exit(f"{PDF} is not the 2016-12-09 revision of the sheet")
    for img in doc[0].get_images(full=True):
        xref, _, w, h = img[0], img[1], img[2], img[3]
        pix = pymupdf.Pixmap(doc, xref)
        if (w, h) == (800, 400) and pix.n == 3:
            return np.frombuffer(pix.samples, dtype=np.uint8).reshape(h, w, 3).astype(float)
    sys.exit("no 800 x 400 chart image in the PDF")


def check_calibration(a):
    lum = a.mean(axis=2)
    grey = (np.ptp(a, axis=2) < 2) & (lum > 200) & (lum < 240)
    rows = [r for r in range(a.shape[0]) if grey[r, 80:720].sum() > 300]
    want = sorted({int(math.floor(Y_AXIS - 29.5 * k + 0.5)) for k in range(1, 10)})
    missing = [r for r in want if r not in rows]
    if missing:
        sys.exit(f"gridline rows {missing} not found (found {rows}): calibration does not apply")
    deficit = np.where(grey, 255.0 - lum, 0.0)
    for f in (100.0, 1000.0, 10000.0):
        x0 = int(round(x_of(f)))
        xs = np.arange(x0 - 3, x0 + 3)
        w = deficit[15:300][:, xs].sum(axis=0)
        centre = (w * (xs + 0.5)).sum() / w.sum()   # canvas x of the line
        if abs(centre - x_of(f)) > 0.5:
            sys.exit(f"decade line at {f:.0f} Hz found at x = {centre:.2f}, model {x_of(f):.2f}")


def trace(alpha, threshold):
    xs, ys = [], []
    y0, y1 = int(Y_TOP), int(Y_AXIS)
    for c in range(int(X_20), int(X_20K)):
        col = alpha[y0:y1, c]
        idx = np.where(col > threshold)[0]
        if len(idx) == 0:
            continue
        runs = np.split(idx, np.where(np.diff(idx) > 1)[0] + 1)
        best = max(runs, key=lambda r: col[r].sum())
        rr = np.arange(max(best[0] - 1, 0), min(best[-1] + 2, len(col)))
        w = col[rr]
        xs.append(c + 0.5)
        ys.append((w * (rr + y0 + 0.5)).sum() / w.sum())
    return np.array(xs), np.array(ys)


def resample(f, v, f_lo, f_hi, per_octave=12):
    """Values at 1 kHz * 2^(k/per_octave) within [f_lo, f_hi], linear in ln f."""
    k0 = math.ceil(per_octave * math.log2(f_lo / 1000.0))
    k1 = math.floor(per_octave * math.log2(f_hi / 1000.0))
    grid = 1000.0 * 2.0 ** (np.arange(k0, k1 + 1) / per_octave)
    return grid, np.interp(np.log(grid), np.log(f), v)


def sidecar(quantity, extra):
    s = {"schema": "acoustilab-curve-sidecar/0.1", "quantity": quantity,
         "smoothing": "none", "compensation": "none",
         "provenance": {"origin": "digitized", "source": SOURCE, "url": URL,
                        "licence": LICENCE, "tool": "tools/driver/digitize_hpd40_2016.py"}}
    s.update(extra)
    return s


def write_curve(name, header, f, v, sc):
    path = os.path.join(OUT, name)
    with open(path, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(["frequency_Hz", header])
        for fk, vk in zip(f, v):
            w.writerow([f"{fk:.6g}", f"{vk:.6g}"])
    with open(path + ".sidecar.json", "w") as fh:
        json.dump(sc, fh, indent=1)
        fh.write("\n")


def main():
    a = fetch()
    check_calibration(a)
    R, G = a[:, :, 0], a[:, :, 1]
    alpha_red = np.clip((R - G) / (LINE_SPL[0] - LINE_SPL[1]), 0, 1)
    alpha_black = np.clip((255.0 - R - (255.0 - LINE_SPL[0]) * alpha_red) / 255.0, 0, 1)

    xz, yz = trace(alpha_black, 0.3)
    fz, z = freq(xz), (Y_AXIS - yz) / PX_PER_OHM
    xs, ys = trace(alpha_red, 0.15)
    fs_, spl = freq(xs), 60.0 + (Y_AXIS - ys) / PX_PER_DB
    # Below about 40 Hz the curve runs along the chart's floor and is
    # clipped there: start after the last point under 62 dB.
    low = np.where((spl < 62.0) & (fs_ < 200.0))[0]
    first = low[-1] + 1 if len(low) else 0
    fs_, spl = fs_[first:], spl[first:]

    os.makedirs(OUT, exist_ok=True)
    gz, vz = resample(fz, z, 20.0, 20000.0)
    gs, vs = resample(fs_, spl, fs_[0], 20000.0)
    sc_z = sidecar("impedance", {
        "notes": "Free-air impedance as plotted; the condition is not stated on the sheet.",
        "uncertainty": {"noise_dB": 0.1}})
    sc_p = sidecar("pressure", {
        "calibrated": True, "drive": {"voltage_V": 2.83}, "source_impedance_ohm": 0,
        "reference_point": "on axis, 1 m",
        "fixture": "half space (the sheet labels its sensitivity 'Half Space')",
        "notes": "On-axis SPL as plotted, 'SPL (dB) @ 2.83V/1m'. The sheet states the sensitivity to +/- 1.0 dB, taken here as two standard deviations of the calibration.",
        "uncertainty": {"microphone_calibration_dB": 0.5, "noise_dB": 0.1}})
    write_curve("impedance.csv", "magnitude_ohm", gz, vz, sc_z)
    write_curve("spl.csv", "level_dB", gs, vs, sc_p)

    i = int(np.argmax(z))
    band = (fs_ >= 300.0) & (fs_ <= 1000.0)
    lb = np.log(fs_[band])
    level_band = float(np.trapezoid(spl[band], lb) / (lb[-1] - lb[0]))
    summary = {
        "source": SOURCE, "url": URL,
        "impedance_max": {"f_Hz": round(float(fz[i]), 1), "value_ohm": round(float(z[i]), 2)},
        "impedance_at": {f"{f:g}": round(float(np.interp(np.log(f), np.log(fz), z)), 2)
                         for f in (20, 50, 1000, 2000, 10000, 20000)},
        "impedance_min_above_peak": {
            "f_Hz": round(float(fz[i:][np.argmin(z[i:])]), 0),
            "value_ohm": round(float(z[i:].min()), 2)},
        "spl_at": {f"{f:g}": round(float(np.interp(np.log(f), np.log(fs_), spl)), 2)
                   for f in (50, 100, 200, 300, 500, 1000, 2000, 5000)},
        "spl_band_300_1000_Hz_mean_dB": round(level_band, 2),
        "spl_first_Hz": round(float(fs_[0]), 1),
    }
    with open(os.path.join(OUT, "summary.json"), "w") as fh:
        json.dump(summary, fh, indent=1)
        fh.write("\n")

    def doc(quantity, key, f, v, sc):
        return {"schema": "acoustilab-curve/0.1", "quantity": quantity,
                "frequencies_Hz": [float(x) for x in f], key: [float(x) for x in v],
                "sidecar": sc}

    spec = {
        "schema": "acoustilab-fit/0.1",
        "parameters": [
            {"name": "fs_Hz", "start": 81.8}, {"name": "Qms", "start": 2.71},
            {"name": "Qes", "start": 1.01}, {"name": "Re_ohm", "start": 32.8},
            {"name": "Mms_g", "start": 0.3}],
        "curves": [
            {"probe": "zin", "curve": doc("impedance", "magnitude_ohm", gz, vz, sc_z),
             "f_min_Hz": 20, "f_max_Hz": 2000},
            {"probe": "p_1m", "curve": doc("pressure", "level_dB", gs, vs, sc_p),
             "f_min_Hz": 250, "f_max_Hz": 1200}],
        "starts": 8, "seed": 1,
    }
    with open(os.path.join(OUT, "fit_spec.json"), "w") as fh:
        json.dump(spec, fh, indent=1)
        fh.write("\n")
    print(json.dumps(summary, indent=1))
    print("wrote", os.path.relpath(OUT, ROOT))


if __name__ == "__main__":
    main()
