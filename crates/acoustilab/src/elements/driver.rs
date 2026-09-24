//! Transducer macros and driver records: D0-D2 moving-coil drivers, creep,
//! record governance checks (spec Section 5).

use super::Constructor;

pub const TYPES: &[&str] = &[];

pub fn constructor(_ty: &str) -> Option<Constructor> {
    None
}
