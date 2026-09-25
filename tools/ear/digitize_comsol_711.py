#!/usr/bin/env python3
"""Digitise the IEC 60318-4 transfer-impedance curve plotted in COMSOL's
"Generic 711 Coupler" model documentation (Fig. 3) into private/.

The only openly published plot of the IEC 60318-4 Table 1 curve found for
this work is Fig. 3 of the COMSOL Application Gallery documentation "Generic
711 Coupler -- An Occluded Ear-Canal Simulator" (COMSOL 6.0), which draws
"Standard (IEC 60318-4)" as a red line with its upper and lower tolerances
as dotted red lines, in dB re 1 MPa s/m^3 from 100 Hz to 10 kHz:
https://doc.comsol.com/6.0/doc/com.comsol.help.models.aco.generic_711_coupler/generic_711_coupler.html

The digitised values are a reproduction of the standard's table, so they
are written only to the untracked private/ directory (CLAUDE.md, erratum
E41), never committed. tools/ear/fit_iec60318_4.py reports the literature
model against them and crates/acoustilab/tests/ear.rs checks them when the
file is present.

Method: the three red curves are separated column by column (anti-aliased
pixels weighted by their redness) at each third-octave frequency; the axes
are calibrated on the figure's gridlines (100 Hz ... 20 kHz, 18 ... 46 dB).
Where the nominal curve is hidden under the model curves the midpoint of
the tolerance lines is used and the row is flagged. Reading accuracy is
about +-0.1 dB (one pixel is 0.093 dB).

Usage:
    python3 tools/ear/digitize_comsol_711.py [--png path]
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
PRIVATE = ROOT / "private"
URL = (
    "https://doc.comsol.com/6.0/doc/com.comsol.help.models.aco.generic_711_coupler/"
    "images/generic_711_coupler.1.1.08.png"
)
DEFAULT_PNG = PRIVATE / "comsol_generic_711_fig3.png"
OUT = PRIVATE / "iec60318_4_comsol_fig3.json"

# Axis calibration of the 477 x 367 px figure, from its gridlines:
# x = X0 + XK*log10(f) (vertical gridlines at 100, 200, 500 Hz, ... 20 kHz),
# dB = (Y0 - y)/YK (horizontal gridlines every 2 dB from 46 to 18 dB).
SIZE = (477, 367)
X0, XK = -292.2446, 171.8490
Y0, YK = 532.0, 10.75
# The legend box covers part of the plot; red pixels inside it are ignored.
LEGEND = (45, 245, 262, 334)  # x0, x1, y0, y1
THIRD_OCTAVES = [100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600,
                 2000, 2500, 3150, 4000, 5000, 6300, 8000, 10000]


def load(path: Path) -> np.ndarray:
    import pymupdf

    pix = pymupdf.Pixmap(str(path))
    if pix.alpha:
        pix = pymupdf.Pixmap(pix, 0)
    if (pix.width, pix.height) != SIZE:
        raise SystemExit(f"{path}: unexpected size {pix.width}x{pix.height}; recalibrate")
    a = np.frombuffer(pix.samples, dtype=np.uint8).reshape(pix.height, pix.width, pix.n)
    return a[:, :, :3].astype(int)


def red_clusters(im: np.ndarray, col: int) -> list[float]:
    """Redness-weighted centroids (in px) of the red runs in one column."""
    r, g, b = im[:, col, 0], im[:, col, 1], im[:, col, 2]
    red = (r - np.maximum(g, b) > 35) & (np.abs(g - b) < 30)
    weight = (r - (g + b) / 2.0) / 255.0
    runs: list[list[int]] = []
    for y in np.where(red)[0]:
        if y < 36 or y > 336:
            continue  # frame and axis labels
        if LEGEND[0] <= col <= LEGEND[1] and LEGEND[2] <= y <= LEGEND[3]:
            continue
        if runs and y - runs[-1][-1] <= 1:
            runs[-1].append(int(y))
        else:
            runs.append([int(y)])
    out = []
    for run in runs:
        w = weight[run]
        out.append(float(np.sum(np.array(run) * w) / np.sum(w)))
    return out


def db(y: float) -> float:
    return (Y0 - y) / YK


def read_frequency(im: np.ndarray, f: float) -> dict:
    x = X0 + XK * math.log10(f)
    triples, pairs = [], []
    for dc in range(-2, 3):
        col = int(round(x)) + dc
        cl = red_clusters(im, col)
        if len(cl) == 3:
            triples.append((col - x, cl))
        elif len(cl) == 2:
            pairs.append((col - x, cl))
    if len(triples) >= 2:
        # Linear in column offset, evaluated at the exact frequency.
        off = np.array([t[0] for t in triples])
        ys = np.array([t[1] for t in triples])
        vals = [float(np.polyval(np.polyfit(off, ys[:, k], 1), 0.0)) for k in range(3)]
        up, nom, lo = (db(v) for v in vals)
        return {"f_Hz": f, "upper_dB": up, "nominal_dB": nom, "lower_dB": lo, "nominal_hidden": False}
    both = [(o, c) for o, c in triples] + [(o, [c[0], c[-1]]) for o, c in pairs]
    both = [(o, [c[0], c[-1]]) for o, c in both]
    if len(both) < 2:
        raise SystemExit(f"{f} Hz: could not separate the tolerance lines")
    off = np.array([t[0] for t in both])
    ys = np.array([t[1] for t in both])
    up, lo = (db(float(np.polyval(np.polyfit(off, ys[:, k], 1), 0.0))) for k in range(2))
    return {"f_Hz": f, "upper_dB": up, "nominal_dB": 0.5 * (up + lo), "lower_dB": lo, "nominal_hidden": True}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--png", type=Path, default=DEFAULT_PNG)
    args = ap.parse_args()
    if not args.png.exists():
        args.png.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["curl", "-sSL", "-o", str(args.png), URL], check=True)
    im = load(args.png)
    rows = [read_frequency(im, f) for f in THIRD_OCTAVES]
    for r in rows:
        for k in ("upper_dB", "nominal_dB", "lower_dB"):
            r[k] = round(r[k], 2)
    out = {
        "source": "Digitised from COMSOL 'Generic 711 Coupler' documentation (6.0) Fig. 3, curve "
        "'Standard (IEC 60318-4)' and its tolerances; a reproduction of IEC 60318-4 Table 1 -- "
        "do not commit",
        "url": URL,
        "units": "dB re 1 MPa s/m^3 (transfer impedance)",
        "reading_accuracy_dB": 0.1,
        "rows": rows,
    }
    OUT.write_text(json.dumps(out, indent=1) + "\n")
    for r in rows:
        hid = " (nominal hidden: midpoint)" if r["nominal_hidden"] else ""
        print(f"{r['f_Hz']:6.0f} Hz  {r['lower_dB']:6.2f} {r['nominal_dB']:6.2f} {r['upper_dB']:6.2f}{hid}")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
