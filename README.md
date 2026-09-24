# AcoustiLab

A headphone design and simulation workbench. At its core is a generic
modified-nodal-analysis solver over a typed electro-mechano-acoustic netlist,
with a thermoviscous element library.

The design specification is `docs/spec/AcoustiLab_v2_spec.pdf`. Its known errors
and their corrections are in `docs/spec-errata.md`. The engine's normative
conventions (analogy, phasors, validity metric) are in `docs/conventions.md`.

**Status: theory-only.** Every model is built from physics, standards and
published data. Nothing has yet been validated against a measurement rig, so
treat absolute levels as predictions, not as measured facts. Comparisons
between designs are more reliable than absolute values.

## Layout

```
crates/acoustilab       engine library (Rust, builds for native and wasm32)
crates/acoustilab-cli   `acoustilab` command-line runner
docs/                   spec, errata, conventions, netlist reference
examples/               example netlists
```

## Build and run

```sh
cargo test                                   # unit, oracle and property tests
cargo run -p acoustilab-cli -- types         # list element types
cargo run -p acoustilab-cli -- solve examples/sealed_cup.json --csv
cargo build -p acoustilab --target wasm32-unknown-unknown --release
```

The netlist format is described in `docs/netlist.md`.
