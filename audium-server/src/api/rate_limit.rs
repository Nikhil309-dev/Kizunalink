// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Per-client token-bucket rate limiting for the public API surface (S06).
//!
//! One bucket per source IP: capacity `limit_per_minute` tokens, refilled
//! continuously at `limit_per_minute / 60` tokens per second. A request costs
//! one token; a request that arrives at an empty bucket is rejected with
//! `429 Too Many Requests` and a `Retry-After` header.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;
use tracing::warn;

use crate::server::AppState;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    updated_at: Instant,
}

/// Outcome of a token-bucket check, including the numbers needed to emit
/// `RateLimit-*` response headers.
#[derive(Debug, Clone, Copy)]
pub struct RateLimitStatus {
    /// `Some` when the caller must be throttled (429).
    pub retry_after: Option<Duration>,
    /// Total bucket capacity (`limit_per_minute`).
    pub limit: u32,
    /// Tokens left after this request.
    pub remaining: u32,
    /// Seconds until the bucket refills to capacity.
    pub reset_after: Duration,
}

/// Round a sub-minute retry delay up to a whole-second `reset` value, never 0.
fn reset_after_from_retry(retry: Duration) -> Duration {
    Duration::from_secs(retry.as_secs().max(1))
}

/// IP-keyed token bucket. `limit_per_minute == 0` disables limiting entirely.
pub struct RateLimiter {
    limit_per_minute: u32,
    buckets: DashMap<IpAddr, Bucket>,
}

impl RateLimiter {
    pub fn new(limit_per_minute: u32) -> Self {
        Self {
            limit_per_minute,
            buckets: DashMap::new(),
        }
    }

    pub fn disabled(&self) -> bool {
        self.limit_per_minute == 0
    }

    fn refill_rate(&self) -> f64 {
        f64::from(self.limit_per_minute) / 60.0
    }

    /// Consume one token for `ip`, refilling from elapsed time first.
    /// Returns `Some(retry_after)` when the caller must be throttled.
    ///
    /// `now` is injected so the logic is deterministic and unit-testable.
    pub fn check(&self, ip: IpAddr, now: Instant) -> Option<Duration> {
        self.check_with_status(ip, now).retry_after
    }

    /// Like [`check`], but also returns the remaining budget so callers can
    /// advertise `RateLimit-*` headers (NodeLink parity).
    pub fn check_with_status(&self, ip: IpAddr, now: Instant) -> RateLimitStatus {
        let capacity = f64::from(self.limit_per_minute);
        if self.disabled() {
            return RateLimitStatus {
                retry_after: None,
                limit: 0,
                remaining: 0,
                reset_after: Duration::ZERO,
            };
        }

        let mut entry = self.buckets.entry(ip).or_insert(Bucket {
            tokens: capacity,
            updated_at: now,
        });

        let elapsed = now
            .saturating_duration_since(entry.updated_at)
            .as_secs_f64();
        entry.tokens = (entry.tokens + elapsed * self.refill_rate()).min(capacity);
        entry.updated_at = now;

        let remaining_before = entry.tokens.floor().max(0.0) as u32;
        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            RateLimitStatus {
                retry_after: None,
                limit: self.limit_per_minute,
                remaining: entry.tokens.floor().max(0.0) as u32,
                reset_after: self.time_to_full(entry.tokens),
            }
        } else {
            let wait_secs = (1.0 - entry.tokens) / self.refill_rate();
            let retry = Duration::from_secs_f64(wait_secs).max(Duration::from_secs(1));
            RateLimitStatus {
                retry_after: Some(retry),
                limit: self.limit_per_minute,
                remaining: remaining_before,
                reset_after: reset_after_from_retry(retry),
            }
        }
    }

    /// Seconds until the bucket is back to full capacity.
    fn time_to_full(&self, tokens: f64) -> Duration {
        let missing = (f64::from(self.limit_per_minute) - tokens).max(0.0);
        Duration::from_secs_f64(missing / self.refill_rate())
    }

    /// Current remaining budget for `ip` without consuming a token.
    ///
    /// Used by the response-stamping middleware *after* a request already passed
    /// the limiter, so normal responses can carry `RateLimit-Remaining` without
    /// double-charging the bucket.
    pub fn peek(&self, ip: IpAddr, now: Instant) -> (u32, u32, u64) {
        if self.disabled() {
            return (0, 0, 0);
        }
        let capacity = f64::from(self.limit_per_minute);
        match self.buckets.get(&ip) {
            Some(entry) => {
                let elapsed = now
                    .saturating_duration_since(entry.updated_at)
                    .as_secs_f64();
                let tokens = (entry.tokens + elapsed * self.refill_rate()).min(capacity);
                (
                    self.limit_per_minute,
                    tokens.floor().max(0.0) as u32,
                    self.time_to_full(tokens).as_secs(),
                )
            }
            None => (self.limit_per_minute, self.limit_per_minute, 0),
        }
    }

    /// Drop buckets idle for longer than `idle_ttl` so memory stays bounded under IP churn.
    pub fn cleanup(&self, now: Instant, idle_ttl: Duration) {
        self.buckets
            .retain(|_, bucket| now.saturating_duration_since(bucket.updated_at) <= idle_ttl);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.buckets.len()
    }
}

/// Axum middleware: throttles by the *connected* peer address.
///
/// When deployed behind a reverse proxy the socket IP is the proxy itself; run the
/// proxy-to-client IP extraction (e.g. `tower_http`'s forwarded header layers) in
/// front of this if you need per-client accounting in that topology.
///
/// Every response — allowed or throttled — carries `RateLimit-Limit`,
/// `RateLimit-Remaining`, `RateLimit-Reset`, `RateLimit-Policy` and (on 429)
/// `Retry-After`, mirroring NodeLink's client-facing backoff contract.
pub async fn rate_limit(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let limiter = state.rate_limiter.as_ref();
    if limiter.disabled() {
        return next.run(req).await;
    }

    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip());

    let Some(ip) = peer else {
        return next.run(req).await;
    };

    let status = limiter.check_with_status(ip, Instant::now());
    match status.retry_after {
        None => {
            let mut resp = next.run(req).await;
            let (limit, remaining, reset) = limiter.peek(ip, Instant::now());
            stamp_headers(resp.headers_mut(), limit, remaining, reset, None);
            resp
        }
        Some(retry_after) => {
            warn!("rate-limit: throttling {ip} for {retry_after:?}");
            let mut resp = Response::new("Too Many Requests".into());
            *resp.status_mut() = StatusCode::TOO_MANY_REQUESTS;
            stamp_headers(
                resp.headers_mut(),
                status.limit,
                status.remaining,
                status.reset_after.as_secs(),
                Some(retry_after),
            );
            resp
        }
    }
}

/// Insert the `RateLimit-*` (and optional `Retry-After`) headers.
fn stamp_headers(
    headers: &mut axum::http::HeaderMap,
    limit: u32,
    remaining: u32,
    reset_secs: u64,
    retry_after: Option<Duration>,
) {
    let mut put = |name: &'static str, value: String| {
        if let Ok(v) = HeaderValue::from_str(&value) {
            headers.insert(HeaderName::from_static(name), v);
        }
    };
    put("ratelimit-limit", limit.to_string());
    put("ratelimit-remaining", remaining.to_string());
    put("ratelimit-reset", reset_secs.to_string());
    put("ratelimit-policy", format!("{limit};w=60"));
    if let Some(retry) = retry_after {
        put("retry-after", retry.as_secs().max(1).to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    #[test]
    fn disabled_limiter_never_blocks() {
        let l = RateLimiter::new(0);
        for _ in 0..1000 {
            assert!(l.check(ip(1), Instant::now()).is_none());
        }
    }

    #[test]
    fn burst_over_capacity_is_rejected_with_retry_after() {
        let l = RateLimiter::new(60);
        let t0 = Instant::now();
        for _ in 0..60 {
            assert!(l.check(ip(1), t0).is_none());
        }
        let over = l.check(ip(1), t0).expect("61st request must be throttled");
        assert!(!over.is_zero());

        assert!(l.check(ip(2), t0).is_none());
    }

    #[test]
    fn buckets_refill_over_time() {
        let l = RateLimiter::new(60);
        let t0 = Instant::now();
        for _ in 0..60 {
            l.check(ip(1), t0);
        }
        assert!(l.check(ip(1), t0).is_some());

        let t3 = t0 + Duration::from_secs(3);
        assert!(l.check(ip(1), t3).is_none());
        assert!(l.check(ip(1), t3).is_none());
        assert!(l.check(ip(1), t3).is_none());
        assert!(l.check(ip(1), t3).is_some());
    }

    #[test]
    fn cleanup_drops_only_idle_buckets() {
        let l = RateLimiter::new(10);
        let t0 = Instant::now();
        l.check(ip(1), t0);
        l.check(ip(2), t0 + Duration::from_secs(5));
        let now = t0 + Duration::from_secs(10);
        l.cleanup(now, Duration::from_secs(8));
        assert_eq!(l.len(), 1, "only the recently-seen bucket survives");
    }

    #[test]
    fn status_reports_limit_and_decreasing_remaining() {
        let l = RateLimiter::new(10);
        let t0 = Instant::now();
        let first = l.check_with_status(ip(1), t0);
        assert_eq!(first.limit, 10);
        assert_eq!(first.remaining, 9);
        assert!(first.retry_after.is_none());
        let second = l.check_with_status(ip(1), t0);
        assert_eq!(second.remaining, 8);
    }

    #[test]
    fn throttled_status_exposes_limit_and_nonzero_retry() {
        let l = RateLimiter::new(2);
        let t0 = Instant::now();
        l.check_with_status(ip(1), t0);
        l.check_with_status(ip(1), t0);
        let over = l.check_with_status(ip(1), t0);
        assert_eq!(over.limit, 2);
        assert_eq!(over.remaining, 0);
        assert!(over.retry_after.expect("throttled").as_secs() >= 1);
        assert!(over.reset_after.as_secs() >= 1);
    }

    #[test]
    fn peek_does_not_consume_a_token() {
        let l = RateLimiter::new(10);
        let t0 = Instant::now();
        l.check_with_status(ip(1), t0);
        assert_eq!(l.peek(ip(1), t0).1, 9, "peek sees tokens left after check");
        assert_eq!(l.peek(ip(1), t0).1, 9, "peek is idempotent");

        assert_eq!(l.peek(ip(9), t0).1, 10);
        assert_eq!(l.len(), 1);
    }
}
