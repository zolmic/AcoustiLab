# Parametric netlists

A netlist can declare named parameters and use them in expressions anywhere
in the document. This is how a design exposes its knobs: vent count and size,
cup depth, leak gap, the ear load. The design panel of the UI draws a control
for each parameter, and sensitivities, fits, tolerance analysis and optimisers
act on the same names. The netlist stays the single source of truth (spec
Section 2): a parameter value only changes the numbers the netlist resolves to.

`examples/design_over_ear.json` is a complete template.

## Declaring parameters

```json
"parameters": {
  "vent_count": {"value": 1, "min": 0, "max": 12, "integer": true,
                 "label": "Number of rear vents", "group": "Rear"},
  "vent_diameter_mm": {"value": 3, "min": 0.3, "max": 8, "step": 0.1,
                       "tolerance": {"abs": 0.05, "source": "estimate (moulding)"}},
  "ear": {"value": "iec60318_4", "choices": [
            {"value": "iec60318_4", "label": "IEC 60318-4 ear simulator"},
            {"value": "type43", "label": "ITU-T P.57 Type 4.3 ear"}]},
  "sealed": false,
  "front_volume_cm3": {"expr": "pi * front_radius_mm^2 * front_depth_mm / 1000"},
  "leak_gap_mm": 0.08
}
```

A bare number, boolean or string is shorthand for `{"value": ...}`. Names start
with a letter or `_` and contain letters, digits and `_`. The names of constants
and functions (`pi`, `e`, `sqrt`, `if`, ...) are reserved.

| kind | declared by | notes |
|---|---|---|
| number | numeric `value` | *`min`*, *`max`* (hard bounds), *`step`* (UI hint), *`log`* (log-scale slider; needs a positive `min`), *`tolerance`* |
| integer | numeric `value` with `"integer": true` | counts such as vents or segments; no tolerance |
| boolean | `true` / `false` | |
| choice | string `value` with `choices` | each choice is a string or `{"value", "label"}` |
| derived | `expr` | computed from other parameters; cannot be set; no bounds or tolerance |

Metadata for any kind: *`label`*, *`group`* (panel section), *`unit`* (display
unit; defaults to the name's unit suffix), *`description`*, *`advanced`* (shown
only in the detailed view).

**Units.** A parameter's value is a plain number in the unit its name ends
with, `_mm`, `_cm3`, `_rayl` and so on. The engine does not convert it; the
key that receives it states the unit, as everywhere in a netlist. When a key
takes a parameter verbatim and both names carry a unit, the units must agree:
`"radius_mm": "=cup_radius_cm"` is an error. Convert in the expression
instead, `"radius_mm": "=cup_radius_cm * 10"`.

**Tolerance.** `{"rel": 0.05}` (±5 %) or `{"abs": 0.05}` (in the parameter's
unit), with *`dist`* and *`source`* (where the number comes from). With μ the
parameter's current value and t the half-width (`rel`·|μ| or `abs`):

| `dist` | samples | tornado ends |
|---|---|---|
| `normal` (default) | μ + (t/2)·z: the tolerance is two standard deviations (95.4 % coverage) | μ ± t |
| `uniform` | flat over μ ± t | μ ± t |
| `lognormal` (needs `rel`) | μ·exp(σ·z), σ = ln(1 + rel)/2: ln x is normal with median μ, and its 2σ points are μ·(1 + rel) and μ/(1 + rel) | μ/(1 + rel), μ·(1 + rel) |

z is a standard normal deviate. A lognormal tolerance is therefore +rel above
and −rel/(1 + rel) below (for 0.5: 1.5μ and 0.667μ), which suits gaps and
leaks that cannot go negative. Monte Carlo analysis samples these (Latin
hypercube, `docs/analysis.md`); samples outside `min`/`max` are clipped to
the bounds and counted.

## Expressions

Any string that begins with `=` is an expression. It is replaced by its value
before the netlist is read. The exceptions are `title`, `description`, `ui`,
`schema`, ids and the `parameters` block itself. Expressions may appear in
element parameters, in arrays and nested objects (`"gaps_mm": ["=g", "=g*2"]`),
in `sweep`, `air`, `level` and `drive`, in node names (`"node": "=if(open,
'ambient', 'a_rear')"`) and in an element's `type`.

| | |
|---|---|
| values | numbers, booleans, strings (`'...'` or `"..."`) |
| arithmetic | `+ - * / %`, `^` or `**` (right-associative; `-2^2` is −4), `+` also joins strings |
| comparison | `== != < <= > >=` (do not chain) |
| logic | `&& \|\| !` (short-circuit) |
| constants | `pi`, `e`, `true`, `false` |
| functions | `sqrt abs exp ln log10 log2 sin cos tan asin acos atan atan2 hypot floor ceil round min max clamp(x, lo, hi) if(cond, a, b)` |

`if` evaluates only the branch it takes, so `if(n > 0, 1/n, 0)` is safe. NaN
and infinities are never values: division by zero, `sqrt(-1)` or an overflow
is an error naming the element and key. An integral result becomes a JSON
integer, so `"count": "=vent_count"` works.

## Enabling and disabling items

Items of `nodes`, `elements` and `probes` may carry `"enabled": true | false |
"=expression"`. A disabled item is removed before anything inside it is
evaluated, so it may contain expressions that are invalid in that state. Two
items may share an id when at most one of them is enabled. The template uses
this for alternatives, such as a vent with a mesh and a vent without one, or
two ear loads both named `ear`, so that probes can refer to `ear.drp` whichever
ear is chosen.

A node that loses all its elements makes the matrix singular. Disable it
together with them.

## Overrides

* CLI: `acoustilab solve design.json --set vent_count=3 --set rear=open`, and
  `acoustilab params design.json` to list the values.
* Rust: `Circuit::from_json_with(text, &overrides)` or
  `Circuit::from_parametric(&Parametric::parse(text)?, &overrides)`.
* WebAssembly: `solve_with(netlist, '{"vent_count": 3}')`, and
  `parameters(netlist, overrides)` for the panel description.

An override must name a declared, non-derived parameter and respect its kind,
bounds and choices; otherwise the error has kind `parameter`. The resolved
values, derived ones included, are reported in `meta.parameters` of every
result.

## The `ui` block

The engine ignores the top-level `ui` object; user interfaces read it for
presentation hints. The template uses `{"template": "over_ear", "primary_probe":
"p_drp"}`. See `docs/web.md` for the keys the web UI understands.
