#!/usr/bin/env python3
"""Derive the ITU-T P.57 Type 4.3 ear-canal area function from the Recommendation.

Source: ITU-T Recommendation P.57 (06/2021), "Artificial ears", free download
from https://www.itu.int/rec/T-REC-P.57-202106-I (English PDF).

What this script reads from the PDF (it does not redistribute the tables):
  * Table 6   - centre-line coordinates (0 to 28 mm in 0.5 mm steps), the DRP,
                the reference plane (17.33 mm) and the EEP projections.
  * Table B.2 - the periphery points of each cross section (0.5, 2, 4, ...,
                28 mm, the reference plane, and the concha-bottom planes at
                29.5, 31 and 32.5 mm) in the local (a_e, b_e) plane.
  * Table 5-c - the transfer-impedance target (times f, relative to 500 Hz)
                with its tolerances.

What it writes:
  * data/ear/type43_geometry.json (committed): *derived* quantities only -
    the area (shoelace formula) and perimeter of each cross-section polygon,
    the arc-length positions, and the axial position of the DRP projected on
    the centre line. These are model parameters computed by this script,
    not a copy of the Recommendation's tables.
  * private/p57_table5c.json (untracked, see CLAUDE.md): the verbatim
    Table 5-c values, used only by the fit script and by a test that skips
    when the file is absent.

Usage:
    python3 tools/ear/p57_geometry.py [--pdf path/to/T-REC-P.57-202106.pdf]

Without --pdf the script looks for private/T-REC-P.57-202106-I.pdf and
downloads it from the ITU site if it is missing (the ITU distributes it
free of charge).
"""

from __future__ import annotations

import argparse
import json
import math
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PRIVATE = ROOT / "private"
DATA = ROOT / "data" / "ear"
PDF_URL = (
    "https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-P.57-202106-I!!PDF-E&type=items"
)
DEFAULT_PDF = PRIVATE / "T-REC-P.57-202106-I.pdf"


def num(s: str) -> float:
    """Parses a number that may use the Unicode minus or en dash."""
    return float(s.replace("–", "-").replace("−", "-").replace(",", "."))


# Running footers (page number, "Rec. ITU-T P.57") sit below this y (points).
PAGE_FOOTER_Y = 790.0

NUM_RE = re.compile(r"^[–−-]?\d+(?:[.,]\d+)?$")


def load_pdf(path: Path):
    import pymupdf  # PyMuPDF

    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        print(f"downloading P.57 to {path}", file=sys.stderr)
        subprocess.run(["curl", "-sSL", "-o", str(path), PDF_URL], check=True)
    return pymupdf.open(str(path))


def page_index(doc, needle: str, start: int = 0) -> list[int]:
    return [i for i in range(start, len(doc)) if needle in doc[i].get_text()]


# ----- Table 6: centre line ------------------------------------------------


def parse_table6(doc) -> dict:
    """Centre-line points (s, x, y, z) and the landmark rows."""
    pages = page_index(doc, "Table 6 – The xm, ym, zm coordinates")
    text = "\n".join(doc[i].get_text() for i in pages)
    lines = [ln.strip() for ln in text.splitlines() if ln.strip()]
    points = []
    landmarks = {}
    i = 0

    def xyz_follows(i: int) -> bool:
        return i + 3 < len(lines) and all(NUM_RE.match(lines[i + k]) for k in (1, 2, 3))

    while i < len(lines):
        ln = lines[i]
        # Landmark rows first, consuming their three coordinates, so that the
        # next centre-line row starts in step (the DRP row sits between the
        # 0.5 mm and 1 mm rows).
        key = None
        if ln in ("DRP", "EEP", "ERP"):
            key = ln
        m = re.match(r"Ref\. Plane \((\d+(?:\.\d+)?)\)", ln)
        if m:
            landmarks["ref_plane_s_mm"] = float(m.group(1))
            key = "ref_plane"
        m = re.match(r"EEP Projection (\d+(?:\.\d+)?) mm", ln)
        if m:
            key = f"eep_projection_{m.group(1)}"
        if key is not None and xyz_follows(i):
            landmarks[key] = [num(lines[i + k]) for k in (1, 2, 3)]
            i += 4
            continue
        if NUM_RE.match(ln) and xyz_follows(i):
            s = num(ln)
            xyz = [num(lines[i + k]) for k in (1, 2, 3)]
            if 0.0 <= s <= 28.0 and abs(s * 2 - round(s * 2)) < 1e-9:
                points.append((s, *xyz))
            i += 4
            continue
        i += 1
    # De-duplicate (the table header repeats across pages) and sort.
    uniq = {p[0]: p for p in points}
    pts = [uniq[k] for k in sorted(uniq)]
    if len(pts) != 57:
        raise RuntimeError(f"Table 6: {len(pts)} centre-line points read, expected 57 (0 to 28 mm)")
    return {"points": pts, "landmarks": landmarks}


# ----- Table B.1: the planes bounding the concha bottom ------------------------


def parse_table_b1(doc) -> dict:
    """Points of the 28 mm plane (r_First) and of the last concha plane (r_Last).

    Annex B.1: the two intermediate concha planes are "evenly distributed"
    on the straight line between these points, r_i = r_First + (r_Last -
    r_First) i/3. Only the points are used; the printed normals are not
    needed (and n_Last's z component carries the opposite sign to the one
    that r_Last = r_First + n_Last D_First-Last implies).
    """
    page = page_index(doc, "Table B.1 – The points and normal vectors")[0]
    text = doc[page].get_text()
    text = text[text.index("Table B.1 – The points and normal vectors"):]
    vals = [num(t.strip()) for t in text.splitlines() if NUM_RE.match(t.strip()) and "." in t]
    if len(vals) < 12:
        raise RuntimeError("Table B.1: expected two points and two normals")
    return {"r_first": vals[0:3], "n_first": vals[3:6], "r_last": vals[6:9], "n_last": vals[9:12]}


# ----- Table B.2: cross-section polygons -------------------------------------


def parse_table_b2(doc) -> dict[str, list[tuple[float, float]]]:
    start = page_index(doc, "Table B.2 – Tabular values")[0]
    stop = page_index(doc, "B.3", start)[0]
    sections: dict[str, list[tuple[float, float]]] = {}
    for pg in range(start, stop + 1):
        words = doc[pg].get_text("words")
        # Section headers: "Cross section <label> mm" or "Cross section Ref. Plane".
        headers = []
        for k, w in enumerate(words):
            if w[4] == "Cross" and k + 2 < len(words) and words[k + 1][4] == "section":
                nxt = words[k + 2][4]
                if nxt == "Ref.":
                    label = "ref"
                else:
                    label = nxt
                headers.append((w[0], w[1], label))
        if not headers:
            continue
        # Stop at the B.3 heading on the last page.
        y_stop = min(
            (w[1] for w in words if w[4] == "B.3"), default=float("inf")
        )
        # Group headers into rows (same y), each followed by its a/b columns.
        rows_y = sorted({round(h[1]) for h in headers})
        for ry_i, ry in enumerate(rows_y):
            hs = sorted([h for h in headers if round(h[1]) == ry], key=lambda h: h[0])
            y_next = rows_y[ry_i + 1] if ry_i + 1 < len(rows_y) else y_stop
            # Column label words (a_e, b_e) just below the header row.
            cols = [
                w
                for w in words
                if ry + 5 < w[1] < ry + 40 and not NUM_RE.match(w[4]) and w[4] not in ("mm",)
            ]
            col_x = sorted(w[0] for w in cols)
            if len(col_x) != 2 * len(hs):
                raise RuntimeError(f"page {pg+1}: {len(col_x)} columns for {len(hs)} sections")
            nums = [
                w
                for w in words
                if NUM_RE.match(w[4]) and ry + 20 < w[1] < min(y_next - 2, PAGE_FOOTER_Y)
            ]
            by_row: dict[int, list] = {}
            for w in nums:
                by_row.setdefault(round(w[1] * 2), []).append(w)
            for _, ws in sorted(by_row.items()):
                cells: dict[int, float] = {}
                for w in ws:
                    # Nearest column: label left edge + 6 pt is the column centre.
                    centre = 0.5 * (w[0] + w[2])
                    j = min(range(len(col_x)), key=lambda c: abs((col_x[c] + 6) - centre))
                    cells[j] = num(w[4])
                for s_i, h in enumerate(hs):
                    a = cells.get(2 * s_i)
                    b = cells.get(2 * s_i + 1)
                    if a is None and b is None:
                        continue
                    if a is None or b is None:
                        raise RuntimeError(f"page {pg+1}: half row in section {h[2]}")
                    sections.setdefault(h[2], []).append((a, b))
    return sections


def polygon_area_perimeter(pts: list[tuple[float, float]]) -> tuple[float, float]:
    n = len(pts)
    a = 0.0
    p = 0.0
    for i in range(n):
        x0, y0 = pts[i]
        x1, y1 = pts[(i + 1) % n]
        a += x0 * y1 - x1 * y0
        p += math.hypot(x1 - x0, y1 - y0)
    return abs(a) / 2.0, p


# ----- Table 5-c ----------------------------------------------------------------


def parse_table_5c(doc) -> list[dict]:
    pages = page_index(doc, "Table 5-c – Transfer impedance")
    rows = []
    for pg in pages:
        text = doc[pg].get_text()
        # Everything after the table caption.
        text = text[text.index("Table 5-c"):]
        toks = [t.strip() for t in text.splitlines() if t.strip()]
        vals = [t for t in toks if NUM_RE.match(t.replace("+", ""))]
        # Rows are 4-tuples: f, level, tol+, tol- (two table halves interleaved).
        k = 0
        while k + 3 < len(vals):
            f, lv, tu, tl = vals[k : k + 4]
            rows.append(
                {
                    "f_Hz": num(f),
                    "level_dB": num(lv),
                    "tol_upper_dB": num(tu.replace("+", "")),
                    "tol_lower_dB": num(tl),
                }
            )
            k += 4
    rows = {r["f_Hz"]: r for r in rows}
    return [rows[k] for k in sorted(rows)]


# ----- Derived geometry -----------------------------------------------------------


def arc_length(points) -> list[float]:
    s = [0.0]
    for p0, p1 in zip(points, points[1:]):
        s.append(s[-1] + math.dist(p0[1:], p1[1:]))
    return s


def drp_axial_position(points, drp) -> float:
    """Arc-length position of the plane, normal to the centre line, through the DRP.

    The centre line is piecewise linear between the tabulated points; the
    position is the parameter where (DRP - c(s)) . c'(s) changes sign.
    """
    best = None
    for p0, p1 in zip(points, points[1:]):
        a = p0[1:]
        b = p1[1:]
        t = [b[i] - a[i] for i in range(3)]
        seg = math.sqrt(sum(v * v for v in t))
        u = [v / seg for v in t]
        f0 = sum((drp[i] - a[i]) * u[i] for i in range(3))
        f1 = sum((drp[i] - b[i]) * u[i] for i in range(3))
        if f0 >= 0.0 >= f1 or (best is None and f0 < 0.0):
            lam = f0 / (f0 - f1) if f0 != f1 else 0.0
            lam = min(max(lam, 0.0), 1.0)
            return p0[0] + lam * (p1[0] - p0[0])
    return best if best is not None else 0.0


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pdf", type=Path, default=DEFAULT_PDF)
    args = ap.parse_args()
    doc = load_pdf(args.pdf)

    t6 = parse_table6(doc)
    pts = t6["points"]
    s_arc = arc_length(pts)
    # Nominal positions are the 0.5 mm labels; report the polyline length too.
    nominal = [p[0] for p in pts]
    stretch = s_arc[-1] / nominal[-1]
    lm = t6["landmarks"]
    s_drp = drp_axial_position(pts, lm["DRP"])
    d_drp_tip = math.dist(lm["DRP"], pts[0][1:])

    # Concha-bottom planes (labels 29.5, 31 and 32.5 mm): beyond the 28 mm
    # plane the labels are names, not distances. Annex B.1 places the planes
    # evenly on the straight line from the 28 mm centre-line point to the
    # last plane's point, so their axial positions are 28 + i*D/3.
    b1 = parse_table_b1(doc)
    d_first_last = math.dist(b1["r_first"], b1["r_last"])
    concha = {29.5: 1, 31.0: 2, 32.5: 3}

    polys = parse_table_b2(doc)
    sections = []
    for label, poly in polys.items():
        area, perim = polygon_area_perimeter(poly)
        if label == "ref":
            pos = lm["ref_plane_s_mm"]
        elif float(label) in concha:
            pos = 28.0 + concha[float(label)] * d_first_last / 3.0
        else:
            pos = float(label)
        sections.append(
            {
                "position_mm": round(pos, 3),
                "label_mm": None if label == "ref" else float(label),
                "area_mm2": round(area, 3),
                "perimeter_mm": round(perim, 3),
                "points": len(poly),
                "ref_plane": label == "ref",
            }
        )
    sections.sort(key=lambda s: s["position_mm"])
    # The EEP is taken at the concha plane labelled 31 mm, the plane nearest
    # to it (its "EEP projection" lies about 1 mm from the EEP).
    eep = next(s["position_mm"] for s in sections if s["label_mm"] == 31.0)

    geometry = {
        "source": "ITU-T P.57 (06/2021), clause 6.4.3.4, Table 6 and Annex B Table B.2; "
        "https://www.itu.int/rec/T-REC-P.57-202106-I",
        "derived_by": "tools/ear/p57_geometry.py (shoelace area and perimeter of each "
        "Table B.2 polygon; positions up to 28 mm are the Recommendation's plane labels "
        "along the curved centre line, measured from the canal tip; the concha-bottom "
        "planes labelled 29.5, 31 and 32.5 mm sit at 28 + i*D/3 with D the distance "
        "between the two points of Table B.1 (Annex B.1))",
        "note": "Areas and perimeters are computed from the tabulated periphery points; the "
        "Recommendation's point lists themselves are not reproduced here.",
        "positions_measured_from": "tip of the ear canal (centre-line origin, Table 6)",
        "ref_plane_mm": lm["ref_plane_s_mm"],
        "eep_mm": eep,
        "eep_note": "EEP taken at the concha plane labelled 31 mm, the plane nearest to it",
        "concha_plane_spacing_mm": round(d_first_last / 3.0, 4),
        "drp_axial_mm": round(s_drp, 3),
        "drp_to_tip_distance_mm": round(d_drp_tip, 3),
        "centre_line_polyline_length_0_to_28_mm": round(s_arc[-1], 3),
        "centre_line_stretch_factor": round(stretch, 4),
        "sections": sections,
    }
    DATA.mkdir(parents=True, exist_ok=True)
    (DATA / "type43_geometry.json").write_text(json.dumps(geometry, indent=2) + "\n")
    print(json.dumps({k: v for k, v in geometry.items() if k != "sections"}, indent=2))
    for s in sections:
        print(f"{s['position_mm']:6.2f} mm  A={s['area_mm2']:7.2f} mm2  P={s['perimeter_mm']:6.2f} mm  n={s['points']}")

    t5c = parse_table_5c(doc)
    PRIVATE.mkdir(parents=True, exist_ok=True)
    out = {
        "source": "ITU-T P.57 (06/2021) Table 5-c (verbatim; do not commit)",
        "reference": "transfer impedance times f, relative to 500 Hz; nominal effective "
        "volume 1.63 cm3; 27.7 MPa.s/m3 at 500 Hz (clause 6.4.3.3 NOTE 2)",
        "rows": t5c,
    }
    (PRIVATE / "p57_table5c.json").write_text(json.dumps(out, indent=1) + "\n")
    print(f"Table 5-c: {len(t5c)} rows -> {PRIVATE / 'p57_table5c.json'}")


if __name__ == "__main__":
    main()
