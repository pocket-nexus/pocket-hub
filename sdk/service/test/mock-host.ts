// The host's end of the host connection (docs/SERVICE-PROTOCOL.md §6, §7) for
// the SDK's tests: a store in memory, answered with the frames the host
// sends, and every frame of the service checked against what the host
// enforces (ids that increase from 1, DATA within the credit granted, a
// body of exactly its declared count). A broken rule is a violation.

import { RESET_CODE, T, frameBytes, jsonFrame, type MFrame } from "./mock-hub.ts";

const WINDOW = 256 * 1024;
const u32 = (value: number) => {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, value >>> 0, true);
  return b;
};
const u16pad = (value: number) => {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint16(0, value, true);
  return b;
};
const enc = new TextEncoder();
const compare = (a: string, b: string) => Buffer.compare(Buffer.from(enc.encode(a)), Buffer.from(enc.encode(b)));

interface Body {
  key: string;
  declared: number;
  chunks: Uint8Array[];
  received: number;
  outstanding: number;
}
interface Sending {
  value: Uint8Array;
  sent: number;
  credit: number;
}

export class MockHost {
  readonly store = new Map<string, Uint8Array>();
  /** Each request the service made: its `t` and arguments. */
  readonly requests: Record<string, any>[] = [];
  /** CANCELs the service sent, by id. */
  readonly cancels: number[] = [];
  maxBytes = 16 * 1024 * 1024;
  /** Hold `store.set` replies until released (to cancel after the last byte). */
  holdSets = false;
  private held: (() => void)[] = [];
  private lastId = 0;
  private readonly bodies = new Map<number, Body>();
  private readonly sending = new Map<number, Sending>();
  private readonly open = new Set<number>();

  constructor(
    private readonly send: (bytes: Uint8Array) => void,
    private readonly violation: (why: string) => void,
  ) {}

  releaseSets(): void {
    for (const go of this.held.splice(0)) go();
  }

  private reply(id: number, value: unknown): void {
    if (!this.open.delete(id)) return;
    this.send(jsonFrame(T.reply, id, value));
  }
  private error(id: number, code: string, message: string): void {
    if (!this.open.delete(id)) return;
    this.bodies.delete(id);
    this.sending.delete(id);
    this.send(jsonFrame(T.reply, id, { code, message }, 0x01));
  }
  private bytesUsed(): number {
    let sum = 0;
    for (const [key, value] of this.store) sum += enc.encode(key).length + value.length;
    return sum;
  }

  frames(frames: MFrame[]): void {
    for (const f of frames) {
      switch (f.type) {
        case T.request:
          this.request(f);
          break;
        case T.data:
          this.data(f);
          break;
        case T.credit: {
          const out = this.sending.get(f.id);
          if (out !== undefined) {
            out.credit += f.value;
            this.pump(f.id);
          }
          break;
        }
        case T.cancel:
          this.cancels.push(f.id);
          if (this.bodies.has(f.id) || this.sending.has(f.id)) {
            this.send(frameBytes(T.reset, 0, f.id, u16pad(RESET_CODE.cancelled)));
            this.error(f.id, "cancelled", "the service cancelled the call");
          }
          break;
        case T.reset:
          if (this.bodies.has(f.id)) this.error(f.id, "cancelled", "the service reset the call's stream");
          break;
        default:
          this.violation(`host: frame 0x${f.type.toString(16)} from the service`);
      }
    }
  }

  private request(f: MFrame): void {
    if (f.id <= this.lastId || f.id > 0x7fff_ffff) this.violation(`host: request id ${f.id} after ${this.lastId}`);
    this.lastId = f.id;
    const json = f.json ?? {};
    this.requests.push(json);
    this.open.add(f.id);
    const body = (f.flags & 0x04) !== 0;
    switch (json.t) {
      case "store.get": {
        const value = this.store.get(json.key);
        if (value === undefined) return this.reply(f.id, { absent: true });
        this.send(jsonFrame(T.event, f.id, { t: "store.head", bytes: value.length }));
        this.sending.set(f.id, { value, sent: 0, credit: 0 });
        return this.pump(f.id);
      }
      case "store.set": {
        if (!body) this.violation("host: store.set without BODY");
        if (json.bytes > 4 * 1024 * 1024) return this.error(f.id, "too_large", "over 4 MiB");
        const grant = Math.min(WINDOW, json.bytes);
        this.bodies.set(f.id, { key: json.key, declared: json.bytes, chunks: [], received: 0, outstanding: grant });
        if (grant > 0) this.send(frameBytes(T.credit, 0, f.id, u32(grant)));
        return;
      }
      case "store.delete":
        return this.reply(f.id, { existed: this.store.delete(json.key) });
      case "store.list": {
        const prefix: string = json.prefix ?? "";
        const limit: number = json.limit ?? 1000;
        const keys = [...this.store.keys()].filter((k) => k.startsWith(prefix) && (json.after === undefined || compare(k, json.after) > 0)).sort(compare);
        return this.reply(f.id, { keys: keys.slice(0, limit), more: keys.length > limit });
      }
      case "store.usage":
        return this.reply(f.id, { bytes: this.bytesUsed(), keys: this.store.size, maxBytes: this.maxBytes });
      default:
        return this.error(f.id, "invalid", `no request ${json.t}`);
    }
  }

  private data(f: MFrame): void {
    const body = this.bodies.get(f.id);
    if (body === undefined) {
      if (f.payload.length > 0) this.violation(`host: DATA on ${f.id} with no body open`);
      return;
    }
    if (f.payload.length > body.outstanding) this.violation(`host: DATA past credit on ${f.id}`);
    body.outstanding -= f.payload.length;
    body.received += f.payload.length;
    body.chunks.push(f.payload.slice());
    if ((f.flags & 0x01) !== 0) {
      this.bodies.delete(f.id);
      if (body.received !== body.declared) return this.error(f.id, "invalid", "the body is not its declared count");
      const value = new Uint8Array(body.received);
      let at = 0;
      for (const chunk of body.chunks) {
        value.set(chunk, at);
        at += chunk.length;
      }
      const commit = () => {
        const before = this.store.get(body.key);
        const would = this.bytesUsed() - (before ? enc.encode(body.key).length + before.length : 0) + enc.encode(body.key).length + value.length;
        if (would > this.maxBytes && would > this.bytesUsed()) return this.error(f.id, "quota", "over the limit");
        this.store.set(body.key, value);
        this.reply(f.id, {});
      };
      if (this.holdSets) this.held.push(commit);
      else commit();
      return;
    }
    const due = Math.min(WINDOW, body.declared - body.received) - body.outstanding;
    if (due > 0 && (due >= WINDOW / 4 || body.outstanding === 0)) {
      body.outstanding += due;
      this.send(frameBytes(T.credit, 0, f.id, u32(due)));
    }
  }

  private pump(id: number): void {
    const out = this.sending.get(id);
    if (out === undefined) return;
    if (out.value.length === 0) {
      this.send(frameBytes(T.data, 0x01, id));
      this.sending.delete(id);
      return this.reply(id, { bytes: 0 });
    }
    while (out.sent < out.value.length && out.credit > 0) {
      const n = Math.min(out.credit, 65_536, out.value.length - out.sent);
      const end = out.sent + n === out.value.length;
      this.send(frameBytes(T.data, end ? 0x01 : 0, id, out.value.subarray(out.sent, out.sent + n)));
      out.sent += n;
      out.credit -= n;
    }
    if (out.sent === out.value.length) {
      this.sending.delete(id);
      this.reply(id, { bytes: out.value.length });
    }
  }
}
