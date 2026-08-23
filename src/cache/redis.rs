use async_trait::async_trait;
use redis::AsyncCommands;

use super::CacheDriver;

pub struct RedisCache {
    pool: deadpool_redis::Pool,
    prefix: String,
}

impl RedisCache {
    pub fn new(redis_url: &str, prefix: &str) -> Result<Self, deadpool_redis::CreatePoolError> {
        let cfg = deadpool_redis::Config::from_url(redis_url);
        let pool = cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1))?;
        Ok(Self {
            pool,
            prefix: prefix.to_string(),
        })
    }

    fn namespaced(&self, key: &str) -> String {
        namespaced_key(&self.prefix, key)
    }
}

pub(crate) fn namespaced_key(prefix: &str, key: &str) -> String {
    format!("{prefix}:{key}")
}

#[async_trait]
impl CacheDriver for RedisCache {
    async fn get(&self, key: &str) -> Option<String> {
        let mut conn = self.pool.get().await.ok()?;
        let k = self.namespaced(key);
        conn.get(&k).await.ok()
    }

    async fn set(&self, key: &str, value: &str, ttl_secs: u64) {
        if let Ok(mut conn) = self.pool.get().await {
            let k = self.namespaced(key);
            let _: Result<(), _> = conn.set_ex(&k, value, ttl_secs).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::namespaced_key;

    #[test]
    fn prefix_is_applied_inside_driver() {
        assert_eq!(
            namespaced_key("rushsocket", "cache:app:private-cache-foo"),
            "rushsocket:cache:app:private-cache-foo"
        );
    }
}
