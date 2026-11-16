//! The JSON-RPC 2.0 error object and the kinds of §1 "Errors".

use serde::{Deserialize, Serialize};

use crate::types::Holder;

/// JSON-RPC's code for a line that is not JSON.
pub const PARSE_ERROR: i64 = -32700;
/// A malformed call, and any request before `hub.hello`.
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
/// Every failure of the operation itself; `data.kind` says which.
pub const OPERATION_FAILED: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    NotLinked,
    Busy,
    Link,
    Cancelled,
    Setup,
    Invalid,
    Daemon,
    Store,
    /// The release of a device's program could not be used.
    Release,
    /// A pairing by a code did not complete.
    Pairing,
    Internal,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::NotLinked => "not_linked",
            Self::Busy => "busy",
            Self::Link => "link",
            Self::Cancelled => "cancelled",
            Self::Setup => "setup",
            Self::Invalid => "invalid",
            Self::Daemon => "daemon",
            Self::Store => "store",
            Self::Release => "release",
            Self::Pairing => "pairing",
            Self::Internal => "internal",
        }
    }
}

/// `error.data` of an operation's failure. Only the members of its kind are
/// present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorData {
    pub kind: ErrorKind,
    /// `busy`: who holds the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<Holder>,
    /// `link`: the link's error code, or `closed`, `timeout`; `unsupported`
    /// also for an operation the device's `supports` does not name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// `link`: the phase the device gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// `setup`: the stage it failed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// `setup`: `unreachable`, `companions_full`, `release_unavailable`,
    /// `storage` or `card`. `store`: `unreachable` or `rejected`.
    /// `release`: `unreachable`, `not_found` or `rejected`.
    /// `pairing`: `wrong_code`, `bad_proof` or `gone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `setup`: the failing step in the setup's own words, for a log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `daemon`: the daemon's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `setup`: the device a key was stored for, with `confirmed: false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<bool>,
}

impl ErrorData {
    pub fn new(kind: ErrorKind) -> Self {
        Self { kind, holder: None, code: None, phase: None, step: None, reason: None, detail: None, id: None, device: None, confirmed: None }
    }
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct RpcError {
    pub code: i64,
    /// One English sentence for the owner.
    pub message: String,
    /// Boxed: an error travels through every `Result` of a call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<ErrorData>>,
}

impl RpcError {
    pub fn protocol(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::protocol(INVALID_REQUEST, message)
    }
    pub fn method_not_found(method: &str) -> Self {
        Self::protocol(METHOD_NOT_FOUND, format!("There is no method {method}."))
    }
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::protocol(INVALID_PARAMS, message)
    }
    /// A failure of the operation itself.
    pub fn op(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { code: OPERATION_FAILED, message: message.into(), data: Some(Box::new(ErrorData::new(kind))) }
    }
    pub fn with(mut self, change: impl FnOnce(&mut ErrorData)) -> Self {
        if let Some(data) = &mut self.data {
            change(data);
        }
        self
    }
    /// The operation kind, None for a protocol error.
    pub fn kind(&self) -> Option<ErrorKind> {
        self.data.as_ref().map(|data| data.kind)
    }
}
