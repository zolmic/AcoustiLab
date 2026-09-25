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
