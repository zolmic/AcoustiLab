//! Frozen blind predictions: generation, manifest, verification and drift.
//!
//! [`predict`] solves every configuration of a protocol at its nominal
//! parameters and runs a Latin-hypercube Monte Carlo over the tolerances of
//! the parameters that configuration uses ([`crate::analysis::mc`]). For
//! each measurement it writes, on the protocol's exchange grid within the
//! configuration's band:
//!
//! * `<id>.csv`: the nominal curve (`frequency_Hz,level_dB,phase_deg` or
//!   `frequency_Hz,magnitude_ohm,phase_deg`, every number in shortest
//!   round-trip form), readable by [`crate::io::text::import`];
//! * `<id>.envelope.csv`: per frequency the nominal value and the Monte Carlo
//!   median, 5, 10, 90 and 95 % points, minimum and maximum (percentiles as
//!   numpy's default, docs/analysis.md), the number of runs, and the validity
//!   shading of the nominal solve (0 none, 1 light, 2 dark; docs/netlist.md);
//!   impedances add the same statistics of the phase;
//! * a sidecar next to each (`FILE.sidecar.json`, docs/fitting.md).
//!
//! It also copies the protocol and the netlist (`protocol.json`,
//! `netlist.json`), so that a frozen set is self-contained, and writes
//! `manifest.json` (`acoustilab-frozen-predictions/0.1`): the engine
//! version, the git commit and whether the tree was clean, the date, the
//! command, the SHA-256 of the netlist and protocol texts, the
//! reproducibility hash of every configuration's expanded netlist
//! ([`crate::analysis::canonical`]), the Monte Carlo plan with its
//! distributions, the nominal solve's warnings, and the SHA-256 of every
//! file. [`verify`] recomputes those hashes; a set whose files changed is
//! refused by [`Frozen::load`]. A new prediction goes into a new version
//! directory: the command line refuses to write into an existing one.

use super::{
    err, netlist_on_grid, overrides_json, sha256_hex, shade, Configuration, Files, Protocol,
    VResult, ValidationError,
};
use crate::analysis::mc::{self, PlanSpec, RunOptions};
use crate::analysis::Design;
use crate::io::curve::from_solve;
use crate::io::sidecar::Provenance;
use crate::io::text::{self, Format};
use crate::io::{Curve, Quantity};
use crate::params::{ParamKind, Parametric};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Schema tag of prediction manifests.
pub const MANIFEST_SCHEMA: &str = "acoustilab-frozen-predictions/0.1";
/// File name of the manifest in a prediction directory.
pub const MANIFEST: &str = "manifest.json";
/// Snapshot of the protocol in a prediction directory.
pub const PROTOCOL_FILE: &str = "protocol.json";
/// Snapshot of the netlist in a prediction directory.
pub const NETLIST_FILE: &str = "netlist.json";

/// Provenance of a prediction run, recorded in the manifest.
#[derive(Debug, Clone, Default)]
pub struct PredictOptions {
    /// Version directory name, e.g. `v1`.
    pub version: String,
    /// UTC date and time, ISO 8601.
    pub created: String,
    /// Commit of the engine that produced the files.
    pub git_commit: Option<String>,
    /// Whether the working tree had uncommitted changes.
    pub git_dirty: Option<bool>,
    /// The command that produced the files.
    pub command: String,
    /// Where the protocol and netlist were read from (repository paths).
    pub protocol_source: String,
    pub netlist_source: String,
    /// Monte Carlo runs instead of the protocol's (tests).
    pub runs: Option<usize>,
}

/// Monte Carlo statistics of one measurement on the prediction grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub quantity: Quantity,
    pub freqs: Vec<f64>,
    /// dB SPL for pressure, ohm for impedance.
    pub nominal: Vec<f64>,
    pub median: Vec<f64>,
    pub p5: Vec<f64>,
    pub p10: Vec<f64>,
    pub p90: Vec<f64>,
    pub p95: Vec<f64>,
    pub min: Vec<f64>,
    pub max: Vec<f64>,
    /// Phase statistics in degrees (impedance): nominal, median, p5, p95,
    /// min, max.
    pub phase: Option<[Vec<f64>; 6]>,
    /// Successful runs at each frequency.
    pub n: Vec<usize>,
    /// Validity shading of the nominal solve: 0 none, 1 light, 2 dark.
    pub shading: Vec<u8>,
}

const STATS: [&str; 8] = ["nominal", "median", "p5", "p10", "p90", "p95", "min", "max"];
const PHASE_STATS: [&str; 6] = ["nominal", "median", "p5", "p95", "min", "max"];

fn fmt_num(x: f64) -> String {
    if x.is_finite() {
        format!("{x}")
    } else {
        String::new()
    }
}

impl Envelope {
    fn unit(&self) -> &'static str {
        match self.quantity {
            Quantity::Pressure => "dB",
            _ => "ohm",
        }
    }

    fn columns(&self) -> [&Vec<f64>; 8] {
        [
            &self.nominal,
            &self.median,
            &self.p5,
            &self.p10,
            &self.p90,
            &self.p95,
            &self.min,
            &self.max,
        ]
    }

    /// The CSV table (module documentation).
    pub fn to_csv(&self) -> String {
        let u = self.unit();
        let mut s = String::from("frequency_Hz");
        for k in STATS {
            let _ = write!(s, ",{k}_{u}");
        }
        if self.phase.is_some() {
            for k in PHASE_STATS {
                let _ = write!(s, ",{k}_phase_deg");
            }
        }
        s.push_str(",runs,shading\n");
        for i in 0..self.freqs.len() {
            s.push_str(&fmt_num(self.freqs[i]));
            for c in self.columns() {
                s.push(',');
                s.push_str(&fmt_num(c[i]));
            }
            if let Some(ph) = &self.phase {
                for c in ph {
                    s.push(',');
                    s.push_str(&fmt_num(c[i]));
                }
            }
            let _ = writeln!(s, ",{},{}", self.n[i], shade(self.shading[i]));
        }
        s
    }

    /// Reads [`Envelope::to_csv`].
    pub fn from_csv(text_: &str, quantity: Quantity) -> VResult<Envelope> {
        let mut lines = text_.lines();
        let header: Vec<&str> = lines
            .next()
            .ok_or_else(|| ValidationError("envelope: empty file".into()))?
            .split(',')
            .collect();
        let col = |name: &str| -> VResult<usize> {
            header
                .iter()
                .position(|h| *h == name)
                .ok_or_else(|| ValidationError(format!("envelope: no column '{name}'")))
        };
        let u = match quantity {
            Quantity::Pressure => "dB",
            _ => "ohm",
        };
        let idx: Vec<usize> = STATS
            .iter()
            .map(|k| col(&format!("{k}_{u}")))
            .collect::<VResult<_>>()?;
        let phase_idx: Option<Vec<usize>> = if header.iter().any(|h| h.ends_with("_phase_deg")) {
            Some(
                PHASE_STATS
                    .iter()
                    .map(|k| col(&format!("{k}_phase_deg")))
                    .collect::<VResult<_>>()?,
            )
        } else {
            None
        };
        let (ci, cn, cs) = (col("frequency_Hz")?, col("runs")?, col("shading")?);
        let mut e = Envelope {
            quantity,
            freqs: Vec::new(),
            nominal: Vec::new(),
            median: Vec::new(),
            p5: Vec::new(),
            p10: Vec::new(),
            p90: Vec::new(),
            p95: Vec::new(),
            min: Vec::new(),
            max: Vec::new(),
            phase: phase_idx.as_ref().map(|_| Default::default()),
            n: Vec::new(),
            shading: Vec::new(),
        };
        for (no, line) in lines.enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split(',').collect();
            if f.len() != header.len() {
                return err(format!(
                    "envelope line {}: {} fields, expected {}",
                    no + 2,
                    f.len(),
                    header.len()
                ));
            }
            let x = |i: usize| -> VResult<f64> {
                if f[i].is_empty() {
                    return Ok(f64::NAN);
                }
                f[i].parse::<f64>().map_err(|_| {
                    ValidationError(format!(
                        "envelope line {}: '{}' is not a number",
                        no + 2,
                        f[i]
                    ))
                })
            };
            e.freqs.push(x(ci)?);
            let cols: [&mut Vec<f64>; 8] = [
                &mut e.nominal,
                &mut e.median,
                &mut e.p5,
                &mut e.p10,
                &mut e.p90,
                &mut e.p95,
                &mut e.min,
                &mut e.max,
            ];
            for (c, &i) in cols.into_iter().zip(&idx) {
                c.push(x(i)?);
            }
            if let (Some(ph), Some(pi)) = (e.phase.as_mut(), &phase_idx) {
                for (c, &i) in ph.iter_mut().zip(pi) {
                    c.push(x(i)?);
                }
            }
            e.n.push(x(cn)? as usize);
            e.shading.push(x(cs)? as u8);
        }
        Ok(e)
    }
}

/// Toleranced continuous parameters that reach the netlist of this
/// configuration with a non-zero spread, in declaration order.
fn sampled_parameters(design: &Design, used: &[String]) -> Vec<String> {
    design
        .parametric
        .defs
        .iter()
        .filter(|d| {
            let spread = d.tolerance.as_ref().is_some_and(|t| {
                design
                    .value(&d.name)
                    .and_then(|v| v.as_num())
                    .is_some_and(|v| t.half_width(v) > 0.0)
            });
            spread
                && matches!(d.kind, ParamKind::Number { integer: false, .. })
                && used.contains(&d.name)
        })
        .map(|d| d.name.clone())
        .collect()
}

/// The sidecar of a frozen prediction: the solve's (quantity, calibration,
/// drive, source impedance) plus the protocol's conditions.
fn prediction_sidecar(
    curve: &mut Curve,
    protocol: &Protocol,
    c: &Configuration,
    m: &super::Measurement,
    opts: &PredictOptions,
    what: &str,
) {
    let sc = &mut curve.sidecar;
    for (k, v) in &c.sidecar {
        let v = v.as_str().map(str::to_string);
        match k.as_str() {
            "fixture" => sc.fixture = v,
            "ear_simulator" => sc.ear_simulator = v,
            "pinna" => sc.pinna = v,
            _ => {}
        }
    }
    sc.reference_point = Some(m.reference_point.clone());
    sc.compensation = Some("none".into());
    sc.temperature_c = Some(23.0);
    sc.date = Some(opts.created.chars().take(10).collect());
    sc.device = Some(format!(
        "open reference cup, design ({})",
        opts.netlist_source
    ));
    sc.provenance = Some(Provenance {
        origin: Some(crate::io::sidecar::Origin::Simulated),
        source: Some(format!(
            "frozen blind prediction {} of '{}' (configuration '{}'), {what}; protocol '{}'",
            opts.version, m.id, c.id, protocol.title
        )),
        url: None,
        licence: protocol.licence.clone(),
        tool: Some(crate::solve::ENGINE.to_string()),
    });
    sc.notes = Some(format!(
        "{}. Overrides: {}.{}",
        c.label,
        overrides_json(&protocol.overrides_for(c)),
        c.instructions
            .as_ref()
            .map(|s| format!(" {s}"))
            .unwrap_or_default()
    ));
}

/// Generates a frozen prediction set (module documentation). `progress`
/// receives one line per configuration.
pub fn predict(
    protocol_text: &str,
    netlist_text: &str,
    opts: &PredictOptions,
    progress: &mut dyn FnMut(String),
) -> VResult<BTreeMap<String, String>> {
    let protocol = Protocol::parse(protocol_text)?;
    let grid = protocol.grid.freqs();
    protocol.check_against(&netlist_on_grid(netlist_text, &grid)?)?;
    let runs = opts.runs.unwrap_or(protocol.mc_runs);
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    let mut confs = Vec::new();
    for c in &protocol.configurations {
        let t0 = std::time::Instant::now();
        let freqs: Vec<f64> = grid
            .iter()
            .copied()
            .filter(|f| *f >= c.band.0 * (1.0 - 1e-12) && *f <= c.band.1 * (1.0 + 1e-12))
            .collect();
        let design = Design::new(
            netlist_on_grid(netlist_text, &freqs)?,
            protocol.overrides_for(c),
        )?;
        let base = design.base_point()?;
        let result = base.solve()?;
        let sampled = sampled_parameters(&design, &result.meta.parameters_used);
        let plan = mc::plan(
            &design,
            &PlanSpec::Lhs {
                n: runs,
                seed: protocol.mc_seed,
                parameters: Some(sampled.clone()),
            },
        )?;
        let probes: Vec<String> = c.measurements.iter().map(|m| m.probe.clone()).collect();
        let chunk = mc::run(
            &design,
            &plan.samples,
            &RunOptions {
                probes: Some(probes),
                metrics: Some(false),
                readouts: None,
            },
        )?;
        let env = mc::envelope(&chunk.freqs_hz, &chunk.samples);
        // The solve's grid: serde_json's float parsing may move a frequency
        // by one ulp on the way into the netlist.
        let freqs = result.freqs_hz.clone();
        if chunk.freqs_hz != freqs {
            return err(format!(
                "configuration '{}': the Monte Carlo grid differs from the nominal solve's",
                c.id
            ));
        }
        let shading: Vec<u8> = freqs.iter().map(|&f| result.shading.band(f)).collect();
        let mut measurements = Vec::new();
        for m in &c.measurements {
            let mut nominal = from_solve(&base.circuit, &result, &m.probe)?;
            let q = nominal.quantity;
            let pe = env.probes.iter().find(|p| p.id == m.probe).ok_or_else(|| {
                ValidationError(format!("no Monte Carlo curves of '{}'", m.probe))
            })?;
            let stats = match q {
                Quantity::Pressure => pe.db.as_ref(),
                _ => pe.magnitude.as_ref(),
            }
            .ok_or_else(|| {
                ValidationError(format!("no Monte Carlo statistics of '{}'", m.probe))
            })?;
            let nominal_values = match q {
                Quantity::Pressure => nominal.level_db(),
                _ => nominal.magnitude.clone(),
            };
            let phase = match (q, pe.phase_deg.as_ref(), nominal.phase_deg.as_ref()) {
                (Quantity::Impedance, Some(s), Some(p)) => Some([
                    p.clone(),
                    s.median.clone(),
                    s.p5.clone(),
                    s.p95.clone(),
                    s.min.clone(),
                    s.max.clone(),
                ]),
                _ => None,
            };
            let envelope = Envelope {
                quantity: q,
                freqs: freqs.clone(),
                nominal: nominal_values,
                median: stats.median.clone(),
                p5: stats.p5.clone(),
                p10: stats.p10.clone(),
                p90: stats.p90.clone(),
                p95: stats.p95.clone(),
                min: stats.min.clone(),
                max: stats.max.clone(),
                phase,
                n: stats.n.clone(),
                shading: shading.clone(),
            };
            let mut env_curve = nominal.clone();
            prediction_sidecar(&mut nominal, &protocol, c, m, opts, "nominal parameters");
            prediction_sidecar(
                &mut env_curve,
                &protocol,
                c,
                m,
                opts,
                &format!(
                    "Monte Carlo envelope of {runs} Latin-hypercube runs (seed {}) over the tolerances of: {}",
                    protocol.mc_seed,
                    sampled.join(", ")
                ),
            );
            let unit = envelope.unit();
            env_curve.sidecar.notes = Some(format!(
                "Columns: frequency_Hz; nominal, median, p5, p10, p90, p95, min, max in {unit}{}; runs (successful Monte Carlo runs); shading (validity of the nominal solve: 0 none, 1 light, 2 dark; docs/netlist.md). Percentiles: linear interpolation between order statistics (numpy default). {}",
                if envelope.phase.is_some() {
                    "; nominal, median, p5, p95, min, max of the phase in degrees"
                } else {
                    ""
                },
                env_curve.sidecar.notes.clone().unwrap_or_default()
            ));
            let nominal_file = format!("{}.csv", m.id);
            let envelope_file = format!("{}.envelope.csv", m.id);
            files.insert(nominal_file.clone(), text::export(&nominal, Format::Csv)?);
            files.insert(
                format!("{nominal_file}.sidecar.json"),
                nominal.sidecar.to_text(),
            );
            files.insert(envelope_file.clone(), envelope.to_csv());
            files.insert(
                format!("{envelope_file}.sidecar.json"),
                env_curve.sidecar.to_text(),
            );
            measurements.push(json!({
                "id": m.id,
                "probe": m.probe,
                "quantity": q.name(),
                "reference_point": m.reference_point,
                "nominal": nominal_file,
                "envelope": envelope_file,
            }));
        }
        let failed = chunk.samples.iter().filter(|s| !s.ok).count();
        let failures: Vec<Value> = chunk
            .samples
            .iter()
            .filter(|s| !s.ok)
            .map(|s| json!({"index": s.index, "error": s.error}))
            .collect();
        let warnings: Vec<Value> = result
            .warnings
            .iter()
            .map(|w| {
                json!({"code": w.code, "severity": w.severity, "element": w.element, "message": w.message})
            })
            .collect();
        confs.push(json!({
            "id": c.id,
            "label": c.label,
            "overrides": overrides_json(&protocol.overrides_for(c)),
            "band_Hz": [c.band.0, c.band.1],
            "points": freqs.len(),
            "netlist_hash": base.hash(),
            "drive": result.meta.drive,
            "shading": result.shading,
            "warnings": warnings,
            "monte_carlo": {
                "method": "lhs",
                "runs": runs,
                "failed": failed,
                "failures": failures,
                "seed": protocol.mc_seed,
                "parameters": sampled,
                "distributions": plan.distributions,
                "clipped": plan.clipped,
            },
            "measurements": measurements,
        }));
        progress(format!(
            "{}: {} runs ({} failed) in {:.1} s",
            c.id,
            runs,
            failed,
            t0.elapsed().as_secs_f64()
        ));
    }
    files.insert(PROTOCOL_FILE.into(), protocol_text.to_string());
    files.insert(NETLIST_FILE.into(), netlist_text.to_string());
    let listed: Vec<Value> = files
        .iter()
        .map(|(name, content)| {
            json!({
                "path": name,
                "sha256": sha256_hex(content.as_bytes()),
                "bytes": content.len(),
                "command": opts.command,
            })
        })
        .collect();
    let manifest = json!({
        "schema": MANIFEST_SCHEMA,
        "version": opts.version,
        "status": "blind: frozen before any measurement of the device existed; never edit these files, write a new version directory instead",
        "created": opts.created,
        "engine": crate::solve::ENGINE,
        "git": {"commit": opts.git_commit, "dirty": opts.git_dirty},
        "command": opts.command,
        "protocol": {"file": PROTOCOL_FILE, "source": opts.protocol_source, "sha256": sha256_hex(protocol_text.as_bytes()), "title": protocol.title},
        "netlist": {"file": NETLIST_FILE, "source": opts.netlist_source, "sha256": sha256_hex(netlist_text.as_bytes())},
        "grid": {"f_min_Hz": protocol.grid.f_min, "f_max_Hz": protocol.grid.f_max, "points_per_octave": protocol.grid.points_per_octave, "kind": "exchange grid 1 kHz * 2^(k/n) (spec Section 14)"},
        "configurations": confs,
        "files": listed,
    });
    files.insert(MANIFEST.into(), format!("{manifest:#}\n"));
    Ok(files)
}

/// Result of [`verify`].
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyReport {
    pub ok: bool,
    pub manifest_sha256: Option<String>,
    pub files_checked: usize,
    pub problems: Vec<String>,
}

/// Checks a frozen set against its manifest: every listed file is present
/// with its SHA-256 and size, and no other file is there.
pub fn verify(files: &Files) -> VerifyReport {
    let mut r = VerifyReport {
        ok: false,
        manifest_sha256: None,
        files_checked: 0,
        problems: Vec::new(),
    };
    let Some(m) = files.get(MANIFEST) else {
        r.problems.push(format!("no {MANIFEST}"));
        return r;
    };
    r.manifest_sha256 = Some(sha256_hex(m));
    let manifest: Value = match serde_json::from_slice(m) {
        Ok(v) => v,
        Err(e) => {
            r.problems.push(format!("{MANIFEST}: {e}"));
            return r;
        }
    };
    if manifest.get("schema").and_then(Value::as_str) != Some(MANIFEST_SCHEMA) {
        r.problems
            .push(format!("{MANIFEST}: schema is not '{MANIFEST_SCHEMA}'"));
    }
    let listed = manifest
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut names = std::collections::BTreeSet::new();
    for f in &listed {
        let (Some(path), Some(hash)) = (
            f.get("path").and_then(Value::as_str),
            f.get("sha256").and_then(Value::as_str),
        ) else {
            r.problems
                .push(format!("{MANIFEST}: a file entry lacks 'path' or 'sha256'"));
            continue;
        };
        names.insert(path.to_string());
        match files.get(path) {
            None => r
                .problems
                .push(format!("{path}: listed in the manifest but missing")),
            Some(content) => {
                r.files_checked += 1;
                let h = sha256_hex(content);
                if h != hash {
                    r.problems.push(format!(
                        "{path}: SHA-256 {h} differs from the manifest's {hash}: a frozen prediction was changed"
                    ));
                }
                if f.get("bytes").and_then(Value::as_u64) != Some(content.len() as u64) {
                    r.problems
                        .push(format!("{path}: size differs from the manifest"));
                }
            }
        }
    }
    for name in files.keys() {
        if name != MANIFEST && !names.contains(name) {
            r.problems.push(format!(
                "{name}: not listed in the manifest (frozen sets are never extended; write a new version)"
            ));
        }
    }
    if listed.is_empty() {
        r.problems.push(format!("{MANIFEST}: lists no files"));
    }
    r.ok = r.problems.is_empty();
    r
}

/// A verified frozen prediction set, parsed.
#[derive(Debug, Clone)]
pub struct Frozen {
    pub version: String,
    pub manifest: Value,
    pub manifest_sha256: String,
    pub protocol: Protocol,
    pub protocol_text: String,
    pub netlist_text: String,
    /// Nominal curves by measurement id.
    pub nominal: BTreeMap<String, Curve>,
    pub envelopes: BTreeMap<String, Envelope>,
}

fn utf8(files: &Files, name: &str) -> VResult<String> {
    let b = files
        .get(name)
        .ok_or_else(|| ValidationError(format!("frozen set: no '{name}'")))?;
    String::from_utf8(b.clone()).map_err(|_| ValidationError(format!("{name}: not UTF-8")))
}

impl Frozen {
    /// Verifies and parses a frozen set; a set that fails [`verify`] is
    /// refused.
    pub fn load(files: &Files) -> VResult<Frozen> {
        let v = verify(files);
        if !v.ok {
            return err(format!(
                "the frozen predictions do not match their manifest: {}",
                v.problems.join("; ")
            ));
        }
        let manifest: Value = serde_json::from_slice(&files[MANIFEST])
            .map_err(|e| ValidationError(format!("{MANIFEST}: {e}")))?;
        let protocol_text = utf8(files, PROTOCOL_FILE)?;
        let protocol = Protocol::parse(&protocol_text)?;
        let mut nominal = BTreeMap::new();
        let mut envelopes = BTreeMap::new();
        for c in manifest
            .get("configurations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for m in c
                .get("measurements")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let s = |k: &str| {
                    m.get(k)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| {
                            ValidationError(format!("{MANIFEST}: measurement without '{k}'"))
                        })
                };
                let id = s("id")?;
                let q = Quantity::parse(&s("quantity")?).ok_or_else(|| {
                    ValidationError(format!("{MANIFEST}: unknown quantity of '{id}'"))
                })?;
                let nf = s("nominal")?;
                let mut curve = text::import(&utf8(files, &nf)?, Format::Csv, Some(q))?;
                curve.sidecar =
                    crate::io::Sidecar::parse(&utf8(files, &format!("{nf}.sidecar.json"))?)?;
                nominal.insert(id.clone(), curve);
                envelopes.insert(
                    id.clone(),
                    Envelope::from_csv(&utf8(files, &s("envelope")?)?, q)?,
                );
            }
        }
        for (_, m) in protocol.measurements() {
            if !nominal.contains_key(&m.id) {
                return err(format!(
                    "frozen set: the protocol measures '{}' but the manifest has no prediction of it",
                    m.id
                ));
            }
        }
        Ok(Frozen {
            version: manifest
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            manifest_sha256: v.manifest_sha256.unwrap_or_default(),
            manifest,
            protocol,
            protocol_text,
            netlist_text: utf8(files, NETLIST_FILE)?,
            nominal,
            envelopes,
        })
    }

    /// Frozen engine version.
    pub fn engine(&self) -> &str {
        self.manifest
            .get("engine")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// The configuration of a measurement.
    pub fn configuration_of(&self, measurement: &str) -> Option<&Configuration> {
        self.protocol.measurement(measurement).map(|(c, _)| c)
    }

    /// The frozen netlist on the prediction grid of a configuration.
    pub fn parametric(&self, c: &Configuration) -> VResult<Parametric> {
        let freqs: Vec<f64> = self
            .protocol
            .grid
            .freqs()
            .into_iter()
            .filter(|f| *f >= c.band.0 * (1.0 - 1e-12) && *f <= c.band.1 * (1.0 + 1e-12))
            .collect();
        netlist_on_grid(&self.netlist_text, &freqs)
    }
}

/// How far a re-solve moved from one frozen nominal curve.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftEntry {
    pub measurement: String,
    pub quantity: Quantity,
    /// Largest |Δ level| in dB (impedance: 20·log10 of the ratio) and where.
    pub max_abs_db: f64,
    pub at_hz: f64,
    /// The same over the frozen credible band (no shading).
    pub max_abs_db_credible: f64,
    /// Largest |Δ phase| in degrees (impedance).
    pub max_abs_phase_deg: Option<f64>,
}

/// Drift of the current engine from a frozen set: the frozen netlist
/// re-solved now (`engine`), and, if given, the current netlist
/// (`model`). Never fails on the size of the drift.
#[derive(Debug, Clone, PartialEq)]
pub struct Drift {
    pub frozen_engine: String,
    pub engine_now: String,
    pub engine: Vec<DriftEntry>,
    pub model: Option<Vec<DriftEntry>>,
}

fn drift_of(frozen: &Frozen, netlist_text: &str) -> VResult<Vec<DriftEntry>> {
    let mut out = Vec::new();
    for c in &frozen.protocol.configurations {
        let freqs = &frozen.envelopes[&c.measurements[0].id].freqs;
        let p = netlist_on_grid(netlist_text, freqs)?;
        let circuit = crate::Circuit::from_parametric(&p, &frozen.protocol.overrides_for(c))?;
        let r = circuit.solve()?;
        for m in &c.measurements {
            let now = from_solve(&circuit, &r, &m.probe)?;
            let then = &frozen.nominal[&m.id];
            let env = &frozen.envelopes[&m.id];
            if now.freqs_hz.len() != then.freqs_hz.len() {
                return err(format!("drift: the grid of '{}' changed", m.id));
            }
            let (ln, lt) = (now.level_db(), then.level_db());
            let mut e = DriftEntry {
                measurement: m.id.clone(),
                quantity: then.quantity,
                max_abs_db: 0.0,
                at_hz: f64::NAN,
                max_abs_db_credible: 0.0,
                max_abs_phase_deg: None,
            };
            for i in 0..lt.len() {
                let d = (ln[i] - lt[i]).abs();
                if d > e.max_abs_db || e.at_hz.is_nan() {
                    e.max_abs_db = d;
                    e.at_hz = then.freqs_hz[i];
                }
                if env.shading[i] == 0 {
                    e.max_abs_db_credible = e.max_abs_db_credible.max(d);
                }
            }
            if then.quantity == Quantity::Impedance {
                if let (Some(a), Some(b)) = (&now.phase_deg, &then.phase_deg) {
                    e.max_abs_phase_deg = Some(
                        a.iter()
                            .zip(b)
                            .map(|(x, y)| crate::io::curve::wrap_deg(x - y).abs())
                            .fold(0.0, f64::max),
                    );
                }
            }
            out.push(e);
        }
    }
    Ok(out)
}

/// Drift report (see [`Drift`]).
pub fn drift(frozen: &Frozen, current_netlist: Option<&str>) -> VResult<Drift> {
    Ok(Drift {
        frozen_engine: frozen.engine().to_string(),
        engine_now: crate::solve::ENGINE.to_string(),
        engine: drift_of(frozen, &frozen.netlist_text)?,
        model: current_netlist.map(|t| drift_of(frozen, t)).transpose()?,
    })
}

impl Drift {
    /// Plain-text table.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Drift from the frozen predictions (engine then: {}, now: {}). Informational: a blind prediction is never updated.\n",
            self.frozen_engine, self.engine_now
        );
        let table = |s: &mut String, title: &str, es: &[DriftEntry]| {
            let _ = writeln!(s, "\n{title}");
            let _ = writeln!(
                s,
                "  {:<18} {:>12} {:>10} {:>16} {:>14}",
                "measurement", "max |dB|", "at Hz", "credible |dB|", "phase |deg|"
            );
            for e in es {
                let _ = writeln!(
                    s,
                    "  {:<18} {:>12.3e} {:>10.0} {:>16.3e} {:>14}",
                    e.measurement,
                    e.max_abs_db,
                    e.at_hz,
                    e.max_abs_db_credible,
                    e.max_abs_phase_deg
                        .map_or("-".to_string(), |x| format!("{x:.3e}"))
                );
            }
        };
        table(
            &mut s,
            "Engine drift (frozen netlist, current engine):",
            &self.engine,
        );
        if let Some(m) = &self.model {
            table(&mut s, "Model drift (current netlist, current engine):", m);
        }
        s
    }
}
