#!/usr/bin/env python3
"""Independent reference values for crates/acoustilab/tests/analysis.rs.

Writes crates/acoustilab/tests/data/analysis_reference.json. Everything is
reimplemented here from the published definitions, not from the Rust code:

* SplitMix64 and xoshiro256** (Steele, Lea and Flood 2014; Blackman and
  Vigna 2021, reference C code at prng.di.unimi.it), in exact integer
  arithmetic.
* Lemire's unbiased bounded integers (ACM TOMACS 29(1), 2019) and the
  Fisher-Yates shuffle drawing below(i + 1) for i from the last index down.
* The normal quantile by mpmath: the root of log(ncdf(z)) = log(p) at two
  working precisions (40 and 60 digits), independent of AS 241.
* The Latin hypercube plan of docs/analysis.md, step by step: permutation,
  jitter ((x >> 32) + 1/2)/2^32, u = (perm + v)/n, then the distribution
  (normal: mu + t/2 * z; uniform: mu + t(2u - 1); lognormal:
  mu * exp(ln(1 + rel)/2 * z)) evaluated with mpmath and clipped.
* The canonical JSON text of docs/analysis.md (sorted keys, 12 significant
  digits in scientific notation) and its SHA-256 by hashlib.
* Percentiles by numpy.percentile (method "linear").

Every float is stored as repr(), which round-trips exactly; the Rust test
parses the strings with str::parse (correctly rounded).

Run from anywhere:  python3 tools/analysis/reference.py
"""

import hashlib
import json
import os
import sys

import mpmath as mp
import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "refgen"))
from common import DATA, REPO, s, stable  # noqa: E402

MASK = (1 << 64) - 1


def rotl(x, k):
    return ((x << k) | (x >> (64 - k))) & MASK


class SplitMix64:
    def __init__(self, seed):
        self.state = seed & MASK

    def next(self):
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK
        return z ^ (z >> 31)


class Xoshiro256:
    def __init__(self, seed):
        sm = SplitMix64(seed)
        self.s = [sm.next() for _ in range(4)]

    def next(self):
        s = self.s
        result = (rotl((s[1] * 5) & MASK, 7) * 9) & MASK
        t = (s[1] << 17) & MASK
        s[2] ^= s[0]
        s[3] ^= s[1]
        s[1] ^= s[2]
        s[0] ^= s[3]
        s[2] ^= t
        s[3] = rotl(s[3], 45)
        return result

    def below(self, n):
        m = self.next() * n
        if (m & MASK) < n:
            t = ((1 << 64) - n) % n
            while (m & MASK) < t:
                m = self.next() * n
        return m >> 64

    def shuffle(self, v):
        for i in range(len(v) - 1, 0, -1):
            j = self.below(i + 1)
            v[i], v[j] = v[j], v[i]


def norm_quantile(p):
    """Phi^-1(p) at the working precision, for a double p in (0, 1)."""
    pm = mp.mpf(p)
    flipped = pm > mp.mpf(0.5)
    if flipped:
        pm = 1 - pm  # exact in mpmath
    if pm == mp.mpf(0.5):
        return mp.mpf(0)
    # Initial guess from the asymptotic tail or a plain normal deviate.
    guess = -mp.sqrt(-2 * mp.log(pm)) if pm < 0.1 else mp.mpf(-1)
    lp = mp.log(pm)
    z = mp.findroot(lambda z: mp.log(mp.ncdf(z)) - lp, guess)
    return -z if flipped else z


def quantiles():
    ps = [1e-300, 1e-100, 1e-20, 1.4e-11, 1.3887943864964021e-11, 1e-10, 1e-5,
          1e-3, 0.01, 0.02425, 0.05, 0.075, 0.07500000000000001, 0.1, 0.2,
          0.3, 0.4, 0.45, 0.49, 0.5, 0.51, 0.6, 0.7, 0.8, 0.9, 0.925, 0.95,
          0.975, 0.99, 0.999, 1 - 1e-10, 1 - 2 ** -53]
    rows = []
    for p in ps:
        z = stable(lambda: norm_quantile(p), rtol=mp.mpf("1e-25"))
        rows.append([s(p), s(z)])
    return rows


def lhs_unit(n, d, seed):
    rng = Xoshiro256(seed)
    u = [[0.0] * d for _ in range(n)]
    for j in range(d):
        perm = list(range(n))
        rng.shuffle(perm)
        for i in range(n):
            v = ((rng.next() >> 32) + 0.5) * (1.0 / 4294967296.0)
            u[i][j] = min((perm[i] + v) / n, 1.0 - 2.0 ** -53)
    return u


def lhs_values(u, dists):
    out = []
    with mp.workdps(40):
        for row in u:
            vals = []
            for uj, d in zip(row, dists):
                z = norm_quantile(uj) if d["dist"] != "uniform" else None
                mu = mp.mpf(d["nominal"])
                if d["dist"] == "normal":
                    x = mu + mp.mpf(d["half_width"]) / 2 * z
                elif d["dist"] == "uniform":
                    x = mu + mp.mpf(d["half_width"]) * (2 * mp.mpf(uj) - 1)
                else:
                    x = mu * mp.exp(mp.log(1 + mp.mpf(d["rel"])) / 2 * z)
                x = float(x)
                if d.get("min") is not None:
                    x = max(x, d["min"])
                if d.get("max") is not None:
                    x = min(x, d["max"])
                vals.append(s(x))
            out.append(vals)
    return out


def canonical_number(x):
    x = float(x)
    if x == 0:
        return "0"
    m, e = f"{x:.11e}".split("e")
    if "." in m:
        m = m.rstrip("0").rstrip(".")
    return f"{m}e{int(e)}"


def canonical(v):
    if v is None:
        return "null"
    if v is True:
        return "true"
    if v is False:
        return "false"
    if isinstance(v, (int, float)):
        return canonical_number(v)
    if isinstance(v, str):
        return json.dumps(v, ensure_ascii=False)
    if isinstance(v, list):
        return "[" + ",".join(canonical(x) for x in v) + "]"
    return "{" + ",".join(json.dumps(k, ensure_ascii=False) + ":" + canonical(v[k])
                          for k in sorted(v)) + "}"


def netlist_text(doc):
    doc = {k: v for k, v in doc.items() if k not in ("title", "description", "ui", "schema")}
    return canonical(doc)


def canonical_cases():
    rng = np.random.RandomState(7)
    floats = [float(m * 10.0 ** e) for m, e in
              zip(rng.uniform(-10, 10, 300), rng.randint(-310, 300, 300))]
    floats += [1.0, 25.0, 0.08, 1e-320, 5e-324, 1.7976931348623157e308, 123456789012.5,
               0.1 + 0.2, 1 / 3, 2 / 3, 100.0, 1e21, 1e-7]
    cases = [
        {"doc": {"b": [1, 2.5, True, None, "x"], "a": {"z": "q\"\\\n\t\u0001", "y": 0.1, "é": -0.0},
                 "n": 123456789012345678, "k": [], "o": {}},
         "netlist": False},
        {"doc": {"floats": floats}, "netlist": False},
    ]
    for name in ("sealed_cup.json", "open_back.json"):
        with open(os.path.join(REPO, "examples", name)) as fh:
            cases.append({"doc": json.load(fh), "netlist": True, "file": name})
    out = []
    for c in cases:
        text = netlist_text(c["doc"]) if c["netlist"] else canonical(c["doc"])
        out.append({
            "doc": json.dumps(c["doc"], ensure_ascii=False),
            "netlist": c["netlist"],
            "text": text,
            "sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(),
        })
    return out


def percentiles():
    rng = np.random.RandomState(11)
    data = rng.normal(100.0, 3.0, size=(37, 6))
    data[5, 2] = data[6, 2]  # a tie
    qs = [0.05, 0.1, 0.5, 0.9, 0.95]
    expected = {str(q): [s(v) for v in np.percentile(data, q * 100, axis=0, method="linear")]
                for q in qs}
    return {
        "data": [[s(v) for v in row] for row in data],
        "q": qs,
        "expected": expected,
        "min": [s(v) for v in data.min(axis=0)],
        "max": [s(v) for v in data.max(axis=0)],
    }


def main():
    streams = {}
    for seed in [0, 1, 42, 0xDEADBEEF, MASK]:
        sm = SplitMix64(seed)
        x = Xoshiro256(seed)
        streams[str(seed)] = {
            "splitmix64": [str(sm.next()) for _ in range(5)],
            "xoshiro256": [str(x.next()) for _ in range(8)],
        }
    x = Xoshiro256(2024)
    below = [[n, [str(x.below(n)) for _ in range(6)]] for n in [1, 2, 3, 7, 1000, 2 ** 63 + 5]]
    x = Xoshiro256(99)
    perm = list(range(20))
    x.shuffle(perm)

    dists = [
        {"name": "a", "dist": "normal", "nominal": 10.0, "half_width": 1.0, "min": 9.25},
        {"name": "b", "dist": "uniform", "nominal": 2.0, "half_width": 0.3},
        {"name": "c", "dist": "lognormal", "nominal": 0.08, "rel": 0.5, "min": 0.05},
    ]
    n, seed = 16, 42
    u = lhs_unit(n, len(dists), seed)
    doc = {
        "about": "Reference values for tests/analysis.rs, written by tools/analysis/reference.py.",
        "streams": streams,
        "below_seed": 2024,
        "below": below,
        "shuffle_seed": 99,
        "shuffle": perm,
        "quantiles": quantiles(),
        "lhs": {
            "n": n,
            "seed": seed,
            "dists": dists,
            "unit": [[s(v) for v in row] for row in u],
            "values": lhs_values(u, dists),
        },
        "canonical": canonical_cases(),
        "percentiles": percentiles(),
    }
    path = os.path.join(DATA, "analysis_reference.json")
    with open(path, "w") as fh:
        json.dump(doc, fh, indent=1, ensure_ascii=False)
        fh.write("\n")
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
