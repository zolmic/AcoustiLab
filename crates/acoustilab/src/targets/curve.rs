//! Magnitude responses in dB on a frequency axis.

use super::{Result, TargetError};

/// A magnitude response: levels in dB at strictly increasing, positive,
/// finite frequencies (at least two).
///
/// Between samples the curve is linear in dB on log frequency:
/// `L(f) = L_j + (L_{j+1} − L_j)·ln(f/f_j)/ln(f_{j+1}/f_j)`. Up to
/// [`END_SLACK`] beyond `[f_0, f_{n−1}]` it reads its end value; further out
/// it is undefined, and nothing is extrapolated.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    f: Vec<f64>,
    db: Vec<f64>,
}

/// Relative slack at the curve ends: 1.3 %, the largest gap between a
/// nominal R40 preferred frequency and the exact grid point `10^(k/40)` it
/// names (17 000 Hz against 16 788 Hz). A curve that ends at a nominal
/// frequency therefore covers the grid point of that name: a curve from
/// 20 Hz reads 19.95 Hz. The level there is the end value, held over at most
/// 0.018 octave.
pub const END_SLACK: f64 = 0.013;

impl Curve {
    pub fn new(f: Vec<f64>, db: Vec<f64>) -> Result<Curve> {
        let err = |m: String| Err(TargetError::Curve(m));
        if f.len() != db.len() {
            return err(format!("{} frequencies but {} levels", f.len(), db.len()));
        }
        if f.len() < 2 {
            return err("needs at least two points".into());
        }
        for (i, (&x, &y)) in f.iter().zip(&db).enumerate() {
            if !(x.is_finite() && x > 0.0) {
                return err(format!(
                    "frequency {x} at index {i} is not positive and finite"
                ));
            }
            if !y.is_finite() {
                return err(format!("level at {x} Hz is not finite"));
            }
            if i > 0 && x <= f[i - 1] {
                return err(format!(
                    "frequencies must increase strictly ({} Hz then {x} Hz at index {i})",
                    f[i - 1]
                ));
            }
        }
        Ok(Curve { f, db })
    }

    pub fn freqs(&self) -> &[f64] {
        &self.f
    }

    pub fn db(&self) -> &[f64] {
        &self.db
    }

    /// First and last frequency.
    pub fn range(&self) -> (f64, f64) {
        (self.f[0], self.f[self.f.len() - 1])
    }

    /// Level at `f`, or `None` outside the curve's range.
    pub fn at(&self, f: f64) -> Option<f64> {
        let (lo, hi) = self.range();
        if !(f >= lo * (1.0 - END_SLACK) && f <= hi * (1.0 + END_SLACK)) {
            return None;
        }
        let f = f.clamp(lo, hi);
        // First index with f_j >= f; the segment is [j-1, j].
        let j = self.f.partition_point(|&x| x < f);
        if j == 0 {
            return Some(self.db[0]);
        }
        if self.f[j] == f {
            return Some(self.db[j]);
        }
        let (f0, f1) = (self.f[j - 1], self.f[j]);
        let t = (f / f0).ln() / (f1 / f0).ln();
        Some(self.db[j - 1] + t * (self.db[j] - self.db[j - 1]))
    }

    /// Levels at each frequency of `grid` (`None` outside the range).
    pub fn sample(&self, grid: &[f64]) -> Vec<Option<f64>> {
        grid.iter().map(|&f| self.at(f)).collect()
    }

    /// The same curve shifted by `d` dB.
    pub fn shifted(&self, d: f64) -> Curve {
        Curve {
            f: self.f.clone(),
            db: self.db.iter().map(|y| y + d).collect(),
        }
    }
}
