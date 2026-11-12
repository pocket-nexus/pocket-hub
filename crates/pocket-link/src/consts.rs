//! The constants of `contracts/spec/link2.ts`. All integers on the wire are
//! little-endian.

// §2 Carriers and discovery
pub const VERSION: u8 = 2;
/// UDP: the device receives Hub's beacons here.
pub const BEACON_PORT: u16 = 8627;
/// TCP: Hub's listener; ephemeral when taken, and the beacon says which.
pub const PORT: u16 = 8628;
pub const BEACON_MAGIC: u32 = 0x424c_4b50;
pub const BEACON_BYTES: usize = 64;
pub const BEACON_NAME_BYTES: usize = 40;
pub const BEACON_INTERVAL_MS: u64 = 1000;
pub const BROADCAST_REFRESH_MS: u64 = 10_000;
pub const HANDSHAKE_TIMEOUT_MS: u64 = 6000;
pub const MAX_CONNECTIONS: usize = 32;
/// A replacement counts when the older connection answers Hub's PING in time.
pub const REPLACED_PING_MS: u64 = 2000;
pub const REPLACED_COUNT: usize = 3;
pub const REPLACED_WINDOW_MS: u64 = 30_000;

// §3 Admission
pub const ID_HEX: usize = 16;
pub const KEY_BYTES: usize = 32;
pub const NONCE_BYTES: usize = 16;
/// Key files a device reads; a ninth is a setup error (§3.6).
pub const MAX_COMPANIONS: usize = 8;
pub const DEVICE_FILE: &str = "device.txt";
pub const COMPANIONS_DIR: &str = "companions";
pub const KEY_SUFFIX: &str = ".txt";
pub const LAST_SUFFIX: &str = ".last";
pub const NEW_SUFFIX: &str = ".new";
pub const LABEL_COMPANION: &str = "pocket-link/2/companion";
pub const LABEL_CONSOLE: &str = "pocket-link/2/console";
pub const NAME_BYTES: usize = 64;
pub const MODEL_BYTES: usize = 32;
pub const TARGET_BYTES: usize = 15;
pub const MAX_FEATURES: usize = 16;
pub const FEATURE_BYTES: usize = 24;
pub const REFUSED_MESSAGE_BYTES: usize = 160;

// §4 Framing
pub const MAGIC: u32 = 0x4b4c_4b50;
pub const PREAMBLE_BYTES: usize = 8;
pub const HEADER_BYTES: usize = 12;
pub const MAX_HANDSHAKE: u32 = 1024;
pub const PAYLOAD_CEILING: u32 = 32_768;
pub const MIN_PAYLOAD: u32 = 1024;
/// What both sides offer on the TCP carrier.
pub const TCP_PAYLOAD: u32 = 16_384;
pub const PING_BYTES: u32 = 4;
pub const CLOSE_REASON_BYTES: usize = 120;
pub const PING_MS: u64 = 3000;
pub const SILENCE_MS: u64 = 10_000;
/// §4.8: the sender of CLOSE closes its socket this long after, at the latest.
pub const CLOSE_DRAIN_MS: u64 = 1000;

// §5 Requests, replies, events and streams
pub const ID_HUB_BIT: u32 = 0x8000_0000;
pub const ID_DEVICE_MAX: u32 = 0x7fff_ffff;
pub const ERROR_MESSAGE_BYTES: usize = 240;
pub const REQUEST_STALL_MS: u64 = 30_000;
pub const CANCEL_GRACE_MS: u64 = 10_000;
/// Outstanding credit of one stream direction; a grant past it closes with `flow`.
pub const MAX_CREDIT: u64 = 16 * 1024 * 1024;
/// runtime.push, runtime.native.install, install: the window at the device.
pub const WINDOW_BULK: u32 = 64 * 1024;
/// Any device stream: the window at Hub.
pub const WINDOW_HUB: u32 = 256 * 1024;
/// An applet event costs one unit per started `APP_EVENT_UNIT` bytes of JSON.
pub const APP_EVENT_UNIT: usize = 512;
pub const APP_EVENT_WINDOW: u32 = 48;
pub const APP_EVENT_WINDOW_HUB: u32 = 256;

// §6 System messages
pub const FEATURES: [&str; 9] = [
    "runtime",
    "runtime.native",
    "devtools",
    "catalog",
    "install",
    "net.http",
    "agent",
    "channel",
    "apps",
];
pub const FEATURES_INTERVAL_MS: u64 = 1000;
pub const APPS_MAX_ROWS: usize = 64;
pub const APP_ID_BYTES: usize = 128;
pub const APP_OUTPUT_BYTES: usize = 64;
pub const APP_VERSION_BYTES: usize = 32;
pub const PUSH_MAX_BYTES: u64 = 24 * 1024 * 1024;
pub const NATIVE_MIN_BYTES: u64 = 32;
pub const NATIVE_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const NATIVE_NAME_MIN: usize = 6;
pub const NATIVE_NAME_MAX: usize = 64;
/// Hub sends no state-changing request this long after a `launching` reply (§8.3).
pub const LAUNCH_HOLD_MS: u64 = 60_000;
pub const DEVTOOLS_LINE_OUT: usize = 64 * 1024;
pub const DEVTOOLS_LINE_IN: usize = 16 * 1024;
pub const CATALOG_MAX_APPS: usize = 64;
pub const CATALOG_TITLE_BYTES: usize = 64;
pub const CATALOG_SUMMARY_BYTES: usize = 120;
pub const CATALOG_SNAPSHOT_MS: u64 = 30_000;
pub const MAX_CATALOG_REQUESTS: usize = 4;
pub const INSTALL_MAX_BYTES: u64 = 24 * 1024 * 1024;
pub const HTTP_MIN_PAYLOAD: u32 = 12_288;
pub const HTTP_MAX_OPEN: usize = 8;

// §7 Applet channels
pub const MAX_CHANNELS: usize = 4;
pub const CHANNEL_OPEN_MS: u64 = 20_000;
pub const CHANNEL_REQUESTS: usize = 8;
pub const CHANNEL_STREAMS: usize = 4;
/// The ceiling of LINK_APP_MSG; the limit in force is min(this, payload limit − 4).
pub const APP_MSG_CEILING: u32 = 8192;
pub const APP_CHANNEL_PREFIX: u32 = 4;
pub const APP_NAME_BYTES: usize = 64;
pub const APP_ERROR_CODE_BYTES: usize = 32;

// §8, §9
/// Hub requests open at the device: in all, state-changing, screenshots.
pub const HUB_REQUESTS: usize = 8;
