//! Numerical identifiability: the singular value decomposition of the
//! weighted Jacobian at the fitted point, the covariance it implies, and a
//! plain-language name for every direction the data cannot fix.
//!
//! Let J be the Jacobian of the weighted residuals with respect to the
//! fitting variables in "report space": ln p for log-scale parameters,
//! p/|p| for linear-scale ones (a relative change), dB for level offsets.
//! With J = U·Σ·Vᵀ, the right singular vector v_i is a direction in
//! parameter space along which the residuals change at rate σ_i. Under
//! the usual linearised least-squares assumptions (residuals independent,
//! zero-mean, with variance s² in weighted units), the fitted point
//! scatters along v_i with standard deviation s/σ_i. A direction with
//! σ_i below `rank_tolerance`·σ_max is numerically null: no curve changes
//! along it, and every parameter with a component above [`NULL_LOADING`]
//! in it is unidentifiable. The covariance is the pseudo-inverse over the
//! other directions, C = s²·Σ_i v_i·v_iᵀ/σ_i².
//!
//! Thresholds on a standard deviation sd in ln units (the same for a
//! parameter and for a direction): **determined** when the 95 % interval
//! is within ±25 % (1.96·sd ≤ ln 1.25), **weakly determined** within a
//! factor of 2 (1.96·sd ≤ ln 2), **undetermined** beyond.

use super::dense::{self, Mat};
use serde::Serialize;

/// Component of a null direction above which a variable is unidentifiable.
pub const NULL_LOADING: f64 = 0.05;
/// Largest sd (ln units) of a determined quantity: 1.96·sd = ln 1.25.
pub const SD_DETERMINED: f64 = 0.113_847_0;
/// Largest sd (ln units) of a weakly determined quantity: 1.96·sd = ln 2.
pub const SD_WEAK: f64 = 0.353_648_4;
/// 95 % two-sided normal quantile.
pub const Z95: f64 = 1.959_963_984_540_054;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Determined,
    WeaklyDetermined,
    Undetermined,
    Unidentifiable,
}

impl Level {
    pub fn from_sd(sd: f64) -> Level {
        if sd <= SD_DETERMINED {
            Level::Determined
        } else if sd <= SD_WEAK {
            Level::WeaklyDetermined
        } else {
            Level::Undetermined
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Component {
    pub name: String,
    pub weight: f64,
}

/// One singular direction of the weighted Jacobian.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Direction {
    pub sigma: f64,
    /// σ/σ_max.
    pub relative: f64,
    /// Standard deviation along the direction (ln units), s/σ; `None` when
    /// numerically null.
    pub sd: Option<f64>,
    pub status: Level,
    /// Components above 5 % of the largest, in variable order, the first
    /// one positive.
    pub components: Vec<Component>,
    pub text: String,
}

/// Result of [`analyse`].
#[derive(Debug, Clone)]
pub struct Analysis {
    pub singular_values: Vec<f64>,
    pub directions: Vec<Direction>,
    /// Covariance in report space (pseudo-inverse times s²).
    pub covariance: Mat,
    /// Per variable, the norm of its components in the null directions.
    pub null_loading: Vec<f64>,
}

fn ratio_text(x: f64) -> String {
    let r = if (x - x.round()).abs() < 0.05 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.2}")
    };
    if x >= 0.0 {
        format!("+{r}")
    } else {
        r
    }
}

fn describe(components: &[Component], status: Level, sd: Option<f64>) -> String {
    let names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
    let list = match names.len() {
        0 => "no parameter".to_string(),
        1 => names[0].to_string(),
        2 => format!("{} and {}", names[0], names[1]),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    };
    let ratios = || {
        let smallest = components
            .iter()
            .map(|c| c.weight.abs())
            .fold(f64::INFINITY, f64::min);
        components
            .iter()
            .map(|c| ratio_text(c.weight / smallest))
            .collect::<Vec<_>>()
            .join(" : ")
    };
    let pct = |sd: f64| ((Z95 * sd).exp() - 1.0) * 100.0;
    let together = if names.len() > 1 {
        format!(
            "{list} move together (changes of ln value in the ratio {})",
            ratios()
        )
    } else {
        format!("{list} alone")
    };
    match (status, sd) {
        (Level::Unidentifiable, _) => {
            format!("{together}: no curve changes along this direction, so the data cannot determine it")
        }
        (Level::Undetermined, Some(sd)) => format!(
            "{together}: the data fix this combination only within a factor of {:.3} (95 %)",
            (Z95 * sd).exp()
        ),
        (Level::WeaklyDetermined, Some(sd)) => format!(
            "{together}: weakly determined, within ±{:.3} % (95 %)",
            pct(sd)
        ),
        (_, Some(sd)) => {
            if names.len() > 1 {
                format!(
                    "the combination of {list} (ln weights {}) is determined within ±{:.3} % (95 %)",
                    ratios(),
                    pct(sd)
                )
            } else {
                format!("{list} alone is determined within ±{:.3} % (95 %)", pct(sd))
            }
        }
        (_, None) => together,
    }
}

/// Analyses the report-space weighted Jacobian `jac` (columns named by
/// `names`) with residual variance scale `s2` (module documentation).
pub fn analyse(names: &[String], jac: &Mat, s2: f64, rank_tolerance: f64) -> Analysis {
    let n = names.len();
    let d = dense::svd(jac);
    let smax = d.s.first().copied().unwrap_or(0.0);
    let s = s2.sqrt();
    let mut covariance = Mat::zeros(n, n);
    let mut null_loading = vec![0.0; n];
    let mut directions = Vec::with_capacity(n);
    for i in 0..n {
        let sigma = d.s[i];
        let relative = if smax > 0.0 { sigma / smax } else { 0.0 };
        let v = d.direction(i);
        let null = !(sigma > 0.0 && relative >= rank_tolerance);
        let sd = (!null).then(|| s / sigma);
        if null {
            for k in 0..n {
                null_loading[k] += v[k] * v[k];
            }
        } else {
            for a in 0..n {
                for b in 0..n {
                    let c = covariance.get(a, b) + s2 * v[a] * v[b] / (sigma * sigma);
                    covariance.set(a, b, c);
                }
            }
        }
        let status = match sd {
            None => Level::Unidentifiable,
            Some(sd) => Level::from_sd(sd),
        };
        let vmax = v.iter().map(|x| x.abs()).fold(0.0, f64::max);
        // Components in the order of the variables, the first one positive.
        let kept: Vec<usize> = (0..n).filter(|&k| v[k].abs() >= 0.05 * vmax).collect();
        let sign = kept.first().map_or(1.0, |&k| v[k].signum());
        let components: Vec<Component> = kept
            .iter()
            .map(|&k| Component {
                name: names[k].clone(),
                weight: sign * v[k],
            })
            .collect();
        let text = describe(&components, status, sd);
        directions.push(Direction {
            sigma,
            relative,
            sd,
            status,
            components,
            text,
        });
    }
    for x in &mut null_loading {
        *x = x.sqrt();
    }
    Analysis {
        singular_values: d.s,
        directions,
        covariance,
        null_loading,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_null_direction_and_its_ratio() {
        // Residuals depend on a + 2b only (a scale invariance a → a + 2t,
        // b → b − t) and on c.
        let rows: Vec<Vec<f64>> = (0..10)
            .map(|i| {
                let x = 1.0 + i as f64;
                vec![x, 2.0 * x, 0.3 * x * x]
            })
            .collect();
        let names = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let an = analyse(&names, &Mat::from_rows(&rows), 1.0, 1e-9);
        let null = an
            .directions
            .iter()
            .find(|d| d.status == Level::Unidentifiable)
            .unwrap();
        assert_eq!(null.components.len(), 2, "{null:?}");
        assert!(null.text.contains("a and b move together"), "{}", null.text);
        assert!(null.text.contains("ratio +2 : -1"), "{}", null.text);
        assert!(an.null_loading[0] > 0.8 && an.null_loading[1] > 0.4);
        assert!(an.null_loading[2] < 1e-9);
        assert!(an.covariance.get(2, 2) > 0.0);
    }
}
