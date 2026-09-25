#!/usr/bin/env python3
"""Cross-check of the preference scores against AutoEq itself.

Runs AutoEq's own `harman_overear_preference_score` and
`harman_inear_preference_score` (autoeq.frequency_response.FrequencyResponse,
MIT licence, https://github.com/jaakkopasanen/AutoEq) on the synthetic
response and target of tools/targets/reference.py, both sampled on the same
native frequencies, with `error = raw - target`:

* over-ear on AutoEq's own evaluation grid;
* over-ear with AutoEq's grid replaced by this project's exact 1/12-octave
  grid 10^(k/40) (k = 52..172). Band membership then agrees exactly (AutoEq
  keeps 50 <= f <= 10000, which is 50.12 Hz to 10 kHz here), so SD and slope
  must agree to rounding;
* in-ear on AutoEq's own grid (its 500 Hz lookup needs a grid point at
  exactly 500 Hz, which the exact grid does not have).

AutoEq evaluates on rounded R40 frequencies; this project on the exact values
behind them. The difference between the two grids is what the in-ear and
first over-ear comparisons measure. AutoEq's grid itself is not written to
the fixture.

Needs AutoEq installed (`pip install autoeq`; 4.1.2 was used). Writes
crates/acoustilab/tests/data/targets_autoeq.json.
Usage: python3 tools/targets/autoeq_crosscheck.py
"""

from __future__ import annotations

import json
import sys
from importlib.metadata import version
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True  # keep tools/targets free of __pycache__
sys.path.insert(0, str(Path(__file__).resolve().parent))
import reference as ref  # noqa: E402

import autoeq.frequency_response as aeq  # noqa: E402
from autoeq.frequency_response import FrequencyResponse  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "acoustilab" / "tests" / "data" / "targets_autoeq.json"


def fr(f, raw, target):
    return FrequencyResponse(name="case", frequency=f.copy(), raw=raw.copy(), error=raw - target)


def main():
    f = ref.native(24)
    raw, target = ref.response_a(f), ref.target_a(f)
    out = {
        "autoeq_version": version("autoeq"),
        "case": "response_a vs target_a on the 1/24-octave native grid (tools/targets/reference.py, case a_vs_dense)",
    }
    s, sd, slope = fr(f, raw, target).harman_overear_preference_score()
    out["over_ear_autoeq_grid"] = {"score": s, "sd": sd, "slope": slope}
    own = list(aeq.HARMAN_OVEREAR_PREFERENCE_FREQUENCIES)
    try:
        aeq.HARMAN_OVEREAR_PREFERENCE_FREQUENCIES = [float(x) for x in ref.grid12()]
        s, sd, slope = fr(f, raw, target).harman_overear_preference_score()
    finally:
        aeq.HARMAN_OVEREAR_PREFERENCE_FREQUENCIES = own
    out["over_ear_exact_grid"] = {"score": s, "sd": sd, "slope": slope}
    s, sd, slope, mean = fr(f, raw, target).harman_inear_preference_score()
    out["in_ear_autoeq_grid"] = {"score": s, "sd": sd, "slope": slope, "me": mean}
    OUT.write_text(json.dumps(out, indent=1, default=float) + "\n")
    print(json.dumps(out, indent=1, default=float))


if __name__ == "__main__":
    main()
