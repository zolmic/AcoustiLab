# Engine conventions

These are normative for every element, import and export. Where they differ
from the spec (`docs/spec/AcoustiLab_v2_spec.pdf`), the reason is given and
the matching erratum is listed in `docs/spec-errata.md`.

## Phasors and time convention

- Time convention is `e^{+jωt}` everywhere. Thermoviscous square roots, the
  thermal wall-layer factor `(1 − j)/2` and modal damping signs follow from it.
- **All phasors are RMS amplitudes.** A 1 V source means 1 V RMS; dB SPL is
  `20·log10(|p| / 20 µPa)` with no further factor. (The spec leaves peak vs RMS
  open; that is a silent 3 dB.)
- Internal units are SI. Netlist keys carry their unit as a suffix
  (`radius_mm`, `volume_cm3`, `Bl_Tm`). A key without a suffix, two spellings of
  the same quantity, or an unused key is an error.

## Network analogy: across/through in every domain

The spec (Section 3) says "impedance analogy in all three domains", but its own
example netlist and its "fixed frame as mechanical ground" are the mobility
picture. The engine uses the **across/through** convention (as Modelica and
Simscape do), in which the netlist topology mirrors the physical structure:

| Domain | Across (node potential) | Through (branch flow) | Ground |
|---|---|---|---|
| Electrical | voltage V | current A | circuit return |
| Mechanical | velocity m/s | force N | inertial frame (zero velocity) |
| Acoustic | pressure Pa | volume velocity m³/s | ambient pressure |

Consequences:

- A **mass** is connected from its node to the frame (ground). **Springs** and
  **dampers** connect two nodes. Parts that move together share a node.
- The **motor** (Bl) is an ideal transformer between the electrical and
  mechanical ports: `e = Bl·v`, `F = Bl·i`.
- The **piston** (Sd) is a gyrator between the mechanical and acoustic ports:
  it injects `U = Sd·v` into the front node, draws it from the rear node, and
  loads the diaphragm with `F = Sd·(p_front − p_rear)`.
- A cavity is a compliance from its node to ambient; a duct, leak or vent is a
  series element between two acoustic nodes.
- Every KCL row sums flows *leaving* the node. A flow probe on an element port
  is positive entering the element at its first terminal. Sources report the
  flow they deliver into the network, so potential/flow at a source is the
  impedance it sees.

## Sign convention

Positive source voltage drives positive coil current, positive force and
velocity toward the ear, and positive front-cavity and drum pressure. The test
is evaluated at a low non-zero frequency (20 Hz), because zero frequency is
excluded from every grid and a closed design always has an equalisation leak.

## Air

- `air.preset = "spec_reference"`: ρ = 1.204 kg/m³, c = 343 m/s,
  μ = 1.81e-5 Pa·s, γ = 1.4, Pr = 0.71, with P0 = ρc²/γ so that `V/(γP0)` and
  `V/(ρc²)` agree exactly. This is what the analytical checks use.
- The default is `standard_23C` (IEC 60318-4 conditions): 23 °C, 101.325 kPa,
  ideal gas, Sutherland viscosity and conductivity.

## Fidelity levels

- `level: 0`, lumped: cavities are compliances with thermal wall loss; ducts are
  frequency-dependent series R + jωM (thermoviscous-corrected, compressibility
  neglected).
- `level: 1`, distributed, the default: ducts are full thermoviscous
  transmission-line two-ports. A two-node cavity (driver face, far face) is a
  depth line whose end faces carry their thermal loss.
- Switching level never changes the netlist. A two-node cavity at L0 joins its
  faces with an ideal short.

## Validity: one error metric

The spec quotes two incompatible lumped-error measures (erratum E3). The engine
uses the worst-case compliance error of a closed duct driven at one end:

`e(kL) = 1 − kL·cot(kL)`, which is 3.0 % at kL 0.3, 8.5 % at 0.5 and 35.8 % at 1.0.

- Shading begins at 10 % (kL ≈ 0.54; 493 Hz for L = 60 mm) and deepens at 36 %
  (kL ≈ 1.0; 910 Hz).
- Lumped ducts use the inertance error `tan(kl)/kl − 1`.
- Distributed ducts are limited by their first transverse mode and by Stinson's
  bound `r·f^1.5 < 1e6` (cm, Hz).
- Continuity checks between levels allow 0.3 dB below the 3 % frequency, and
  0.1 dB below kL ≈ 0.17 (1 %).

## Thermoviscous functions

The spec's Appendix D is wrong for circular ducts (erratum E1). The engine uses:

- `k_v = sqrt(jωρ/μ)` and `k_t = k_v·sqrt(Pr)`.
- Slits: `F(z) = tanh(z)/z` with `z = k·h/2`, where h/2 is the half-gap.
- Circles: `F(z) = 2·I1(z)/(z·I0(z))` with `z = k·a`, where a is the radius. These
  are *modified* Bessel functions, and `1 − F = I2/I0` is computed without
  cancellation.
- `ρ_eff = ρ/(1 − F_v)` and `K_eff = γP0/(1 + (γ−1)·F_t)`.
- `Γ = jω·sqrt(ρ_eff/K_eff)` on the branch with `Re Γ ≥ 0`, and
  `Z_c = sqrt(ρ_eff·K_eff)/S`.
- Rectangular ducts with both sides finite (`Section::Rect`, element
  `rect_duct`) use Stinson's 1991 double series. `Section::shape` takes the
  complex wavenumber k, because the rectangle depends on both sides.
- Lumped (L0) ducts are shaded where |tan x/x − 1| reaches 10 % and 36 %, with
  the lossy complex length `x = −jΓl`. Narrow ducts leave the lumped regime
  well below the lossless kl estimate.

## Linear solve

The linear solve is a dense LU with row/column equilibration and partial
pivoting, followed by up to two steps of iterative refinement against the
unscaled matrix. Without refinement, potentials far below the largest one
are accurate only norm-wise. `linalg::Lu` keeps the factors so further
right-hand sides (refinement, sensitivities) reuse them.

## Known limits of level 0

- A two-node cavity at L0 joins its faces with an ideal short. This drops the
  depth line's air inertance ρd/S. If the far face opens onto a load of
  comparable inertance (a wide vent or a radiating hole), L0 and L1 differ at
  every frequency, and the kL shading does not flag it. Use L1 for such
  designs.
- L0 ducts neglect compressibility, so near a duct's anti-resonance the two
  levels can differ by more than the shading suggests.

## Two-ports

Two-ports are stamped directly in transmission (ABCD) form, with two MNA branch
unknowns. This stays valid where B or C vanish, such as lossless half-wave
lines, so no admittance-conversion fallback is needed (erratum E14).

## Driver records

The reference driver's datasheet is internally inconsistent in Qes (erratum E5).
Records declare a primary parameter set: fs, Qms, Qes, Re, Mms and Sd. Bl, Cms
and Rms are derived from it, and any other datasheet values are kept for the
consistency report.
