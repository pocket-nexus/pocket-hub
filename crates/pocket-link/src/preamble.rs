//! §4.1 Preamble and version negotiation: 8 bytes from each side before its
//! first frame, the device first.
//!
//! ```text
//! off 0  u32  magic     'P','K','L','K'
//! off 4  u8   highest   the highest version the sender speaks
//! off 5  u8   lowest    the lowest version the sender speaks
//! off 6  u8   selected  device: 0. Hub: the version in use, or 0 for none
//! off 7  u8   reserved  0
//! ```

use crate::consts::{MAGIC, PREAMBLE_BYTES, VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preamble {
    pub highest: u8,
    pub lowest: u8,
    pub selected: u8,
}

/// The versions one side speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub highest: u8,
    pub lowest: u8,
}

/// What this Hub speaks: version 2 alone.
pub const HUB_RANGE: Range = Range { highest: VERSION, lowest: VERSION };

pub fn encode(preamble: Preamble) -> [u8; PREAMBLE_BYTES] {
    let mut out = [0u8; PREAMBLE_BYTES];
    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4] = preamble.highest;
    out[5] = preamble.lowest;
    out[6] = preamble.selected;
    out
}

/// None when the 8 bytes do not start with the magic. The reserved byte is not
/// checked: a later version may use it, and this one keeps the rules.
pub fn decode(bytes: &[u8]) -> Option<Preamble> {
    if bytes.len() != PREAMBLE_BYTES || u32::from_le_bytes(bytes[0..4].try_into().ok()?) != MAGIC {
        return None;
    }
    Some(Preamble { highest: bytes[4], lowest: bytes[5], selected: bytes[6] })
}

/// Hub's choice: the highest version inside both ranges, or 0. A version-1
/// console's preamble (PKLK, 1, 0, 0, 0) reads as highest 1, lowest 0.
pub fn select_version(hub: Range, device: Preamble) -> u8 {
    let top = hub.highest.min(device.highest);
    let bottom = hub.lowest.max(device.lowest).max(1);
    if top >= bottom { top } else { 0 }
}

/// Hub's answer to a device's preamble: the bytes to send and the version in
/// use, or 0 when Hub closes after sending them.
pub fn hub_answer(hub: Range, device: Preamble) -> ([u8; PREAMBLE_BYTES], u8) {
    let selected = select_version(hub, device);
    (encode(Preamble { highest: hub.highest, lowest: hub.lowest, selected }), selected)
}

/// The device's check of Hub's preamble: the version to use, or None to close.
pub fn accept_selection(device: Range, hub: Preamble) -> Option<u8> {
    if hub.selected == 0 || hub.selected < device.lowest || hub.selected > device.highest {
        return None;
    }
    Some(hub.selected)
}
