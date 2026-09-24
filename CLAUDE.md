# AcoustiLab — notes for Claude sessions

- Spec: `docs/spec/AcoustiLab_v2_spec.pdf`. Before implementing anything from it,
  check `docs/spec-errata.md`; several of its formulas and test oracles are wrong.
- Normative engine conventions are in `docs/conventions.md`: across/through
  analogy (mechanical velocity is the node potential), RMS phasors, `e^{+jωt}`,
  and the single lumped-validity metric. Do not reintroduce the spec's
  "impedance analogy" wording or peak phasors.
- The project is theory-only: no measurement rig yet. Every empirical number
  (pads, fabrics, leak distributions, driver internals) is data with provenance,
  never a constant buried in code.
- Standards tables (IEC, ISO) must not be committed. Test vectors from paid
  standards go in the untracked `private/` directory, and tests that need them
  skip when it is absent. ITU-T P.57 is free to download but still copyrighted:
  commit derived model parameters with citations, not verbatim tables.

## Commands

```sh
cargo fmt --all && cargo clippy --all-targets && cargo test
cargo build -p acoustilab --target wasm32-unknown-unknown --release
```

## Adding an element family

Each family is a file in `crates/acoustilab/src/elements/` that exposes `TYPES`
and `constructor(type_name)`, and is registered in `FAMILIES` in
`elements/mod.rs`. Parse parameters through `Build::params` (unit-suffixed keys;
`finish()` rejects leftovers). Reuse `OnePort`, `TwoPort` and `Composite` where
they fit. Every new element needs tests against a closed form or published
data, and an entry in `docs/netlist.md`.
