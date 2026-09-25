//! Reproducibility hash of an expanded netlist (spec Sections 10 and 12:
//! "design-of-experiments tables list every run with its metrics and
//! netlist hash").
//!
//! The hash is SHA-256 (FIPS 180-4) of a canonical JSON text of the
//! document the engine actually reads: the expanded netlist
//! ([`crate::params::Resolved::doc`]: every `=expression` replaced by its
//! value, disabled items removed) without the keys the engine ignores
//! (`title`, `description`, `ui`, `schema`). The canonical text is:
//!
//! * objects with their keys sorted by UTF-8 bytes, no whitespace;
//! * strings escaped as serde_json writes them (`"`, `\`, control
//!   characters);
//! * every number, integer or not, in scientific notation with 12
//!   significant digits, correctly rounded (round half to even on the exact
//!   binary value), trailing zeros removed and a plain exponent:
//!   `25 → 2.5e1`, `0.08 → 8e-2`, `1 → 1e0`, `0 → 0`, `-1.5e-7 → -1.5e-7`.
//!
//! Twelve digits, not the 17 of a round trip, make the hash robust to the
//! last-bit differences between the platform libm implementations (native
//! and wasm32) that derived parameters computed with `^`, `sin`, `ln`, ...
//! may show: a one-ulp change alters the text only if the value lies within
//! one ulp of a 12-digit rounding boundary (probability about 1e-4 per
//! affected value). Two netlists whose numbers agree to 12 digits give the
//! same results to far better than any tolerance in the engine.
//!
//! `tools/analysis/reference.py` reimplements the canonical text and uses
//! Python's `hashlib` for the tests.

use serde_json::Value;
use std::fmt::Write;

/// Top-level keys the engine does not read, left out of the hash.
pub const IGNORED_KEYS: &[&str] = &["title", "description", "ui", "schema"];

/// Canonical text of a number (see the module documentation).
pub fn number(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    if !x.is_finite() {
        // Never produced by the engine (NaN and infinities are not values).
        return "null".into();
    }
    let s = format!("{x:.11e}");
    let (mant, exp) = s.split_once('e').expect("LowerExp has an exponent");
    let mant = if mant.contains('.') {
        mant.trim_end_matches('0').trim_end_matches('.')
    } else {
        mant
    };
    format!("{mant}e{}", exp.parse::<i32>().expect("integer exponent"))
}

/// Canonical JSON text of a value.
pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write_value(v, &mut out);
    out
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(s) => out.push_str(&serde_json::to_string(s).expect("strings serialise")),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("strings serialise"));
                out.push(':');
                write_value(&o[k.as_str()], out);
            }
            out.push('}');
        }
    }
}

/// Canonical text of an expanded netlist, without [`IGNORED_KEYS`].
pub fn netlist_text(doc: &Value) -> String {
    match doc {
        Value::Object(o) => {
            let mut o = o.clone();
            for k in IGNORED_KEYS {
                o.remove(*k);
            }
            canonical(&Value::Object(o))
        }
        v => canonical(v),
    }
}

/// Reproducibility hash of an expanded netlist: lowercase hex SHA-256 of
/// [`netlist_text`].
pub fn netlist_hash(doc: &Value) -> String {
    hex(&sha256(netlist_text(doc).as_bytes()))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 digest (FIPS 180-4).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    let mut w = [0u32; 64];
    for block in msg.chunks_exact(64) {
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_fips_vectors() {
        // FIPS 180-2 Appendix B examples and the empty message.
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha256(&million)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn canonical_numbers_and_ordering() {
        assert_eq!(number(25.0), "2.5e1");
        assert_eq!(number(0.08), "8e-2");
        assert_eq!(number(1.0), "1e0");
        assert_eq!(number(-0.0), "0");
        assert_eq!(number(-1.5e-7), "-1.5e-7");
        assert_eq!(number(123_456_789_012_345.0), "1.23456789012e14");
        // 12 digits: last-bit neighbours canonicalise alike.
        let x = std::f64::consts::PI * 625.0 * 15.0 / 1000.0;
        assert_eq!(number(x), number(f64::from_bits(x.to_bits() + 1)));
        let v = json!({"b": [1, 2.5, true, null], "a": {"z": "q\"", "y": 0.1}});
        assert_eq!(
            canonical(&v),
            r#"{"a":{"y":1e-1,"z":"q\""},"b":[1e0,2.5e0,true,null]}"#
        );
        let with_title = json!({"title": "x", "ui": {}, "schema": "s", "level": 1});
        assert_eq!(netlist_text(&with_title), r#"{"level":1e0}"#);
    }
}
