// SPDX-License-Identifier: GPL-3.0-or-later
//! Pre-registered gate statistics.
//!
//! Lanes are paired on the run seed, so every comparison is over paired
//! differences. Per-run values are already aggregated (a run's hidden-test pass
//! rate, a run's cost, the mean of an artifact's judge repeats), so resampling
//! pairs is a cluster bootstrap over runs.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PairedSummary {
    pub n: usize,
    pub mean_diff: f64,
    /// One-sided lower confidence bound on the mean difference.
    pub lower: f64,
    /// One-sided upper confidence bound on the mean difference.
    pub upper: f64,
}

/// Paired bootstrap of mean(a − b) with one-sided bounds at level `1 − alpha`.
pub fn paired_bootstrap(
    pairs: &[(f64, f64)],
    iters: usize,
    alpha: f64,
    seed: u64,
) -> PairedSummary {
    let diffs: Vec<f64> = pairs.iter().map(|(a, b)| a - b).collect();
    let n = diffs.len();
    if n == 0 {
        return PairedSummary {
            n: 0,
            mean_diff: f64::NAN,
            lower: f64::NAN,
            upper: f64::NAN,
        };
    }
    let mean = diffs.iter().sum::<f64>() / n as f64;
    let mut rng = SplitMix(seed);
    let mut means: Vec<f64> = (0..iters)
        .map(|_| {
            let s: f64 = (0..n)
                .map(|_| diffs[(rng.next() % n as u64) as usize])
                .sum();
            s / n as f64
        })
        .collect();
    means.sort_by(f64::total_cmp);
    PairedSummary {
        n,
        mean_diff: mean,
        lower: quantile(&means, alpha),
        upper: quantile(&means, 1.0 - alpha),
    }
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let w = pos - lo as f64;
    sorted[lo] * (1.0 - w) + sorted[hi] * w
}

/// Non-inferiority of `a` versus `b`: lower bound of mean(a − b) above `-margin`.
pub fn non_inferior(summary: &PairedSummary, margin: f64) -> bool {
    summary.n > 0 && summary.lower > -margin
}

/// Superiority on a cost-like metric: upper bound of mean(a − b) below zero.
pub fn strictly_lower(summary: &PairedSummary) -> bool {
    summary.n > 0 && summary.upper < 0.0
}

/// One-sided Clopper–Pearson upper bound for a rate with `k` events in `n` trials.
pub fn binomial_upper(k: u64, n: u64, alpha: f64) -> f64 {
    if n == 0 {
        return 1.0;
    }
    if k >= n {
        return 1.0;
    }
    // Find p with P(X <= k | n, p) = alpha; the CDF decreases in p.
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if binomial_cdf(k, n, mid) > alpha {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

fn binomial_cdf(k: u64, n: u64, p: f64) -> f64 {
    if p <= 0.0 {
        return 1.0;
    }
    if p >= 1.0 {
        return if k >= n { 1.0 } else { 0.0 };
    }
    let (lp, lq) = (p.ln(), (1.0 - p).ln());
    let mut ln_choose = 0.0_f64; // ln C(n, 0)
    let mut total = 0.0;
    for i in 0..=k {
        if i > 0 {
            ln_choose += ((n - i + 1) as f64).ln() - (i as f64).ln();
        }
        total += (ln_choose + i as f64 * lp + (n - i) as f64 * lq).exp();
    }
    total.min(1.0)
}

/// Number of paired runs needed to show non-inferiority at `margin` when the
/// true difference is zero, for a paired-difference standard deviation `sd`
/// (one-sided alpha = 0.05, power = 0.8).
pub fn pairs_for_non_inferiority(sd: f64, margin: f64) -> usize {
    const Z_ALPHA: f64 = 1.644_853_6;
    const Z_BETA: f64 = 0.841_621_2;
    ((Z_ALPHA + Z_BETA).powi(2) * sd * sd / (margin * margin)).ceil() as usize
}

pub fn sample_sd(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 2 {
        return f64::NAN;
    }
    let mean = values.iter().sum::<f64>() / n as f64;
    (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
}

struct SplitMix(u64);
impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_lanes_are_non_inferior_and_not_cheaper() {
        let pairs: Vec<_> = (0..20)
            .map(|i| (0.5 + 0.01 * i as f64, 0.5 + 0.01 * i as f64))
            .collect();
        let s = paired_bootstrap(&pairs, 2000, 0.05, 1);
        assert_eq!(s.mean_diff, 0.0);
        assert!(non_inferior(&s, 0.05));
        assert!(!strictly_lower(&s));
    }

    #[test]
    fn clearly_worse_lane_fails_non_inferiority() {
        let pairs: Vec<_> = (0..20)
            .map(|i| (0.4 + 0.005 * (i % 3) as f64, 0.6))
            .collect();
        let s = paired_bootstrap(&pairs, 2000, 0.05, 1);
        assert!(s.mean_diff < -0.15);
        assert!(!non_inferior(&s, 0.05));
    }

    #[test]
    fn consistently_cheaper_lane_is_strictly_lower() {
        let pairs: Vec<_> = (0..12).map(|i| (1.0 + 0.1 * (i % 4) as f64, 2.0)).collect();
        assert!(strictly_lower(&paired_bootstrap(&pairs, 2000, 0.05, 9)));
    }

    #[test]
    fn binomial_upper_matches_rule_of_three_for_zero_events() {
        // With zero events the one-sided 95% bound is 1 − 0.05^(1/n) ≈ 3/n.
        let n = 60;
        let expected = 1.0 - 0.05_f64.powf(1.0 / n as f64);
        assert!((binomial_upper(0, n, 0.05) - expected).abs() < 1e-6);
        assert!(binomial_upper(2, 60, 0.05) > binomial_upper(0, 60, 0.05));
    }

    #[test]
    fn sample_size_grows_as_margin_shrinks() {
        assert!(pairs_for_non_inferiority(3.0, 1.0) > pairs_for_non_inferiority(3.0, 2.0));
        assert_eq!(pairs_for_non_inferiority(3.0, 2.0), 14);
    }
}
