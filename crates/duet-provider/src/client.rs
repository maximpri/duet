// SPDX-License-Identifier: GPL-3.0-or-later
//! HTTP client for every wire dialect: streaming, deadlines and retries.

use crate::dialect::Dialect;
use crate::endpoint::check_local_endpoint;
use crate::error::{ErrorKind, ProviderError, is_context_overflow};
use crate::retry::{backoff, parse_retry_after};
use crate::sse::SseDecoder;
use crate::types::{AttemptUsage, Request, Response, Usage, UsageStatus, estimate_tokens};
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// The raw HTTP exchange, abstracted so tests can script provider behaviour.
pub struct HttpReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BoxStream<'static, Result<Bytes, String>>,
}

pub trait Transport: Send + Sync {
    fn post(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>>;
}

pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new(connect_timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(connect_timeout)
                .build()
                .expect("reqwest client"),
        }
    }
}

impl Transport for ReqwestTransport {
    fn post(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let mut rb = self.client.post(url).body(body);
        for (k, v) in headers {
            rb = rb.header(k, v);
        }
        Box::pin(async move {
            let resp = rb
                .send()
                .await
                .map_err(|e| ProviderError::new(ErrorKind::Transport, e.to_string()))?;
            let status = resp.status().as_u16();
            let headers = resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_owned(),
                        v.to_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect();
            let body = resp
                .bytes_stream()
                .map(|r| r.map_err(|e| e.to_string()))
                .boxed();
            Ok(HttpReply {
                status,
                headers,
                body,
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// Receives only gated, boundary-checked content.
    Frontier,
    /// Reads sensitive content; must be loopback or an owner-allowlisted host,
    /// and a remote host needs TLS unless the owner allows plain HTTP.
    Local {
        allowlist: Vec<String>,
        allow_plaintext: bool,
    },
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Endpoint base, e.g. `https://api.z.ai/api/coding/paas/v4` or `http://127.0.0.1:8080/v1`.
    pub base_url: String,
    pub model: String,
    /// The API the endpoint speaks (Chat Completions unless set).
    pub dialect: Dialect,
    /// Environment variable holding the bearer token (never the token itself).
    pub api_key_env: Option<String>,
    pub headers: Vec<(String, String)>,
    pub role: Role,
    /// Deadline for the first response byte (local prefill can be slow).
    pub first_byte_timeout: Duration,
    /// Deadline between stream chunks once output has started.
    pub idle_timeout: Duration,
    /// Attempts per request; `None` (the default) retries infrastructure
    /// failures in place until `deadline` or `cancel` stops them.
    pub max_attempts: Option<u32>,
    /// When retrying must stop: the run's wall-clock budget. A request still
    /// failing then ends with [`ErrorKind::Deadline`]; an attempt in flight is
    /// cut off at it.
    pub deadline: Option<Instant>,
    /// Set when the run is interrupted; retries stop with [`ErrorKind::Cancelled`].
    pub cancel: Option<Arc<AtomicBool>>,
    /// Multiplier on backoff delays (1.0 in production; 0.0 in tests).
    pub backoff_scale: f64,
    /// Recover a tool call written as text (useful for some local models).
    pub recover_text_tool_calls: bool,
}

impl ProviderConfig {
    pub fn new(base_url: &str, model: &str, role: Role) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            dialect: Dialect::Chat,
            api_key_env: None,
            headers: Vec::new(),
            recover_text_tool_calls: matches!(role, Role::Local { .. }),
            role,
            first_byte_timeout: Duration::from_secs(600),
            idle_timeout: Duration::from_secs(180),
            max_attempts: None,
            deadline: None,
            cancel: None,
            backoff_scale: 1.0,
        }
    }
}

/// How often a retry wait looks at the cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(200);

fn deadline_error(last: &str) -> ProviderError {
    ProviderError::new(
        ErrorKind::Deadline,
        format!("the run's wall-clock budget ended while retrying ({last})"),
    )
}

/// A model endpoint speaking one of the [`Dialect`]s (the name predates the
/// other dialects).
pub struct ChatProvider {
    config: ProviderConfig,
    transport: Box<dyn Transport>,
}

impl ChatProvider {
    pub fn new(
        config: ProviderConfig,
        transport: Box<dyn Transport>,
    ) -> Result<Self, ProviderError> {
        if let Role::Local {
            allowlist,
            allow_plaintext,
        } = &config.role
        {
            check_local_endpoint(&config.base_url, allowlist, *allow_plaintext)?;
        }
        Ok(Self { config, transport })
    }

    pub fn with_reqwest(config: ProviderConfig) -> Result<Self, ProviderError> {
        Self::new(
            config,
            Box::new(ReqwestTransport::new(Duration::from_secs(20))),
        )
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    /// The request body exactly as [`ChatProvider::create`] sends it, for the
    /// outbound gate to check and audit.
    pub fn body(&self, req: &Request) -> Value {
        self.config
            .dialect
            .build_body(&self.config.model, req, true)
    }

    fn headers(&self) -> Result<Vec<(String, String)>, ProviderError> {
        let dialect = self.config.dialect;
        let mut h = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("accept".to_owned(), "text/event-stream".to_owned()),
        ];
        h.extend(dialect.fixed_headers());
        if let Some(var) = &self.config.api_key_env {
            let key = std::env::var(var)
                .map_err(|_| ProviderError::new(ErrorKind::Auth, format!("{var} is not set")))?;
            h.extend(dialect.auth_headers(&key));
        }
        h.extend(self.config.headers.iter().cloned());
        Ok(h)
    }

    /// Sends `req`, retrying transient failures in place. Only the deadline,
    /// the cancel flag or `max_attempts` end the retries; errors a fresh attempt
    /// cannot fix (credentials, an invalid request) are returned at once.
    ///
    /// Estimated usage of attempts that failed after output started is kept
    /// apart from the billed usage: in `Response::attempts` on success, and in
    /// `ProviderError::failed_usage` when the request fails in the end.
    pub async fn create(&self, req: &Request) -> Result<Response, ProviderError> {
        let mut attempts = AttemptUsage::default();
        self.retrying(req, &mut attempts).await.map_err(|mut e| {
            e.failed_usage = attempts.estimated_failed;
            e
        })
    }

    async fn retrying(
        &self,
        req: &Request,
        attempts: &mut AttemptUsage,
    ) -> Result<Response, ProviderError> {
        let body = serde_json::to_vec(&self.body(req))
            .map_err(|e| ProviderError::new(ErrorKind::Malformed, e.to_string()))?;
        let url = format!("{}{}", self.config.base_url, self.config.dialect.path());
        let headers = self.headers()?;
        let started = Instant::now();
        loop {
            if self.cancelled() {
                return Err(ProviderError::new(ErrorKind::Cancelled, "interrupted"));
            }
            attempts.attempts += 1;
            let attempt = self.attempt(&url, &headers, &body, req);
            let result = match self.config.deadline {
                Some(at) => match tokio::time::timeout_at(at, attempt).await {
                    Ok(r) => r,
                    Err(_) => return Err(deadline_error("the request was still in flight")),
                },
                None => attempt.await,
            };
            match result {
                Ok(mut response) => {
                    attempts.billed = response.usage;
                    response.attempts = *attempts;
                    return Ok(response);
                }
                Err(err) => {
                    if err.output_started {
                        // The prompt was processed and some output generated.
                        let failed = &mut attempts.estimated_failed;
                        failed.input += estimate_tokens(body.len());
                        failed.output += estimate_tokens(err.partial_output_bytes);
                        failed.status = UsageStatus::Estimated;
                    }
                    let capped = self
                        .config
                        .max_attempts
                        .is_some_and(|max| attempts.attempts >= max);
                    if !err.is_retryable() || capped {
                        return Err(err);
                    }
                    let jitter = f64::from(started.elapsed().subsec_nanos() % 1000) / 1000.0;
                    let delay = err
                        .retry_after
                        .unwrap_or_else(|| backoff(attempts.attempts - 1, jitter))
                        .mul_f64(self.config.backoff_scale);
                    self.pause(delay, &err).await?;
                }
            }
        }
    }

    fn cancelled(&self) -> bool {
        self.config
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::SeqCst))
    }

    /// Waits `delay` before the next attempt, watching the cancel flag; fails
    /// when the deadline comes first.
    async fn pause(&self, delay: Duration, last: &ProviderError) -> Result<(), ProviderError> {
        let wake = Instant::now() + delay;
        if let Some(at) = self.config.deadline
            && wake >= at
        {
            tokio::time::sleep_until(at).await;
            return Err(deadline_error(&last.to_string()));
        }
        loop {
            if self.cancelled() {
                return Err(ProviderError::new(ErrorKind::Cancelled, "interrupted"));
            }
            let now = Instant::now();
            if now >= wake {
                return Ok(());
            }
            let step = if self.config.cancel.is_some() {
                (wake - now).min(CANCEL_POLL)
            } else {
                wake - now
            };
            tokio::time::sleep(step).await;
        }
    }

    async fn attempt(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
        req: &Request,
    ) -> Result<Response, ProviderError> {
        let reply = tokio::time::timeout(
            self.config.first_byte_timeout,
            self.transport
                .post(url.to_owned(), headers.to_vec(), body.to_vec()),
        )
        .await
        .map_err(|_| {
            ProviderError::new(
                ErrorKind::Timeout,
                "no response headers before the first-byte deadline",
            )
        })??;
        let mut stream = reply.body;
        if !(200..300).contains(&reply.status) {
            let mut text = Vec::new();
            while let Ok(Some(Ok(chunk))) =
                tokio::time::timeout(self.config.idle_timeout, stream.next()).await
            {
                text.extend_from_slice(&chunk);
                if text.len() > 64 * 1024 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&text).into_owned();
            let kind = match reply.status {
                401 | 403 => ErrorKind::Auth,
                400 | 413 if is_context_overflow(&text) => ErrorKind::ContextOverflow,
                code => ErrorKind::Status(code),
            };
            let mut e = ProviderError::new(kind, text);
            e.retry_after = reply
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                .and_then(|(_, v)| parse_retry_after(v, time::OffsetDateTime::now_utc()));
            return Err(e);
        }
        let mut decoder = SseDecoder::default();
        let mut assembler = self.config.dialect.assembler();
        let mut first = true;
        loop {
            let deadline = if first {
                self.config.first_byte_timeout
            } else {
                self.config.idle_timeout
            };
            let next = tokio::time::timeout(deadline, stream.next()).await;
            let chunk = match next {
                Err(_) => {
                    let mut e = ProviderError::new(ErrorKind::Timeout, "stream stalled");
                    e.output_started = assembler.output_bytes() > 0;
                    e.partial_output_bytes = assembler.output_bytes();
                    return Err(e);
                }
                Ok(None) => break,
                Ok(Some(Err(msg))) => {
                    let mut e = ProviderError::new(ErrorKind::Transport, msg);
                    e.output_started = assembler.output_bytes() > 0;
                    e.partial_output_bytes = assembler.output_bytes();
                    return Err(e);
                }
                Ok(Some(Ok(bytes))) => bytes,
            };
            first = false;
            for event in decoder.push(&chunk) {
                assembler.apply(&event.data)?;
            }
        }
        for event in decoder.finish() {
            assembler.apply(&event.data)?;
        }
        let recover = self
            .config
            .recover_text_tool_calls
            .then_some(req.tools.as_slice());
        assembler.finish(&self.config.model, recover)
    }
}

/// Sum of billed usage across responses, for ledgers.
pub fn sum_usage<'a>(responses: impl IntoIterator<Item = &'a Usage>) -> Usage {
    responses.into_iter().fold(Usage::default(), |mut acc, u| {
        acc.input += u.input;
        acc.cache_read += u.cache_read;
        acc.cache_write += u.cache_write;
        acc.output += u.output;
        acc.reasoning += u.reasoning;
        acc
    })
}
