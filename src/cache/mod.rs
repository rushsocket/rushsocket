pub mod memory;
pub mod redis;

use async_trait::async_trait;

#[async_trait]
pub trait CacheDriver: Send + Sync + 'static {
    async fn get(&self, key: &str) -> Option<String>;
    async fn set(&self, key: &str, value: &str, ttl_secs: u64);
    async fn forget(&self, key: &str);
}
