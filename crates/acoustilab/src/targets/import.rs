//! Minimal import of a target curve from CSV with a fixture tag (spec
//! Section 14: "target curves, CSV with fixture tag").
//!
//! This parser only reads target curves; a general curve importer (FRD,
//! REW, CrinGraph, AutoEq files) is separate, and the two can be unified
//! later.
//!
//! ```text
//! # name: My target
//! # fixture: bk5128
//! # licence: CC-BY-4.0
//! # source: where it comes from
//! frequency_Hz,dB
//! 20,0.5
//! 25,0.6
//! ...
//! ```
//!
//! * Comment lines start with `#`. `# key: value` lines set `name`,
//!   `fixture`, `label`, `family`, `licence` (or `license`), `source`,
//!   `url`, `reference_point`, `baseline`; a tag given twice is an error,
//!   and other comments are free text.
//! * A fixture tag is required, in the file or as the `fixture` argument
//!   (which then must agree with the file). A target is never used without
//!   its fixture.
//! * Data rows hold exactly two numbers, frequency in Hz and level in dB,
//!   separated by a comma, semicolon, tab or spaces. One header row of text
//!   before the first data row is skipped. Frequencies must increase
//!   strictly.

use super::curve::Curve;
use super::target::{Provenance, ProvenanceClass, Target};
use super::{Result, TargetError};
use serde_json::Value;

fn split(line: &str) -> Vec<&str> {
    let sep: &[char] = if line.contains(',') {
        &[',']
    } else if line.contains(';') {
        &[';']
    } else if line.contains('\t') {
        &['\t']
    } else {
        &[' ']
    };
    line.split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parses a fixture-tagged target CSV (see the module docs). `fixture` and
/// `name` supply or confirm the tags when the file lacks them.
pub fn import_target_csv(text: &str, fixture: Option<&str>, name: Option<&str>) -> Result<Target> {
    let csv = |line: usize, msg: String| TargetError::Csv { line, msg };
    let mut meta: Vec<(String, String)> = Vec::new();
    let (mut f, mut db) = (Vec::new(), Vec::new());
    let mut header_seen = false;
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            if let Some((k, v)) = c.split_once(':') {
                let mut k = k.trim().to_ascii_lowercase();
                if k == "license" {
                    k = "licence".into();
                }
                const KEYS: [&str; 9] = [
                    "name",
                    "fixture",
                    "label",
                    "family",
                    "licence",
                    "source",
                    "url",
                    "reference_point",
                    "baseline",
                ];
                if !KEYS.contains(&k.as_str()) {
                    // Any other comment is free text.
                    continue;
                }
                if meta.iter().any(|(m, _)| *m == k) {
                    return Err(csv(n, format!("tag '{k}' given twice")));
                }
                meta.push((k, v.trim().to_string()));
            }
            continue;
        }
        let cols = split(line);
        let nums: Vec<Option<f64>> = cols.iter().map(|c| c.parse::<f64>().ok()).collect();
        if nums.iter().any(Option::is_none) {
            if f.is_empty() && !header_seen {
                header_seen = true;
                continue;
            }
            return Err(csv(n, format!("'{line}' is not two numbers")));
        }
        if cols.len() != 2 {
            return Err(csv(
                n,
                format!(
                    "expected two columns (frequency in Hz, level in dB), found {}",
                    cols.len()
                ),
            ));
        }
        let (x, y) = (nums[0].unwrap_or_default(), nums[1].unwrap_or_default());
        if !(x.is_finite() && x > 0.0 && y.is_finite()) {
            return Err(csv(
                n,
                format!("frequency {x} or level {y} is out of range"),
            ));
        }
        if let Some(&last) = f.last() {
            if x <= last {
                return Err(csv(
                    n,
                    format!("frequency {x} Hz does not increase (after {last} Hz)"),
                ));
            }
        }
        f.push(x);
        db.push(y);
    }
    let tag = |k: &str| meta.iter().find(|(m, _)| m == k).map(|(_, v)| v.clone());
    let fixture = match (tag("fixture"), fixture) {
        (Some(a), Some(b)) if a != b => {
            return Err(csv(0, format!("the file's fixture '{a}' differs from the given fixture '{b}'")))
        }
        (Some(a), _) => a,
        (None, Some(b)) => b.to_string(),
        (None, None) => {
            return Err(csv(
                0,
                "no fixture tag: add '# fixture: <id>' or pass the fixture (a target is never used without its fixture)".into(),
            ))
        }
    };
    if fixture.trim().is_empty() {
        return Err(csv(0, "empty fixture tag".into()));
    }
    let name = tag("name")
        .or_else(|| name.map(str::to_string))
        .unwrap_or_else(|| "imported".into());
    let curve = Curve::new(f, db).map_err(|e| csv(0, e.to_string()))?;
    let (lo, hi) = curve.range();
    Ok(Target {
        label: tag("label").unwrap_or_else(|| name.clone()),
        version: String::new(),
        family: tag("family").unwrap_or_else(|| "user".into()),
        group: None,
        primary: false,
        reference_point: tag("reference_point").unwrap_or_else(|| "drp".into()),
        baseline: tag("baseline").unwrap_or_else(|| "not stated".into()),
        normalisation_hz: 500.0,
        valid_range_hz: (lo, hi),
        provenance: Provenance {
            class: ProvenanceClass::User,
            source: tag("source").unwrap_or_else(|| "user-supplied CSV".into()),
            doi: None,
            url: tag("url"),
            licence: tag("licence").unwrap_or_else(|| {
                "not stated (user-supplied; not redistributed by AcoustiLab)".into()
            }),
            retrieved: None,
            attribution: None,
        },
        flags: vec!["user_supplied".into()],
        notes: Vec::new(),
        name,
        fixture,
        curve,
        extra: Value::Null,
    })
}
