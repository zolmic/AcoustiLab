# Design templates

A design template is a parametric netlist (`docs/parameters.md`) for a
generic class of headphone, never a product (spec Section 18). It opens in
the web UI's Design tab with a cross-section sketch (`docs/web.md`), and the
CLI and the analyses act on the same parameter names. Three templates ship:

| Template | File | Driver | Ear loads | Fit states | Sketch |
|---|---|---|---|---|---|
| Over-ear (circumaural), closed or open back | `examples/design_over_ear.json` | 40 mm, Tymphany HPD-40N16PET00-32 primary set | IEC 60318-4; P.57 Type 4.3 | leak gap (0.08 mm: fixture seal) | `over_ear` |
| On-ear (supra-aural) | `examples/design_on_ear.json` | the same 40 mm driver | IEC 60318-4 behind the P.57 10 mm canal extension (`type33`); P.57 Type 4.3 from its entrance point | sealed, P.57 Type 3.2 low and high leak, custom slit | `on_ear` |
| In-ear (IEM) | `examples/design_in_ear.json` | 10 mm class micro-driver | IEC 60318-4; P.57 Type 4.3 canal from its reference plane | sealed, slight, loose, custom leak tube | `in_ear` |

```sh
cargo run -p acoustilab-cli -- params examples/design_on_ear.json
cargo run -p acoustilab-cli -- solve examples/design_in_ear.json --set fit=loose --csv
```

**Theory only.** No template has been checked against a measured
headphone. Every default is either a public anchor, cited below, or an
estimate, and the netlist's `description` says which. The defaults describe
a measurement on the template's default ear load, with the leak of a fixture
(a well-sealed pad, a sealed ear tip) or the fit state named.

## Conventions

Every template follows these; `crates/acoustilab/tests/templates.rs` checks
the ones a program can check.

- Parameters carry a `label` and a `group`. Groups run from the driver
  outwards: driver, front, pad or tip and leak, rear, ear, source, model.
  Numbers have `min` and `max`; a `tolerance` names its `source`.
- Names are shared where the part is the same: `driver_*`, `front_depth_mm`,
  `front_volume_cm3`, `rear`, `open_back`, `rear_volume_cm3`, `rear_depth_mm`,
  `vent_*`, `grille_rayl`, `damping_*`, `driver_back_volume_cm3`, `ear`,
  `drive_mW`, `rated_impedance_ohm`, `source_impedance_ohm`, `fidelity`,
  `points_per_octave`.
- Alternatives are items with `enabled` expressions sharing one id (two
  vents with and without a mesh, two ear loads named `ear`), so probes such as
  `ear.drp` and `u_vent` work in every state. A parameter read only by a
  disabled item is reported `active: false` (docs/parameters.md): the vent
  sizes with an open back, the leak sizes when sealed, the custom leak when a
  preset fit is chosen. A leak size is 0 when sealed, so the sketch draws no
  gap.
- Probes: `p_drp` (the ear's drum reference point, the primary probe), the
  pressures in front of and behind the driver, `zin` (electrical input
  impedance), `x` (diaphragm displacement), and the volume velocities of the
  vent and the leak (and the IEM's nozzle).
- The drive is 1 mW into the rated impedance; warnings are checked at it.
- `ui`: `template`, `primary_probe: "p_drp"`, `ear_load: "ear"` and a
  `sketch` whose kind is the template's name (docs/web.md, "The ui block").
- The `description` says what is modelled, what the anchors are, what is
  estimated and what is not modelled, and points here.

## Over-ear

**Modelled.** The front cavity is a cylinder between the diaphragm and the
ear (a two-node cavity: a depth line at L1), closed by an impermeable
(protein-leather) pad except for a thermoviscous leak slit of 8 segments
around its inner edge. Behind the diaphragm an air space leads through a
damping cloth over the driver's rear openings (`mesh`, R_s/A) into either a
closed rear cavity with vents (optionally meshed) or an open grille that
radiates into a baffle. The ear load is the IEC 60318-4 simulator (a
flat-plate fixture) or the ITU-T P.57 Type 4.3 ear.

### Realism of the defaults

Before this revision the default design was a 29.5 cm³ front cavity, a
25 cm³ rear cavity behind a 3 mm vent under a 160 rayl mesh, and the
undamped Tymphany driver (Qms 2.71). With Sd = 10 cm², the air springs are
ρc²·Sd²/V = 4.8 N/mm (front) and 5.7 N/mm (rear, which the mesh leaves
sealed above its 40 Hz corner), against the suspension's 0.079 N/mm. The
diaphragm therefore resonated on the air at 0.94 kHz with a Q of 6 to 7
(a lumped pole estimate): the drum level peaked at 125 dB near 0.92 kHz,
16 dB above its 100 Hz level, the input impedance peaked at 67 Ω near
0.95 kHz (twice Re), and the response fell 12 dB per octave above it.
Published data on circumaural cups:

| Quantity | Value | Source | Status |
|---|---|---|---|
| Front volume of a circumaural cup | "typically about 50 CC" | Bose, US 6,597,792 B1 (Sapiejewski, Monahan), noise-reducing headset | patent |
| Front volume, flat surface on the uncompressed pad, ear not subtracted | 62.4 and 78.6 cm³ | Focal-JMlab, US 12,513,448 B2 (Rezaei, filed 2021) | patent |
| Front volume of a military ANR cup | 100 cm³ | Bose, US 2014/0294222 A1, headset porting | patent |
| Rear volume of an ANR cup | 15 cm³; 11.1 cm³ | the two Bose documents above | patent |
| Rear ports of an ANR cup | 2.25 mm² conventional; 3.4 mm × 37 mm mass port | Bose, US 2014/0294222 A1 | patent |
| Intentional front leak | a 1.45 mm × 10 mm vent, cut-off 83 Hz | Focal-JMlab, US 12,513,448 B2 | patent |
| Resonance in a tightly closed coupling space | rises to 1.5–4 kHz for minimum-mass diaphragms whose own resonance is 70–250 Hz | AKG, US 4,389,542 (Görike, 1983) | patent |
| Impedance maximum | about 100 Hz for a circumaural headphone (about 6 kHz for an intra-concha one); the patent does not say whether it was measured on an ear, a fixture or in free air, nor whether the back is open or closed | Fraunhofer, US 11,039,243 B2 (2016), Fig. 4A | patent, example of unstated condition |
| Impedance of a closed-back headphone on a GRAS 45CA | "dead flat" (ATH-M50x) | Audio Science Review, review of 2 March 2021 | independent measurement, not peer-reviewed |
| Spacing between diaphragm and damping material | 0.1–5 mm | Sennheiser, US 6,934,401 B2 | patent |
| Damping textiles | 200, 600 and 650 rayl grades | `data/materials/meshes.json` (Hough and Law 2025 as quoted in the spec) | unverified |

The old front volume was below every published figure. A lightly damped
resonance 16 dB high near 1 kHz, with the input impedance doubling there,
is not what the one fixture measurement found here shows: a common
closed-back measures flat on a GRAS 45CA. (The Fraunhofer example peaks
near 100 Hz, where drivers resonate in free air; behind a sealed front
cavity of tens of cm³ a 10 cm² diaphragm cannot resonate there, and the
patent does not state the condition, so it is no evidence either way.) No
public source gives the damping of a given product's driver. The
AKG patent explains the mechanism: a small closed coupling volume pushes the
resonance into the kHz range, and headphone drivers are damped acoustically,
behind the diaphragm, where the Tymphany driver's free-air Qms of 2.71 shows
that this OEM driver brings little damping of its own.

**Revision.** The front cavity is 27.5 mm in radius and 20 mm deep, 47.5 cm³
(within the published range; the Focal figures are for an uncompressed pad
with no ear, so a compressed pad on a fixture sits below them). A damping
cloth of 600 rayl over 2 cm² of rear openings, behind a 1 cm³ air space, is
added, with its parameters (`damping_rayl`, 0 removes the cloth and leaves
the air space open to the rear cavity or grille; `damping_area_cm2` and
`driver_back_volume_cm3` in the detailed view). 600
rayl is a grade of the materials database; the area and the air space are
estimates. The rear cavity, vent, pad and leak are unchanged: the 0.08 mm
leak is a fixture seal (spec Section 8: flat-plate fixtures over-seal).
Nothing was adjusted to follow a target curve.

**What the defaults give** (1 mW into 32 Ω, L1, IEC 60318-4, levels at the
drum reference point):

| Design | 20 Hz | 100 Hz | 300 Hz | 1 kHz | 3 kHz | Input impedance maximum |
|---|---|---|---|---|---|---|
| Default (closed, damped) | 109.2 dB | 106.1 dB | 105.1 dB | 100.1 dB | 89.5 dB | 34.6 Ω at 0.97 kHz |
| Without the damping cloth | 109.7 | 106.9 | 107.6 | 113.8 (peak 122.1 at 843 Hz) | 89.2 | 73.4 Ω at 843 Hz |
| Leak gap 0.2 mm | 95.2 | 101.6 | 105.1 | 100.7 | 89.6 | 34.6 Ω |
| Leak gap 0.5 mm | 72.3 | 83.6 | 101.3 (peak 110.5 at 516 Hz) | 102.6 | 89.7 | 34.6 Ω |
| Open back | 114.7 | 113.5 | 108.7 | 98.9 | 88.8 | 34.4 Ω at 563 Hz |

The damped default has no resonance peak: the drum level stays within
0.5 dB of its 100 Hz value or below it up to 2 kHz, and the input impedance
stays within 10 % of Re (both tested). The coupled resonance is still there
(the impedance maximum at 0.97 kHz), damped to a Q below 1. The diaphragm
moves 1.4 µm RMS at 100 Hz, where the level is 106 dB; at 100 dB that is
0.7 µm, inside erratum E39's 0.4–1.4 µm for a sealed 30–100 cm³ cup.

**How much the cloth decides.** Its acoustic resistance R_s/A =
3.0 MPa·s/m³ is 3.0 N·s/m at the diaphragm (R·Sd²): 53 times the
suspension's own loss (ωs·Mms/Qms = 0.057 N·s/m) and 20 times the
electrical damping (Bl²/Re = 0.15 N·s/m). It overdamps the coupled
resonance (Q about 0.5, a lumped pole estimate), so the default's drum
level falls steadily from the bass, 1 dB down at 300 Hz and 6 dB at 1 kHz.
The cloth's value is an estimate, and it sets the response between 300 Hz
and 2 kHz (same conditions as above):

| `damping_rayl` | Highest level above the 100 Hz level, 100 Hz–2 kHz | 1 kHz re 100 Hz | Input impedance maximum |
|---|---|---|---|
| 0 | +15.2 dB at 843 Hz | +6.9 dB | 73.4 Ω |
| 150 | +4.7 dB at 773 Hz | +2.1 dB | 38.9 Ω |
| 300 | +0.8 dB at 632 Hz | −1.5 dB | 36.1 Ω |
| 600 (default) | none | −6.0 dB | 34.6 Ω |
| 1000 | none | −9.2 dB | 34.0 Ω |

The cloth also damps the driver in free air: mounted with it, the Tymphany
driver's free-air impedance maximum falls from 121 Ω at 82 Hz to 34 Ω. A
headphone whose free-air impedance shows a clear hump at the driver's
resonance has less rear damping than this default.

### What the engine lacks for over-ears, and where it shows

- **Pinna and concha.** No ear load includes them (docs/ear-loads.md, open
  issue 7). On a head, the pinna, the concha and the canal's quarter-wave
  gain under a low-impedance cup shape the drum response above about 2 kHz
  (spec Section 7). The template's response above 1 kHz, falling about 6 dB
  per octave in the damped, resistance-controlled region, is the lumped
  model's, and the shading (from 1.0 kHz, the rear cavity's lumped limit) says
  so. It is not a prediction of a real cup's treble.
- **Transverse modes of the front chamber.** The two-node cavity is a
  one-dimensional depth line. Its first transverse mode, 1.84c/(2πr), is at
  3.7 kHz for r = 27.5 mm (shaded from 2.6 kHz), and the depth line's own
  half-wave puts a peak at the drum near 7 kHz that a real chamber, driven
  off-axis and lined with an absorbing pad, would not show so strongly. The
  engine's `modal_cavity` can model the chamber; the template does not use
  it, because its ports need positions a generic template does not have.
- **A porous pad path.** Velour and fabric pads pass air through the pad.
  The elements exist (`mesh`, `porous_layer`), but no measured cover or pad
  foam data do (spec Section 8: velour "expected in the 50 to 600 rayl class
  but was not measured"), so the pad is impermeable.
- **Cup and cushion mechanics** (cushion compliance, cup mass, clamp force;
  spec Section 8). They set the isolation and the lowest bass on a head,
  not the response on a rigid fixture.
- **Driver above the piston range.** The rigid piston ends at ka = 1
  (3.1 kHz for 10 cm², erratum E28, shaded); break-up (D3) is not modelled,
  and the D2 surround keys have no data for this driver.
- **Driver internals.** The damping is one cloth over the rear openings
  with one air space; pole vents and front screens (spec Section 5) are not
  separate elements.

### Open back

The open back stays an option of this template (`rear: open`) rather than
a separate one. The comparison it answers, the same driver, cup and ear with
the back opened, is then one control and a baseline, and no second file of
thirty parameters can drift from the first. The option replaces the rear
cavity and vents by a 30 rayl grille over the diaphragm area radiating into
a baffle, behind the same damping cloth. With the template's impermeable pad
on a sealed fixture, opening the back raises the bass by 7.4 dB at 100 Hz
(no rear air spring) and moves the impedance maximum down to 563 Hz.
Open-back headphones often have porous (velour or fabric) pads, which the
template does not model (above), so the option shows an open back on a
sealed pad, not an open-back headphone as usually built, on a head.

## On-ear

**Modelled.** The pad rests on the pinna. The front chamber is a cylinder
of the pad's inner radius, from the diaphragm to the compressed pinna, and
the concha (a cavity at the ear side) is joined to it. The leak between pad
and pinna is a thermoviscous slit (`leak`, one or two segments) chosen by
the fit state. The rear is the over-ear template's: damping cloth, closed
cavity with meshed vents or open grille. The ear load is the IEC 60318-4
simulator behind the 10 mm, 7.5 mm bore canal extension of the P.57 Type 3
ears (`type33`; its pinna simulator is not modelled), or the P.57 Type 4.3
ear driven at its entrance point (EEP), which already contains the 0.68 cm³
between its reference plane and the EEP, the outer canal and the bottom of
the concha (docs/ear-loads.md).

**Fit states.** The spec (Sections 8 and 18) asks for the P.57 Type 3.2
slits. ITU-T P.57 (06/2021), clause 6.3.2 and Table 3-a, defines the Type 3.2
simplified pinna simulator's leaks for receivers held firmly (low) or
loosely (high): slits 0.26 mm high and 2.8 mm deep over 84°, and 0.50 mm high
and 1.9 mm deep over 240°. P.57 recommends this ear for supra-aural receivers
and marks every leak dimension as guidance only; the leaks come from
telephone handsets (erratum E38). The slit breadth is the arc of the opening
angle at the slit's mid-depth in the rim of the simulator's 25 mm cavity
(P.57 Figure 6; this reading of the figure is ours): 20.4 mm for the low
leak, drawn in Figure 6 as two 42° slits (two segments), and 56.3 mm for
the high leak (one segment). `custom` is a slit of chosen height and
breadth, as deep as the pad face is wide. `sealed` is a reference only.

A review check (not a test, because the cavity depth is read from a
drawing): a Type 3.2 simulator built from engine elements, the `type33`
load with a 25 mm cavity 10 to 11.5 mm deep in front of it (P.57 Figure 6)
and the template's slit between the cavity and the room, has its
input-impedance maximum at 751 to 710 Hz with a Q of 1.85 to 1.87 for the
low leak. P.57's normative acoustic specification (Table 4-a) is
713.8 ± 25 Hz and Q 1.81 ± 0.18, which supports this reading of Figure 6
for the low leak. The high leak does not match: 1841 to 1747 Hz with a Q of
6.2 to 6.7, against 1570 ± 50 Hz and Q 3.5 ± 0.35; the 33 holes of P.57's
other high-leak construction (Table 3-b) give the same. As dimensioned, the
high-leak slit carries less air mass and less loss than the high leak P.57
specifies acoustically (its NOTE 7 lets makers adjust the dimensions to meet
the impedance), so the template's high state is a lighter and less damped
leak than P.57's high leak.

**Anchors.** Driver: as the over-ear. Front volume: Bose, US 8,111,858 B2
(supra-aural noise reduction), gives a front enclosed volume "greater than
10 cc", "about 25 cc", "in the range of 30 cc" with the cushion foam; the
template's 10.4 cm³ (6.1 cm³ under the pad plus the concha) is at the low
end, as a pad pressed on the pinna leaves little air. Concha: Burkhard and
Sachs measured a mean of 4.65 cm³ for men and 3.94 cm³ for women (SD 0.76
and 0.81 cm³), as quoted in Red Tail Hawk / Gentex, US 8,638,963 B2; the
template uses 4.3 cm³ ± 1.6 cm³ (2 SD). **Estimates:** the pad (18 mm inner
radius, 12 mm wide), the 6 mm depth to the pinna, the 12 cm³ rear cavity,
the vent, and the damping cloth.

**What the defaults give** (1 mW into 32 Ω, L1, `type33`):

| Fit | 20 Hz | 100 Hz | 300 Hz | 1 kHz | 3 kHz | Displacement at 100 Hz |
|---|---|---|---|---|---|---|
| Sealed | 113.1 dB | 110.1 dB | 110.0 dB | 111.4 dB | 107.0 dB | 0.56 µm |
| Low leak (default) | 86.9 | 96.0 | 108.3 | 112.5 | 107.3 | 1.08 µm |
| High leak | 58.4 | 73.0 | 91.1 | 112.3 | 109.0 | 1.12 µm |

The bass is set by the leak (spec Section 18): at 100 Hz the low leak
takes 14 dB off the sealed level and the high leak another 23 dB, while
the level above 1 kHz hardly moves; with the low leak most of the
diaphragm's volume velocity leaves through the slit at 100 Hz (tested). The
excursion roughly doubles when the leak opens (spec Section 8: "excursion
rises with leak"). Not modelled: the pinna's own compliance, the pad's
compression under clamp force, leaks elsewhere on a real pinna, a porous pad
path, and everything listed for the over-ear.

## In-ear (IEM)

**Modelled.** A dynamic micro-driver whose front faces a small front volume
that opens into the nozzle bore; an optional mesh or damper across the
nozzle outlet; the area step from the bore into the canal; the ear tip,
sealed or leaking through an equivalent tube to the room; behind the
diaphragm an air space, the driver's damping over its rear openings, the
rear volume of the shell and a pressure-relief vent that can carry a mesh.
The IEM's cavities are cylinders as wide as the diaphragm (the sketch
draws them so). The ear load is the IEC 60318-4 simulator (the coupler for
insert earphones) or the P.57 Type 4.3 canal driven at its reference plane
(`input: "ref"`).

**Anchors.** The driver's numbers are those of the small in-ear headphone
of C. Poldy, *Headphone Fundamentals*, AES 120th Convention tutorial
(Paris, 2006), chapter 3, Fig. 20: mechanical ("vacuum") resonance 255 Hz,
moving mass 17.5 mg, Bl 0.46 T·m, coil resistance 14.4 Ω, diaphragm area
0.8 cm², rear cavity 0.15 cm³, driven at 0.126 V (1 mW into 16 Ω). That
example is an earbud, not a sealed insert: it rests at the canal entrance
behind a foam cover (Poldy's Fig. 17), its circuit shunts the canal
entrance to the room through the foam (a resistance labelled 4e5, a
hundredth of the back holes' 400e5), and its rear cavity opens to the room
through back holes and a bass tube. With those leaks its
Fig. 21 gives about 95 dB at the IEC 711 drum point at 100 Hz. It is a
simulation example, not a measured driver, and it has no mechanical loss
(Rms = 0); the template takes Qms = 3 as an estimate and derives
Qes = 2π·fs·Mms·Re/Bl² = 1.91. The tutorial was read in a copy hosted by a
third party, not by the AES, so the values are not checked against an
official copy. The rated impedance is 16 Ω (estimate). The
leak tubes are two of the sizes Groon, Rasetshwane, Kopun, Gorga and Neely
(Ear and Hearing 36(1), 155–163, 2015; PMC4272628) inserted through foam
ear tips to simulate leaks, 13 mm long: 0.020 in (0.508 mm, "slight") and
0.040 in (1.016 mm, "loose"). They found the effect significant above an
equivalent diameter of 0.010 in. Which leak is typical of a person is not
established, so the states are illustrations. The vent mesh is the 260 rayl
Saati grade of the materials database (unverified). **Estimates:** front
volume 0.1 cm³, nozzle 2 mm × 7 mm, rear volume 0.5 cm³, vent 0.5 mm
through a 1.5 mm wall, damping 300 rayl over 0.1 cm², Xmax, rated power and
every tolerance.

**What the defaults give** (1 mW into 16 Ω, L1, IEC 60318-4):

| Fit | 20 Hz | 50 Hz | 100 Hz | 300 Hz | 1 kHz | 3 kHz |
|---|---|---|---|---|---|---|
| Sealed (default) | 121.0 dB | 118.8 dB | 118.2 dB | 118.4 dB | 123.0 dB | 115.2 dB |
| Slight leak | 109.9 | 114.0 | 117.7 | 119.2 | 123.0 | 115.2 |
| Loose | 87.5 | 94.6 | 104.4 | 124.8 | 123.1 | 115.3 |

Sealed, the IEM is a pressure chamber: the drum level is flat within 1 dB
from 50 Hz to a third of the in-situ resonance (the input impedance's local
maximum near 1.3 kHz; tested). The relief vent lifts the level below its corner near
25 Hz. Loose, the bass is lost below the leak's resonance with the coupler
(near 300 Hz). The front volume and nozzle resonate near 5 kHz, where the
input impedance has its maximum (16.3 Ω); a nozzle mesh damps it (200 rayl:
the level falls smoothly from 118 dB at 100 Hz to 112 dB at 3 kHz). Without
its mesh the vent relieves the rear volume up to a few hundred hertz and
the bass rises 9 dB.

The predicted level, 118 dB SPL at 1 mW, is above the 100–110 dB/mW that
product sheets of 16 Ω dynamic in-ear headphones commonly state (not
surveyed here). It is not a solver artefact. In the pressure chamber the
diaphragm moves x = Bl·i/(k_s + ρc²·Sd²·(1/V_front + 1/V_rear)), and the
pressure is p = ρc²·Sd·x/V_front, which is Bl·i/(Sd·(1 + V_front/V_rear))
when the suspension's stiffness k_s is small. With i = 8.8 mA,
V_front = 1.42 cm³ (the 0.1 cm³ front volume, the nozzle's 0.02 cm³ and
the coupler's 1.30 cm³ at 100 Hz) and V_rear = 0.65 cm³ (the air space and
the rear volume, joined through the damping, whose 3 MPa·s/m³ is small
against the rear volume's reactance below about 1.5 kHz), this gives
117.8 dB; the engine gives 118.2 dB. Two choices set the level: a driver
from an earbud that plays into a large leak, and the template's estimated
0.65 cm³ of rear air. With Poldy's 0.15 cm³ as the only, sealed rear
volume the same formula gives 107.6 dB. Neither is adjusted to a level;
the rear volume is the estimate to question first. Not modelled: the tip's own
compliance and mass, the compliance of a real canal wall, diaphragm
break-up, and balanced-armature drivers (out of the spec's scope).

## Verification

`crates/acoustilab/tests/templates.rs`:

- **Corners.** Every `examples/design_*.json` solves with finite probe
  values at L0 and L1: all parameters at their minimum and all at their
  maximum for every combination of choices, each parameter alone at either
  bound, 64 random corners, and the densest grid: over 600 solves over the
  three templates, at 6 points per octave from 10 Hz to 20 kHz (96 for the
  densest grid). This checks robustness only: a finite result can still be
  wrong.
- **Inactive parameters.** For every combination of choices, each parameter
  `parameters()` reports inactive is moved to both bounds, and every probe
  value stays bit-identical. The topology switches named above are checked
  by name.
- **Conventions.** Labels, groups, bounds, tolerance sources, and a `ui`
  block whose probe, ear-load choice and sketch bindings exist.
- **Hand-written networks.** Each template against a lumped network written
  out in the test from this page's description: the driver from its primary
  set; adiabatic compliances with the first-order thermal wall layer; slits
  (tanh form) and tubes (Bessel form, by power series) as incompressible
  thermoviscous ducts; textbook end corrections and radiation loads; meshes
  as R_s/A; the ear's input impedance from the engine. This checks that each
  netlist is wired as described and that its parameters reach the right
  elements. It is not an independent check of the element physics: the
  oracle uses the engine's element conventions (the wall layer, the wall
  area it assumes for a volume-only cavity, the end corrections and end
  resistances), which the element tests check against their sources. The
  front (or canal-entrance) pressure agrees within
  0.01 dB at L0 from 20 Hz to 2 kHz (observed: 1e-6 dB sealed, 0.003 dB with
  the slit leaks, whose end correction the oracle takes from the wide-slit
  asymptote) and within 0.05 dB at L1 from 20 to 500 Hz (observed 0.033 dB;
  the oracle's Π sections are the first-order form of the depth line and
  the nozzle). Cases: over-ear with and without the damping cloth; on-ear
  sealed, low and high leak; in-ear sealed and loose.
- **Behaviour.** The claims in the tables above: no resonance peak and an
  impedance within 10 % of Re for the damped over-ear, and a peak of at
  least 8 dB between 0.5 and 1.2 kHz without the cloth; the on-ear bass falls
  by more than 10 dB from sealed to low leak and again to high leak, with the
  slit carrying most of the flow; the sealed IEM flat within 1 dB, the fit
  states losing bass in order.

`web/tests/templates.spec.ts` opens the on-ear and in-ear templates in
Design mode and checks their sketches against the parameters (docs/web.md,
"Tests").

## Sources

- Bose Corporation, US 6,597,792 B1, "Headset noise reducing" (R. Sapiejewski,
  M. J. Monahan). <https://patents.google.com/patent/US6597792>
- Bose Corporation, US 2014/0294222 A1, "Headset porting".
  <https://patents.google.com/patent/US20140294222A1/en>
- Bose Corporation, US 8,111,858 B2, "Supra-aural headphone noise reducing"
  (R. Sapiejewski, filed 2009). <https://patents.google.com/patent/US8111858>
- Focal-JMlab, US 12,513,448 B2, "Audio headset with active noise reduction"
  (S. Rezaei, filed 2021). <https://patents.google.com/patent/US12513448B2/en>
- AKG Akustische und Kino-Geräte, US 4,389,542, "Orthodynamic headphone"
  (R. Görike, 1983). <https://patents.google.com/patent/US4389542A/en>
- Fraunhofer-Gesellschaft, US 11,039,243 B2 (F. Leschka et al., filed 2016).
  <https://patents.google.com/patent/US11039243B2/en>
- Sennheiser electronic, US 6,934,401 B2, "Closed headphones with transducer
  system" (A. Grell, K. Kaddig, 2005). <https://patents.google.com/patent/US6934401B2/en>
- Red Tail Hawk Corp. (now Gentex), US 8,638,963 B2, "Ear defender with
  concha simulator" (J. W. Parkins), quoting Burkhard and Sachs.
  <https://patents.google.com/patent/US8638963B2/en>
- Audio Science Review, "Audio Technica ATH-M50X Review (Closed Headphone)",
  2 March 2021, measured on a GRAS 45CA.
  <https://www.audiosciencereview.com/forum/index.php?threads/audio-technica-ath-m50x-review-closed-headphone.20880/>
- ITU-T P.57 (06/2021), clause 6.3.2, Table 3-a and Figure 6, free from
  <https://www.itu.int/rec/T-REC-P.57-202106-I>. Only the derived slit
  sizes are used, with this citation.
- C. Poldy, *Headphone Fundamentals*, tutorial, AES 120th Convention, Paris,
  2006 (Philips Sound Solutions), chapter 3, Figs. 17, 20 and 21. Read in a
  copy hosted by a third party; not checked against an AES copy.
- K. A. Groon, D. M. Rasetshwane, J. G. Kopun, M. P. Gorga, S. T. Neely,
  "Air-leak effects on ear-canal acoustic absorbance", Ear and Hearing 36(1),
  155–163 (2015). <https://www.ncbi.nlm.nih.gov/pmc/articles/PMC4272628/>

Patents state what their applicants built or claim, not measured
population data; they are used here for orders of magnitude only.
