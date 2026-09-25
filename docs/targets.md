# Targets, smoothing, response metrics and preference scores

This note covers spec Section 11 (targets, scoring and preference models), the
"Automated readouts" of Section 10 (error statistics against the selected
target) and left-right tracking from Section 12. It lists every target's
provenance and licence, states the fixture rule, and defines the grid,
smoothing, normalisation, metrics and scores exactly, with their sources.

| | |
|---|---|
| Code | `crates/acoustilab/src/targets/` (`curve`, `grid`, `smoothing`, `shelf`, `fixture`, `target`, `import`, `metrics`, `scores`) |
| Data | `data/targets/`: `ravizza2023_5128.json`, `fixtures.json`, `preference_models.json`, `bs708.json`, `personalisation.json` |
| Tests | `crates/acoustilab/tests/targets.rs`, `crates/acoustilab-wasm/tests/targets.rs`, `crates/acoustilab-cli/tests/score.rs` |
| Generators | `tools/targets/ravizza2023.py` (bundled data), `reference.py` (independent implementation), `autoeq_crosscheck.py` (AutoEq run) |
| Errata | E6, E7, E36; new: E47–E49 (`docs/spec-errata.md`) |

Every empirical number (model coefficients and bands, mask breakpoints, shelf
settings, listener classes, band widening, curves) is read from
`data/targets/*.json`, where it carries its source. The code holds only
definitions.

## The fixture rule

Every published target is a drum-reference-point curve on one fixture, and a
response measured or simulated on another fixture differs from it by several
dB above 2 kHz. So a target always names its fixture, and so does a response.
Loading a target for a response on another fixture is allowed, but the report
then carries a `fixture_mismatch` flag the interface must show.

Fixtures (`data/targets/fixtures.json`):

| id | fixture | ear simulator | pinna | engine ear load |
|---|---|---|---|---|
| `iec60318_4` | IEC 60318-4 simulator, bare | IEC 60318-4 | none | `iec60318_4` |
| `iec60318_4_canal_extender` | GRAS RA0045 with GR0433 canal extender (Harman in-ear) | IEC 60318-4 | none | – |
| `p57_type3_3` | ITU-T P.57 Type 3.3 (60318-4 with 10 × 7.5 mm extension) | IEC 60318-4 | none | `type33` |
| `gras45ca_harman` | GRAS 45CA, RA0045 couplers, Harman custom pinnae (E6) | IEC 60318-4 | Harman | – |
| `gras45ca10_kb5000` | GRAS 45CA-10 with KB5000 pinnae (Olive and Clark 2025) | IEC 60318-4 | KB5000 | – |
| `bk5128` | B&K HATS 5128 (P.57 Type 4.3 ear, anthropometric head) | P.57 Type 4.3 | 5128 | – |
| `p57_type4_3` | P.57 Type 4.3 canal and drum, no pinna or head | P.57 Type 4.3 | none | `type43` |
| `human_ear_model` | modelled canal and eardrum | – | none | (tag explicitly) |

- **Matching.** Two tags `match` when equal. They are `same_ear_simulator`
  when they share the ear simulator but differ in pinna, head or canal
  extension, and `different` otherwise. A user's own tag matches only itself.
- **Inference.** A pressure probe on an internal node of an ear macro
  (`ear.drp`, `ear.eep`, `ear.ref`) belongs to the fixture that lists that
  macro's type. The design template's `p_drp` is therefore `iec60318_4` or
  `p57_type4_3`, depending on its `ear` parameter.
- **Consequence.** The engine models an ear simulator without pinna or head,
  so a simulated response is never on a head and torso simulator: against a
  5128 or 45CA target it is always flagged.
- **Harman fixture.** Olive's 2022 article (Acoustics Today 18(1), p. 65) says
  the Harman over-ear measurements used "an ear simulator according to IEC
  60318-1 (2009) equipped with a custom pinna". This project follows erratum
  E6 (RA0045, i.e. IEC 60318-4, on a 45CA with custom pinnae, after Olive and
  Clark 2025). Listen's sequence note also names the 45CA with custom pinna.

## Target objects

A target (`targets::target::Target`, JSON from `Target::to_json`) has these
fields:

- `name`, `label`, `version`;
- `family`: the lineage a preference model's training pair is checked
  against;
- `group` and `primary`: for display;
- `fixture`, `reference_point` (`drp`), `baseline` (what the levels are
  relative to);
- `normalisation_Hz` (500) and `valid_range_Hz`;
- `provenance`: `class`, `source`, `doi`, `url`, `licence`, `retrieved`,
  `attribution`;
- `flags` and `notes`;
- the curve, `frequencies_Hz` and `dB`;
- `extra` for dataset-specific data such as ratings.

Provenance classes:

| class | meaning |
|---|---|
| `peer_reviewed` | reviewed as a complete manuscript |
| `research` | published research reviewed on a summary only (AES Express Papers) |
| `manufacturer` | from a manufacturer |
| `community` | community curve, not peer-reviewed |
| `user` | supplied by the user |

Flags used:

| flag | meaning |
|---|---|
| `approximation` | parametric approximation, not published data |
| `adaptation` | the depositors' adaptation of a third-party curve |
| `third_octave` | third-octave resolution |
| `selection` | chosen from a set by this project's rule |
| `data_anomaly` | probable error in the source data, kept as published |
| `user_supplied` | supplied by the user |
| `shelf_q_assumed` | the shelf Q is assumed, not published |

### Bundled: Ravizza et al. 2023, B&K 5128 (CC-BY-4.0)

**Source.** G. Ravizza, J. Villegas, C. P. Volk and T. Stegenborg-Andersen,
"Preference ratings and 32 magnitude frequency response curves", Zenodo,
v1.0, 2023-09-29, doi:10.5281/zenodo.8388242. The companion paper is "An
over-ear headphone target curve for Brüel & Kjær head and torso simulator
type 5128 measurements", Proc. 155th AES Convention, 2023 (Express Paper 127,
AES e-library 22281).

**Licence.** CC-BY-4.0. This was read from the record's own metadata
(`license.id = "cc-by-4.0"` at `https://zenodo.org/api/records/8388242`,
2026-09-25). The files' MD5 sums match the record's
(`MagnitudeFrequencyResponses.csv` 70dc33df…, `PreferenceRatings.csv`
51318a6f…). Attribution is kept in every target object and in the data file.

**Content.** 32 curves on the HATS 5128C. Each curve is given as 30
graphic-equaliser band gains at the band centres printed in the CSV header
(31 Hz to 25 kHz), normalised to 0 dB at the largest band. The record
also holds 16 128 preference ratings: 56 naive assessors (30 Danish, 26
Japanese), 11 programmes (9 per assessor: 7 common and 2 per country), a
0–100 scale, 8 curves per trial page.

**Bundled form.** `tools/targets/ravizza2023.py` copies the gains verbatim and
summarises the ratings per curve (n, mean, SD, median, per-country means,
rank). The raw ratings are not copied; the script regenerates the summary
from the record.

**Representation.** The record does not describe the equaliser's filters. A
curve is therefore its band gains at those frequencies, read linearly in dB
on log frequency. The resolution is third-octave (flag `third_octave`), and
the valid range is 31 Hz–20 kHz.

**Targets.**

- `ravizza2023:<id>` for each curve.
- `ravizza2023_5128` for the recommended one: APHarm2018v2, which has the
  highest mean rating (62.7). SoundGuys' report of the paper (URL in the
  data file) independently names the modified Harman 2018 curve as ranked
  first and the SoundGuys curve as 10th of 32; the summary reproduces both.
  The paper itself is paywalled and was not read, so this designation is
  this project's (flag `selection`). The ratings are repeated measures, so the
  means are descriptive, not the paper's mixed-model estimates. The top five
  means lie within 1.9 points (62.7, 62.4, 61.5, 61.0, 60.8) against a
  per-curve SD of about 30 over 504 ratings, so the first place is a
  convention, not an established winner; the second is a modified measured
  headphone (HP3Mod1).

**Provenance class.** `research`: AES Express Papers are reviewed on an
extended summary, not as a complete manuscript.

**Adaptations.** APHarm2015/2015v2/2018/2018v2 (Harman over-ear targets
adapted to the 5128), DF5128 and FF5128 (diffuse- and free-field curves for
the 5128) and Soundguys are the depositors' adaptations of third-party
curves. They publish them under CC-BY-4.0. These curves are flagged
`adaptation`; they are not the originators' data.

**Anomalies, kept as published and flagged `data_anomaly`.**

- HP6, HP6Mod1 and HP6Mod2 have +17.3 dB in the 20 kHz band between −17.3 dB
  neighbours, probably a sign error.
- Soundguys has 0.0 dB at 25 kHz after −18.1 dB, probably a missing value.
- The ratings file spells AVGAllMeas as AvgAllmeas.

### Not bundled

- **Harman around-ear/on-ear (2013, 2015, 2018) and in-ear (2017, 2019).**
  These are peer-reviewed, but the data are not redistributable. See the
  Harman-style reconstruction below, and import a curve you hold the rights
  to.
- **Diffuse- and free-field targets per ISO 11904-2.** ISO tables are not
  committed. The standard's data cover only 20 Hz–10 kHz (E36), so an
  imported ISO-based curve should carry that valid range.
- **Olive and Clark 2025** (45CA-10 and 5128). Data availability is
  unconfirmed.
- **Community 5128 and 60318-4 targets.** Import only, never bundled (spec).

### CSV import (`targets::import::import_target_csv`)

This minimal parser reads target curves only; the general curve importers
live elsewhere and can absorb it later.

```text
# name: My target
# fixture: bk5128
# licence: CC-BY-4.0
# source: where it comes from
frequency_Hz,dB
20,0.5
25,0.6
```

- **Tags.** `# key: value` lines set `name`, `fixture`, `label`, `family`,
  `licence` (or `license`), `source`, `url`, `reference_point` and
  `baseline`. Other comments are free text, and a tag given twice is an
  error.
- **Fixture.** The fixture is required, in the file or as an argument; if
  both are given they must agree.
- **Rows.** Each row holds exactly two numbers, separated by a comma,
  semicolon, tab or spaces. One text header row is skipped, frequencies must
  increase strictly, and errors name the line.
- **Result.** An imported target has provenance class `user` and flag
  `user_supplied`. Its licence is "not stated" unless tagged.

### Personalisation shelves

Bass and treble shelves are first-class personalisation parameters, stored
separately from the reference target (`Shelves`, JSON `personalisation`).
They are the analog prototypes of the RBJ Audio EQ Cookbook shelving filters,
evaluated as magnitude responses. With `s = j·f/f_c` and `A = 10^(G/40)`:

```
low shelf:  H(s) = A·(s² + (√A/Q)·s + A) / (A·s² + (√A/Q)·s + 1)
high shelf: H(s) = A·(A·s² + (√A/Q)·s + 1) / (s² + (√A/Q)·s + A)
```

- **Shape.** A shelf is G dB on its shelf side, 0 dB on the other, and
  exactly G/2 dB at f_c for any Q. The RBJ digital biquads are the bilinear
  transforms of these prototypes and agree with them away from Nyquist.
- **Corners.** 105 Hz and 2.5 kHz, the frequencies of Olive, Welti and
  McMullin (AES 135th, 2013) and of Olive and Welti (AES 139th, 2015). Those
  papers were not read. The frequencies, the reasons for them, and the fact
  that the papers publish no Q come from the PEQdB white paper (2025,
  section 1.2), which quotes the 2013 paper.
- **Q.** Q = 1/√2 (RBJ shelf slope S = 1, the steepest monotonic shelf) is an
  assumption, flagged where it matters. AutoEq uses 0.7.

### Harman-style reconstruction (`reconstruct_harman_style`)

```
target(f) = baseline(f) + low_shelf(f; 105 Hz, G_bass, Q) + high_shelf(f; 2.5 kHz, G_treble, Q)
```

**What the user must supply.** The baseline is the response of the fixture
the target is for (for example `gras45ca_harman`, tagged as such) to an
accurate loudspeaker equalised to a flat in-room response in a reference
listening room, measured at the fixture's drum reference point. In Harman's
method (Olive and Welti 2015, slides 7–11) that was a Revel Performa F208 in
the Harman reference room, equalised flat over 9 microphone positions and
averaged over three head angles. Neither this baseline nor a diffuse-field
response of the 45CA is published as data, so neither is shipped.

**Preset `olive_welti_2015_mean`.** +6.44 dB bass and −1.41 dB treble. These
are the mean preferred levels of 249 listeners, relative to that
equalisation (Olive and Welti 2015, presentation slides 19–20, posted by the
author, read from the Internet Archive).

**What the result is not.** It is flagged `approximation` and
`shelf_q_assumed`, and has family `harman_style_reconstruction`, so no
preference model ever treats it as its training target. It approximates the
2015-era Harman over-ear target on the user's baseline. Harman's 2018 target
also lowered the region near 3 kHz, which no shelf reproduces. No in-ear
recipe is given: the in-ear targets add about 4 dB more bass and changes
above 1 kHz, and no public parameters describe them (US 2019/0087739 A1;
Olive 2022, p. 59).

**Stored resolution.** The result is stored at the baseline's points plus
every base-10 1/48-octave point in range. Reading it between points is off
by less than 1e-3 dB.

### Preference bands ("bounds, not lines")

Listener classes (Olive, Acoustics Today 18(1), 2022, p. 63):

| class | share | bass relative to the target |
|---|---|---|
| prefer the target as is | 64 % | 0 dB |
| more bass is better | 15 % | +4 to +6 dB |
| less bass is better | 21 % | −2 dB |

- **Band edges.** Each class variant is the target plus a 105 Hz bass shelf
  of the class gain, normalised like the metrics. The band is their
  envelope, so it runs from −2 dB to +6 dB in the bass and collapses to a
  few tenths of a dB near 1 kHz.
- **Widening.** Each ramp is linear in dB on log frequency (engineering
  parameters, overridable):
  - above 2 kHz: 0 dB at 2 kHz rising to ±5 dB at 4 kHz. This is the spec's
    "main ear resonance varies by about 2 kHz and 10 dB across people" read
    as ±5 dB. The spec gives no source and none was found;
  - above 8 kHz: a further 0 to ±3 dB between 8 and 16 kHz, for "fixture
    validity". The spec gives no number; 3 dB is an estimate.
- **Reference.** The band is built around the reference target, without
  personalisation.

## Evaluation grid, interpolation, smoothing, normalisation

**Grid.** `f_k = 10^(k/40)` Hz, k = 52…172: 121 points from 19.95 Hz to
19.95 kHz. This is `1000·G^(x/12)` with the IEC 61260-1 base-10 octave ratio
`G = 10^(3/10)`, the exact values behind the rounded R40 frequencies
(20, 21.2, 22.4 … 20 000 Hz). Listen's SoundCheck Harman template expects
"R40 (1/12 octave)" data, and AutoEq evaluates on the rounded values. The
grid contains 100 Hz, 1 kHz and 10 kHz but not 500 Hz (501.19 Hz).
IEC 61260-1's own even-b midbands sit half a band away. A custom grid
(`grid_Hz`) can be passed.

**Band membership.** A band `[lo, hi]` holds the grid points within half a
1/12-octave step of it: `lo·2^(−1/24) ≤ f ≤ hi·2^(1/24)`. On the default
grid this is membership by nominal frequency: 20 Hz selects 19.95 Hz, 8 kHz
7.94 kHz, 16 kHz 15.85 kHz. On a coarser custom grid, no point further
outside a band is ever counted. A statistic is `partial` when the points it
used stop more than half a 1/12-octave step short of a band edge.

**Interpolation.** Linear in dB on log frequency between samples. Up to
1.3 % beyond a curve's ends (or a target's valid range) a curve reads its
end value; further out a grid point has no value, and nothing is
extrapolated. The 1.3 % is the largest gap between a nominal R40 frequency
and the exact grid point it names (17 000 Hz against 16 788 Hz), so a curve
from 20 Hz covers the grid point named 20 Hz (19.95 Hz), and a response and
target from 20 Hz to 20 kHz cover the whole grid.

**Smoothing** (`smooth_power`, 1/N octave, N ∈ {1, 2, 3, 6, 12, 24, 48}, or
none). At each frequency f_i of the curve:

```
S_i = 10·log10( (1/2h_i) ∫ P(x) dx  over [x_i − h_i, x_i + h_i] ),   x = log10 f
P   = 10^(L/10), linear in x between samples
h   = 3/(20N) decades  (the IEC 61260-1 base-10 1/N-octave band, edges f·G^(±1/2N))
h_i = min(h, x_i − x_0, x_{n−1} − x_i)
```

- **Integral.** Exact (trapezoids over the window's pieces). The segments'
  integrals are kept in a sum tree, so a dense curve (an FFT measurement
  with 65 536 points) is smoothed in O(n log n), in about 30 ms natively.
- **Ends.** Near the ends the window shrinks symmetrically, so a sloped
  response is not tilted; the two end points keep their levels.
- **Where it is used.** In the metrics, smoothing is applied to each ear's
  curve before sampling (`smoothing`, default none). Tracking defaults to
  third-octave, because BS.708 measures in third-octave bands.

**Complex smoothing** (`smooth_complex`) averages the real and imaginary
parts over the same window. It is a different operation: where the phase
turns inside the window, the complex mean falls below the power mean. Use it
only when phase must be kept.

**Normalisation** (spec Section 11, compensation pipeline):

- `{"at_Hz": 500}` (the default) subtracts the level at 500 Hz, read between
  grid points linearly on log frequency;
- `{"band_mean_Hz": [200, 500]}` subtracts the mean over the band's grid
  points.

Response and target are normalised the same way. The level subtracted from
the response is reported as its `reference_level_dB`, with the drive, because
every comparison states its reference level. Loudness alignment per ITU-R
BS.1770 is not implemented (it applies to programme signals, not to static
curves).

## Error metrics (spec Section 11, "never one number")

The pipeline:

1. Each ear is optionally smoothed and sampled on the grid.
2. Left and right are averaged **in dB** where both exist. This is the
   patent's "average magnitude response of the left and right channels"; dB
   and power averages differ by less than 0.03 dB when the ears differ by
   less than 1 dB.
3. Response and target are normalised.
4. The error is `e_k = R_k − T_k`, where T includes any personalisation
   shelves.

Each statistic is over the grid points of a band that have a value (n
points, frequencies f_k):

| metric | definition |
|---|---|
| RMS error | `sqrt(Σe²/n)` |
| mean error | `Σe/n` |
| mean absolute error | `Σ|e|/n` |
| standard deviation | `sqrt(Σ(e − ē)²/(n − 1))` (sample, ddof = 1, E7) |
| slope | `b = Σ(u − ū)(e − ē)/Σ(u − ū)²`, `u = ln f` (dB per neper of frequency); `|b|` is the models' AS; `b·ln 2` is reported as dB/octave |
| max \|e\| | with its frequency |

Bands:

| band | metrics reported |
|---|---|
| 20 Hz–10 kHz | all (main, as in the Section 10 readouts) |
| 10–20 kHz | all, reported separately (no preference model is validated above 10 kHz) |
| 20–200 Hz, 200 Hz–2 kHz, 2–8 kHz, 8–20 kHz | band-limited RMS and mean |

Every statistic states its band, the range it actually used, n, and whether
it is partial.

**Mask compliance.** This is the percentage of grid points in a mask's range
where the error lies inside the mask, with the worst excursion. Two masks
are applied:

- **ITU-R BS.708.** The mask is ±2 dB at 100 Hz, ±1.5 dB at 500 Hz,
  ±1.5 dB at 4 kHz and ±4 dB at 16 kHz, linear in dB on log frequency
  between them and undefined outside 100 Hz–16 kHz. These breakpoints were
  read from Figure 1 of Rec. ITU-R BS.708 (1990), which gives the mask only
  as a drawing. The lines were traced from the rendered PDF against the
  figure's grid (`data/targets/bs708.json` records the readings, which
  match the log-linear model within the reading accuracy of about 0.1 dB).
  The drawn lines continue flat to about 88 Hz and 18 kHz, the outer edges
  of the 100 Hz and 16 kHz third-octave bands; the mask is applied between
  the band centres. The spec's "±2 dB below about 250 Hz" does not match
  the figure: at 250 Hz the limit is 1.71 dB (E47).
  BS.708 applies the mask to diffuse-field responses of studio monitors
  measured on 16 subjects; here it is an indicator on the error against the
  selected target.
- **Preference band.** The band above, over 20 Hz–20 kHz, applied to the
  error against the reference target.

**Left-right tracking** (BS.708 recommends 3: "should not exceed 1 dB in the
frequency range 100 Hz–8 kHz and 2 dB in the frequency range 10 kHz–16 kHz").
The quantity is `L − R` on the grid, not normalised, since a level mismatch
is part of tracking. Each ear is third-octave smoothed by default. For each
band the report gives pass/fail, the largest |L − R| and where, and the
percentage within the limit. The 8–10 kHz gap, where BS.708 states no limit,
is reported as a plain maximum. The tracking response of IEC 60268-7:2025 is
not implemented (paid standard).

Bass extension relative to the 500 Hz level belongs to the readouts package
and is not computed here.

## Preference scores (spec Section 11, E7)

`score = intercept − Σ weight·variable`. The variables are computed on the
error of the left-right average against the **reference** target (no
personalisation), both normalised at 500 Hz as the models define. SD and AS
are as above; ME is the mean |e|.

| model | formula | bands | fit | source |
|---|---|---|---|---|
| `harman_oe_2018` (around-ear, on-ear) | 114.49 − 12.62·SD − 15.52·AS | SD, AS: 50 Hz–10 kHz (E7) | r = 0.86, RMSE 6.7 | US 2019/0087739 A1 Eq. (5), Table 2; model paper AES 144th 2018, paper 9919 |
| `harman_oe_2018_20Hz` (same, other band reading) | 114.49 − 12.62·SD − 15.52·AS | SD, AS: 20 Hz–10 kHz | as above | the patent's variable definitions, and the spec's band; not the default |
| `harman_ie_patent` (in-ear) | 68.685 − 3.238·SD − 4.473·AS − 2.658·ME | SD, AS: 20 Hz–10 kHz; ME: 40 Hz–10 kHz | r = 0.91, RMSE 5.5 | US 2019/0087739 A1 Eq. (1)–(4), Table 1, claim 9 |
| `harman_ie_listen` (in-ear) | 100.0795 − 8.5·SD − 6.796·AS − 3.475·ME | as above | not published | AutoEq `harman_inear_preference_score` (MIT), stated there to reproduce Listen's Excel template (E7) |

What was verified, and against what:

- **The patent.** US patent application 2019/0087739 A1 (Harman; Olive,
  Welti, Khonsaripour; priority 2017-09-15, published 2019-03-21) was read in
  full text (Google Patents). It gives both equations, r and RMSE, and the
  bands: SD and AS from 20 Hz to 10 kHz, ME from 40 Hz to 10 kHz. It
  measures "20 Hz to 20 kHz in 48 log spaced points", optionally smoothed
  to 1/12 octave.
- **Olive 2022.** The Acoustics Today article gives r = 0.86 and an error of
  6.7 points for the over-ear model, but no coefficients. The spec's
  attribution of the coefficients to it is corrected in E48.
- **The over-ear band.** This is the least certain part. The patent
  defines SD and AS on 20 Hz–10 kHz, and says the same variables were the
  candidates for the AE/OE model. E7 and AutoEq use 50 Hz–10 kHz. AutoEq
  says its implementation gives "the exact same numbers as the Excel from
  Listen Inc", which was built from the authors' own spreadsheets (Listen
  sequence note), and exact agreement would be impossible with a different
  band. The model paper (AES 144th, paywalled) was not available to settle
  it. This project follows E7 in `harman_oe_2018` and reports the literal
  patent reading as `harman_oe_2018_20Hz`, because the choice matters: on
  the synthetic test curves the two differ by about 10 points, in either
  direction (a bass shortfall below 60 Hz lowers the 20 Hz score).
- **The coefficients.** AutoEq uses the unrounded 114.490443008238 and
  15.5163857197367. The rounded patent values used here differ by
  0.0004 + 0.0036·AS points.
- **Training pairs.** Over-ear: `gras45ca_harman` with the Harman AE/OE 2018
  target (family `harman_ae_oe_2018`). In-ear: `iec60318_4_canal_extender`
  with the Harman in-ear target of 2017 (`harman_ie_2017`), per the Listen
  note.

**Greying rules.** A score is `greyed` when any of these holds:

| flag | condition |
|---|---|
| `training_fixture_mismatch` | the response's fixture is not the model's training fixture, or is unknown |
| `training_target_mismatch` | the target's family or fixture differ from the training target |
| `partial_range` | the data do not cover a variable's band (the Ravizza set starts at 31 Hz, so the in-ear models are always partial against it) |

These flags are notices and do not grey a score:

| flag | meaning |
|---|---|
| `simulated` | "simulated, not measured": always, for engine results (`measured: true` only for imported measurements) |
| `coupler_extrapolated` | the response is on an IEC 60318-4-based fixture and a band starts below 100 Hz; the 20–100 Hz band is flagged (the report also carries `coupler_extrapolated_Hz: [20, 100]`) |
| `outside_scale` | the value falls outside the 0–100 rating scale the linear model was fitted to |

A greyed score keeps its value. In this tool today, every score of a
simulated design is greyed when its fixture is inferred from the ear load,
because no engine ear load is a training fixture. A score is un-greyed
only for a response tagged with the training fixture, against a target the
user holds the rights to. For the over-ear model that means a CSV tagged
`# fixture: gras45ca_harman` and `# family: harman_ae_oe_2018`, and a
response scored with `fixture: gras45ca_harman` (and `measured: true` for a
measurement). The fixture option is the caller's statement: a simulated
response tagged with the training fixture by hand is not greyed, but it
still carries the `simulated` notice, which the interface must show.

**Grid sensitivity.** On the synthetic test case (a response with a 7 dB
notch 0.15 octave wide), evaluating on AutoEq's rounded R40 grid instead of
the exact one moves the over-ear score by 0.37 points and the in-ear score by
0.19 points. That is small against the models' 5.5–6.7 point error, but
larger than rounding. To reproduce SoundCheck or AutoEq numbers exactly,
pass their grid as `grid_Hz`.

## Interfaces

### Rust

- `targets::evaluate(&Response, &Target, &Options) -> Report`. A
  `Response` holds the left curve, an optional right curve, the fixture, a
  label and the drive. `Report::to_json()` gives the JSON below.
- `targets::target::{bundled, find, resolve, reconstruct_harman_style,
  recipe_preset, preference_band, Shelves, BandParams}`,
  `targets::import::import_target_csv`,
  `targets::smoothing::{smooth_power, smooth_complex}` and
  `targets::fixture::{for_probe, compare}`.

### WebAssembly (`crates/acoustilab-wasm/src/targets.rs`)

Every export takes and returns JSON text. Errors are
`{"error", "kind"}`, with kind `target`, `options`, `curve`, `csv` (plus
`line`), `input`, or an engine kind.

| export | returns |
|---|---|
| `targets_list()` | `{"targets": [{name, label, group, primary, fixture, fixture_label, family, flags, licence, provenance_class, valid_range_Hz}], "fixtures": [..], "models": [{id, label, kind, training, source}], "smoothing_fractions": [1, 2, 3, 6, 12, 24, 48]}` |
| `target(spec)` | the target object; `spec` is a bundled name or a target object |
| `target_curve(spec, options)` | `{name, label, fixture, flags, normalisation, personalisation, grid_Hz, reference_dB, target_dB, band_lower_dB, band_upper_dB, band: {classes, widening, source, widening_basis}}` |
| `target_metrics(input, target_spec, options)` | the report (below) |
| `smooth(curve, fraction)` | `{"frequencies_Hz", "dB", "smoothing", "method": "power"}`, or `re`/`im` with `"method": "complex"` when the curve has `re` and `im`; `fraction` is `"none"`, `"N"` or `"1/N"` |
| `import_target_csv(text, fixture, name)` | the target object (`fixture`, `name` may be `""`) |
| `probe_fixture(netlist, overrides, probe)` | `{"probe", "fixture": id or null, "fixture_label"}` |
| `reconstruct_target(baseline_spec, options)` | the reconstructed target; options `{"preset": "olive_welti_2015_mean"}` or shelf keys, and `"name"` |

`target_metrics` inputs:

- **`input`.** A `solve` result document (choose the pressure probe with
  `options.probe` unless there is only one), a curve
  `{"frequencies_Hz", "dB" | "spl_dB"}`, or `{"left": .., "right": ..}` of
  either.
- **`options`.** All keys are optional, and unknown keys are rejected.

| key | value |
|---|---|
| `probe` | the pressure probe to read |
| `fixture` | the response's fixture (use `probe_fixture`; a result document does not carry it) |
| `measured` | `true` for imported measurements (refused for a `solve` result, which is always simulated) |
| `normalisation` | `{"at_Hz": f}` or `{"band_mean_Hz": [lo, hi]}` |
| `smoothing`, `tracking_smoothing` | `"none"`, N or `"1/N"` (defaults `"none"` and 3) |
| `personalisation` | `{"bass_dB", "treble_dB", "bass_fc_Hz", "treble_fc_Hz", "bass_Q", "treble_Q"}`; gains ±40 dB, corners 1 Hz–100 kHz, Q 0.1–10 |
| `grid_Hz` | an increasing array |
| `band` | `{"above_2kHz_dB", "above_8kHz_dB"}` (widening half-widths) |

Report (`Report::to_json`):

```json
{
  "target": {"name", "label", "fixture", "family", "flags", "licence", "provenance_class", "valid_range_Hz"},
  "response": {"label", "fixture", "fixture_label", "drive", "reference_level_dB", "measured"},
  "fixture_match": "same" | "same_ear_simulator" | "different" | null,
  "options": {"normalisation", "smoothing", "tracking_smoothing", "personalisation", "grid"},
  "grid_Hz": [..], "response_dB": [..], "target_dB": [..], "error_dB": [..], "target_offset_dB": ..,
  "metrics": {
    "main": {"band_Hz", "used_Hz", "n", "partial", "rms_dB", "sd_dB", "slope_dB_per_ln_f", "abs_slope",
             "slope_dB_per_octave", "mean_dB", "mae_dB", "max_abs_dB", "max_abs_at_Hz"},
    "above_10kHz": {..}, "band_rms": [{..} x4],
    "bs708_mask": {"band_Hz", "n", "within", "compliance_percent", "partial", "worst": {"f_Hz", "excess_dB"}},
    "preference_band": {..}
  },
  "preference_band": {"lower_dB", "upper_dB", "about": {"classes", "widening", "source", "widening_basis"}},
  "tracking": null | {"smoothing", "grid_Hz", "difference_dB", "bands": [{"band_Hz", "limit_dB", "pass",
                "max_abs_dB", "max_abs_at_Hz", "compliance_percent", "n", "partial"}], "unspecified_8_to_10kHz", "source"},
  "scores": [{"model", "label", "kind", "score", "greyed", "variables": [{"variable", "value", "weight",
              "band_Hz", "used_Hz", "n"}], "formula", "training", "fit", "source", "flags": [{"code", "message"}]}],
  "coupler_extrapolated_Hz": null | [20, 100],
  "flags": [{"code", "message"}]
}
```

Arrays on the grid use `null` where there is no value. Report-level flags:

| flag | meaning |
|---|---|
| `fixture_unspecified` | the response's fixture is unknown |
| `fixture_mismatch` | the response and target are on different fixtures |
| `target_note` | the target's own flags |
| `simulated` | the response is simulated |
| `coupler_extrapolated` | the response is on an IEC 60318-4-based fixture (20–100 Hz) |
| `partial_range` | the data do not cover all of 20 Hz–10 kHz |
| `personalised` | the error metrics use the personalised target |

### CLI

```sh
acoustilab score examples/design_over_ear.json --target ravizza2023_5128
acoustilab score design.json --target my_target.csv --probe p_drp --smoothing 1/6 --json --set ear=type43
acoustilab score --list
```

**Probe.** `--probe`, else the netlist's `ui.primary_probe`, else its only
pressure probe on an ear's drum reference point, else its only pressure
probe.

**Fixture.** Inferred from the ear load; `--fixture` overrides it.

**Target.** A bundled name, or a fixture-tagged CSV file (`*.csv`).

## Verification

**Independent implementation.** `tools/targets/reference.py` re-implements
every definition above with numpy and scipy, sharing no code with the
engine:

- smoothing by `scipy.integrate.quad` over the piecewise-linear power,
  instead of the engine's exact trapezoid sum; the two agree to 3e-14 dB;
- shelves by `scipy.signal.freqs` from their polynomials;
- slopes by `scipy.stats.linregress`.

It generates `targets_reference.json`:

- the grid, interpolation, band indices and BS.708 limits (1e-12);
- smoothing at five fractions, and complex smoothing (1e-9 dB; 1e-11 of the
  peak magnitude);
- shelves (1e-10 dB);
- four complete reports: sparse and dense targets, two ears with band-mean
  normalisation, 1/6-octave smoothing and shelves, and the bundled target.
  Every error value, statistic, mask count and worst point, band edge,
  tracking value and score is checked (1e-9 dB);
- the Harman-style reconstruction.

Both sides are double precision, so 1e-9 is far above their disagreement
and far below anything that matters.

**AutoEq itself.** `tools/targets/autoeq_crosscheck.py` runs AutoEq 4.1.2's
own score functions:

- On the exact grid, its over-ear SD and slope match the engine to 1e-12,
  and its score matches once its unrounded coefficients are accounted for.
- Given AutoEq's own grid (the R40 series, as `grid_Hz`), the engine
  reproduces AutoEq's in-ear SD, slope and ME to 1e-12 and its in-ear score
  to 1e-9, and its over-ear SD and slope to 1e-12 (the score up to the
  rounded coefficients).
- On the exact grid instead, the grid effect is bounded (0.5 / 0.3
  points).

**Closed forms.**

- The score of an error exactly `a·ln(f/500)` (all four models, 1e-9).
- The shelves: G/2 at f_c for every Q, the asymptotes, and S = 1
  monotonicity.
- Smoothing leaves constants and log-linear power unchanged, and complex
  smoothing leaves log-linear functions unchanged.
- The BS.708 breakpoints and 1.71 dB at 250 Hz.
- Tracking of a 1.5 dB offset.
- The preference band contains every class variant.

**Data.**

- The Ravizza generator checks the record's MD5 sums and the top-ranked
  curve.
- The tests check the APHarm2018v2 row literally, the rank permutation
  against the means, and every curve's size, flags and rating count.

**Interfaces.**

- The wasm and CLI outputs equal the engine's.
- Every documented input form is read, and bad input returns an error
  naming the problem.

## Known gaps

- **Harman targets.** None is shipped, and there is no in-ear parametric
  recipe (see above).
- **Spec Section 11 items not implemented:**
  - the Miller and Downey high-frequency extension for inserts;
  - fixture translation (paired-headphone deltas);
  - ISO 226/532-1 level-dependent balance;
  - the IEC 60268-7:2025 tracking response and effective frequency range;
  - BS.1770 loudness alignment.
- **Band widening.** The widening numbers (±5 dB above 2 kHz, ±3 dB above
  8 kHz) are engineering parameters without a published source.
- **Unread primary sources.** The over-ear model's band (50 Hz, E7) and the
  Listen in-ear coefficient set rest on AutoEq and E7, not on the model
  papers, which were not available.
- **Ravizza curves.** The paper was not read. The curve ids' meanings and
  the designation of APHarm2018v2 rest on the record's description, the
  ratings themselves and a secondary report.
