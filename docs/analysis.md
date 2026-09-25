# Design analysis

Sensitivities, tornado charts, explain sentences, Monte Carlo and design of
experiments, and automated readouts, all computed by re-solving the netlist
(spec Sections 3, 10, 12 and 15; errata E15, E24 and E32). The numbers are
only as good as the models they re-solve, and nothing here has been compared
with a measurement rig yet.

| analysis | Rust (`acoustilab::analysis`) | wasm export | CLI |
|---|---|---|---|
| sensitivities, heat map | `sensitivity::jacobian`, `Jacobian::heat_map` | `sensitivity` | `acoustilab sens` |
| tornado chart | `tornado::tornado` | `tornado` | `acoustilab tornado` |
| explain sentences | `explain::explain` | `explain` | `acoustilab explain` |
| readouts | `readouts::readouts` | `readouts` | `acoustilab readouts` |
| Monte Carlo, DOE | `mc::plan`, `mc::run`, `mc::envelope`, `mc::to_csv` | `mc_plan`, `mc_run`, `mc_envelope`, `mc_csv` | `acoustilab mc` |

Every analysis starts from a `Design`: a parametric netlist
(`docs/parameters.md`) and the **base overrides** the user is looking at
(`Design::parse(text, &overrides)`). Each evaluation re-expands the netlist
with some parameters changed, compiles it and solves it exactly as
`Circuit::solve` does, drive scaling included. Every result carries the
engine version (`engine`), the drive (`drive`, as `meta.drive` of a solve)
and the reproducibility hash of the base design's expanded netlist (`hash`;
see "Reproducibility hash" below).

The wasm exports take the netlist text, the overrides (`{"name": value}`,
"" for none) and an options object ("" for the defaults), and return the
result document or `{"error", "kind", ...}`. Engine errors keep their kinds
(`parameter`, `probe`, `element`, ...); malformed or invalid options have
kind `options`.

The **credible band** is the set of grid frequencies without validity
shading, upper or lower (`docs/netlist.md`, "Validity limits and shading").
Explain sentences use only this band. Readouts report the shading at each
frequency they find. Sensitivities and envelopes cover the whole grid and
carry the shading, so a UI can grey out the rest (spec Section 2, rule 4).

## Sensitivities

**Method (erratum E15): central differences of complete solves in
log-parameter space.** For a continuous parameter p (a number, not an
integer, not derived, not zero), the design is solved at p·e^{+h} and at
p·e^{−h}. For every probe y and grid frequency:

```text
dB per %       S_dB  = [20·log10|y(p·e^h)| − 20·log10|y(p·e^−h)|] / (2h) / 100
degrees per %  S_deg = arg[y(p·e^h) / y(p·e^−h)] (degrees) / (2h) / 100
```

This is the derivative with respect to ln p divided by 100 (spec Section 3).

The stepped designs can be evaluated in two ways (option `method`):

* `complete_solves` (the default, and the reference): each stepped design is
  expanded, compiled and solved like any design.
* `forward_sensitivity`: the spec's "reusing the factorisation", with the
  method named as erratum E15 asks. At each frequency the base matrix A₀ is
  factored once, and each stepped design's solution is the first-order
  update x₀ + A₀⁻¹·(b − A·x₀), with A and b stamped at the stepped value.
  This is x₀ + h·dx/d(ln p), where dx/dp = A₀⁻¹·(db/dp − (dA/dp)·x₀) and the
  stamp derivatives are taken over the same step. Only the elements whose
  expanded records the step changes are restamped (the assembly is a sum of
  element stamps; a change of `air` or `level` restamps everything). The
  update is one chord (modified Newton) step. With A = A₀ + h·A₁ + O(h²)
  and r = b − A·x₀ = h·r₁ + O(h²), it differs from the exact stepped
  solution x₀ + A⁻¹·r by (A₀⁻¹ − A⁻¹)·r = h²·A₀⁻¹A₁A₀⁻¹r₁ + O(h³). The h²
  term is even in h, so it cancels in the central difference and in the
  one-sided formula. The O(h³) remainder leaves an O(h²) error, the same
  order as the difference's own truncation, so both schemes stay second
  order. On a lightly damped pressure chamber, for h from 1e-2 to 1e-5,
  the forward path's error was 0.94 to 1.24 times that of complete solves
  against the closed form. The drive factor and the probes are evaluated on each stepped
  circuit, exactly as for a solve. On the template this method is 3.7 to
  4.4 times faster. It agrees with complete solves to 4e-8 of each
  parameter's largest sensitivity in the credible band, and to 3e-6 next to
  the lightly damped depth resonance in the shaded band, which is about
  either method's own truncation error there. The adjoint method (spec
  Section 3) would be cheaper still for many parameters and few probes.

* **Drive.** Probes are the solve's, at the stated drive. A characteristic
  drive renormalises the level at 500 Hz by a real factor. Each level
  derivative is therefore the voltage-drive one minus the reference probe's
  at 500 Hz, and phases are unchanged. Under `power_mW` the power is itself a
  parameter, worth +0.0434 dB/% on every level.
* **Step h = 1e-5** (option `step`, at most 0.05). The truncation error of
  the central difference is h²/6·g''' and the rounding error is ε/h. Near a
  resonance of quality Q the derivatives grow like (2Q)^k, so the relative
  truncation error is (2Q·h)²/6: 7e-9 at Q = 10, 1e-6 at Q = 120. On
  `examples/design_over_ear.json` the solve noise ε is about 1e-14 dB. On
  that design, the difference between step h and step 1e-5, relative to
  each parameter's largest sensitivity, was:

  | h | 1e-3 | 1e-4 | 3e-5 | 3e-6 | 1e-6 | 1e-7 |
  |---|---|---|---|---|---|---|
  | `front_depth_mm` (depth resonance, shaded band) | 1.5e-2 | 1.6e-4 | 1.3e-5 | 1.5e-6 | 1.6e-6 | 1.6e-6 |
  | `driver_Sd_cm2` | 5.1e-4 | 5.0e-6 | 4.1e-7 | 4.6e-8 | 5.0e-8 | 5.0e-8 |
  | `driver_Re_ohm` (level linear in ln p) | 7e-11 | 7e-11 | 8e-11 | 2.2e-10 | 6.3e-10 | 5.3e-9 |

  The h² scaling above 1e-5 is truncation. The plateau below 1e-5 is the
  truncation error of the h = 1e-5 reference itself. The growth in the last
  row at small h is rounding (ε/h). At h = 1e-4 the error near the lightly
  damped depth resonance would be 1.6e-4. At 1e-5 it stays below 2e-6
  everywhere, with rounding near 1e-10. The closed-form tests check both
  terms.
* **Bounds.** When p·e^{±h} would leave [min, max], the second-order
  one-sided formula (−3g₀ + 4g₁ − g₂)/(2h) is used, with points at 0, h and
  2h (or 0, −h and −2h). `scheme` is then `forward` or `backward`, and
  `note` says why.
* **Topology.** Integers, booleans, choices and derived parameters are
  listed in `excluded` with the reason. So are:
  * parameters at zero (a relative step is undefined);
  * parameters whose step changes the frequency grid or the number of
    unknowns (such as a leak segment count computed with `round`);
  * parameters whose step changes the structure of the expanded netlist: an
    `enabled` condition, an element type or a string switches inside the
    step, so the documents differ once all numbers are replaced by null.

  A kink or jump that leaves the structure unchanged (`round`, `floor`,
  `min`, `max`, `abs`, `clamp` or `if` in an expression) is flagged in
  `warnings`. The test is that the forward and backward one-sided differences
  disagree by more than half the probe's largest central derivative.

Options (all optional): `{"parameters": [names], "probes": [ids], "step":
1e-5, "method": "complete_solves" | "forward_sensitivity"}`. A name or id
listed twice is an error, here and in the other analyses. Result:

```json
{
  "method": "central differences of complete solves in ln(p); ...",
  "step": 1e-5,
  "frequencies_Hz": [10.0, ...],
  "probes": ["p_drp", "zin"],
  "parameters": [
    {"name": "driver_Mms_g", "label": "Moving mass Mms", "value": 0.3, "unit": "g",
     "scheme": "central", "warnings": [],
     "dB_per_pct": [[...per frequency...], [...next probe...]],
     "deg_per_pct": [[...], [...]]}
  ],
  "excluded": [{"name": "vent_count", "reason": "integer: not a continuous parameter"}],
  "shading": {"begin_hz": 1061.3, "deep_hz": 1962.3, "low_begin_hz": 100.0, "low_deep_hz": null},
  "drive": {...}, "hash": "...", "engine": "acoustilab 0.1.0"
}
```

`dB_per_pct[j][k]` is probe `probes[j]` at `frequencies_Hz[k]`. Values that
do not exist (for a zero probe) are `null`. The **heat map** of one probe is
a slice of this document, `Jacobian::heat_map(probe)`: `{probe, parameters,
labels, frequencies_Hz, dB_per_pct: [parameter][frequency], shading}`. A UI
can split a long map over several calls with a few `parameters` in each;
each call also solves the base design once.

## Tornado charts

For one metric, each parameter is moved to the ends of its tolerance and the
design is **re-solved**, not linearised. The ends are:

* `normal`: the 2σ points, value ± t;
* `lognormal`: the 2σ points, value/(1 + rel) and value·(1 + rel);
* `uniform`: the full range, value ± t.

The ends are clipped to the bounds (`clipped_low`, `clipped_high`).
Continuous parameters without a tolerance move ±10 % (`default_rel`) and are
marked `"basis": "assumed"`. Rows are sorted by the larger absolute change.
A move that crosses an `enabled` condition still gives a valid design; its
row has `topology_changed: true`.

Metrics:

| `kind` | keys | value |
|---|---|---|
| `level` | `probe`, `f_Hz` | exact solve at the pinned frequency: dB SPL for a pressure, \|y\| in the probe's unit otherwise |
| `band_mean` | `probe`, `f_min_Hz`, `f_max_Hz` | mean of 20·log10\|y\| (dB SPL for pressures) over the grid frequencies in the band, i.e. a mean over log frequency |
| `readout` | `name` | a scalar readout (see below), e.g. `coupled_resonance_Hz`, `bass_extension_Hz` |

Options: `{"metric": {...}, "parameters": [...], "default_rel": 0.1,
"readouts": {readout options}}`. The default metric is the level of the
response probe at 1 kHz. Result: `{metric, description, unit, base,
shading, rows: [{name, label, value, low_value, high_value, basis,
tolerance, clipped_low, clipped_high, metric_low, metric_high, delta_low,
delta_high, effect, topology_changed, errors}], excluded, drive, hash,
engine}`.

## Explain sentences

Spec Section 15 asks for sentences such as "raising front volume 10 percent
lowers drum pressure 1.8 dB from 40 to 300 Hz and moves the coupled
resonance from 1.20 to 1.14 kHz". That example is physically impossible
(erratum E24), which is why no sentence is ever written by hand. For each
continuous parameter, the design is re-solved with the parameter raised by
`step_pct` (10 %). If raising would pass its maximum, it is lowered instead.
ΔdB(f) of the pressure probe is then read on the grid:

1. Only credible frequencies count, meaning unshaded in both solves.
2. A parameter's effect is max |ΔdB| over those frequencies. The `top` (5)
   largest effects of at least `threshold_dB` (0.3 dB) get a sentence; the
   rest are listed in `quiet` with their effect.
3. A band is a run of consecutive credible grid frequencies where ΔdB stays
   beyond ±threshold with one sign. The sentence states the band holding
   the largest |ΔdB| and, of the others, the one with the largest
   |mean| × points. Each gets its **mean** ΔdB ("on average") and
   its first and last grid frequencies. When the band's largest |ΔdB| is
   more than twice its mean, that value and its frequency follow in
   parentheses: next to a resonance the mean alone understates the change.
4. The coupled resonance is mentioned only when it is robust (which
   excludes an ambiguous one) and unshaded in both designs, and the two
   values differ at three significant digits.

On the template: "Raising diaphragm area Sd 10 % lowers p_drp by 1.4 dB on
average from 100 to 945 Hz (4.6 dB at 893 Hz), raises it by 4.2 dB on
average from 1.00 to 1.06 kHz and moves the coupled resonance from 934 Hz
to 1.03 kHz."

A parameter whose change alters the netlist's structure is skipped and
listed in `skipped` with the reason. Options: `{"probe", "top", "step_pct",
"threshold_dB", "parameters", "driver"}`. Result:

```json
{
  "probe": "p_drp", "step_pct": 10, "threshold_dB": 0.3, "statistic": "mean",
  "frequencies_Hz": [...], "credible": [false, ..., true, ...],
  "base_resonance": {"f_Hz": 934.1, "driver": "drv", "prominence_dB": 25.9, "competing": null,
                     "ambiguous": false, "robust": true, "shading": 0},
  "sentences": [
    {"text": "Raising ...", "parameter": "driver_Sd_cm2", "label": "Diaphragm area Sd",
     "direction": "raise", "from_value": 10, "to_value": 11, "effect_dB": 5.38,
     "bands": [{"f_min_Hz": 100.1, "f_max_Hz": 945.4, "effect": "lowers", "mean_dB": -1.42,
                "max_abs_dB": 4.63, "at_Hz": 892.5, "first": 80, "last": 158}, ...],
     "stated": [0, 1],
     "resonance": {"from_Hz": 934.1, "to_Hz": 1026.6},
     "delta_dB": [...per frequency...]}
  ],
  "quiet": [{"name": "driver_Re_ohm", "effect_dB": 0.41}],
  "skipped": [{"name": "rear", "reason": "choice: not a continuous parameter"}],
  "drive": {...}, "hash": "...", "engine": "..."
}
```

`first` and `last` index `frequencies_Hz`, so a UI can highlight the plot
region a sentence talks about.

## Readouts

Each readout states its method in `methods`. Values between grid points are
refined on exact re-solves, never interpolated: maxima by Brent's method
and crossings by the Illinois method, both in ln f. A maximum's location is
accurate to about 1e-7 relative (flat minima to 1e-6), since Brent's method
cannot locate an extremum better than √ε in ln f.

**Impedance.** The probe is the `impedance` option, else the impedance the
`vsource` sees, which excludes its source impedance.

* `peaks`: interior local maxima of |Z| with at least 0.1 dB topographic
  prominence. `resonance` (in situ) is the first peak and `z_max` the
  highest.
* `Re_ohm`: the `Re_ohm` option, else Re Z extrapolated to 0 Hz from the two
  lowest frequencies, assuming Re Z − Re ∝ f². That is the low-frequency law
  of a moving coil behind a passive load. `re_source` says which was used.
* `q`: Qms, Qes and Qts by the sqrt(r0) method (Small 1972; derivation at
  `ts::extract_ts`): r0 = Z(fres)/Re, f1 < fres < f2 where |Z| = Re·√r0,
  Qms = fres·√r0/(f2 − f1), Qes = Qms/(r0 − 1), Qts = Qms/r0. The method is
  exact for a single lumped resonance with a real Re and no inductance. In
  situ, the acoustic load's losses count as mechanical ones. A note is added
  when √(f1·f2) is more than 1 % from fres, or when there is more than one
  peak. The upper crossing is searched only below the next peak. When |Z|
  stays above Re·√r0 up to that peak, the two resonances overlap, the
  method does not apply, and `q` is `null` with a note. Before this check,
  a 0.3 mm leak on the template gave Qms = 0.34 from a bandwidth that
  spanned both peaks.
* `z_1kHz_ohm` is the nominal impedance, and `min_above_resonance` (with
  `min_at_sweep_end`) the smallest |Z| above the resonance.
* `rated_check`: |Z| below 80 % of the rated impedance anywhere in the sweep
  (spec Section 4), with the violating grid ranges. The rated impedance is
  the `rated_ohm` option, else the drive key's.

**Drivers.** For every `driver` element:

* its D0 identities: fs, Qms, Qes, Qts, Re, Bl, Mms, Cms, Rms and Sd;
* `free_air`: the sqrt(r0) readouts of `Driver::unloaded_impedance`, with
  both acoustic ports at ambient and the driver's DC resistance. This
  includes Le, LR-2, creep and D2 when set.

**Response.** The probe is the `probe` option, else the netlist's
`ui.primary_probe`, else its first pressure probe; `probe_source` says
which. All values are under the stated drive:

* `level_500Hz_dB`, `level_1kHz_dB`: exact solves.
* `sensitivity`: dB SPL per volt of source EMF and per milliwatt into the
  rated impedance, at 500 Hz and 1 kHz. dB/mW = dB/V − 10·log10(1000/Z_rated)
  (erratum E32: never Re). Without a rated impedance, only dB/V is given.
* `bass_extension`: the frequency where the level is 3 dB below its 500 Hz
  level. It is found by walking the grid down from 500 Hz to the first point
  below that level, then refining the crossing. It is `null`, with a note,
  when the level stays within 3 dB down to the start of the sweep. The
  template's sealed over-ear design on the IEC 60318-4 ear is such a case.
* `coupled_resonance`: the frequency of maximum diaphragm velocity per unit
  coil current, |v/i|, of the driver (option `driver`, else the first
  driver). The motor force is Bl·i, so this is the minimum of the total
  mechanical impedance the motor drives: suspension, moving mass and every
  acoustic load. It does not depend on the drive convention, the source
  impedance or the coil inductance. For a lossless lumped cavity it is
  exactly spec Appendix C2's f_c = (1/2π)·√((1/Cms + Sd²/Caf)/Mms). The
  response peak at the drum was not used, because ear-simulator and canal
  resonances compete with it.

  With a single resonance, the three definitions agree. On the template,
  closed or open back, with either ear, the |v/i| maximum, the in-situ
  |Z| peak and the drum-pressure peak lie within 1 % of each other.

  Open vents or a large leak add a second resonance, and then no single
  frequency is *the* coupled resonance. Every maximum of |v/i| with at
  least 1 dB of topographic prominence counts as a resonance. Those
  within 10 dB of the highest are refined, and the highest is reported.
  The next one is reported as `competing: {f_Hz, margin_dB}`.

  When the margin is below 3 dB, the value is `ambiguous`. A few percent
  of a parameter can then swap the two peaks. With a 1 mm leak on the
  template (margin 1.2 dB), +5 % of Mms moves the maximum from 1185 to
  516 Hz. An ambiguous resonance is left out of `scalars()`, so a tornado
  or a Monte Carlo run never mixes the two.

  The value is `robust` when |v/i| at the peak exceeds its values one
  octave either side by at least 1 dB and it is not ambiguous. The
  impedance `resonance` is the *first* |Z| peak. In a two-resonance
  design it can name the other resonance: with a 0.3 mm leak, the first
  |Z| peak is at 349 Hz and the coupled resonance at 996 Hz.

  Result: `{f_Hz, driver, prominence_dB, competing, ambiguous, robust,
  shading}`.

Options: `{"probe", "impedance", "rated_ohm", "Re_ohm", "driver"}`. The
result nests `impedance`, `drivers`, `response`, `notes`, `methods`,
`drive`, `hash` and `engine`. `Readouts::scalars()` flattens it to the names
that a tornado or a Monte Carlo run uses as metrics:

`level_500Hz_dB`, `level_1kHz_dB`, `sensitivity_500Hz_dB_per_V`,
`sensitivity_1kHz_dB_per_V`, `sensitivity_500Hz_dB_per_mW`,
`sensitivity_1kHz_dB_per_mW`, `bass_extension_Hz`, `coupled_resonance_Hz`,
`z_resonance_Hz`, `z_resonance_ohm`, `z_max_Hz`, `z_max_ohm`, `Re_ohm`,
`Qms`, `Qes`, `Qts`, `z_1kHz_ohm`, `z_min_above_resonance_Hz`,
`z_min_above_resonance_ohm`, `z_min_ohm`, `z_min_over_rated`.

A readout that does not exist for a design is `null`: no bass extension
in the sweep, no Q for overlapping resonances, or an ambiguous coupled
resonance. A tornado refuses a metric that is `null` for the base design.
A Monte Carlo envelope counts only the runs that have the metric (`n`).

## Monte Carlo and design of experiments

A run is planned once, solved in chunks, and then summarised:

```text
mc_plan(netlist, overrides, spec)                        -> {samples: [{index, overrides}], ...}
mc_run(netlist, overrides, {"plan": spec, "first": i, "count": k}, options)
                                                         -> {frequencies_Hz, samples: [results]}   (repeat)
mc_envelope({frequencies_Hz, samples})                   -> median, 5/10/90/95 %, min, max
mc_csv({engine, samples}, parameters)                    -> {"csv": "..."}
```

Each call is bounded, so a worker can report progress and cancel between
calls. Results are identical however the samples are chunked. `mc_run` also
accepts a JSON array of samples. The plan-and-range form is preferred: the
plan is made in Rust (once, and kept for the following calls with the same
netlist, overrides and spec), so sampled values never pass through a JSON
parser. serde_json's float parsing is best effort and can move a value by
one ulp. The same applies to the numbers that `mc_envelope` and `mc_csv`
read back; the hashes are unaffected.

### Plans

```json
{"method": "lhs", "n": 200, "seed": 1, "parameters": ["driver_fs_Hz", "leak_gap_mm"]}
{"method": "factorial", "factors": {"ear": ["iec60318_4", "type43"],
                                    "vent_count": {"levels": 3, "from": 0, "to": 4},
                                    "leak_gap_mm": {"levels": 2, "from": 0.02, "to": 0.2, "log": true}}}
{"method": "runs", "runs": [{"rear": "open"}, {"rear": "closed", "vent_count": 2}]}
```

**Latin hypercube** (`lhs`). `parameters` defaults to every continuous
parameter with a tolerance, in declaration order (a name listed twice, or
nothing to sample, is an error; n is 1 to 100 000). The generator is
seeded with `seed`, and for each parameter in turn:

1. Draw a permutation π of 0..n by Fisher–Yates: `below(i + 1)` for i from
   n − 1 down to 1, with Lemire's unbiased bounded integers.
2. Draw n jitters v = ((x >> 32) + ½)·2⁻³², where x is the next 64-bit
   output.
3. Set u_i = (π(i) + v_i)/n, so each of the n strata of (0, 1) is hit once.
4. Map u through the tolerance (table in `docs/parameters.md`), with μ the
   parameter's current value after the base overrides:
   * normal: μ + (t/2)·Φ⁻¹(u);
   * uniform: μ + t·(2u − 1);
   * lognormal: μ·exp(ln(1 + rel)/2·Φ⁻¹(u)).
5. Clip to [min, max]. Clipped values are counted per parameter (`clipped`
   in `distributions`) and listed per sample.

The generator is xoshiro256** seeded through SplitMix64, using integer
arithmetic only. Φ⁻¹ is Wichura's AS 241 (PPND16, accurate to 1e-16).
Φ⁻¹ and the lognormal exponential use a logarithm and an exponential built
from IEEE basic operations (`analysis::detmath`). A seed therefore gives
**bit-identical samples on every platform**. This was checked for a 200-run
plan of the template on x86-64 and on wasm32 under Node. The solved runs
are not bit-identical across platforms, since the engine uses the platform
libm, but the hashes are. On the first 40 runs of that plan, the largest
difference was 7e-7 relative, in the location of the flat impedance
minimum (a refined extremum; see "Readouts"); everything else agreed to
2e-13. Seeds are unsigned 64-bit integers; keep them below 2⁵³ when
they pass through JavaScript.

**Factorial**: the full factorial of the levels, with the first factor
varying slowest. Levels are values of any kind (numbers, booleans, choices)
or `{"levels": k, "from": a, "to": b, "log": false}` (k from 2 to 100 000);
integer parameters are rounded. The product of the level counts is at most
100 000. **Runs**: explicit override objects. Both kinds are checked
against the parameters' kinds and bounds when the plan is made.

Plan result: `{method, seed, parameters, distributions: [{name, dist,
nominal, half_width, sigma, sigma_ln, min, max, clipped, source}], factors:
[{name, levels}], samples: [{index, overrides, clipped}], clipped, engine}`.

### Runs

Options: `{"probes": [ids], "metrics": true, "readouts": {readout
options}}`. Each run is the design with the sample's overrides on top of the
base overrides. Result (`RunChunk`):

```json
{
  "engine": "acoustilab 0.1.0",
  "frequencies_Hz": [...],
  "samples": [
    {"index": 0, "overrides": {"driver_fs_Hz": 75.9, ...}, "hash": "bd24...", "ok": true,
     "curves": [{"id": "p_drp", "dB": [...]},
                {"id": "zin", "magnitude": [...], "phase_deg": [...]},
                {"id": "x", "magnitude": [...]}],
     "metrics": {"bass_extension_Hz": null, "coupled_resonance_Hz": 956.2, ...}}
  ]
}
```

Pressures report dB SPL, impedances |Z| and phase, and other probes their
magnitude. A run that fails, for example on a singular matrix or an element
rejecting a value, has `"ok": false` and its `error`; the other runs go on.
A run whose grid differs from the base design's carries its own
`frequencies_Hz`.

### Envelopes and tables

`mc_envelope` takes `{"frequencies_Hz", "samples"}`, with the samples of
all chunks together. For every probe quantity and every metric it returns
the median, the 5, 10, 90 and 95 % points, the minimum, the maximum and the
count of finite values; for curves, these are per frequency:

```json
{"frequencies_Hz": [...], "runs": 200, "failed": 0, "other_grid": 0,
 "probes": [{"id": "p_drp", "dB": {"median": [...], "p5": [...], "p10": [...], "p90": [...],
                                    "p95": [...], "min": [...], "max": [...], "n": [...]}}],
 "metrics": {"coupled_resonance_Hz": {"median": 935.3, "p5": 916.9, "n": 200, ...}},
 "percentile_method": "linear interpolation between order statistics (numpy default; Hyndman and Fan type 7)"}
```

Percentiles interpolate linearly between order statistics, h = (n − 1)·q,
exactly as `numpy.percentile` does by default. `mc_csv` writes the
design-of-experiments table (RFC 4180) with the columns `run`, `hash`,
`engine`, the plan's parameters, every metric, and `error`. Numbers are in
shortest round-trip form; missing values are empty.

### Reproducibility hash

Every run, and every analysis result, carries the SHA-256 of a canonical
text of its **expanded netlist**. That is the document the engine reads,
after expressions are replaced and disabled items removed, without `title`,
`description`, `ui` and `schema`. The canonical text:

* sorts object keys by UTF-8 bytes and has no whitespace;
* escapes strings as serde_json does;
* writes every number, integer or not, in scientific notation with 12
  significant digits, correctly rounded, with trailing zeros removed and a
  plain exponent (`25 → 2.5e1`, `0.08 → 8e-2`, `0 → 0`).

Twelve digits, rather than a full round trip, make the hash immune to
last-bit differences between the native and wasm32 libm in derived
parameters. A one-ulp change alters the text only when the value lies within
one ulp of a 12-digit rounding boundary, about 1e-4 per affected value. Two
runs with equal hashes solved the same netlist to 12 digits. A parameter
that only feeds a disabled element does not change the hash. The engine
version is reported next to the hash. `tools/analysis/reference.py`
reimplements the text, and the tests compare against Python's `hashlib`.

## CLI

```sh
acoustilab sens examples/design_over_ear.json --probe p_drp          # largest dB/% in the credible band
acoustilab sens examples/design_over_ear.json --csv > sens.csv       # frequency x (parameter@probe)
acoustilab sens examples/design_over_ear.json --method forward_sensitivity --json
acoustilab tornado examples/design_over_ear.json --probe p_drp --band 100 1000
acoustilab tornado examples/design_over_ear.json --readout coupled_resonance_Hz
acoustilab explain examples/design_over_ear.json --set rear=open
acoustilab readouts examples/design_over_ear.json --json
acoustilab mc examples/design_over_ear.json -n 200 --seed 1 --csv > doe.csv
```

Every subcommand takes `--set NAME=VALUE`, `--json` for the full result
document and `--out FILE`. `mc` reports its progress on stderr.

## Verification

The tests are `crates/acoustilab/tests/analysis.rs` (tolerances and their
reasons in its header), `crates/acoustilab-wasm/tests/analysis.rs`, and
unit tests in each module.

* **Sensitivities against closed forms**, with both methods.
  * A series RC, H = 1/(1 + jωRC) and Zin = R + 1/jωC: R and C, magnitude
    and phase, to 1e-7 relative.
  * The central-difference error equals h²/6·g''' at h = 1e-3 and 1e-4,
    within 5 %.
  * A lossless sealed cavity: −20/ln 10/100 dB per % of volume at every
    frequency, to 1e-10.
  * A D0 driver in a lossless pressure chamber: all seven of Bl, Sd, Mms, V,
    Re, Kms and Rms against the complex log-derivatives of the lumped closed
    form (and Zin for Bl and Re), to 1e-7.
  * The characteristic and power drives against the voltage drive.
  * Forward sensitivities against complete solves on the template under
    all five drive conventions (power, voltage, characteristic, current,
    none) and at levels 0 and 1: 1e-6 of the largest sensitivity in the
    credible band, 1e-5 in the shaded band.
  * One-sided schemes at both bounds.
  * The exclusions (integer, boolean, choice, derived, zero, an `enabled`
    switch inside the step, a grid change) and the kink warning.
* **Readouts against closed forms.**
  * The free-air impedance of a D0 driver recovers fs, Qms and Qes to 1e-7
    on a 12-per-octave grid (1e-6 with the extrapolated Re).
  * The coupled resonance and the in-situ peak equal C2's f_c (1204 Hz) to
    1e-7, with peak height Re + Bl²/Rms (C5).
  * The bass extension of a first-order high-pass, to 1e-9.
  * Levels equal exact solves; dB/V − dB/mW = 10·log10(1000/Z_rated); a
    characteristic drive gives 94 dB at 500 Hz.
  * The rated-impedance check finds the analytic violating band.
  * Two overlapping |Z| resonances (series RLC tanks, closed form): no Q.
  * A vented box with two |v/i| peaks 0.45 dB apart: both located to 1e-7
    and the margin to 1e-9 against the closed form, flagged ambiguous and
    left out of the scalars; with a 7 dB margin, robust and reported.
* **Tornado.** RC ends against |H| (1e-12), assumed ±10 %, uniform and
  lognormal ends, clipping, band means against the solve, and the coupled
  resonance against 1/√Mms.
* **Explain.** Every sentence for the template (closed and open back) and
  for a pressure chamber is re-checked against re-solves made in the test:
  the effect; each band's points, sign, maximality, mean and frequencies;
  the numbers in the text; and the resonance shift against the readouts.
  Erratum E24 is checked too: +10 % front volume lowers the C2 chamber's bass
  by only 0.15 dB, which no sentence mentions at the default threshold.
* **Against the Python reference** (`tools/analysis/reference.py`):
  * SplitMix64 and xoshiro256** streams, bounded integers and a shuffle,
    exactly;
  * Φ⁻¹ against mpmath, to 1e-15 relative from p = 1e-300 to 1 − 2⁻⁵³;
  * a Latin hypercube plan: unit points bit-identical, values to 1e-14,
    clipping included;
  * canonical texts and SHA-256, exactly (plus a pinned text and hash of a
    small parametric netlist written in the test, not the template, so
    that template edits do not break it);
  * percentiles against `numpy.percentile`, to 1e-12.
* **Monte Carlo.**
  * Every stratum is hit once.
  * The moments of 20 000 samples: normal mean and σ; uniform range and
    σ = t/√3; lognormal median and σ_ln, and the 2.3 % tails beyond
    μ·(1 + rel) and μ/(1 + rel).
  * Determinism, and chunked runs equal to one run, both natively and
    through the wasm API.
  * Curves equal the solve's, and hashes equal the expanded netlist's.
  * A failing run is reported while the others go on.
  * Factorial order and validation, and CSV quoting.

## Performance

Measured on `examples/design_over_ear.json` (265 frequencies, 37 unknowns,
7 probes, 19 continuous parameters), release builds, one core of the
development container:

| task | native | wasm32 (Node 22) |
|---|---|---|
| one solve | 19 ms | 22 ms |
| readouts (solve included) | 22 ms | 26 ms |
| full sensitivity map, complete solves (39 solves) | 0.62 s | 0.82 s |
| full sensitivity map, forward sensitivities | 0.15 s | 0.21 s |
| sensitivity map, 16 parameters, complete solves / forward | not measured | 0.69 s / 0.18 s |
| full sensitivity map at 96 points per octave (1053 frequencies) | not measured | 3.3 s |
| tornado at a pinned frequency | 13 ms | 14 ms |
| tornado of the coupled resonance (a readout per end) | 0.79 s | 1.0 s |
| explain (default options) | 0.34 s | 0.41 s |
| Monte Carlo plan, 200 runs | not measured | 4 ms |
| 200 Monte Carlo runs with metrics | 4.1 s | 5.1 s (10 calls of 20 runs, 0.5 s each) |
| envelope of 200 runs | not measured | 93 ms |

A solve's time is 74 % LU factorisation and refinement and 26 % stamping.
Forward sensitivities replace a parameter's two complete solves by the
restamping of the elements it changes and two back substitutions against
the base factors. Measured in process (best of five): 0.62 s against 0.15 s
for the closed-back template (4.1×), 0.54 s against 0.12 s open-back (4.4×),
and 0.32 s against 0.085 s at level 0 (3.7×). Both timings of the
sensitivity map in the table include the base design's own solve.

**Bounds of one call.** A sensitivity map costs two solves per parameter,
a tornado two evaluations of its metric per parameter, and explain one
solve per parameter. A solve's cost grows with the sweep, which the
netlist sets (at most 10⁶ points). A readout adds at most a few dozen
single-frequency solves per peak (Brent's method stops after 200 steps and
the Illinois method after 100). A wasm worker that must report progress
splits a sensitivity map or a tornado by `parameters`, and a Monte Carlo
run by `first` and `count`. Plans hold at most 100 000 runs. A parameter or
probe listed twice is refused, so a request cannot repeat work. Finding
the peaks' prominences walks the grid outwards from each peak: O(n²) for n
grid points in the worst case, negligible below about 10⁴ points.

## Known limits

* Kinks and jumps inside elements, such as a mode count that changes with a
  dimension or a switch between series and asymptotic forms, are caught only
  by the one-sided consistency check, not by the structure comparison.
* Explain sentences state at most two bands; the others are only in
  `bands`. With the one-sign rule, a resonance shift splits into a rise and
  a fall, and small bands beside it can go unmentioned.
* The bass extension walks the grid, so a notch narrower than the grid
  spacing is missed.
* Monte Carlo samples parameter tolerances only. The spec's fit variation
  (Section 8) and ear variation (Section 7) enter only through parameters
  that the netlist exposes, such as `leak_gap_mm`.
* In a two-resonance design, the coupled resonance is the highest |v/i|
  maximum. A large change can hand that maximum to the other resonance
  even when the base design is not ambiguous. On the open-back template
  with a 0.3 mm leak (margin 5.7 dB), the tornado's upper leak end
  (0.45 mm) moves the maximum from 778 to 55 Hz. The `competing` peak
  shows when this can happen. Nothing tracks one resonance across designs.
* A plan holds at most 100 000 runs (`mc::MAX_RUNS`), since it is returned
  whole. That is about 38 MB of JSON for the template.
