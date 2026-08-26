//! Student-t intervals used by the artifact harnesses.
//!
//! This is the Rust port of `scripts/smallsample.py`. The inverse incomplete
//! beta comes from `qomm-sim`, the same numerical implementation used by the
//! DP audit, rather than being copied into the harness.

use qomm_measure::beta::beta_ppf;
use serde_json::{json, Value};

pub fn t_critical(n: usize, alpha: f64) -> Result<f64, &'static str> {
    if n < 2 {
        return Err("an interval needs at least two observations");
    }
    let degrees = (n - 1) as f64;
    let x = beta_ppf(alpha, degrees / 2.0, 0.5);
    Ok((degrees * (1.0 / x - 1.0)).sqrt())
}

pub use qomm_measure::fsum::fsum;

pub fn mean_ci(values: &[f64], alpha: f64) -> Value {
    let n = values.len();
    if n == 0 {
        return json!({
            "mean": null, "half_width": null, "excludes_zero": null, "n": 0,
        });
    }
    let mean = fsum(values.iter().copied()) / n as f64;
    if n == 1 {
        return json!({
            "mean": mean,
            "half_width": 0.0,
            "excludes_zero": null,
            "n": 1,
            "multiplier": null,
        });
    }
    let sd = (fsum(values.iter().map(|value| {
        let delta = value - mean;
        delta * delta
    })) / (n - 1) as f64)
        .sqrt();
    let multiplier = t_critical(n, alpha).expect("n was checked above");
    let half_width = multiplier * sd / (n as f64).sqrt();
    json!({
        "mean": mean,
        "half_width": half_width,
        "excludes_zero": mean.abs() > half_width,
        "n": n,
        "multiplier": multiplier,
    })
}

#[derive(Clone, Copy)]
struct DoubleDouble {
    high: f64,
    low: f64,
}

impl DoubleDouble {
    fn new(high: f64, low: f64) -> Self {
        let (high, low) = two_sum(high, low);
        Self { high, low }
    }

    fn from_product(left: f64, right: f64) -> Self {
        let high = left * right;
        let low = left.mul_add(right, -high);
        Self { high, low }
    }

    fn add(self, other: Self) -> Self {
        let (high, carry) = two_sum(self.high, other.high);
        Self::new(high, carry + self.low + other.low)
    }

    fn sub(self, other: Self) -> Self {
        self.add(Self {
            high: -other.high,
            low: -other.low,
        })
    }

    fn mul(self, other: Self) -> Self {
        let base = Self::from_product(self.high, other.high);
        Self::new(
            base.high,
            base.low + self.high * other.low + self.low * other.high + self.low * other.low,
        )
    }

    fn div_scalar(self, divisor: f64) -> Self {
        let quotient = self.high / divisor;
        let remainder = self.sub(Self::from_product(quotient, divisor));
        Self::new(quotient, (remainder.high + remainder.low) / divisor)
    }
}

fn two_sum(left: f64, right: f64) -> (f64, f64) {
    let sum = left + right;
    let right_virtual = sum - left;
    let left_virtual = sum - right_virtual;
    let right_error = right - right_virtual;
    let left_error = left - left_virtual;
    (sum, left_error + right_error)
}

/// Population standard deviation with enough guard precision to match
/// `statistics.pstdev`'s exact-rational calculation for finite `f64` inputs.
pub fn population_sd(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sum = DoubleDouble {
        high: 0.0,
        low: 0.0,
    };
    let mut squares = sum;
    for &value in values {
        sum = sum.add(DoubleDouble {
            high: value,
            low: 0.0,
        });
        squares = squares.add(DoubleDouble::from_product(value, value));
    }
    let count = values.len() as f64;
    let mean = sum.div_scalar(count);
    let variance = squares.div_scalar(count).sub(mean.mul(mean));
    let root = (variance.high + variance.low).sqrt();
    let residual = variance.sub(DoubleDouble::from_product(root, root));
    let correction = (residual.high + residual.low) / (2.0 * root);
    Some(DoubleDouble::new(root, correction).high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_student_table_matches() {
        for (n, expected) in [
            (2, 12.706),
            (3, 4.303),
            (5, 2.776),
            (8, 2.365),
            (13, 2.179),
            (21, 2.086),
            (25, 2.064),
            (121, 1.980),
        ] {
            assert!((t_critical(n, 0.05).unwrap() - expected).abs() < 5e-4);
        }
    }

    #[test]
    fn empty_and_singleton_shapes_match_python() {
        assert_eq!(mean_ci(&[], 0.05)["n"], 0);
        assert_eq!(mean_ci(&[3.0], 0.05)["multiplier"], Value::Null);
    }

    #[test]
    fn population_spread_rounds_like_python_statistics() {
        let values = [
            f64::from_bits(0x4025_0364_8ca5_520c),
            f64::from_bits(0xc016_8d43_3d11_e3a0),
            f64::from_bits(0x3ffe_214f_936e_6b60),
            f64::from_bits(0x3ff4_4027_7ac1_b240),
            f64::from_bits(0xc020_08f1_cfe2_63f0),
        ];
        assert_eq!(
            population_sd(&values).unwrap().to_bits(),
            0x401a_0591_867e_5e25
        );
    }
}
