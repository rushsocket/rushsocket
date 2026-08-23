/// Pusher protocol error/close codes (only values that are actually emitted).
pub const UNKNOWN_APP: u16 = 4001;
pub const APP_DISABLED: u16 = 4003;
pub const APP_OVER_CONNECTION_QUOTA: u16 = 4004;
pub const CONNECTION_UNAVAILABLE: u16 = 4009;
pub const SERVER_CLOSING: u16 = 4200;
pub const ACTIVITY_TIMEOUT: u16 = 4201;

#[cfg(test)]
mod tests {
    #[test]
    fn connection_quota_is_4004() {
        assert_eq!(super::APP_OVER_CONNECTION_QUOTA, 4004);
    }

    #[test]
    fn connection_unavailable_is_4009_not_activity_timeout() {
        assert_eq!(super::CONNECTION_UNAVAILABLE, 4009);
        assert_eq!(super::ACTIVITY_TIMEOUT, 4201);
        assert_ne!(super::CONNECTION_UNAVAILABLE, super::ACTIVITY_TIMEOUT);
    }
}
