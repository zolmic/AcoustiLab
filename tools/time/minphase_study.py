#!/usr/bin/env python3
"""Design study of the cepstral minimum phase (docs/time-domain.md, "Minimum phase").

An independent numpy implementation of the engine's method and of simpler
variants, applied to analytic spectra served the way the engine serves them
(fs = 48 kHz, N = 8192, band solved to 20 kHz, power-law continuation and
half-cosine taper above, DC from the asymptote). It prints the largest phase
error of the analysis counterpart against the analytic phase over
20 Hz-20 kHz for each variant, and the energy at negative times of the
minimum-phase filter. Writes no fixture; the engine tests
(crates/acoustilab/tests/time.rs) assert the resulting accuracy.

Variants:
  plain      cepstrum of ln|H| on the N grid (DC value floored)
  dc         the DC zero divided out analytically (D = s/(s + w_c), with
             the order and corner the engine estimates from its probe)
  full P,R   D and the high-frequency factor (1 + s/w_t)^n divided out,
             refinement R by zero padding the impulse response of the
             remainder X, frequency extension P with X flat above f_hi
  pad H      as full P=8 R=4, but zero padding the impulse response of H
             itself (wrong near DC when H's response is longer than N)

Run from anywhere:  python3 tools/time/minphase_study.py
"""

import numpy as np

FS, N, FHI = 48000.0, 8192, 20000.0
DF = FS / N
K = np.arange(N // 2 + 1)
F = K * DF
S = 2j * np.pi * F


def hp(s, fc=100.0):
    a = 2 * np.pi * fc
    return s / (s + a)


def lp2(s, f0=1000.0, q=5.0):
    w = 2 * np.pi * f0
    return w * w / (s * s + s * w / q + w * w)


def notch(s, f0=200.0, qz=20.0, qp=2.0):
    w = 2 * np.pi * f0
    return (s * s + s * w / qz + w * w) / (s * s + s * w / qp + w * w)


CASES = [
    ("high-pass 100 Hz", hp),
    ("low-pass 1 kHz Q5", lp2),
    ("notch 200 Hz Qz20", notch),
    ("leak 0.2 Hz x low-pass 1 kHz Q2 x notch 3 kHz",
     lambda s: hp(s, 0.2) * lp2(s, 1000.0, 2.0) * notch(s, 3000.0, 10.0, 2.0)),
]


def served(fun):
    """The engine's uniform spectrum: solved to FHI, continuation and taper above."""
    h = fun(S).astype(complex)
    khi = int(np.floor(FHI / DF))
    kb = int(round(khi * 2 ** (-1 / 6)))
    n_hi = np.clip(np.log(abs(h[khi]) / abs(h[kb])) / np.log(khi / kb), -4, 1)
    dphidf = np.sum(np.angle(h[kb + 1:khi + 1] / h[kb:khi])) / ((khi - kb) * DF)
    for k in range(khi + 1, N // 2 + 1):
        f = k * DF
        w = 0.5 * (1 + np.cos(np.pi * (f - FHI) / (FS / 2 - FHI)))
        h[k] = abs(h[khi]) * (f / FHI) ** n_hi * w * np.exp(1j * (np.angle(h[khi]) + dphidf * (f - FHI)))
    fa = DF / 1024
    ha, hb = fun(2j * np.pi * fa), fun(4j * np.pi * fa)
    slope = np.log2(abs(hb) / abs(ha))
    h[0] = 0.0 if slope > 0.5 else (2 * ha - hb).real
    h[-1] = h[-1].real
    # DC order and corner as the engine estimates them from the probe.
    m = int(round(slope)) if abs(slope) > 0.5 else 0
    fc = min(max(fa * (abs(h[1]) / abs(ha)) ** (1.0 / m), fa), DF) if m else DF
    return h, khi, n_hi, (fa, ha), m, fc


def fold_exp(log_half):
    m = 2 * (len(log_half) - 1)
    c = np.fft.ifft(np.concatenate([log_half, log_half[-2:0:-1]])).real
    cf = np.zeros(m)
    cf[0], cf[m // 2] = c[0], c[m // 2]
    cf[1:m // 2] = 2 * c[1:m // 2]
    return np.exp(np.fft.fft(cf))[:m // 2 + 1]


def plain(h):
    mag = np.abs(h)
    return fold_exp(np.log(np.maximum(mag, 1e-15 * mag.max())))


def factors(m, fc, khi, n_hi, use_g):
    wc, wt = 2 * np.pi * (fc or DF), 2 * np.pi * khi * DF

    def fac(f):
        s = 2j * np.pi * np.asarray(f, dtype=float)
        d = (s / (s + wc)) ** m if m else np.ones_like(s)
        return d * ((1 + s / wt) ** n_hi if use_g else 1.0)
    return fac


def method(h, khi, n_hi, probe, m, fc, refine, extend, use_g=True, pad_h=False):
    fac = factors(m, fc, khi, n_hi, use_g)
    with np.errstate(divide="ignore", invalid="ignore"):
        x = h / fac(F)
    x0 = probe[1] / fac(probe[0]) if m else x[1]
    x[0] = abs(x0) * np.sign(x0.real)
    x[-1] = x[-1].real
    nr = N * refine
    ir = np.fft.irfft(h if pad_h else x, N)
    buf = np.zeros(nr)
    buf[:N // 2], buf[nr - N // 2:] = ir[:N // 2], ir[N // 2:]
    fine = np.abs(np.fft.rfft(buf)) if refine > 1 else np.abs(x)
    if pad_h and refine > 1:
        with np.errstate(divide="ignore", invalid="ignore"):
            fine = fine / np.abs(fac(np.arange(nr // 2 + 1) * DF / refine))
    fine[::refine] = np.abs(x)
    m_len = nr * extend
    ff = np.arange(m_len // 2 + 1) * DF / refine
    jhi = khi * refine
    rem = np.empty(m_len // 2 + 1)
    rem[:jhi + 1] = fine[:jhi + 1]
    rem[jhi + 1:] = abs(h[khi]) * (ff[jhi + 1:] / (khi * DF)) ** n_hi / np.abs(fac(ff[jhi + 1:]))
    rem[0] = abs(x[0])
    xm = fold_exp(np.log(np.maximum(rem, 1e-15 * rem.max())))
    return xm[:(N // 2) * refine + 1:refine] * fac(F)


def fir_late(h, probe, m, fc):
    """Energy at negative times of the discrete minimum-phase filter of |h|."""
    fac = factors(m, fc, 1, 0.0, False)
    with np.errstate(divide="ignore", invalid="ignore"):
        x = h / fac(F)
    x[0] = abs(probe[1] / fac(probe[0])) if m else abs(x[1])
    x[-1] = x[-1].real
    r = 4
    ir = np.fft.irfft(x, N)
    buf = np.zeros(N * r)
    buf[:N // 2], buf[N * r - N // 2:] = ir[:N // 2], ir[N // 2:]
    fine = np.abs(np.fft.rfft(buf))
    fine[::r] = np.abs(x)
    hm = fold_exp(np.log(np.maximum(fine, 1e-15 * fine.max())))[::r] * fac(F)
    hm = np.abs(h) * np.exp(1j * np.angle(hm))
    hm[0], hm[-1] = abs(h[0]), hm[-1].real
    t = np.fft.irfft(hm, N)
    return 10 * np.log10(np.sum(t[N // 2:] ** 2) / np.sum(t ** 2))


def err(h_true, hm):
    band = (F >= 20) & (F <= 20000)
    e = np.degrees(np.abs(np.angle(h_true[band] / hm[band])))
    return e.max()


def main():
    for name, fun in CASES:
        h_true = fun(S)
        h, khi, n_hi, probe, m, fc = served(fun)
        rows = [("plain", plain(h))]
        if m:
            rows.append(("dc", method(h, khi, n_hi, probe, m, fc, 1, 1, use_g=False)))
        for p in (1, 4, 8, 16):
            for r in (1, 4):
                rows.append((f"full P={p:2d} R={r}", method(h, khi, n_hi, probe, m, fc, r, p)))
        rows.append(("P=8 R=4 pad H", method(h, khi, n_hi, probe, m, fc, 4, 8, pad_h=True)))
        print(name)
        for label, hm in rows:
            with np.errstate(invalid="ignore"):
                print(f"  {label:14s} max phase error {err(h_true, hm):10.4f} deg")
        mixed = np.fft.irfft(h, N)
        print(f"  negative-time energy: mixed phase {10 * np.log10(np.sum(mixed[N // 2:] ** 2) / np.sum(mixed ** 2)):7.1f} dB, "
              f"minimum-phase filter {fir_late(h, probe, m, fc):7.1f} dB")


if __name__ == "__main__":
    main()
