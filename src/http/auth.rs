use std::collections::BTreeMap;

use md5::{Digest, Md5};

use crate::auth as crypto;

/// Verify a Pusher HTTP API request signature.
///
/// The signature is computed as:
/// HMAC-SHA256(secret, "POST\n/apps/{app_id}/events\n{sorted_query_params}")
///
/// Query params must be sorted and include auth_key, auth_timestamp, auth_version,
/// and body_md5 (for POST requests with body).
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
    crypto::sign(secret, &format!("{}\n{}\n{}", method, path, query_string)) == auth_signature
        || expected == auth_signature
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

    #[test]
    fn test_body_md5() {
        let md5 = body_md5(b"hello world");
        assert_eq!(md5.len(), 32); // MD5 = 16 bytes = 32 hex chars
        assert!(md5.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_body_md5_empty() {
        let md5 = body_md5(b"");
        // MD5 of empty string is well-known
        assert_eq!(md5, "d41d8cd98f00b204e9800998ecf8427e");
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

        // Compute the expected signature
        let query_string = "auth_key=my-key&auth_timestamp=1234567890&auth_version=1.0&body_md5=abc123";
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
        params.insert("auth_signature".to_string(), "should_be_excluded".to_string());

        // The signing string should NOT include auth_signature
        let query_string = "auth_key=key";
        let sig = crypto::sign(secret, &format!("{}\n{}\n{}", method, path, query_string));

        assert!(verify_http_signature(secret, method, path, &params, &sig));
    }
}
