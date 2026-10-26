//! Repeatability statistics.
//!
//! The engine's central promise is that deterministic metrics do not move.
//! These helpers compute the spread that proves (or disproves) it, and the
//! bootstrap confidence interval used for indicative wall-clock numbers.

use super::{Spread, Uncertainty, UncertaintyKind};
use std::collections::BTreeMap;

/// Median of a sample. Returns `None` for an empty sample.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// Summarise a sample into a `Spread`. `spread_pct` is
/// `(max - min) / median * 100`, so a perfectly stable metric reports 0.
pub fn spread(values: &[f64]) -> Option<Spread> {
    if values.is_empty() {
        return None;
    }
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let med = median(values)?;
    let spread_pct = if med == 0.0 {
        if max == min {
            0.0
        } else {
            f64::INFINITY
        }
    } else {
        (max - min) / med * 100.0
    };
    Some(Spread {
        min,
        max,
        median: med,
        spread_pct,
    })
}

/// Deterministic xorshift64* PRNG.
///
/// Bootstrap resampling must be reproducible, otherwise the "same answer on
/// any machine" promise leaks through the statistics. A seeded PRNG keeps
/// confidence intervals stable across runs.
pub struct Xorshift64Star(u64);

impl Xorshift64Star {
    pub fn new(seed: u64) -> Self {
        Xorshift64Star(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// Percentile of an already-sorted slice, using nearest-rank.
pub fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (p * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// Bootstrap 95% confidence interval of the median.
///
/// Resampling is seeded, so the same input always yields the same interval.
pub fn bootstrap_ci95_median(values: &[f64], iterations: usize, seed: u64) -> Option<Uncertainty> {
    if values.len() < 2 {
        return None;
    }
    let mut rng = Xorshift64Star::new(seed);
    let n = values.len();
    let mut medians = Vec::with_capacity(iterations);
    let mut sample = vec![0.0f64; n];
    for _ in 0..iterations {
        for slot in sample.iter_mut() {
            *slot = values[rng.below(n)];
        }
        medians.push(median(&sample)?);
    }
    medians.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = percentile_sorted(&medians, 0.025);
    let hi = percentile_sorted(&medians, 0.975);
    Some(Uncertainty {
        kind: UncertaintyKind::Ci95,
        value: (hi - lo) / 2.0,
    })
}

/// Build the `per_metric` map for a capsule from raw repeat samples.
pub fn summarise(samples: &BTreeMap<String, Vec<f64>>) -> BTreeMap<String, Spread> {
    samples
        .iter()
        .filter_map(|(name, values)| spread(values).map(|s| (name.clone(), s)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_samples_have_zero_spread() {
        let s = spread(&[1000.0, 1000.0, 1000.0]).unwrap();
        assert_eq!(s.spread_pct, 0.0);
        assert_eq!(s.median, 1000.0);
    }

    #[test]
    fn spread_is_relative_to_median() {
        let s = spread(&[90.0, 100.0, 110.0]).unwrap();
        assert!((s.spread_pct - 20.0).abs() < 1e-9);
    }

    #[test]
    fn median_handles_even_and_odd() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn bootstrap_is_deterministic() {
        let sample = vec![10.0, 11.0, 9.0, 10.5, 10.2, 9.8];
        let a = bootstrap_ci95_median(&sample, 1000, 42).unwrap();
        let b = bootstrap_ci95_median(&sample, 1000, 42).unwrap();
        assert_eq!(a.value, b.value);
        assert!(a.value >= 0.0);
    }

    #[test]
    fn rng_is_reproducible() {
        let mut a = Xorshift64Star::new(7);
        let mut b = Xorshift64Star::new(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}
