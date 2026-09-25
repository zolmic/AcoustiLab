#!/usr/bin/env python3
"""Build data/targets/ravizza2023_5128.json from the Zenodo record.

Source: G. Ravizza, J. Villegas, C. P. Volk and T. Stegenborg-Andersen,
"Preference ratings and 32 magnitude frequency response curves", Zenodo,
version 1.0, 2023-09-29, doi:10.5281/zenodo.8388242, licence CC-BY-4.0
(read from the record's metadata, `metadata.license.id == "cc-by-4.0"`).
Companion paper: "An over-ear headphone target curve for Bruel & Kjaer head
and torso simulator type 5128 measurements", Proc. 155th AES Convention,
New York, October 2023 (Express Paper 127, AES e-library 22281).

The two CSV files are downloaded (or read from --from DIR), checked against
the MD5 sums Zenodo publishes, and converted:

* the 32 curves are copied verbatim (30 graphic-equaliser band gains in dB at
  the band centres printed in the CSV header, 31 Hz to 25 kHz);
* the 16 128 preference ratings (56 assessors, 11 programmes of which each
  assessor heard 9, 0-100 scale) are
  summarised per curve: n, mean, sample SD, median, the per-country means
  (assessor ids start with DK or JP) and the rank by mean. The raw ratings are
  not copied; the script regenerates the summary from the record.

Nothing is smoothed, re-referenced or corrected. Anomalies found in the data
are written into the file's notes, not fixed.

Usage: python3 tools/targets/ravizza2023.py [--from DIR]
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import statistics
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "data" / "targets" / "ravizza2023_5128.json"

RECORD = "https://zenodo.org/api/records/8388242"
FILES = {
    "MagnitudeFrequencyResponses.csv": "70dc33df8b206801a9661d1f609b8e82",
    "PreferenceRatings.csv": "51318a6f933a81544b2765bbef4a2341",
}
RETRIEVED = "2026-09-25"

# The curve designated as the bundled target: the highest mean preference
# rating in the published ratings (checked below, not assumed).
PRIMARY = "APHarm2018v2"


def fetch(name: str, local: Path | None) -> bytes:
    if local is not None:
        data = (local / name).read_bytes()
    else:
        url = f"{RECORD}/files/{name}/content"
        with urllib.request.urlopen(url, timeout=60) as r:
            data = r.read()
    md5 = hashlib.md5(data).hexdigest()
    if md5 != FILES[name]:
        raise SystemExit(f"{name}: md5 {md5} does not match the record's {FILES[name]}")
    return data


def number(text: str) -> float | int:
    v = float(text)
    return int(v) if v.is_integer() and "." not in text else v


def curves(data: bytes):
    rows = list(csv.reader(io.StringIO(data.decode("utf-8")), delimiter=";"))
    header = rows[0]
    assert header[0] == "HeadphoneCurve/Frequency band", header[0]
    freqs = [number(h) for h in header[1:]]
    out = []
    for r in rows[1:]:
        if not r:
            continue
        assert len(r) == len(header), r
        out.append({"id": r[0], "gain_dB": [number(v) for v in r[1:]]})
    return freqs, out


def ratings(data: bytes):
    rows = list(csv.DictReader(io.StringIO(data.decode("utf-8")), delimiter=";"))
    assert {r["Attributes"] for r in rows} == {"Preference"}
    by: dict[str, list[float]] = {}
    by_country: dict[tuple[str, str], list[float]] = {}
    for r in rows:
        v = float(r["Rating"])
        by.setdefault(r["HeadphoneCurve"], []).append(v)
        by_country.setdefault((r["HeadphoneCurve"], r["AssessorID"][:2]), []).append(v)
    assessors = sorted({r["AssessorID"] for r in rows})
    programmes = sorted({r["Sample"] for r in rows})
    summary = {}
    for k, v in by.items():
        summary[k] = {
            "n": len(v),
            "mean": round(statistics.fmean(v), 4),
            "sd": round(statistics.stdev(v), 4),
            "median": round(statistics.median(v), 4),
            "mean_DK": round(statistics.fmean(by_country[(k, "DK")]), 4),
            "mean_JP": round(statistics.fmean(by_country[(k, "JP")]), 4),
        }
    order = sorted(summary, key=lambda k: -summary[k]["mean"])
    for i, k in enumerate(order):
        summary[k]["rank"] = i + 1
    return summary, assessors, programmes, len(rows)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--from", dest="local", type=Path, default=None)
    args = ap.parse_args()
    freqs, cs = curves(fetch("MagnitudeFrequencyResponses.csv", args.local))
    summary, assessors, programmes, n_rows = ratings(fetch("PreferenceRatings.csv", args.local))

    # The ratings file spells one curve differently from the curves file.
    alias = {"AvgAllmeas": "AVGAllMeas"}
    summary = {alias.get(k, k): v for k, v in summary.items()}
    ids = [c["id"] for c in cs]
    assert sorted(ids) == sorted(summary), (sorted(ids), sorted(summary))
    top = min(summary, key=lambda k: summary[k]["rank"])
    assert top == PRIMARY, top
    for c in cs:
        c["ratings"] = summary[c["id"]]

    doc = {
        "schema": "acoustilab-targets/0.1",
        "dataset": "ravizza2023_5128",
        "title": "Ravizza et al. 2023: 32 over-ear magnitude responses on the B&K 5128 with preference ratings",
        "provenance": {
            "class": "research",
            "class_note": (
                "AES Express Paper: reviewed on an extended summary, not as a complete manuscript "
                "(AES convention author guidelines), so not classed as peer-reviewed."
            ),
            "source": (
                "G. Ravizza, J. Villegas, C. P. Volk, T. Stegenborg-Andersen, 'Preference ratings and 32 "
                "magnitude frequency response curves', Zenodo dataset v1.0, 2023-09-29. Companion paper: "
                "G. Ravizza, J. Villegas, T. Stegenborg-Andersen, C. P. Volk, 'An over-ear headphone target "
                "curve for Bruel & Kjaer head and torso simulator type 5128 measurements', Proc. 155th AES "
                "Convention, New York, October 2023, Express Paper 127 (AES e-library 22281)."
            ),
            "doi": "10.5281/zenodo.8388242",
            "url": "https://zenodo.org/records/8388242",
            "licence": "CC-BY-4.0",
            "licence_evidence": "Zenodo record metadata, license.id = 'cc-by-4.0' (read 2026-09-25 from https://zenodo.org/api/records/8388242)",
            "attribution": "Curves and ratings: Ravizza, Villegas, Volk and Stegenborg-Andersen (2023), doi:10.5281/zenodo.8388242, CC BY 4.0. Converted to JSON and summarised by AcoustiLab (tools/targets/ravizza2023.py); the gains are unchanged.",
            "retrieved": RETRIEVED,
            "files": [{"name": k, "md5": v} for k, v in FILES.items()],
            "generator": "tools/targets/ravizza2023.py",
        },
        "fixture": "bk5128",
        "reference_point": "drp",
        "baseline": (
            "Drum-reference-point response on the B&K HATS 5128C as a 30-band graphic-equaliser gain per "
            "band, relative to an arbitrary level (the record normalises each curve so that its largest "
            "band gain is 0 dB)."
        ),
        "band_centres_Hz": freqs,
        "representation": (
            "Gains at the graphic-equaliser band centres printed in the CSV header (nominal third-octave "
            "values, 31 Hz to 25 kHz). The record does not describe the equaliser's filters, so a curve is "
            "taken as its band gains at those frequencies with linear interpolation in dB on log frequency. "
            "Resolution is third-octave; finer detail of the stimuli is not represented."
        ),
        "ratings_summary": {
            "rows": n_rows,
            "assessors": len(assessors),
            "assessors_DK": sum(a.startswith("DK") for a in assessors),
            "assessors_JP": sum(a.startswith("JP") for a in assessors),
            "programmes": programmes,
            "scale": "0 to 100, attribute 'Preference', 8 curves per multi-stimulus trial page",
            "statistics": (
                "Per curve over all assessors, programmes and trials: n, mean, sample SD, median, mean of "
                "the DK and the JP assessors, and rank by mean (1 = highest). Descriptive only: the ratings "
                "are repeated measures, so these are not the paper's mixed-model estimates."
            ),
        },
        "primary": {
            "curve": PRIMARY,
            "reason": (
                "Highest mean preference rating in the published ratings (computed by the generator). "
                "SoundGuys' report of the paper (https://www.soundguys.com/soundguys-headphone-preference-"
                "curve-validated-in-aes-paper-102536/, read 2026-09-25) also names the modified Harman 2018 "
                "curve as ranked first overall and the SoundGuys curve as 10th of 32, which the summary "
                "reproduces. The paper itself (AES e-library, paywalled) was not read."
            ),
        },
        "notes": [
            "Curves whose ids start with HP are the paper's eight measured closed circumaural headphones (HP1-HP8) and two modifications of each (Mod1, Mod2). APHarm2015, APHarm2015v2, APHarm2018 and APHarm2018v2 are the authors' adaptations of the Harman 2015 and 2018 over-ear targets to the 5128; DF5128 and FF5128 are diffuse- and free-field curves for the 5128; Soundguys is the SoundGuys preference curve; AVGAllMeas is the average of the measured headphones. These readings of the ids follow the record's description and the SoundGuys report; the paper was not read.",
            "The adaptations of third-party curves (Harman, SoundGuys, the 5128 diffuse and free field) are published by the depositors under CC-BY-4.0. They are the depositors' curves, not the originators' data, and are flagged as adaptations.",
            "HP6, HP6Mod1 and HP6Mod2 have +17.3 dB in the 20 kHz band between -17.3 dB at 16 kHz and 25 kHz; a sign error in the record is likely. Kept as published.",
            "Soundguys has 0.0 dB in the 25 kHz band after -18.1 dB at 20 kHz; probably a missing value. Kept as published.",
            "The ratings file spells AVGAllMeas as AvgAllmeas; the generator maps it.",
        ],
        "curves": cs,
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(doc, indent=1, ensure_ascii=False)
    # One curve per line keeps the file reviewable.
    OUT.write_text(compact_arrays(text) + "\n")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(cs)} curves, top {top}")


def compact_arrays(text: str) -> str:
    """Puts every array of numbers or short strings on one line."""
    out, buf, depth = [], [], 0
    for line in text.splitlines():
        s = line.strip()
        if depth == 0 and s.endswith("[") and not s.endswith("{["):
            buf = [line.rstrip()]
            depth = 1
            continue
        if depth:
            if s.startswith("{") or s.startswith("["):
                out.extend(buf)
                out.append(line)
                buf, depth = [], 0
                continue
            if s in ("]", "],"):
                items = " ".join(x.strip() for x in buf[1:])
                out.append(f"{buf[0]}{items}{s}")
                buf, depth = [], 0
                continue
            buf.append(s)
            continue
        out.append(line)
    return "\n".join(out)


if __name__ == "__main__":
    main()
