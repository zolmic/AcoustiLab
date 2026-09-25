# AcoustiLab

A headphone design and simulation workbench. At its core is a generic
modified-nodal-analysis solver over a typed electro-mechano-acoustic netlist,
with a thermoviscous element library, and around it a browser UI for designing
a headphone with sliders while the full netlist stays one tab away.

The design specification is `docs/spec/AcoustiLab_v2_spec.pdf`. Its known errors
and their corrections are in `docs/spec-errata.md`. The engine's normative
conventions (analogy, phasors, validity metric) are in `docs/conventions.md`.

**Status: theory-only.** Every model is built from physics, standards and
published data. Nothing has yet been validated against a measurement rig, so
treat absolute levels as predictions, not as measured facts. Comparisons
between designs are more reliable than absolute values. The reference cup in
`validation/reference_cup/` is the planned first validation: its predictions
were frozen before any measurement.

![Web UI](docs/img/web-ui.png)

## What it does

| | | docs |
|---|---|---|
| **Design** | Parametric netlists: named parameters with ranges, tolerances and expressions; design templates for over-ear (closed or open back), on-ear and in-ear headphones; a Design tab with grouped controls, a to-scale cross-section and live solving | `docs/parameters.md`, `docs/templates.md`, `docs/web.md` |
| **Physics** | Driver macro (D0–D2, creep), cavities (lumped, depth line, modal), thermoviscous tubes, slits and leaks, vents, meshes, porous layers, fills, radiation, cup walls, IEC 60318-4, Type 3.3 and Type 4.3 ear loads; validity shading on every curve, upper and lower | `docs/netlist.md`, `docs/ear-loads.md` |
| **Drive and warnings** | Every level states its drive (voltage, power into rated impedance, IEC characteristic voltage, constant current); warnings for vent and leak air speed, excursion and coil power at that drive | `docs/netlist.md` |
| **Analysis** | Readouts (resonance, Q, sensitivity, bass extension, impedance checks), sensitivities in dB per %, tornado charts, explanation sentences computed from re-solves, Monte Carlo tolerance envelopes | `docs/analysis.md` |
| **Targets** | Target curves with fixture and provenance (the CC-BY Ravizza 2023 B&K 5128 set), smoothing, error metrics, BS.708 mask, Harman preference scores (greyed where they do not apply) | `docs/targets.md` |
| **Time domain and isolation** | Impulse, step and energy-time curves, minimum and excess phase, group delay, vector fitting with a pole/Q table, passive insertion loss and bleed | `docs/time-domain.md`, `docs/isolation.md` |
| **Measurements** | FRD, ZMA, REW and CSV import/export with metadata sidecars, parameter fitting with an identifiability report, and a virtual rig for synthetic measurements until a real one exists | `docs/fitting.md` |
| **Listening** | Difference filters between two designs (or a design and a target), level matched per BS.1770, played through an AudioWorklet convolver with a true-peak limiter | `docs/auralization.md` |
| **Validation** | A 3-D printable reference cup around the Tymphany HPD-40N16PET00-32, its model, a measurement protocol, frozen blind predictions and `acoustilab validate` | `docs/reference-cup.md` |

## Layout

```
crates/acoustilab        engine library (Rust, builds for native and wasm32)
crates/acoustilab-cli    `acoustilab` command-line runner
crates/acoustilab-wasm   wasm-bindgen wrapper used by the web UI
web/                     browser UI (Vite + TypeScript), see docs/web.md
data/                    driver records, ear loads, materials, targets, fixtures (with provenance)
examples/                design templates and example netlists
validation/              the reference cup: CAD, protocol, frozen predictions
tools/                   Python generators for reference fixtures and fits
docs/                    spec, errata, conventions and one document per topic above
```

## Build and run

The browser UI (Rust with the `wasm32-unknown-unknown` target, and Node 20.19+
or 22.12+):

```sh
cd web && npm ci && npm run dev               # http://localhost:5173
```

The engine and command line:

```sh
cargo test                                    # unit, oracle and property tests
cargo run -p acoustilab-cli -- help           # every subcommand
cargo run -p acoustilab-cli -- params examples/design_over_ear.json
cargo run -p acoustilab-cli -- solve examples/design_over_ear.json --set vent_count=3 --set rear=open --csv
cargo run -p acoustilab-cli -- readouts examples/design_over_ear.json
cargo run -p acoustilab-cli -- explain examples/design_over_ear.json
cargo run -p acoustilab-cli -- score examples/design_over_ear.json --target ravizza2023_5128
cargo run -p acoustilab-cli -- validate validation/reference_cup --verify
```
