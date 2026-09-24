//! Acoustic materials: meshes and screens, perforated plates, porous layers,
//! filled cavities, membrane vents (spec Sections 6 and 13).

use super::Constructor;

pub const TYPES: &[&str] = &[];

pub fn constructor(_ty: &str) -> Option<Constructor> {
    None
}
