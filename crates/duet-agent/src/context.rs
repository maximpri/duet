// SPDX-License-Identifier: GPL-3.0-or-later
//! Keeps the frontier's context under its window without summarizing.
//!
//! The conversation only grows at the end. When it passes `mask_at` of the
//! window, old tool results are replaced — all at once — by short stubs, and
//! masking continues until the estimate is under half the window. Doing it in
//! one batch means the provider's prefix cache is invalidated rarely. Tool
//! calls and their results are never separated, and the latest results stay.

use duet_boundary::model::Item;

/// Rough token estimate (about 3.5 bytes per token).
pub fn estimate(items: &[Item], system: &str) -> u64 {
    let bytes: usize = system.len()
        + items
            .iter()
            .map(|i| serde_json::to_string(i).map_or(0, |s| s.len()))
            .sum::<usize>();
    (bytes as u64).div_ceil(7) * 2
}

const KEEP_RECENT_RESULTS: usize = 6;
const STUB_PREFIX: &str = "[earlier output removed to save context";

/// Masks old tool results if needed. Returns how many were masked.
pub fn mask_if_needed(items: &mut [Item], system: &str, window: u64, mask_at: f64) -> usize {
    if (estimate(items, system) as f64) < mask_at * window as f64 {
        return 0;
    }
    let target = window / 2;
    let result_positions: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i, Item::ToolResult { content, .. } if !content.starts_with(STUB_PREFIX)))
        .map(|(n, _)| n)
        .collect();
    let maskable = result_positions.len().saturating_sub(KEEP_RECENT_RESULTS);
    let mut masked = 0;
    for &pos in result_positions.iter().take(maskable) {
        if estimate(items, system) <= target {
            break;
        }
        if let Item::ToolResult { content, .. } = &mut items[pos] {
            let tokens = (content.len() as u64).div_ceil(7) * 2;
            *content =
                format!("{STUB_PREFIX} (~{tokens} tokens); run the tool again if you need it]");
            masked += 1;
        }
    }
    masked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize, size: usize) -> Vec<Item> {
        let mut v = vec![Item::User {
            text: "task".into(),
        }];
        for i in 0..n {
            v.push(Item::Assistant {
                text: String::new(),
                reasoning: None,
                tool_calls: vec![],
            });
            v.push(Item::ToolResult {
                call_id: format!("c{i}"),
                content: "x".repeat(size),
            });
        }
        v
    }

    #[test]
    fn nothing_masked_below_threshold() {
        let mut v = items(3, 100);
        assert_eq!(mask_if_needed(&mut v, "s", 100_000, 0.7), 0);
    }

    #[test]
    fn masks_oldest_first_keeps_recent_and_drops_below_half() {
        let mut v = items(20, 7000);
        let before = estimate(&v, "s");
        let window = before + 1000;
        let masked = mask_if_needed(&mut v, "s", window, 0.7);
        assert!(masked > 0);
        assert!(estimate(&v, "s") <= window / 2 + 3000);
        let last: Vec<&Item> = v.iter().rev().take(12).collect();
        assert!(last.iter().all(
            |i| !matches!(i, Item::ToolResult { content, .. } if content.starts_with(STUB_PREFIX))
        ));
        assert!(
            matches!(&v[2], Item::ToolResult { content, .. } if content.starts_with(STUB_PREFIX))
        );
        // A second call right after does nothing (batching).
        assert_eq!(mask_if_needed(&mut v, "s", window, 0.7), 0);
    }
}
