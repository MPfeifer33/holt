//! Per-endpoint rate limiter to prevent hammering rate-limited APIs.
//!
//! When one agent hits 429, all agents sharing the same endpoint are delayed
//! using exponential backoff (1s → 2s → 4s cap).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Per-endpoint rate limiter to prevent hammering rate-limited APIs.
/// When one agent hits 429, all agents sharing the same endpoint are delayed.
pub struct RateLimiter {
    endpoints: HashMap<String, EndpointState>,
}

struct EndpointState {
    backoff_until: Option<Instant>,
    current_backoff_ms: u64,
    retry_count: u32,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            endpoints: HashMap::new(),
        }
    }

    /// Extract the base URL (scheme + host + port) for rate limit grouping.
    fn base_url(endpoint: &str) -> String {
        if let Ok(url) = reqwest::Url::parse(endpoint) {
            format!(
                "{}://{}{}",
                url.scheme(),
                url.host_str().unwrap_or("unknown"),
                url.port().map(|p| format!(":{}", p)).unwrap_or_default()
            )
        } else {
            endpoint.to_string()
        }
    }

    /// Record a 429 response for an endpoint and set backoff.
    pub fn record_rate_limit(&mut self, endpoint: &str, retry_after: Option<u64>) {
        let base = Self::base_url(endpoint);
        let state = self.endpoints.entry(base).or_insert(EndpointState {
            backoff_until: None,
            current_backoff_ms: 1000,
            retry_count: 0,
        });

        state.retry_count += 1;

        let backoff_ms = if let Some(secs) = retry_after {
            secs * 1000
        } else {
            // Exponential backoff: 1s, 2s, 4s
            let ms = state.current_backoff_ms;
            state.current_backoff_ms = (ms * 2).min(4000);
            ms
        };

        state.backoff_until = Some(Instant::now() + Duration::from_millis(backoff_ms));
    }

    /// Clear rate limit state for an endpoint (after a successful request).
    pub fn clear(&mut self, endpoint: &str) {
        let base = Self::base_url(endpoint);
        self.endpoints.remove(&base);
    }

    /// Check if we need to wait before making a request to this endpoint.
    /// Returns the duration to wait, if any.
    pub fn check_wait(&self, endpoint: &str) -> Option<Duration> {
        let base = Self::base_url(endpoint);
        if let Some(state) = self.endpoints.get(&base) {
            if let Some(until) = state.backoff_until {
                let now = Instant::now();
                if now < until {
                    return Some(until - now);
                }
            }
        }
        None
    }

    /// Get the current retry count for an endpoint.
    pub fn retry_count(&self, endpoint: &str) -> u32 {
        let base = Self::base_url(endpoint);
        self.endpoints
            .get(&base)
            .map(|s| s.retry_count)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_basic() {
        let mut rl = RateLimiter::new();
        let endpoint = "https://api.example.com/v1";

        assert!(rl.check_wait(endpoint).is_none());

        rl.record_rate_limit(endpoint, Some(2));
        assert!(rl.check_wait(endpoint).is_some());

        rl.clear(endpoint);
        assert!(rl.check_wait(endpoint).is_none());
    }

    #[test]
    fn test_rate_limiter_shared_endpoint() {
        let mut rl = RateLimiter::new();

        // Both point to the same base URL
        rl.record_rate_limit("https://api.example.com/v1/chat/completions", None);

        // Different path, same base — should also be rate limited
        assert!(rl.check_wait("https://api.example.com/v1/models").is_some());
    }

    #[test]
    fn test_rate_limiter_exponential_backoff() {
        let mut rl = RateLimiter::new();
        let endpoint = "https://api.example.com/v1";

        rl.record_rate_limit(endpoint, None); // 1s
        assert_eq!(rl.retry_count(endpoint), 1);

        rl.record_rate_limit(endpoint, None); // 2s
        assert_eq!(rl.retry_count(endpoint), 2);

        rl.record_rate_limit(endpoint, None); // 4s (capped)
        assert_eq!(rl.retry_count(endpoint), 3);
    }
}
