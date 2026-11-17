//! §4.2 to §4.8: frames.
//!
//! ```text
//! off 0  u8   type
//! off 1  u8   flags     per type; bit 7 reserved for encryption
//! off 2  u16  reserved  0
//! off 4  u32  length    payload bytes
//! off 8  u32  id        request, stream or channel id; 0 where the type says so
//! off 12 ...  payload
//! ```

use bytes::{BufMut, Bytes, BytesMut};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::consts::*;
use crate::text::utf8_prefix;

/// §4.3
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameType {
    Ping = 0x01,
    Pong = 0x02,
    Close = 0x03,
    Handshake = 0x04,
    Request = 0x10,
    Reply = 0x11,
    Event = 0x12,
    Cancel = 0x13,
    Data = 0x20,
    Credit = 0x21,
    Reset = 0x22,
}

impl FrameType {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0x01 => Self::Ping,
            0x02 => Self::Pong,
            0x03 => Self::Close,
            0x04 => Self::Handshake,
            0x10 => Self::Request,
            0x11 => Self::Reply,
            0x12 => Self::Event,
            0x13 => Self::Cancel,
            0x20 => Self::Data,
            0x21 => Self::Credit,
            0x22 => Self::Reset,
            _ => return None,
        })
    }

    /// The flag bits this type defines in a dialect.
    fn known_flags(self, dialect: Dialect) -> u8 {
        match (self, dialect) {
            (Self::Request, Dialect::Wire) => FLAG_APP | FLAG_BODY,
            (Self::Request, Dialect::Local) => FLAG_BODY,
            (Self::Reply, _) => FLAG_ERROR,
            (Self::Event, Dialect::Wire) => FLAG_APP,
            (Self::Data, _) => FLAG_END,
            _ => 0,
        }
    }

    fn before_ready(self) -> bool {
        matches!(self, Self::Ping | Self::Pong | Self::Close | Self::Handshake)
    }
}

/// REPLY: the payload is an error object.
pub const FLAG_ERROR: u8 = 0x01;
/// DATA: the last frame of a stream direction.
pub const FLAG_END: u8 = 0x01;
/// REQUEST, EVENT: a u32 channel id precedes the JSON object (§7.2).
pub const FLAG_APP: u8 = 0x02;
/// REQUEST with APP: the requester's bytes follow on the request id.
pub const FLAG_BODY: u8 = 0x04;

/// §4.8
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum CloseCode {
    GoingAway = 1,
    Protocol = 2,
    TooLarge = 3,
    Silence = 4,
    Flow = 5,
    Auth = 6,
    Replaced = 7,
    Forgotten = 8,
    Restart = 9,
    HandshakeTimeout = 11,
}

impl CloseCode {
    /// A receiver treats an unknown code as `going_away`.
    pub fn from_u16(value: u16) -> Self {
        match value {
            2 => Self::Protocol,
            3 => Self::TooLarge,
            4 => Self::Silence,
            5 => Self::Flow,
            6 => Self::Auth,
            7 => Self::Replaced,
            8 => Self::Forgotten,
            9 => Self::Restart,
            11 => Self::HandshakeTimeout,
            _ => Self::GoingAway,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::GoingAway => "going_away",
            Self::Protocol => "protocol",
            Self::TooLarge => "too_large",
            Self::Silence => "silence",
            Self::Flow => "flow",
            Self::Auth => "auth",
            Self::Replaced => "replaced",
            Self::Forgotten => "forgotten",
            Self::Restart => "restart",
            Self::HandshakeTimeout => "handshake_timeout",
        }
    }
}

/// §5.6
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum ResetCode {
    Cancelled = 1,
    Refused = 2,
    Length = 3,
    Storage = 4,
    Superseded = 5,
}

impl ResetCode {
    pub fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::Cancelled,
            2 => Self::Refused,
            3 => Self::Length,
            4 => Self::Storage,
            5 => Self::Superseded,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Refused => "refused",
            Self::Length => "length",
            Self::Storage => "storage",
            Self::Superseded => "superseded",
        }
    }
}

/// The code a receiver closes the connection with, and why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {reason}", code.name())]
pub struct LinkClose {
    pub code: CloseCode,
    pub reason: String,
}

impl LinkClose {
    pub fn new(code: CloseCode, reason: impl Into<String>) -> Self {
        Self { code, reason: reason.into() }
    }
    pub fn protocol(reason: impl Into<String>) -> Self {
        Self::new(CloseCode::Protocol, reason)
    }
}

/// A frame a sender must not put on the wire.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct EncodeError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub ty: FrameType,
    pub flags: u8,
    pub id: u32,
    pub payload: Bytes,
}

/// Which side allocated an id (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Device,
    Hub,
}

/// None for 0 and 0x80000000, which are in neither range.
pub fn id_range(id: u32) -> Option<Side> {
    if id == 0 || id == ID_HUB_BIT {
        None
    } else if id > ID_DEVICE_MAX {
        Some(Side::Hub)
    } else {
        Some(Side::Device)
    }
}

/// Which connection a frame travels on.
///
/// `Wire` is a device's link, as the specification defines it. `Local` is the
/// connection between Hub and a service for one applet channel
/// (docs/SERVICE-PROTOCOL.md): the same header and frame types, but the
/// connection is the channel, so no frame carries the `APP` flag or a channel
/// id, a `REQUEST` may carry `BODY` by itself, and a `HANDSHAKE` frame is
/// valid at any time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Wire,
    Local,
}

/// The header and payload-size checks of §4.2 and §4.7 for a known type.
pub fn check_frame(ty: FrameType, flags: u8, id: u32, length: u32) -> Result<(), LinkClose> {
    check_frame_in(Dialect::Wire, ty, flags, id, length)
}

/// `check_frame` for a dialect.
pub fn check_frame_in(dialect: Dialect, ty: FrameType, flags: u8, id: u32, length: u32) -> Result<(), LinkClose> {
    if flags & !ty.known_flags(dialect) != 0 {
        return Err(LinkClose::protocol(format!("frame {ty:?}: undefined flag")));
    }
    if dialect == Dialect::Wire && ty == FrameType::Request && flags & FLAG_BODY != 0 && flags & FLAG_APP == 0 {
        return Err(LinkClose::protocol("REQUEST: BODY without APP"));
    }
    let named = id_range(id).is_some();
    match ty {
        FrameType::Ping | FrameType::Pong => {
            if id != 0 {
                return Err(LinkClose::protocol("PING/PONG: id is not 0"));
            }
            if length != PING_BYTES {
                return Err(LinkClose::protocol("PING/PONG: payload is not 4 bytes"));
            }
        }
        FrameType::Close => {
            if id != 0 {
                return Err(LinkClose::protocol("CLOSE: id is not 0"));
            }
            if length < 4 || length as usize > 4 + CLOSE_REASON_BYTES {
                return Err(LinkClose::protocol("CLOSE: bad payload size"));
            }
        }
        FrameType::Handshake => {
            if id != 0 {
                return Err(LinkClose::protocol("HANDSHAKE: id is not 0"));
            }
            if length > MAX_HANDSHAKE {
                return Err(LinkClose::new(CloseCode::TooLarge, "HANDSHAKE: over 1,024 bytes"));
            }
        }
        FrameType::Request | FrameType::Reply | FrameType::Cancel => {
            if !named {
                return Err(LinkClose::protocol("request id is 0 or 0x80000000"));
            }
            if ty == FrameType::Cancel && length != 0 {
                return Err(LinkClose::protocol("CANCEL: payload is not empty"));
            }
        }
        FrameType::Event => {
            if id == ID_HUB_BIT {
                return Err(LinkClose::protocol("EVENT: id 0x80000000"));
            }
        }
        FrameType::Data => {
            if !named {
                return Err(LinkClose::protocol("DATA: stream id is 0 or 0x80000000"));
            }
            if length == 0 && flags & FLAG_END == 0 {
                return Err(LinkClose::protocol("DATA: 0 bytes without END"));
            }
        }
        FrameType::Credit | FrameType::Reset => {
            if !named {
                return Err(LinkClose::protocol("CREDIT/RESET: id is 0 or 0x80000000"));
            }
            if length != 4 {
                return Err(LinkClose::protocol("CREDIT/RESET: payload is not 4 bytes"));
            }
        }
    }
    if matches!(ty, FrameType::Request | FrameType::Event) && flags & FLAG_APP != 0 && length < APP_CHANNEL_PREFIX {
        return Err(LinkClose::protocol("APP frame without its channel id"));
    }
    Ok(())
}

fn put_header(out: &mut BytesMut, ty: u8, flags: u8, length: u32, id: u32) {
    out.put_u8(ty);
    out.put_u8(flags);
    out.put_u16_le(0);
    out.put_u32_le(length);
    out.put_u32_le(id);
}

/// Encodes one frame. A payload over `limit`, or a frame that breaks its
/// type's rules, is an `EncodeError`: a sender never puts one on the wire.
pub fn encode(ty: FrameType, flags: u8, id: u32, payload: &[u8], limit: u32) -> Result<Bytes, EncodeError> {
    encode_in(Dialect::Wire, ty, flags, id, payload, limit)
}

/// `encode` for a dialect.
pub fn encode_in(dialect: Dialect, ty: FrameType, flags: u8, id: u32, payload: &[u8], limit: u32) -> Result<Bytes, EncodeError> {
    if payload.len() > limit as usize {
        return Err(EncodeError(format!("frame payload {} over the limit {limit}", payload.len())));
    }
    check_frame_in(dialect, ty, flags, id, payload.len() as u32).map_err(|close| EncodeError(close.reason))?;
    if ty == FrameType::Credit && payload == [0, 0, 0, 0] {
        return Err(EncodeError("CREDIT of 0".into()));
    }
    let mut out = BytesMut::with_capacity(HEADER_BYTES + payload.len());
    put_header(&mut out, ty as u8, flags, payload.len() as u32, id);
    out.put_slice(payload);
    Ok(out.freeze())
}

fn encode_fixed(ty: FrameType, id: u32, payload: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(HEADER_BYTES + payload.len());
    put_header(&mut out, ty as u8, 0, payload.len() as u32, id);
    out.put_slice(payload);
    out.freeze()
}

pub fn encode_ping(counter: u32) -> Bytes {
    encode_fixed(FrameType::Ping, 0, &counter.to_le_bytes())
}

/// The PONG that answers a PING: the same 4 bytes.
pub fn encode_pong(ping_payload: &[u8]) -> Bytes {
    encode_fixed(FrameType::Pong, 0, &ping_payload[..4])
}

pub fn encode_close(code: CloseCode, reason: &str) -> Bytes {
    let text = utf8_prefix(reason, CLOSE_REASON_BYTES).as_bytes();
    let mut payload = Vec::with_capacity(4 + text.len());
    payload.extend_from_slice(&(code as u16).to_le_bytes());
    payload.extend_from_slice(&[0, 0]);
    payload.extend_from_slice(text);
    encode_fixed(FrameType::Close, 0, &payload)
}

pub fn parse_close(payload: &[u8]) -> (CloseCode, String) {
    let code = CloseCode::from_u16(u16::from_le_bytes([payload[0], payload[1]]));
    (code, String::from_utf8_lossy(&payload[4..]).into_owned())
}

/// Bytes on a stream id; units when `id` is an applet channel's (§5.6).
/// `amount` is at least 1.
pub fn encode_credit(id: u32, amount: u32) -> Bytes {
    debug_assert!(amount >= 1 && id_range(id).is_some());
    encode_fixed(FrameType::Credit, id, &amount.to_le_bytes())
}

pub fn parse_credit(payload: &[u8]) -> u32 {
    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]])
}

pub fn encode_reset(id: u32, code: ResetCode) -> Bytes {
    let mut payload = [0u8; 4];
    payload[0..2].copy_from_slice(&(code as u16).to_le_bytes());
    encode_fixed(FrameType::Reset, id, &payload)
}

/// None for a code this version does not define.
pub fn parse_reset(payload: &[u8]) -> Option<ResetCode> {
    ResetCode::from_u16(u16::from_le_bytes([payload[0], payload[1]]))
}

pub fn encode_cancel(id: u32) -> Bytes {
    encode_fixed(FrameType::Cancel, id, &[])
}

/// One DATA frame. `bytes` is at most the payload limit in force, and empty
/// only with `end`.
pub fn encode_data(id: u32, bytes: &[u8], end: bool) -> Bytes {
    debug_assert!(end || !bytes.is_empty());
    let mut out = BytesMut::with_capacity(HEADER_BYTES + bytes.len());
    put_header(&mut out, FrameType::Data as u8, if end { FLAG_END } else { 0 }, bytes.len() as u32, id);
    out.put_slice(bytes);
    out.freeze()
}

/// LINK_APP_MSG for a connection: min(8,192, payload limit − 4) (§7.2).
pub fn app_message_limit(payload_limit: u32) -> u32 {
    APP_MSG_CEILING.min(payload_limit.saturating_sub(APP_CHANNEL_PREFIX))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct JsonFrame {
    /// REPLY: the ERROR flag.
    pub error: bool,
    /// REQUEST, EVENT: the APP flag with this channel id before the object.
    pub channel: Option<u32>,
    /// REQUEST with a channel: a body follows on the request id.
    pub body: bool,
}

/// One JSON object as the vectors fix it: no whitespace, keys in the order of
/// the value. `limit` is the payload limit in force; with a channel the object
/// is checked against LINK_APP_MSG as the sender's own endpoint does (§7.2).
pub fn encode_json<T: Serialize>(ty: FrameType, id: u32, object: &T, options: JsonFrame, limit: u32) -> Result<Bytes, EncodeError> {
    let json = serde_json::to_vec(object).map_err(|error| EncodeError(error.to_string()))?;
    encode_json_bytes(ty, id, &json, options, limit)
}

/// As `encode_json`, for an object already encoded.
pub fn encode_json_bytes(ty: FrameType, id: u32, json: &[u8], options: JsonFrame, limit: u32) -> Result<Bytes, EncodeError> {
    let limit = if ty == FrameType::Handshake { MAX_HANDSHAKE } else { limit };
    let mut flags = 0;
    if options.error {
        flags |= FLAG_ERROR;
    }
    match options.channel {
        None => encode(ty, flags, id, json, limit),
        Some(channel) => {
            if json.len() > app_message_limit(limit) as usize {
                return Err(EncodeError(format!("too_large: applet object of {} bytes over {}", json.len(), app_message_limit(limit))));
            }
            flags |= FLAG_APP;
            if options.body {
                flags |= FLAG_BODY;
            }
            let mut payload = Vec::with_capacity(4 + json.len());
            payload.extend_from_slice(&channel.to_le_bytes());
            payload.extend_from_slice(json);
            encode(ty, flags, id, &payload, limit)
        }
    }
}

/// The JSON of a REQUEST, REPLY, EVENT or HANDSHAKE frame.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonPayload {
    /// The channel id of an APP frame.
    pub channel: Option<u32>,
    /// The JSON object; None when the bytes are not one JSON object.
    pub object: Option<Map<String, Value>>,
    /// The object's bytes as they came in the frame.
    pub raw: Bytes,
}

impl JsonPayload {
    /// The object's string `t`, when it has one.
    pub fn t(&self) -> Option<&str> {
        self.object.as_ref()?.get("t")?.as_str()
    }
}

/// Splits an APP frame's channel id from its object. `raw` is the object's
/// bytes, unparsed: Hub forwards an applet's object without reading it (§7.2).
pub fn split_app(frame: &Frame) -> (Option<u32>, Bytes) {
    if matches!(frame.ty, FrameType::Request | FrameType::Event) && frame.flags & FLAG_APP != 0 {
        let channel = u32::from_le_bytes([frame.payload[0], frame.payload[1], frame.payload[2], frame.payload[3]]);
        (Some(channel), frame.payload.slice(4..))
    } else {
        (None, frame.payload.clone())
    }
}

/// Parses a frame's JSON object. A payload that is not one JSON object gives
/// `object: None`: the receiver answers a request with `malformed` and ignores
/// an event (§4.7).
pub fn parse_json(frame: &Frame) -> JsonPayload {
    let (channel, raw) = split_app(frame);
    let object = match serde_json::from_slice::<Value>(&raw) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    };
    JsonPayload { channel, object, raw }
}

/// What the decoder made of the bytes so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Frame(Frame),
    /// A frame of an unknown type, skipped by its length (§4.7). It counts as
    /// a frame received for the silence timer (§4.6).
    Skipped,
}

struct Pending {
    ty: FrameType,
    flags: u8,
    id: u32,
    length: usize,
}

/// Streaming frame decoder. Before `ready()` only HANDSHAKE, PING, PONG and
/// CLOSE are valid and the payload limit is LINK_MAX_HANDSHAKE; `ready(limit)`
/// puts the negotiated limit in force. A header whose `length` is over the
/// limit fails before a payload byte is buffered, so a forged length cannot
/// drive an allocation. The decoder is dead after an error.
pub struct FrameDecoder {
    buffer: BytesMut,
    pending: Option<Pending>,
    skip: usize,
    limit: u32,
    linked: bool,
    dialect: Dialect,
    dead: bool,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buffer: BytesMut::new(), pending: None, skip: 0, limit: MAX_HANDSHAKE, linked: false, dialect: Dialect::Wire, dead: false }
    }

    /// A decoder for a local channel connection (`Dialect::Local`): every
    /// frame type is valid from the first byte, under `limit`.
    pub fn local(limit: u32) -> Self {
        Self { buffer: BytesMut::new(), pending: None, skip: 0, limit, linked: true, dialect: Dialect::Local, dead: false }
    }

    /// `ready` was sent or read: the negotiated payload limit applies from here.
    pub fn ready(&mut self, limit: u32) {
        debug_assert!((MIN_PAYLOAD..=PAYLOAD_CEILING).contains(&limit));
        self.limit = limit;
        self.linked = true;
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// The next whole frame of the bytes pushed so far, or None when more
    /// bytes are needed. Not `Iterator::next`: a decoding error is part of
    /// the answer and ends the decoder, which an iterator cannot say.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Decoded>, LinkClose> {
        if self.dead {
            return Err(LinkClose::protocol("decoder used after an error"));
        }
        match self.step() {
            Err(close) => {
                self.dead = true;
                Err(close)
            }
            ok => ok,
        }
    }

    fn step(&mut self) -> Result<Option<Decoded>, LinkClose> {
        loop {
            if self.skip > 0 {
                let n = self.skip.min(self.buffer.len());
                let _ = self.buffer.split_to(n);
                self.skip -= n;
                if self.skip > 0 {
                    return Ok(None);
                }
                return Ok(Some(Decoded::Skipped));
            }
            if let Some(pending) = &self.pending {
                if self.buffer.len() < pending.length {
                    return Ok(None);
                }
                let pending = self.pending.take().expect("checked above");
                let payload = self.buffer.split_to(pending.length).freeze();
                if pending.ty == FrameType::Credit && parse_credit(&payload) == 0 {
                    return Err(LinkClose::protocol("CREDIT of 0"));
                }
                if matches!(pending.ty, FrameType::Close | FrameType::Reset) && (payload[2] != 0 || payload[3] != 0) {
                    return Err(LinkClose::protocol("CLOSE/RESET: non-zero padding"));
                }
                return Ok(Some(Decoded::Frame(Frame { ty: pending.ty, flags: pending.flags, id: pending.id, payload })));
            }
            if self.buffer.len() < HEADER_BYTES {
                return Ok(None);
            }
            let header = self.buffer.split_to(HEADER_BYTES);
            let raw_type = header[0];
            let flags = header[1];
            let reserved = u16::from_le_bytes([header[2], header[3]]);
            let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            let id = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
            if reserved != 0 {
                return Err(LinkClose::protocol("non-zero reserved field"));
            }
            let ty = FrameType::from_u8(raw_type);
            if !self.linked && !ty.is_some_and(FrameType::before_ready) {
                return Err(LinkClose::protocol(format!("frame 0x{raw_type:02x} before ready")));
            }
            if self.linked && self.dialect == Dialect::Wire && ty == Some(FrameType::Handshake) {
                return Err(LinkClose::protocol("HANDSHAKE after ready"));
            }
            if length > self.limit {
                return Err(LinkClose::new(CloseCode::TooLarge, format!("length {length} over the limit {}", self.limit)));
            }
            let Some(ty) = ty else {
                if length == 0 {
                    return Ok(Some(Decoded::Skipped));
                }
                self.skip = length as usize;
                continue;
            };
            check_frame_in(self.dialect, ty, flags, id, length)?;
            if length == 0 {
                return Ok(Some(Decoded::Frame(Frame { ty, flags, id, payload: Bytes::new() })));
            }
            self.pending = Some(Pending { ty, flags, id, length: length as usize });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_dialect_takes_body_alone_and_refuses_app() {
        let body = encode_in(Dialect::Local, FrameType::Request, FLAG_BODY, 5, b"{\"t\":\"sink\"}", 16384).unwrap();
        assert!(encode(FrameType::Request, FLAG_BODY, 5, b"{\"t\":\"sink\"}", 16384).is_err());
        let mut local = FrameDecoder::local(16384);
        local.push(&body);
        local.push(&encode(FrameType::Handshake, 0, 0, b"{}", 16384).unwrap());
        assert!(matches!(local.next(), Ok(Some(Decoded::Frame(Frame { ty: FrameType::Request, flags: FLAG_BODY, id: 5, .. })))));
        assert!(matches!(local.next(), Ok(Some(Decoded::Frame(Frame { ty: FrameType::Handshake, .. })))));

        let mut local = FrameDecoder::local(16384);
        local.push(&encode(FrameType::Event, FLAG_APP, 0, b"\x03\0\0\0{}", 16384).unwrap());
        assert_eq!(local.next().unwrap_err().code, CloseCode::Protocol);
    }
}
