# Measurement session: open reference cup

The script of one session on the open reference cup (spec Section 17,
"Validation under limited fixture access"). `protocol.json` is the same
protocol in the form the tool reads; the blind predictions in
`predictions/v1/` were computed from it and from `examples/reference_cup.json`
before any cup existed. Fixture time is scarce, so the tool lists what the
session must produce, and the session is checked while the fixture is still
booked:

```sh
acoustilab validate --verify                       # the frozen predictions are intact
acoustilab validate --session session-2026-10-01   # sidecar templates + SESSION.txt checklist
# ... measure, save each export next to its template ...
acoustilab validate session-2026-10-01 --out report.json   # at any time: missing files, residuals, acceptance
```

Run the commands from the repository root (they default to
`validation/reference_cup/protocol.json` and the newest prediction version).
Never edit the frozen predictions or change the protocol during a session:
a new model or a new protocol is a new prediction version, compared with the
same measurements, and the v1 comparison stays the blind record.

## Equipment

* Analyser with a logarithmic sweep and two channels (microphone, and the
  current through the driver for impedance), exporting FRD, ZMA, REW text or
  CSV. A 2 s sweep at 48 kHz with an FFT of at least 65 536 points gives
  more than 24 points per octave at 20 Hz.
* Amplifier whose output impedance is below 1 ohm; measure it and state it
  in every sidecar (`source_impedance_ohm`).
* IEC 60318-4 ear simulator mounted flush in a flat plate at least 100 mm
  across (for example a headphone test fixture's flat plate).
* A damped IEC 60318-4 variant in the same plate (for example GRAS
  RA0401/RA0402; the manufacturer states compliance with IEC 60318-4 from
  100 Hz to 10 kHz with the 13.5 kHz resonance damped).
* A head and torso simulator with ITU-T P.57 Type 4.3 ears.
* Sound calibrator for the microphones, thermometer (the model is at 23 °C),
  kitchen scale (clamping force), the plug set, pin gauges.

## Before the session

1. Build check and leak check (`README.md`): every dimension inside its
   limit, and with all plugs sealed on the flat plate the drum response flat
   within 0.5 dB from 20 to 100 Hz.
2. Measure the amplifier's output impedance and the room temperature.
3. Calibrate the microphones. Pressure exports are calibrated dB SPL re
   20 uPa at the stated drive (`"calibrated": true`).
4. Set the drive: **17.9 mV RMS open-circuit at the amplifier output**, which
   is 10 uW into the 32 ohm rating (`{"power_mW": 0.01, "rated_ohm": 32}`).
   It keeps every meshed and sealed state far below the 1 m/s particle
   velocity at which a hole's flow resistance starts to grow; at 1 mW the
   open rear hole would reach 1.5 m/s. The linearity check `iec_rhole_hi` is
   at 56.6 mV (100 uW).
5. Create the session directory with `acoustilab validate --session DIR`
   and fill in each template before or right after its measurement: `date`,
   `device` (cup serial and side), `temperature_C`, `source_impedance_ohm`
   and the analyser in `provenance.tool`. Add the uncertainty budget you
   know (`uncertainty`: microphone calibration, coupler tolerance; see
   docs/fitting.md): the comparison combines it with the spread between
   seatings, which the tool measures itself.

## Files

One export per seating: `<measurement>_s<k>.<frd|zma|txt|csv>` with k from
1, next to `<measurement>_s<k>.sidecar.json` (the template; a sidecar named
`<export>.sidecar.json` is read too). Export without smoothing and without
compensation. Pressure and impedance of the same seating come from the same
sweep. The tool reads nothing else; other files in the directory are listed
as not part of the protocol.

## Measurements, in order

| step | configuration | plugs (rear / front) | fixture | seatings | files |
|---|---|---|---|---|---|
| 1 | `driver_free`: bare driver, before assembly | - | free air | 3 | `driver_free_z` |
| 2 | `cup_free_sealed` | sealed / mesh | free air | 3 | `cup_free_sealed_z` |
| 3 | `cup_free_hole` | hole / mesh | free air | 3 | `cup_free_hole_z` |
| 4 | `cup_free_mesh` | mesh / mesh | free air | 3 | `cup_free_mesh_z` |
| 5 | `iec_ref` (reference state) | mesh / mesh | IEC 60318-4, flat plate | 5 | `iec_ref_p`, `iec_ref_z` |
| 6 | `iec_rsealed` | sealed / mesh | same seating as step 5 | 5 | `iec_rsealed_p`, `iec_rsealed_z` |
| 7 | `iec_rhole` | hole / mesh | same seating | 5 | `iec_rhole_p`, `iec_rhole_z` |
| 8 | `iec_rhole_hi`: step 7 at 56.6 mV | hole / mesh | same seating | 5 | `iec_rhole_hi_p` |
| 9 | `iec_fsealed` | mesh / sealed | same seating | 5 | `iec_fsealed_p`, `iec_fsealed_z` |
| 10 | `iec_sealed` (pressure chamber) | sealed / sealed | same seating | 5 | `iec_sealed_p`, `iec_sealed_z` |
| 11 | `iecd_ref` (reference state) | mesh / mesh | damped IEC 60318-4, flat plate | 5 | `iecd_ref_p`, `iecd_ref_z` |
| 12 | `t43_ref` (reference state) | mesh / mesh | head and torso simulator, Type 4.3 | 5 | `t43_ref_p`, `t43_ref_z` |

`p` files are the drum-point pressure (reference point `drp`), `z` files the
electrical input impedance at the driver's terminals (`terminals`). Steps 5
to 10 are one loop per seating: seat the cup, take steps 5 to 10 by swapping
plugs without lifting it, restore the reference plugs, lift, and seat again.
Swapping plugs inside a seating means that the six states share their leak
and front volume, so their differences are the plugs' alone.

**Free air (steps 1 to 4).** Hang the driver, then the assembled cup with
its gasket up, at least 30 cm from any surface; re-hang or handle it between
repeats. The bare driver's impedance anchors the unit's own Thiele-Small
parameters; the cup in free air isolates the rear chamber and its plugs from
every fixture and from the leak.

**Seating on the flat plate (steps 5 to 11).** Plate horizontal, the cup
centred on the ear simulator's entrance, 5.0 N in total: weigh the assembled
cup and put 510 g minus its mass on the rear shell. To reseat, lift the cup
clear, wait five seconds, centre it again and put the weight back.

**Head and torso simulator (step 12).** Centre the cup on the ear canal
entrance as far as the pinna allows, with 5 N of clamping force (a headband
or a calibrated spring). The model sees the pinna only as 10 cm3 taken from
the front cavity (an estimate) and a larger residual leak, so this is the
comparison to expect least from above 1 kHz.

## What the report shows

`acoustilab validate DIR` prints (and `--out` writes as JSON,
`acoustilab-validation-report/0.1`) three sections, kept apart:

1. **Measurements and sidecars.** Each measurement is `missing`,
   `incomplete` (fewer seatings than required) or `complete`; the spread
   between seatings; every sidecar that breaks the protocol (wrong fixture,
   ear simulator, reference point or drive; compensation; smoothing coarser
   than 1/24 octave; an uncalibrated pressure; a source impedance above
   1 ohm or unstated: these exclude the file) and every warning (missing
   date, device or temperature, a temperature more than 3 °C from 23 °C).
2. **Against the frozen blind predictions**, without any fitting: per band
   (below 20 Hz, 20 Hz to 1 kHz, 1 to 4 kHz, above 4 kHz) the RMS and largest
   residual, the share of points within two combined standard uncertainties
   (the Monte Carlo spread of the prediction and the measurement's own
   uncertainty) and the share inside the 5-95 % Monte Carlo envelope, and
   the largest residual where the model is not shaded.
3. **Acceptance** (spec Section 17): the residual leak and the front volume
   are fitted to each pressure curve of the fixture states, and the residual
   must stay within 2 dB from 20 Hz to 1 kHz and within 4 dB from 1 to
   4 kHz; above 4 kHz it is reported without a bound. The verdict is taken on
   the three reference states `iec_ref_p`, `iecd_ref_p` and `t43_ref_p`; the
   other states are reported the same way. A second run, labelled
   `driver_anchored`, first fits the driver to its own free-air impedance
   (step 1): it separates a driver unit that differs from its datasheet from
   an error of the model, and is not the spec's criterion.

## If something fails during the session

* **Missing or incomplete:** the list names the files; measure them while
  the fixture is booked.
* **Sidecar errors:** fix the sidecar if it was only filled in wrongly;
  re-measure if the condition was wrong (drive, smoothing, fixture).
* **Large spread between seatings** (more than about 1 dB below 1 kHz):
  check the gasket and the seating; a leak varies from seating to seating.
* **Acceptance fails below 1 kHz on every fixture state:** compare with
  the free-air cup impedance (steps 2 to 4) before blaming the fixture: the
  driver's internal rear holes act between the diaphragm and the rear
  chamber, and they show first there (docs/reference-cup.md).
