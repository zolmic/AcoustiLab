# AcoustiLab v2 spec — errata

These errata apply to `docs/spec/AcoustiLab_v2_spec.pdf` (dated 22 Sep 2026).
Each item was found in a review and then re-derived independently: numbers were
recomputed and formulas implemented and evaluated. The engine follows the
corrected statement. "p." is the PDF page.

## Formulas the engine must not copy

- **E1 — Appendix D, circular duct (p. 76). High severity.** With
  `k_v = sqrt(jωρ/μ)`, the function `2J1(z)/(zJ0(z))` gives exactly *negative*
  Poiseuille resistance and negative thermal loss. Use `2I1(z)/(zI0(z))` (modified
  Bessel), or keep J with argument `a·sqrt(−jωρ/μ)`. The slit form `tanh(z)/z` is
  correct.
- **E2 — Appendix D, "hydraulic radius r".** A single radius cannot serve both
  shapes. Slits need `z = k·h/2` (A/P); circles need `z = k·a` (2A/P). Using one
  convention for both puts the other shape's Poiseuille resistance off by 4×.
- **E3 — lumped-validity metric (pp. 8, 34, 75, 57).** Section 2 and Section 17
  use the input-compliance error `1 − kL·cot kL`. Section 9 uses the far-wall
  ratio `1/cos kL`. C8 attributes `(kL)²/3` and "3 dB at kL = 1" to `1/cos kL`,
  which actually gives 5.35 dB at kL = 1. As a result, the 3 dB point of a 60 mm
  cavity is 910 Hz on one page and 715 Hz on another. The engine uses
  `1 − kL·cot kL`: 3.0 / 8.5 / 35.8 % at kL 0.3 / 0.5 / 1.0, which is 0.27 / 0.77 /
  3.85 dB. The 10 % point is at 493 Hz for 60 mm.
- **E4 — continuity rule (p. 10).** "Under 0.1 dB below the lower level's 3 %
  frequency" cannot pass for L0→L1. The lumped model is about 0.27 dB off there
  by definition, and Section 17 itself allows 0.3 dB. Use 0.3 dB below the 3 %
  frequency, or 0.1 dB below kL ≈ 0.17.

## Reference data

- **E5 — Tymphany HPD-40N16PET00-32 (p. 16).** The datasheet is internally
  inconsistent. Bl, Re, Mms and fs give Qes = 0.864, not the stated 1.01, and
  Qts = 0.655, not 0.74. Mms and Cms (in mm/N) give fs = 84.95 Hz, not 81.8 Hz.
  The spec's own 5 % electrical-Q governance check would flag its reference
  record, and the Phase 1 "3 % in total Q" gate depends on which fields are
  primary. The engine treats fs, Qms, Qes, Re, Mms and Sd as primary, which
  derives Bl = 2.24 T·m.
- **E6 — Harman over-ear fixture (pp. 3, 27, 41).** The Harman around-ear and
  on-ear targets were defined on a GRAS 45CA with RA0045 couplers (IEC 60318-4)
  and Harman's custom pinnae (Olive & Clark 2025), not on IEC 60318-1. This also
  contradicts the spec's own 60318-4 row (p. 25).
- **E7 — Harman preference models (p. 42).**
  - The over-ear SD and |slope| are computed over 50 Hz–10 kHz, not 20 Hz–10 kHz.
    They use the sample SD (ddof = 1) and a regression slope on ln f.
  - The in-ear coefficients printed are from Harman's patent application
    US 2019/0087739. The set used by Listen/SoundCheck and AutoEq is
    100.0795 − 8.5·SD − 6.796·|AS| − 3.475·AME.
  - Name the source; the two sets differ by 10–30 points.

## Section 17 oracles that a correct solver fails

- **E8 — high-frequency slope.** For the 0.1 g example (fc ≈ 1204 Hz), the exact
  lumped slope over 4–16 kHz is −12.4 to −12.7 dB/oct, outside ±0.3 dB. Measure
  over f ≥ 8·fc, or compare against the exact second-order response.
- **E9 — leak slopes.** "+6 and +12 dB/oct within 0.3 dB over two octaves below
  the corner" fails for ideal first- and second-order high-passes, and the
  lossless inertive case is singular at the corner. Use a band ending at fc/8 or
  lower, or compare against `H = Z_L/(Z_L + 1/jωC)`.
- **E10 — pressure-chamber level.** With the L0 wall loss the spec requires, a
  correct solver sits 0.2 dB below ρc²·Sd·x/V at 20 Hz, against a 0.05 dB
  tolerance. Test with a lossless cavity, or include the wall-loss factor.
- **E11 — fill thermodynamics.** −2.92 dB and −0.83 dB hold only at constant
  volume velocity. At the constant voltage of the neighbouring tests the C2
  example gives about −0.59 dB and −0.15 dB. The drive must be stated.
- **E12 — sign test.** It cannot be evaluated at 0 Hz (DC is excluded, and the
  equalisation leak makes p → 0). Evaluate it at 20 Hz on a sealed netlist.
- **E13 — asymptote test.** For a sealed lumped cavity p/U falls 6 dB/oct, not
  12. The 12 dB/oct slope belongs to p/V above the coupled resonance.
- **E14 — two-port fallback.** The impedance-form fallback cannot rescue a
  half-wave line, because B and C vanish together. Stamp ABCD directly with
  branch unknowns.
- **E15 — sensitivities.** "Central finite differences reusing the
  factorisation" needs a named method: a Sherman–Morrison low-rank update, or
  forward sensitivities `A·dx/dp = −(dA/dp)·x`.
- **E16 — "every element value is positive".** This conflicts with modal areas
  that may be zero or negative, electrostatic negative stiffness and polarity.
  Restrict it to passive R, L and C, and test passivity on port impedances.
- **E17 — `Re(Zin) ≥ Re`.** This holds only for a single moving coil driven
  directly. The general condition is `Re(Zin) ≥ 0`.
- **E18 — creep.** The log-frequency compliance must be the causal complex form
  `C0·[1 − λ·log10(jω/ω0)]`. A real-only law is non-causal and goes negative for
  large λ.
- **E45 — slit worked examples.** They have no tolerances. The exact model puts
  the 1 mm slit peak at ≈523 Hz ("near 550"). The 0.2 mm slit's −3 dB point is
  73 Hz, against an RC corner of 83 Hz, with a 0.8 dB bump. Define the corner as
  `1/(2π·R_pois·C)` and state tolerances.
- **E46 — causality test.** The windows are undefined. An IR length of
  T60 ≈ 2.2·Q/f leaves the aliased tail around −40 to −60 dB. Size the buffer
  so that N/(2·fs) ≥ 2.9·Q/f.

## Terminology and consistency

- **E19 — PDR.** `Z_ear/(Z_ear + Z_hp)` is the headphone pressure division. The
  PDR of Møller et al. 1995 is `(Z_ear + Z_rad)/(Z_ear + Z_hp)`, which equals 1
  for an FEC headphone.
- **E20 — in-browser 3-D.** Section 9 (p. 36) allows it, but the non-goals and
  the roadmap forbid it. The 1.5 GB guard is also below the memory a 100k-DOF
  3-D factorisation needs.
- **E21 — ERP.** The scope statement lists it as an output; the glossary says it
  is not a headphone output.
- **E33 — cross-reference.** "Coupled resonance of Section 4" should read
  Section 5 / Appendix C2.
- **E32 — sensitivity conversion.** C7 uses Re; Sections 3 and 4 use rated
  impedance, which is correct.
- **E30 — grids.** 512 base points over 10 Hz–40 kHz is 42.7 per octave, while
  the exchange grid is 48 per octave (576 points). The engine defaults to 48 per
  octave.
- **E41 — copyright policy.** The CI self-tests need IEC 60318-4 Table 1
  verbatim, which conflicts with "never embed tables". Test vectors from paid
  standards live in an untracked `private/` directory, and those tests skip when
  it is absent.
- **E42 — Appendix B example netlist.**
  - The Leach coil values roll off the treble by about 6 dB at 10 kHz.
  - Several keys lack unit suffixes.
  - There is a single leak segment.
  - The front screen sits where it is inert.
  - The mechanical wiring only works under the across/through convention.

## Worked numbers

- **E22 — vent (p. 23).** A 2 mm hole in a 1.5 mm wall on 60 cm³, with both ends
  flanged, tunes to ≈221 Hz lossless (≈213 Hz thermoviscous), not 230 Hz.
  Including end resistance gives Q ≈ 8–9, not 13.
- **E23 — end-correction sentence (p. 23).** The direction is reversed.
  Including the corrections doubles the mass and lowers the resonance by ≈30 %.
- **E24 — explain-panel example (p. 52).** It is impossible. +10 % front volume
  can lower pressure-chamber SPL by at most 0.83 dB; the C2 example gives about
  0.15 dB at constant voltage.
- **E25 — mode lists (p. 34).** The box list omits 5.72, 6.87, 7.62 and
  8.14 kHz. The cylinder list omits m = 2 at 6.67 kHz and (1,0,1) at 7.95 kHz.
- **E26 — temperature.** From 20 to 37 °C, mode frequencies shift by 2.9 %, not
  2 %.
- **E27 — modal truncation.** It keeps about 1.4k modes up to 20 kHz and about
  10k up to 40 kHz.
- **E28 — ka = 1.** The limit is 2.7 kHz for a 40 mm geometric diameter and
  3.06 kHz for Sd = 10 cm².
- **E29 — cavity Q.** A wall-loss Q of 1/ε = 30–200 holds only in the bass; at
  mode frequencies it is 500–1000.
- **E31 — FEM pollution.** For quadratic elements the pollution term is
  `k·L·(kh)^4`, not `k³h²`.
- **E39 — excursion.** At 100 dB in a sealed 30–100 cm³ cup it is about
  0.4–1.4 µm RMS, not "a few micrometres".
- **E40 — C1 example.** 107.5 dB assumes 1 µm RMS.

## External facts

- **E34 — aluminium.** Aluminium and CCA wire have a temperature coefficient of
  about 0.0040/K (pure aluminium 0.0043/K), not 0.00377/K.
- **E35 — ConvolverNode.** WebKit and Gecko both move late IR partitions to a
  background thread; Chromium does not. The "Safari alone" claim is wrong.
- **E36 — ISO 11904-2.** Its conversion data cover only 20 Hz–10 kHz.
- **E37 — references.**
  - IEC 60318-7:2022 replaces TS 60318-7:2017.
  - AES2-2012 (r2023) supersedes AES2-1984.
  - The Hearpiece database is published in Acta Acustica 5, 2 (2021).
- **E38 — P.57 Type 3.2 leaks.** The slit dimensions are guidance only; the
  acoustic specification is normative, and the leaks are handset-derived.
- **E43 — companion service.** Browsers have no LAN-discovery API, and Local
  Network Access, mixed-content and TLS rules apply. Use a paired or user-entered
  endpoint with token authentication.
