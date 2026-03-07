/// Pusher protocol error/close codes.
pub const UNKNOWN_APP: u16 = 4001;
pub const APP_DISABLED: u16 = 4003;
pub const APP_OVER_CONNECTION_QUOTA: u16 = 4100;
pub const PATH_NOT_FOUND: u16 = 4200; // Also used for graceful shutdown
pub const INVALID_VERSION: u16 = 4006;
pub const OVER_CAPACITY: u16 = 4301;
pub const GENERIC_ERROR: u16 = 4009;
pub const INVALID_EVENT_DATA: u16 = 4302;
pub const SERVER_CLOSING: u16 = 4200;
pub const ACTIVITY_TIMEOUT: u16 = 4201;
pub const INVALID_APP_KEY: u16 = 4001;
