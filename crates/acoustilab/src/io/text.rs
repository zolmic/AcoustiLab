//! Text curve formats: FRD, ZMA, REW text exports and generic CSV/TXT.
//!
//! One reader serves every format; the format only supplies defaults:
//!
//! | format | columns | default quantity |
//! |---|---|---|
//! | FRD | frequency (Hz), level (dB), *phase (deg)* | pressure (dB SPL re 20 µPa) |
//! | ZMA | frequency (Hz), impedance (ohm), *phase (deg)* | impedance |
//! | REW | as FRD or ZMA, after `*` comment lines ending in a column header such as `* Freq(Hz) SPL(dB) Phase(degrees)` or `* Freq(Hz) Z(Ohms) Phase(degrees)` | from the header |
//! | CSV/TXT | named by a header row where there is one | from the header's units, else the caller's quantity |
//!
//! Reading rules:
//!
//! * Lines starting with `*`, `#`, `;`, `%`, `'`, `!` or `//` are comments,
//!   blank lines are skipped, and a UTF-8 byte-order mark is ignored. Lines
//!   end at LF, CR LF or a lone CR.
//! * A data line begins with a number. As in REW, other lines are text and
//!   are ignored, and anything after the last number of a data line is a
//!   comment. Every data line must have the same count of numbers.
//! * The delimiter is sniffed from the first data line: tab, else
//!   semicolon, else comma, else white space. A comma inside
//!   white-space-separated numbers (`20,5 65,1`) is a decimal comma; with
//!   tab or semicolon delimiters a decimal comma is recognised when a
//!   number contains a comma and no point. A file may not mix the two, and
//!   `1.234,5` (digit grouping) is rejected as ambiguous.
//! * The column header is the last text or comment line before the data
//!   whose first field names the frequency (`Freq(Hz)`, `frequency_Hz`,
//!   `Frequency [kHz]`, `f`), or that names it in another field and has one
//!   field per data column and no numbers. Units in parentheses, brackets
//!   or a `_unit`
//!   suffix set the scale: Hz or kHz; dB, ohm, Pa, m, mm, µm, m/s, mm/s;
//!   degrees or radians. Columns named `re`/`real` and `im`/`imag` give a
//!   complex value. Without a header the columns are frequency, magnitude
//!   and phase.
//! * Frequencies are sorted; an exact repeat of a point is dropped and a
//!   repeated frequency with different values is an error naming both
//!   lines. Errors name the line.
//! * In a REW export (recognised by its first header line, whatever the
//!   extension) a phase column of zeros means "no phase": REW writes 0.0
//!   when a measurement has none.
//!
//! Writing: FRD, ZMA and REW text are written as tab-separated columns with
//! six decimals (the precision REW and most crossover tools use), after a
//! `*` header; CSV is written with every number in its shortest exact form,
//! so a CSV plus its sidecar is a lossless archive.

use super::curve::{order_points, Curve, Quantity, RawPoint};
use super::sidecar::{Provenance, Sidecar, Smoothing};
use super::CurveError;
use crate::C64;

/// A curve file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Detect: a REW header if present, else generic text.
    Auto,
    Frd,
    Zma,
    Rew,
    Csv,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Auto => "auto",
            Format::Frd => "frd",
            Format::Zma => "zma",
            Format::Rew => "rew",
            Format::Csv => "csv",
        }
    }

    /// `auto`, `frd`, `zma`, `rew` (or `txt`), `csv`.
    pub fn parse(s: &str) -> Option<Format> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Some(Format::Auto),
            "frd" => Some(Format::Frd),
            "zma" => Some(Format::Zma),
            "rew" | "txt" => Some(Format::Rew),
            "csv" => Some(Format::Csv),
            _ => None,
        }
    }

    /// From a file name's extension: `.frd`, `.zma`, `.csv`, and `.txt` or
    /// `.dat` for the REW text convention.
    pub fn from_path(path: &str) -> Option<Format> {
        let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
        match ext.as_str() {
            "frd" => Some(Format::Frd),
            "zma" => Some(Format::Zma),
            "csv" => Some(Format::Csv),
            "txt" | "dat" => Some(Format::Rew),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Delim {
    Whitespace,
    Tab,
    Semicolon,
    Comma,
}

fn is_comment(t: &str) -> Option<&str> {
    for m in ["//", "*", "#", ";", "%", "'", "!"] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some(rest.trim());
        }
    }
    None
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s)
        .trim()
}

/// Splits a line into fields (surrounding double quotes removed).
fn fields(t: &str, d: Delim) -> Vec<&str> {
    match d {
        Delim::Whitespace => t.split_whitespace().map(unquote).collect(),
        Delim::Tab => t
            .split('\t')
            .map(unquote)
            .filter(|s| !s.is_empty())
            .collect(),
        Delim::Semicolon | Delim::Comma => {
            let c = if d == Delim::Comma { ',' } else { ';' };
            let mut v: Vec<&str> = t.split(c).map(unquote).collect();
            while v.last().is_some_and(|s| s.is_empty()) {
                v.pop();
            }
            v
        }
    }
}

/// Does the line begin with a number (REW's test for a data line)?
fn starts_with_number(t: &str) -> bool {
    let head: String = t
        .trim_start_matches('"')
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ',' && *c != ';' && *c != '"')
        .collect();
    head.parse::<f64>().is_ok_and(f64::is_finite)
}

fn decimal_comma_number(tok: &str) -> bool {
    // "20," in "20, 65.1" is a number followed by a comma delimiter.
    tok.contains(',')
        && !tok.ends_with(',')
        && !tok.starts_with(',')
        && !tok.contains('.')
        && tok.matches(',').count() == 1
        && tok.replace(',', ".").parse::<f64>().is_ok()
}

fn sniff(t: &str) -> (Delim, Option<bool>) {
    if t.contains('\t') {
        (Delim::Tab, None)
    } else if t.contains(';') {
        (Delim::Semicolon, None)
    } else if t.contains(',') {
        let ws: Vec<&str> = t.split_whitespace().collect();
        let numeric: Vec<&&str> = ws
            .iter()
            .take_while(|x| x.replace(',', ".").parse::<f64>().is_ok())
            .collect();
        if numeric.len() >= 2
            && numeric.iter().any(|x| decimal_comma_number(x))
            && numeric
                .iter()
                .all(|x| decimal_comma_number(x) || !x.contains(','))
        {
            (Delim::Whitespace, Some(true))
        } else {
            (Delim::Comma, Some(false))
        }
    } else {
        (Delim::Whitespace, None)
    }
}

/// Parses one field as a number under the file's decimal convention;
/// `Ok(None)` for a non-numeric field.
fn number(
    tok: &str,
    decimal_comma: &mut Option<bool>,
    line: usize,
) -> Result<Option<f64>, CurveError> {
    let has_comma = tok.contains(',');
    let has_point = tok.contains('.');
    if has_comma && has_point {
        return if tok.replace([',', '.'], "").parse::<f64>().is_ok() {
            Err(CurveError::at(
                line,
                format!("'{tok}' is ambiguous: digit grouping is not supported"),
            ))
        } else {
            Ok(None)
        };
    }
    let text = if has_comma {
        tok.replace(',', ".")
    } else {
        tok.to_string()
    };
    let Ok(x) = text.parse::<f64>() else {
        return Ok(None);
    };
    if !x.is_finite() {
        return Err(CurveError::at(
            line,
            format!("'{tok}' is not a finite number"),
        ));
    }
    if has_comma || has_point {
        match decimal_comma {
            None => *decimal_comma = Some(has_comma),
            Some(c) if *c != has_comma => {
                return Err(CurveError::at(
                    line,
                    "the file mixes decimal commas and decimal points",
                ))
            }
            _ => {}
        }
    }
    Ok(Some(x))
}

/// Role of a column named in a header.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Role {
    Freq { scale: f64 },
    Mag(MagUnit),
    Phase { radians: bool },
    Re(MagUnit),
    Im(MagUnit),
    Other,
}

/// Unit of a magnitude column.
#[derive(Debug, Clone, Copy, PartialEq)]
enum MagUnit {
    Db,
    /// Linear, with the factor to SI and the quantity the unit implies.
    Linear(f64, Option<Quantity>),
    /// No unit given.
    Unknown,
}

fn parse_unit(u: &str) -> Option<Role> {
    let u = u
        .trim()
        .to_ascii_lowercase()
        .replace('µ', "u")
        .replace(' ', "");
    let lin = |f: f64, q: Quantity| Role::Mag(MagUnit::Linear(f, Some(q)));
    Some(match u.as_str() {
        "hz" => Role::Freq { scale: 1.0 },
        "khz" => Role::Freq { scale: 1e3 },
        "db" | "dbspl" | "db_spl" | "dbre20upa" => Role::Mag(MagUnit::Db),
        "ohm" | "ohms" | "Ω" | "ω" => lin(1.0, Quantity::Impedance),
        "pa" => lin(1.0, Quantity::Pressure),
        "mpa" => lin(1e-3, Quantity::Pressure),
        "m" => lin(1.0, Quantity::Displacement),
        "mm" => lin(1e-3, Quantity::Displacement),
        "um" => lin(1e-6, Quantity::Displacement),
        "m/s" | "m_per_s" => lin(1.0, Quantity::Velocity),
        "mm/s" | "mm_per_s" => lin(1e-3, Quantity::Velocity),
        "deg" | "degree" | "degrees" | "°" => Role::Phase { radians: false },
        "rad" | "radian" | "radians" => Role::Phase { radians: true },
        _ => return None,
    })
}

/// Splits `Name (unit)`, `Name[unit]` or `name_unit` into a lower-case
/// name and a unit role.
fn header_field(tok: &str) -> (String, Option<Role>) {
    let t = tok.trim().trim_matches('"').trim();
    for (open, close) in [('(', ')'), ('[', ']')] {
        if let (Some(a), Some(b)) = (t.find(open), t.rfind(close)) {
            if a < b {
                let name = t[..a].trim().to_ascii_lowercase();
                return (name, parse_unit(&t[a + 1..b]));
            }
        }
    }
    let lower = t.to_ascii_lowercase();
    for suffix in ["_m_per_s", "_mm_per_s"] {
        if let Some(n) = lower.strip_suffix(suffix) {
            return (n.to_string(), parse_unit(&suffix[1..]));
        }
    }
    if let Some((n, u)) = t.rsplit_once('_') {
        if let Some(r) = parse_unit(u) {
            return (n.to_ascii_lowercase(), Some(r));
        }
    }
    (lower, None)
}

fn role_of(tok: &str) -> Role {
    let (name, unit) = header_field(tok);
    let name = name.as_str();
    let mag_unit = match unit {
        Some(Role::Mag(u)) => u,
        _ => MagUnit::Unknown,
    };
    if name.starts_with("freq") || name == "f" || name == "hz" {
        return match unit {
            Some(r @ Role::Freq { .. }) => r,
            _ => Role::Freq { scale: 1.0 },
        };
    }
    if name.starts_with("phase") || ["ph", "angle", "arg", "phi"].contains(&name) {
        return match unit {
            Some(r @ Role::Phase { .. }) => r,
            _ => Role::Phase { radians: false },
        };
    }
    if ["re", "real", "re(z)", "resistance", "real part"].contains(&name) {
        return Role::Re(mag_unit);
    }
    if [
        "im",
        "imag",
        "imaginary",
        "im(z)",
        "reactance",
        "imaginary part",
    ]
    .contains(&name)
    {
        return Role::Im(mag_unit);
    }
    const MAG: [&str; 17] = [
        "spl",
        "db",
        "level",
        "magnitude",
        "mag",
        "amplitude",
        "raw",
        "response",
        "gain",
        "z",
        "|z|",
        "impedance",
        "imp",
        "modulus",
        "value",
        "displacement",
        "velocity",
    ];
    if MAG.contains(&name) {
        let u = match (mag_unit, name) {
            (MagUnit::Unknown, "spl" | "db" | "raw") => MagUnit::Db,
            (MagUnit::Unknown, "z" | "|z|" | "impedance" | "imp") => {
                MagUnit::Linear(1.0, Some(Quantity::Impedance))
            }
            (u, _) => u,
        };
        return Role::Mag(u);
    }
    match unit {
        Some(r @ Role::Mag(_)) => r,
        // A header row of bare units under the names (`Hz;dB;deg`).
        _ => match parse_unit(name) {
            Some(r @ (Role::Mag(_) | Role::Phase { .. })) => r,
            _ => Role::Other,
        },
    }
}

/// Metadata of a REW text export header.
#[derive(Default)]
struct RewHeader {
    tool: Option<String>,
    date: Option<String>,
    smoothing: Option<Smoothing>,
    notes: Vec<String>,
}

fn rew_header(comments: &[String]) -> Option<RewHeader> {
    let first = comments.first()?;
    let is_rew = first.contains("by REW") || first.contains("(REW text convention)");
    if !is_rew {
        return None;
    }
    let mut h = RewHeader {
        tool: first.rsplit_once(" by ").map(|(_, t)| t.trim().to_string()),
        ..RewHeader::default()
    };
    for c in &comments[1..] {
        let Some((k, v)) = c.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "Dated" => h.date = Some(v.to_string()),
            "Smoothing" => h.smoothing = Smoothing::parse(v),
            "Measurement" | "Note" => h.notes.push(format!("{}: {v}", k.trim())),
            _ => {}
        }
    }
    Some(h)
}

/// Reads a curve. `quantity` states what the file holds when the format
/// and header cannot (a CSV without units); it must agree with them
/// otherwise.
pub fn import(text: &str, format: Format, quantity: Option<Quantity>) -> Result<Curve, CurveError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // CR LF (Windows) and a lone CR (classic Mac, still written by some
    // spreadsheet exports) end a line as LF does.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .collect();
    let first_data = lines
        .iter()
        .position(|(_, t)| is_comment(t).is_none() && starts_with_number(t))
        .ok_or_else(|| {
            CurveError::new("no data: no line begins with a number (see docs/fitting.md, formats)")
        })?;
    let (delim, mut decimal_comma) = sniff(lines[first_data].1);
    // Comments and text before the data; the header is chosen from them
    // once the data's column count is known.
    let mut comments = Vec::new();
    let mut candidates: Vec<(usize, Vec<String>)> = Vec::new();
    for &(no, t) in &lines[..first_data] {
        if t.is_empty() {
            continue;
        }
        let body = is_comment(t).unwrap_or(t);
        comments.push(body.to_string());
        let mut toks: Vec<String> = Vec::new();
        for f in fields(body, delim) {
            match toks.last_mut() {
                Some(last)
                    if delim == Delim::Whitespace && (f.starts_with('(') || f.starts_with('[')) =>
                {
                    last.push_str(f);
                }
                _ => toks.push(f.to_string()),
            }
        }
        if toks.len() >= 2 {
            candidates.push((no, toks));
        }
    }
    // Data lines.
    struct Row {
        line: usize,
        nums: Vec<f64>,
    }
    let mut rows: Vec<Row> = Vec::new();
    for &(no, t) in &lines[first_data..] {
        if t.is_empty() {
            continue;
        }
        if let Some(c) = is_comment(t) {
            comments.push(c.to_string());
            continue;
        }
        if !starts_with_number(t) {
            comments.push(t.to_string());
            continue;
        }
        let mut nums = Vec::new();
        for f in fields(t, delim) {
            match number(f, &mut decimal_comma, no)? {
                Some(x) => nums.push(x),
                None => break,
            }
        }
        if nums.len() < 2 {
            return Err(CurveError::at(
                no,
                format!("expected a frequency and at least one value, got '{t}'"),
            ));
        }
        if let Some(r) = rows.first() {
            if r.nums.len() != nums.len() {
                return Err(CurveError::at(
                    no,
                    format!(
                        "{} numbers, but line {} has {}",
                        nums.len(),
                        r.line,
                        r.nums.len()
                    ),
                ));
            }
        }
        rows.push(Row { line: no, nums });
    }
    let ncol = rows[0].nums.len();
    // The header is the last text line before the data whose first field
    // names the frequency, or which names it in another field and has one
    // field per data column and no numbers (`SPL (dB),Frequency (Hz)`).
    let is_freq = |t: &String| matches!(role_of(t), Role::Freq { .. });
    let header: Option<(usize, Vec<Role>)> = candidates
        .iter()
        .rev()
        .find(|(_, toks)| {
            is_freq(&toks[0])
                || (toks.len() == ncol
                    && toks.iter().any(is_freq)
                    && !toks.iter().any(|t| unquote(t).parse::<f64>().is_ok()))
        })
        .map(|(no, toks)| (*no, toks.iter().map(|x| role_of(x)).collect()));
    // Column roles.
    let roles: Vec<Role> = match &header {
        Some((_, r)) => (0..ncol)
            .map(|i| r.get(i).copied().unwrap_or(Role::Other))
            .collect(),
        None => {
            if ncol > 3 {
                return Err(CurveError::at(
                    rows[0].line,
                    format!("{ncol} columns and no header: name the columns (frequency, magnitude, phase)"),
                ));
            }
            let mut r = vec![Role::Freq { scale: 1.0 }, Role::Mag(MagUnit::Unknown)];
            if ncol == 3 {
                r.push(Role::Phase { radians: false });
            }
            r
        }
    };
    let find = |pred: &dyn Fn(&Role) -> bool| roles.iter().position(pred);
    let fcol = find(&|r| matches!(r, Role::Freq { .. })).unwrap_or(0);
    let fscale = match roles[fcol] {
        Role::Freq { scale } => scale,
        _ => 1.0,
    };
    let mcol = find(&|r| matches!(r, Role::Mag(_)));
    let (recol, imcol) = (
        find(&|r| matches!(r, Role::Re(_))),
        find(&|r| matches!(r, Role::Im(_))),
    );
    let pcol = find(&|r| matches!(r, Role::Phase { .. }));
    let header_line = header.as_ref().map(|h| h.0);
    let complex = mcol.is_none() && recol.is_some() && imcol.is_some();
    let mcol = match (mcol, complex) {
        (Some(c), _) => Some(c),
        (None, true) => None,
        (None, false) if ncol >= 2 && fcol == 0 && !matches!(roles[1], Role::Phase { .. }) => {
            Some(1)
        }
        _ => {
            return Err(CurveError::new(format!(
                "no magnitude column found in the header (line {})",
                header_line.unwrap_or(0)
            )))
        }
    };
    let unit = match (mcol, recol) {
        (Some(c), _) => match roles[c] {
            Role::Mag(u) => u,
            _ => MagUnit::Unknown,
        },
        (None, Some(c)) => match roles[c] {
            Role::Re(u) => u,
            _ => MagUnit::Unknown,
        },
        _ => MagUnit::Unknown,
    };
    // Quantity and unit: the caller's quantity, the format, the header.
    let unit_q = match unit {
        MagUnit::Linear(_, q) => q,
        _ => None,
    };
    let format_q = match format {
        Format::Frd => Some(Quantity::Pressure),
        Format::Zma => Some(Quantity::Impedance),
        _ => None,
    };
    let q = quantity.or(unit_q).or(format_q);
    let unit = match unit {
        MagUnit::Unknown => match (format, q) {
            (Format::Frd, _) => MagUnit::Db,
            (Format::Zma, _) => MagUnit::Linear(1.0, Some(Quantity::Impedance)),
            (_, Some(Quantity::Pressure)) => MagUnit::Db,
            (_, Some(q)) => MagUnit::Linear(1.0, Some(q)),
            (_, None) => {
                return Err(CurveError::new(
                    "cannot tell the magnitude unit: give a header with units, the format (frd: dB, zma: ohm) or the quantity",
                ))
            }
        },
        u => u,
    };
    let q = match (q, unit) {
        (Some(q), _) => q,
        (None, MagUnit::Db) => Quantity::Pressure,
        (None, MagUnit::Linear(_, Some(q))) => q,
        (None, _) => Quantity::Generic,
    };
    if let MagUnit::Linear(_, Some(uq)) = unit {
        if uq != q && q != Quantity::Generic {
            return Err(CurveError::new(format!(
                "the magnitude column is in {} units but the curve is to be {}",
                uq.name(),
                q.name()
            )));
        }
    }
    if unit == MagUnit::Db && q == Quantity::Impedance {
        return Err(CurveError::new(
            "the magnitude column is in dB but the curve is an impedance (ZMA holds ohm)",
        ));
    }
    let rew = rew_header(&comments);
    let phase_radians = pcol.is_some_and(|c| matches!(roles[c], Role::Phase { radians: true }));
    let mut pts = Vec::with_capacity(rows.len());
    for r in &rows {
        let f = r.nums[fcol] * fscale;
        let (mag, phase) = if complex {
            let s = match unit {
                MagUnit::Linear(s, _) => s,
                _ => 1.0,
            };
            let z = C64::new(r.nums[recol.unwrap()], r.nums[imcol.unwrap()]) * s;
            (z.norm(), Some(z.arg().to_degrees()))
        } else {
            let v = r.nums[mcol.unwrap()];
            let mag = match unit {
                MagUnit::Db => q.db_reference() * 10f64.powf(v / 20.0),
                MagUnit::Linear(s, _) => v * s,
                MagUnit::Unknown => v,
            };
            let ph = pcol.map(|c| {
                if phase_radians {
                    r.nums[c].to_degrees()
                } else {
                    r.nums[c]
                }
            });
            (mag, ph)
        };
        pts.push(RawPoint {
            f,
            mag,
            phase,
            origin: r.line,
        });
    }
    if rew.is_some() && pts.iter().all(|p| p.phase == Some(0.0)) {
        for p in &mut pts {
            p.phase = None;
        }
    }
    let (freqs_hz, magnitude, phase_deg) = order_points(pts, true)?;
    let mut sidecar = Sidecar::for_quantity(q);
    if let Some(h) = rew {
        sidecar.date = h.date;
        sidecar.smoothing = h.smoothing;
        if !h.notes.is_empty() {
            sidecar.notes = Some(h.notes.join("; "));
        }
        sidecar.provenance = Some(Provenance {
            origin: None,
            source: None,
            url: None,
            licence: None,
            tool: h.tool,
        });
    }
    Ok(Curve {
        quantity: q,
        freqs_hz,
        magnitude,
        phase_deg,
        sidecar,
        comments,
    })
}

fn level_label(q: Quantity) -> &'static str {
    match q {
        Quantity::Pressure => "level dB SPL re 20 uPa",
        Quantity::Impedance => "impedance ohm",
        Quantity::Displacement => "level dB re 1 m",
        Quantity::Velocity => "level dB re 1 m/s",
        Quantity::Generic => "level dB re 1",
    }
}

/// Writes a curve. FRD holds levels in dB and ZMA impedance in ohm; REW
/// text follows REW's own export (a `*` header, tab-separated frequency,
/// level or impedance, and phase, 0.0 when there is none); CSV names its
/// columns with units and writes exact numbers. The sidecar goes in its
/// own file ([`Sidecar::to_text`]).
pub fn export(curve: &Curve, format: Format) -> Result<String, CurveError> {
    let q = curve.quantity;
    let level = curve.level_db();
    let phase = curve.phase_deg.as_deref();
    let mut s = String::new();
    let engine = crate::solve::ENGINE;
    match format {
        Format::Auto => {
            return Err(CurveError::new(
                "choose an export format: frd, zma, rew or csv",
            ))
        }
        Format::Frd | Format::Zma => {
            let (what, values): (&str, &[f64]) = match (format, q) {
                (Format::Zma, Quantity::Impedance) => ("ZMA", &curve.magnitude),
                (Format::Zma, _) => {
                    return Err(CurveError::new(format!(
                        "ZMA holds impedance; this curve is {}",
                        q.name()
                    )))
                }
                (_, Quantity::Impedance) => {
                    return Err(CurveError::new(
                        "FRD holds levels in dB; write an impedance as ZMA",
                    ))
                }
                _ => ("FRD", &level),
            };
            s += &format!(
                "* {what} written by {engine}: frequency Hz, {}{}\n",
                level_label(q),
                if phase.is_some() { ", phase deg" } else { "" }
            );
            for (i, f) in curve.freqs_hz.iter().enumerate() {
                s += &format!("{f:.6}\t{:.6}", values[i]);
                if let Some(p) = phase {
                    s += &format!("\t{:.4}", p[i]);
                }
                s.push('\n');
            }
        }
        Format::Rew => {
            let sc = &curve.sidecar;
            s += &format!("* Measurement data exported by {engine} (REW text convention)\n");
            if let Some(p) = &sc.provenance {
                if let Some(src) = p.source.as_ref().or(p.tool.as_ref()) {
                    s += &format!("* Source: {src}\n");
                }
            }
            s += &format!("* Format: {}\n", level_label(q));
            if let Some(d) = &sc.date {
                s += &format!("* Dated: {d}\n");
            }
            if let Some(d) = &sc.device {
                s += &format!("* Measurement: {d}\n");
            }
            s += &format!(
                "* Smoothing: {}\n",
                match sc.smoothing {
                    Some(Smoothing::Octave(n)) => format!("1/{n} octave"),
                    _ => "None".into(),
                }
            );
            if let Some(n) = &sc.notes {
                s += &format!("* Note: {}\n", n.replace('\n', " "));
            }
            s += "*\n";
            let (col, values): (&str, &[f64]) = match q {
                Quantity::Impedance => ("Z(Ohms)", &curve.magnitude),
                Quantity::Pressure => ("SPL(dB)", &level),
                _ => ("Magnitude(dB)", &level),
            };
            s += &format!("* Freq(Hz)\t{col}\tPhase(degrees)\n");
            for (i, f) in curve.freqs_hz.iter().enumerate() {
                let p = phase.map_or(0.0, |p| p[i]);
                s += &format!("{f:.6}\t{:.6}\t{p:.4}\n", values[i]);
            }
        }
        Format::Csv => {
            let (col, values): (String, &[f64]) = match q {
                Quantity::Pressure => ("level_dB".into(), &level),
                _ => (q.magnitude_key().into(), &curve.magnitude),
            };
            s += &format!("frequency_Hz,{col}");
            if phase.is_some() {
                s += ",phase_deg";
            }
            s.push('\n');
            for (i, f) in curve.freqs_hz.iter().enumerate() {
                s += &format!("{f},{}", values[i]);
                if let Some(p) = phase {
                    s += &format!(",{}", p[i]);
                }
                s.push('\n');
            }
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_fields() {
        assert_eq!(role_of("Freq(Hz)"), Role::Freq { scale: 1.0 });
        assert_eq!(role_of("Frequency [kHz]"), Role::Freq { scale: 1e3 });
        assert_eq!(role_of("frequency_Hz"), Role::Freq { scale: 1.0 });
        assert_eq!(role_of("SPL(dB)"), Role::Mag(MagUnit::Db));
        assert_eq!(role_of("level_dB"), Role::Mag(MagUnit::Db));
        assert_eq!(
            role_of("Z(Ohms)"),
            Role::Mag(MagUnit::Linear(1.0, Some(Quantity::Impedance)))
        );
        assert_eq!(
            role_of("magnitude_m_per_s"),
            Role::Mag(MagUnit::Linear(1.0, Some(Quantity::Velocity)))
        );
        assert_eq!(role_of("Phase(degrees)"), Role::Phase { radians: false });
        assert_eq!(role_of("phase [rad]"), Role::Phase { radians: true });
        assert_eq!(role_of("raw"), Role::Mag(MagUnit::Db));
        assert_eq!(role_of("error"), Role::Other);
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff("20\t65.1\t3"), (Delim::Tab, None));
        assert_eq!(sniff("20;65,1;3"), (Delim::Semicolon, None));
        assert_eq!(sniff("20,5 65,1 3,2"), (Delim::Whitespace, Some(true)));
        assert_eq!(sniff("20.5,65.1,3"), (Delim::Comma, Some(false)));
        assert_eq!(sniff("27.0, 68.31, a comment"), (Delim::Comma, Some(false)));
        assert_eq!(sniff("20 65.1"), (Delim::Whitespace, None));
    }
}
