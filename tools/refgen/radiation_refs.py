#!/usr/bin/env python3
"""mpmath reference for the unflanged-pipe radiation (special::unflanged_pipe).

Levine & Schwinger, "On the radiation of sound from an unflanged circular
pipe", Phys. Rev. 73, 383-406 (1948). With theta(x) the continuous branch of
arctan(-J1(x)/Y1(x)) (= atan2(J1, -Y1) below the first zero of J1), the
plane-mode reflection coefficient at the open end is R = -|R| exp(-2jkL)
(e^{+jwt}) with, for ka < j_{1,1} = 3.8317,

  -ln|R| = (2ka/pi) int_0^ka theta(x) dx / (x sqrt(k^2a^2 - x^2))
  L/a    = (1/pi) int_0^ka ln[pi J1(x) sqrt(J1^2 + Y1^2)] dx / (x sqrt(k^2a^2 - x^2))
         + (1/pi) int_0^inf ln[1 / (2 I1(x) K1(x))] dx / (x sqrt(x^2 + k^2a^2)).

These forms are checked here against the low-frequency limits |R| = 1 -
(ka)^2/2 and L/a(0) = 0.6127 (Levine & Schwinger printed 0.6133; the
integral evaluates to 0.6127010359..., as also reported (0.6127) in AIP Conf.
Proc. 2195, 020034 (2019)), and against the approximation formulae of Silva et al., J.
Sound Vib. 322, 255 (2009), Eqs. (21)-(22), which were fitted to
Levine-Schwinger values (the script prints the deviation; the paper states
under 2 % for ka < 3).

Writes crates/acoustilab/tests/data/special_unflanged.json with
A = -ln|R| and L/a for ka in [0, 3.8].

Usage: python3 tools/refgen/radiation_refs.py
"""

import numpy as np
import mpmath as mp

from common import s, stable, write

GEN = "tools/refgen/radiation_refs.py"


def theta(x):
    return mp.atan2(mp.besselj(1, x), -mp.bessely(1, x))


def attenuation(ka):
    # x = ka sin(phi) removes the endpoint singularity.
    f = lambda p: theta(ka * mp.sin(p)) / mp.sin(p)
    return 2 / mp.pi * mp.quad(f, [0, mp.pi / 4, 3 * mp.pi / 8, 7 * mp.pi / 16, mp.pi / 2])


def extra_digits(x):
    """Near x = 0 both logarithms are O(x^2 ln x): 2 I1 K1 and
    pi J1 |H1| cancel against 1, so evaluate them with extra digits."""
    return 10 + int(2 * max(0, -mp.log10(x))) if x > 0 else 10


def neg_log_2i1k1(x):
    with mp.extradps(extra_digits(x)):
        return +(-mp.log(2 * mp.besseli(1, x) * mp.besselk(1, x)))


def end_correction(ka):
    def g(p):
        x = ka * mp.sin(p)
        with mp.extradps(extra_digits(x)):
            j, y = mp.besselj(1, x), mp.bessely(1, x)
            return +(mp.log(mp.pi * j * mp.sqrt(j * j + y * y)) / x)

    first = mp.quad(g, [0, mp.pi / 4, 3 * mp.pi / 8, 7 * mp.pi / 16, mp.pi / 2]) / mp.pi if ka > 0 else 0
    h = lambda x: neg_log_2i1k1(x) / (x * mp.sqrt(x * x + ka * ka))
    pts = sorted(set([mp.mpf(0), mp.mpf(1), mp.mpf(10), mp.mpf(100)] + ([mp.mpf(ka)] if ka > 0 else [])))
    second = mp.quad(h, pts + [mp.inf]) / mp.pi
    return first + second


def silva(ka):
    """Silva et al. (2009) Eqs. (21)-(22), Table 1, unflanged."""
    beta, a1, a2, a3 = 0.5, 0.800, 0.266, 0.0263
    eta, b1, b2, b3, b4 = 0.6133, 0.0599, 0.238, -0.0153, 0.00150
    x2 = ka * ka
    r = (1 + a1 * x2) / (1 + (beta + a1) * x2 + a2 * x2**2 + a3 * x2**3)
    l = eta * (1 + b1 * x2) / (1 + b2 * x2 + b3 * x2**2 + b4 * x2**3)
    return r, l


def main():
    kas = [0.0] + list(np.logspace(-4, -1, 7)) + list(np.arange(0.2, 3.8001, 0.2)) + [3.7, 3.75, 3.8]
    rows = []
    for ka in sorted(set(float(round(k, 12)) for k in kas)):
        v = stable(lambda: [attenuation(mp.mpf(ka)) if ka > 0 else mp.mpf(0), end_correction(mp.mpf(ka))],
                   dps=(30, 40), rtol=mp.mpf("1e-20"))
        r, l = mp.exp(-v[0]), v[1]
        if ka > 0:
            rs, ls = silva(ka)
            print(f"ka={ka:<8.4g} |R|={mp.nstr(r, 12):<16} L/a={mp.nstr(l, 12):<16} "
                  f"Silva dev |R| {100 * (rs / float(r) - 1):+.2f} %, L {100 * (ls / float(l) - 1):+.2f} %, "
                  f"|R|/(1-(ka)^2/2)-1 = {mp.nstr(r / (1 - ka * ka / 2) - 1, 3)}")
        else:
            print(f"ka=0: L/a = {mp.nstr(l, 20)}")
        rows.append([s(ka), s(v[0]), s(v[1])])
    write(
        "special_unflanged.json",
        {
            "generator": GEN,
            "mpmath": mp.__version__,
            "precision": "mpmath quad at 30 and 40 digits, agreeing to 1e-20",
            "description": "Levine-Schwinger unflanged pipe: A = -ln|R| and end correction L/a versus ka",
        },
        ["ka", "attenuation", "end_correction"],
        rows,
    )


if __name__ == "__main__":
    main()
