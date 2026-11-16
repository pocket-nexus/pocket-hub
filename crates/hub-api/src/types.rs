//! The objects of §2, the parameters and results of §3 and the events of §4.
//!
//! A field the contract writes as "X or null" is an `Option` that is always
//! present (`null` when None); a field it marks optional (`port?`, `home?`,
//! `dropped?`, `done?`, `total?`) is left out when None.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Hello and the hub
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    Gui,
    Cli,
    Remote,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gui => "gui",
            Self::Cli => "cli",
            Self::Remote => "remote",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub kind: ClientKind,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloParams {
    pub protocol: u32,
    pub client: ClientInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubInfo {
    pub id: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    pub hub: HubInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carrier {
    /// `tcp`, `usb`, `uart` or `ble`.
    pub kind: String,
    /// The port in use, when the carrier has one (the link port, or the
    /// ephemeral one Hub fell back to).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BunInfo {
    pub version: String,
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub home: String,
    pub started_at: i64,
    pub carriers: Vec<Carrier>,
    pub bun: BunInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameParams {
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeParams {
    #[serde(default)]
    pub topics: Option<Vec<String>>,
}

/// The `{}` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

// ---------------------------------------------------------------------------
// Devices (§2)
// ---------------------------------------------------------------------------

/// Who holds a device's mutex: the kind and name of the holding client's
/// hello, and the method that holds the device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Holder {
    pub kind: ClientKind,
    pub name: String,
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Busy {
    pub holder: Holder,
    pub since: i64,
    /// True while the device restarts into a program it was told to start
    /// (the launch hold: up to 60 s, until it links again).
    pub launching: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    /// `tcp`, `usb`, `uart` or `ble`, for display only.
    pub carrier: String,
    pub peer: String,
    pub since: i64,
    pub payload_limit: u32,
    /// The groups both sides list (link specification §6.1).
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRow {
    pub id: String,
    pub output: String,
    pub version: String,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRow {
    pub app: String,
    pub generation: u32,
    pub since: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    /// The device family: `3ds`, `tab5`, or empty for one Hub does not know.
    pub kind: String,
    /// What Hub can ask of a device of this family: `push`, `screenshot`,
    /// `logs`, `program`. A client offers an operation only when it is here.
    pub supports: Vec<String>,
    pub name: String,
    pub model: String,
    pub target: String,
    pub host_abi: u32,
    pub last_address: Option<String>,
    pub last_seen: Option<i64>,
    pub setup_unconfirmed: bool,
    pub duplicate_id: bool,
    pub link: Option<LinkState>,
    pub status: Option<Value>,
    pub apps: Option<Vec<AppRow>>,
    pub busy: Option<Busy>,
    /// The device's own program.
    pub program: ProgramState,
    pub channels: Vec<ChannelRow>,
}

/// `Device.program`: the version the device reports for its own program
/// (the `home` row of its apps), the newest release Hub knows, and whether
/// that orders above the device's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramState {
    pub version: Option<String>,
    pub latest: Option<String>,
    pub update: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceList {
    pub devices: Vec<Device>,
}

/// `{device}`: a device id, or `"*"` when exactly one device is paired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceParams {
    pub device: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathParams {
    pub device: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushedApp {
    pub id: String,
    pub output: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResult {
    pub app: PushedApp,
    pub hash: String,
    pub generation: u64,
}

/// The progress of `devices.push` and `devices.installProgram`: the
/// device's phase, and the file's bytes Hub has sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    pub phase: String,
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchParams {
    pub device: String,
    pub app: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchResult {
    pub output: String,
}

/// `devices.remove`: `app` is an installed applet's id or output name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveParams {
    pub device: String,
    pub app: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotParams {
    pub device: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub frame: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsParams {
    pub device: String,
    pub on: bool,
}

/// `devices.installProgram`: `program` defaults to `{"release":"latest"}`,
/// `launch` to true.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallProgramParams {
    pub device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<ProgramChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallProgramResult {
    /// The device's reply: `installed`, `staged` or `launching`.
    pub phase: String,
    /// The program's name on the device (for `3ds`: `pocket-shell.3dsx`).
    pub name: String,
    /// The release's version; null for a local file whose version is unknown.
    pub version: Option<String>,
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindParams {
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub address: String,
    pub port: u16,
    pub banner: String,
    /// The banner is ftpd's own: a client offers these first.
    pub ftpd: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverResult {
    pub candidates: Vec<Candidate>,
}

/// `{"release":"latest"}`, `{"release": <a release tag>}` or `{"path": <a local file>}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgramChoice {
    Release { release: String },
    Path { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupInstallParams {
    pub kind: String,
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub program: Option<ProgramChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupCardParams {
    pub kind: String,
    pub path: String,
    #[serde(default)]
    pub program: Option<ProgramChoice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetupStage {
    Connecting,
    Reading,
    Downloading,
    Uploading,
    Pairing,
    Done,
}

impl SetupStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Reading => "reading",
            Self::Downloading => "downloading",
            Self::Uploading => "uploading",
            Self::Pairing => "pairing",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupProgress {
    pub stage: SetupStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairingResult {
    /// A new key was written.
    Key,
    /// The pairing was in place.
    Kept,
    /// Only Hub's address was rewritten.
    Address,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgramAction {
    Installed,
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramInfo {
    pub action: ProgramAction,
    pub version: Option<String>,
    pub runtime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReport {
    pub device: String,
    pub pairing: PairingResult,
    pub program: ProgramInfo,
    /// Always true in a report: a failed setup is the error `setup`.
    pub confirmed: bool,
    pub next: String,
}

// ---------------------------------------------------------------------------
// Pairing by a code the device shows
// ---------------------------------------------------------------------------

/// One device that asks to be paired and waits for the code it shows (§2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    /// The device id the request names. Nothing has proved it yet.
    pub device: String,
    /// The family of its target, as in a Device.
    pub kind: String,
    pub name: String,
    pub model: String,
    pub target: String,
    /// Where the request came from.
    pub address: String,
    pub since: i64,
    /// When Hub closes the request: the device keeps its code as long.
    pub expires: i64,
}

/// The result of `pairing.list`, and the members of `pairing.changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequests {
    pub requests: Vec<PairRequest>,
}

/// `pairing.answer`: `device` is a request's device id, or `"*"` when exactly
/// one request waits; `code` is the 6 digits the device shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairAnswerParams {
    pub device: String,
    pub code: String,
}

// ---------------------------------------------------------------------------
// Daemons
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastExit {
    pub code: Option<i32>,
    /// A signal's name, such as `SIGTERM`.
    pub signal: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Daemon {
    pub id: String,
    pub version: String,
    pub state: DaemonState,
    pub pid: Option<u32>,
    pub since: Option<i64>,
    pub channels: u32,
    pub last_exit: Option<LastExit>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonList {
    pub daemons: Vec<Daemon>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonPathParams {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonIdParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonRemoveParams {
    pub id: String,
    /// Remove the daemon's data folder too.
    #[serde(default)]
    pub purge: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonLogsParams {
    pub id: String,
    #[serde(default)]
    pub tail: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonLogLine {
    pub at: i64,
    /// `stdout`, `stderr`, or `hub` for Hub's own lines about the process.
    pub stream: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonLogs {
    pub lines: Vec<DaemonLogLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInstallProgress {
    /// `runtime` (Bun is being downloaded) or `installing`.
    pub stage: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeParams {
    #[serde(default)]
    pub install: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub version: String,
    pub installed: bool,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProgress {
    pub done: u64,
    pub total: Option<u64>,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// The copy of an applet a device holds, from its last apps report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledCopy {
    pub version: String,
    pub hash: String,
}

/// One applet the store offers, at its newest listed version (§2 StoreApp).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreApp {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub author: String,
    pub version: String,
    /// Published with the store's official key.
    pub official: bool,
    /// The device targets the package has a variant for.
    pub targets: Vec<String>,
    /// The package's size.
    pub bytes: u64,
    /// The icon's absolute path in Hub's cache; null until Hub has fetched it.
    pub icon: Option<String>,
    /// Hub holds the verified package. A device's own store page lists an
    /// applet only once this is true.
    pub cached: bool,
    pub services: Vec<String>,
    pub hosts: Vec<String>,
    /// Only in an answer for a device: absent without one, null when the
    /// device holds no copy.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "present_or_null")]
    pub installed: Option<Option<InstalledCopy>>,
    /// Only in an answer for a device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<bool>,
}

/// A member that is absent (`None`), null (`Some(None)`) or a value.
mod present_or_null {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreAppsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreApps {
    pub apps: Vec<StoreApp>,
    /// Epoch milliseconds of the catalog's fetch.
    pub fetched_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreDetailsParams {
    /// The applet's id.
    pub app: String,
}

/// What an applet's package says about it, as the store reads it out (§2
/// StoreDetails): of the newest listed version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreDetails {
    pub id: String,
    pub version: String,
    /// The devices the package was built for, in the store's order.
    pub devices: Vec<BuiltFor>,
    /// The capabilities it needs of a device, by id.
    pub needs: Vec<String>,
    /// The ones it uses where a device has them.
    pub uses: Vec<String>,
    /// Null when the package names no surface.
    pub surface: Option<AppSurface>,
    pub framework: Option<String>,
    /// The applet API's version.
    pub api: Option<u32>,
}

/// A device a package was built for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltFor {
    pub target: String,
    /// The store's name for the device, the target for one it has no name for.
    pub name: String,
    /// Null when the store names no build.
    pub host_abi: Option<u32>,
}

/// The surface an applet lays itself out for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSurface {
    /// A touch surface of its own, not the device's screen.
    pub touch: bool,
    /// The sizes it is laid out for, in points; null when it names none.
    pub size: Option<SurfaceSize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSize {
    /// `[width, height]`.
    pub min: [u32; 2],
    pub max: [u32; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreInstallParams {
    pub device: String,
    /// The applet's id.
    pub app: String,
}

// ---------------------------------------------------------------------------
// Events (§4)
// ---------------------------------------------------------------------------

/// The `params` of an `event` notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "topic")]
pub enum Event {
    #[serde(rename = "device.changed")]
    DeviceChanged { device: Box<Device> },
    #[serde(rename = "device.removed")]
    DeviceRemoved { id: String },
    #[serde(rename = "device.unlinked")]
    DeviceUnlinked {
        id: String,
        /// The link close code's name (`going_away`, `replaced`, `silence`,
        /// …), or `lost` when the carrier ended without a `CLOSE`.
        code: String,
        reason: String,
    },
    #[serde(rename = "device.log")]
    DeviceLog {
        id: String,
        level: String,
        source: String,
        text: String,
        frame: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dropped: Option<u64>,
    },
    #[serde(rename = "device.screenshot")]
    DeviceScreenshot { id: String, path: String, width: u32, height: u32, frame: u64 },
    /// The requests that wait for a code changed: the whole list.
    #[serde(rename = "pairing.changed")]
    PairingChanged { requests: Vec<PairRequest> },
    #[serde(rename = "daemon.changed")]
    DaemonChanged { daemon: Daemon },
    #[serde(rename = "daemon.removed")]
    DaemonRemoved { id: String },
    #[serde(rename = "daemon.log")]
    DaemonLog { id: String, at: i64, stream: String, text: String },
    #[serde(rename = "hub.changed")]
    HubChanged { hub: HubInfo },
    /// The catalog Hub holds changed; a client calls `store.apps` again.
    #[serde(rename = "store.changed")]
    StoreChanged {
        #[serde(rename = "fetchedAt")]
        fetched_at: i64,
    },
}

impl Event {
    pub fn topic(&self) -> &'static str {
        use crate::topics::*;
        match self {
            Self::DeviceChanged { .. } => DEVICE_CHANGED,
            Self::DeviceRemoved { .. } => DEVICE_REMOVED,
            Self::DeviceUnlinked { .. } => DEVICE_UNLINKED,
            Self::DeviceLog { .. } => DEVICE_LOG,
            Self::DeviceScreenshot { .. } => DEVICE_SCREENSHOT,
            Self::PairingChanged { .. } => PAIRING_CHANGED,
            Self::DaemonChanged { .. } => DAEMON_CHANGED,
            Self::DaemonRemoved { .. } => DAEMON_REMOVED,
            Self::DaemonLog { .. } => DAEMON_LOG,
            Self::HubChanged { .. } => HUB_CHANGED,
            Self::StoreChanged { .. } => STORE_CHANGED,
        }
    }
}
