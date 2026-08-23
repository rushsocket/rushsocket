#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Public,
    Private,
    Presence,
    EncryptedPrivate,
    CachePublic,
    CachePrivate,
    CachePresence,
    CacheEncryptedPrivate,
}

impl ChannelKind {
    pub fn from_name(name: &str) -> Self {
        // Pusher v7 cache forms (longest prefix first).
        if name.starts_with("private-encrypted-cache-") {
            return ChannelKind::CacheEncryptedPrivate;
        }
        if name.starts_with("presence-cache-") {
            return ChannelKind::CachePresence;
        }
        if name.starts_with("private-cache-") {
            return ChannelKind::CachePrivate;
        }
        // Legacy leading-`cache-` family (cache-private-*, cache-presence-*, cache-*).
        if let Some(rest) = name.strip_prefix("cache-") {
            return match Self::from_name_no_cache(rest) {
                ChannelKind::Public => ChannelKind::CachePublic,
                ChannelKind::Private => ChannelKind::CachePrivate,
                ChannelKind::Presence => ChannelKind::CachePresence,
                ChannelKind::EncryptedPrivate => ChannelKind::CacheEncryptedPrivate,
                ChannelKind::CachePublic
                | ChannelKind::CachePrivate
                | ChannelKind::CachePresence
                | ChannelKind::CacheEncryptedPrivate => unreachable!(),
            };
        }
        Self::from_name_no_cache(name)
    }

    fn from_name_no_cache(name: &str) -> Self {
        if name.starts_with("presence-") {
            ChannelKind::Presence
        } else if name.starts_with("private-encrypted-") {
            ChannelKind::EncryptedPrivate
        } else if name.starts_with("private-") {
            ChannelKind::Private
        } else {
            ChannelKind::Public
        }
    }

    pub fn is_private(self) -> bool {
        matches!(
            self,
            ChannelKind::Private
                | ChannelKind::EncryptedPrivate
                | ChannelKind::CachePrivate
                | ChannelKind::CacheEncryptedPrivate
        )
    }

    pub fn is_presence(self) -> bool {
        matches!(self, ChannelKind::Presence | ChannelKind::CachePresence)
    }

    pub fn is_cache(self) -> bool {
        matches!(
            self,
            ChannelKind::CachePublic
                | ChannelKind::CachePrivate
                | ChannelKind::CachePresence
                | ChannelKind::CacheEncryptedPrivate
        )
    }

    pub fn requires_auth(self) -> bool {
        self.is_private() || self.is_presence()
    }

    /// Client events are allowed on private/presence (including cache variants), not public or encrypted.
    pub fn allows_client_events(self) -> bool {
        matches!(
            self,
            ChannelKind::Private
                | ChannelKind::CachePrivate
                | ChannelKind::Presence
                | ChannelKind::CachePresence
        )
    }
}

pub fn is_valid_channel_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 200 {
        return false;
    }
    let bytes = name.as_bytes();
    let start = if bytes[0] == b'#' { 1 } else { 0 };
    if start == bytes.len() {
        return false;
    }
    bytes[start..].iter().all(|&b| {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=' | b'@' | b',' | b'.' | b';')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_kind_public() {
        assert_eq!(ChannelKind::from_name("my-channel"), ChannelKind::Public);
        assert_eq!(ChannelKind::from_name("foo"), ChannelKind::Public);
    }

    #[test]
    fn test_channel_kind_private() {
        assert_eq!(ChannelKind::from_name("private-foo"), ChannelKind::Private);
    }

    #[test]
    fn test_channel_kind_presence() {
        assert_eq!(
            ChannelKind::from_name("presence-room"),
            ChannelKind::Presence
        );
    }

    #[test]
    fn test_channel_kind_encrypted_private() {
        assert_eq!(
            ChannelKind::from_name("private-encrypted-secret"),
            ChannelKind::EncryptedPrivate
        );
    }

    #[test]
    fn test_channel_kind_cache_variants() {
        assert_eq!(
            ChannelKind::from_name("cache-my-channel"),
            ChannelKind::CachePublic
        );
        assert_eq!(
            ChannelKind::from_name("cache-private-foo"),
            ChannelKind::CachePrivate
        );
        assert_eq!(
            ChannelKind::from_name("cache-presence-room"),
            ChannelKind::CachePresence
        );
        assert_eq!(
            ChannelKind::from_name("cache-private-encrypted-x"),
            ChannelKind::CacheEncryptedPrivate
        );
        assert_eq!(
            ChannelKind::from_name("private-cache-prices"),
            ChannelKind::CachePrivate
        );
        assert_eq!(
            ChannelKind::from_name("presence-cache-room"),
            ChannelKind::CachePresence
        );
        assert_eq!(
            ChannelKind::from_name("private-encrypted-cache-x"),
            ChannelKind::CacheEncryptedPrivate
        );
        assert!(!ChannelKind::from_name("private-foo").is_cache());
        assert!(ChannelKind::from_name("private-cache-prices").is_cache());
        assert!(ChannelKind::from_name("cache-private-foo").is_cache());
    }

    #[test]
    fn test_allows_client_events() {
        assert!(ChannelKind::Private.allows_client_events());
        assert!(ChannelKind::Presence.allows_client_events());
        assert!(ChannelKind::CachePrivate.allows_client_events());
        assert!(ChannelKind::CachePresence.allows_client_events());
        assert!(!ChannelKind::Public.allows_client_events());
        assert!(!ChannelKind::CachePublic.allows_client_events());
        assert!(!ChannelKind::EncryptedPrivate.allows_client_events());
        assert!(!ChannelKind::CacheEncryptedPrivate.allows_client_events());
    }

    #[test]
    fn test_is_private() {
        assert!(ChannelKind::Private.is_private());
        assert!(ChannelKind::EncryptedPrivate.is_private());
        assert!(ChannelKind::CachePrivate.is_private());
        assert!(ChannelKind::CacheEncryptedPrivate.is_private());
        assert!(!ChannelKind::Public.is_private());
        assert!(!ChannelKind::Presence.is_private());
    }

    #[test]
    fn test_is_presence() {
        assert!(ChannelKind::Presence.is_presence());
        assert!(ChannelKind::CachePresence.is_presence());
        assert!(!ChannelKind::Public.is_presence());
        assert!(!ChannelKind::Private.is_presence());
    }

    #[test]
    fn test_is_cache() {
        assert!(ChannelKind::CachePublic.is_cache());
        assert!(ChannelKind::CachePrivate.is_cache());
        assert!(ChannelKind::CachePresence.is_cache());
        assert!(ChannelKind::CacheEncryptedPrivate.is_cache());
        assert!(!ChannelKind::Public.is_cache());
        assert!(!ChannelKind::Private.is_cache());
    }

    #[test]
    fn test_requires_auth() {
        assert!(ChannelKind::Private.requires_auth());
        assert!(ChannelKind::Presence.requires_auth());
        assert!(ChannelKind::EncryptedPrivate.requires_auth());
        assert!(ChannelKind::CachePrivate.requires_auth());
        assert!(ChannelKind::CachePresence.requires_auth());
        assert!(!ChannelKind::Public.requires_auth());
        assert!(!ChannelKind::CachePublic.requires_auth());
    }

    #[test]
    fn test_valid_channel_names() {
        assert!(is_valid_channel_name("my-channel"));
        assert!(is_valid_channel_name("private-foo"));
        assert!(is_valid_channel_name("presence-bar"));
        assert!(is_valid_channel_name("a"));
        assert!(is_valid_channel_name("#my-channel"));
        assert!(is_valid_channel_name("chan_with_underscores"));
        assert!(is_valid_channel_name("chan=with@special,.;chars"));
    }

    #[test]
    fn test_invalid_channel_names() {
        assert!(!is_valid_channel_name(""));
        assert!(!is_valid_channel_name("has spaces"));
        assert!(!is_valid_channel_name("has\nnewline"));
        assert!(!is_valid_channel_name(&"a".repeat(201)));
    }
}
