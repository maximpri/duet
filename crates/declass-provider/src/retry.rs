// SPDX-License-Identifier: GPL-3.0-or-later
//! Retry timing.

use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;

/// Parses `Retry-After` as delay-seconds or an HTTP date.
pub fn parse_retry_after(value: &str, now: OffsetDateTime) -> Option<Duration> {
    let v = value.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = OffsetDateTime::parse(&v.replace(" GMT", " +0000"), &Rfc2822).ok()?;
    let delta = when - now;
    Some(Duration::from_secs(delta.whole_seconds().max(0) as u64))
}

/// Longest wait between attempts. Retries have no attempt cap, so a long
/// outage is probed about once a minute until it ends or the run's budget does.
pub const MAX_BACKOFF_SECONDS: f64 = 60.0;

/// Exponential backoff: 0.5 s doubling, capped at [`MAX_BACKOFF_SECONDS`], with
/// ±10% jitter from `jitter` in [0, 1).
pub fn backoff(attempt: u32, jitter: f64) -> Duration {
    let base = 0.5 * 2f64.powi(attempt.min(16) as i32);
    let capped = base.min(MAX_BACKOFF_SECONDS);
    Duration::from_secs_f64(capped * (0.9 + 0.2 * jitter.clamp(0.0, 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn retry_after_seconds_and_dates() {
        let now = datetime!(2026-09-23 12:00:00 UTC);
        assert_eq!(parse_retry_after("7", now), Some(Duration::from_secs(7)));
        assert_eq!(
            parse_retry_after("Wed, 23 Sep 2026 12:00:30 GMT", now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse_retry_after("Wed, 23 Sep 2026 11:00:00 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert!(backoff(0, 0.5) < backoff(1, 0.5));
        assert!(backoff(20, 1.0) <= Duration::from_secs_f64(66.0));
        assert!(backoff(20, 0.0) >= Duration::from_secs_f64(54.0));
        assert!(backoff(u32::MAX, 0.5) <= Duration::from_secs_f64(66.0));
    }
}
