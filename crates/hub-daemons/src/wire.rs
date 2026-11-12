//! §3: frames on the daemon socket, through `pocket_link`'s codec.

use bytes::Bytes;
use pocket_link::consts::{HEADER_BYTES, MAX_HANDSHAKE};
use pocket_link::frame::{encode_json, parse_json, Decoded, JsonFrame, JsonPayload};
use pocket_link::{Frame, FrameDecoder, FrameType};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The payload limit on the control connection (§3).
pub const CONTROL_LIMIT: u32 = 16_384;

/// Reads exactly one frame: its 12-byte header, then exactly `length` bytes.
/// Not one byte past the frame is taken from `from`, so the bytes after it
/// stay in the socket for whoever reads next (the relay, after
/// `channel.attach`). `limit` bounds the payload before any of it is read.
pub async fn read_frame_exact(from: &mut (impl AsyncRead + Unpin), limit: u32) -> Result<Frame, String> {
    let mut header = [0u8; HEADER_BYTES];
    from.read_exact(&mut header).await.map_err(|error| format!("reading a frame header: {error}"))?;
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if length > limit {
        return Err(format!("a frame of {length} bytes over the limit {limit}"));
    }
    let mut frame = vec![0u8; HEADER_BYTES + length as usize];
    frame[..HEADER_BYTES].copy_from_slice(&header);
    from.read_exact(&mut frame[HEADER_BYTES..]).await.map_err(|error| format!("reading a frame payload: {error}"))?;
    // The codec checks the header and the type's rules on the whole frame.
    let mut decoder = FrameDecoder::local(limit);
    decoder.push(&frame);
    match decoder.next() {
        Ok(Some(Decoded::Frame(frame))) => Ok(frame),
        Ok(Some(Decoded::Skipped)) => Err(format!("a frame of unknown type 0x{:02x}", header[0])),
        Ok(None) => Err("an incomplete frame".into()),
        Err(close) => Err(close.reason),
    }
}

/// The opening `HANDSHAKE` of a connection, with its `t`.
pub async fn read_handshake(from: &mut (impl AsyncRead + Unpin)) -> Result<JsonPayload, String> {
    let frame = read_frame_exact(from, MAX_HANDSHAKE).await?;
    if frame.ty != FrameType::Handshake {
        return Err(format!("the first frame is {:?}, not HANDSHAKE", frame.ty));
    }
    let json = parse_json(&frame);
    if json.t().is_none() {
        return Err("the HANDSHAKE is not a JSON object with a string `t`".into());
    }
    Ok(json)
}

pub fn handshake<T: Serialize>(object: &T) -> Bytes {
    // Hub's own handshake objects are small and fixed in shape; a text that
    // overflows 1,024 bytes is cut by the caller.
    encode_json(FrameType::Handshake, 0, object, JsonFrame::default(), MAX_HANDSHAKE).expect("a handshake object within 1,024 bytes")
}

pub fn event<T: Serialize>(object: &T) -> Result<Bytes, String> {
    encode_json(FrameType::Event, 0, object, JsonFrame::default(), CONTROL_LIMIT).map_err(|error| error.0)
}

pub async fn write_all(to: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> std::io::Result<()> {
    to.write_all(bytes).await?;
    to.flush().await
}
