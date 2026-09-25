# The over-ear template: sealed and vented cups

This note explains why `examples/design_over_ear.json` resonated near
0.94 kHz and lost about 20 dB between 1 and 4 kHz, which parameter values
caused it, and what the vented default that replaced them rests on. The
units and conventions follow `docs/conventions.md`. Levels are at the
template's drive (1 mW into the 32 ohm rated impedance) on the IEC 60318-4
ear, level 1, unless stated otherwise.

## Summary

- **The engine solved the netlist it was given.** The old template closed
  both sides of the diaphragm. The front was sealed by a 0.08 mm pad leak,
  which is resistive with a corner near 18 Hz. The rear was a 25 cm³ cavity
  whose only exit, a 3 mm hole under 160 rayl, is effectively closed above
  about 40 Hz. The air springs of the two volumes, 4.6 and 5.7 kN/m, swamp
  the suspension (79 N/m). They raise the driver's resonance from 81.8 Hz to
  the coupled resonance of spec Appendix C2: 936 Hz by hand, 930 Hz solved.
  Above it the mass-controlled diaphragm drives a compliance, so the drum
  pressure falls at 12 dB per octave.
- **That is the documented behaviour of a sealed headphone.** Matsushita's
  patent on sealed headphones (US 4,239,945, 1980) says the prior-art sealed
  headphone "exhibits a peak between 500 Hz and 1 KHz and it is difficult to
  reproduce higher frequencies". Drivers clamped on a small sealed coupler
  show the same physics at a higher frequency: a resonance near 3 kHz,
  against 80 Hz in free air.
- **Published closed over-ears do not behave like that.** In every
  published fixture curve we found for closed over-ears (12 entries, table
  below), the low-frequency impedance maximum lies at about 25 to 95 Hz.
  The peak is 1.08 to 1.81 times the mid-band impedance. None has a coupled
  resonance in the hundreds of hertz. For the one headphone measured both ways, the fixture
  lowered the peak from about 62 Hz in free air to about 55 Hz. A sealed cup
  cannot do that with a realistic volume: its resonance is
  fs·sqrt(1 + Vas/V_front + Vas/V_rear), with Vas = 1.79 L here.
- **They are vented.** "Normally leaks are desirable and a deliberate part
  of the acoustical design" (Poldy 2006). Meshed paths join the front and
  rear volumes and lead to the outside. The literature calls this resistive
  coupling (Shaw and Thiessen 1962; Görike, US 4,389,542; Sennheiser,
  US 6,934,401; Chen et al. 2019).
- **Diagnosis: parameter values, not missing element physics.** The `vent`
  and `mesh` elements already express a front-to-rear vent. The cause is
  that the template gave the cup no designed leak.
- **New default:** four 4 mm holes in the 2 mm driver baffle, each under a
  65 rayl mesh (Saati Acoustex 065).
  - The fixture impedance maximum moves to 82 Hz, 1.11 times its mid-band
    minimum, and the impedance only falls from 300 Hz to 3 kHz.
  - The diaphragm is resistance-controlled in the bass.
  - The drum response stays within 1 dB of its 500 Hz level from 100 Hz to
    1.5 kHz.
  - `baffle_vent_count = 0` restores the previous template: its solved
    curves are identical byte for byte (CSV output).
- **What remains in 2–8 kHz:** the drum response there is still 12.6 dB
  below its 500 Hz level, against 18.5 dB before. Scored against
  `ravizza2023_5128`, the band's mean error goes from −28.1 to −22.2 dB.
  - About 9.5 dB of the remaining error is the target's own rise over its
    500 Hz level: the ear gain of the 5128's pinna, concha and canal. The
    bare simulator's short canal supplies only 1 to 5 dB of such gain, and
    that is already inside the 12.6 dB (fixture mismatch,
    `docs/targets.md`).
  - The other 12.6 dB is the mass-controlled fall above the vents'
    Helmholtz frequency, which bounds any lumped cup with this 0.3 g driver
    (see "What is left", below).
- **Spec.** The spec's pp. 34–35 place the coupled resonance of circumaural
  cups at "several hundred hertz to about a kilohertz". They also label the
  fall above it "a model artefact". Erratum E53 corrects both.

## Reproducing

```sh
cargo run -p acoustilab-cli --release -- score examples/design_over_ear.json --target ravizza2023_5128
cargo run -p acoustilab-cli --release -- score examples/design_over_ear.json --target ravizza2023_5128 --set baffle_vent_count=0
cargo run -p acoustilab-cli --release -- solve examples/design_over_ear.json --csv --set baffle_vent_count=0
python3 tools/over_ear/lumped_check.py    # independent lumped model against the engine
```

| | sealed (`baffle_vent_count = 0`) | vented (default) |
|---|---|---|
| impedance maximum below 3 kHz | 69.9 Ω at 930 Hz | 36.6 Ω at 82 Hz |
| peak ÷ minimum up to 3 kHz | 2.13 | 1.11 |
| drum level at 500 Hz, 1 mW | 111.7 dB | 106.8 dB |
| drum re 500 Hz: 100 Hz / 1 / 2 / 4 / 8 kHz | −2.6 / +10.6 / −12.4 / −22.0 / −18.9 dB | +0.9 / +0.3 / −4.7 / −16.4 / −14.1 dB |
| drum re 500 Hz, mean over 2–8 kHz (log-spaced) | −18.5 dB | −12.6 dB |
| score vs `ravizza2023_5128`: 20–200 Hz mean error | −5.2 dB | −1.7 dB |
| 200 Hz–2 kHz RMS error | 6.3 dB | 2.6 dB |
| 2–8 kHz mean error | −28.1 dB | −22.2 dB |
| lowest resonant drum pole (vector fit, `acoustilab poles`) | 936 Hz | 1.53 kHz, Q = 1.45 |
| diaphragm excursion at 20 Hz, 10 mW (rated power) | 0.007 mm | 0.19 mm (Xmax 0.8 mm) |

Mean errors are reported by `acoustilab score`; drum ratios are
interpolated on the 24-per-octave sweep. With `ear=type43` the vented
default scores −23.7 dB in 2–8 kHz, and the sealed cup −29.6 dB.

## Published behaviour of closed over-ears

### Impedance on fixtures

Collected on 2026-09-25. "Text" means the number is stated in the source.
"Plot" means it was read from the published graph. "Canvas" means it was
read from the drawing coordinates of Reference Audio Analyzer's interactive
plot (about 3 % in frequency, 0.3 dB in level). The ratio is the peak over
the mid-band minimum.

| headphone | source | fixture | low-frequency maximum | ratio | other features |
|---|---|---|---|---|---|
| "Headphone 1", closed circumaural, 63 Ω | Audio Precision app note (2017), Fig. 17 (plot) | ATF / free air | ≈55 Hz, ≈110 Ω / ≈62 Hz, ≈113 Ω | ≈1.45 | ≈3.1 kHz, ≈88 Ω in both conditions |
| Sony MDR-7506 | ASR, 2021-01-05 (plot); RAA (canvas) | GRAS 45C; RAA HDM-X | ≈65 Hz, ≈105 Ω; 62 Hz, 116.7 Ω | 1.4–1.55 | ≈3.2 kHz, ≈85 Ω |
| Audio-Technica ATH-M50x | RAA (canvas); ASR 2021-03-02 ("Fixed 39 ohm", text) | HDM-X; GRAS 45CA | 48 Hz, 39.7 Ω | 1.08 | +1.6 Ω at ≈3.9 kHz |
| Audio-Technica ATH-M40x | RAA (canvas) | HDM-X | 75–84 Hz, 44.2 Ω | 1.18 | +1.7 Ω at ≈3.7 kHz |
| Beyerdynamic DT 770 PRO 80 | RAA (canvas) | HDM-X | 84–93 Hz, 108.5 Ω | 1.26 | none |
| Beyerdynamic DT 770 PRO 250 | RAA (canvas) | HDM-X | 54–60 Hz, 293 Ω | 1.24 | none |
| Beyerdynamic DT 770 Studio | SoundStage! Solo, Aug 2020 (plot) | GRAS 43AG, RA0402, KB5000 | ≈85–90 Hz, ≈105 Ω | ≈1.2 | none |
| Sennheiser HD 280 PRO | RAA (canvas) | HDM-X | 84 Hz, 99.9 Ω | 1.74 | +2.8 Ω at ≈4.2 kHz |
| AKG K371 | SoundStage! Solo, Nov 2019 ("running between 35 and 41", text, the unit misprinted as Hz); ASR 2021-01-23 (plot) | GRAS 43AG; GRAS 45CA | ≈25 Hz | 1.08–1.14 | ≈3.5 and 4.3 kHz, ≈+1 Ω |
| Focal Elegia | SoundStage! Solo, Oct 2018 ("57 ohms peak at the 70Hz system resonance", text); ASR 2021-11-10 (cursor 93.9 Hz, 52.5 Ω) | GRAS 43AG; GRAS 45CA | 70–94 Hz | 1.6–1.7 | ≈3 and 4.5 kHz, ≈+1 Ω |
| Focal Stellia | SoundStage! Solo, Feb 2019 ("as high as 56 and as low as 31 ohms", text) | GRAS 43AG | ≈80 Hz (plot) | 1.81 | ≈3 and 4.5 kHz |
| Sony MDR-Z7 | RAA (canvas) | HDM-X | 35 Hz, 81.8 Ω | 1.09 | +2 Ω at ≈870 Hz |
| Sennheiser HD 25 (on-ear, for contrast) | RAA (canvas) | HDM-X | 116 Hz, 82.9 Ω | 1.15 | +3.8 Ω at ≈4 kHz |

- **Sources.** ASR is audiosciencereview.com. RAA is
  reference-audio-analyzer.pro; it does not state where the headphone sits
  during its impedance sweep. SoundStage! Solo is soundstagenetwork.com.
- **Butterworth on the Elegia.** SoundStage's reviewer calls its curve
  "typical for closed-back, dynamic-driver headphones".
- **The Audio Precision app note.** It measured "Headphone 1" on an ATF and
  in free air, "to illustrate the effect of the acoustic loading". The ATF
  lowered and slightly damped the peak; it did not raise it.
- **Other features.** The 3–4.5 kHz features appear in free air too, so
  they belong to the driver or cup, not to coupling with the ear. Only two
  models show anything between 300 Hz and 3 kHz, of 1 to 2 Ω.
- **Sensitivity.** SoundStage's sensitivities (300 Hz–3 kHz, 1 mW, 43AG with
  pinnae) are 95.8 dB (DT 770 Studio), 104.0 dB (Stellia) and 106.7 dB
  (K371). The sealed template gave 111.7 dB at 500 Hz on a bare simulator.

### What a sealed cup does

- **Matsushita, US 4,239,945, "Sealed headphone" (filed 1977, published
  1980).** "Since the prior art sealed headphone uses a compliance control
  region, it can exhibit a flat playback characteristic in a low frequency
  range, but it exhibits a peak between 500 Hz and 1 KHz and it is difficult
  to reproduce higher frequencies". The patent adds, "the upper limit
  frequency fH for reproduced sound has heretofore been limited to below
  1 KHz". Its remedy is a coupling aperture whose acoustic mass is chosen
  "such that fp is approximately 5 KHz".
- **Görike (AKG), US 4,389,542, "Orthodynamic headphone" (1983).** "If a
  headphone with the dynamic transducer systems and diaphragms having
  minimum masses is operated with a tightly closed coupling space, the
  restoring force of the air enclosed in the coupling space causes the
  diaphragm resonance to rise to between 1,500 to 4,000 Hz, regardless of
  the fundamental resonance of the diaphragm which may range between 70
  and 250 Hz".
- **Ole Wolff OWR-4009T-38E (rev 01, 2022) and OWR-4007T-32C (rev 01,
  2021).** These are 40 mm drivers measured on a GRAS 43AA (IEC 60318-1)
  with a flat-plate adaptor, so the coupler volume is a few cm³. The sheets
  state a "resonance frequency on IEC318 coupler" of 3.0 and 3.1 kHz
  ±15 %, against 80 Hz in free field for the first. The coupler responses
  are flat below about 300 Hz, peak near 3 kHz, and are 11 to 16 dB below
  their 1 kHz level at 8 kHz (read from the plots).
- **The lumped model agrees.** A sealed cup follows it, and the fall above
  the coupled resonance is physics, not an artefact of lumping (E53).

### How commercial designs avoid it

- **Poldy, "Headphone fundamentals", AES 120th Convention tutorial, Paris,
  2006, p. 5.** Uncontrolled leaks go "through hair and/or through porous
  cushions", while "Others are led in a controlled way to the outside: (c)
  or via the vented rear cavity (d). Normally leaks are desirable and a
  deliberate part of the acoustical design. As a result of controlled leaks
  the SPL in the cavity becomes more stable with respect to any additional
  chance leaks". On the working principle: "Membrane movement is largely
  stiffness controlled C or resistance controlled (damping R), but usually
  a combination of C and R. This is unlike loudspeakers, which are
  predominantly mass controlled". His Figure 1 pairs "Constant SPL in real
  headphone" with "Constant velocity (sometimes)".
- **Shaw and Thiessen, "Acoustics of circumaural earphones", JASA 34,
  1233–1246 (1962).** The paper covers earphones in noise-excluding cups,
  and only its abstract was available: "the preferred
  system being a rigid structure with resistive coupling between the two
  cavities. This system has maximum effective coupling volume at low
  frequencies ... and a smaller effective volume at high frequencies
  affording increased earphone response".
- **Görike, US 4,389,542.** "By closing the coupling space with an acoustic
  resistance on the order of magnitude of the wave impedance of air, a
  constant sound pressure can be produced in the coupling space". The
  diaphragm "oscillates at a constant velocity", so that "the oscillations
  of the diaphragm are predominantly impeded by friction and the
  requirement of a critical damping of the low frequency resonance of the
  diaphragm (70 to 300 Hz) is satisfied".
- **Grell and Kaddig (Sennheiser), US 6,934,401 B2, "Closed headphones with
  transducer system" (2005).** "a flow resistance 11 is provided between the
  front volume and the rear volume so that air (when there is overpressure
  relative to the front volume) can flow out of the rear volume into the
  front volume". On drivers generally: "In known transducer systems, the
  natural resonance of the electrodynamic transducer is damped by a body
  functioning as a flow resistance". That body leaves a gap of "about
  0.1 mm to 5 mm" behind the diaphragm.
- **Chen, Li, Wu and Liu, "Study on the Acoustic Characteristics of
  Headphone with ECM Simulation and Reverse Engineering", SMONT 2019,
  Adv. Intell. Syst. Res. 165, pp. 34–37 (CC BY-NC).** This is a DENON
  monitor headphone with 40 mm drivers.
  - Its equivalent circuit has a driver rear chamber, holes and "acoustic
    paper" to the cup, small holes to the outside, and a cushion volume
    with a leak branch.
  - It traces its low-frequency loss to "the ventilation materials of the
    front frame connecting the rear and front chambers".
  - Its Fig. V gives the front frame's holes: 24 of 1.875 mm and 4 of
    1 mm, each 1 mm deep, about 69 mm² in all.
  - Its driver, measured in vacuum with a Klippel LPM, has Mms = 0.056 g,
    Cms = 6.65 mm/N, Bl = 1.645 T·m, Re = 33.4 Ω and fs = 259.5 Hz
    (Fig. III), and Sd = 6.16 cm² (Fig. V).
  - Its plotted impedance moves from a sharp peak near 200 Hz in air to a
    heavily damped one near 100 Hz in the headphone on a HATS (read from
    the figures).

## Why the sealed template resonates at 0.94 kHz

With the across/through convention, the diaphragm's mechanical impedance is
Zm = jωMms + Rms + 1/(jωCms) + Sd²·Za, where Za is the acoustic impedance of
its loads. A closed volume V is a compliance V/(γP0). Behind a sealed cup
the stiffness is

```
k = 1/Cms + γP0·Sd²·(1/V_f + 1/V_r) = (1/Cms)·(1 + Vas/V_f + Vas/V_r),   Vas = γP0·Sd²·Cms
f_c = fs·sqrt(1 + Vas/V_f + Vas/V_r)
```

- **Inputs.** The driver's primary set gives Cms = 12.62 mm/N, so
  Vas = 1.79 L. The front volume is the 29.45 cm³ cavity plus the
  simulator's 1.26 cm³. The rear is 25 cm³.
- **Result.** f_c = 81.8 Hz × 11.4 = 936 Hz. The solved impedance maximum
  is at 930 Hz; test `sealed_cup_resonates_where_the_air_springs_put_it`
  holds it to 2 %.
- **Q.** The solved peak height, 69.9 Ω, puts the mechanical resistance
  at Bl²/(69.9 − 32.8) = 0.135 N·s/m: Rms plus 0.078 N·s/m of acoustic
  loss. With the electrical damping Bl²/Re = 0.153 N·s/m this gives
  Q = sqrt(k·Mms)/0.29 ≈ 6. The drum response rises 13 dB above its 500 Hz
  level near 930 Hz.
- **The general bound.** For the peak to stay within 10 % of fs, as the
  fixture data show, Vas/V_f + Vas/V_r must stay below about 0.2. That
  means effective volumes of about 18 L on both sides. So on a fixture the
  paths out of a real cup's volumes must have impedance small compared with
  1/(ωC) at the resonance: leaks and vents.

Above f_c the diaphragm is mass-controlled: v = Bl·i/(jωMms), and the
front pressure is U/(jωC_f). This gives the mass line

```
|p_front| ≈ γP0·Sd·Bl·|i| / (ω²·Mms·V_f)
```

At 1 mW (|i| ≈ 5.5 mA above resonance) and V_f = 30.7 cm³ it is 107.5,
95.5, 88.5, 83.5 and 71.4 dB at 1, 2, 3, 4 and 8 kHz.

## The vented default

### Network

The template adds the element `baffle_vent`, a `vent` from `a_front` to
`a_rear`. Its keys are `baffle_vent_count`, `baffle_vent_diameter_mm`,
`baffle_vent_length_mm` (the baffle thickness) and `baffle_vent_mesh_rayl`.
The vent is a thermoviscous tube with a flanged inner end and a mesh; the
baffled-piston radiation reactance supplies the outer end correction. For
the defaults:

- the mesh gives R_b = R_s/A = 65 rayl / 50.3 mm² = 1.29 MPa·s/m³;
- the holes' air mass is M_b = ρ·(L + 2·0.82a)/A = 125 kg/m⁴.

Above about 40 Hz the pad leak and the rear vent carry little flow. The cup
then holds a fixed amount of air, so C_f·p_front + C_r·p_rear = 0. The
diaphragm pumps U from the rear node into the front node and sees

```
Za = (p_front − p_rear)/U = Z_b ∥ 1/(jωC_s),   Z_b = R_b + jωM_b,   C_s = C_f·C_r/(C_f + C_r)
p_front = (C_r/(C_f + C_r))·(p_front − p_rear)
```

### Three regimes

1. **Resistance control, up to about 0.7 kHz.** Below 1/(2π·R_b·C_s) =
   1.27 kHz the mesh short-circuits the air springs. The diaphragm sees
   Sd²·R_b = 1.29 N·s/m against Rms = 0.057 N·s/m and ωMms (equal at
   0.69 kHz). The velocity is Bl·i/(Sd²·R_b + Rms), and
   `p_front = (F/Sd)·(C_r/(C_f + C_r))·Sd²R_b/(Sd²R_b + Rms)` with F = Bl·i.
   The expression contains no frequency; this is Görike's constant pressure
   under an acoustic resistance. It gives 107.4 dB at 100 Hz against 107.7 dB
   solved (test `vented_default_is_flat_to_the_baffle_helmholtz_frequency`).
   The motional impedance Zin − Re is nearly real from 50 to 300 Hz: its
   phase is +7°, −2° and −18° at 50, 100 and 300 Hz. No air spring raises the
   resonance, so the impedance maximum lies near the suspension resonance
   (82 Hz, 1.11 times the mid-band minimum). This is the heavily damped end
   of the published range, next to the ATH-M50x and K371.
2. **Mass control into the vents' mass, 0.7 to 1.5 kHz.** The diaphragm's
   mass takes over from the mesh. Meanwhile the vents' air mass resonates
   with the two volumes in series at f_H = 1/(2π·sqrt(M_b·C_s)) = 1.44 kHz.
   The vector fit finds this as the drum pole at 1.53 kHz with Q = 1.45. The
   pole attribution gives d ln f/d ln p = +0.79 for the vent diameter,
   −0.50 for the cup radius and −0.34 for the rear volume. The load's rise
   towards f_H offsets the mass roll-off. The drum response stays within
   −0.1 to +0.9 dB of its 500 Hz level from 100 Hz to 1.5 kHz, and within
   1.5 dB from 50 Hz (same test).
3. **Above f_H.** The vents are blocked by their own mass, the volumes act as
   separate springs, and the pressure joins the sealed cup's mass line. The
   two configurations are 1.3 dB apart at 3.6 kHz (level 0). The vented cup
   does not raise the treble. It gives up the pressure-chamber gain below
   f_c, so its treble sits nearer its 500 Hz level.

`tools/over_ear/lumped_check.py` re-implements this network with numpy and
compares it with the engine at level 0. It uses Poiseuille slits, flanged
end corrections and the driver's primary set, and shares no code with the
engine. From 30 Hz to 3.6 kHz the two agree to 0.16 dB and 0.02 Ω for the
vented default. For the sealed cup they agree to 0.24 dB and 0.04 Ω away
from the peak; there the simple model lacks the wall loss and vent air mass.

### Choice of values

| parameter | value | provenance |
|---|---|---|
| `baffle_vent_mesh_rayl` | 65 | Saati Acoustex 065, 65 MKS rayl, 30 µm pores, 24 % open area (Saati technical datasheet ADS1200019EN V8, 2015-09-29, which lists headphones among the applications); tolerance ±12 %, industry practice as for the rear vent |
| `baffle_vent_count` | 4 | design choice; see below |
| `baffle_vent_diameter_mm` | 4 | design choice; ±0.05 mm moulding estimate. The total, 50 mm², is of the order of the 69 mm² of front-frame holes that join the front and rear chambers of the DENON headphone in Chen et al. (Fig. V) |
| `baffle_vent_length_mm` | 2 | the baffle thickness, as for the rear vent |

The count, diameter and mesh were chosen together. The criteria come from
the data above:

- the fixture impedance maximum lies in the published 25–95 Hz band, with a
  ratio inside 1.08–1.81;
- the impedance has no maximum between 300 Hz and 3 kHz;
- the drum response is flat to about 1.5 kHz.

They are not a measured product. A grid over 2 to 6 holes of 2 to 4 mm under
0 to 160 rayl gives these results:

- **Open holes (0 rayl)** keep a sharp peak at 42 to 71 Hz, 2.2 to 3.5
  times Re.
- **The 160 rayl grade of the rear vent** leaves the coupled resonance at
  0.82 to 0.95 kHz.
- **Grades of 45 to 65 rayl on 50 mm² of holes** give the published
  pattern. At 80 rayl the maximum moves to 100 Hz.

Tolerances:

- **Mesh, ±12 %** (57 or 73 rayl): the maximum moves to 77 or 89 Hz, and
  the drum response stays within −0.7 to +1.3 dB of its 500 Hz level from
  100 Hz to 1.5 kHz.
- **Hole diameter, ±0.05 mm:** the maximum stays at 82 Hz, and the flat
  band moves by 0.1 dB.
- **One hole fewer or more:** the maximum moves to 94 or 79 Hz, and the
  flat band is within 1.8 dB.

The tests fix the criteria, not the values.

## What is left in the 2–8 kHz band

Above f_H every lumped cup with this driver runs along the mass line. Its
drum response relative to 500 Hz is therefore set by how far the 500 Hz
level sits above the mass line. Only three things can lift the 2–8 kHz
band relative to 500 Hz within a lumped model:

- a lower 500 Hz level, at the cost of sensitivity;
- a lighter diaphragm or stronger motor, since the line scales with
  Bl/Mms;
- a smaller front volume.

The template's driver is heavy for its area: 0.3 g for 10 cm², against
0.056 g for 6.2 cm² in Chen et al.'s commercial 40 mm driver.

Real products measured on a head-and-torso simulator reach target-like
levels in this band. The AP app note's "Headphone 1" is 13 dB above its
500 Hz level at 2.8 kHz on its ATF, and the target itself is 6 to 12 dB
above. The template leaves out, or does not use, the following:

- **Pinna and concha.** These are absent from the engine. They cause the
  fixture mismatch against a 5128 target, which is about 9.5 dB of the
  remaining error (`docs/targets.md`, "The fixture rule").
- **The full canal of a head-and-torso simulator.** It shows a quarter-wave
  gain under a large cup. `ear=type43` includes the canal from its
  reference plane; the IEC 60318-4 includes only the part an insert
  earphone leaves.
- **Cup cross-modes.** They begin at about 4 kHz in a 50 mm cup; the spec's
  p. 34 lists them. The template's cavity is a depth line (L1); the
  `modal_cavity` element exists but is not used.
- **Diaphragm modes.** The D2 dome and surround model exists but has no
  data for this driver.

This part of the shortfall is therefore justified as a modelling limit, not
fixed. No public measurement combines a known driver, a known cup and a
bare-simulator response that could calibrate it. The spec's open reference
headphone (Section 17) would.

**The driver record.**

- **The plots disagree with the printed parameters.** The 2016-12-09 sheet
  of the same driver (Peerless by Tymphany, HPD-40N16PET00-32, Rev 1) plots
  its free-air impedance with a maximum of about 66 Ω near 105 Hz. The
  printed primary set implies about 121 Ω at 81.8 Hz. Its plotted on-axis
  SPL at 2.83 V, 1 m is about 82–83 dB from 500 Hz to 1 kHz, and it prints
  83.5 dB. The primary set implies about 76 dB, and the 2018 sheet prints
  74.18 dB. Both discrepancies point to a lighter or more damped diaphragm
  than the primary set.
- **Scale of the effect.** An Mms of 0.18 g would lift the mass line by
  4.4 dB and move the sealed cup's resonance to 1.2 kHz.
- **Left as it is.** The record keeps the 2018 primary set (erratum E5); this
  discrepancy is an open item.

## Not missing physics, and what the spec says

- **The elements are sufficient.** Front-to-rear vents with meshes, pad
  leaks, rear vents and cavities are all existing elements, and their
  closed forms are tested in `tests/vents_and_leaks.rs` and
  `tests/materials.rs`.
- **Nothing in the families changes.** The 1-D and lumped cup model is
  adequate up to f_H once the leak paths are in the netlist. What it cannot
  represent above that is listed in the previous section.
- **Damping screens behind the diaphragm.** A driver's internal back
  chamber and damping screen can be built today from `cavity` and `mesh`.
  - Adding one to the sealed cup (1.5 cm³ behind a 0.5 cm², 1000 rayl
    screen) raises the diaphragm's stiffness above the screen's corner.
    From 1 to 8 kHz the drum response then stays within −2.4 to +8.5 dB of
    its 500 Hz level, which is 94.8 dB/mW. Below 100 Hz it rises by
    9–11 dB, and its impedance maximum moves to 2.8 kHz.
  - That maximum resembles the 3–4.5 kHz features of the published curves,
    but whether those features come from such a chamber is not
    established.
  - No published values tie such a chamber to this driver, so the template
    does not include one.
- **The spec's claims.** Section 9 (pp. 34–35) treats the coupled resonance
  of "several hundred hertz to about a kilohertz" as typical of circumaural
  cups, and the fall above it as "a model artefact". Erratum E53 corrects
  both: the fall is the physics of a sealed cup, and published closed
  over-ears avoid the resonance by venting.

## Verification

| check | where | tolerance |
|---|---|---|
| sealed cup: impedance peak at the Appendix C2 frequency; drum peak between 500 Hz and 1 kHz, 10 dB over 500 Hz; −20 dB at 4 kHz | `tests/over_ear_template.rs` | 2 % |
| vented default: impedance maximum in 25–95 Hz, peak/minimum in 1.08–1.81, no rise between 300 Hz and 3 kHz, motional phase within 25° at 50–300 Hz | same | as stated |
| vented default: resistive-coupling level at 100 Hz; flat 50 Hz–1.5 kHz; 2–8 kHz band > 5 dB above the sealed cup's relative to 500 Hz | same | 0.5 dB; 1.5 dB |
| `baffle_vent_count = 0` reproduces the previous template | CSV output of both, both ear loads, compared byte for byte (this review) | exact |
| independent lumped model | `tools/over_ear/lumped_check.py` | 0.3 dB, 0.1 Ω |

The fitting, isolation and time-domain worked examples that were built on
the sealed cup are pinned to `baffle_vent_count = 0`. These are the fit
case study in `docs/fitting.md`, the leak–vent cancellation in
`docs/isolation.md`, and the figures in `docs/time-domain.md`.

## Sources

Accessed 2026-09-25 unless stated otherwise.

- C. Poldy, "Headphone fundamentals", tutorial, AES 120th Convention,
  Paris, May 2006 (text mirrored at pubhtml5.com/twal/ddtl/basic/).
- E. A. G. Shaw and G. J. Thiessen, "Acoustics of circumaural earphones",
  J. Acoust. Soc. Am. 34(9A), 1233–1246 (1962), doi:10.1121/1.1918311
  (abstract).
- N. Atoji, S. Kusomoto, K. Sato (Matsushita), US 4,239,945, "Sealed
  headphone", 1980.
- R. Görike (AKG), US 4,389,542, "Orthodynamic headphone", 1983.
- A. Grell, H. Kaddig (Sennheiser), US 6,934,401 B2, "Closed headphones
  with transducer system", 2005.
- H. W. Chen, S. G. Li, S. Wu, Y. C. Liu, "Study on the Acoustic
  Characteristics of Headphone with ECM Simulation and Reverse
  Engineering", SMONT 2019, Advances in Intelligent Systems Research 165,
  pp. 34–37, https://www.atlantis-press.com/article/55917611.pdf.
- Audio Precision, "Headphone Electroacoustic Measurements" application
  note, June 2017, Figs. 11 and 17,
  https://www.elektronikfokus.dk/wp-content/uploads/sites/5/Audio-Precision-AppNote-Headphone-EA-Measurements-0617.pdf.
- SoundStage! Solo measurements (B. Butterworth): Focal Elegia (Oct 2018),
  Focal Stellia (Feb 2019), AKG K371 (Nov 2019), Beyerdynamic DT 770 Studio
  (Aug 2020), soundstagenetwork.com.
- Audio Science Review headphone reviews: Sony MDR-7506 (2021-01-05),
  ATH-M50x (2021-03-02), AKG K371 (2021-01-23), Focal Elegia (2021-11-10),
  audiosciencereview.com.
- Reference Audio Analyzer headphone reports, reference-audio-analyzer.pro
  (undated pages).
- Ole Wolff OWR-4009T-38E (rev 01, 2022-03-22) and OWR-4007T-32C (rev 01,
  2021-11-04) product specifications, media.digikey.com.
- Peerless by Tymphany HPD-40N16PET00-32 driver specification sheet,
  2016-12-09, and Tymphany's sheet of 2018-07-11 (the record's source).
- Saati, Saatifil Acoustex technical data sheet ADS1200019EN V8,
  2015-09-29.
