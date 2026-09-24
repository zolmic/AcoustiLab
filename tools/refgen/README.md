# Reference-data generators

These scripts compute the reference values that the Rust tests compare against.
They use Python 3 with mpmath and numpy. Each script writes its JSON fixture to
`crates/acoustilab/tests/data/`. Every number is stored as a decimal string
that round-trips to the exact double, so Rust parses the same inputs that
mpmath evaluated.

| script | fixture | checked by |
|---|---|---|
| `special_refs.py` | `special_bessel_ratio.json`, `special_shape.json`, `special_piston.json` | `tests/special_functions.rs` |
| `radiation_refs.py` | `special_unflanged.json` (Levine–Schwinger) | `tests/special_functions.rs` |
| `thermo_refs.py` | `thermo_duct.json`, `thermo_rect.json` | `tests/thermoviscous.rs` |

```sh
python3 tools/refgen/special_refs.py     # a few seconds
python3 tools/refgen/thermo_refs.py      # about a minute
python3 tools/refgen/radiation_refs.py   # about 8 minutes
```

Each value is computed at two working precisions, and the two results must
agree. The documentation string of each script states its formulas and
sources.
