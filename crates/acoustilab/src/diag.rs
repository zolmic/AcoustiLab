//! Warnings attached to a solve (docs/netlist.md, "Warnings").
//!
//! Two sources:
//!
//! * **Notes**: static remarks an element makes about its own data, such as
//!   estimated values or a literature model not verified against its
//!   standard ([`crate::elements::Element::notes`]).
//! * **Operating limits**: quantities checked at every frequency on the
//!   solution at the stated drive, such as particle velocity in a vent
//!   against the laminar-flow assumption, or diaphragm excursion against
//!   the driver's Xmax ([`crate::elements::Element::operating`]). A limit
//!   exceeded anywhere in the sweep becomes one warning with its frequency
//!   range and worst value.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Worth knowing; results stand.
    Info,
    /// A model assumption or a component limit is violated.
    Warning,
}

/// A static remark an element makes about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
}

/// One quantity checked against a limit at one frequency.
#[derive(Debug, Clone, PartialEq)]
pub struct Operating {
    /// Stable identifier, e.g. `particle_velocity`, `excursion`.
    pub code: &'static str,
    /// What was measured, e.g. "RMS particle velocity in the hole".
    pub what: String,
    pub value: f64,
    pub limit: f64,
    pub unit: &'static str,
    /// Why the limit matters, e.g. "laminar-flow assumption".
    pub reason: &'static str,
}

/// A warning in a solve result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Warning {
    pub code: String,
    pub severity: Severity,
    pub element: Option<String>,
    pub message: String,
    /// Range of frequencies where an operating limit is exceeded.
    #[serde(rename = "f_min_Hz")]
    pub f_min_hz: Option<f64>,
    #[serde(rename = "f_max_Hz")]
    pub f_max_hz: Option<f64>,
    /// Worst value and where it occurs.
    pub value: Option<f64>,
    #[serde(rename = "at_Hz")]
    pub at_hz: Option<f64>,
    pub limit: Option<f64>,
    pub unit: Option<String>,
}

impl Warning {
    pub fn from_note(element: &str, n: &Note) -> Warning {
        Warning {
            code: n.code.to_string(),
            severity: n.severity,
            element: Some(element.to_string()),
            message: n.message.clone(),
            f_min_hz: None,
            f_max_hz: None,
            value: None,
            at_hz: None,
            limit: None,
            unit: None,
        }
    }
}

struct Exceedance {
    element: String,
    code: &'static str,
    what: String,
    reason: &'static str,
    unit: &'static str,
    limit: f64,
    f_min: f64,
    f_max: f64,
    worst: f64,
    at: f64,
}

/// Accumulates operating checks over a sweep.
#[derive(Default)]
pub struct Collector {
    items: Vec<Exceedance>,
}

impl Collector {
    /// Records the checks of `element` at frequency `f`; only values above
    /// their limit are kept.
    pub fn add(&mut self, element: &str, f: f64, checks: &[Operating]) {
        for c in checks.iter().filter(|c| c.value > c.limit) {
            match self
                .items
                .iter_mut()
                .find(|e| e.element == element && e.code == c.code && e.what == c.what)
            {
                Some(e) => {
                    e.f_min = e.f_min.min(f);
                    e.f_max = e.f_max.max(f);
                    if c.value > e.worst {
                        e.worst = c.value;
                        e.at = f;
                    }
                }
                None => self.items.push(Exceedance {
                    element: element.to_string(),
                    code: c.code,
                    what: c.what.clone(),
                    reason: c.reason,
                    unit: c.unit,
                    limit: c.limit,
                    f_min: f,
                    f_max: f,
                    worst: c.value,
                    at: f,
                }),
            }
        }
    }

    pub fn into_warnings(self) -> Vec<Warning> {
        self.items
            .into_iter()
            .map(|e| Warning {
                message: format!(
                    "{} reaches {} {} at {} Hz, above the {} {} limit ({}), between {} and {} Hz at the stated drive",
                    e.what,
                    sig(e.worst),
                    e.unit,
                    sig(e.at),
                    sig(e.limit),
                    e.unit,
                    e.reason,
                    sig(e.f_min),
                    sig(e.f_max)
                ),
                code: e.code.to_string(),
                severity: Severity::Warning,
                element: Some(e.element),
                f_min_hz: Some(e.f_min),
                f_max_hz: Some(e.f_max),
                value: Some(e.worst),
                at_hz: Some(e.at),
                limit: Some(e.limit),
                unit: Some(e.unit.to_string()),
            })
            .collect()
    }
}

/// Three significant digits.
fn sig(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let d = 2 - x.abs().log10().floor() as i32;
    if d > 0 {
        format!("{:.*}", d as usize, x)
    } else {
        let p = 10f64.powi(-d);
        format!("{}", (x / p).round() * p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_ranges_and_worst_values() {
        let mut c = Collector::default();
        let op = |v: f64| Operating {
            code: "particle_velocity",
            what: "RMS particle velocity".into(),
            value: v,
            limit: 1.0,
            unit: "m/s",
            reason: "laminar-flow assumption",
        };
        c.add("vent", 50.0, &[op(0.5)]);
        c.add("vent", 60.0, &[op(1.5)]);
        c.add("vent", 70.0, &[op(2.5)]);
        c.add("vent", 80.0, &[op(1.2)]);
        c.add("leak", 80.0, &[op(0.2)]);
        let w = c.into_warnings();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].f_min_hz, Some(60.0));
        assert_eq!(w[0].f_max_hz, Some(80.0));
        assert_eq!(w[0].value, Some(2.5));
        assert_eq!(w[0].at_hz, Some(70.0));
        assert!(
            w[0].message.contains("2.50 m/s at 70.0 Hz"),
            "{}",
            w[0].message
        );
        assert_eq!(sig(1234.5), "1230");
        assert_eq!(sig(0.012345), "0.0123");
    }
}
