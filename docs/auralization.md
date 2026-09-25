# Auralization

This note covers spec Section 16 (auralization) with errata E35, E46 and
E52. The filter is designed by the engine (`crates/acoustilab/src/audition/`,
tests `crates/acoustilab/tests/audition.rs`, independent references from
`tools/audition/reference.py`); playback, level matching, programme
material, meters and the limiter are in the browser (`web/src/audio/`, the
Listen view `web/src/views/listen.view.ts`, tests
`web/tests/audio-dsp.spec.ts` and `web/tests/listen.spec.ts`).

**What can and cannot be verified here.** Nothing in this project has been
listened to: the development container has no audio output. Every
processing step is checked offline, in Rust, in Node and in Chromium's
OfflineAudioContext, against closed forms, direct computations and
published data. The real-time path (an AudioContext playing through the
worklet) is started, metered and switched in headless Chromium, which
renders to a silent device. Whether a difference is audible, whether the
0.5 ms phase threshold is right, and how the tool sounds on real headphones
are open.

## What is auralized

The audition filter is the candidate's response divided by a baseline's at
the same reference point (Section 16, "What is auralized"). The Listen view
plays the programme through the filter (**A**) and the programme itself
(**B**), both loudness matched. The listener's own headphones multiply A
and B alike, so what changes between them is the difference between the
designs. This is the logic of Harman's virtual headphone method, with the
baseline standing where the replicator headphone stood.

| baseline | how it is divided |
|---|---|
| another netlist (a frozen baseline, or the current netlist with the loaded template's parameter values) | exactly: both designs are solved at every bin |
| a target curve (bundled, or a fixture-tagged CSV) | as a magnitude, inverted with regularisation (below) |
| an imported measured curve (FRD, ZMA, REW text, CSV, curve JSON) | the same; its phase is not used |
| none | **absolute** mode: the candidate's own response, uncompensated, flagged `absolute_diagnostic` |

The report flags a candidate and a baseline on different fixtures
(`reference_point_mismatch` for two netlists, `fixture_mismatch` for a
target or curve: responses on different fixtures differ by several dB above
2 kHz, and the filter then contains that difference too), different drives
(`drive_differs`), and a probe that is not a pressure.

## The filter

### Pipeline

1. **Check grid.** Both designs are solved exactly at 48 points per
   octave over the audition band (default 20 Hz to 20 kHz, or 0.9 of
   Nyquist if lower), with the band edges, the inversion band edges and
   the anchor band edges inserted (483 frequencies by default).
2. **Analytic filter D(f).** The ratio |H_c/H_b| (or |H_c|·c(f) for a
   magnitude baseline, or |H_c| alone), exact inside the band. Outside it
   the filter is **held**: it keeps its band-edge level, reached with a
   continuous slope. With s the slope at the edge (dB/octave) and x the
   distance beyond it (octaves), the level is L_edge ± s·(x − x²/2w) for
   x ≤ w and L_edge ± s·w/2 beyond, w = min(1, 12/|s|) octaves, so it adds
   at most 6 dB beyond the edge. A hard hold would leave a kink that a
   finite filter reproduces only to about 0.15 dB at the edge (observed
   0.137 dB at 20 Hz for a leak change, against 0.012 dB with the smooth
   hold). D is normalised to 0 dB mean over 500 Hz–2 kHz (the **anchor**,
   a log-frequency-weighted mean); `anchor_gain_dB` states what was
   removed.
3. **Length (erratum E46).** The filter's analytic response in band is
   vector fitted (`time::poles::fit_values`, order 30). Every resonant pole
   (Q ≥ 1, in band, weight ≥ −20 dB) of the *ratio* (the candidate's poles
   and the baseline's zeros) needs N/(2·fs) ≥ 2.93·Q/f to decay 80 dB. N is
   the larger of the default and the smallest power of two that holds the
   binding pole, capped at twice the default. Defaults scale with the rate
   so the filter keeps its duration: 8192 at 44.1 and 48 kHz, 16 384 at
   88.2 and 96 kHz; the range is half to twice the default (4096 to
   16 384 at 48 kHz, as in the spec). A pole the cap cannot hold is flagged
   (`ir_length_short`).
4. **FFT grid.** Both designs are re-solved on the linear grid f_k = k·fs/N
   (`time::uniform`, every bin up to 0.9 of Nyquist an exact solve, never
   an interpolation of the log grid), with their minimum-phase analysis
   counterparts (`time::minphase`). The bin magnitudes M_k are the
   ratio's, held and normalised as in step 2.
5. **Phase** (below), inverse FFT, window, f32 taps.
6. **Check.** The taps' own frequency response (a direct DTFT of the f32
   values, double precision) against D on the check grid.

### Phase modes

The Section 16 decision is taken on the **filter**, whose excess phase is
the candidate's minus the baseline's (each against its own minimum-phase
counterpart; a magnitude-only baseline counts as minimum phase), over the
bins both designs trust (solved, unshaded, within 60 dB of the peak).

| mode | construction | latency |
|---|---|---|
| `minimum` | cepstral minimum phase of M (`min_phase_fir_spectrum`, 4× refined), with the filter's polarity | 0 |
| `mixed` | below the validity frequency f_v, the model's own complex ratio advanced by the pure-delay estimate (delay alignment); above f_v, minimum phase, crossfaded over 1/3 octave (the **hybrid**: "forced above the model's declared validity frequency") | N/32 samples of pre-response |
| `linear` | M delayed by N/2: a diagnostic (`linear_phase_diagnostic`), since its pre-ringing would be blamed on the design | N/2 |
| `auto` (default) | `minimum` when the filter's largest excess group delay in the trusted band is below 0.5 ms, else `mixed` | |

f_v is the lowest `begin_hz` of the two designs' shading (10 % lumped
error; 1061 Hz for the design template), capped at the band top. In the
crossfade above f_v the excess phase goes to the multiple of 2π *below* it,
so the phase keeps falling and the group delay stays non-negative, unless
the multiple above is within 45°. For a 0.5 ms all-pass lattice, whose
excess phase has reached −178° at 20 kHz, returning it to 0 puts −5.7 dB
of the filter's energy before t = 0; going down to −360° leaves −12.2 dB,
which is the band-limited pulse's own neighbours within 0.13 ms of t = 0
(erratum E52: a response strong up to Nyquist cannot be band-limited
causally). Below the band the excess phase is kept (it tends to 0 or π at
DC); crossfading it to minimum phase over the few bins below 20 Hz made a
narrowband phase feature whose decay outlasted N.

Windows (Tukey): minimum phase, a half-cosine over the last N/16 samples;
mixed phase, a rise over the first N/32 (the pre-response) and the same
fall; linear phase, N/16 at both ends. The report gives the energy of the
last N/16 samples before the window (`tail_energy_dB`) and of the mixed
filter's pre-response (`pre_energy_dB`).

The reason for the chosen mode is an engine sentence in the report, e.g.
"Minimum phase: the filter's largest excess group delay in its trusted
band (105 Hz to 1061 Hz) is 0.001 ms, below the 0.5 ms threshold". The
0.5 ms threshold is the spec's, "to be confirmed from" Blauert and Laws
(1978); it has not been checked against that paper here.

### Inverting a target or a measured curve

For a magnitude-only baseline B (`audition::inversion`), in this order, on
a working grid of 96 points per octave over the curve's range:

1. **Pre-smoothing**: 1/6-octave power smoothing
   (`targets::smoothing::smooth_power`, IEC 61260-1 base-10 windows): B₆.
2. **Notch rule**: E = B₆ smoothed over one octave. A notch is a maximal
   interval where B₆ < E; one whose deepest point lies more than 15 dB
   below E is not inverted: B₆ is replaced by E across the interval
   (continuous at its ends, where B₆ = E). Notches of 3 dB or more are
   listed in the report with their depth and whether they were inverted.
3. **Reference**: b = 10^((B₆ − L_ref)/20), L_ref the mean of B₆ over
   500 Hz–2 kHz (the filter's own anchor band). Boost and cut are relative
   to it. The spec does not say what the cap is relative to; a mean over
   the whole band lets a strong bass make the treble look "low".
4. **Kirkeby–Nelson inverse** (Kirkeby, Nelson, Hamada and
   Orduña-Bustamante, IEEE Trans. Speech Audio Process. 6(2), 1998):
   c = b/(b² + β). Its largest value is 1/(2√β), so the 12 dB boost cap
   fixes **β = 1/(4·10^(12/10)) = 0.015774** inside the band. The cap is
   Kirkeby's own soft limit, reached where the baseline is 18.0 dB below
   L_ref. It costs accuracy towards the cap: the inverse differs from 1/b
   by 20·log10(b²/(b² + β)): −0.03 dB at +6 dB, −0.14 dB at L_ref,
   −0.53 dB at −6 dB, −1.94 dB at −12 dB, −6.02 dB at the cap point.

**Regularisation profile.** β(f) = β_in inside the inversion band
(20 Hz–10 kHz, clipped to the curve's range; the bundled Ravizza target
starts at 31 Hz); outside it the baseline is not inverted at all: the
*audition filter* holds its band-edge level (step 2 of the pipeline). This
is the limit β → ∞ of the Tikhonov problem regularised towards the
band-edge filter rather than towards zero. Classic Kirkeby regularisation
towards zero would low-pass the programme at 10 kHz; holding the edge
keeps it full band, and A and B then differ only inside the band. The
report gives the notches, the largest boost of the inverse and its largest
departure from the exact inverse (smoothing, notch rule and cap together).

Every number above is an option (`inversion`: `band_Hz`, `smoothing`,
`boost_cap_dB`, `notch_limit_dB`) with the spec's value as default.

### Accuracy

Section 16: "the convolved chain must reproduce the analytic response
within 0.1 dB from 20 Hz to 20 kHz". Observed largest error of the taps
against the analytic filter (`tests/audition.rs`):

| filter (48 kHz, N = 8192 unless stated) | largest error |
|---|---|
| RLC ratio (1 kHz, Q = 2 over 1.5 kHz, Q = 0.7), closed form | 3e-5 dB |
| design template, 3 vents over 1 | 0.003 dB |
| front depth 14 mm (a Q = 147 resonance at 12.4 kHz in the ratio) | 0.0006 dB |
| leak gap 0.3 mm (the ratio falls 24 dB below the anchor at 20 Hz) | 0.012 dB |
| open back | 0.005 dB |
| Type 4.3 ear (flagged: another reference point) | 0.004 dB |
| 3 vents, mixed phase / linear phase | 0.074 dB / 0.002 dB |
| 3 vents at 44.1 kHz / 96 kHz (N = 16 384) | 0.002 / 0.003 dB |
| target baseline (Ravizza 2023, 5128) | 0.004 dB |
| 200 Hz, Q = 8 resonance over a divider (N = 16 384 by E46) | 0.0009 dB |

The whole chain gives the same figures: the impulse response of the
worklet, rendered in Chromium's OfflineAudioContext with the engine's taps
loaded (3 vents, minimum and mixed phase at 48 kHz, minimum at 96 kHz),
matches the analytic filter as the taps do, to 1e-3 dB
(`listen.spec.ts`).

Where it cannot be met the report says so and why. A 100 Hz, Q = 20
resonance needs 0.586 s to decay 80 dB (N = 65 536); capped at 16 384 its
tail wraps around the buffer, and the report lists the frequencies beyond
0.1 dB (around the resonance, and wherever the ratio lies far below its
peak) with the frequency resolution fs/N. Detail narrower than fs/N
(5.9 Hz at the defaults) is not reproduced either.

The minimum-phase taps are the minimum-phase sequence of their own
magnitude (to 7e-14° against a cepstrum of their DTFT computed in the
test). Their phase differs from the *analog* minimum phase by a near-
constant group delay (0.38 µs, 0.02 sample, for the RLC ratio): a discrete
filter's Hilbert transform stops at Nyquist.

## Playback

### Chain

`web/src/audio/processor.ts`, an AudioWorklet:

programme (looping AudioBuffer) → partitioned convolution (slot A or B,
crossfaded) → volume (a k-rate AudioParam, ramped across each quantum) →
true-peak limiter → output.

- **Uniformly partitioned overlap-save convolution** (`convolver.ts`):
  block B = 128 (the render quantum), FFT 2B = 256, one frequency-domain
  delay line shared by all filter banks, independent left and right
  filters. It equals direct convolution exactly (1e-12 in double
  precision in Node; 1e-6 through the worklet's f32 I/O in Chromium).
  Arithmetic is double-precision JavaScript, not the WebAssembly SIMD FFT
  the spec mentions: a 16 384-tap filter at 48 kHz costs about 50 million
  multiply-adds per second per channel.
- **Filter updates** (a slider drag, A/B) crossfade linearly over two
  quanta between two outputs that are both exact convolutions of the same
  input history: out = (1 − g)·y_old + g·y_new, g = (k + 1)/256. A new
  filter is transformed four partitions per channel per quantum
  (16 quanta, 43 ms for 8192 taps), so no quantum does the whole
  transform. Three banks: A, B, and one loading.
- **No allocation or message posting inside `process()`**: every buffer
  is allocated when the node is created; messages are handled in the
  port's `onmessage`. The page reads the processor's state (the playing
  slot, loading, fading) and the limiter's gain from two extra outputs
  through AnalyserNodes.
- **ConvolverNode fallback** (no AudioWorklet, e.g. an insecure context;
  also a diagnostics switch): two ConvolverNodes per slot with
  `normalize = false` set before the buffer is assigned (the default
  normalisation rescales the filter and destroys the level match: by more
  than 1 dB for the template filter in the test), A/B by 5 ms gain ramps,
  and a DynamicsCompressorNode standing in for the true-peak limiter,
  which it is not. Erratum E35: WebKit and Gecko move late partitions of
  long ConvolverNode filters to a background thread; Chromium does not.

### Level matching and safety

- **BS.1770-5 integrated loudness** (`loudness.ts`, `level.ts`), computed
  over the actual programme after convolution: the programme loops, so the
  listener hears its circular convolution with the filter, computed
  exactly (FFT overlap-save over the periodic extension), with the
  K-filters in their periodic steady state. Integrated loudness scales
  exactly with gain, so one measurement per filter gives the gain to the
  target (−23 LUFS before the volume control): A and B match to rounding.
  Rendered through the worklet and re-measured, they agree within 0.1 LU
  (`listen.spec.ts`). The work runs on its own worker.
- **Alternatives**: RMS (used automatically for the sine sweep, a pure tone
  at every instant and so outside the loudness algorithm's scope), and the
  mid-band anchor (the engine's 500 Hz–2 kHz normalisation: A and B get
  the same gain). The view always states which is active. A-weighted RMS
  is not offered.
- **K-weighting at any rate.** BS.1770 gives the coefficients at 48 kHz
  only. Both stages are bilinear transforms, with prewarping at their own
  frequency, of analogue second-order sections (the RBJ cookbook forms),
  so the analogue prototype is recovered exactly: with
  1 + a1 + a2 = 4K²/a0 and 1 − a1 + a2 = 4/a0, K² = (1 + a1 + a2)/(1 − a1 + a2),
  Q = K(1 − a1 + a2)/(2(1 − a2)), the shelf's Vh = (b0 − b1 + b2)/(1 − a1 + a2)
  and Vb = (b0 − b2)·a0·Q/(2K), and the high-pass's passband gain
  G = a0 = 1.00499 (+0.043 dB, absorbed at 48 kHz by BS.1770's −0.691).
  This gives f0 = 1681.974 Hz, Q = 0.70718, Vh = +3.99984 dB,
  Vb = Vh^0.49967 and f0 = 38.135 Hz, Q = 0.50033: the parameters
  libebur128 uses, to 12 digits. Re-deriving the 48 kHz coefficients
  reproduces the published values to 1e-14. At other rates the response
  over 20 Hz–20 kHz departs from the 48 kHz one by at most 0.0015 dB at
  44.1 kHz, 0.0058 dB at 88.2 kHz, 0.0062 dB at 96 kHz and 0.0077 dB at
  192 kHz (the rates warp the shelf differently where it is still
  rising). libebur128 keeps the high-pass numerator [1, −2, 1] at every
  rate, which lowers the passband by 0.022 dB at 96 kHz; this
  implementation keeps G.
- **Gating**: 400 ms blocks with 75 % overlap, absolute gate −70 LKFS,
  relative gate −10 LU, whole blocks only. EBU Tech 3341 test signals
  1–5 (stereo 1 kHz tones in level steps) read their derived loudness to
  0.01 LU and −23.0 ± 0.1 LUFS (cases 1 and 5 also at 44.1 and 96 kHz).
- **Meters**: momentary (400 ms) and short-term (3 s) loudness, true peak
  (4×, 3 s hold) and the limiter's gain reduction, read from analysers
  every 100 ms. They are indicative (the analyser's window is not locked
  to the audio clock); the level match does not use them.
- **True peak** (`oversample.ts`): 4× oversampling by a Kaiser-windowed
  sinc of 32 taps per phase (β = 8), each phase normalised to unit DC
  gain, designed here rather than copied from BS.1770 Annex 2. It
  reconstructs sinusoids up to 20 kHz at 48 kHz within 0.002 dB (with
  24 taps the error reached 0.13 dB at 20 kHz: the window's main lobe
  reaches into the band). Between 4× points a band-limited signal can
  still exceed the largest by up to 0.70 dB (Bernstein's inequality,
  B = fs/2). A stretch of a longer signal is measured with its
  neighbours: cut off abruptly, a signal overshoots near its ends.
- **Limiter** (`limiter.ts`): hard, stereo-linked, look-ahead 2 ms,
  release 50 ms, ceiling −1 dBTP applied to the 4× peaks less that
  0.70 dB margin. Running minimum of the required gain over the
  look-ahead, then a moving average over it: the gain on each sample
  leaving the delay line is never above what that sample requires.
  Measured by an independent 16× reconstruction, the output never
  exceeds −1 dBTP (Node and Chromium). Below the ceiling it is an exact
  delay (112 samples at 48 kHz, the same for A and B).
- **Start-up**: nothing plays until Play is pressed; the volume starts at
  −20 dB below the matched level; a warning says to start with the
  headphone volume low, and that a filter can boost some frequencies by
  more than 10 dB (the report's `large_boost` flag gives the figure).

### Programme material (`noise.ts`)

- **PRNG**: xoshiro128** (Blackman and Vigna, public domain), seeded
  through SplitMix64; normal deviates by Box–Muller. Checked against the
  reference test vector and a Python transcription.
- **Pink noise, Kellet**: white noise through Paul Kellet's "refined"
  filter, Σ g_i/(1 − p_i·z⁻¹) + d + e·z⁻¹ (music-dsp mailing list,
  17 October 1999, quoted at https://www.firstpr.com.au/dsp/pink-noise/).
  Kellet's claim of ±0.05 dB above 9.2 Hz at 44.1 kHz checks out (0.038 dB).
  For 48, 88.2 and 96 kHz the filter is refitted (`tools/audio/pink_kellet.py`):
  all 14 numbers fitted by least squares on the dB error against
  10·log10(C/f) from 9.2 Hz·fs/44.1 kHz to 0.9 of Nyquist, reweighted
  towards minimax, with the poles bounded inside the unit circle (the dB
  error sees only |H|: an unbounded fit put a pole at −1.19 at 88.2 and
  96 kHz, whose magnitude was right and whose recursion diverged; the
  test now generates every rate's whole loop). Coefficients in
  `web/src/audio/pink-coefficients.ts` (generated). Largest deviation from
  −3.01 dB/octave over 20 Hz–20 kHz: 0.033 dB at 44.1 kHz, 0.028 dB at
  48 kHz, 0.030 dB at 88.2 kHz and 0.031 dB at 96 kHz; 0.031 dB over each
  refit's whole fit band, against Kellet's 0.038 dB. The refit gains
  little from the rate itself: in normalised frequency the fit band is
  the same at every rate and 1/f has no scale, so the refits converge to
  one filter (the 48 and 88.2 kHz poles agree to 8 digits; the slowest,
  0.99888, is Kellet's 0.99886); what the rate moves is where 20 Hz
  falls in that band (at 96 kHz, on its lower edge). Generated noise (2^21 samples at
  48 kHz) has flat third-octave band levels to within 0.05 dB/octave of
  slope, each band within 0.3 dB above 200 Hz. The loop is made seamless
  by running the filter to its periodic steady state first. At a rate
  without coefficients, pink noise is white noise shaped by 1/√f in the
  frequency domain, exact and periodic, and the programme notes say
  so.
- **Pink noise, Voss–McCartney**: R rows (16 at 48 kHz) of held Gaussian
  values, row k renewed every 2^(k+1) samples by McCartney's trailing-zeros
  schedule, plus a white value every sample, laid out circularly so the
  loop is exact. Its spectrum, Σ_k (1/L_k)·[sin(πfL_k/fs)/sin(πf/fs)]² + 1
  (a sum of Fejér kernels), departs from −3.01 dB/octave by up to 1.30 dB
  over 20 Hz–20 kHz; the view reports the figure. Generated noise matches
  the derived spectrum in third-octave bands to 0.5 dB from 125 Hz and
  0.15 dB from 1 kHz.
- **White noise**, and an **exponential sine sweep** (Farina, AES 108th
  Convention, 2000) from 20 Hz to 20 kHz over the loop with 10 ms fades,
  matched by RMS.
- **Audio files** decoded by the browser at the context's rate, cut to
  their first 30 s, identified by the SHA-256 of the file.

Noise loops are 2^⌈log2(10·fs)⌉ samples (10.9 s at 48 kHz), −23 dB RMS.

### Rate

The context is requested at 48 kHz. If the device refuses, the default
rate is used and the filter is redesigned for it (the engine takes `fs_Hz`)
before anything plays; the diagnostics say so.

## The Listen view

- **Filter**: baseline (frozen baselines, the template's values, the
  bundled target, an imported measured curve), mode (difference or the
  flagged absolute diagnostic), phase (automatic, minimum, mixed, linear),
  "Design filter" with Cancel. With "Redesign when the design changes" the
  filter follows every new live result (coalesced: at most one design in
  flight, only the newest kept), and while playing each new filter is level
  matched and crossfaded in.
- **Report**: every sentence and number about the filter is the engine's
  (phase mode and why, length and the E46 check, the magnitude check,
  range, band, inversion, flags, notes). Plots: the analytic filter and the
  taps' response, and their difference with the ±0.1 dB tolerance, with
  the candidate's validity shading, crosshair readout and a data table.
- **Playback**: programme and seed, level-match method (the active one is
  stated), Play/Stop, A/B, volume (slider and numeric entry), meters, and
  the level-match table (loudness, RMS and true peak of A and B over the
  programme, their gains and matched loudness).
- **Diagnostics**: context rate, base and output latency where exposed,
  render quantum, cross-origin isolation, AudioWorklet availability and the
  path used, WebAssembly SIMD support, filter-synthesis and level-match
  times.
- **Audition state**: the engine's state plus the level match, programme
  (kind, seed or file hash, rate, length) and playback settings, as JSON to
  copy. The netlist's SHA-256 is the same the browser computes
  (`crypto.subtle`), which the tests check.

Leaving the view stops the sound (its Stop button would be out of sight).

## Interfaces

### Rust (`acoustilab::audition`)

- `Design::new(text, &overrides, overrides_json, probe)`: a compiled
  design that caches its check-grid and FFT-grid solves.
- `audition(&mut candidate, Baseline::{Design, Magnitude, None}, &Options)`
  → `Audition` (taps, latency, phase report, length check, check,
  arrays on the check grid, inversion, flags, notes, state).
- `report::{parse_design, parse_baseline, parse_options, target_baseline,
  curve_baseline, to_json}`: the JSON layer shared by the wasm export and
  the CLI. `inversion::invert`, `sha256::sha256_hex`.

### WebAssembly

`audition_filter(candidate_json, baseline_json, options_json)`:

- candidate: `{"netlist": "<text>", "overrides": {..}, "probe": "p_drp"}`;
- baseline: `{"kind": "netlist", "netlist", "overrides", "probe", "label"}`,
  `{"kind": "target", "target": "<name>" | {target object}}`,
  `{"kind": "curve", "curve": {acoustilab-curve document}, "label"}`, or
  `{"kind": "none"}` / `""` (absolute mode);
- options: `mode` (`difference` | `absolute`), `phase` (`auto` | `minimum` |
  `mixed` | `linear`), `fs_Hz` (48000; 8–384 kHz), `n` (power of two, or
  `"auto"`), `band_Hz`, `threshold_ms` (0.5), `pre_samples`,
  `ir_length_check` (true), `verify` (true), `taps` (true), `inversion`
  {`band_Hz`, `smoothing`, `boost_cap_dB`, `notch_limit_dB`}. Unknown keys
  are rejected (kind `options`).

The report (`acoustilab-audition-filter/0.1`): `mode`, `fs_Hz`, `n`,
`latency_samples`, `latency_s`, `band_Hz`, `anchor_Hz`, `anchor_gain_dB`,
`phase` {requested, used, reason, decision, polarity, delay_removed_s,
hybrid_from_Hz}, `ir_length` {fitted, poles, binding, needed_s,
recommended_n, n, half_length_s, covered, rhp_zeros_in_decision_band_Hz,
fit_max_dB, notes}, `ir_length_error`, `check` {band_Hz, tolerance_dB,
max_abs_error_dB, at_Hz, rms_error_dB, met, exceeded_Hz, resolution_Hz},
`tail_energy_dB`, `pre_energy_dB`, `max_boost_dB`, `max_cut_dB`,
`frequencies_Hz`, `design_dB`, `fir_dB`, `error_dB`, `candidate`,
`baseline`, `inversion` (or null) {band_Hz, smoothing, envelope_smoothing,
boost_cap_dB, notch_limit_dB, beta, reference_dB, method, notches,
max_boost_dB, max_departure_from_exact_dB, frequencies_Hz, inverse_dB,
exact_inverse_dB, smoothed_baseline_dB}, `flags` [{code, message}],
`notes`, `state` {schema `acoustilab-audition/0.1`, engine, candidate
{netlist_sha256, overrides, probe}, baseline {kind, netlist_sha256 |
name, sha256, ...}, mode, phase {requested, used}, fs_Hz, n, band_Hz},
and `taps` (f32 values). Errors: `{"error", "kind"}` with kind `options`
or `baseline`, or the engine's kinds with `"design": "candidate"` or
`"baseline"`.

The worker keeps the last four prepared designs (keyed by the SHA-256 of
the netlist, the overrides and the probe), so a filter that follows a
slider drag re-solves only the candidate; results are identical with and
without the cache (tested). Cost (template with 2 vents against the
template, N = 8192): natively about 0.65 s with both designs fresh; in
headless Chromium in the development container 4.1 s for the first design
(including the view worker's start), 1.5 s when the candidate changed,
0.5–0.7 s with both designs cached; the level match over the 10.9 s
programme takes 1.5–1.6 s on its worker. The diagnostics panel shows the
figures of the running browser.

### CLI

```sh
acoustilab audition design.json --set vent_count=3 [--baseline-set NAME=VALUE]... \
    [--baseline other.json] [--target NAME|FILE.csv] [--curve FILE] [--absolute] \
    [--phase auto|minimum|mixed|linear] [--fs 48000] [--n N] [--probe ID] [--wav FILE] [--taps] [--out FILE]
```

Without a baseline option the baseline is the same netlist without the
candidate's `--set` values: the template it was edited from. `--wav` writes
the taps as 32-bit float WAV (0 dB = the filter's 500 Hz–2 kHz mean).

## Known gaps

- Nothing has been listened to (above). The 0.5 ms phase threshold, the
  12 dB cap's reference and the hold rules are engineering choices.
- Two ears: the worklet takes independent left and right filters, but the
  engine designs one filter per call from one probe; per-ear parameter
  vectors, Monte Carlo draws to both ears and the interaural readouts of
  Section 16 are not implemented.
- The convolution runs in JavaScript, not a WebAssembly SIMD FFT; A-weighted
  RMS matching, streaming decoding of long files, and the ABX / multi-
  stimulus listening-test module are not implemented.
- The meters are indicative (polled analysers), and cross-origin isolation
  (for a SharedArrayBuffer meter path) is reported, not used.
- The ConvolverNode fallback has no true-peak limiter.
