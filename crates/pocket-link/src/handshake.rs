//! §3.3 to §3.8: the handshake of a paired device.
//!
//! An HMAC-SHA256 handshake proves at every connection that both ends hold
//! the same 32-byte link key. No key travels on the link.

use bytes::Bytes;
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::consts::*;
use crate::frame::{self, EncodeError, FrameType, JsonFrame};
use crate::text::{encoded_bytes, encoded_prefix, is_key, is_link_id, is_nonce};

/// §3.4, device to Hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hello {
    pub t: &'static str,
    pub device: String,
    pub companion: String,
    pub nonce: String,
    pub name: String,
    pub model: String,
    pub target: String,
    #[serde(rename = "hostAbi")]
    pub host_abi: u32,
    pub features: Vec<String>,
    #[serde(rename = "maxPayload")]
    pub max_payload: u32,
}

/// §3.4, Hub to device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Challenge {
    pub t: &'static str,
    pub nonce: String,
    pub proof: String,
}

impl Challenge {
    pub fn new(nonce: String, proof: String) -> Self {
        Self { t: "challenge", nonce, proof }
    }
}

/// §3.4, device to Hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Auth {
    pub t: &'static str,
    pub proof: String,
}

/// Hub's wall clock at sending, and the same instant in its zone:
/// `local - utc` is its UTC offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Clock {
    pub utc: i64,
    pub local: i64,
}

/// §3.4, Hub to device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ready {
    pub t: &'static str,
    pub name: String,
    pub features: Vec<String>,
    #[serde(rename = "maxPayload")]
    pub max_payload: u32,
    pub clock: Clock,
}

impl Ready {
    pub fn new(name: &str, features: Vec<String>, max_payload: u32, clock: Clock) -> Self {
        Self { t: "ready", name: encoded_prefix(name, NAME_BYTES).to_owned(), features, max_payload, clock }
    }
}

/// §3.8
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    UnknownDevice,
    BadProof,
    Malformed,
    Busy,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::UnknownDevice => "unknown_device",
            Self::BadProof => "bad_proof",
            Self::Malformed => "malformed",
            Self::Busy => "busy",
        }
    }
}

/// §3.4, Hub to device, in place of `challenge` or of `ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refused {
    pub t: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl Refused {
    pub fn new(refusal: Refusal, message: &str) -> Self {
        Self { t: "refused", code: refusal.code(), message: encoded_prefix(message, REFUSED_MESSAGE_BYTES).to_owned() }
    }
}

/// A HANDSHAKE frame. A message over LINK_MAX_HANDSHAKE is the caller's error.
pub fn encode<T: Serialize>(message: &T) -> Result<Bytes, EncodeError> {
    frame::encode_json(FrameType::Handshake, 0, message, JsonFrame::default(), MAX_HANDSHAKE)
}

// ---------------------------------------------------------------------------
// §3.5 HMAC inputs
// ---------------------------------------------------------------------------

/// The label, the nonces as sent and the ids as sent, joined by `\n`.
pub fn proof_message(label: &str, device_nonce: &str, hub_nonce: &str, device: &str, companion: &str) -> String {
    [label, device_nonce, hub_nonce, device, companion].join("\n")
}

/// HMAC-SHA256 keyed with the raw 32 bytes of the link key, as 64 lowercase
/// hex digits. None when `key_hex` is not a link key.
pub fn link_hmac(key_hex: &str, message: &str) -> Option<String> {
    if !is_key(key_hex) {
        return None;
    }
    let key = hex::decode(key_hex).ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).ok()?;
    mac.update(message.as_bytes());
    Some(hex::encode(mac.finalize().into_bytes()))
}

/// Hub's proof, sent in `challenge`.
pub fn companion_proof(key: &str, device_nonce: &str, hub_nonce: &str, device: &str, companion: &str) -> Option<String> {
    link_hmac(key, &proof_message(LABEL_COMPANION, device_nonce, hub_nonce, device, companion))
}

/// The device's proof, sent in `auth`.
pub fn console_proof(key: &str, device_nonce: &str, hub_nonce: &str, device: &str, companion: &str) -> Option<String> {
    link_hmac(key, &proof_message(LABEL_CONSOLE, device_nonce, hub_nonce, device, companion))
}

/// Constant-time comparison of two proofs as sent.
pub fn proof_equals(a: &str, b: &str) -> bool {
    is_key(a) && is_key(b) && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

fn valid_payload_offer(value: u64) -> bool {
    (MIN_PAYLOAD as u64..=PAYLOAD_CEILING as u64).contains(&value)
}

/// The payload limit of a connection (§4.4): the smaller of the two offers.
pub fn payload_limit(hello_max: u32, ready_max: u32) -> u32 {
    hello_max.min(ready_max)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A JSON number that is an integer in `0..=max`, as `Number.isInteger` reads
/// it: `12` and `12.0` both pass.
fn integer(value: Option<&Value>, max: u64) -> Option<u64> {
    let number = value?.as_number()?;
    let n = match number.as_u64() {
        Some(n) => n,
        None => {
            let f = number.as_f64()?;
            if f < 0.0 || f.fract() != 0.0 || f > max as f64 {
                return None;
            }
            f as u64
        }
    };
    (n <= max).then_some(n)
}

fn string(value: Option<&Value>, max: usize) -> Option<&str> {
    let text = value?.as_str()?;
    (encoded_bytes(text) <= max).then_some(text)
}

/// A `features` list within its limits (§6.1): for hello, ready and the
/// `features` event, whose receiver closes with `protocol` on None.
pub fn parse_features(value: Option<&Value>) -> Option<Vec<String>> {
    let list = value?.as_array()?;
    if list.len() > MAX_FEATURES {
        return None;
    }
    let mut out = Vec::with_capacity(list.len());
    for entry in list {
        let text = entry.as_str()?;
        if text.is_empty() || encoded_bytes(text) > FEATURE_BYTES {
            return None;
        }
        out.push(text.to_owned());
    }
    Some(out)
}

/// Hub's reading of a `hello` object: the message, or the field that makes it
/// `malformed` (§3.8): missing, of the wrong type or over its limit.
pub fn parse_hello(object: Option<&Map<String, Value>>) -> Result<Hello, &'static str> {
    let object = object.ok_or("not a hello")?;
    if object.get("t").and_then(Value::as_str) != Some("hello") {
        return Err("not a hello");
    }
    let id = |key: &'static str| object.get(key).and_then(Value::as_str).filter(|v| is_link_id(v)).ok_or(key);
    let device = id("device")?;
    let companion = id("companion")?;
    let nonce = object.get("nonce").and_then(Value::as_str).filter(|v| is_nonce(v)).ok_or("nonce")?;
    let name = string(object.get("name"), NAME_BYTES).ok_or("name")?;
    let model = string(object.get("model"), MODEL_BYTES).ok_or("model")?;
    let target = string(object.get("target"), TARGET_BYTES).filter(|v| !v.is_empty()).ok_or("target")?;
    let host_abi = integer(object.get("hostAbi"), u32::MAX as u64).ok_or("hostAbi")?;
    let features = parse_features(object.get("features")).ok_or("features")?;
    let max_payload = integer(object.get("maxPayload"), u32::MAX as u64).filter(|&v| valid_payload_offer(v)).ok_or("maxPayload")?;
    Ok(Hello {
        t: "hello",
        device: device.to_owned(),
        companion: companion.to_owned(),
        nonce: nonce.to_owned(),
        name: name.to_owned(),
        model: model.to_owned(),
        target: target.to_owned(),
        host_abi: host_abi as u32,
        features,
        max_payload: max_payload as u32,
    })
}

/// Hub's reading of an `auth` object: its proof, or None.
pub fn parse_auth(object: Option<&Map<String, Value>>) -> Option<String> {
    let object = object?;
    if object.get("t").and_then(Value::as_str) != Some("auth") {
        return None;
    }
    object.get("proof").and_then(Value::as_str).filter(|v| is_key(v)).map(str::to_owned)
}
