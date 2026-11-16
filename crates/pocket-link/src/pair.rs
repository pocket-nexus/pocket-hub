//! §3.11: pairing by a code the device shows.
//!
//! A device with a screen, and no storage Hub can write through, sends `pair`
//! in place of `hello`. Hub's owner enters the 6 digits the device shows; Hub
//! mints a link key and sends it in `pair.offer`, masked and proved under
//! that code. The device answers `pair.accept` with a proof keyed with the
//! link key, and Hub stores the record then. The pairing this leaves is the
//! one of §3.1, and every later connection is the handshake of §3.3.
//!
//! ```text
//! pair.offer.proof  = HMAC(code,    "pocket-link/2/pair/companion" \n nD \n nC \n device \n companion)
//! pair.offer.key    = linkKey XOR HMAC(code, "pocket-link/2/pair/key" \n nD \n nC \n device \n companion)
//! pair.accept.proof = HMAC(linkKey, "pocket-link/2/pair/console"   \n nD \n nC \n device \n companion)
//! ```
//!
//! **The key of the first two HMACs is the code's 6 digits as ASCII bytes**;
//! the third is keyed with the raw 32 bytes of the link key, as the proofs of
//! §3.5 are. No key appears in a log line: nothing here formats one.

use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::Sha256;

use crate::consts::*;
use crate::handshake::{link_hmac, proof_message, string};
use crate::text::{encoded_prefix, is_key, is_link_id, is_nonce};

/// Device to Hub, in place of `hello`: the device asks to be paired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pair {
    pub t: &'static str,
    pub device: String,
    pub nonce: String,
    pub name: String,
    pub model: String,
    pub target: String,
}

/// Hub to device, once Hub's owner entered the code. `key` is the link key
/// XOR HMAC(code, the `pair/key` message): 64 hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairOffer {
    pub t: &'static str,
    pub companion: String,
    pub name: String,
    pub nonce: String,
    pub proof: String,
    pub key: String,
}

impl PairOffer {
    /// The offer of `key` under `code`, from Hub `companion` named `name`, to
    /// the device that sent `pair` (its id and nonce) and with Hub's own
    /// `nonce`. None when `code` is not a code or `key` not a link key.
    pub fn new(code: &str, key: &str, pair: &Pair, companion: &str, name: &str, nonce: &str) -> Option<Self> {
        Some(Self {
            t: "pair.offer",
            companion: companion.to_owned(),
            name: encoded_prefix(name, NAME_BYTES).to_owned(),
            nonce: nonce.to_owned(),
            proof: companion_proof(code, &pair.nonce, nonce, &pair.device, companion)?,
            key: mask_key(code, key, &pair.nonce, nonce, &pair.device, companion)?,
        })
    }
}

/// Device to Hub: the proof that it unmasked the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairAccept {
    pub t: &'static str,
    pub proof: String,
}

/// Hub to device, after it stored the record; Hub closes behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairDone {
    pub t: &'static str,
}

impl PairDone {
    pub fn new() -> Self {
        Self { t: "pair.done" }
    }
}

impl Default for PairDone {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// The code and its HMACs
// ---------------------------------------------------------------------------

/// A code as a device shows it: exactly 6 ASCII decimal digits. Digits of
/// another script are not a code: the device keys its HMAC with the ASCII
/// bytes it shows.
pub fn is_code(value: &str) -> bool {
    value.len() == PAIR_CODE_DIGITS && value.bytes().all(|b| b.is_ascii_digit())
}

/// HMAC-SHA256 keyed with the code's digits as ASCII bytes.
fn code_hmac(code: &str, message: &str) -> Option<[u8; KEY_BYTES]> {
    if !is_code(code) {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(code.as_bytes()).ok()?;
    mac.update(message.as_bytes());
    Some(mac.finalize().into_bytes().into())
}

/// `pair.offer.proof`: Hub's proof that the code its owner entered is the
/// one the device shows.
pub fn companion_proof(code: &str, device_nonce: &str, hub_nonce: &str, device: &str, companion: &str) -> Option<String> {
    code_hmac(code, &proof_message(LABEL_PAIR_COMPANION, device_nonce, hub_nonce, device, companion)).map(hex::encode)
}

/// `pair.offer.key`: the link key XOR HMAC(code, …). The mask is its own
/// inverse, so the device unmasks the key with the same call.
pub fn mask_key(code: &str, key_hex: &str, device_nonce: &str, hub_nonce: &str, device: &str, companion: &str) -> Option<String> {
    if !is_key(key_hex) {
        return None;
    }
    let mask = code_hmac(code, &proof_message(LABEL_PAIR_KEY, device_nonce, hub_nonce, device, companion))?;
    let mut key = hex::decode(key_hex).ok()?;
    for (byte, mask) in key.iter_mut().zip(mask) {
        *byte ^= mask;
    }
    Some(hex::encode(key))
}

/// `pair.accept.proof`: the device's proof that it holds the link key.
pub fn console_proof(key: &str, device_nonce: &str, hub_nonce: &str, device: &str, companion: &str) -> Option<String> {
    link_hmac(key, &proof_message(LABEL_PAIR_CONSOLE, device_nonce, hub_nonce, device, companion))
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

fn text<'a>(object: &'a Map<String, Value>, key: &str, valid: impl Fn(&str) -> bool) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str).filter(|value| valid(value))
}

/// Hub's reading of a `pair` object: the message, or the field that makes it
/// `malformed` (§3.8). The fields have the limits of `hello` (§3.4).
pub fn parse_pair(object: Option<&Map<String, Value>>) -> Result<Pair, &'static str> {
    let object = object.ok_or("not a pair")?;
    if object.get("t").and_then(Value::as_str) != Some("pair") {
        return Err("not a pair");
    }
    let device = text(object, "device", is_link_id).ok_or("device")?;
    let nonce = text(object, "nonce", is_nonce).ok_or("nonce")?;
    let name = string(object.get("name"), NAME_BYTES).ok_or("name")?;
    let model = string(object.get("model"), MODEL_BYTES).ok_or("model")?;
    let target = string(object.get("target"), TARGET_BYTES).ok_or("target")?;
    Ok(Pair { t: "pair", device: device.to_owned(), nonce: nonce.to_owned(), name: name.to_owned(), model: model.to_owned(), target: target.to_owned() })
}

/// The device's reading of a `pair.offer` object, or None.
pub fn parse_pair_offer(object: Option<&Map<String, Value>>) -> Option<PairOffer> {
    let object = object?;
    if object.get("t").and_then(Value::as_str) != Some("pair.offer") {
        return None;
    }
    Some(PairOffer {
        t: "pair.offer",
        companion: text(object, "companion", is_link_id)?.to_owned(),
        name: string(object.get("name"), NAME_BYTES)?.to_owned(),
        nonce: text(object, "nonce", is_nonce)?.to_owned(),
        proof: text(object, "proof", is_key)?.to_owned(),
        key: text(object, "key", is_key)?.to_owned(),
    })
}

/// Hub's reading of a `pair.accept` object: its proof, or None.
pub fn parse_pair_accept(object: Option<&Map<String, Value>>) -> Option<String> {
    let object = object?;
    if object.get("t").and_then(Value::as_str) != Some("pair.accept") {
        return None;
    }
    text(object, "proof", is_key).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_is_its_own_inverse() {
        let key = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let (nd, nc) = ("a0a1a2a3a4a5a6a7a8a9aaabacadaeaf", "b0b1b2b3b4b5b6b7b8b9babbbcbdbebf");
        let masked = mask_key("314159", key, nd, nc, "0123456789abcdef", "fedcba9876543210").unwrap();
        assert_ne!(masked, key);
        assert_eq!(mask_key("314159", &masked, nd, nc, "0123456789abcdef", "fedcba9876543210").unwrap(), key);
        // Another code unmasks another key.
        assert_ne!(mask_key("271828", &masked, nd, nc, "0123456789abcdef", "fedcba9876543210").unwrap(), key);
        assert!(mask_key("31415", key, nd, nc, "0123456789abcdef", "fedcba9876543210").is_none());
        assert!(mask_key("314159", "not a key", nd, nc, "0123456789abcdef", "fedcba9876543210").is_none());
    }
}
