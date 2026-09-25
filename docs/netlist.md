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

## Radiation loads and duct cross-sections

### `radiation`

| `baffle` | model | low-ka limit | validity |
|---|---|---|---|
| `infinite` (default) | Rigid piston in an infinite baffle: Z = ρc/S·[1 − 2J1(2ka)/(2ka) + j·2H1(2ka)/(2ka)], exact at all ka | R = (ka)²/2·ρc/S, end correction 8a/(3π) = 0.8488a | no limit |
| `free` | Open end of an unflanged, thin-walled circular pipe: the Levine & Schwinger (1948) Wiener–Hopf solution for the plane mode, evaluated numerically. Z = ρc/S·tanh(A/2 + j·ka·L/a), where \|R\| = e^{−A} and L is the end correction | R = (ka)²/4·ρc/S, L = 0.6127a | exact below ka = j₁,₁ = 3.8317. Shading begins at 0.9·j₁,₁ and deepens at j₁,₁ |

Notes on `free`:

- Levine & Schwinger printed the low-frequency end correction as 0.6133a, but
  their integral evaluates to 0.61270a (`tools/refgen/radiation_refs.py`,
  mpmath). The value 0.6127 is also reported in AIP Conf. Proc. 2195, 020034
  (2019). The tube's `outlet: "unflanged"` end correction still uses 0.6133a.
- Above ka = 3.8 the element continues with the fitted formulae of Silva et al.,
  J. Sound Vib. 322, 255–263 (2009), Eqs. (21)–(22), scaled to be continuous
  at 3.8. They keep it passive and smooth; they are not a model of the pipe
  there, since higher modes propagate.
- Up to ka = 3 the numerical solution agrees with the Silva et al. fits to
  within 0.9 % in |R| and 2.9 % in L. The paper claims under 2 %.

Accuracy of the numerics, checked against mpmath (`tools/refgen/`,
`crates/acoustilab/tests/special_functions.rs`):

| quantity | agreement |
|---|---|
| R1, X1 of the baffled piston, 0 ≤ 2ka ≤ 500 | ≤ 5e-15 relative |
| −ln\|R\| and L/a of the unflanged pipe, 0 ≤ ka ≤ 3.8 | ≤ 1e-14 relative |

### Duct cross-sections (`thermoviscous::Section`)

`tube` uses `Circle` and `slit` uses `Slit`. Two-node cavities use
`Equivalent`. The engine API also offers `Rect { a, b }`, a rectangular duct
with both sides finite (full side lengths). It uses Stinson's (1991)
double-series shape function, summed in closed form over one index. For
Re(k·min(a, b)) > 42 it uses its exact boundary-layer form
F = P/(kA) − 16/(πk²A), whose remainder is of order e^{−k·min(a,b)}. `Rect`
has no netlist element type yet.

| section | shape length (shear wavenumber) | Poiseuille limit (L0 oracle) |
|---|---|---|
| `Circle { radius }` | radius a | R = 8μl/(πa⁴), M = (4/3)·ρl/S |
| `Slit { gap, width }` | gap/2 | R = 12μl/(w h³), M = (6/5)·ρl/S |
| `Rect { a, b }` | min(a, b)/2 | R = 12μl/(w h³)/[1 − (192h/(π⁵w))·Σ_{n odd} tanh(nπw/2h)/n⁵] (h ≤ w). A square gives R = 28.45μl/a⁴ and M = 1.378·ρl/S |
| `Equivalent { area, perimeter }` | 2A/P | circle at radius 2A/P; correct only when the boundary layer is thin |

For a `Rect`, the first transverse mode is set by the longer side, as for a
slit's width. The Stinson bound uses half the shorter side.

The duct model is checked against an independent mpmath implementation for
circles, slits and rectangles. The shear wavenumbers are 0.1 to 1000.
ρ_eff, K_eff, Γ, Z_c and the ABCD matrix agree to better than 1e-13
(`tests/thermoviscous.rs`).

## Materials, vents and leaks

Elements from `elements/materials.rs` and the additions to `elements/ducts.rs`
(spec Sections 6, 8 and 13, App. C3–C6). All are passive; every one exposes
ports that balance in `Circuit::power_absorbed`.

### Ducts: additions
| type | nodes | parameters |
|---|---|---|
| `tube` | [a, *b*] | as before, plus *`end_resistance`*: `none` (default) \| `maa` (R_s/2 per end) \| `ingard` (R_s per end), applied to each end whose `inlet`/`outlet` correction is not `none`. R_s = ½·sqrt(2μρω) is Ingard's surface resistance, referred to the hole area |
| `area_step` | [a, *b*] | `radius1_mm` \| `diameter1_mm` \| `area1_mm2`, and the same with `2`. Series inertance ρ·δ(α)/(πa²) of a coaxial step, a the smaller radius, α = a/b; δ from the exact static solution (0.8216a as α → 0), see `ducts::step_end_correction` |
| `vent` | [inner, *outer*] | `radius_mm` or `diameter_mm`, `length_mm` (wall thickness), *`count`*, *`inlet`* (`flanged` default, `piston`, `unflanged`, `none`), *`baffle`* (`infinite` default \| `free`) for the outer radiation load, *`end_resistance`* (`maa` default), *`mesh`*: a material id or an object of `mesh` keys (area defaults to the total hole area). Internal nodes `<id>.mouth` and `<id>.mesh` |
| `leak` | [inside, *outside*] | `perimeter_mm`, `depth_mm` (pad face width), `gap_mm` (uniform, with *`segments`*, default 8) or `gaps_mm: [..]` (one per segment; 0 = sealed), *`ends`*: `flanged` (default; baffled rectangular-piston correction per end) \| `none`. Each segment is a thermoviscous slit of breadth perimeter/N and needs breadth ≥ 5 × gap |

`vent` is a tube with the inner end correction, the optional mesh and the
exact baffled-piston radiation impedance (whose reactance is the outer end
correction) in series. Probe port 0 is the flow entering at the inner node,
port 1 the flow entering at the outer node. `leak` exposes the same two ports.
The vent's mesh is a `materials::Mesh` part (`<id>.mesh`) of the composite,
so its pore velocity can be read like that of a stand-alone mesh.

### Materials
| type | nodes | parameters |
|---|---|---|
| `mesh` | [a, *b*] | `R_s_rayl` (specific flow resistance, MKS rayl), `area_cm2` \| `radius_mm` \| `diameter_mm`, and two of *`thickness_um`*, *`open_area`* (fraction), *`pore_diameter_um`* (the third follows from R_s = 8μt/(φa²)); none of them gives a pure resistance R_s/A. *`end_resistance`* (`maa` default), *`material`* (database id) |
| `perforated_plate` | [a, *b*] | `hole_diameter_mm` \| `hole_radius_mm`, `thickness_mm`, `porosity` (open-area ratio) or `holes` (count), area as for `mesh`, *`end_resistance`* (`maa` default), *`interaction`* (true: Fok-reduced end correction) |
| `porous_layer` | [a, *b*] or [a] with `backing: "rigid"` | `thickness_mm`, area as for `mesh`, `sigma_Pa_s_per_m2` \| `sigma_kPa_s_per_m2`; for JCA also `porosity`, `tortuosity`, `viscous_length_um`, `thermal_length_um`; for JCAL also `thermal_permeability_m2`. *`model`*: `jca` \| `jcal` \| `delany_bazley` \| `miki` (default: JCA/JCAL when their parameters are given, else Miki). *`material`* |
| `fill` | [cavity node] | `volume_cm3` (the cavity's volume), *`fraction`* (0–1, default 1), and one of `tau_ms`, `f_relax_Hz`, or `fibre_diameter_um` + `bulk_density_kg_per_m3` + `fibre_density_kg_per_m3` |
| `membrane_vent` | [a, *b*] | `R_s_rayl`, area as for `mesh`, membrane mass as `thickness_um` + `density_kg_per_m3` or `mass_mg`, *`profile`* (`membrane` 4/3 default \| `plate` 9/5 \| `piston` 1: acoustic-mass factor of the deflection shape), compliance as `C_m3_per_Pa` \| `resonance_Hz` \| `tension_N_per_m` (membrane profile: C = πa⁴/8T), and the membrane-branch loss as `R_m_Pa_s_per_m3` or `Q_m` (required: datasheets do not give it) |

Models, in short (the doc comments give formulas and sources):

- `mesh`: N = φA/(πa²) equivalent cylindrical pores, each a thermoviscous
  tube of length t with the piston end correction 8a/3π per side reduced by
  Fok's function ψ(√φ), plus the resistive end correction. Exactly R_s/A at
  DC. The Ingard–Ising nonlinear resistance is *not* applied; call
  `MeshModel::pore_velocity` (or `Mesh::pore_velocity` on a solution) and
  compare with `PORE_VELOCITY_WARNING` (1 m/s). `MeshModel::badge` returns
  the resistive-flat badge (resistance change < 1 dB over 20 Hz–20 kHz).
- `perforated_plate`: the same hole model (Crandall's exact circular-tube
  impedance). `materials::maa_impedance` is Maa's (1998) approximation for
  comparison; `PerforateModel::perforate_constant` gives k = d·sqrt(ωρ/4μ)
  and `regime` the spec's resistive / transitional / mass badge.
- `porous_layer`: a slab two-port with ρ_eq, K_eq (L1), the lumped series
  impedance jωρ_eq·t/A (L0; DC limit σt/A), or Z_c·coth(Γt)/A with a rigid
  backing. Delany–Bazley and Miki use the variable f/σ (σ in Pa·s/m², no
  air density) and are fitted for 0.01 ≤ f/σ ≤ 1: the upper end is a
  validity limit, the lower end is reported by `PorousModel::window` (limits
  can only express upper bounds). Neither power law is physically
  admissible at low f/σ: their Im K_eq turns negative below f/σ ≈ 0.0106
  (Delany–Bazley) and ≈ 0.00105 (Miki), e.g. below about 50 Hz for a
  50 kPa·s/m² pad foam with Miki. The engine clips Im K_eq at zero there,
  so every porous layer stays passive.
- `fill`: a shunt admittance jω·ΔC at the cavity node with
  ΔC = V/(γP0)·(γ−1)·φ/(1 + jωτ) (App. C6). τ from fibres uses Tarnow's
  isothermal-fibre cell and the matching first moment (Lafarge's thermal
  permeability). Under a prescribed volume velocity the low-frequency SPL
  falls by 2.92 dB at full fill and 0.83 dB at 25 %; at constant voltage the
  drop is smaller (erratum E11). The fibres' own volume is not subtracted.
- `membrane_vent`: Z = R ∥ (R_m + jωM + 1/(jωC)): the pores and the
  membrane moving as a whole share the pressure drop.

### Material database

`data/materials/*.json` holds entries with value, method, state, tolerance,
status (`measured`, `datasheet`, `estimated`, `unverified`), source and
licence; they are embedded at compile time. `material: "<id>"` on `mesh`
(kinds `mesh`, `fabric`) and `porous_layer` (kind `porous`) supplies the
entry's parameters; keys given on the element override them. The current
entries are quoted from the specification and marked `unverified` or
`estimated` until the primary sources are checked.

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

## Ear loads

Canal, eardrum and ear-simulator elements (spec Section 7). Derivations,
sources, fits and validation are in `docs/ear-loads.md`.

| type | nodes | parameters |
|---|---|---|
| `canal` | [entrance, drum] | Profile, exactly one form: `positions_mm` + `areas_mm2` (arrays; positions run from the first node to the second, strictly monotonic, radius linear between knots); or `length_mm` + `area_entrance_mm2` + `area_end_mm2` [+ *`profile`*: `conical` (default) \| `exponential`]; or `length_mm` + `area_mm2` (mid-canal) [+ *`taper`*: end/entrance area ratio, default 1]. Optional: *`segments`* (40), *`wall_loss`* (true), *`entrance_offset_mm`* (trims the canal from the entrance side). One two-port whatever the segment count |
| `eardrum` | [drum, *back*] | *`model`*: `hudde_engel` (default) \| `type43` \| `iec60318_4` \| `rigid`; *`R_scale`*, *`M_scale`*, *`C_scale`* (default 1); *`middle_ear_volume_cm3`* (tympanic cavity; `hudde_engel` and `type43` only) |
| `iec60318_4` | [entrance] | *`microphone`*: `bk4192` (default) \| `rigid`; *`side_volume_scale`* (scale on Luan et al.'s side-cavity volumes; default: the fitted 1.281). Literature model, not verified against the IEC table |
| `type33` | [entrance] | As `iec60318_4`, plus *`extension_length_mm`* (10.0) and *`extension_diameter_mm`* (7.5) |
| `type43` | [entrance] | *`input`*: `eep` (default) \| `ref` (drive the reference plane; the canal lateral to it is omitted); *`segments`* (48); *`wall_loss`* (true); *`drum`*: `type43` (default) \| `hudde_engel` \| `iec60318_4` \| `rigid`, with the `eardrum` scale keys |

The macros join their terminal to the internal node `<id>.eep` with an ideal
short and expose `<id>.drp`, the drum reference point, which is the
microphone plane. `type33` and `type43` also expose the reference plane
`<id>.ref`; `type43` with `input: "ref"` has no `<id>.eep`. Probe these nodes
by name, e.g. `{"id": "drp", "quantity": "pressure", "node": "ear.drp"}`. An
`impedance` probe on the macro (port 0) reads the ear's input impedance at
its terminal. Other elements may also connect to the internal nodes, e.g. a
leak or a probe-microphone volume.

## Modal cavity (`modal_cavity`)

The L1 closed-form modal expansion of a box or cylinder (spec Section 9):
an N-port whose ports are footprints on the walls, such as the driver, the
canal entrance, a leak or a vent. Implementation:
`crates/acoustilab/src/elements/cavity.rs` and `crates/acoustilab/src/modes.rs`.

```json
{"id": "front", "type": "modal_cavity", "lx_mm": 60, "ly_mm": 45, "lz_mm": 20,
 "f_max_Hz": 20000,
 "ports": [
   {"node": "a_drv",  "face": "z0", "x_mm": 0,  "y_mm": 0,   "radius_mm": 17.8},
   {"node": "a_ear",  "face": "z1", "x_mm": 12, "y_mm": -4,  "radius_mm": 4},
   {"node": "a_leak", "face": "y1", "x_mm": 0,  "z_mm": 19,  "lx_mm": 20, "lz_mm": 0.4},
   {"node": "a_vent", "face": "x0", "y_mm": 5,  "z_mm": 10,  "diameter_mm": 3}
 ]}
```

| key | meaning |
|---|---|
| geometry | `lx_mm`, `ly_mm`, `lz_mm` (box, depth `lz`) or `radius_mm` + `depth_mm` (cylinder). A volume alone is rejected: it has no modes |
| `ports` | non-empty array of port objects (below). The element takes no `node`/`nodes` |
| *`wall_loss`* | boundary-layer loss, default true |
| *`surface_factor`* | 1–10, scales every wall integral (ribbed walls), default 1 |
| *`wall_area_cm2`* | overrides the geometric wall area of the loss (scales every wall integral), as for `cavity` |
| *`f_max_Hz`* | highest analysed frequency, default 40 kHz. Modes are kept below 3·2π·f_max/c; a cutoff that would keep more than 5·10⁵ modes (about 2.7 L of volume at 40 kHz) is rejected |
| *`residual`* | add the quasi-static contribution of the omitted modes, default true; `false` is for diagnostics only |
| *`max_distance_mm`* | length of the lumped-validity criterion at L0, default the largest dimension |

**Coordinates.** The origin is the centre of the driver face (z = 0), and z
points into the cavity. A box spans x ∈ [−lx/2, lx/2], y ∈ [−ly/2, ly/2] and
z ∈ [0, lz]. A cylinder has its axis on z; the azimuth is measured from +x
towards +y.

**Port objects.** Each port gives `node`, `face`, the two position keys of
that face (the footprint centre) and one footprint: a disk (`radius_mm` or
`diameter_mm`) or a rectangle given by its extents along the two face axes.

| shape | `face` | position keys | rectangle keys |
|---|---|---|---|
| box | `z0` (z = 0), `z1` (z = lz) | `x_mm`, `y_mm` | `lx_mm`, `ly_mm` |
| box | `x0` (x = −lx/2), `x1` (x = +lx/2) | `y_mm`, `z_mm` | `ly_mm`, `lz_mm` |
| box | `y0` (y = −ly/2), `y1` (y = +ly/2) | `x_mm`, `z_mm` | `lx_mm`, `lz_mm` |
| cylinder | `z0`, `z1` | `x_mm`, `y_mm` | `lx_mm`, `ly_mm` |
| cylinder | `side` | `angle_deg`, `z_mm` | `arc_mm` (arc length), `lz_mm` |

Footprints on the side wall are defined on the unrolled wall, in (arc length,
z). Rejected input: an unknown face or key, a missing position key, zero or
two footprints, rectangle keys of another face, a footprint that leaves its
face (touching an edge is allowed, so a full-face piston is valid), two
footprints that overlap on the same face, two ports on the same node, and a
non-acoustic node. A port may be connected to ambient (`gnd`).

**Model.** Each port is a rigid piston: volume velocity enters with uniform
normal velocity over the footprint, and the port pressure is the mean
pressure over it. The impedance matrix is the rigid-wall Green's function
sum Z_ij = (jωρ/V)·Σ_n φ̄_n,i·φ̄_n,j/(k_n² − k² + (j − 1)·η_n). It is
symmetric (reciprocal), and its uniform term is exactly the lumped
compliance of `cavity`, so L0 is its first term. The wall loss η_n is the
Morse–Ingard boundary-layer perturbation, thermal on every wall and viscous
where the mode moves tangentially. For the uniform mode it is the `cavity`
element's thermal correction. Modes are kept below 3× the highest analysed
wavenumber (erratum E27). The omitted modes are added quasi-statically,
because plain truncation leaves 10–100 % errors in the driving-point
impedance of small ports, whose near-field mass sits in those modes; their
continuum part includes the footprint's mirror images in the nearby walls,
which matter for slits along an edge. Against independent waveguide-mode
references (one of them with no continuum model at all, for slits and disks
flush against walls) the result agrees to 1e-4 below f_max/4 and to 1 % at
f_max. The exception is the mutual impedance of two footprints within a
fraction of a millimetre of each other on one face (e.g. parallel 0.3 mm
slits 0.2 mm apart), about 1e-3 off.

**Levels.** At L1 the element is the N-port above. At L0 it is one lumped
compliance with all its port nodes joined, so switching levels never changes
the netlist. Validity: at L0 the lumped kL criterion; at L1 shading begins
at `f_max` and deepens at 3·f_max, where the kept modes run out.

**Probes.** Port k of the element is port k of `ports`. `flow` is the volume
velocity entering the cavity through that footprint, `port_potential` is the
pressure of its node, and `impedance` is their ratio with the rest of the
network attached. A port whose node has no other element acts as a pressure
probe (its flow is zero).

**Loss.** The modal Q from the wall layers is 200–400 for the first modes of
a 60 × 45 × 20 mm box and 280–540 for a cylinder of radius 25 mm and depth
25 mm. Erratum E29's 500–1000 is the thermal part alone; the viscous layer
roughly halves it for modes with tangential motion.

**Cost.** A 60 × 45 × 20 mm box keeps 1446 modes for f_max = 20 kHz and 10 623
for 40 kHz (E27: about 1.4k and 10k). The coupling integrals, wall integrals
and residual are computed once, on the first L1 use. Each frequency then
costs one pass over the kept modes, with P(P + 1)/2 multiply-adds per mode.
For the four-port box of the test `per_frequency_cost` (driver, ear, slit
leak, vent) in a native release build:

| f_max | modes | build | per frequency |
|---|---|---|---|
| 20 kHz | 1446 | 0.11 s | 13 µs |
| 40 kHz | 10 623 | 0.2 s | 90 µs |

Run the test with
`cargo test --release -p acoustilab --test cavity -- --ignored --nocapture`.
