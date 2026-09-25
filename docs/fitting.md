# Measured curves and parameter identification

Spec Sections 12 and 14 and case study 2 of Section 18: import and export
curves with their metadata, fit netlist parameters to measured impedance and
responses, report what the data can and cannot determine, and exercise all
of it on a virtual rig until a measurement rig exists.

| module | content |
|---|---|
| `acoustilab::io` | `Curve`, the metadata `Sidecar` with its uncertainty budget and compatibility check, FRD/ZMA/REW/CSV reading and writing, resampling |
| `acoustilab::fit` | `fit()` (the identification), `lm` (Levenberg–Marquardt, shared with `ts::fit_impedance`), `jacobian`, `identify` (singular values and named directions), `roles` (the Section 12 rules), `rig` (virtual rig), `rng`, `dense` (small SVD) |
| CLI | `acoustilab fit`, `measure`, `convert`, `export` |
| wasm | `import_curve`, `export_curve`, `compare_curves`, `probe_curve`, `fit`, `virtual_measure` |

**Status: theory-only.** No file here has come from a measurement rig.
Everything is exercised with synthetic measurements from netlists with known
true parameters (the virtual rig). The readers follow the published layouts
of the formats; they have been tested on REW's own export layout, not yet on
files from every tool.

## Curves

A curve is a quantity on positive, increasing frequencies: linear RMS
magnitude in SI units with an optional phase in degrees (`e^{+jωt}`,
docs/conventions.md), plus a sidecar. Quantities: `impedance` (ohm),
`pressure` (Pa; levels in dB SPL re 20 µPa), `displacement` (m), `velocity`
(m/s), `generic`.

JSON form, schema `acoustilab-curve/0.1`, unknown keys rejected:

```json
{"schema": "acoustilab-curve/0.1", "quantity": "pressure",
 "frequencies_Hz": [20, 25.2], "level_dB": [96.1, 96.3], "phase_deg": [170.2, 168.9],
 "sidecar": {"schema": "acoustilab-curve-sidecar/0.1", "fixture": "GRAS 45CA"}}
```

The magnitude key carries the unit: `magnitude_ohm`, `level_dB` (pressure;
`magnitude_Pa` is also read), `magnitude_m`, `magnitude_m_per_s`, `magnitude`
or `level_dB` (generic, dB re 1). `re` and `im` may replace magnitude and
phase.

## File formats

One reader serves every format; the format supplies defaults.

| format | extension | columns | default quantity |
|---|---|---|---|
| FRD | `.frd` | frequency Hz, level dB, *phase deg* | pressure (dB SPL) |
| ZMA | `.zma` | frequency Hz, impedance ohm, *phase deg* | impedance |
| REW text | `.txt`, `.dat` | `*` header ending in `* Freq(Hz) SPL(dB) Phase(degrees)` or `* Freq(Hz) Z(Ohms) Phase(degrees)` | from the header |
| CSV/TXT | `.csv` | named by a header row, or frequency, magnitude, phase | from the header's units, else the caller's |

Reading:

* Lines starting with `*`, `#`, `;`, `%`, `'`, `!` or `//` are comments;
  blank lines, a byte-order mark and CRLF or lone-CR line endings are
  accepted.
* A data line begins with a number (REW's rule). Other lines are text and
  are kept as comments; anything after the last number of a data line is a
  comment (`27.0, 68.31, this line has a comment`). Every data line must
  have the same count of numbers.
* The delimiter is sniffed from the first data line: tab, else semicolon,
  else comma, else white space. A comma inside white-space-separated numbers
  (`20,5 65,1`) is a decimal comma; with tab or semicolon delimiters a
  number with a comma and no point has a decimal comma. Mixing decimal
  commas and points is an error, and `1.234,5` (digit grouping) is rejected
  as ambiguous. Surrounding double quotes are removed.
* The column header is the last text or comment line before the data whose
  first field names the frequency (`Freq(Hz)`, `frequency_Hz`, `Frequency
  [kHz]`, `f`), or which names it in another field and has one field per
  data column and no numbers (`SPL (dB),Frequency (Hz)`). A row of bare
  units under the names (`Hz;dB;deg`) serves as the header. Units in
  parentheses, brackets or a `_unit` suffix set the
  scale: Hz, kHz; dB, ohm, Pa, mPa, m, mm, µm, m/s, mm/s; degrees, radians.
  Magnitude columns are recognised by name (`SPL`, `level`, `magnitude`,
  `Z`, `impedance`, `raw` as in AutoEq CSVs, ...) or by a magnitude unit;
  `re`/`real` and `im`/`imag` columns give a complex value. Without a header
  the columns are frequency, magnitude and phase; more than three unnamed
  columns are an error.
* A bare CSV whose magnitude unit is unknown needs a quantity (from the
  caller or the sidecar): pressure is then read as dB, other quantities as
  linear.
* Frequencies are sorted. An exact repeat of a point is dropped; a
  repeated frequency with different values is an error naming both lines.
  Every error names its line.
* In a REW export a phase column of zeros means "no phase" (REW writes 0.0
  when a measurement has none). REW's `Dated`, `Smoothing`, `Measurement`
  and `Note` lines fill the sidecar's `date`, `smoothing` and `notes`, and
  its first line the provenance `tool`.

Writing: FRD, ZMA and REW text are tab-separated with six decimals (four for
the phase), the precision REW and crossover tools use, after a `*` header;
REW text follows REW's own export layout. CSV has a unit-suffixed header
(`frequency_Hz,level_dB,phase_deg`, `frequency_Hz,magnitude_ohm,phase_deg`)
and every number in its shortest exact form, so a CSV plus its sidecar is a
lossless archive. FRD refuses impedance and ZMA anything else. Every file
has a sidecar next to it, `FILE.sidecar.json`.

Simulated curves: `io::curve::from_solve(circuit, result, probe)` turns any
probe of a solve into a curve whose sidecar states the quantity, absolute
levels, the solve's drive (the netlist's `drive`, else its source voltage)
and source impedance, and `simulated` provenance with the engine version
(spec Section 14, "CSV of any probe with sidecar"); `acoustilab export` and
the wasm `probe_curve` wrap it.

Not provided: dedicated Klippel or DATS adapters. DATS exports impedance as
three-column `.zma`/`.txt` files, which the ZMA reader accepts (not checked
against a real DATS file); Klippel's native formats are not publicly
documented, so use their text or CSV export. WAV impulse responses are not
read.

## The sidecar (`acoustilab-curve-sidecar/0.1`)

Every key but `schema` is optional; keys carry their unit and unknown keys
are rejected (also inside `provenance` and `uncertainty`).

| key | meaning |
|---|---|
| `quantity`, *`unit`* | what the curve is (must match the curve); `unit` for generic curves |
| `calibrated` | levels are absolute at the stated drive (`false`: arbitrary reference; a fit gives the curve a free level offset) |
| `fixture`, `ear_simulator`, `pinna` | fixture and ear simulator, pinna type |
| `seatings`, `averaging` | number of seatings and how they were averaged: `none`, `magnitude` (power mean of magnitudes), `db` (mean level), `complex` (vector mean) |
| `smoothing` | `none` or `1/N` (octave fraction) |
| `drive`, `source_impedance_ohm` | drive convention and level in the netlist's `drive` keys (`{"voltage_V": 1}`, `{"power_mW": 1, "rated_ohm": 32}`, `{"characteristic": "p_drp"}`, `{"current_mA": 10}`), and the amplifier's output impedance |
| `compensation` | `none`, `diffuse_field`, `free_field` or a description |
| `reference_point` | `drp`, `eep`, `erp`, `coupler`, `terminals`, ... |
| `temperature_C`, `date`, `device` | conditions and device identifier |
| `provenance` | `origin` (`measured`, `simulated`, `virtual_rig`, `published`, `digitized`, `user`), `source`, `url`, `licence`, `tool` |
| `uncertainty` | the measurement uncertainty budget (below) |
| `virtual_rig` | settings and true parameters of a virtual-rig curve |
| `notes` | free text |

**Uncertainty budget** (spec Section 12). Standard uncertainties (one
standard deviation), each a number or a table
`{"frequencies_Hz": [..], "values_dB": [..]}` (`values_deg` for the phase),
interpolated linearly in ln f and held beyond its ends:

| term | kind |
|---|---|
| `coupler_dB` | coupler or ear-simulator tolerance (from the standard's table, which the user supplies: the engine bundles none) |
| `microphone_calibration_dB` | sensor calibration |
| `repositioning_dB` | spread between seatings, one seating |
| `fixture_to_human_dB` | translation to a human ear; include only when the model's load is not the fixture |
| `numerical_dB` | numerical error of a simulated curve |
| `noise_dB` | random noise, one seating |
| `phase_deg` | phase uncertainty, one seating |

The combined level uncertainty of a curve averaged over N seatings is
u = sqrt(coupler² + calibration² + fixture² + numerical² + (repositioning² +
noise²)/N), and the phase uncertainty is phase_deg/√N. The per-seating terms
average down, the systematic ones do not. A fit does not weight the points
by the calibration term but gives the curve a level offset with that prior
(see Fitting, "Weights").

**Compatibility.** `io::sidecar::compare(a, b)` lists the fields in which two
curves differ. Differences in quantity, fixture, ear simulator, pinna,
compensation, drive (convention and level; `1 mW into 32 ohm` equals
`0.001 W into 32 ohm`), source impedance or reference point block a
comparison, and `Comparison::check(allow)` refuses it with a message naming
each difference unless every one is in `allow` (`["all"]` allows
everything). A field stated on one side only blocks too ("unstated"),
because the match cannot be confirmed; a field stated on neither side is a
note. Averaging, smoothing and calibration differences are notes. Text is
compared ignoring case and repeated spaces.

**Resampling.** `Curve::resample(freqs)` interpolates the level in dB and the
unwrapped phase linearly in ln f; it never extrapolates. Unwrapping takes
each step between neighbours within ±180°, so a curve sampled too coarsely
for its phase slope cannot be unwrapped correctly. The exchange grid
(`exchange_grid`, `Curve::resample_exchange`) is 1 kHz·2^(k/48): 48 points per
octave (spec Section 14, erratum E30), anchored at 1 kHz so that curves from
different sources share their points. Interpolation correlates the noise of
neighbouring points; fit the original points where possible.

## Fitting

```rust
let spec = FitSpec::new(
    vec![FitParam::new("leak_gap_mm"), FitParam::new("front_depth_mm")],
    vec![CurveSpec::new("zin", impedance), CurveSpec::new("p_drp", drum)],
);
let report = acoustilab::fit::fit(&Parametric::parse(&text)?, &spec)?;
```

JSON form (`acoustilab-fit/0.1`, unknown keys rejected):

```json
{"schema": "acoustilab-fit/0.1",
 "parameters": ["driver_Re_ohm", {"name": "leak_gap_mm", "start": 0.1, "min": 0.01, "max": 0.5, "scale": "log"}],
 "overrides": {"ear": "iec60318_4"},
 "curves": [
   {"probe": "zin", "curve": {"...": "curve document"}},
   {"probe": "zin", "curve": {"...": "..."}, "overrides": {"added_mass_mg": 150}},
   {"probe": "p_drp", "curve": {"...": "..."}, "use_phase": false, "offset": "none",
    "weight": 1, "f_min_Hz": 20, "f_max_Hz": 10000, "allow": ["compensation"]}],
 "f_min_Hz": 10, "f_max_Hz": 20000,
 "max_iterations": 100, "max_evaluations": 5000, "starts": 1, "seed": 1,
 "allow_spl_only": false, "rank_tolerance": 1e-6}
```

* `parameters`: continuous parameters of the netlist (numbers, not integers,
  choices, booleans or derived parameters), by name or with a `start`,
  narrower `min`/`max` and a `scale`. The default scale is `log` when the
  value is positive and the minimum is not negative, else `linear`.
* `overrides` fix parameters for every curve; a curve's own `overrides` are
  its measurement condition (an added mass, a box volume). Curves under the
  same condition share one solve.
* A curve is compared with a probe of the same quantity at the curve's own
  frequencies within the band (default 10 Hz to 20 kHz, spec Section 12).

**Residuals.** For impedance, displacement and velocity curves each point
contributes the level error 20·log10|Z_model/Z| in dB and the phase error
arg(Z_model/Z) in degrees, each divided by its standard uncertainty. Together
they are the complex logarithm ln(Z_model/Z), which equals the relative
complex error (Z_model − Z)/Z to first order and is the residual of
`ts::fit_impedance` in another basis. This choice over residuals on the real
and imaginary parts in ohm: every point counts by its relative error, where
ohm residuals let the resonance peak (Zmax several times Re) dominate, and
analysers state magnitude and phase accuracy separately, so the two parts
are weighted separately. Pressure curves contribute the level error in dB;
their phase is used only on request (`use_phase`), since a measured acoustic
phase carries an unknown time of flight.

**Weights.** Each residual is divided by the curve's combined standard
uncertainty from its sidecar budget at that frequency (spec Section 12:
"the budget sets the fitting weights"), times sqrt(`weight`). Without a
budget: 1 % for impedance (20·log10 1.01 = 0.086 dB) and 0.5 dB for other
curves. Without a phase term the phase uncertainty is the level's
equivalent, u_φ = u_L·(ln 10/20)·(180/π) degrees. The smallest uncertainty
used is 0.001 dB.

The sensor calibration term (`microphone_calibration_dB`) is not noise: it
is one level error shared by every point. Divided into each point's
weight it would average down over the points, and a parameter that sets
the absolute level (Bl from a laser or calibrated SPL curve) would be
reported many times more precise than the calibration allows (a 0.5 dB
calibration gave Bl within ±1.2 % while the error reached 8 %). Its common
part c, the smallest value of the term over the curve's band, is therefore
a level offset with the Gaussian prior c (below), and the points are
weighted by the other terms and what is left of the calibration term,
sqrt(u_cal² − c²). A budget with no other level term leaves the scatter of
the points unstated, and the default uncertainty stands in for it. The
other systematic terms (coupler, fixture-to-human, numerical) vary with
frequency and are still treated as independent per point (Limitations).

**Drive and level offsets.** A pressure (or displacement, velocity) curve
is simulated at the drive its sidecar states, whatever the netlist's. A
curve without a stated drive, or with `calibrated: false`, gets a free level
offset in dB, a nuisance parameter reported with its interval. A curve
whose budget has a calibration term gets an offset with that prior (see
"Weights"). `offset` may also be set to `"none"` (the calibration term
then weights the points), `"free"` or `{"prior_dB": σ}` (free with a
Gaussian prior). A curve whose sidecar states a
compensation other than `none` is refused (the model's probe is
uncompensated) unless the curve's `allow` lists `compensation`. The fit
warns when a curve is smoothed coarser than 1/6 octave, when its source
impedance differs from the netlist's, and when some of its points lie where
the model is outside its validity (dark shading).

**Optimiser** (`fit::lm`). Levenberg–Marquardt on u = ln p (log scale) or
u = p/unit (linear scale; unit is the bounded range, else max(|start|, 1)).
Each step solves the Marquardt-damped normal equations in u within the
directions the Jacobian resolves: right singular vectors with σ above
1e-7·σ_max and above 1 (in units of the weighted residuals, σ < 1 means
moving e-fold changes χ² by less than 1). Directions the data do not
determine are not stepped along, so those combinations stay where they
are instead of drifting on noise to a bound. The test is made where the
fit stands: a direction that is still resolved at the start and becomes
flat on the way (the fs–Qms–Qes direction of the over-ear case below) can
end far from its start, and is reported as undetermined. The σ > 1
condition holds only at an acceptable misfit: if the iteration would stop
with such directions left out and a reduced χ² above 10, it continues
along them. A
parameter that starts where the data hardly respond to it (a pad leak
starting nearly closed, σ = 0.02) would otherwise stop at once with a
reduced χ² of 350 and be reported converged. The parameters' `min`/`max`
become bounds on u, kept by the fraction-to-the-boundary rule of
interior-point methods: a step component that would cross a bound goes 90 %
of the way to it. Trial points never reach a bound, so a parameter is never
pinned there as by projection onto the box, and one whose optimum lies
beyond a bound approaches it geometrically and is reported at the bound. A
start on a bound is allowed. (A smooth change of variable such as a logistic
map was tried first: its derivative vanishes at the bounds, which froze
parameters that started on one, such as a coil inductance starting at 0.) A
trial point where the network cannot be built or solved (a singular system,
a value an element rejects) is a rejected step, not an error. Convergence:
a relative cost reduction below 1e-10, a step below 1e-9, no descending
step (`no_further_reduction`, or `flat` when directions below σ = 1 were
left out), or three accepted steps lowering χ² by less than 1e-3 in total
(a change far below the Δχ² = 1 of one standard deviation).

**Jacobian.** Central differences of full solves, step 1e-4 in u, one-sided
at a bound or where one side cannot be evaluated
(`fit::jacobian::central_differences`, the one place a sensitivity method
would plug in). Against the closed-form derivative of a driver's impedance
its error is 1.6e-7 relative next to the resonance, where the truncation
h²·r‴/6 dominates, and 1e-8 or less elsewhere (rounding ε/h ~ 1e-9 at a solve
accuracy ε ~ 1e-13); `jacobian_of_full_solves_matches_the_closed_form`.
Offsets have analytic columns.

**Multi-start.** `starts` > 1 adds starts from a Latin hypercube over the
parameters' ranges in u (for a parameter without both bounds: a factor of 2
either side of its start on a log scale, half its unit either side on a
linear one), seeded by `seed`; the lowest cost wins and every start is
reported. All starts share `max_evaluations`.

**Bounded runtime.** `max_iterations` and `max_evaluations` (model
evaluations, Jacobian columns included) cap a call; one evaluation solves
every condition at its curves' frequencies (natively about 0.06 ms per
point, so 5 ms for the 72-point case study and 26 ms at 430 points). At
most 1000 `starts`, and at most 10^6 distinct frequencies per condition
(the engine's sweep limit). A caller that wants progress or cancellation
(a web worker) runs a few iterations per call and passes the report's
`fitted` values as the next call's `start` values, with each parameter's
`scale` from the first report: the default scale follows the start, so a
parameter that started at 0 (linear) would be fitted on a log scale once
resumed from a positive value, with another interval and status. Each call
restarts the damping and the level offsets.

### The report (`acoustilab-fit-report/0.1`)

| key | content |
|---|---|
| `converged`, `stop`, `stop_reason`, `iterations`, `evaluations`, `failed_evaluations`, `starts` | how the optimiser ended; failed evaluations are rejected trial points |
| `cost`, `degrees_of_freedom`, `reduced_chi2`, `covariance_scale` | χ² of the weighted residuals, χ²/(m − n), and s² = max(χ²/(m − n), 1) |
| `parameters` | per parameter: `value`, `start`, `ci95` (95 %), `sd` (of ln value, or on a linear scale of value/r, r = |value| but at least 1 % of the range), `status`, driver `roles`, `notes` |
| `offsets` | level offsets with intervals and priors |
| `correlation` | correlation matrix of the fitted variables (null for unidentifiable ones) |
| `curves` | per curve: points and band, RMS residual in dB, degrees and ohm, weighted RMS, lag-1 autocorrelation, runs test, `structured`, `inflation`, and the residuals themselves |
| `identifiability` | singular values, every singular direction with its components and a sentence, the Section 12 findings |
| `fitted` | `{name: value}`, ready to use as overrides |
| `summary`, `warnings` | sentences |

**Intervals.** The covariance of the fitted variables is
C = s²·(J̃ᵀJ̃)⁺, with J̃ the Jacobian of the weighted residuals in "report
space" (ln p; p/r for linear parameters, r as for `sd`; dB for offsets), the pseudo-inverse
taken over the identifiable directions, and s² = max(χ²_ν, 1). The 95 %
interval is ln p ± 1.96·sd (symmetric in ln p) or p ± 1.96·sd·|p|.
Assumptions: the model is right, the linearisation holds over the interval,
the weighted residuals are independent with variance s², and the budget is a
floor: a fit can never be more certain than the stated measurement
uncertainty allows (with χ²_ν < 1 the budget is taken as conservative;
with χ²_ν > 1 the intervals widen). Correlated residuals violate the
independence: each curve's lag-1 autocorrelation ρ (uncentred, frequency
order) is computed, and when it exceeds 2/√n its rows count as
(1 − ρ)/(1 + ρ) of their information, the variance inflation of an AR(1)
series. The intervals then remain optimistic for errors correlated over
wider bands, and the report says so.

**Structured residuals.** A curve is `structured` when the Wald–Wolfowitz
runs test on the signs of its residuals gives z < −3, or when ρ > 3/√n and
ρ > 0.3. That is the signature of model-form error (the model cannot follow
the data) or of smooth measurement errors (coupler ripple, repositioning); a
reduced χ² above 10 adds a warning. Neither is hidden behind a small
interval.

**Status of a parameter.** From its sd in ln units: `determined` when the
95 % interval is within ±25 % (1.96·sd ≤ ln 1.25), `weakly_determined`
within a factor of 2, `undetermined` beyond; `unidentifiable` when it has a
component above 0.05 in a numerically null direction (σ below
`rank_tolerance`·σ_max, default 1e-6); `at_bound` when it ended within
0.1 % of the range from a bound; `scale_ambiguous` by the Section 12 rule.
Only determined and weakly determined parameters carry an interval. A
linear-scale parameter is judged by its sd relative to r = |value|, but at
least 1 % of its range (or of max(|start|, 1) without both bounds), so a
signed parameter fitted near zero is judged on its own scale rather than
against zero.

## Identifiability (spec Section 12)

**Singular directions.** The weighted Jacobian J̃ = U·Σ·Vᵀ at the fitted point
gives, for each right singular vector v_i, a parameter combination along
which the residuals change at rate σ_i, so the fit scatters along it with
standard deviation s/σ_i. The report lists every direction with its
components (in parameter order, first one positive, those above 5 % of the
largest) and a sentence, for example:

> Bl_Tm, Mms_g, Cms_mm_per_N and Rms_Ns_per_m move together (changes of ln
> value in the ratio +1 : +2 : -2 : +2): no curve changes along this
> direction, so the data cannot determine it

The same statuses as for parameters apply to directions. Directions that
are not determined, and the Section 12 findings, also go in `summary`.

**The impedance-only rule.** The input impedance of a moving-coil driver
without acoustic load,
Z = Re + Z_L + Bl²/(jωMms + Rms + 1/(jωCms)),
is unchanged by Bl → α·Bl, Mms → α²·Mms, Cms → Cms/α², Rms → α²·Rms: it
determines only Bl²/Mms, Bl²·Cms and Bl²/Rms (with Re and the inductance
terms). An acoustic load enters as Sd²·Z_a and keeps the invariance with
Sd → α·Sd. In the primary set (fs, Qms, Qes, Re, Mms, Sd) the same
transformation moves only Mms (α²) and Sd (α). `fit::roles` finds which
driver key each fitted parameter feeds (directly or through derived
parameters) on `driver`, `motor`, `suspension`, `piston` and `coil`
elements. When the fitted parameters of a driver span the scale direction
(the primary set's Mms; or every one of Bl, Mms, Cms or Kms, and Rms that
the physical set gives) and the data contain none of the data below, those
parameters (and Sd when fitted) are marked `scale_ambiguous`, with the data
that would resolve the scale, whatever the numerics say: a single impedance
in a modelled load (a closed cup) pins α only through the load model and
Sd, and the report notes that. Fixing any one of the scale parameters
removes the ambiguity.

The data that fix the scale:

| method | what the fit needs | why it works |
|---|---|---|
| added mass (`added_mass`) | impedance curves under two conditions that differ in a known `mass` element on the driver's mechanical node (`<driver>.m`), the mass not fitted | Mms = Δm/((fs/fs′)² − 1) |
| known volume (`known_volume`) | impedance curves under two conditions that differ in a cavity on the driver's face (free air and a sealed box), Sd not fitted | the box adds the known stiffness Sd²·ρc²/V, which separates Cms from Bl (one curve in the box is not enough: erratum E51) |
| laser (`displacement`) | a displacement or velocity curve of the diaphragm at a stated drive | x = Bl·i/(jω·Z_m) fixes Bl/Z_m, which impedance does not |
| calibrated SPL (`spl_known_load`) | a pressure curve with an absolute level at a stated drive, Sd not fitted | p ∝ Sd·Bl/Z_m in a known load |

With such data the finding is `scale_resolved`, and the numerics say how
well. The added-mass method is flagged `added_mass_unreliable` for a moving
mass below 0.5 g (spec Section 12): a 10 mg adhesive dot is 3 % of 0.33 g,
and since δMms/Mms = δΔm/Δm, an error of 10 mg in a 150 mg test mass moves
Mms by 6.7 % and Bl by 3.3 %, beyond intervals that take the test mass as
exact. A test mass under 20 % of Mms is flagged `added_mass_small`.

**SPL-only fits** of driver parameters are refused (`FitError::Refused`,
wasm kind `fit_refused`): a pressure response scales with Bl·Sd/(Re·|Zm|)
and with the microphone calibration, so pressure data alone cannot separate
the driver's parameters (spec Section 12). `allow_spl_only` overrides the
refusal and records it. Fitting load parameters (a leak, a volume) to
pressure curves is allowed.

A worked case of a direction the rule does not cover: the over-ear template
in its cup. The air springs of the front and rear cavities are about 100
times stiffer than the suspension, so the free-air fs, Qms and Qes cannot be
told apart from impedance and drum pressure measured on the fixture: the
report names the direction "driver_fs_Hz, driver_Qms and driver_Qes move
together (ratio about 1 : 1 : 1)", which changes only Cms; Qes/fs and
Qms/fs (Bl and Rms, given Mms) are determined: over five seeds about
±1.5 % and ±10 % (95 %, from the sds and the correlation), each covering
the truth, while fs itself ended anywhere from 34 to 123 Hz (truth 90 Hz)
and is reported as undetermined.

## Virtual rig (`fit::rig`)

`rig::measure(p, spec)` solves the netlist under the true overrides on the
measurement grid (default: the exchange grid, 10 Hz to 20 kHz at 48 per
octave) and applies seeded noise. With x = ln(f/f_first)/ln(f_last/f_first):

* seating k of N measures H_k = H·10^((d_k(f) + n_k)/20)·exp(j(φ_k − 2πf·τ_k)):
  per-point normal noise n_k (`level_dB`) and φ_k (`phase_deg`), a normal
  delay τ_k per seating (`repositioning_delay_us`), and a smooth level change
  d_k(f) = σ_r·[g/√2 + Σ_{j=1..3}(b_j cos jπx + c_j sin jπx)/√6] with
  standard deviation σ_r (`repositioning_dB`) at every frequency;
* the seatings are averaged (`averaging`): `complex` (vector mean, which
  loses treble: its expected magnitude is |H|·exp(−(2πfσ_τ)²/2)), `magnitude`
  (power mean) or `db` (mean level), the latter two with the phase of the
  vector mean;
* systematic errors drawn once: a sensor calibration offset
  (`microphone_offset_dB`) and slope per decade from 1 kHz
  (`microphone_slope_dB_per_decade`), and a coupler ripple
  c(f) = σ_c·Σ_{j=1..4}(b_j cos jπx + c_j sin jπx)/2 of RMS σ_c (`coupler_dB`).

Impedance curves get the per-point noise and the averaging; pressure curves
everything; displacement and velocity curves the noise, the averaging and
the calibration errors. All deviates come from one xoshiro256** stream
seeded by `seed` (seeded through SplitMix64) in a fixed order: a seed always
gives the same curve (bit-identical on one platform; Box–Muller's `ln` and
`cos` may differ in the last bit between math libraries).

The sidecar says what the curve is: `provenance.origin = "virtual_rig"`, the
rig settings and every resolved true parameter in `virtual_rig`, the drive
and source impedance of the netlist, seatings and averaging, and an
uncertainty budget stating the noise settings (per-seating noise and
repositioning, the calibration as a table when a slope is set, the coupler
RMS), unless the spec gives its own `uncertainty`. A rig without noise states
no budget, so a fit uses its default uncertainties rather than trusting a
zero one.

JSON form:

```json
{"probe": "p_drp", "overrides": {"leak_gap_mm": 0.12},
 "f_min_Hz": 20, "f_max_Hz": 10000, "points_per_octave": 12,
 "noise": {"seed": 7, "level_dB": 0.1, "phase_deg": 1, "seatings": 5, "averaging": "complex",
           "repositioning_dB": 0.2, "repositioning_delay_us": 5,
           "microphone_offset_dB": 0.2, "microphone_slope_dB_per_decade": 0.1, "coupler_dB": 0.1},
 "sidecar": {"fixture": "GRAS 45CA", "ear_simulator": "iec60318_4"}}
```

(`frequencies_Hz` may replace the grid keys.) One call draws at most
2·10^7 samples (seatings times frequencies); the seatings are summed as
they are drawn, so memory does not grow with their number.

## Interfaces

**CLI.**

```sh
acoustilab measure examples/driver_bench.json --probe zin --out free.zma \
    --noise-db 0.05 --noise-deg 0.3 --seed 3 --ppo 12 --set Bl_Tm=2.5 --set Mms_g=0.33
acoustilab measure examples/driver_bench.json --probe zin --out mass.zma \
    --noise-db 0.05 --noise-deg 0.3 --seed 4 --ppo 12 --set Bl_Tm=2.5 --set Mms_g=0.33 \
    --set added_mass_mg=150
acoustilab fit examples/driver_bench.json --curve zin=free.zma \
    --curve zin=mass.zma --condition added_mass_mg=150 \
    --param Re_ohm --param Bl_Tm --param Mms_g --param Cms_mm_per_N --param Rms_Ns_per_m \
    --out report.json
acoustilab convert free.zma free.csv --ppo 48
acoustilab export examples/design_over_ear.json --probe p_drp --out drp.csv --set leak_gap_mm=0.12
```

`measure` writes FILE and FILE.sidecar.json; `fit` reads each curve's
sidecar from FILE.sidecar.json (or `--curve PROBE=FILE:SIDECAR.json`), takes
`--condition NAME=VALUE` for the preceding curve, `--param NAME=START`,
`--spec FIT.json` (the JSON form above), `--band F1:F2`, `--starts`,
`--seed`, `--max-iter`, `--allow-spl-only`, and prints a summary (`--json`
for the report); `--set` fixes parameters (for `measure`, the true values).
`export` writes a simulated probe (on the netlist's sweep, or on the
exchange grid with `--ppo`/`--band`) with its sidecar. The fit above prints,
among others:

```text
  Bl_Tm      2.501629 Tm   95 % [2.493494, 2.509791]   Determined
  Mms_g      0.330906 g    95 % [0.328527, 0.333301]   Determined
driver 'drv': the moving mass is 0.3309 g, below about 0.5 g, so the added-mass method is
unreliable (spec Section 12): a 10 mg adhesive dot is 3.022 % of it, and an error of 10 mg
in the 0.1500 g test mass alone moves Mms by 6.667 % and Bl by 3.333 % ...
```

Without `mass.zma` the same fit marks Bl, Mms, Cms and Rms scale-ambiguous.

**wasm** (JSON strings in and out, errors `{"error", "kind", ...}` with kinds
`curve` (+ `line`), `fit_spec`, `fit_refused`, `json` or the engine's):

| export | returns |
|---|---|
| `import_curve(text, options)` | a curve document; `options` is a format name or `{"format", "quantity", "sidecar"}` |
| `export_curve(curve_json, format)` | `{"format", "extension", "text", "sidecar"}` |
| `compare_curves(a_json, b_json, allow_json)` | `{"ok", "blocking": [{field, a, b}], "notes": [...], "message"}` |
| `fit(netlist, spec_json)` | the fit report |
| `probe_curve(netlist, overrides_json, probe)` | a curve document of the solved probe with a `simulated` sidecar |
| `virtual_measure(netlist, spec_json)` | `{"curve", "format", "extension", "text", "sidecar"}`; the rig spec may add `"format"` |
| `curve_uncertainty(curve_json)` | `{"frequencies_Hz", "seatings", "level_dB", "phase_deg"}`: the combined standard uncertainty at each frequency from the sidecar budget (null without such a term); used by the web UI's Fit view (docs/web.md) |

`examples/driver_bench.json` is an identification bench: a driver in its
physical set (Re, Bl, Mms, Cms, Rms, Sd, creep, Le and an external LR-2
branch) with a test mass (`added_mass_mg`) and a sealed box
(`box_volume_cm3`) as parameters, and impedance, displacement and box
pressure probes.

## Verification

`crates/acoustilab/tests/io.rs`, `tests/fit.rs`, the unit tests of each
module, `crates/acoustilab-wasm/tests/fit.rs` and
`crates/acoustilab-cli/tests/fit_cli.rs`.

| check | reference | tolerance |
|---|---|---|
| LM optimum and covariance of ln parameters for a weighted resonance fit | scipy `curve_fit` (analytic Jacobian, `absolute_sigma=False`), `tools/fit/reference.py` | 1e-9 (optimum), 1e-6 of sqrt(C_ii·C_jj) (covariance; the engine's Jacobian is by central differences) |
| singular values and vectors of a 7×4 matrix with a near-null direction | numpy `linalg.svd` | 1e-14·σ_max; vectors 1e-12 (1e-7 for the near-null one) |
| format round trips | exact for CSV; FRD, ZMA, REW within their six decimals | 5e-7 |
| resampling | a level and phase linear in ln f | 1e-9 |
| impedance-only fit of the physical set | the null direction is (1, 2, −2, 2)/√13 in (Bl, Mms, Cms, Rms); Bl²/Mms, Bl²·Cms, Bl²/Rms recovered | 1e-4 (direction), 1 % (combinations) |
| added mass, known volume, laser, calibrated SPL in a box | every parameter recovered within 2.576 sd (99 %) | — |
| interval calibration, 30 seeds × 4 parameters | 95 % coverage ≥ 102 of 120 (binomial: 114 ± 2.4); empirical over reported sd within 0.7–1.3 | measured: 115 of 120; ratios 0.92 to 1.14 |
| Jacobian of network solves (d level and d phase by d ln fs of a driver) | closed-form derivative of the D0 impedance | 1e-6 relative (measured: 1.6e-7 next to the resonance, ≤ 1e-8 elsewhere) |
| model-form error (creep and Le in the data, not the model) | runs z < −3, ρ > 0.5, inflation > 3, intervals ≥ 5× the right model's | — |
| complex averaging | exp(−(2πfσ_τ)²/2) with 4000 seatings | 0.04 |
| case study (over-ear template, impedance + drum, 5 seatings, calibration and coupler errors) | leak gap, front depth, Re within 99 %; the fs–Qms–Qes direction named; Qes/fs and Qms/fs within 2.576 of their sds from the correlation matrix, sds below 1.5 % and 10 % | — |
| sensor calibration (laser curve with 0.5 dB calibration in its budget), 12 seeds | Bl's sd at least the calibration's 0.058 in ln; covered in at least 10 of 12 | measured: 12 of 12 (29 of 30 over 30 seeds; 6 of 30 before the offset) |
| a start 24× from the optimum along a flat direction (pad leak) | converges to the truth | — |
| a signed linear parameter at 0 | determined, interval containing 0 | — |

Cost of the case-study fit (6 parameters, impedance and drum response,
native release build, `fit_cost`): 0.8–0.9 s for 72 points per curve,
2.2–2.5 s for 215 and 4.3–4.4 s for 430 (160 evaluations, 11 iterations;
about 0.06 ms per frequency point and solve; the ranges are two runs).

## Limitations

* The intervals are linearised and assume independent residuals; the
  autocorrelation inflation only partly corrects correlated errors, and a
  structured residual means the model is missing something. Of the
  systematic budget terms only the calibration's common part is modelled
  (as a level offset); the coupler, fixture-to-human and numerical terms
  weight the points as if independent, so a parameter that responds to a
  broad level trend can come out more precise than those terms allow, and
  a trend absorbed by a parameter leaves no structure in the residuals.
* A trial point is rejected when the network cannot be solved there, but
  a Jacobian that cannot be formed at an accepted point (both sides of a
  difference fail) ends that start with an error.
* The Section 12 rules recognise drivers by element type and the scale keys
  of the D0 set; the D2 surround keys are not part of the scale rule.
* A test mass counts only when it is a `mass` element set by a parameter
  that differs between conditions and is not fitted.
* No CMA-ES; multi-start by Latin hypercube instead.
* The coupler tolerance of a standard is not bundled (copyright): the user
  supplies it as a table in the sidecar.
