# Ear loads: canal, eardrum and ear simulators

This note covers the ear-side elements of spec Section 7: the conical
`canal`, the `eardrum` terminations, and the `iec60318_4`, `type33` and
`type43` macros. It records where every number comes from, what was fitted
and to what, and how each model was checked. The code is
`crates/acoustilab/src/elements/ear.rs`, the data is `data/ear/*.json`, the
generator and fit scripts are in `tools/ear/`, and the tests are in
`crates/acoustilab/tests/ear.rs`.

Conventions follow `docs/conventions.md`: `e^{+jωt}`, RMS phasors,
pressure and volume velocity as across and through. Effective volumes are
`γP0 / (ω·|Z|)` with γP0 = 1.4 × 101 325 Pa. That is the adiabatic
compliance at the standards' reference conditions (23 °C, 101.325 kPa).

## Summary

| Element | Construction | Parameters from | Validity reported |
|---|---|---|---|
| `canal` | Chain of truncated cones cascaded into one two-port | User area function | First transverse mode and Stinson bound at the widest section (L1); lumped kl (L0) |
| `eardrum` `hudde_engel` | Hudde & Engel (1998) drum, ossicles, cochlea and middle-ear cavities | COMSOL guide 6.4, Table 2-8 | Defined to 16 kHz |
| `eardrum` `type43` | Two-branch drum in series with a middle-ear cavity | Fitted to ITU-T P.57 Table 5-c | Fitted from 20 Hz to 20 kHz |
| `eardrum` `iec60318_4` | The coupler's two Helmholtz branches and its microphone lumped at one plane | Luan et al. 2019 | Human-valid to 10 kHz |
| `iec60318_4` | Main cavity in three sections, two shunt Helmholtz branches, microphone | Luan et al. 2019, plus one fitted side-volume scale | Human-valid from 100 Hz to 10 kHz, coupler-only above; Stinson bound 19.1 kHz |
| `type33` | 10 mm × 7.5 mm extension in front of `iec60318_4` | P.57 clause 6.3.1 | As `iec60318_4` |
| `type43` | P.57 canal (conical chain), fitted drum at the DRP, tip stub | P.57 Table 6 and Annex B; fit to Table 5-c | 20 Hz to 20 kHz by P.57; line bound at the widest modelled section (19.2 kHz from the reference plane) |

## Canal: conical transfer-matrix chain

### Segment matrix

With `q = x·p`, where x is the distance from the cone's apex, the horn equation
for a cone filled with a uniform medium becomes `q'' = Γ²q`. It integrates
exactly (Mapes-Riordan, JAES 41(6) 1993; Chaigne & Kergomard, *Acoustics of
Musical Instruments*, 2016, Ch. 7). Written without apex distances, so that
a cylinder needs no special case, with `u = ΓL`, `Z0 = sqrt(ρ_eff·K_eff)`
and end radii r1, r2:

```
A = (r2/r1)·cosh u − ((r2−r1)/r1)·sinh(u)/u
B = Z0·sinh(u) / (π r1 r2)
C = (π r1 r2 / Z0)·[ sinh u + ((r2−r1)²/(r1 r2))·(u cosh u − sinh u)/u² ]
D = (r1/r2)·cosh u + ((r2−r1)/r2)·sinh(u)/u
```

`(u cosh u − sinh u)/u²` is evaluated by its Taylor series below |u| = 0.1,
where the direct form cancels. Γ and Z0 come from the engine's circular-duct
medium (modified Bessel form, erratum E1), evaluated at the segment's mean
radius.

The segment is exact for a cone with a uniform effective medium. The only
approximation is that the wall loss varies with the radius inside a segment.
The error from that is second order in the segment length.

### Segmentation

Every knot of the area function is a segment boundary. Interval i is split
into `⌈|span_i| / (L/N) − 1e-9⌉` equal parts (at least one), where L is the
total length and N the requested count (`segments`, default 40). The count
is therefore about N, and never fewer than the number of intervals. Between
knots the radius is linear.

### Checks (tests `det_t_is_one_for_every_cone` to `conical_chain_converges_to_the_lossy_horn_equation`)

- **det T = 1** to 1e-12 for lossy and lossless cones, 5 Hz to 40 kHz.
- **N identical cylinders give one uniform tube**: they reproduce the engine's
  thermoviscous tube to rounding (1e-12·N, normalised entries).
- **Lossless cone against the textbook apex-distance form** (a different
  algebraic form, computed in Python): agreement to 1e-11.
- **Subdividing a lossless cone** leaves the matrix unchanged to 1e-10.
- **Convergence against the lossy horn equation** with the medium taken at
  the *local* radius, integrated numerically (scipy DOP853, rtol 1e-11).
  The table gives the maximum normalised entry error (B/Z0, C·Z0):

  | N | 2→4 mm cone, 10 mm, 10 kHz | P.57 canal ref→DRP, 10 kHz |
  |---|---|---|
  | 10 | 6.5e-5 | 6.2e-6 |
  | 20 | 1.6e-5 | 1.7e-6 |
  | 40 | 4.1e-6 | 5.2e-7 |
  | 80 | 2.6e-7 | 1.5e-7 |

  Across 20 Hz to 20 kHz the observed order is 2.0 for the cone. For the
  P.57 canal it is 1.8–2.0, because the per-interval segment counts are
  integers and do not double exactly. The test requires an error below
  5e-6 at N = 40 and an order between 1.6 and 2.4 between N = 20 and 80.
- **Exponential profile against the closed-form Webster horn**, lossless,
  S = S0·e^{2mx}, 30 → 60 mm² over 25 mm, cut-off 757 Hz. The cone chain
  converges as N^-2. At N = 40 the error is 1.3e-5 above cut-off and 8e-7
  at 100 Hz; the tolerance is 5e-5.
- **Low-frequency compliance** of a closed canal equals the frustum volume
  over γP0, to 1e-6.

### L0 and validity

At `level: 0` the canal is one T-section: series `Z/2`, shunt `Y`, series
`Z/2`, with `Z = jω Σ ρ_eff L/(π r1 r2)` and `Y = jω Σ V_frustum/K_eff`. Below
kL ≈ 0.17 it matches L1 within 0.1 dB (test
`lumped_canal_joins_the_line_at_low_frequency`).

At L1 the canal reports two limits at its widest section: the first
transverse mode, 1.8412c/(2πa), and Stinson's bound r·f^1.5 < 1e6 (cm, Hz).

Cross-section shape is not modelled; losses use the circle of equal area.
The P.57 polygons have perimeters 2–4 % above the equal-area circle along
the canal and 12–16 % above it in the last 6 mm before the tip. The boundary
loss is proportional to perimeter, so this model underestimates the wall
loss by that fraction. A straight axis is also assumed. The curvature
correction of Xia et al. (JASA 155(1), 2024) is not implemented.

## Eardrum terminations

### Hudde & Engel (`model: "hudde_engel"`, the default)

**Source.** H. Hudde and A. Engel, "Measuring and modeling basic properties of
the human middle ear and ear canal", Parts I–III, *ACUSTICA – acta acustica*
84 (1998) 720–738, 894–913 and 1091–1109. The original papers could not be
retrieved for this work. The equations and element values are those
documented in the COMSOL Multiphysics Acoustics Module User's Guide 6.4,
"Physiological Models / Human Ear Drum Impedance", Eq. 2-34, Eq. 2-35 and
Table 2-8 (<https://doc.comsol.com/6.4/doc/com.comsol.help.aco/aco_ug_pressure.05.176.html>).
They are copied in `data/ear/hudde_engel.json`.

**Structure.** The middle-ear cavities (tympanic cavity R + C in parallel
with the aditus R + L, leading to the antrum and the resonant mastoid air
cells) are in series with a chain matrix K. K maps the stapes–cochlea load
to the drum through:

- a frequency-dependent effective drum area A_D(ω);
- a drum shunt admittance Y_ac with a frequency-dependent inertance and a
  phase law;
- the malleus–incus network.

**Implementation decisions**, all recorded in the data file:

- The phase laws `Φ_A = s_Aph·log(ω/ω_Aph) + φ_A` and
  `Φ_Y = s_Yph·log(1 + ω/ω_Yph)` use the natural logarithm, which is what
  `log` means in COMSOL's expression syntax. The check below favours ln
  clearly: with log10, |Z| misses the plotted curve by 2.5 dB at 3 kHz.
- `Z_cpl` and `Z_free` are admittances, as in the 6.4 guide. The 6.0 guide
  prints `Z_cpl` without the inverse, which is dimensionally inconsistent.
- φ_A = −0.8038 rad is COMSOL's value, chosen for phase continuity. With the
  other parameters, arg A(ω_Aph) = −0.8023 rad; the 0.9 mrad step is kept.

**Cross-check.** The only published curve of this implementation is
Fig. 5 of Nielsen & Herring Jensen, "The digital twin of a new and
standardized fullband ear simulator", DAGA 2022, pp. 182–185
(<https://pub.dega-akustik.de/DAGA_2022/data/articles/000465.pdf>). It was
digitised from the figure's pixels; the plot reads to about ±0.3 dB and ±1°.

| Band | Magnitude difference | Phase difference |
|---|---|---|
| 165–430 Hz | ≤ 0.1 dB | ≤ 1° |
| 0.87–2.3 kHz | −1.1 to +2.5 dB | up to +16° |
| 2.4–13.5 kHz | ≤ 0.7 dB | +4° rising to +28° (model leads) |

The magnitude agrees outside the drum resonance. Above 1 kHz the phase
leads the plotted curve by a growing amount, the equivalent of about 4.4 µs
of delay. The cause is not known without the original papers. The high-
frequency phase of this model is therefore uncertain (open issue 2). The
test pins the low-frequency values: 26.5 dB re 8e6 Pa·s/m³ at 165 Hz and a
minimum of 12.2 dB near 865 Hz, both ±0.3 dB.

**Passivity.** Re Z > 0 from 1 Hz to 67.8 kHz. The phase laws make it
negative above that, far outside the 16 kHz for which the model is defined.
The element reports "defined to 16 kHz".

**Scale factors.** `R_scale`, `M_scale` and `C_scale` multiply every
resistance, every mass or inertance, and every compliance of the drum,
ossicles and cochlea (spec p.27). `middle_ear_volume_cm3` replaces the 0.5 cm³
tympanic cavity. The antrum and mastoid are unchanged.

### Type 4.3 drum (`model: "type43"`)

P.57 does not publish the drum network of a Type 4.3 simulator.
Nielsen & Herring Jensen (DAGA 2022) fitted a free-form impedance,
frequency by frequency, starting from Hudde & Engel. Here a small lumped
network is fitted instead:

```
Z     = Z_cav + 1 / (jωC_m + 1/(R_o + jωM_o))
Z_cav = 1 / (jωC_t + 1/(R_a + 1/(jωC_a)))
```

- C_m is the drum-membrane compliance.
- R_o and M_o form the path through which the drum couples to the
  middle-ear cavity.
- C_t is the exposed middle-ear cavity compliance.
- R_a and C_a are an aditus resistance and an antrum.

All compliances are stored as air volumes, V/γP0. This is the
Hudde & Engel topology reduced to constant elements.

Two alternatives were tried in exploratory, unscripted fits:

- The Hudde & Engel model with fitted R, M, C and cavity scales did not
  reproduce the low-frequency roll-off of Table 5-c (−4.56 dB at 20 Hz):
  the best fit stayed 3.3 dB high at 20 Hz, with 95 of 121 points in
  tolerance.
- A plain series R–M–C drum in series with the same cavity did not hold the
  sharp 10.6 kHz half-wave peak together with the smooth 1–3 kHz blocking.
  Its best fit left 5 of 121 points outside tolerance, at 9.5–11.2 kHz.

**Fit** (`tools/ear/fit_type43.py`): least squares in log-parameters, then a
second pass minimising the 8-norm, which is close to minimax. Inputs:

- all 121 Table 5-c points, each deviation divided by the tolerance on its
  side;
- the absolute 27.7 MPa·s/m³ at 500 Hz (clause 6.4.3.3 NOTE 2), weighted
  at 0.2 dB.

The canal is the P.57 canal described below, with 48 segments, at 23 °C.
Result (`data/ear/type43_drum.json`):

| Quantity | Value |
|---|---|
| C_m | 0.0897 cm³ |
| R_o | 2.109e7 Pa·s/m³ |
| M_o | 418 kg/m⁴ |
| C_t (middle-ear cavity) | 1.429 cm³ |
| R_a | 7.32e8 Pa·s/m³ |
| C_a | 30.6 cm³ |
| Points within Table 5-c tolerance | 121 / 121 |
| Largest deviation / tolerance | 0.875 |
| RMS deviation | 0.75 dB |
| \|Z_T\| at 500 Hz | 28.21 MPa·s/m³ (+1.8 %) |
| Effective volume at 500 Hz | 1.601 cm³ (P.57: 1.63 ± 0.10) |

C_a is constrained only by the 20–40 Hz points. Between 5 and 100 cm³ it
moves the 20 Hz level by 0.9 dB and leaves 500 Hz unchanged to 1e-3.
The scale factors act on R_o, M_o and C_m. `middle_ear_volume_cm3`
replaces C_t.

### IEC 60318-4 equivalent (`model: "iec60318_4"`) and rigid

`iec60318_4` lumps the two Helmholtz branches and the microphone of the
coupler model below at one plane. This is the "traditional" tympanic
impedance, model 3 of Luan et al. (2019). It is a diagnostic termination,
human-valid to 10 kHz. `rigid` has zero admittance.

## IEC 60318-4 occluded-ear simulator (`iec60318_4`)

**Status: literature model, not verified against the IEC 60318-4 Table 1.**

**Topology.** Nielsen, Schuhmacher, Liu & Jønsson, "Simulation of the IEC
60711 occluded ear simulator", AES 116th Convention (2004), paper 6162.
Luan, Sgard, Benacchio, Nélisse & Doutres, "A transfer matrix model of the
IEC 60318-4 ear simulator", *Acta Acustica united with Acustica* 105(6)
1258–1268 (2019), Fig. 2 and Eq. 6. The accepted manuscript is open at
<https://espace2.etsmtl.ca/20020/>.

The transfer matrix is `T1 · shunt(Y2) · T3 · shunt(Y4) · T5`, terminated by
the microphone:

- T1, T3 and T5 are the main-cavity sections, as thermoviscous tubes of
  radius R0. Luan et al. treat them as lossless.
- Branch 2 is a rectangular slit a2 × b2 × h2 (thermoviscous slit medium)
  into an annular cavity r2..R2 of thickness d1. The slit length includes
  an end correction at both ends: Munjal et al., *Formulas of Acoustics*
  (2008) p. 319, as Luan Eq. A.7. For h2 = 0.16 mm it is 0.199 mm.
- Branch 4 is a radial slit of gap h4 from R0 to r4 over three arcs of
  95.33°, into an annular cavity r4..R4 of thickness d2. It is a chain of
  32 stepped slit segments of area angle·r·h4; the relative error at
  3 kHz is 3.5e-6 against 128 steps. End corrections are 0.096 mm (inner)
  and 0.099 mm (outer), with each arc's perimeter as the width.
- The cavities are compliances with the thermal wall-layer correction of
  the `cavity` element, using the wall area of both faces and both rims.
  Above the branch resonances the slit inertance carries the branch impedance,
  so the radial-wave cavity form of Luan Eq. A.8 is not needed.

**Geometry** (Luan et al. Table 1, mean micro-CT values of a G.R.A.S.
RA0045, in mm): R0 3.77; L1 3.12; L3 4.75; L5 4.69; a2 2.53; b2 2.35; h2 0.16;
r2 6.30; R2 9.01; d1 1.91; r4 4.66; h4 0.05; R4 9.01; d2 1.40.

**Microphone.** B&K Type 4192, as a series R–M–C: C = 0.62e-13 m⁵/N (8.8 mm³),
R = 119e6 Pa·s/m³, M = 710 kg/m⁴. Values from the COMSOL "Generic 711
Coupler" model documentation, after the B&K Microphone Handbook (1995)
pp. 6–18. Use `microphone: "rigid"` for a rigid end.

**Fit** (`tools/ear/fit_iec60318_4.py`). Only publicly stated facts are used:

- effective volume 1260 mm³ at 500 Hz (GRAS RA0045 product data, "Volume
  1260 mm³ @ 500 Hz");
- half-wave resonance near 13.5 kHz (COMSOL documentation: "prescribed by
  the IEC standard … around 13.5 kHz").

With the geometry exactly as published, the model gives 1068 mm³ at
500 Hz, 15 % short. Luan et al.'s own Fig. 7 reads 166.5 dB re 1 Pa·s/m³
at 100 Hz, which corresponds to 1070 mm³, so the deficit is in the geometry
rather than this code.

The slit heights cannot close the gap within their stated micro-CT
uncertainty:

| Slit height | Effective volume at 500 Hz |
|---|---|
| h2 from 0.10 to 0.22 mm | 886–1090 mm³ |
| h4 from 0.03 to 0.07 mm | 900–1112 mm³ |
| COMSOL's pair, 0.17 / 0.069 mm | 1122 mm³ |

The one fitted parameter is therefore a common scale on the two side-cavity
volumes. The fit gives **side_volume_scale = 1.631**, which takes the
cavities to 406 and 427 mm³. That is far outside the micro-CT uncertainty
and is flagged as open issue 1.

The half-wave resonance is checked, not fitted: 13.55 kHz, against the
required 13.5 ± 1.5 kHz. Resulting effective volumes:

| Frequency | Effective volume |
|---|---|
| 20 Hz | 1576 mm³ |
| 100 Hz | 1462 mm³ |
| 500 Hz | 1260 mm³ |
| 1 kHz | 785 mm³ |
| 2 kHz | 522 mm³ |
| 5 kHz | 433 mm³ |

**Validity reported.** "IEC 60318-4 literature model: human-valid 100 Hz to
10 kHz, coupler-only above": shading begins at 10 kHz and deepens at 16 kHz.
The main cavity also reports its transverse mode (26.7 kHz) and Stinson
bound (19.1 kHz). Below 100 Hz the model is coupler-extrapolated. A
`ValidityLimit` has no lower bound, so this appears only in the criterion
text.

The macro's terminal is joined by an ideal short to `<id>.eep`, the
reference plane. `<id>.drp` is the microphone plane.

## Type 3.3 (`type33`)

`type33` puts a cylindrical ear-canal extension in front of `iec60318_4`: bore
7.5 mm and length 10.0 mm, between `<id>.eep` and `<id>.ref`.

- The bore is the P.57 principal-cavity diameter (clause 6.4.4.4.1).
- ITU-T P.57 clause 6.3.1 states the 10.0 mm length for Type 3.1.
  Nielsen & Herring Jensen (DAGA 2022) used 10 mm for Type 3.3. It is
  still **to be verified for Type 3.3**.

`extension_length_mm` and `extension_diameter_mm` override both. The pinna
is not modelled. Test: the extension adds its thermally corrected volume to
the low-frequency effective volume, within 0.5 %.

## ITU-T P.57 Type 4.3 (`type43`)

### Geometry (`tools/ear/p57_geometry.py` → `data/ear/type43_geometry.json`)

ITU-T P.57 (06/2021) is free from <https://www.itu.int/rec/T-REC-P.57-202106-I>.
The script reads the following from the PDF:

- Table 6: centre-line points 0–28 mm in 0.5 mm steps, the DRP, the
  reference plane at 17.33 mm and the EEP projections.
- Table B.2: the periphery points of the cross sections at 0.5, 2, 4, …,
  28 mm, the reference plane, and the concha-bottom planes at 29.5, 31 and
  32.5 mm.

It writes only derived quantities: each polygon's shoelace area and
perimeter, and the plane positions. Positions are the Recommendation's plane
labels, measured from the tip along the curved centre line. The polyline
through the Table 6 points is 27.90 mm long from 0 to 28 mm, 0.35 % shorter
than the labels, and the labels are used.

The DRP (105.5, 42.02, 53.76) is not on the centre line. Its axial position
is where the plane normal to the centre line passes through it: 4.02 mm from
the tip. The DRP is 4.48 mm from the tip point.

Resulting areas: 4.5 mm² at 0.5 mm, 20.8 at 4, 29.4 at 8, 41.6 at the
reference plane, 44.2 at 20, 56.8 at 28, and 99.8 at the EEP (31 mm).
Volumes: tip to DRP 44.4 mm³, DRP to reference plane 451.3 mm³, reference
plane to EEP 705.4 mm³.

### Model

```
EEP (31 mm) ── canal ── ref (17.33 mm) ── canal ── DRP (4.02 mm) ─┬─ drum (type43 fit)
                                                                   └─ tip stub to 0.5 mm, rigid end
```

- The inclined drum is lumped at the DRP's axial position, the
  microphone plane. Above about 10 kHz the field near a real inclined drum
  is three-dimensional (spec p.28), and the DRP is defined as the
  microphone-plane pressure.
- The wedge between the DRP plane and the tip is a rigid-ended conical stub
  in parallel with the drum. The last 0.5 mm, about 1 mm³, is omitted.
- The 48 segments are shared out over 0.5–31 mm in proportion to length.
- `input: "ref"` drives the reference plane directly and omits the outer
  canal, as for an insert earphone or the Table 5-c measurement. `<id>.eep`
  then does not exist.
- `drum` selects another termination (`hudde_engel`, `iec60318_4`, `rigid`),
  and the drum scale keys are passed through.

### Checks

- The Rust macro reproduces the Python model to 1e-9 (tests
  `type43_matches_independent_implementation`).
- Headline values (test `type43_headline_values`): |Z_T| at 500 Hz =
  28.21 MPa·s/m³ against 27.7 ± 5 %; effective volume 1.601 cm³ against
  1.63 ± 5 %.
- Table 5-c (test `type43_reproduces_p57_table_5c_when_available`): all 121
  points are within tolerance. This test reads `private/p57_table5c.json`,
  which `p57_geometry.py` writes from the PDF, and skips when the file is
  absent (CLAUDE.md, erratum E41).
- Validity: "ITU-T P.57 Type 4.3: specified 20 Hz to 20 kHz" (begin 20 kHz).
  The canal's line bounds are taken at its widest modelled section:
  - from the reference plane (`input: "ref"`): r = 3.75 mm at 20 mm, giving
    the Stinson bound 19.2 kHz (spec p.26, "about 19 kHz") and cut-on 26.8 kHz;
  - from the EEP: the concha-bottom section at 31 mm (99.8 mm², r = 5.64 mm)
    gives a Stinson bound of 14.6 kHz and cut-on 17.8 kHz, which is honest
    but pessimistic for the flaring concha region.

## Internal nodes and power balance

Each macro joins its single terminal to `<id>.eep` with an ideal short. It
then exposes `<id>.eep`, `<id>.ref` (where the simulator has one) and
`<id>.drp`. The macro has one port per node it touches. The port's flow is
the net volume velocity entering the macro there, so Tellegen's sum holds
even when other elements are connected to internal nodes. The test
`power_balances_with_ear_loads_and_internal_node_connections` hangs a
resistor on `e711.drp`, a resistor and cavity on `t43.ref`, and an eardrum on
`t33.ref`. Across 20 Hz to 19 kHz the sum stays below 1e-10 of the delivered
power, and every ear part absorbs power.

## Errata that bear on these loads

- **E6.** The Harman over-ear fixture is a GRAS 45CA with RA0045 couplers
  (IEC 60318-4) and custom pinnae, not IEC 60318-1. Its ear load is the
  `iec60318_4` coupler; the pinnae are not modelled.
- **E19.** The pressure division ratio of Møller et al. (1995) is
  `(Z_ear + Z_rad)/(Z_ear + Z_hp)`. The macros provide Z_ear as the
  terminal's input impedance (port 0 of the element). No PDR output is
  added here.
- **E36.** The ISO 11904-2 conversion data cover only 20 Hz–10 kHz. No such
  conversion is implemented here.
- **E41.** The standard tables stay out of the repository. The Table 5-c
  test runs only when `private/p57_table5c.json` is present.

## Open issues

1. **IEC 60318-4 volume deficit.** The published RA0045 micro-CT geometry
   gives 1068 mm³ at 500 Hz against 1260. The fitted side-volume scale of
   1.63 is not physically confirmed. A measured transfer impedance of an IEC
   60318-4 coupler, from a clone-calibration session, would settle it.
2. **Hudde & Engel phase above 1 kHz.** The phase differs from COMSOL's
   plotted curve by up to 28° at 13.5 kHz. The 1998 papers were not
   available to resolve this.
3. **Type 4.3 drum.** The network is a fitted surrogate, not the physical
   Type 4620 drum simulator. C_a is weakly identified, by the 20–40 Hz
   points only.
4. **Type 3.3 extension length** of 10 mm needs confirming from P.57 or a
   manufacturer.
5. **Canal shape and curvature.** Neither the non-circular perimeter
   (+2–16 % wall loss) nor centre-line curvature (Xia et al. 2024) is
   modelled.
6. **No lower validity bound.** The "coupler-extrapolated below 100 Hz"
   flag of the 60318-4 model cannot be expressed as a `ValidityLimit`.
7. Not in this package: the damped 60318-4 variant, Type 4.4, IEC 60318-1
   and -8 loads, population mode and pinna treatments.

## Reproducing

```sh
python3 tools/ear/p57_geometry.py      # downloads P.57 into private/, writes data/ear/type43_geometry.json
python3 tools/ear/fit_type43.py        # needs private/p57_table5c.json; writes data/ear/type43_drum.json
python3 tools/ear/fit_iec60318_4.py    # writes data/ear/iec60318_4.json
python3 tools/ear/gen_fixtures.py      # writes crates/acoustilab/tests/data/ear_reference.json
```

The scripts need Python 3 with numpy, scipy and PyMuPDF. The reference
models are `tools/ear/earmodels.py` and `tools/ear/type43.py`.
