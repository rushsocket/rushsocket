use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;

use super::CacheDriver;

struct CacheEntry {
    value: String,
    expires_at: Instant,
}

pub struct MemoryCache {
    store: Arc<DashMap<String, CacheEntry>>,
}

impl MemoryCache {
    pub fn new() -> Self {
        let store = Arc::new(DashMap::new());
        // Spawn a cleanup task
        let store_clone = store.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                store_clone.retain(|_: &String, v: &mut CacheEntry| v.expires_at > Instant::now());
            }
        });
        Self { store }
    }
}

#[async_trait]
impl CacheDriver for MemoryCache {
    async fn get(&self, key: &str) -> Option<String> {
        self.store.get(key).and_then(|entry| {
            if entry.expires_at > Instant::now() {
                Some(entry.value.clone())
            } else {
                None
            }
        })
    }

    async fn set(&self, key: &str, value: &str, ttl_secs: u64) {
        self.store.insert(
            key.to_string(),
            CacheEntry {
                value: value.to_string(),
                expires_at: Instant::now() + Duration::from_secs(ttl_secs),
            },
        );
    }

    async fn forget(&self, key: &str) {
        self.store.remove(key);
    }
}
