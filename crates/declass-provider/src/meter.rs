// SPDX-License-Identifier: GPL-3.0-or-later
//! Local operating-cost estimates. Rates are user supplied, zero by default.
use crate::types::{Usage, UsageStatus};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Rates {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_requests: u64,
    /// Requests canceled before the provider returned final usage. Their
    /// unreported tokens may still be billed by a remote endpoint.
    #[serde(default)]
    pub unpriced_cancelled_requests: u64,
    pub cost_usd: f64,
}

type Observer = Arc<dyn Fn(&Snapshot) + Send + Sync>;

pub struct Meter {
    pub rates: Rates,
    state: Mutex<(Snapshot, Option<Observer>)>,
}

impl std::fmt::Debug for Meter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meter")
            .field("rates", &self.rates)
            .finish_non_exhaustive()
    }
}

impl Meter {
    pub fn new(rates: Rates) -> Self {
        Self {
            rates,
            state: Mutex::new((Snapshot::default(), None)),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
            .clone()
    }
    pub fn restore(&self, snapshot: Snapshot) {
        if snapshot.cost_usd.is_finite() && snapshot.cost_usd >= 0.0 {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0 = snapshot;
        }
    }
    pub fn observe(&self, observer: Observer) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1 = Some(observer);
    }
    pub fn record(&self, billed: Usage, failed: Usage) {
        self.record_inner(billed, failed, false);
    }

    /// Preserve usage from completed attempts when the caller drops a local
    /// request, and mark the in-flight attempt's price as unknown.
    pub fn record_cancelled(&self, billed: Usage, failed: Usage) {
        self.record_inner(billed, failed, true);
    }

    fn record_inner(&self, billed: Usage, failed: Usage, cancelled: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = &mut state.0;
        s.requests += 1;
        s.unpriced_cancelled_requests += u64::from(cancelled);
        let input = billed.total_input().saturating_add(failed.total_input());
        let output = billed.output.saturating_add(failed.output);
        s.input_tokens = s.input_tokens.saturating_add(input);
        s.output_tokens = s.output_tokens.saturating_add(output);
        s.estimated_requests += u64::from(
            billed.status == UsageStatus::Estimated || failed.total_input() + failed.output > 0,
        );
        s.cost_usd += (input as f64 * self.rates.input_per_million
            + output as f64 * self.rates.output_per_million)
            / 1_000_000.0;
        // Serial notification preserves cumulative order for concurrent readers/explorers.
        if let Some(observer) = &state.1 {
            observer(&state.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_rates_default_to_zero_and_resume_never_reprices_old_tokens() {
        let usage = Usage {
            input: 100,
            cache_read: 200,
            cache_write: 300,
            output: 20,
            ..Usage::default()
        };
        let zero = Meter::new(Rates::default());
        zero.record(usage, Usage::default());
        assert_eq!(zero.snapshot().cost_usd, 0.0);
        assert_eq!(zero.snapshot().input_tokens, 600);
        let priced = Meter::new(Rates {
            input_per_million: 2.0,
            output_per_million: 5.0,
        });
        priced.restore(zero.snapshot());
        priced.record(
            usage,
            Usage {
                input: 100,
                output: 10,
                status: UsageStatus::Estimated,
                ..Usage::default()
            },
        );
        let s = priced.snapshot();
        assert_eq!(
            (
                s.requests,
                s.input_tokens,
                s.output_tokens,
                s.estimated_requests
            ),
            (2, 1300, 50, 1)
        );
        assert!((s.cost_usd - 0.00155).abs() < 1e-12);
    }
}
