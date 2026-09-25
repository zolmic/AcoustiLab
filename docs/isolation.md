# Passive isolation, cup-wall transmission and bleed

This note covers spec Section 10 ("Passive isolation", "Bleed") and the
leak and cup-cushion model of Section 8. The code is
`crates/acoustilab/src/isolation.rs` and `crates/acoustilab/src/elements/shell.rs`,
the tests are `crates/acoustilab/tests/isolation.rs`, and the mpmath
reference is `tools/time/isolation_refs.py`
(`crates/acoustilab/tests/data/isolation_reference.json`). The worked
example is `examples/closed_cup_isolation.json`.

## The ambient convention

A netlist has two kinds of acoustic "ground", which coincide in a normal
solve:

| terminal name | means | in isolation |
|---|---|---|
| `ambient`, `a_amb` | the outside air | driven by the outside pressure |
| `gnd`, `a_gnd`, an omitted terminal | the reference, zero acoustic pressure: a cavity's compliance, a simulator's internals | stays at zero |

Name the outer terminal of every path to the outside air `ambient`: leaks,
vents, grilles and their radiation loads, the rear face of an open driver,
and a `shell`. Isolation warns (`undriven_path`) when a `leak`, `vent`,
`radiation`, `mesh`, `perforated_plate`, `membrane_vent`, `porous_layer`,
`shell`, duct, lumped acoustic path, the rear face of a `driver`, or a
`modal_cavity` port ends at the reference instead, because it will not be
driven. A netlist without any ambient terminal gets `no_ambient` and an
infinite loss.

## Method

1. **Occluded ear.** The parameters are resolved and the expanded netlist
   is transformed:
   * every ambient terminal is rewired to a new node `iso_outside`, driven
     by a 1 Pa pressure source (`iso_ambient`) to the reference;
   * every independent source is set to zero with its impedance kept, so
     a `vsource` keeps `Zs_ohm` and the driver still sees its amplifier;
   * the `drive` key is dropped.
   The pressure at the drum-point probe is p_occluded. The outside field is
   simplified to one blocked pressure, equal in phase at every opening, in
   the spirit of a diffuse field.
2. **Open ear.** The ear load alone, driven at its entrance node by the same
   1 Pa: p_open. The ear load is every element reachable from the drum
   point without crossing the entrance node. The open reference ignores
   head and pinna diffraction and the radiation impedance of the open
   entrance: the unoccluded entrance pressure is taken to be the outside
   pressure.
3. **Insertion loss** IL(f) = 20·log10(|p_open|/|p_occluded|), positive when
   the headphone attenuates. The ear is a one-port at its entrance, so its
   own transfer to the drum cancels, and IL equals −20·log10 of the
   occluded entrance pressure. The test checks this identity to 1e-9 dB
   for both ear loads of the design template.

**Drum probe and entrance.** The drum-point probe is `drum_probe`, else
the netlist's `ui.primary_probe`, and must be a `pressure` probe. The
entrance is `entrance_node`, else inferred:

* when the drum node is an internal node of an element (`ear.drp`), that
  element's first terminal (`a_ear`);
* otherwise the first node of a `canal` that ends at the drum node.

If the reachable set contains a source, a driver or an element with an
ambient terminal, the entrance does not separate the ear from the
headphone, and the call is refused.

**Paths.** By superposition, each element with an ambient terminal is
driven alone (the other ambient terminals held at the reference). The
contributions sum to p_occluded to 1e-12, and each is reported as a level
relative to p_open.

**ETSI TS 103 640.** Following V1.4.1 (2026-04), clause 5.1.3 (passive
insertion loss: the difference of the open-ear and occluded levels at the
eardrum, in 1/3-octave bands from 20 Hz to 20 kHz, for each ear), the loss
is also given in the base-ten bands of IEC 61260-1. The mid-band
frequencies are 1000·10^(x/10) with nominal labels 20, 25, 31.5, …, 20 000
Hz, and the band edges sit at 10^(±1/20) of them. Each band holds the ratio
of ∫|p|² df over the band, which is the flat excitation of a white noise.
Only bands lying entirely inside the sweep are reported. The summary gives
what clause 5.1.1 asks for: the largest loss and its band, the range where
the loss is at least 6 dB, and the mean over the bands. The netlist models
one ear.

## Cup-wall transmission: `shell`

A panel between an inside and an outside node:
Z = R + jω·κ·m/A² + 1/(jωC_a).

| key | meaning |
|---|---|
| nodes | [inside, *outside*] (name the outside `ambient`) |
| area | `area_cm2`, `radius_mm` or `diameter_mm` |
| mass | exactly one of `surface_density_kg_per_m2`, `mass_g` (any mass suffix), or `thickness_mm` + `density_kg_per_m3` |
| *`profile`* | `piston` (default, κ = 1), `plate` (κ = 9/5), `membrane` (κ = 4/3) |
| *stiffness* | `resonance_Hz` (C_a = 1/(ω0²·M_a)) or `C_m3_per_Pa`; omit both for a limp panel |
| loss | `Q` (R = sqrt(M_a/C_a)/Q) or `R_Pa_s_per_m3`; required with a stiffness (without one the panel is a short at its resonance) |

κ = A·∫w² dA/(∫w dA)² is the acoustic-mass factor of the deflection shape
w, the same as for `membrane_vent`.

* **`piston`** is the rigid case: the whole cup assembly moving on its
  cushion against the head. Section 8 names this branch. The outside
  pressure pushes the cup, the cushion spring reacts against the fixed
  head, and the cup's motion changes the front-cavity volume: a piston
  between the front cavity and `ambient`. Below its resonance it is
  stiffness-controlled, and the divider of its compliance and the front
  cavity's sets the sealed-cup isolation plateau (Zwislocki 1955, cited by
  Section 8).
* **`plate`** is a wall clamped at its rim, flexing in its fundamental
  mode, w ∝ (1 − r²/a²)².

Above resonance the panel is mass-controlled: |Z| grows 6 dB per doubling
of frequency or of mass. Between a matched source and a matched load
(ρc/A each side) it reproduces the normal-incidence mass law
τ = 1/|1 + jωm_s/(2ρc)|² exactly. The model is the lumped fundamental: it
holds below the panel's next mode (3.89 × the fundamental for a clamped
circular plate's second axisymmetric mode), and the radiation mass of the
air on each face is not included (add a `radiation` element where it
matters).

## The fixture's own ceiling

A measured isolation cannot exceed the fixture's self-insertion loss. For
the GRAS 45CA the manufacturer states more than 50 dB from 80 to 250 Hz,
more than 65 dB from 350 Hz to 4 kHz, and more than 55 dB from 5 to 20 kHz,
measured with closed ear simulators. The spec quotes the first two bands.
The record is `data/fixtures/gras_45ca_self_insertion_loss.json`, with the
URL and retrieval date. The report lists the frequencies where the
predicted loss exceeds those bounds (`fixture_self_insertion_loss.exceeded_at_Hz`):
a measurement on that fixture could not confirm the prediction there. For
the design template these are 5.0–6.9 kHz and 15.9–18.3 kHz.

## Bleed

At the netlist's drive, the net volume velocity U leaving through the
ambient terminals is read from a 0 Pa source on the rewired outside node,
which leaves the solution unchanged. It radiates as a monopole,
|p(r)| = ρ·f·|U|/(2r) (spec Appendix C9), reported at 0.3 and 1 m with a
±6 dB band. The C9 worked example, a sealed 30 cm³ front at 94 dB SPL
radiated from the rear, gives 32.07 dB at 1 m at 1 kHz; C9 quotes 32 dB, and
the same construction gives 60.0 dB at 5 kHz, against C9's 60 dB. The
dipole correction of C9 needs the separation of the front and rear
openings, which a netlist does not carry. The complex sum over the
openings already contains their cancellation.

## Verification

| check | reference | tolerance |
|---|---|---|
| Sealed rigid cup with one slit leak, L0 and L1: occluded pressure and IL (leak-limited isolation) | mpmath divider of the slit's impedance (L0) or ABCD (L1) and the cup compliance | 1e-9 in pressure, 1e-8 dB |
| Low-frequency limit: the Poiseuille divider, and 6 dB/octave above the 16.6 Hz leak corner | closed form | 1e-4 relative; 0.3 dB/octave |
| `shell` impedance for each profile and mass input | closed form | 1e-12 |
| Normal-incidence mass law, 6.02 dB per doubling of frequency and of mass | closed form | 1e-9; 0.002 dB of 20·log10 2 |
| Shell + leak in parallel into the cup; superposition of paths | closed form; sum of paths | 1e-9; 1e-12 |
| Design template, both ears: IL = −20·log10\|p_entrance\|; paths sum | identity | 1e-9 dB |
| Energy balance of the occluded circuit: the outside source's power is absorbed by the network | Tellegen's theorem | 1e-9 of the source power |
| Transformation: zeroed sources keep Zs, no drive, one probe; warnings | — | exact |
| 1/3-octave bands: nominal labels, bands inside the sweep, a constant ratio gives its loss | — | 1e-9 dB |
| Bleed: C9 worked example; the drive scales it | closed form | 0.01 dB |

## Interfaces

```sh
acoustilab isolation design.json [--probe ID] [--entrance NODE] [--csv] [--out FILE] [--set NAME=VALUE]
```

The JSON report (also returned by the wasm export
`isolation(netlist, overrides, options)`, whose options are `drum_probe`,
`entrance_node`, `paths`, `bleed` and `bleed_distances_m`) holds:

* `frequencies_Hz`, `insertion_loss_dB`, and `p_occluded` and `p_open`
  {re, im, spl_dB} (spl_dB for 1 Pa outside, 94 dB SPL);
* `third_octave_bands` [{nominal_Hz, center_Hz, insertion_loss_dB}] and
  `summary` {max_dB, max_at_Hz, range_6dB_Hz, mean_dB};
* `ear` {drum_probe, drum_node, entrance_node, elements};
* `driven`, and `paths` [{element, re, im, level_re_open_dB}];
* `warnings`, `shading`, `parameters`, `fixture_self_insertion_loss`,
  `convention`;
* `bleed` {frequencies_Hz, u_out {re, im}, distances_m, spl_dB [per
  distance], band_dB, drive, model}.

The CSV has the frequency, the IL, both drum SPLs and one column per path.
Native cost for the design template is 56 ms for its four sweeps (the
occluded ear, the open ear, and each of the two paths alone) and 17 ms for
the bleed; in wasm, 87 ms for both.

## Limits

* One blocked pressure, in phase at every opening. Real diffuse fields
  arrive with random phase at openings a few centimetres apart; above
  about 2 kHz the paths no longer add coherently.
* No head, pinna or torso: the open-ear reference is the outside pressure
  at the entrance, not a diffuse-field HRTF.
* The occlusion-effect estimate of Section 10 (the canal wall driven as a
  volume-velocity source) and structure-borne paths are not covered.
* Flanking through the pad's porous material needs a `porous_layer` path to
  `ambient` in the netlist. The template has none.
