// SPDX-License-Identifier: GPL-3.0-or-later
//! Durable pre-send reservations for a single priced text-chat benchmark batch.
//! A journal is deliberately single-use: a crash or unknown usage cannot reset spending.
use crate::cost::Price;
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
};

pub struct Budget {
    model: String,
    input_limit: u64,
    output_limit: u64,
    input_rate: u64,
    output_rate: u64,
    cap: u64,
    inner: Mutex<State>,
}
struct State {
    journal: File,
    charged: u64,
    reserved: u64,
    next: u64,
    halted: bool,
}
pub struct Reservation {
    budget: Arc<Budget>,
    id: u64,
    amount: u64,
    finished: bool,
}

// Integer nano-dollars: round prices upwards, never downwards.
fn rate(price: f64) -> Result<u64> {
    ensure!(
        price.is_finite() && price > 0.0 && price <= 1_000_000.0,
        "invalid budget price"
    );
    Ok((price * 1000.0).ceil() as u64)
}
fn event(state: &mut State, value: Value) -> Result<()> {
    let result = (|| {
        serde_json::to_writer(&mut state.journal, &value)?;
        state.journal.write_all(b"\n")?;
        state.journal.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        state.halted = true;
    }
    result
}
impl Budget {
    pub fn create(
        path: &Path,
        dollars: f64,
        price: &Price,
        input_limit: u64,
        output_limit: u64,
    ) -> Result<Arc<Self>> {
        ensure!(
            dollars.is_finite() && dollars > 0.0 && dollars <= 50.0,
            "budget must be above zero and at most $50"
        );
        ensure!(
            price.verified && input_limit > 0 && output_limit > 0,
            "verified price and conservative provider token ceilings required"
        );
        for value in [
            price.input,
            price.cache_read,
            price.cache_write,
            price.output,
        ] {
            rate(value)?;
        }
        let input_rate = rate(price.input.max(price.cache_read).max(price.cache_write))?;
        let output_rate = rate(price.output)?;
        input_limit
            .checked_mul(input_rate)
            .and_then(|v| {
                output_limit
                    .checked_mul(output_rate)
                    .and_then(|o| v.checked_add(o))
            })
            .ok_or_else(|| anyhow::anyhow!("reservation overflow"))?;
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut state = State {
            journal: opts.open(path)?,
            charged: 0,
            reserved: 0,
            next: 0,
            halted: false,
        };
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
        event(
            &mut state,
            json!({"event":"opened","schema_version":1,"cap_usd":dollars,"model":price.model,"input_token_ceiling":input_limit,"output_token_ceiling":output_limit,"input_nano_usd_per_token":input_rate,"output_nano_usd_per_token":output_rate,"price_source":price.source,"price_observed":price.observed}),
        )?;
        Ok(Arc::new(Self {
            model: price.model.clone(),
            input_limit,
            output_limit,
            input_rate,
            output_rate,
            cap: (dollars * 1e9).floor() as u64,
            inner: Mutex::new(state),
        }))
    }
    pub fn check(&self) -> Result<()> {
        let state = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("budget lock poisoned"))?;
        ensure!(
            !state.halted,
            "budget halted: unknown usage, interrupted request, or exhausted cap; inspect budget.jsonl"
        );
        Ok(())
    }
    pub fn reserve(self: &Arc<Self>, method: &str, path: &str, body: &[u8]) -> Result<Reservation> {
        ensure!(
            method == "POST" && matches!(path, "/chat/completions" | "/v1/chat/completions"),
            "bounded budget permits only text chat completions"
        );
        let value: Value = serde_json::from_slice(body)?;
        let object = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("chat request must be an object"))?;
        ensure!(
            object.keys().all(|key| matches!(
                key.as_str(),
                "model"
                    | "messages"
                    | "stream"
                    | "stream_options"
                    | "tools"
                    | "tool_choice"
                    | "max_tokens"
                    | "max_completion_tokens"
                    | "temperature"
                    | "response_format"
                    | "thinking"
                    | "reasoning_effort"
                    | "top_p"
                    | "n"
                    | "seed"
                    | "stop"
                    | "parallel_tool_calls"
            )),
            "request option has no verified budget policy"
        );
        ensure!(
            value["model"].as_str() == Some(&self.model),
            "budget model mismatch"
        );
        ensure!(
            value.get("n").is_none_or(|n| n.as_u64() == Some(1)),
            "budget requires one completion"
        );
        ensure!(
            value.get("modalities").is_none()
                && value.get("audio").is_none()
                && value.get("web_search_options").is_none(),
            "budget only covers text/function tools"
        );
        let messages = value["messages"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing chat messages"))?;
        ensure!(
            messages
                .iter()
                .all(|m| m.get("content").is_none_or(|v| v.is_null()
                    || v.is_string()
                    || v.as_array().is_some_and(|parts| parts
                        .iter()
                        .all(|p| p["type"] == "text" && p["text"].is_string())))),
            "multimodal content has no verified budget price"
        );
        ensure!(
            value.get("tools").is_none_or(|v| v
                .as_array()
                .is_some_and(|a| a.iter().all(|t| t["type"] == "function"))),
            "unpriced tool type"
        );
        ensure!(
            value.get("tool_choice").is_none_or(|choice| {
                choice
                    .as_str()
                    .is_some_and(|s| matches!(s, "auto" | "none" | "required"))
                    || (choice["type"] == "function" && choice["function"]["name"].is_string())
            }),
            "unpriced tool choice"
        );
        for key in ["max_tokens", "max_completion_tokens"] {
            ensure!(
                value
                    .get(key)
                    .is_none_or(|v| v.as_u64().is_some_and(|n| n > 0 && n <= self.output_limit)),
                "output limit exceeds budget ceiling"
            );
        }
        // Fixed full-provider ceilings include tokenizer and framing overhead;
        // this intentionally does not try to estimate tokens from request bytes.
        let amount = self.input_limit * self.input_rate + self.output_limit * self.output_rate;
        let mut state = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("budget lock poisoned"))?;
        ensure!(!state.halted, "budget halted");
        if state
            .charged
            .checked_add(state.reserved)
            .and_then(|v| v.checked_add(amount))
            .is_none_or(|v| v > self.cap)
        {
            state.halted = true;
            event(
                &mut state,
                json!({"event":"halted","reason":"cap_exhausted"}),
            )?;
            bail!("budget cap cannot cover another conservative reservation");
        }
        state.next += 1;
        let id = state.next;
        event(
            &mut state,
            json!({"event":"reserved","id":id,"nano_usd":amount}),
        )?;
        state.reserved += amount;
        Ok(Reservation {
            budget: self.clone(),
            id,
            amount,
            finished: false,
        })
    }
}
impl Reservation {
    pub fn finish(mut self, body: &[u8], success: bool) -> Result<()> {
        let mut last = None;
        let text = std::str::from_utf8(body)?;
        let mut visit = |s: &str| {
            if let Ok(v) = serde_json::from_str::<Value>(s)
                && let Some(u) = v.get("usage")
            {
                last = Some(u.clone());
            }
        };
        if text.trim_start().starts_with('{') {
            visit(text);
        } else {
            ensure!(
                text.lines().any(|line| line.trim() == "data: [DONE]"),
                "stream ended without completion marker"
            );
            for line in text.lines() {
                if let Some(s) = line.strip_prefix("data:") {
                    visit(s.trim());
                }
            }
        }
        let usage =
            last.ok_or_else(|| anyhow::anyhow!("provider did not report complete usage"))?;
        let input = usage["prompt_tokens"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing input usage"))?;
        let output = usage["completion_tokens"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing output usage"))?;
        ensure!(
            success && input <= self.budget.input_limit && output <= self.budget.output_limit,
            "provider error or usage exceeds reserved ceilings"
        );
        // Cache discounted tokens are charged at the full input price: conservative.
        let amount = input * self.budget.input_rate + output * self.budget.output_rate;
        let mut state = self
            .budget
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("budget lock poisoned"))?;
        event(
            &mut state,
            json!({"event":"settled","id":self.id,"input_tokens":input,"output_tokens":output,"charged_nano_usd":amount,"released_nano_usd":self.amount-amount}),
        )?;
        state.reserved -= self.amount;
        state.charged += amount;
        self.finished = true;
        Ok(())
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.finished
            && let Ok(mut state) = self.budget.inner.lock()
        {
            state.halted = true;
            let _ = event(
                &mut state,
                json!({"event":"halted","reason":"unknown_or_interrupted_usage","id":self.id,"retained_nano_usd":self.amount}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn price() -> Price {
        Price {
            model: "test".into(),
            input: 1.,
            cache_read: 0.1,
            cache_write: 1.,
            output: 2.,
            source: "test".into(),
            observed: "test".into(),
            verified: true,
        }
    }
    fn request(b: &Arc<Budget>) -> Result<Reservation> {
        b.reserve(
            "POST",
            "/chat/completions",
            br#"{"model":"test","messages":[]}"#,
        )
    }
    #[test]
    fn concurrent_reservations_and_single_use_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget.jsonl");
        let b = Budget::create(&path, 0.006, &price(), 1000, 1000).unwrap();
        let a = request(&b).unwrap();
        let c = request(&b).unwrap();
        assert!(request(&b).is_err());
        drop((a, c));
        assert!(b.check().is_err());
        assert!(Budget::create(&path, 50., &price(), 1000, 1000).is_err());
    }
    #[test]
    fn known_usage_releases_but_missing_usage_halts() {
        let dir = tempfile::tempdir().unwrap();
        let b = Budget::create(&dir.path().join("b"), 0.003, &price(), 1000, 1000).unwrap();
        request(&b)
            .unwrap()
            .finish(
                br#"{"usage":{"prompt_tokens":0,"completion_tokens":0}}"#,
                true,
            )
            .unwrap();
        assert!(
            request(&b)
                .unwrap()
                .finish(br#"{"usage":{"completion_tokens":0}}"#, true)
                .is_err()
        );
        assert!(request(&b).is_err());
    }
    #[test]
    fn usage_overflow_and_unfinished_stream_keep_full_reservation() {
        for body in [
            r#"{"usage":{"prompt_tokens":1001,"completion_tokens":0}}"#,
            "data: {\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let b = Budget::create(&dir.path().join("b"), 1.0, &price(), 1000, 1000).unwrap();
            assert!(request(&b).unwrap().finish(body.as_bytes(), true).is_err());
            assert!(b.check().is_err());
            let state = b.inner.lock().unwrap();
            assert_eq!(state.reserved, 3_000_000);
            assert_eq!(state.charged, 0);
        }
    }

    #[test]
    fn invalid_prices_models_endpoints_and_interrupted_requests_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = price();
        p.input = f64::NAN;
        // Every individual price must be finite, including discounted prices.
        assert!(Budget::create(&dir.path().join("invalid"), 50., &p, 1000, 1000).is_err());
        let b = Budget::create(&dir.path().join("b"), 50., &price(), 1000, 1000).unwrap();
        assert!(b.reserve("GET", "/models", b"{}").is_err());
        assert!(
            b.reserve(
                "POST",
                "/chat/completions",
                br#"{"model":"other","messages":[]}"#
            )
            .is_err()
        );
        for choice in [
            serde_json::json!({"type":"web_search"}),
            serde_json::json!("web_search"),
            serde_json::json!({"type":"function"}),
        ] {
            let body = serde_json::to_vec(
                &serde_json::json!({"model":"test","messages":[],"tool_choice":choice}),
            )
            .unwrap();
            assert!(b.reserve("POST", "/chat/completions", &body).is_err());
        }
        assert_eq!(b.inner.lock().unwrap().reserved, 0);
        drop(request(&b).unwrap());
        assert!(b.check().is_err());
    }
}
