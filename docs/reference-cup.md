# The open reference cup and its validation

Spec Section 17 ("Open reference headphone", "First-release thresholds",
"Validation under limited fixture access") and the reference preset of
Section 18: a printable circumaural cup around the Tymphany
HPD-40N16PET00-32, measured on an IEC 60318-4 simulator in undamped and
damped form and on a Type 4.3 fixture with at least five re-seats each, plus
impedance on the fixture and in free air, published under CC-BY with its
geometry. Nothing has been built or measured yet. What exists is everything
a first session needs: the cup, its model, the session's script, blind
predictions that were frozen before any measurement existed, and the tool
that compares a session with them.

| what | where |
|---|---|
| parts, dimensions, STL, bill of materials, print notes, licence | `validation/reference_cup/README.md`, `dimensions.json`, `geometry/` |
| the model | `examples/reference_cup.json` |
| the session | `validation/reference_cup/protocol.json` (read by the tool), `protocol.md` |
| frozen blind predictions | `validation/reference_cup/predictions/v1/` |
| engine | `acoustilab::validation` (`predict`, `session`, `simulate`) |
| command line | `acoustilab validate` |
| tests | `crates/acoustilab/tests/reference_cup.rs`, `crates/acoustilab-cli/tests/validate_cli.rs` |
| CAD tools | `tools/refcup/` (dimensions to OpenSCAD, STL export, wall check) |

## The artefact

A rigid printed cup (PETG, 100 % infill): a baffle that seals the driver
behind a 38 mm lip; a front cavity 66 mm across, closed by a 22 mm rigid pad
ring and a 3 mm closed-cell silicone gasket; a 66 mm rear chamber 20 mm deep
(66.4 cm3 with the retainer and the driver's rear boss taken out); two front
ports in the pad ring and one rear port in the back plate, each taking a
14 mm plug with an O-ring. Rear plugs: `sealed`, `hole` (one open 3.0 mm
hole, 6 mm long) and `mesh` (one 8.0 mm hole covered by SAATI Acoustex 260).
Front plugs: `sealed` and `mesh` (8.0 mm holes, 12 mm long, Acoustex 260).

It is designed for predictable acoustics rather than comfort:

* the defined leak is the pair of meshed front plugs: resistive, as a pad
  leak is, and set by a data-sheet flow resistance rather than by a gap
  (a slit's resistance goes as the gap cubed, and a printed gap of 0.2 mm is
  only good to tens of per cent). The gasket seals on the flat plate; what
  it leaves, the residual leak, is the one leak the acceptance test fits;
* plugs are swapped without lifting the cup, so the states of one seating
  share their leak and front volume and their differences are the plugs'
  alone;
* the joints meet face to face on O-rings, so no gasket thickness enters a
  cavity; the walls are stiff enough to be rigid (below);
* the 66 mm width and 22 mm pad ring keep the notch that the front cavity's
  radial field puts at the ear port near 5 kHz, above the 4 kHz end of the
  acceptance band (68 mm and 18 mm put it at 4.1 kHz, inside it);
* every acoustically relevant dimension is in `dimensions.json`, which is
  also the only source of the CAD and of the netlist's geometry: a test
  checks that the netlist's parameters (values and tolerances) and the
  OpenSCAD include match it.

**Wall stiffness** (`tools/refcup/wall_check.py`, PETG modulus 2.0 GPa, an
estimate): the 6 mm back plate, clamped at its rim, has 0.10 % of the rear
air's compliance and its first resonance near 3.5 kHz; it passes 0.03 % of
the meshed rear vent's flow at 20 Hz and 1.6 % at 1 kHz. Solved with the
back plate as a `shell` element (plate, its 3.5 kHz resonance, Q = 20), the
drum response changes by at most 0.014 dB up to 4 kHz (0.6 dB near 19 kHz)
and the impedance peak by 0.1 %. The walls are rigid for this purpose; the
gasket is not (see the table of what is not modelled).

## The model (`examples/reference_cup.json`)

```
amp ─ driver ─┬─ front modal cylinder ─┬─ ear port (z1 centre) ─ ear load, residual leak
  (vsource,   │  66 mm, depth from the │                          (or the open face: radiation)
  Zs)         │  front volume          ├─ front port 0 (side, 0°)   ─ vent: 8 mm hole + Acoustex 260
              │                        └─ front port 1 (side, 180°) ─ vent (or nothing: sealed)
              └─ rear modal cylinder ──── rear port (z1 centre) ─ vent: 3 mm hole, or 8 mm + mesh, or nothing
```

* **Cavities** are `modal_cavity` cylinders at level 1 (rigid walls, wall
  losses, footprints as rigid pistons: the diaphragm, of area Sd, on the
  driver face; the ear port on the far face; the ports on the side wall).
  The front one's depth is the front volume over the cup's cross-section, so
  fitting the front volume moves its modes. In a 66 mm cup the pressure at
  a 3.8 mm ear port leaves the cavity's mean pressure above about 2 kHz; a
  depth-line cavity has no radial field and would miss the notch near
  5 kHz. At level 0 each becomes one lumped compliance.
* **Loads** (`load`): `iec60318_4` (the engine's literature model on the
  simulator's 3.77 mm entrance), `iec60318_4_damped` (the same model: the
  engine cannot represent the damping, whose construction is unpublished;
  since the damped variant complies with IEC 60318-4 from 100 Hz to 10 kHz,
  its prediction is frozen and compared up to 10 kHz only), `type43` (the
  engine's P.57 Type 4.3 ear at the EEP; the pinna enters only as
  `pinna_volume_cm3` taken from the front cavity), `cup_free_air` (front
  open: the far face is a radiating opening), `driver_free_air` (both faces
  of the bare driver at ambient: the datasheet's own condition).
* **Driver**: the physical set derived from the record's primary set
  (erratum E5: Bl = 2.238 T·m, Cms = 12.62 mm/N, Rms = 0.0569 N·s/m), times
  unit-to-unit factors. The datasheet's fs tolerance (±15 %) is put on the
  suspension compliance (fs ∝ 1/√Cms, so a lognormal factor with 2σ points
  at 1.15² and 1/1.15²), Re carries the datasheet's ±5 %, and Bl, Rms, Mms
  and Sd carry estimates. Sampling fs and Qes of the primary set instead
  would tie Bl to fs (Bl ∝ √fs at fixed Qes), which is not how drivers vary.
* **Configurations**: `load`, `rear_plug` and `front_plug` select every
  state of the protocol; the protocol also sets the nominal leak and pinna
  volume per fixture and the drive.
* **Drive**: 10 µW into the 32 ohm rating (17.9 mV RMS). At 1 mW the open
  rear hole would reach 1.5 m/s on the fixture and 6 m/s in free air, where
  the laminar model no longer holds. At 10 µW no element exceeds its
  operating limits (tested).

### Tolerances and their sources

| parameter | nominal | tolerance (2σ) | source |
|---|---|---|---|
| `driver_Re_ohm` | 32.8 ohm | ±5 % | datasheet |
| `driver_compliance_factor` | 1 | lognormal ×/÷ 1.3225 | datasheet fs ±15 %, put on Cms (assumption) |
| `driver_Mms_g` | 0.30 g | ±5 % | estimate |
| `driver_Sd_cm2` | 10 cm² | ±5 % | estimate |
| `driver_Bl_factor` | 1 | ±5 % | estimate |
| `driver_Rms_factor` | 1 | lognormal ×/÷ 1.2 | estimate |
| `mesh_rayl` (all ports, one sheet) | 260 rayl | ±12 % | SAATI data sheet (no tolerance given); 10–15 % industry practice |
| cup bore, rear depth | 66, 20 mm | ±0.2 mm | build check (FDM XY/Z, estimate) |
| pad ring height and width, back plate, lip | 22, 12, 6, 1.2 mm | ±0.1 mm | build check (estimate) |
| plug holes | 3.0, 8.0 mm | ±0.05 mm | build check: drilled, pin gauges |
| gasket under 5 N | 2.9 mm | ±0.2 mm | estimate |
| driver gasket, diaphragm set-back, rear boss | 0.3, 0.5, 22 mm | ±0.1, ±0.3, ±3 mm | estimates |
| `residual_leak_gap_mm` | 0.01 (plate), 0.05 (head) mm | lognormal ×/÷ 1.7 | estimate; fitted in acceptance |
| `pinna_volume_cm3` | 0 (plate), 10 (head) cm³ | ±50 % | estimate |

"Build check" tolerances are go/no-go limits the builder measures
(`validation/reference_cup/README.md`); a part outside them is remade, so
the frozen predictions keep describing the cup.

### What the engine cannot model, and where it will show

| not modelled | size | where it shows first |
|---|---|---|
| the driver's internal rear volume and rear holes (two of about 2.7 mm and a pole vent, scaled from the drawing; the datasheet does not describe them) | the free-air Qms of 2.71 bounds their resistance to Rms/Sd² ≈ 6·10⁴ Pa·s/m³, but nothing bounds their mass: plain holes through 1 mm would be about 350 kg/m⁴, which resonates with the rear chamber near 400 Hz | the cup in free air (`cup_free_*_z`: the impedance peak's frequency and height), then the drum response from 200 to 800 Hz; the model-form test shows such a mass failing the acceptance by several dB |
| the free-air air load inside the datasheet's Mms | (8/3)ρa³ = 0.018 g, 6 % of Mms | the in-cup resonance up to 3 % low in frequency |
| the gasket's own compliance | at most 8 % of the front cavity's (all its gas exposed; `wall_check.py`) | a fitted front volume factor above 1 |
| pad compression dynamics, clamping force other than 5 N | the gasket's thickness (±0.2 mm estimated) | front volume, the 5 kHz notch |
| the pinna and head on the Type 4.3 fixture | taken only as 10 cm³ of volume and a larger leak | `t43_ref_p` above about 1 kHz |
| the damping of the damped IEC 60318-4 variant | the undamped model stands in, up to 10 kHz | above 8–10 kHz (not compared) |
| the open hole's flow-dependent resistance | the model is linear; at 10 µW the hole's RMS velocity reaches 0.15 m/s on the fixture (116 Hz) and 0.6 m/s with the cup in free air (29 Hz), 1.5 and 6 m/s at 1 mW | `iec_rhole_hi_p` against `iec_rhole_p` near the vent's notch around 200 Hz; `cup_free_hole_z` below 50 Hz |
| diaphragm break-up above ka = 1 | the rigid-piston limit is 3.06 kHz (erratum E28) | shaded from 3 kHz; the notch near 5 kHz |
| a leak gap below about 4 µm at level 1 | the distributed slit's transfer matrix becomes too ill-conditioned for the solver (a 1 µm gap fails at 1.6 kHz) | the nominal leak is kept at 0.01 mm and the acceptance fit is bounded at 0.005 mm |

## Frozen blind predictions

`acoustilab validate --predict --out validation/reference_cup/predictions/v1`
was run once, on a clean tree at the commit recorded in its manifest. For
every configuration of the protocol it solved the nominal netlist on the
exchange grid (1 kHz·2^(k/48), 10 Hz to 19.9 kHz; the damped variant to
10 kHz) and ran 200 Latin-hypercube Monte Carlo runs (seed 20260925) over
the toleranced parameters that reach that configuration's netlist with a
non-zero spread. Every measurement has:

* `<id>.csv`: the nominal curve (`frequency_Hz,level_dB,phase_deg` or
  `frequency_Hz,magnitude_ohm,phase_deg`, numbers in shortest round-trip
  form), readable by `acoustilab convert` and the curve reader;
* `<id>.envelope.csv`: nominal, median, 5, 10, 90 and 95 % points, minimum,
  maximum (and the same for the impedance phase), the number of runs and
  the validity shading of the nominal solve (0 credible, 1 light, 2 dark);
* a sidecar for each (`acoustilab-curve-sidecar/0.1`): the drive, the
  fixture, ear simulator, pinna and reference point the measurement must
  match, `simulated` provenance with the version, and the configuration's
  overrides;
* `protocol.json` and `netlist.json`: the exact inputs, so the set is
  self-contained;
* `manifest.json` (`acoustilab-frozen-predictions/0.1`): engine version, git
  commit and whether the tree was clean, date, command, SHA-256 of the
  netlist and protocol texts, the reproducibility hash of every
  configuration's expanded netlist (docs/analysis.md), the Monte Carlo plan
  with every distribution and failure, the nominal solve's warnings and
  shading, and the SHA-256, size and command of every file.

**Nothing can change silently.** `acoustilab validate --verify` and
`predict::verify` recompute every file's hash and refuse any file that is
changed, missing or not listed. `Frozen::load`, which every comparison uses,
refuses a set that fails verification. The test
`frozen_predictions_match_their_manifest` pins the SHA-256 of each version's
manifest in its source, and fails for any prediction directory that is not
pinned; a new version is a new directory (the command line refuses to
write into one that exists) plus one reviewed line in that test.

**Drift.** `acoustilab validate --drift [--netlist examples/reference_cup.json]`
(and the non-failing test `drift_from_the_frozen_predictions`, whose output
`cargo test -- --nocapture` shows) re-solves the frozen netlist with the
current engine (engine drift) and, if given, the current netlist (model
drift), and prints the largest level and phase change per measurement,
overall and in the credible band. It never fails on the size of the drift:
a model change shows up there, and the blind record stays what it was.

### What v1 predicts

At the protocol's drive (10 µW into 32 ohm), nominal with the 5–95 % Monte
Carlo range in brackets, drum-point level in dB SPL (`p`) and impedance
maximum (`z`); the credible band ends at 3.08 kHz (rigid piston) and, on
the IEC simulator, starts at 100 Hz:

| measurement | 20 Hz | 100 Hz | coupled peak | 1 kHz | 3 kHz |
|---|---|---|---|---|---|
| `iec_ref_p` | 85.9 [85.3, 86.5] | 87.5 [86.9, 88.1] | 98.2 at 553 Hz | 81.9 [80.8, 82.8] | 62.9 [62.0, 63.6] |
| `iec_sealed_p` | 88.2 | 88.5 | 102.6 at 545 Hz | 81.5 | 62.8 |
| `iec_rsealed_p` | 78.7 | 87.4 | 100.1 at 545 Hz | 81.8 | 62.9 |
| `iec_rhole_p` (notch 71.3 at 177 Hz) | 94.6 | 95.5 | 99.9 at 569 Hz | 81.9 | 62.9 |
| `iec_fsealed_p` | 93.9 | 89.5 | 100.1 at 545 Hz | 81.5 | 62.8 |
| `t43_ref_p` | 85.8 | 87.7 | 99.2 at 569 Hz | 84.4 [83.1, 85.6] | 66.4 |

The sealed rear chamber stiffens the diaphragm (the rear plug `mesh`
raises the 20 Hz level by 7 dB over `sealed`), the meshed front ports leak
8 dB at 20 Hz against sealed ones, and the open rear hole resonates with
the rear chamber into a notch at 177 Hz, 17 dB below the reference state.
The front cavity's notch at the ear port is at 5.04 kHz (38 dB, 5–95 %: 34
to 50 dB), shaded.

| impedance | maximum |
|---|---|
| `driver_free_z` | 120.8 ohm at 82 Hz (Re + Bl²/Rms) |
| `cup_free_sealed_z`, `_hole_z`, `_mesh_z` | 111 ohm at 408 Hz, 101 ohm at 445 Hz, 59 ohm at 414 Hz |
| `iec_ref_z`, `iec_sealed_z`, `t43_ref_z` | 56.6 ohm at 561 Hz, 111.6 ohm at 553 Hz, 54.7 ohm at 569 Hz |

The air springs of the front and rear chambers are 20 and 27 times stiffer
than the suspension (Sd²·Cms against V/ρc²), together 47 times, so the
in-cup resonance sits near 81.8 Hz·√48 = 567 Hz almost whatever the
driver's free-air fs; the meshed rear port is what damps it (56.6 against
111.6 ohm).

## A session: `acoustilab validate`

```sh
acoustilab validate --session DIR          # templates and SESSION.txt
acoustilab validate DIR [--out report.json] [--json] [--no-anchor] [--max-evals N] [--fit-ppo N]
acoustilab validate --simulate DIR [--seed S] [--ppo N] [--omit ID] [--truth-netlist F] [--set NAME=VALUE]
```

The steps (`acoustilab::validation::session`):

1. **Import.** Seating k of measurement `M` is `M_s<k>.<frd|zma|txt|csv>`
   with its sidecar (`M_s<k>.<ext>.sidecar.json` or the template's
   `M_s<k>.sidecar.json`), read with the curve reader (docs/fitting.md).
2. **Sidecar checks** against the protocol and the frozen sidecar. Errors
   exclude the file: a different quantity, fixture, ear simulator, pinna,
   reference point or drive; compensation other than none; an uncalibrated
   pressure; a source impedance unstated or above 1 ohm; smoothing coarser
   than 1/24 octave; an origin other than measured or virtual rig; a
   missing sidecar; a second file for one seating. Warnings: missing date,
   device, temperature or origin, a temperature more than 3 °C from 23 °C,
   files that are not single seatings, finer smoothing, an impedance without
   phase. `virtual_rig` curves mark the whole report as synthetic.
3. **Averaging.** The seatings are resampled to the frozen grid within
   their common band and averaged in dB (impedance phase: circular mean).
   Their sample standard deviation, pooled over ±1/6 octave because five
   seatings give only four degrees of freedom per point, is the observed
   repositioning spread s. The mean's standard uncertainty is
   u_m = sqrt(s²/N + Σ systematic²), with the coupler, calibration,
   fixture-to-human and numerical terms of the first sidecar's budget.
4. **Blind comparison**, no fitting: r = L_measured − L_frozen nominal (for
   impedance, 20·log10 of the ratio, and the phase difference),
   u_p = (p95 − p5)/(2·1.645) from the envelope, z = r/sqrt(u_p² + u_m²);
   per band (below 20 Hz, 20 Hz–1 kHz, 1–4 kHz, above 4 kHz) the RMS and
   largest |r|, the share of |z| ≤ 2 and of points inside the 5–95 %
   envelope; and the largest |r| where the frozen solve is not shaded.
5. **Acceptance** (spec Section 17, read as erratum E54 states): for each
   pressure measurement of a configuration marked `acceptance`, the
   protocol's fit parameters, `residual_leak_gap_mm` (0.005 to 0.5 mm, log
   scale) and `front_volume_factor` (0.5 to 1.5), are fitted to the mean
   curve from 20 Hz to 4 kHz with `fit::fit` (weights 1/u_m, no level
   offset, the source impedance the sidecars state), and the residual
   against the fitted model must be within 2 dB at every point from 20 Hz
   to 1 kHz and within 4 dB from 1 to 4 kHz. A band counts only when the
   data reach both of its ends (within 1/24 octave). Above 4 kHz the
   residual is reported without a bound. The verdict is taken on the three
   reference states (`iec_ref_p`, `iecd_ref_p`, `t43_ref_p`): pass, fail,
   or not evaluated when one of them is missing.
6. **Driver anchor** (reported separately, not the spec's criterion): the
   driver's `Re`, compliance, Bl and damping factors are fitted to the bare
   driver's free-air impedance (Mms and Sd stay at the datasheet: an
   impedance cannot fix the scale, erratum E50), and the acceptance is run
   again with them. A pass here and a fail above points at the driver unit,
   a fail in both at the model.
7. **Report**: missing and incomplete measurements, every issue, the blind
   comparison and the acceptance in separate sections, and the residuals at
   every frequency in the JSON (`acoustilab-validation-report/0.1`).

A full session at level 1 takes three to four minutes in a release build
(fifteen fits of 20 to 45 evaluations, most of each evaluation the modal
cavities' set-up). `--fit-ppo 12` fits on a coarser grid and still checks
the bounds at every point; `--set fidelity=0` gives a fast lumped check,
which is not the frozen model above 1 kHz. A fit that reaches
`--max-evals` (200) is reported as not converged and judged where it
stopped: a leak the data cannot see (a sealing gasket) is pushed towards
its bound one geometric step at a time when the model misses something
else, and that is where the cap is usually reached.

## Verification

`crates/acoustilab/tests/reference_cup.rs` (engine), with every synthetic
session from the virtual rig (`validation::simulate`: the true cup is the
frozen netlist or a modified one; each seating on a fixture draws its own
residual leak, σ_ln = 0.3, and front volume, σ_ln = 0.01, shared by the
plug states of that seating; per-point noise 0.05 dB and 0.3°):

| test | checks |
|---|---|
| `dimensions_scad_and_netlist_agree` | every model dimension is a netlist parameter with the same value and tolerance, CAD-only ones are not, every netlist geometry parameter is a dimension; `dimensions.scad` is exactly the generator's output; the netlist's rear and front volumes equal the closed forms from the dimensions to 1e-12 |
| `the_written_protocol_names_every_measurement` | `protocol.md` names every configuration and measurement of `protocol.json` |
| `every_configuration_solves_within_its_operating_limits` | the protocol matches the netlist (overrides, probes, fit parameters) and no configuration exceeds an operating limit at the protocol's drive |
| `frozen_predictions_match_their_manifest` | every version verifies and its manifest hash is the pinned one |
| `verification_refuses_any_change` | one changed digit, an extra file or a missing file fails verification and loading |
| `frozen_predictions_are_complete_and_consistent` | clean-tree commit, netlist hash, no failed Monte Carlo run, envelope nominal equal to the nominal curve, ordered percentiles, sidecars stating the protocol's conditions |
| `drift_from_the_frozen_predictions` | runs and prints the drift report (never fails on its size) |
| `a_true_cup_within_its_tolerances_passes` | a cup with Bl +4 %, compliance +20 %, Rms −10 %, Mms 0.31 g, mesh 240 rayl and front volume +6 % passes on the IEC and Type 4.3 states (largest residual 0.7 dB below 1 kHz); the fitted front volume factor is within 0.03 of the true 1.06 (1.037 and 1.050: the driver error it cannot fit biases it); the driver anchor finds Bl within 2 % (1.023 for 1.04), after which the Type 4.3 leak comes out at 0.042 mm (true 0.05 with a seat spread) instead of running to its bound; the blind section carries no pass or fail |
| `a_model_form_error_fails_and_a_small_one_passes` | an internal rear-hole mass of 350 kg/m⁴ with 2 cm³ behind the diaphragm fails: 9.9 dB at 595 Hz after the fit; 3 kg/m⁴ with 0.1 cm³ passes (0.7 dB) |
| `missing_files_and_protocol_violations_are_reported` | a missing measurement, a lost seating, a wrong fixture, a coarse smoothing, a different drive, a missing and an unreadable sidecar are each reported, the bad files left out, the acceptance "not evaluated", and a stray file listed |
| `session_templates_name_every_file` | a template for every seating file, parseable, with the conditions filled in |
| `full_session_at_level_1` (ignored; release, about 3.5 minutes) | the whole protocol at level 1 from a synthetic session of the nominal cup: every acceptance run passes with residuals of at most 0.41 dB below 4 kHz (the open vent's notch) and 8–16 dB above, where seatings differing by 1 % in front volume smear the cup's notches in the mean; the blind comparison of `iec_ref_p` stays within 0.1 dB and inside the 5–95 % envelope below 4 kHz |

The end-to-end tests run the true cup and the fitted model at level 0 so
that a debug `cargo test` stays short; the pipeline is the same at level 1.
`crates/acoustilab-cli/tests/validate_cli.rs` checks the command line:
`--verify` on the committed set, refusing to overwrite a version or to
predict with `--set`, and a session made only of templates reported as
incomplete and not evaluated.

## Limits

* **Nothing here is measured.** The predictions are the engine's; the
  pipeline has seen synthetic data only. The analyser's own export layouts
  are read by the tested curve readers (docs/fitting.md), not yet by files
  from this session's equipment.
* The acceptance statistic is the strict reading of E54; the report's
  residuals allow any other.
* The comparison uses the first sidecar's systematic uncertainty terms for
  every seating of a measurement.
* The driver anchor fits a model without voice-coil inductance or creep
  (the datasheet gives Le = 0); a structured residual in its fit says so.
* The damped IEC 60318-4 simulator has no model of its own; its prediction
  is the undamped one below 10 kHz.
* The seating-to-seating variation of a real cup (and so the weights of the
  acceptance fit) is only known once measured; the synthetic sessions assume
  one.
