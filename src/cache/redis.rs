use async_trait::async_trait;
use redis::AsyncCommands;

use super::CacheDriver;

pub struct RedisCache {
    pool: deadpool_redis::Pool,
}

impl RedisCache {
    pub fn new(redis_url: &str) -> Result<Self, deadpool_redis::CreatePoolError> {
        let cfg = deadpool_redis::Config::from_url(redis_url);
        let pool = cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1))?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl CacheDriver for RedisCache {
    async fn get(&self, key: &str) -> Option<String> {
        let mut conn = self.pool.get().await.ok()?;
        conn.get(key).await.ok()
    }

    async fn set(&self, key: &str, value: &str, ttl_secs: u64) {
        if let Ok(mut conn) = self.pool.get().await {
            let _: Result<(), _> = conn.set_ex(key, value, ttl_secs).await;
        }
    }

    async fn forget(&self, key: &str) {
        if let Ok(mut conn) = self.pool.get().await {
            let _: Result<(), _> = conn.del(key).await;
        }
    }
}
