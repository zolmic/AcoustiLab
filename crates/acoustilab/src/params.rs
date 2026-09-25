//! Parametric netlists: the `parameters` block, `=expression` values and
//! `enabled` flags (docs/parameters.md).
//!
//! ```json
//! "parameters": {
//!   "vent_count": {"value": 2, "min": 0, "max": 8, "integer": true,
//!                  "label": "Rear vents", "group": "Rear"},
//!   "vent_diameter_mm": {"value": 2, "min": 0.5, "max": 6, "step": 0.1,
//!                        "tolerance": {"abs": 0.05, "source": "moulding"}},
//!   "ear": {"value": "iec60318_4", "choices": ["iec60318_4", "type43"]},
//!   "front_volume_cm3": {"expr": "pi * cup_radius_mm^2 * depth_mm / 1000"},
//!   "leak_gap_mm": 0.1
//! }
//! ```
//!
//! A parameter's value is a plain number in the unit its name ends with
//! (`_mm`, `_cm3`, ...); the engine does not convert it. Any string that
//! begins with `=` elsewhere in the document is an expression over the
//! parameters, replaced by its value before the netlist is read. Items of
//! `nodes`, `elements` and `probes` may carry `"enabled": <bool or
//! expression>`; disabled items are removed before anything else in them is
//! evaluated. `title`, `description` and `ui` are never evaluated.

use crate::error::{Error, Result};
use crate::expr::{is_identifier, is_reserved, Expr, PValue};
use crate::units::unit_suffix;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

/// Parameter overrides by name.
pub type Overrides = BTreeMap<String, PValue>;

/// Distribution of a toleranced parameter for Monte Carlo sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dist {
    /// Normal, with the tolerance at two standard deviations (95 % coverage).
    Normal,
    /// Uniform over ± the tolerance.
    Uniform,
    /// Log-normal: ln(value) normal, with ±tolerance (relative) at 2σ.
    Lognormal,
}

/// Manufacturing or datasheet tolerance of a numeric parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tolerance {
    /// Relative half-width (0.05 = ±5 %).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rel: Option<f64>,
    /// Absolute half-width, in the parameter's unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs: Option<f64>,
    pub dist: Dist,
    /// Where the tolerance comes from ("datasheet", "industry practice", ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Tolerance {
    /// Half-width in the parameter's unit at `value`.
    pub fn half_width(&self, value: f64) -> f64 {
        match (self.rel, self.abs) {
            (Some(r), _) => r * value.abs(),
            (None, Some(a)) => a,
            (None, None) => 0.0,
        }
    }
}

/// One option of a choice parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Choice {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    Number {
        value: f64,
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
        integer: bool,
        /// Hint for sliders: logarithmic scale.
        log: bool,
    },
    Bool {
        value: bool,
    },
    Choice {
        value: String,
        choices: Vec<Choice>,
    },
    /// Computed from other parameters; cannot be overridden.
    Derived {
        expr: Expr,
    },
}

/// A declared parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDef {
    pub name: String,
    pub kind: ParamKind,
    pub label: Option<String>,
    pub group: Option<String>,
    /// Display unit; defaults to the name's unit suffix.
    pub unit: Option<String>,
    pub description: Option<String>,
    /// Shown only in the detailed view of a design.
    pub advanced: bool,
    pub tolerance: Option<Tolerance>,
}

impl ParamDef {
    pub fn is_derived(&self) -> bool {
        matches!(self.kind, ParamKind::Derived { .. })
    }

    /// Default value (`None` for derived parameters).
    pub fn default_value(&self) -> Option<PValue> {
        match &self.kind {
            ParamKind::Number { value, .. } => Some(PValue::Num(*value)),
            ParamKind::Bool { value } => Some(PValue::Bool(*value)),
            ParamKind::Choice { value, .. } => Some(PValue::Str(value.clone())),
            ParamKind::Derived { .. } => None,
        }
    }

    /// Continuous numeric parameter (the ones sensitivities, fits and Monte
    /// Carlo act on).
    pub fn is_continuous(&self) -> bool {
        matches!(self.kind, ParamKind::Number { integer: false, .. })
    }

    /// Bounds of a numeric parameter.
    pub fn bounds(&self) -> (Option<f64>, Option<f64>) {
        match self.kind {
            ParamKind::Number { min, max, .. } => (min, max),
            _ => (None, None),
        }
    }

    /// Checks that `v` is an admissible value.
    pub fn check(&self, v: &PValue) -> std::result::Result<(), String> {
        match (&self.kind, v) {
            (ParamKind::Derived { .. }, _) => Err("is derived; it cannot be set".into()),
            (
                ParamKind::Number {
                    min, max, integer, ..
                },
                PValue::Num(x),
            ) => {
                if !x.is_finite() {
                    return Err(format!("{x} is not finite"));
                }
                if *integer && x.fract() != 0.0 {
                    return Err(format!("must be an integer, got {x}"));
                }
                if let Some(lo) = min {
                    if x < lo {
                        return Err(format!("{x} is below the minimum {lo}"));
                    }
                }
                if let Some(hi) = max {
                    if x > hi {
                        return Err(format!("{x} is above the maximum {hi}"));
                    }
                }
                Ok(())
            }
            (ParamKind::Bool { .. }, PValue::Bool(_)) => Ok(()),
            (ParamKind::Choice { choices, .. }, PValue::Str(s)) => {
                if choices.iter().any(|c| &c.value == s) {
                    Ok(())
                } else {
                    Err(format!(
                        "'{s}' is not one of {}",
                        choices
                            .iter()
                            .map(|c| format!("'{}'", c.value))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
            (k, v) => Err(format!(
                "expects a {}, got {}",
                match k {
                    ParamKind::Number { .. } => "number",
                    ParamKind::Bool { .. } => "boolean",
                    _ => "string",
                },
                v.type_name()
            )),
        }
    }

    /// Description for user interfaces, with `value` the resolved value.
    pub fn describe(&self, value: Option<&PValue>) -> Value {
        let mut o = Map::new();
        o.insert("name".into(), json!(self.name));
        let kind = match &self.kind {
            ParamKind::Number {
                value: d,
                min,
                max,
                step,
                integer,
                log,
            } => {
                o.insert("default".into(), json!(d));
                o.insert("min".into(), json!(min));
                o.insert("max".into(), json!(max));
                o.insert("step".into(), json!(step));
                o.insert("log".into(), json!(log));
                if *integer {
                    "integer"
                } else {
                    "number"
                }
            }
            ParamKind::Bool { value: d } => {
                o.insert("default".into(), json!(d));
                "boolean"
            }
            ParamKind::Choice { value: d, choices } => {
                o.insert("default".into(), json!(d));
                o.insert("choices".into(), json!(choices));
                "choice"
            }
            ParamKind::Derived { expr } => {
                o.insert("expr".into(), json!(expr.source()));
                "derived"
            }
        };
        o.insert("kind".into(), json!(kind));
        o.insert("value".into(), value.map_or(Value::Null, PValue::to_json));
        o.insert(
            "label".into(),
            json!(self.label.clone().unwrap_or_else(|| self.name.clone())),
        );
        o.insert("group".into(), json!(self.group));
        o.insert("unit".into(), json!(self.display_unit()));
        o.insert("description".into(), json!(self.description));
        o.insert("advanced".into(), json!(self.advanced));
        o.insert("tolerance".into(), json!(self.tolerance));
        Value::Object(o)
    }

    /// Display unit: the declared `unit`, else the name's suffix.
    pub fn display_unit(&self) -> Option<String> {
        self.unit
            .clone()
            .or_else(|| unit_suffix(&self.name).map(|(s, _, _)| s.to_string()))
    }
}

/// Keys a parameter object may carry.
const PARAM_KEYS: &[&str] = &[
    "value",
    "expr",
    "min",
    "max",
    "step",
    "integer",
    "log",
    "choices",
    "label",
    "group",
    "unit",
    "description",
    "advanced",
    "tolerance",
];

fn perr(name: &str, msg: impl Into<String>) -> Error {
    Error::Parameter {
        name: name.to_string(),
        msg: msg.into(),
    }
}

fn opt_str(o: &Map<String, Value>, name: &str, key: &str) -> Result<Option<String>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(perr(name, format!("'{key}' must be a string"))),
    }
}

fn opt_num(o: &Map<String, Value>, name: &str, key: &str) -> Result<Option<f64>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map(Some)
            .ok_or_else(|| perr(name, format!("'{key}' must be a finite number"))),
    }
}

fn opt_bool(o: &Map<String, Value>, name: &str, key: &str) -> Result<bool> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(perr(name, format!("'{key}' must be true or false"))),
    }
}

fn parse_tolerance(name: &str, v: &Value) -> Result<Tolerance> {
    let o = v
        .as_object()
        .ok_or_else(|| perr(name, "'tolerance' must be an object"))?;
    if let Some(k) = o
        .keys()
        .find(|k| !["rel", "abs", "dist", "source"].contains(&k.as_str()))
    {
        return Err(perr(name, format!("tolerance: unknown key '{k}'")));
    }
    let rel = opt_num(o, name, "rel")?;
    let abs = opt_num(o, name, "abs")?;
    match (rel, abs) {
        (Some(r), None) if r > 0.0 && r < 1.0 => {}
        (None, Some(a)) if a > 0.0 => {}
        (Some(_), Some(_)) | (None, None) => {
            return Err(perr(name, "tolerance needs exactly one of 'rel' or 'abs'"))
        }
        _ => {
            return Err(perr(
                name,
                "tolerance 'rel' must be in (0, 1) and 'abs' positive",
            ))
        }
    }
    let dist = match opt_str(o, name, "dist")?.as_deref() {
        None | Some("normal") => Dist::Normal,
        Some("uniform") => Dist::Uniform,
        Some("lognormal") => Dist::Lognormal,
        Some(d) => {
            return Err(perr(
                name,
                format!("tolerance: unknown dist '{d}' (normal, uniform, lognormal)"),
            ))
        }
    };
    if dist == Dist::Lognormal && rel.is_none() {
        return Err(perr(name, "a lognormal tolerance needs 'rel'"));
    }
    Ok(Tolerance {
        rel,
        abs,
        dist,
        source: opt_str(o, name, "source")?,
    })
}

fn parse_def(name: &str, v: &Value) -> Result<ParamDef> {
    if !is_identifier(name) {
        return Err(perr(
            name,
            "names must start with a letter or '_' and contain only letters, digits and '_'",
        ));
    }
    if is_reserved(name) {
        return Err(perr(name, "is a reserved name (a constant or function)"));
    }
    // Shorthand: a bare scalar is the value.
    let obj = match v {
        Value::Object(o) => o.clone(),
        Value::Number(_) | Value::Bool(_) | Value::String(_) => {
            let mut o = Map::new();
            o.insert("value".into(), v.clone());
            o
        }
        _ => return Err(perr(name, "must be a number, boolean, string or object")),
    };
    if let Some(k) = obj.keys().find(|k| !PARAM_KEYS.contains(&k.as_str())) {
        return Err(perr(name, format!("unknown key '{k}'")));
    }
    let kind = match (obj.get("value"), obj.get("expr")) {
        (Some(_), Some(_)) => return Err(perr(name, "give 'value' or 'expr', not both")),
        (None, None) => return Err(perr(name, "needs 'value' or 'expr'")),
        (None, Some(Value::String(src))) => {
            let src = src.strip_prefix('=').unwrap_or(src);
            let expr = Expr::parse(src).map_err(|e| perr(name, format!("expr '{src}': {e}")))?;
            for k in [
                "min",
                "max",
                "step",
                "integer",
                "log",
                "choices",
                "tolerance",
            ] {
                if obj.contains_key(k) {
                    return Err(perr(name, format!("a derived parameter takes no '{k}'")));
                }
            }
            ParamKind::Derived { expr }
        }
        (None, Some(_)) => return Err(perr(name, "'expr' must be a string")),
        (Some(Value::Number(n)), None) => {
            let value = n
                .as_f64()
                .filter(|x| x.is_finite())
                .ok_or_else(|| perr(name, "value must be finite"))?;
            if obj.contains_key("choices") {
                return Err(perr(name, "'choices' needs a string value"));
            }
            let step = opt_num(&obj, name, "step")?;
            if step.is_some_and(|s| s <= 0.0) {
                return Err(perr(name, "'step' must be positive"));
            }
            let (min, max) = (opt_num(&obj, name, "min")?, opt_num(&obj, name, "max")?);
            if let (Some(a), Some(b)) = (min, max) {
                if a > b {
                    return Err(perr(name, format!("min {a} exceeds max {b}")));
                }
            }
            let log = opt_bool(&obj, name, "log")?;
            if log && min.is_none_or(|m| m <= 0.0) {
                return Err(perr(name, "a 'log' parameter needs a positive 'min'"));
            }
            ParamKind::Number {
                value,
                min,
                max,
                step,
                integer: opt_bool(&obj, name, "integer")?,
                log,
            }
        }
        (Some(Value::Bool(b)), None) => {
            for k in [
                "min",
                "max",
                "step",
                "integer",
                "log",
                "choices",
                "tolerance",
            ] {
                if obj.contains_key(k) {
                    return Err(perr(name, format!("a boolean parameter takes no '{k}'")));
                }
            }
            ParamKind::Bool { value: *b }
        }
        (Some(Value::String(s)), None) => {
            if s.starts_with('=') {
                return Err(perr(name, "use 'expr' for a computed parameter"));
            }
            for k in ["min", "max", "step", "integer", "log", "tolerance"] {
                if obj.contains_key(k) {
                    return Err(perr(name, format!("a choice parameter takes no '{k}'")));
                }
            }
            let choices = match obj.get("choices") {
                None => return Err(perr(name, "a string parameter needs 'choices'")),
                Some(Value::Array(a)) if !a.is_empty() => a
                    .iter()
                    .map(|c| match c {
                        Value::String(v) => Ok(Choice {
                            value: v.clone(),
                            label: None,
                        }),
                        Value::Object(o) => {
                            if let Some(k) = o.keys().find(|k| *k != "value" && *k != "label") {
                                return Err(perr(name, format!("choice: unknown key '{k}'")));
                            }
                            Ok(Choice {
                                value: opt_str(o, name, "value")?
                                    .ok_or_else(|| perr(name, "choice objects need 'value'"))?,
                                label: opt_str(o, name, "label")?,
                            })
                        }
                        _ => Err(perr(name, "choices must be strings or {value, label}")),
                    })
                    .collect::<Result<Vec<_>>>()?,
                Some(_) => return Err(perr(name, "'choices' must be a non-empty array")),
            };
            for (i, c) in choices.iter().enumerate() {
                if choices[..i].iter().any(|d| d.value == c.value) {
                    return Err(perr(name, format!("duplicate choice '{}'", c.value)));
                }
            }
            ParamKind::Choice {
                value: s.clone(),
                choices,
            }
        }
        (Some(_), None) => return Err(perr(name, "value must be a number, boolean or string")),
    };
    let tolerance = obj
        .get("tolerance")
        .map(|t| parse_tolerance(name, t))
        .transpose()?;
    if tolerance.is_some() {
        if let ParamKind::Number { integer: true, .. } = kind {
            return Err(perr(name, "integer parameters take no tolerance"));
        }
    }
    let def = ParamDef {
        name: name.to_string(),
        kind,
        label: opt_str(&obj, name, "label")?,
        group: opt_str(&obj, name, "group")?,
        unit: opt_str(&obj, name, "unit")?,
        description: opt_str(&obj, name, "description")?,
        advanced: opt_bool(&obj, name, "advanced")?,
        tolerance,
    };
    if let Some(v) = def.default_value() {
        def.check(&v)
            .map_err(|e| perr(name, format!("default {e}")))?;
    }
    Ok(def)
}

/// A netlist document with its parameter declarations.
#[derive(Debug, Clone)]
pub struct Parametric {
    /// The document without its `parameters` block.
    pub doc: Map<String, Value>,
    /// Declarations in document order.
    pub defs: Vec<ParamDef>,
}

/// A document with every parameter resolved and every expression replaced.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Values of every parameter (derived included), in declaration order.
    pub values: Vec<(String, PValue)>,
    /// The expanded document, ready for [`crate::netlist::Document`].
    pub doc: Value,
}

impl Resolved {
    pub fn get(&self, name: &str) -> Option<&PValue> {
        self.values.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// `{name: value}` in declaration order.
    pub fn values_json(&self) -> Value {
        Value::Object(
            self.values
                .iter()
                .map(|(n, v)| (n.clone(), v.to_json()))
                .collect(),
        )
    }
}

impl Parametric {
    pub fn parse(text: &str) -> Result<Parametric> {
        Self::from_value(serde_json::from_str(text)?)
    }

    pub fn from_value(v: Value) -> Result<Parametric> {
        let mut doc = match v {
            Value::Object(m) => m,
            _ => return Err(Error::Netlist("top level must be an object".into())),
        };
        let defs = match doc.remove("parameters") {
            None => Vec::new(),
            Some(Value::Object(m)) => m
                .iter()
                .map(|(k, v)| parse_def(k, v))
                .collect::<Result<Vec<_>>>()?,
            Some(_) => {
                return Err(Error::Netlist(
                    "'parameters' must be an object of named parameters".into(),
                ))
            }
        };
        Ok(Parametric { doc, defs })
    }

    pub fn def(&self, name: &str) -> Option<&ParamDef> {
        self.defs.iter().find(|d| d.name == name)
    }

    /// Resolves every parameter (defaults, then `overrides`, then derived
    /// ones in dependency order).
    pub fn values(&self, overrides: &Overrides) -> Result<Vec<(String, PValue)>> {
        for (name, v) in overrides {
            let def = self
                .def(name)
                .ok_or_else(|| perr(name, "no such parameter"))?;
            def.check(v).map_err(|e| perr(name, e))?;
        }
        let mut known: HashMap<&str, PValue> = HashMap::new();
        for d in &self.defs {
            if let Some(v) = overrides
                .get(&d.name)
                .cloned()
                .or_else(|| d.default_value())
            {
                known.insert(&d.name, v);
            }
        }
        // Derived parameters, depth-first with cycle detection.
        let index: HashMap<&str, &ParamDef> =
            self.defs.iter().map(|d| (d.name.as_str(), d)).collect();
        fn visit<'a>(
            name: &'a str,
            index: &HashMap<&'a str, &'a ParamDef>,
            known: &mut HashMap<&'a str, PValue>,
            stack: &mut Vec<&'a str>,
        ) -> Result<()> {
            if known.contains_key(name) {
                return Ok(());
            }
            let Some(def) = index.get(name) else {
                return Ok(()); // reported as an unknown name when evaluated
            };
            let ParamKind::Derived { expr } = &def.kind else {
                return Ok(());
            };
            if let Some(pos) = stack.iter().position(|s| *s == name) {
                let mut cycle: Vec<&str> = stack[pos..].to_vec();
                cycle.push(name);
                return Err(perr(
                    name,
                    format!("circular definition: {}", cycle.join(" -> ")),
                ));
            }
            stack.push(def.name.as_str());
            for dep in expr.names() {
                if let Some((k, _)) = index.get_key_value(dep.as_str()) {
                    visit(k, index, known, stack)?;
                }
            }
            stack.pop();
            let v = expr
                .eval(&|n| known.get(n).cloned())
                .map_err(|e| perr(name, format!("expr '{}': {e}", expr.source())))?;
            known.insert(def.name.as_str(), v);
            Ok(())
        }
        let mut stack = Vec::new();
        for d in &self.defs {
            visit(d.name.as_str(), &index, &mut known, &mut stack)?;
        }
        Ok(self
            .defs
            .iter()
            .map(|d| (d.name.clone(), known[d.name.as_str()].clone()))
            .collect())
    }

    /// Resolves parameters and expands the document.
    pub fn resolve(&self, overrides: &Overrides) -> Result<Resolved> {
        let values = self.values(overrides)?;
        let env: HashMap<&str, &PValue> = values.iter().map(|(n, v)| (n.as_str(), v)).collect();
        let lookup = |n: &str| env.get(n).map(|v| (*v).clone());
        let mut doc = self.doc.clone();
        for (key, v) in doc.iter_mut() {
            match key.as_str() {
                "title" | "description" | "ui" | "schema" => {}
                "nodes" | "elements" | "probes" => expand_list(key, v, &lookup)?,
                _ => substitute(v, &Loc::top(key), key, &lookup)?,
            }
        }
        Ok(Resolved {
            values,
            doc: Value::Object(doc),
        })
    }

    /// Description of every parameter for user interfaces, with resolved
    /// values, plus the document's `ui` block.
    pub fn describe(&self, overrides: &Overrides) -> Result<Value> {
        let values = self.values(overrides)?;
        let params: Vec<Value> = self
            .defs
            .iter()
            .map(|d| d.describe(values.iter().find(|(n, _)| *n == d.name).map(|(_, v)| v)))
            .collect();
        Ok(json!({
            "parameters": params,
            "ui": self.doc.get("ui").cloned().unwrap_or(Value::Null),
        }))
    }
}

/// Where a value sits, for error messages.
struct Loc {
    kind: LocKind,
    id: String,
    path: String,
}

#[derive(Clone, Copy)]
enum LocKind {
    Top,
    Node,
    Element,
    Probe,
}

impl Loc {
    fn top(key: &str) -> Loc {
        Loc {
            kind: LocKind::Top,
            id: String::new(),
            path: key.to_string(),
        }
    }

    fn err(&self, msg: String) -> Error {
        match self.kind {
            LocKind::Top => Error::Netlist(format!("'{}': {msg}", self.path)),
            LocKind::Node => Error::Netlist(format!("node '{}': {msg}", self.id)),
            LocKind::Element => {
                Error::element(self.id.clone(), format!("key '{}': {msg}", self.path))
            }
            LocKind::Probe => Error::Probe {
                id: self.id.clone(),
                msg: format!("key '{}': {msg}", self.path),
            },
        }
    }
}

/// Parses and evaluates the expression `src` (the text after `=`).
fn eval_str(
    src: &str,
    lookup: &dyn Fn(&str) -> Option<PValue>,
    loc: &Loc,
) -> Result<(Expr, PValue)> {
    let e = Expr::parse(src).map_err(|e| loc.err(format!("expression '={src}': {e}")))?;
    let v = e
        .eval(lookup)
        .map_err(|m| loc.err(format!("expression '={src}': {m}")))?;
    Ok((e, v))
}

/// Replaces every `=expr` string inside `v`. `key` is the nearest object
/// key, used to check the units of bare parameter references.
fn substitute(
    v: &mut Value,
    loc: &Loc,
    key: &str,
    lookup: &dyn Fn(&str) -> Option<PValue>,
) -> Result<()> {
    match v {
        Value::String(s) => {
            if let Some(src) = s.strip_prefix('=') {
                let (expr, value) = eval_str(src, lookup, loc)?;
                if let Some(name) = expr.as_name() {
                    check_units(key, name).map_err(|m| loc.err(m))?;
                }
                *v = value.to_json();
            }
            Ok(())
        }
        Value::Array(a) => {
            for (i, x) in a.iter_mut().enumerate() {
                let sub = Loc {
                    kind: loc.kind,
                    id: loc.id.clone(),
                    path: format!("{}[{i}]", loc.path),
                };
                substitute(x, &sub, key, lookup)?;
            }
            Ok(())
        }
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                let sub = Loc {
                    kind: loc.kind,
                    id: loc.id.clone(),
                    path: if loc.path.is_empty() {
                        k.clone()
                    } else {
                        format!("{}.{k}", loc.path)
                    },
                };
                substitute(x, &sub, k, lookup)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A key and a parameter it takes verbatim must share a unit when both
/// names carry one: `"radius_mm": "=cup_radius_cm"` is an error.
fn check_units(key: &str, param: &str) -> std::result::Result<(), String> {
    match (unit_suffix(key), unit_suffix(param)) {
        (Some((ks, kd, kf)), Some((ps, pd, pf))) if kd != pd || kf != pf => Err(format!(
            "unit mismatch: '{key}' takes {ks} but parameter '{param}' is in {ps}; convert it in the expression"
        )),
        _ => Ok(()),
    }
}

/// Expands `nodes`, `elements` or `probes`: drops disabled items, then
/// substitutes expressions in the rest.
fn expand_list(list: &str, v: &mut Value, lookup: &dyn Fn(&str) -> Option<PValue>) -> Result<()> {
    let Value::Array(items) = v else {
        return Ok(()); // Document reports the type error
    };
    let kind = match list {
        "nodes" => LocKind::Node,
        "elements" => LocKind::Element,
        _ => LocKind::Probe,
    };
    let mut kept = Vec::with_capacity(items.len());
    for (i, mut item) in std::mem::take(items).into_iter().enumerate() {
        let Value::Object(o) = &mut item else {
            kept.push(item);
            continue;
        };
        let id = o
            .get("id")
            .and_then(Value::as_str)
            .map_or_else(|| format!("{list}[{i}]"), str::to_string);
        let loc = Loc {
            kind,
            id,
            path: String::new(),
        };
        if let Some(en) = o.remove("enabled") {
            let enabled = match en {
                Value::Bool(b) => b,
                Value::String(s) if s.starts_with('=') => {
                    let src = &s[1..];
                    eval_str(src, lookup, &loc)?.1.as_bool().ok_or_else(|| {
                        loc.err(format!(
                            "'enabled' expression '={src}' must give true or false"
                        ))
                    })?
                }
                _ => return Err(loc.err("'enabled' must be true, false or an =expression".into())),
            };
            if !enabled {
                continue;
            }
        }
        for (k, x) in o.iter_mut() {
            if k == "id" {
                continue;
            }
            let sub = Loc {
                kind: loc.kind,
                id: loc.id.clone(),
                path: k.clone(),
            };
            substitute(x, &sub, k, lookup)?;
        }
        kept.push(item);
    }
    *items = kept;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(doc: Value, ov: &[(&str, PValue)]) -> Result<Resolved> {
        let p = Parametric::from_value(doc)?;
        let ov: Overrides = ov.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        p.resolve(&ov)
    }

    fn doc() -> Value {
        json!({
            "title": "=not evaluated",
            "parameters": {
                "r_mm": {"value": 2, "min": 0.5, "max": 5, "tolerance": {"rel": 0.05}},
                "n": {"value": 2, "min": 0, "max": 8, "integer": true},
                "area_mm2": {"expr": "pi * r_mm^2 * n"},
                "sealed": false,
                "ear": {"value": "a", "choices": ["a", {"value": "b", "label": "Bee"}]},
                "gap_mm": 0.1
            },
            "sweep": {"f_min_Hz": 10, "f_max_Hz": "=1000 * 20", "points_per_octave": 3},
            "nodes": [
                {"id": "x", "domain": "acoustic"},
                {"id": "y", "domain": "acoustic", "enabled": "=!sealed"}
            ],
            "elements": [
                {"id": "v", "type": "tube", "nodes": ["x", "y"], "radius_mm": "=r_mm",
                 "length_mm": 2, "count": "=n", "enabled": "=n > 0"},
                {"id": "k", "type": "slit", "gaps_mm": ["=gap_mm", "=gap_mm * 2"]},
                {"id": "t", "type": "=if(ear == 'a', 'cavity', 'tube')", "enabled": "=!sealed"}
            ]
        })
    }

    #[test]
    fn expands_values_arrays_and_enabled() {
        let r = resolve(doc(), &[]).unwrap();
        assert_eq!(r.doc["title"], "=not evaluated");
        assert_eq!(r.doc["sweep"]["f_max_Hz"], json!(20000));
        let el = r.doc["elements"].as_array().unwrap();
        assert_eq!(el.len(), 3);
        assert_eq!(el[0]["radius_mm"], json!(2));
        assert_eq!(el[0]["count"], json!(2));
        assert!(el[0].get("enabled").is_none());
        assert_eq!(el[1]["gaps_mm"], json!([0.1, 0.2]));
        assert_eq!(el[2]["type"], "cavity");
        let area = r.get("area_mm2").unwrap().as_num().unwrap();
        assert!((area - 8.0 * std::f64::consts::PI).abs() < 1e-12);

        let r = resolve(
            doc(),
            &[
                ("n", PValue::Num(0.0)),
                ("sealed", PValue::Bool(true)),
                ("ear", PValue::Str("b".into())),
            ],
        )
        .unwrap();
        let el = r.doc["elements"].as_array().unwrap();
        assert_eq!(el.len(), 1);
        assert_eq!(el[0]["id"], "k");
        assert_eq!(r.doc["nodes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn rejects_bad_overrides() {
        let bad = [
            ("r_mm", PValue::Num(9.0)),
            ("n", PValue::Num(1.5)),
            ("area_mm2", PValue::Num(1.0)),
            ("ear", PValue::Str("c".into())),
            ("sealed", PValue::Num(1.0)),
            ("nope", PValue::Num(1.0)),
        ];
        for (k, v) in bad {
            let e = resolve(doc(), &[(k, v)]).unwrap_err();
            assert!(
                matches!(e, Error::Parameter { ref name, .. } if name == k),
                "{e}"
            );
        }
    }

    #[test]
    fn reports_errors_where_they_are() {
        let mut d = doc();
        d["elements"][0]["length_mm"] = json!("=r_mm / (n - 2)");
        let e = resolve(d, &[]).unwrap_err().to_string();
        assert!(
            e.contains("element 'v'") && e.contains("length_mm") && e.contains("division by zero"),
            "{e}"
        );
        // Disabled elements are not evaluated.
        let mut d = doc();
        d["elements"][0]["length_mm"] = json!("=r_mm / n");
        assert!(resolve(d, &[("n", PValue::Num(0.0))]).is_ok());

        let mut d = doc();
        d["elements"][0]["radius_mm"] = json!("=r_cm");
        d["parameters"]["r_cm"] = json!(0.2);
        let e = resolve(d, &[]).unwrap_err().to_string();
        assert!(e.contains("unit mismatch"), "{e}");

        let mut d = doc();
        d["parameters"]["a"] = json!({"expr": "b + 1"});
        d["parameters"]["b"] = json!({"expr": "a * 2"});
        let e = resolve(d, &[]).unwrap_err().to_string();
        assert!(e.contains("circular"), "{e}");

        let mut d = doc();
        d["elements"][0]["enabled"] = json!("=n");
        assert!(resolve(d, &[])
            .unwrap_err()
            .to_string()
            .contains("true or false"));
    }

    #[test]
    fn validates_declarations() {
        let cases = [
            json!({"x": {"value": 1, "min": 2}}),
            json!({"x": {"value": 1, "tolerance": {"rel": 0.1, "abs": 1}}}),
            json!({"x": {"value": 1, "integer": true, "tolerance": {"abs": 1}}}),
            json!({"x": {"value": "a"}}),
            json!({"x": {"value": "a", "choices": ["b"]}}),
            json!({"x": {"expr": "1", "min": 0}}),
            json!({"x": {"value": 1, "colour": "red"}}),
            json!({"pi": 3}),
            json!({"2x": 3}),
            json!({"x": {"value": 1, "log": true}}),
            json!({"x": [1]}),
        ];
        for c in cases {
            assert!(
                Parametric::from_value(json!({"parameters": c.clone()})).is_err(),
                "{c}"
            );
        }
    }

    #[test]
    fn describes_for_user_interfaces() {
        let p = Parametric::from_value(doc()).unwrap();
        let d = p.describe(&Overrides::new()).unwrap();
        let params = d["parameters"].as_array().unwrap();
        let r = &params[0];
        assert_eq!(r["name"], "r_mm");
        assert_eq!(r["kind"], "number");
        assert_eq!(r["unit"], "mm");
        assert_eq!(r["tolerance"]["rel"], json!(0.05));
        assert_eq!(r["tolerance"]["dist"], "normal");
        assert_eq!(params[1]["kind"], "integer");
        assert_eq!(params[2]["kind"], "derived");
        assert_eq!(params[2]["unit"], "mm2");
        assert_eq!(params[4]["choices"][1]["label"], "Bee");
    }
}
