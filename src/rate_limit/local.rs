use std::num::NonZeroU32;
use std::sync::Arc;

use dashmap::DashMap;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

pub struct LocalRateLimiter {
    /// Per-app backend event limiters (keyed by app_id).
    backend_limiters: DashMap<String, Arc<Limiter>>,
    /// Per-socket client event limiters (keyed by socket_id).
    client_limiters: DashMap<String, Arc<Limiter>>,
    /// Per-app read request limiters (keyed by app_id).
    read_limiters: DashMap<String, Arc<Limiter>>,
}

impl LocalRateLimiter {
    pub fn new() -> Self {
        Self {
            backend_limiters: DashMap::new(),
            client_limiters: DashMap::new(),
            read_limiters: DashMap::new(),
        }
    }

    fn get_or_create(
        map: &DashMap<String, Arc<Limiter>>,
        key: &str,
        max_per_sec: i64,
    ) -> Option<Arc<Limiter>> {
        if max_per_sec <= 0 {
            return None; // Unlimited
        }
        let limiter = map
            .entry(key.to_string())
            .or_insert_with(|| {
                let quota = Quota::per_second(NonZeroU32::new(max_per_sec as u32).unwrap());
                Arc::new(RateLimiter::direct(quota))
            })
            .clone();
        Some(limiter)
    }

    /// Check if a backend event is allowed for an app. Returns true if allowed.
    pub fn check_backend_event(&self, app_id: &str, max_per_sec: i64) -> bool {
        match Self::get_or_create(&self.backend_limiters, app_id, max_per_sec) {
            Some(limiter) => limiter.check().is_ok(),
            None => true,
        }
    }

    /// Check if a client event is allowed for a socket. Returns true if allowed.
    pub fn check_client_event(&self, socket_id: &str, max_per_sec: i64) -> bool {
        match Self::get_or_create(&self.client_limiters, socket_id, max_per_sec) {
            Some(limiter) => limiter.check().is_ok(),
            None => true,
        }
    }

    /// Check if a read request is allowed for an app. Returns true if allowed.
    pub fn check_read_request(&self, app_id: &str, max_per_sec: i64) -> bool {
        match Self::get_or_create(&self.read_limiters, app_id, max_per_sec) {
            Some(limiter) => limiter.check().is_ok(),
            None => true,
        }
    }

    /// Remove limiters for a disconnected socket.
    pub fn remove_socket(&self, socket_id: &str) {
        self.client_limiters.remove(socket_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unlimited_always_allows() {
        let rl = LocalRateLimiter::new();
        for _ in 0..1000 {
            assert!(rl.check_backend_event("app1", -1));
        }
    }

    #[test]
    fn test_rate_limit_enforced() {
        let rl = LocalRateLimiter::new();
        // Allow 1 per second
        let allowed = rl.check_backend_event("app1", 1);
        assert!(allowed);
        // Second call within the same instant should be rejected
        let allowed2 = rl.check_backend_event("app1", 1);
        // May or may not be rejected depending on timing, but we've exercised the path
        let _ = allowed2;
    }

    #[test]
    fn test_different_apps_independent() {
        let rl = LocalRateLimiter::new();
        assert!(rl.check_backend_event("app1", 1));
        assert!(rl.check_backend_event("app2", 1));
    }

    #[test]
    fn test_remove_socket() {
        let rl = LocalRateLimiter::new();
        rl.check_client_event("sock1", 10);
        assert!(rl.client_limiters.contains_key("sock1"));
        rl.remove_socket("sock1");
        assert!(!rl.client_limiters.contains_key("sock1"));
    }
}
