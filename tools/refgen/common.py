"""Shared helpers for the reference-data generators.

Every fixture stores its numbers as decimal strings produced by Python's
`repr(float)`, which round-trips exactly. The Rust tests parse them with
`str::parse::<f64>`, which is correctly rounded, so the Rust code sees the
same double inputs that mpmath evaluated. (serde_json's default float parser
is only "best effort" and may differ by one ulp.)

mpmath evaluates at the exact binary value of each double input; a result is
accepted only if two evaluations at different working precisions agree to
far below the tolerances the tests use (see `stable`).
"""

import json
import os

import mpmath as mp

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DATA = os.path.join(REPO, "crates", "acoustilab", "tests", "data")


def s(x):
    """Exact decimal string of a double (or of an mpf rounded to double)."""
    return repr(float(x))


def mpc_of(re, im):
    """mpmath complex with the exact binary value of two doubles."""
    return mp.mpc(mp.mpf(float(re)), mp.mpf(float(im)))


def stable(f, dps=(40, 60), rtol=mp.mpf("1e-30")):
    """Evaluates f() at two working precisions and checks they agree.

    Returns the higher-precision value. Raises if the two differ by more than
    `rtol` relative (or absolute, for values near zero), which would mean the
    reference itself is not trustworthy.
    """
    vals = []
    is_list = False
    for d in dps:
        with mp.workdps(d):
            v = f()
            is_list = isinstance(v, (list, tuple))
            vals.append(list(v) if is_list else [v])
    for a, b in zip(vals[0], vals[1]):
        scale = max(abs(b), mp.mpf(1e-300))
        if abs(a - b) > rtol * scale and abs(a - b) > mp.mpf("1e-60"):
            raise RuntimeError(f"unstable reference: {a} vs {b}")
    return vals[1] if is_list else vals[1][0]


def write(name, meta, columns, rows):
    path = os.path.join(DATA, name)
    os.makedirs(DATA, exist_ok=True)
    assert all(len(r) == len(columns) for r in rows)
    with open(path, "w") as fh:
        fh.write("{\n")
        for k, v in meta.items():
            fh.write(f"{json.dumps(k)}: {json.dumps(v)},\n")
        fh.write(f'"columns": {json.dumps(columns)},\n"rows": [\n')
        fh.write(",\n".join(json.dumps(r) for r in rows))
        fh.write("\n]\n}\n")
    print(f"wrote {path}: {len(rows)} rows")
