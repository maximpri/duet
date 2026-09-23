// SPDX-License-Identifier: GPL-3.0-or-later
//! List prices, so a run can enforce a dollar budget and report its cost.

use crate::types::Usage;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Price {
    pub model: String,
    /// USD per million tokens.
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

impl Price {
    pub fn cost(&self, u: &Usage) -> f64 {
        let m = 1_000_000.0;
        u.input as f64 * self.input / m
            + u.cache_read as f64 * self.cache_read / m
            + u.cache_write as f64 * self.cache_write / m
            + u.output as f64 * self.output / m
    }
}

/// Built-in prices for the default frontier models (observed 2026-09-23).
pub fn builtin(model: &str) -> Option<Price> {
    let p = |input, cache_read, cache_write, output| Price {
        model: model.to_owned(),
        input,
        cache_read,
        cache_write,
        output,
    };
    Some(match model {
        "glm-5.3" | "glm-5.2" => p(1.40, 0.26, 1.40, 4.40),
        "glm-5.3-flash" => p(0.15, 0.03, 0.15, 0.50),
        "glm-5" => p(1.00, 0.20, 1.00, 3.20),
        "claude-opus-5-5" => p(4.00, 0.20, 5.00, 20.00),
        "claude-sonnet-5" => p(2.00, 0.20, 2.50, 10.00),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_usage() {
        let u = Usage {
            input: 1_000_000,
            cache_read: 1_000_000,
            output: 100_000,
            ..Usage::default()
        };
        let c = builtin("glm-5.3").unwrap().cost(&u);
        assert!((c - (1.40 + 0.26 + 0.44)).abs() < 1e-9);
        assert!(builtin("unknown").is_none());
    }
}
