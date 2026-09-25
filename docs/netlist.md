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
| *`f_max_Hz`* | highest analysed frequency, default 40 kHz. Modes are kept below 3·2π·f_max/c |
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
impedance of small ports, whose near-field mass sits in those modes. Against
an independent waveguide-mode reference the result agrees to 1e-4 below
f_max/4 and to 1 % at f_max.

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
