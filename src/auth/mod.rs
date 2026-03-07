use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Sign a message with HMAC-SHA256 and return hex-encoded signature.
pub fn sign(secret: &str, message: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Verify a channel subscription auth token.
/// Expected format: `"{app_key}:{signature}"`.
/// The signed string is `"{socket_id}:{channel}"` (or with channel_data for presence).
pub fn verify_channel_auth(
    app_key: &str,
    app_secret: &str,
    socket_id: &str,
    channel: &str,
    channel_data: Option<&str>,
    auth: &str,
) -> bool {
    // Check key prefix without allocating
    let rest = match auth.strip_prefix(app_key) {
        Some(r) => r,
        None => return false,
    };
    let signature = match rest.strip_prefix(':') {
        Some(s) => s,
        None => return false,
    };

    // Streaming HMAC — no intermediate String allocation
    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(socket_id.as_bytes());
    mac.update(b":");
    mac.update(channel.as_bytes());
    if let Some(data) = channel_data {
        mac.update(b":");
        mac.update(data.as_bytes());
    }
    let expected_sig = hex::encode(mac.finalize().into_bytes());
    constant_time_eq(signature.as_bytes(), expected_sig.as_bytes())
}

/// Verify a user signin auth token.
/// The signed string is `"{socket_id}::user::{user_data}"`.
pub fn verify_signin_auth(
    app_key: &str,
    app_secret: &str,
    socket_id: &str,
    user_data: &str,
    auth: &str,
) -> bool {
    let rest = match auth.strip_prefix(app_key) {
        Some(r) => r,
        None => return false,
    };
    let signature = match rest.strip_prefix(':') {
        Some(s) => s,
        None => return false,
    };

    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(socket_id.as_bytes());
    mac.update(b"::user::");
    mac.update(user_data.as_bytes());
    let expected_sig = hex::encode(mac.finalize().into_bytes());
    constant_time_eq(signature.as_bytes(), expected_sig.as_bytes())
}

/// Sign an HTTP API request using Pusher's signature scheme.
pub fn sign_http_request(
    secret: &str,
    method: &str,
    path: &str,
    query_params: &str,
) -> String {
    let string_to_sign = format!("{}\n{}\n{}", method, path, query_params);
    sign(secret, &string_to_sign)
}

/// Constant-time comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_deterministic() {
        let sig1 = sign("secret", "hello");
        let sig2 = sign("secret", "hello");
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_sign_different_messages() {
        let sig1 = sign("secret", "hello");
        let sig2 = sign("secret", "world");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_sign_different_secrets() {
        let sig1 = sign("secret1", "hello");
        let sig2 = sign("secret2", "hello");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_sign_produces_hex() {
        let sig = sign("secret", "hello");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(sig.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
    }

    #[test]
    fn test_verify_channel_auth_public() {
        let secret = "app-secret";
        let key = "app-key";
        let socket_id = "123.456";
        let channel = "private-chat";

        let string_to_sign = format!("{}:{}", socket_id, channel);
        let sig = sign(secret, &string_to_sign);
        let auth = format!("{}:{}", key, sig);

        assert!(verify_channel_auth(key, secret, socket_id, channel, None, &auth));
    }

    #[test]
    fn test_verify_channel_auth_with_channel_data() {
        let secret = "app-secret";
        let key = "app-key";
        let socket_id = "123.456";
        let channel = "presence-room";
        let channel_data = r#"{"user_id":"1","user_info":{}}"#;

        let string_to_sign = format!("{}:{}:{}", socket_id, channel, channel_data);
        let sig = sign(secret, &string_to_sign);
        let auth = format!("{}:{}", key, sig);

        assert!(verify_channel_auth(
            key,
            secret,
            socket_id,
            channel,
            Some(channel_data),
            &auth
        ));
    }

    #[test]
    fn test_verify_channel_auth_wrong_signature() {
        assert!(!verify_channel_auth(
            "app-key",
            "app-secret",
            "123.456",
            "private-chat",
            None,
            "app-key:wrong_signature"
        ));
    }

    #[test]
    fn test_verify_channel_auth_wrong_key_prefix() {
        let sig = sign("app-secret", "123.456:private-chat");
        let auth = format!("wrong-key:{}", sig);
        assert!(!verify_channel_auth(
            "app-key",
            "app-secret",
            "123.456",
            "private-chat",
            None,
            &auth
        ));
    }

    #[test]
    fn test_verify_signin_auth() {
        let secret = "app-secret";
        let key = "app-key";
        let socket_id = "123.456";
        let user_data = r#"{"id":"user1","name":"Test"}"#;

        let string_to_sign = format!("{}::user::{}", socket_id, user_data);
        let sig = sign(secret, &string_to_sign);
        let auth = format!("{}:{}", key, sig);

        assert!(verify_signin_auth(key, secret, socket_id, user_data, &auth));
    }

    #[test]
    fn test_verify_signin_auth_wrong_sig() {
        assert!(!verify_signin_auth(
            "app-key",
            "app-secret",
            "123.456",
            r#"{"id":"user1"}"#,
            "app-key:bad_signature"
        ));
    }

    #[test]
    fn test_sign_http_request() {
        let sig = sign_http_request("secret", "POST", "/apps/123/events", "auth_key=key&auth_timestamp=1234");
        assert_eq!(sig.len(), 64);
        // Verify it matches signing the concatenated string
        let expected = sign("secret", "POST\n/apps/123/events\nauth_key=key&auth_timestamp=1234");
        assert_eq!(sig, expected);
    }

}
