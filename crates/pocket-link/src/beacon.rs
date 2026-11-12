//! §2.2 Beacon: one 64-byte datagram a second from Hub to UDP 8627.
//!
//! ```text
//! off 0   u32     magic       'P','K','L','B'
//! off 4   u8      version     2
//! off 5   u8      flags       0 (version 1 used bit 0; a receiver ignores it)
//! off 6   u16     tcpPort     1..65535
//! off 8   u8[16]  companion   Hub's id, 16 lowercase hex digits
//! off 24  u8[40]  name        UTF-8, cut on a code point, NUL-padded
//! ```

use crate::consts::{BEACON_BYTES, BEACON_MAGIC, BEACON_NAME_BYTES, VERSION};
use crate::text::{is_link_id, utf8_prefix};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    pub port: u16,
    pub companion: String,
    pub name: String,
}

/// None when the id is not 16 lowercase hex digits or the port is 0.
pub fn encode(beacon: &Beacon) -> Option<[u8; BEACON_BYTES]> {
    if !is_link_id(&beacon.companion) || beacon.port == 0 {
        return None;
    }
    let mut out = [0u8; BEACON_BYTES];
    out[0..4].copy_from_slice(&BEACON_MAGIC.to_le_bytes());
    out[4] = VERSION;
    out[5] = 0;
    out[6..8].copy_from_slice(&beacon.port.to_le_bytes());
    out[8..24].copy_from_slice(beacon.companion.as_bytes());
    let name = utf8_prefix(&beacon.name, BEACON_NAME_BYTES).as_bytes();
    out[24..24 + name.len()].copy_from_slice(name);
    Some(out)
}

/// None for a datagram a version-2 receiver drops: not 64 bytes, another
/// magic or version, port 0, or an id that is not 16 lowercase hex digits.
/// The flags byte is ignored.
pub fn decode(bytes: &[u8]) -> Option<Beacon> {
    if bytes.len() != BEACON_BYTES {
        return None;
    }
    if u32::from_le_bytes(bytes[0..4].try_into().ok()?) != BEACON_MAGIC || bytes[4] != VERSION {
        return None;
    }
    let port = u16::from_le_bytes(bytes[6..8].try_into().ok()?);
    if port == 0 {
        return None;
    }
    let companion = std::str::from_utf8(&bytes[8..24]).ok()?;
    if !is_link_id(companion) {
        return None;
    }
    let name = &bytes[24..24 + BEACON_NAME_BYTES];
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    Some(Beacon {
        port,
        companion: companion.to_owned(),
        name: String::from_utf8_lossy(&name[..end]).into_owned(),
    })
}
