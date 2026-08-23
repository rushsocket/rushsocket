use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::http::StatusCode;
use md5::{Digest, Md5};

use crate::app::App;
use crate::auth as crypto;
use crate::state::AppState;

pub type AuthError = (StatusCode, Json<serde_json::Value>);

fn auth_err(code: StatusCode, msg: &str) -> AuthError {
    (code, Json(serde_json::json!({"error": msg})))
}

/// Verify a Pusher HTTP API request signature with constant-time compare.
pub fn verify_http_signature(
    secret: &str,
    method: &str,
    path: &str,
    query_params: &BTreeMap<String, String>,
    auth_signature: &str,
) -> bool {
    let query_string: String = query_params
        .iter()
        .filter(|(k, _)| k.as_str() != "auth_signature")
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");

    let expected = crypto::sign_http_request(secret, method, path, &query_string);
    crypto::constant_time_eq(expected.as_bytes(), auth_signature.as_bytes())
}

/// Shared REST authenticator for all `/apps/{id}/*` routes.
pub fn authenticate_app(
    state: &AppState,
    app_id: &str,
    method: &str,
    path: &str,
    params: &BTreeMap<String, String>,
    body: &[u8],
) -> Result<Arc<App>, AuthError> {
    let app = state
        .app_manager
        .find_by_id(app_id)
        .ok_or_else(|| auth_err(StatusCode::NOT_FOUND, "App not found"))?;

    if !app.enabled {
        return Err(auth_err(StatusCode::FORBIDDEN, "App disabled"));
    }

    let auth_signature = params
        .get("auth_signature")
        .ok_or_else(|| auth_err(StatusCode::UNAUTHORIZED, "Missing auth_signature"))?;

    if params.get("auth_key").map(|s| s.as_str()) != Some(app.key.as_str()) {
        return Err(auth_err(StatusCode::UNAUTHORIZED, "Invalid auth key"));
    }

    if params.get("auth_version").map(|s| s.as_str()) != Some("1.0") {
        return Err(auth_err(StatusCode::UNAUTHORIZED, "Invalid auth_version"));
    }

    let ts_raw = params
        .get("auth_timestamp")
        .ok_or_else(|| auth_err(StatusCode::UNAUTHORIZED, "Missing auth_timestamp"))?;
    let ts: i64 = ts_raw
        .parse()
        .map_err(|_| auth_err(StatusCode::UNAUTHORIZED, "Invalid auth_timestamp"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let grace = state.config.auth_timestamp_grace_secs;
    if ts.abs_diff(now) > grace {
        tracing::warn!(reason = "expired_timestamp", "HTTP auth rejected");
        return Err(auth_err(StatusCode::UNAUTHORIZED, "Expired auth_timestamp"));
    }

    if !body.is_empty() {
        let expected = params
            .get("body_md5")
            .ok_or_else(|| auth_err(StatusCode::BAD_REQUEST, "Missing body_md5"))?;
        let actual = body_md5(body);
        if &actual != expected {
            tracing::warn!(reason = "md5_mismatch", "HTTP auth rejected");
            return Err(auth_err(StatusCode::BAD_REQUEST, "body_md5 mismatch"));
        }
    }

    if !verify_http_signature(&app.secret, method, path, params, auth_signature) {
        tracing::warn!(reason = "bad_signature", "HTTP auth rejected");
        return Err(auth_err(StatusCode::UNAUTHORIZED, "Invalid signature"));
    }

    Ok(app)
}

/// Compute MD5 hex digest of a body.
pub fn body_md5(body: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(body);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::static_manager::StaticAppManager;
    use crate::config::Config;
    use crate::state::AppState;
    use std::sync::Arc;

    fn signed_params(
        secret: &str,
        method: &str,
        path: &str,
        extra: &[(&str, &str)],
    ) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        params.insert("auth_key".to_string(), "rushsocket-key".to_string());
        params.insert(
            "auth_timestamp".to_string(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        );
        params.insert("auth_version".to_string(), "1.0".to_string());
        for (k, v) in extra {
            params.insert((*k).to_string(), (*v).to_string());
        }
        let query_string: String = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let sig = crypto::sign_http_request(secret, method, path, &query_string);
        params.insert("auth_signature".to_string(), sig);
        params
    }

    fn test_state(enabled: bool) -> Arc<AppState> {
        let mut cfg = Config::default();
        cfg.apps[0].enabled = enabled;
        let apps = cfg.apps.clone();
        let manager = Arc::new(StaticAppManager::new(apps));
        let adapter = Arc::new(crate::adapter::local::LocalAdapter::new());
        let cache: Arc<dyn crate::cache::CacheDriver> =
            Arc::new(crate::cache::memory::MemoryCache::new());
        Arc::new(AppState::new_with_cache(
            cfg,
            manager,
            adapter,
            cache,
            Arc::new(crate::metrics::Metrics::new()),
        ))
    }

    #[test]
    fn test_body_md5() {
        let md5 = body_md5(b"hello world");
        assert_eq!(md5.len(), 32);
        assert!(md5.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_body_md5_empty() {
        assert_eq!(body_md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[tokio::test]
    async fn authenticate_accepts_fresh_signed_get() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let params = signed_params("rushsocket-secret", "GET", path, &[]);
        let app = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap();
        assert_eq!(app.key, "rushsocket-key");
    }

    #[tokio::test]
    async fn authenticate_rejects_disabled_app() {
        let state = test_state(false);
        let path = "/apps/rushsocket-id/channels";
        let params = signed_params("rushsocket-secret", "GET", path, &[]);
        let err = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn authenticate_requires_body_md5_on_nonempty_post() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/events";
        let params = signed_params("rushsocket-secret", "POST", path, &[]);
        let err =
            authenticate_app(&state, "rushsocket-id", "POST", path, &params, b"{}").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authenticate_unknown_app_is_404() {
        let state = test_state(true);
        let path = "/apps/missing/channels";
        let params = signed_params("rushsocket-secret", "GET", path, &[]);
        let err = authenticate_app(&state, "missing", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn authenticate_bad_signature_is_401() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let mut params = signed_params("rushsocket-secret", "GET", path, &[]);
        params.insert("auth_signature".to_string(), "ab".repeat(32));
        let err = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticate_wrong_auth_key_is_401() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let mut params = signed_params("rushsocket-secret", "GET", path, &[]);
        params.insert("auth_key".to_string(), "other".to_string());
        let err = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticate_wrong_auth_version_is_401() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let mut params = signed_params("rushsocket-secret", "GET", path, &[]);
        params.insert("auth_version".to_string(), "1.1".to_string());
        let err = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticate_get_does_not_require_body_md5() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let params = signed_params("rushsocket-secret", "GET", path, &[]);
        assert!(!params.contains_key("body_md5"));
        authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap();
    }

    #[tokio::test]
    async fn authenticate_empty_post_does_not_require_body_md5() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/users/u/terminate_connections";
        let params = signed_params("rushsocket-secret", "POST", path, &[]);
        authenticate_app(&state, "rushsocket-id", "POST", path, &params, &[]).unwrap();
    }

    #[tokio::test]
    async fn authenticate_accepts_matching_body_md5() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/events";
        let body = b"{\"name\":\"e\",\"channel\":\"c\",\"data\":\"{}\"}";
        let md5 = body_md5(body);
        let params = signed_params(
            "rushsocket-secret",
            "POST",
            path,
            &[("body_md5", md5.as_str())],
        );
        authenticate_app(&state, "rushsocket-id", "POST", path, &params, body).unwrap();
    }

    #[tokio::test]
    async fn authenticate_md5_mismatch_is_400() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/events";
        let params = signed_params(
            "rushsocket-secret",
            "POST",
            path,
            &[("body_md5", "00000000000000000000000000000000")],
        );
        let err =
            authenticate_app(&state, "rushsocket-id", "POST", path, &params, b"{}").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    fn resign(params: &mut BTreeMap<String, String>, method: &str, path: &str) {
        let query_string: String = params
            .iter()
            .filter(|(k, _)| k.as_str() != "auth_signature")
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let sig = crypto::sign_http_request("rushsocket-secret", method, path, &query_string);
        params.insert("auth_signature".to_string(), sig);
    }

    #[tokio::test]
    async fn authenticate_rejects_future_timestamp_outside_grace() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let mut params = signed_params("rushsocket-secret", "GET", path, &[]);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let grace = state.config.auth_timestamp_grace_secs as i64;
        params.insert("auth_timestamp".to_string(), (now + grace + 1).to_string());
        resign(&mut params, "GET", path);
        let err = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticate_accepts_timestamp_at_grace_bound() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let mut params = signed_params("rushsocket-secret", "GET", path, &[]);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let grace = state.config.auth_timestamp_grace_secs as i64;
        params.insert("auth_timestamp".to_string(), (now + grace).to_string());
        resign(&mut params, "GET", path);
        authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap();
    }

    #[tokio::test]
    async fn authenticate_rejects_past_timestamp_outside_grace() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let mut params = signed_params("rushsocket-secret", "GET", path, &[]);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let grace = state.config.auth_timestamp_grace_secs as i64;
        params.insert("auth_timestamp".to_string(), (now - grace - 1).to_string());
        resign(&mut params, "GET", path);
        let err = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticate_rejects_i64_min_timestamp_without_panic() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let mut params = signed_params("rushsocket-secret", "GET", path, &[]);
        params.insert("auth_timestamp".to_string(), i64::MIN.to_string());
        resign(&mut params, "GET", path);
        let err = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticate_rejects_stale_timestamp() {
        let state = test_state(true);
        let path = "/apps/rushsocket-id/channels";
        let mut params = signed_params("rushsocket-secret", "GET", path, &[]);
        params.insert("auth_timestamp".to_string(), "1".to_string());
        // Re-sign with stale timestamp
        let query_string: String = params
            .iter()
            .filter(|(k, _)| k.as_str() != "auth_signature")
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let sig = crypto::sign_http_request("rushsocket-secret", "GET", path, &query_string);
        params.insert("auth_signature".to_string(), sig);
        let err = authenticate_app(&state, "rushsocket-id", "GET", path, &params, &[]).unwrap_err();
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_verify_http_signature_valid() {
        let secret = "my-secret";
        let method = "POST";
        let path = "/apps/123/events";
        let mut params = BTreeMap::new();
        params.insert("auth_key".to_string(), "my-key".to_string());
        params.insert("auth_timestamp".to_string(), "1234567890".to_string());
        params.insert("auth_version".to_string(), "1.0".to_string());
        params.insert("body_md5".to_string(), "abc123".to_string());
        let query_string =
            "auth_key=my-key&auth_timestamp=1234567890&auth_version=1.0&body_md5=abc123";
        let sig = crypto::sign(secret, &format!("{}\n{}\n{}", method, path, query_string));
        params.insert("auth_signature".to_string(), sig.clone());
        assert!(verify_http_signature(secret, method, path, &params, &sig));
    }

    #[test]
    fn test_verify_http_signature_invalid() {
        let mut params = BTreeMap::new();
        params.insert("auth_key".to_string(), "key".to_string());
        assert!(!verify_http_signature(
            "secret",
            "GET",
            "/apps/1/channels",
            &params,
            "bad_signature"
        ));
    }

    #[test]
    fn test_verify_http_signature_excludes_auth_signature_from_signing() {
        let secret = "secret";
        let method = "GET";
        let path = "/apps/1/channels";
        let mut params = BTreeMap::new();
        params.insert("auth_key".to_string(), "key".to_string());
        params.insert(
            "auth_signature".to_string(),
            "should_be_excluded".to_string(),
        );
        let query_string = "auth_key=key";
        let sig = crypto::sign(secret, &format!("{}\n{}\n{}", method, path, query_string));
        assert!(verify_http_signature(secret, method, path, &params, &sig));
    }
}
