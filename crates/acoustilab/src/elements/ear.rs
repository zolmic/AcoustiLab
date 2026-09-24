//! Ear, head and fixture loads: ear-canal transfer-matrix chains, ITU-T P.57
//! Type 4.3, IEC 60318-4 literature model, eardrum terminations (spec Section 7).

use super::Constructor;

pub const TYPES: &[&str] = &[];

pub fn constructor(_ty: &str) -> Option<Constructor> {
    None
}
