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
| R1, X1 of the baffled piston, 0 ≤ 2ka ≤ 500 | ≤ 3e-15 relative |
| −ln\|R\| and L/a of the unflanged pipe, 0 ≤ ka ≤ 3.8 | ≤ 3e-15 relative |

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
