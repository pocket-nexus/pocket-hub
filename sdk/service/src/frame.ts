// Pocket Link 2 frames in the local dialect (docs/SERVICE-PROTOCOL.md §3; the
// link specification, vendor/pocketjs/docs/POCKET-LINK.md §4.2–§4.3).
//
//   off 0  u8   type
//   off 1  u8   flags     per type
//   off 2  u16  reserved  0
//   off 4  u32  length    payload bytes after the header
//   off 8  u32  id        request, stream or channel id; 0 where the type says so
//   off 12 ...  payload
//
// All integers are little-endian. The local dialect is the wire's frames with
// the connection standing for the channel: no frame carries the APP flag or a
// channel id, a REQUEST may carry BODY (0x04) by itself, and a HANDSHAKE frame
// is valid at any time. The checks below are those of the Rust codec Hub uses
// (`crates/pocket-link/src/frame.rs`, `check_frame_in(Dialect::Local, …)`),
// so a frame this module encodes is one Hub's decoder accepts.
//
// The constants are copied from the link specification (`contracts/spec/
// link2.ts` in PocketJS, `crates/pocket-link/src/consts.rs` here): this
// package ships on its own and imports nothing from either.

export const FRAME = {
  ping: 0x01,
  pong: 0x02,
  close: 0x03,
  handshake: 0x04,
  request: 0x10,
  reply: 0x11,
  event: 0x12,
  cancel: 0x13,
  data: 0x20,
  credit: 0x21,
  reset: 0x22,
} as const;
export type FrameType = (typeof FRAME)[keyof typeof FRAME];

/** REPLY: the payload is an error object. */
export const FLAG_ERROR = 0x01;
/** DATA: the last frame of a stream direction. */
export const FLAG_END = 0x01;
/** REQUEST: the requester's bytes follow on the request id (local dialect: without APP). */
export const FLAG_BODY = 0x04;

/** §4.4 */
export const HEADER_BYTES = 12;
export const MAX_HANDSHAKE = 1024;
export const MIN_PAYLOAD = 1024;
export const PAYLOAD_CEILING = 32_768;
/** The payload limit of the control connection (SERVICE-PROTOCOL.md §3). */
export const CONTROL_PAYLOAD = 16_384;

/** §5.1: ids with bit 31 set are Hub's; 0 and 0x80000000 are in neither range. */
export const ID_HUB_BIT = 0x8000_0000;
export const ID_DEVICE_MAX = 0x7fff_ffff;
export const ID_HUB_FIRST = 0x8000_0001;
export const ID_HUB_LAST = 0xffff_ffff;

/** §5.2: an error's `message` is at most 240 bytes of its encoded form. */
export const ERROR_MESSAGE_BYTES = 240;
/** §5.6: a grant may not raise one direction's outstanding credit past this. */
export const MAX_CREDIT = 16 * 1024 * 1024;
/** §5.6: one event unit per started 512 bytes of JSON. */
export const EVENT_UNIT = 512;
/** §5.6: the units Hub's side (here: the service) grants the applet. */
export const EVENT_WINDOW_SERVICE = 256;
/** §5.6: Hub's recommended window on an inbound stream, refilled as it is consumed. */
export const STREAM_WINDOW = 256 * 1024;
/** §7.6: open requests per channel and direction; open streams per channel and direction. */
export const CHANNEL_REQUESTS = 8;
export const CHANNEL_STREAMS = 4;
/** §7.2: LINK_APP_MSG = min(8,192, payload limit − 4). */
export const APP_MSG_CEILING = 8192;
export const APP_CHANNEL_PREFIX = 4;
/** §7.2: an applet operation name is 1 to 64 bytes of [a-z0-9._-]. */
export const APP_NAME_BYTES = 64;
/** §7.2: an applet's own error code is at most 32 bytes of [a-z0-9._-]. */
export const APP_ERROR_CODE_BYTES = 32;
/** §5.5: the sender of an event's stream gives up after 30 s without credit. */
export const STREAM_STALL_MS = 30_000;

/** §5.6 RESET codes. */
export const RESET = { cancelled: 1, refused: 2, length: 3, storage: 4, superseded: 5 } as const;
export type ResetName = keyof typeof RESET;
export const resetName = (code: number): ResetName | null =>
  (Object.keys(RESET) as ResetName[]).find((name) => RESET[name] === code) ?? null;

export const appMessageLimit = (payloadLimit: number) => Math.min(APP_MSG_CEILING, payloadLimit - APP_CHANNEL_PREFIX);
export const eventUnits = (jsonBytes: number) => Math.max(1, Math.ceil(jsonBytes / EVENT_UNIT));

/** Which side allocated an id (§5.1); null for 0 and 0x80000000. */
export function idRange(id: number): "device" | "hub" | null {
  if (id === 0 || id === ID_HUB_BIT) return null;
  return id > ID_DEVICE_MAX ? "hub" : "device";
}

const APP_NAME = /^[a-z0-9._-]+$/;
export const validAppName = (t: unknown): t is string =>
  typeof t === "string" && t.length > 0 && t.length <= APP_NAME_BYTES && APP_NAME.test(t);
export const validErrorCode = (code: unknown): code is string =>
  typeof code === "string" && code.length > 0 && code.length <= APP_ERROR_CODE_BYTES && APP_NAME.test(code);

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

export interface Frame {
  type: number;
  flags: number;
  id: number;
  payload: Uint8Array;
}

/** A frame the other side must not have sent, or this side must not send. */
export class FrameError extends Error {
  constructor(readonly code: "protocol" | "too_large", message: string) {
    super(message);
  }
}

/** The flag bits a type defines in the local dialect. */
function knownFlags(type: number): number {
  switch (type) {
    case FRAME.request:
      return FLAG_BODY;
    case FRAME.reply:
      return FLAG_ERROR;
    case FRAME.data:
      return FLAG_END;
    default:
      return 0;
  }
}

/** The header and size checks of §4.2 and §4.7 for a known type, local dialect. Throws FrameError. */
export function checkFrame(type: number, flags: number, id: number, length: number): void {
  if ((flags & ~knownFlags(type)) !== 0) throw new FrameError("protocol", `frame 0x${type.toString(16)}: undefined flag`);
  const named = idRange(id) !== null;
  switch (type) {
    case FRAME.ping:
    case FRAME.pong:
      if (id !== 0) throw new FrameError("protocol", "PING/PONG: id is not 0");
      if (length !== 4) throw new FrameError("protocol", "PING/PONG: payload is not 4 bytes");
      return;
    case FRAME.close:
      if (id !== 0) throw new FrameError("protocol", "CLOSE: id is not 0");
      if (length < 4 || length > 4 + 120) throw new FrameError("protocol", "CLOSE: bad payload size");
      return;
    case FRAME.handshake:
      if (id !== 0) throw new FrameError("protocol", "HANDSHAKE: id is not 0");
      if (length > MAX_HANDSHAKE) throw new FrameError("too_large", "HANDSHAKE: over 1,024 bytes");
      return;
    case FRAME.request:
    case FRAME.reply:
    case FRAME.cancel:
      if (!named) throw new FrameError("protocol", "request id is 0 or 0x80000000");
      if (type === FRAME.cancel && length !== 0) throw new FrameError("protocol", "CANCEL: payload is not empty");
      return;
    case FRAME.event:
      if (id === ID_HUB_BIT) throw new FrameError("protocol", "EVENT: id 0x80000000");
      return;
    case FRAME.data:
      if (!named) throw new FrameError("protocol", "DATA: stream id is 0 or 0x80000000");
      if (length === 0 && (flags & FLAG_END) === 0) throw new FrameError("protocol", "DATA: 0 bytes without END");
      return;
    case FRAME.credit:
    case FRAME.reset:
      if (!named) throw new FrameError("protocol", "CREDIT/RESET: id is 0 or 0x80000000");
      if (length !== 4) throw new FrameError("protocol", "CREDIT/RESET: payload is not 4 bytes");
      return;
  }
}

const isKnownType = (type: number) => (Object.values(FRAME) as number[]).includes(type);

/** Encodes one frame. `limit` is the payload limit in force; a HANDSHAKE is held to 1,024 bytes. */
export function encodeFrame(type: number, flags: number, id: number, payload: Uint8Array, limit: number): Uint8Array {
  const max = type === FRAME.handshake ? MAX_HANDSHAKE : limit;
  if (payload.length > max) throw new FrameError("too_large", `frame payload ${payload.length} over the limit ${max}`);
  checkFrame(type, flags, id, payload.length);
  if (type === FRAME.credit && readU32(payload, 0) === 0) throw new FrameError("protocol", "CREDIT of 0");
  const out = new Uint8Array(HEADER_BYTES + payload.length);
  const view = new DataView(out.buffer);
  view.setUint8(0, type);
  view.setUint8(1, flags);
  view.setUint16(2, 0, true);
  view.setUint32(4, payload.length, true);
  view.setUint32(8, id >>> 0, true);
  out.set(payload, HEADER_BYTES);
  return out;
}

/** The UTF-8 bytes of one JSON object, as `JSON.stringify` writes it: no whitespace, no trailing newline (§4.3). */
export function jsonEncode(object: unknown): Uint8Array {
  return encoder.encode(JSON.stringify(object));
}

export function encodeJson(type: number, id: number, object: unknown, limit: number, flags = 0): Uint8Array {
  return encodeFrame(type, flags, id, jsonEncode(object), limit);
}

export function encodeJsonBytes(type: number, id: number, json: Uint8Array, limit: number, flags = 0): Uint8Array {
  return encodeFrame(type, flags, id, json, limit);
}

export const encodeHandshake = (object: Record<string, unknown>) => encodeJson(FRAME.handshake, 0, object, MAX_HANDSHAKE);

export function encodeData(id: number, bytes: Uint8Array, end: boolean, limit: number): Uint8Array {
  return encodeFrame(FRAME.data, end ? FLAG_END : 0, id, bytes, limit);
}

export function encodeCredit(id: number, amount: number): Uint8Array {
  const payload = new Uint8Array(4);
  new DataView(payload.buffer).setUint32(0, amount >>> 0, true);
  return encodeFrame(FRAME.credit, 0, id, payload, 4);
}

export function encodeReset(id: number, code: ResetName): Uint8Array {
  const payload = new Uint8Array(4);
  new DataView(payload.buffer).setUint16(0, RESET[code], true);
  return encodeFrame(FRAME.reset, 0, id, payload, 4);
}

export function encodeCancel(id: number): Uint8Array {
  return encodeFrame(FRAME.cancel, 0, id, new Uint8Array(0), 0);
}

export function readU32(bytes: Uint8Array, at: number): number {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(at, true);
}
export function readU16(bytes: Uint8Array, at: number): number {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint16(at, true);
}

/** The JSON object of a REQUEST, REPLY, EVENT or HANDSHAKE payload; null when it is not one JSON object. */
export function parseJson(payload: Uint8Array): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(decoder.decode(payload));
    return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** Bytes a string takes inside a JSON frame: its encoded form without the quotes (§4.4). */
export function encodedBytes(text: string): number {
  return encoder.encode(JSON.stringify(text)).length - 2;
}

/** The longest prefix of `text` whose encoded form fits `max` bytes, cut on a code point. */
export function encodedPrefix(text: string, max: number): string {
  if (encodedBytes(text) <= max) return text;
  let used = 0;
  let kept = "";
  for (const ch of text) {
    const length = encodedBytes(ch);
    if (used + length > max) break;
    kept += ch;
    used += length;
  }
  return kept;
}

/**
 * Streaming decoder for one connection, local dialect: every frame type is
 * valid from the first byte (a HANDSHAKE included), under `limit`. A header
 * whose `length` is over the limit fails before its payload is buffered.
 * Frames of an unknown type are skipped by their length (§4.7). The decoder
 * is dead after an error.
 */
export class FrameDecoder {
  private buffer = new Uint8Array(0);
  private dead = false;
  constructor(public limit: number) {}

  push(chunk: Uint8Array): Frame[] {
    if (this.dead) throw new FrameError("protocol", "decoder used after an error");
    if (this.buffer.length === 0) this.buffer = chunk.slice();
    else {
      const joined = new Uint8Array(this.buffer.length + chunk.length);
      joined.set(this.buffer);
      joined.set(chunk, this.buffer.length);
      this.buffer = joined;
    }
    const frames: Frame[] = [];
    let at = 0;
    try {
      while (this.buffer.length - at >= HEADER_BYTES) {
        const view = new DataView(this.buffer.buffer, this.buffer.byteOffset + at, HEADER_BYTES);
        const type = view.getUint8(0);
        const flags = view.getUint8(1);
        const reserved = view.getUint16(2, true);
        const length = view.getUint32(4, true);
        const id = view.getUint32(8, true);
        if (reserved !== 0) throw new FrameError("protocol", "non-zero reserved field");
        const max = type === FRAME.handshake ? Math.max(this.limit, MAX_HANDSHAKE) : this.limit;
        if (length > max) throw new FrameError("too_large", `length ${length} over the limit ${this.limit}`);
        if (isKnownType(type)) checkFrame(type, flags, id, length);
        if (this.buffer.length - at < HEADER_BYTES + length) break;
        const payload = this.buffer.slice(at + HEADER_BYTES, at + HEADER_BYTES + length);
        at += HEADER_BYTES + length;
        if (!isKnownType(type)) continue;
        if (type === FRAME.credit && readU32(payload, 0) === 0) throw new FrameError("protocol", "CREDIT of 0");
        if ((type === FRAME.reset || type === FRAME.close) && readU16(payload, 2) !== 0) throw new FrameError("protocol", "CLOSE/RESET: non-zero padding");
        frames.push({ type, flags, id, payload });
      }
    } catch (error) {
      this.dead = true;
      throw error;
    }
    this.buffer = this.buffer.subarray(at);
    return frames;
  }
}
