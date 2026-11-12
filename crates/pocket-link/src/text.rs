//! Sizes of strings as the wire counts them, and the id formats.
//!
//! Every size limit of a JSON field counts the bytes of its encoded form in
//! the frame, escapes included (§4.4). `serde_json` escapes the same set as
//! the reference codec's `JSON.stringify`: `"`, `\` and the control characters
//! below U+0020.

use rand::RngCore;

use crate::consts::{ID_HEX, KEY_BYTES, NONCE_BYTES};

/// Bytes a string takes inside a JSON frame: its encoded form without quotes.
pub fn encoded_bytes(value: &str) -> usize {
    let mut bytes = 0;
    for ch in value.chars() {
        bytes += encoded_char_bytes(ch);
    }
    bytes
}

fn encoded_char_bytes(ch: char) -> usize {
    match ch {
        '"' | '\\' | '\u{08}' | '\u{0c}' | '\n' | '\r' | '\t' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// Bytes of a value's JSON as this codec writes it.
pub fn json_bytes<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map(|v| v.len()).unwrap_or(usize::MAX)
}

/// Longest prefix of `text` whose UTF-8 encoding fits `max` bytes, cut on a
/// code point.
pub fn utf8_prefix(text: &str, max: usize) -> &str {
    let mut end = 0;
    for (at, ch) in text.char_indices() {
        if at + ch.len_utf8() > max {
            break;
        }
        end = at + ch.len_utf8();
    }
    &text[..end]
}

/// Longest prefix of `text` whose encoded form (§4.4) fits `max` bytes.
pub fn encoded_prefix(text: &str, max: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    for (at, ch) in text.char_indices() {
        let length = encoded_char_bytes(ch);
        if used + length > max {
            break;
        }
        used += length;
        end = at + ch.len_utf8();
    }
    &text[..end]
}

fn is_lower_hex(value: &str, digits: usize) -> bool {
    value.len() == digits && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A device or Hub id: 16 lowercase hex digits.
pub fn is_link_id(value: &str) -> bool {
    is_lower_hex(value, ID_HEX)
}
/// A nonce as sent: 32 lowercase hex digits.
pub fn is_nonce(value: &str) -> bool {
    is_lower_hex(value, NONCE_BYTES * 2)
}
/// A link key or a proof as sent: 64 lowercase hex digits.
pub fn is_key(value: &str) -> bool {
    is_lower_hex(value, KEY_BYTES * 2)
}
/// A 64-bit hash as sent: 16 lowercase hex digits.
pub fn is_hash(value: &str) -> bool {
    is_lower_hex(value, 16)
}

fn random_hex(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut raw);
    hex::encode(raw)
}

pub fn new_link_id() -> String {
    random_hex(ID_HEX / 2)
}
pub fn new_nonce() -> String {
    random_hex(NONCE_BYTES)
}
pub fn new_key() -> String {
    random_hex(KEY_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_bytes_match_serde_json() {
        for text in ["plain", "quo\"te", "back\\slash", "line\nbreak", "\u{01}", "\u{7f}", "é漢🙂", "\u{2028}", "a/b"] {
            assert_eq!(encoded_bytes(text), serde_json::to_string(text).unwrap().len() - 2, "{text:?}");
        }
    }

    #[test]
    fn prefixes_cut_on_code_points() {
        assert_eq!(utf8_prefix("aé", 2), "a");
        assert_eq!(utf8_prefix("aé", 3), "aé");
        assert_eq!(encoded_prefix("a\nb", 2), "a");
        assert_eq!(encoded_prefix("a\nb", 3), "a\n");
    }
}
