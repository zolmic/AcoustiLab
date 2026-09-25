# Netlist reference

A netlist is one JSON document. Every dimensional key carries its unit as a
suffix: `_mm`, `_cm3`, `_g`, `_Tm`, and so on. See `crates/acoustilab/src/units.rs`
for the accepted suffixes of each dimension. The engine rejects unknown keys,
keys without a unit suffix, and duplicate spellings of the same quantity.

```json
{
  "schema": "acoustilab-netlist/0.1",
  "air": {"preset": "standard_23C"},
  "sweep": {"f_min_Hz": 10, "f_max_Hz": 40000, "points_per_octave": 48},
  "level": 1,
  "nodes": [{"id": "a_front", "domain": "acoustic"}],
  "elements": [{"id": "front", "type": "cavity", "node": "a_front", "volume_cm3": 30}],
  "probes": [{"id": "p", "quantity": "pressure", "node": "a_front"}]
}
```

## Top level

| Key | Meaning |
|---|---|
| `air` | `{"preset": "standard_23C" \| "spec_reference"}` or `{"T_C": .., "P_kPa": .., "RH": ..}`. Default: standard_23C |
| `sweep` | `{"f_min_Hz", "f_max_Hz", "points_per_octave"}` or `{"frequencies_Hz": [..]}`. Default: 10 Hz–40 kHz at 48 points per octave |
| `level` | `0` lumped, `1` distributed (default) |
| `nodes` | `{"id", "domain": "electrical" \| "mechanical" \| "acoustic"}` |
| `elements` | `{"id", "type", "nodes": [..]` or `"node": ".."`, then parameters `}` |
| `probes` | `{"id", "quantity", "node" \| "element", "port"}` |

Ground names: `gnd` is valid in any domain. Domain-specific aliases are
`e_gnd`, `m_gnd` or `frame`, and `a_amb`, `a_gnd` or `ambient`. Terminals that
are omitted default to ground.

## Elements

Parameters in *italics* are optional.

### Electrical
| type | nodes | parameters |
|---|---|---|
| `resistor` | [a, *b*] | `R_ohm` |
| `inductor` | [a, *b*] | `L_H` |
| `capacitor` | [a, *b*] | `C_F` |
| `coil` | [a, *b*] | `Re_ohm`, *`Le_H`*, *`L2_H` + `R2_ohm`* (LR-2 eddy-current branch) |
| `vsource` | [+, *−*] | *`V_V`* (RMS, default 1), *`Zs_ohm`* (source impedance) |
| `isource` | [into, *from*] | *`I_A`* |

### Mechanical (velocity across, force through)
| type | nodes | parameters |
|---|---|---|
| `mass` | [n] (to frame) | `M_kg` |
| `spring` | [a, *b*] | `C_m_per_N` or `K_N_per_m` |
| `damper` | [a, *b*] | `R_Ns_per_m` |
| `suspension` | [n] (to frame) | `Mms_g`, `Cms_mm_per_N` or `Kms_N_per_m`, *`Rms_Ns_per_m`* |
| `force_source` | [into, *from*] | *`F_N`* |

### Couplers
| type | nodes | parameters |
|---|---|---|
| `motor` | [e+, e−, m+, m−] | `Bl_Tm`. Transformer: e = Bl·v, F = Bl·i |
| `piston` | [m+, m−, a_front, a_rear] | `Sd_cm2`. Gyrator: U = Sd·v, F = Sd·Δp |

### Acoustic
| type | nodes | parameters |
|---|---|---|
| `cavity` | [face] or [driver face, far face] | one geometry: `lx/ly/lz_mm` (box, depth = lz), `radius_mm` + `depth_mm` (cylinder), or `volume_cm3` [+ `depth_mm`]. Optional: *`wall_loss`* (true), *`surface_factor`* (1–10), *`wall_area_cm2`*, *`max_distance_mm`* (validity length) |
| `tube` | [a, *b*] | `radius_mm` or `diameter_mm`, `length_mm`, *`count`*, *`inlet`*, *`outlet`*: `none` \| `flanged` (0.8216a) \| `piston` (0.8488a) \| `unflanged` (0.6133a) |
| `slit` | [a, *b*] | `gap_mm`, `width_mm`, `length_mm`, *`count`* |
| `radiation` | [a, *b*] | `radius_mm` or `area_cm2`, *`baffle`*: `infinite` \| `free`, *`count`* |
| `acoustic_resistance` / `_inertance` / `_impedance` / `_compliance` | [a, *b*] | `R_Pa_s_per_m3`, `M_kg_per_m4`, `C_m3_per_Pa` (user overrides) |
| `flow_source` | [into, *from*] | *`U_m3_per_s`* |
| `pressure_source` | [+, *−*] | *`p_Pa`*, *`Zs_Pa_s_per_m3`* |

## Probes

| quantity | target | unit |
|---|---|---|
| `pressure`, `voltage`, `velocity`, `potential` | `node` | Pa, V, m/s |
| `displacement`, `acceleration` | mechanical `node` | m, m/s² |
| `flow`, `current`, `force`, `volume_velocity` | `element`, *`port`* | through quantity at the port |
| `port_potential` | `element`, *`port`* | across quantity at the port |
| `impedance` | `element`, *`port`* | port potential / port flow (e.g. electrical input impedance at a `vsource`) |

Element port flows are positive *entering* the element at the port's first
terminal. For a duct, port 0 is the through-flow from its first node towards
its second, and port 1 is the negative of that at the far end. Independent
sources are the exception: they report the flow they deliver, so an
`impedance` probe on a source reads the load it sees.

Results report every probe as complex RMS values, plus magnitude and phase.
Acoustic pressures also report dB SPL re 20 µPa.

## Transducers

### `driver` (moving-coil macro, spec Section 5)

Nodes `[e+, e−, a_front, a_rear]`; `a_rear` may be omitted (ambient), and any
terminal may be a ground name (`"ambient"` for a free-air face). The macro
creates internal nodes that probes can read:

| node | domain | meaning |
|---|---|---|
| `<id>.e` | electrical | between the coil impedance and the motor |
| `<id>.m` | mechanical | dome and coil velocity |
| `<id>.m2` | mechanical | surround velocity; joined to `<id>.m` by an ideal short below D2 |

Key `model` selects the level (default `"D1"`). Every key is accepted at every
level, so switching level never changes the netlist; a level uses only what
it needs.

| model | adds | keys it uses |
|---|---|---|
| `D0` | Re, motor Bl (transformer), Mms/Cms/Rms to the frame, piston Sd (gyrator) | the D0 set below |
| `D1` | coil inductance: Re + jωLe + (R2 ∥ jωL2) | *`Le_H`*, *`L2_H` + `R2_ohm`* (LR-2 eddy branch) |
| `D2` | two-degree-of-freedom dome and surround | `Msur_g`, `Ssur_cm2`, `Kbend_N_per_m` or `Cbend_mm_per_N`, *`Rbend_Ns_per_m`* |

D0 set, one of:

* the **primary set** `fs_Hz`, `Qms`, `Qes`, `Re_ohm`, `Mms_g`, `Sd_cm2`, from which
  Cms = 1/(ωs²·Mms), Rms = ωs·Mms/Qms and Bl = sqrt(ωs·Mms·Re/Qes) are derived
  (erratum E5); `Bl`, `Cms`, `Kms` and `Rms` are then rejected;
* the **physical set** `Re_ohm`, `Bl_Tm`, `Mms_g`, `Cms_mm_per_N` or `Kms_N_per_m`,
  *`Rms_Ns_per_m`* (default 0), `Sd_cm2`;
* `"record": "<name>"`, which supplies the primary set and the record's
  `model` keys. A key given both inline and in the record is an error.

Creep, at every level (erratum E18): *`creep_lambda`* (λ ≥ 0) and
*`creep_f0_Hz`* (default fs) give the Knudsen–Jensen compliance
C(jω) = C0·[1 − λ·log10(jω/ω0)] (complex form per Klippel AN49). The
compliance is causal, and the spring is passive on the jω axis, for λ ≥ 0.
λ must keep Re C > 0 up to 40 kHz, i.e. λ < 1/log10(40 kHz/f0) (0.37 for
f0 = 81.8 Hz). Where Re C crosses zero, f0·10^(1/λ), the stiffness has a
real right-half-plane pole, so a driver with creep is causal only to about
1e-7 of |Z| in band at the largest allowed λ (1e-13 for λ up to half of it).
That is negligible for frequency-domain solves but rules the law out for
time-domain use. In D2 it scales both springs.

D2 derives the split from the D0 set so that the free-air (motor-driven)
behaviour is unchanged at low frequency: C_outer = Cms − Cbend,
r = C_outer/Cms, dome mass Mms − r²·Msur, dome area Sd − r·Ssur, dome damping
Rms − (1 − r)²·Rbend (each must stay positive; checked at every level when
the surround keys are given). The effective area U/v_dome = S1 + S2·v2/v1 is
complex and dips where the surround moves in antiphase. Under acoustic load the surround adds a pressure compliance
r·(1 − r)·Ssur²·Cms that free-air data cannot see. The surround keys have no
datasheet source; mark them as estimates.

Ports: 0 electrical (e+, e−), current into e+ (an `impedance` probe on port 0
is the driver's input impedance); 1 acoustic (a_front, a_rear), volume
velocity entering the front (minus the diaphragm output); 2 and 3 mechanical
(`<id>.m`, `<id>.m2` to the frame), the force applied by elements outside the
macro (zero unless, for example, an added test mass hangs on `<id>.m`).
Validity: the rigid-piston range ends at ka = 1 (3.06 kHz for 10 cm², erratum
E28).

```json
{"id": "drv", "type": "driver", "nodes": ["e_in", "gnd", "a_front", "a_rear"],
 "record": "tymphany_hpd_40n16pet00_32", "model": "D1", "creep_lambda": 0.05}
```

### Driver records (`data/drivers/*.json`)

One file per driver, embedded in the engine at build time. Schema
`acoustilab-driver/0.1`:

| key | content |
|---|---|
| `name`, `title` | record name used by `"record"`, description |
| `provenance` | `origin` (`datasheet` \| `measured` \| `user`), `source`, *`url`*, `date`, *`retrieved`*, `licence`, `condition` (free air, coupler, vacuum), `air_load` |
| `primary` | `fs_Hz`, `Qms`, `Qes`, `Re_ohm`, `Mms_g`, `Sd_cm2`: the only values the network uses |
| `datasheet` | values kept for the consistency report, exactly as printed: *`Bl_Tm`*, *`Cms_*`*, *`Vas_L`*, *`Le_*`*, *`Qts`*, *`Zmin_ohm`*, *`Xmax_mm`*, *`rated_impedance_ohm`*, *`rated_power_mW`*, *`sensitivity`*: [{`level_dB`, `drive_V` \| `drive_W`, *`distance_m`*, *`f_Hz`*, *`condition`*}] |
| `tolerances` | stated relative tolerances by parameter |
| `model` | element keys beyond the primary set (D1, creep, D2) |
| `estimated` | keys of `model` that are estimates |
| `notes` | free text |

The governance report (`elements::driver::governance`) checks, at 5 %: the
resonance identity (fs against Mms and the printed Cms), electrical Q (Qes
against Bl, Re, Mms, fs), Vas against ρc²·Sd²·Cms, and total Q. It warns when
Qms/Qes < 0.1 (hump too small to identify), checks the minimum impedance
against 80 % of the rated impedance, and reports the impedance implied by a
pair of voltage and power sensitivities. A unit anomaly is flagged when a
decimal factor on one field (µ/m/k for masses, compliances and volumes; cm²/mm²
for areas) repairs a failing identity without worsening any other. A record
with an anomaly in a primary field is refused by the element. The Tymphany
record fails the electrical-Q identity (0.864 against 1.01, E5) and flags
`Cms_um_per_N` as `Cms_mm_per_N`.

Thiele–Small conversions, the sqrt(R0) extraction and the Levenberg–Marquardt
impedance fit (creep and LR-2) are in `acoustilab::ts`. Drive conventions
(characteristic voltage, 1 mW into the rated impedance, 1 V, constant
current) and sensitivity readouts in dB/V and dB/mW, converted with the rated
impedance (E32), are in `acoustilab::drive`; they scale a solve and do not
use netlist keys yet.
