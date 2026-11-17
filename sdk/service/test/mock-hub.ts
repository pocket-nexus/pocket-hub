// A mock of Pocket Hub's service socket for the SDK's tests: it listens on a
// Unix socket in a temp folder, speaks the Hub side of docs/SERVICE-PROTOCOL.md
// at the frame level, and plays the applet on each channel connection.
//
// It shares no code with the SDK: its decoder and encoder are written here
// from the protocol document and `crates/pocket-link/src/frame.rs`
// (`check_frame_in(Dialect::Local, …)`), and **every frame the service sends is
// checked against what Hub enforces** (SERVICE-PROTOCOL.md §5, "What Hub
// enforces"): the local dialect's header rules, the payload limit, the
// message limit, ids from the Hub range in order, at most 8 open requests and
// 4 open streams toward the applet, the head step, DATA within the credit the
// applet granted, events within the units the applet granted. A broken rule
// is recorded in `violations`; tests assert there are none.

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Socket, UnixSocketListener } from "bun";
import { MockHost } from "./mock-host.ts";

export const T = { handshake: 0x04, request: 0x10, reply: 0x11, event: 0x12, cancel: 0x13, data: 0x20, credit: 0x21, reset: 0x22 } as const;
const KNOWN = new Set([0x01, 0x02, 0x03, 0x04, 0x10, 0x11, 0x12, 0x13, 0x20, 0x21, 0x22]);
export const RESET_CODE = { cancelled: 1, refused: 2, length: 3, storage: 4, superseded: 5 } as const;

export interface MFrame {
  type: number;
  flags: number;
  id: number;
  payload: Uint8Array;
  /** The parsed JSON object of a HANDSHAKE, REQUEST, REPLY or EVENT; null otherwise. */
  json: Record<string, any> | null;
  /** The u32 of a CREDIT, the u16 code of a RESET. */
  value: number;
  /** The frame's bytes as they came, header included. */
  raw: Uint8Array;
}

const enc = new TextEncoder();
const dec = new TextDecoder();

export function frameBytes(type: number, flags: number, id: number, payload: Uint8Array = new Uint8Array(0)): Uint8Array {
  const out = new Uint8Array(12 + payload.length);
  const view = new DataView(out.buffer);
  view.setUint8(0, type);
  view.setUint8(1, flags);
  view.setUint16(2, 0, true);
  view.setUint32(4, payload.length, true);
  view.setUint32(8, id >>> 0, true);
  out.set(payload, 12);
  return out;
}
export const jsonFrame = (type: number, id: number, object: unknown, flags = 0) => frameBytes(type, flags, id, enc.encode(JSON.stringify(object)));
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
export const units = (bytes: number) => Math.max(1, Math.ceil(bytes / 512));
const isHub = (id: number) => id > 0x8000_0000;

/** The local dialect's header checks, as `check_frame_in(Dialect::Local, …)` makes them. Returns a reason, or null. */
export function localFrameRule(type: number, flags: number, id: number, length: number): string | null {
  const known = type === 0x10 ? 0x04 : type === 0x11 ? 0x01 : type === 0x20 ? 0x01 : 0;
  if ((flags & ~known) !== 0) return `type 0x${type.toString(16)}: undefined flag 0x${flags.toString(16)}`;
  const named = id !== 0 && id !== 0x8000_0000;
  switch (type) {
    case 0x01:
    case 0x02:
      return id !== 0 || length !== 4 ? "PING/PONG rule" : null;
    case 0x03:
      return id !== 0 || length < 4 || length > 124 ? "CLOSE rule" : null;
    case 0x04:
      return id !== 0 ? "HANDSHAKE id" : length > 1024 ? "HANDSHAKE over 1,024" : null;
    case 0x10:
    case 0x11:
      return named ? null : "request id 0 or 0x80000000";
    case 0x13:
      return !named ? "CANCEL id" : length !== 0 ? "CANCEL payload" : null;
    case 0x12:
      return id === 0x8000_0000 ? "EVENT id 0x80000000" : null;
    case 0x20:
      return !named ? "DATA id" : length === 0 && (flags & 1) === 0 ? "DATA 0 bytes without END" : null;
    case 0x21:
    case 0x22:
      return !named ? "CREDIT/RESET id" : length !== 4 ? "CREDIT/RESET payload" : null;
  }
  return null;
}

/** Decodes frames from the service, local dialect, under `limit`. Reports broken rules instead of throwing. */
class MockDecoder {
  private buffer = new Uint8Array(0);
  dead = false;
  constructor(public limit: number, private readonly violation: (why: string) => void) {}
  push(chunk: Uint8Array): MFrame[] {
    if (this.dead) return [];
    const joined = new Uint8Array(this.buffer.length + chunk.length);
    joined.set(this.buffer);
    joined.set(chunk, this.buffer.length);
    this.buffer = joined;
    const frames: MFrame[] = [];
    let at = 0;
    while (this.buffer.length - at >= 12) {
      const view = new DataView(this.buffer.buffer, this.buffer.byteOffset + at, 12);
      const type = view.getUint8(0);
      const flags = view.getUint8(1);
      const reserved = view.getUint16(2, true);
      const length = view.getUint32(4, true);
      const id = view.getUint32(8, true);
      const broken = reserved !== 0 ? "reserved not 0" : length > this.limit ? `length ${length} over the payload limit ${this.limit}` : !KNOWN.has(type) ? `unknown type 0x${type.toString(16)}` : localFrameRule(type, flags, id, length);
      if (broken !== null) {
        this.violation(`frame: ${broken}`);
        this.dead = true;
        return frames;
      }
      if (this.buffer.length - at < 12 + length) break;
      const raw = this.buffer.slice(at, at + 12 + length);
      const payload = raw.subarray(12);
      at += 12 + length;
      let json: Record<string, any> | null = null;
      let value = 0;
      if (type === T.handshake || type === T.request || type === T.reply || type === T.event) {
        try {
          const parsed = JSON.parse(dec.decode(payload));
          json = parsed !== null && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : null;
        } catch {
          json = null;
        }
        if (json === null) this.violation(`type 0x${type.toString(16)} id ${id}: payload is not one JSON object`);
      } else if (type === T.credit) {
        value = new DataView(payload.buffer, payload.byteOffset, 4).getUint32(0, true);
        if (value === 0) this.violation("CREDIT of 0");
      } else if (type === T.reset) {
        value = new DataView(payload.buffer, payload.byteOffset, 4).getUint16(0, true);
        if (new DataView(payload.buffer, payload.byteOffset, 4).getUint16(2, true) !== 0) this.violation("RESET padding not 0");
      }
      frames.push({ type, flags, id, payload, json, value, raw });
    }
    this.buffer = this.buffer.slice(at);
    return frames;
  }
}

/** One accepted connection, before it is known as control or channel. */
class MockSocket {
  readonly decoder: MockDecoder;
  socket: Socket<MockSocket> | null = null;
  private queue: Uint8Array[] = [];
  closed = false;
  onFrames: (frames: MFrame[]) => void = () => {};
  onClose: () => void = () => {};
  constructor(limit: number, violation: (why: string) => void) {
    this.decoder = new MockDecoder(limit, violation);
  }
  send(bytes: Uint8Array): void {
    if (this.closed || this.socket === null) return;
    this.queue.push(bytes);
    this.flush();
  }
  flush(): void {
    while (this.queue.length > 0 && this.socket !== null && !this.closed) {
      const head = this.queue[0]!;
      const n = this.socket.write(head);
      if (n < 0) return;
      if (n < head.length) {
        this.queue[0] = head.subarray(n);
        return;
      }
      this.queue.shift();
    }
  }
  end(): void {
    if (this.closed) return;
    this.flush();
    this.socket?.end();
  }
}

export interface OfferFields {
  ticket: string;
  device: string;
  deviceName: string;
  model: string;
  target: string;
  app: string;
  hash: string;
  generation: number;
  payloadLimit: number;
}

const hex32 = () => [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");

interface Waiter {
  pred: (frame: MFrame) => boolean;
  resolve: (frame: MFrame) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/** The applet's end of one channel connection, with Hub's checks on the service's frames. */
export class MockChannel {
  readonly frames: MFrame[] = [];
  readonly violations: string[];
  /** Units the applet granted the service and the service has not spent. */
  unitsForService = 0;
  /** Units the service granted the applet and the applet has not spent. */
  unitsFromService = 0;
  /** Every CREDIT the service sent on the channel id, in order. */
  readonly unitGrants: number[] = [];
  /** Regrant the service's events at once, as an applet that takes every event (examples). */
  autoUnits = false;
  /** Byte credit the service granted the applet, per stream id. */
  private readonly creditFromService = new Map<number, number>();
  /** The most byte credit the service had outstanding toward the applet at once, per stream id. */
  readonly mostCreditFrom = new Map<number, number>();
  /** Byte credit the applet granted the service and the service has not used, per stream id. */
  private readonly creditForService = new Map<number, number>();
  /** Requests of the service without a REPLY of the applet yet. */
  readonly serviceRequests = new Set<number>();
  /** Requests of the applet without a REPLY of the service yet. */
  readonly appletRequests = new Set<number>();
  /** Applet requests the applet cancelled (Hub answered them): nothing more may come for them. */
  private readonly cancelled = new Set<number>();
  /** Applet requests that got a progress event: their response may carry bytes. */
  private readonly headed = new Set<number>();
  /** Streams toward the applet that are open (no END, no RESET). */
  readonly outboundStreams = new Set<number>();
  private lastServiceId = 0x8000_0000;
  private nextAppletId: number;
  private waiters: Waiter[] = [];
  private consumers = new Map<number, (frame: MFrame) => void>();
  private creditWaiters = new Map<number, (() => void)[]>();
  private closeNoticeSent = false;
  /** Frames of the service read after Hub's close notice (in flight when it was sent). */
  afterNotice = 0;
  /** Applet requests whose response stream has started. */
  private readonly responses = new Set<number>();
  serviceClosed = false;
  /** The largest DATA payload the service sent. */
  maxData = 0;
  readonly closed: Promise<void>;
  private resolveClosed!: () => void;

  constructor(
    private readonly sock: MockSocket,
    readonly channel: number,
    readonly payloadLimit: number,
    readonly messageLimit: number,
    readonly offer: OfferFields,
    hubViolations: string[],
    firstAppletId: number,
  ) {
    this.violations = hubViolations;
    this.nextAppletId = firstAppletId;
    this.closed = new Promise((resolve) => (this.resolveClosed = resolve));
    sock.onFrames = (frames) => {
      for (const frame of frames) this.receive(frame);
    };
    sock.onClose = () => {
      this.resolveClosed();
      for (const w of this.waiters.splice(0)) {
        clearTimeout(w.timer);
        w.reject(new Error("the channel connection closed"));
      }
    };
  }

  private violation(why: string): void {
    this.violations.push(`channel ${this.channel}: ${why}`);
  }

  private receive(frame: MFrame): void {
    if (this.serviceClosed) this.violation(`a frame (type 0x${frame.type.toString(16)}) after the service's channel.close`);
    // After Hub's own notice the service may still have frames in flight; Hub drops them (§5 "Close").
    if (this.closeNoticeSent) this.afterNotice += 1;
    this.frames.push(frame);
    this.check(frame);
    this.consumers.get(frame.id)?.(frame);
    for (const w of [...this.waiters]) {
      if (w.pred(frame)) {
        this.waiters.splice(this.waiters.indexOf(w), 1);
        clearTimeout(w.timer);
        w.resolve(frame);
      }
    }
  }

  /** Hub's rules on the service's frames (SERVICE-PROTOCOL.md §5). */
  private check(frame: MFrame): void {
    const { type, id, json } = frame;
    if ((type === T.request || type === T.reply || type === T.event) && frame.payload.length > this.messageLimit && !(type === T.event && id === this.channel)) {
      this.violation(`a JSON object of ${frame.payload.length} bytes over messageLimit ${this.messageLimit}`);
    }
    switch (type) {
      case T.handshake:
        this.violation("HANDSHAKE after channel.ready");
        return;
      case T.request: {
        if (!isHub(id)) this.violation(`REQUEST id ${id} outside the Hub range`);
        this.nextServiceId(id, "REQUEST");
        if (typeof json?.t !== "string") this.violation("REQUEST without a string t");
        this.serviceRequests.add(id);
        if (this.serviceRequests.size > 8) this.violation("a ninth open request of the service");
        if ((frame.flags & 0x04) !== 0) this.openOutbound(id);
        return;
      }
      case T.reply:
        if (!this.appletRequests.has(id)) {
          this.violation(this.cancelled.has(id) ? `REPLY for request ${id}, which the applet cancelled` : `REPLY for ${id}, not an open request of the applet`);
          return;
        }
        this.appletRequests.delete(id);
        return;
      case T.event: {
        if (id === this.channel) {
          if (json?.t !== "channel.close") this.violation(`an event other than channel.close on the channel id`);
          this.serviceClosed = true;
          return;
        }
        if (typeof json?.t !== "string") this.violation("EVENT without a string t");
        const cost = units(frame.payload.length);
        if (cost > this.unitsForService) this.violation(`an event of ${cost} units past the applet's ${this.unitsForService}`);
        this.unitsForService -= cost;
        if (this.autoUnits) this.grantUnits(cost);
        if (id === 0) return;
        if (this.appletRequests.has(id)) {
          this.headed.add(id);
          return;
        }
        if (this.cancelled.has(id)) return this.violation(`progress on request ${id}, which the applet cancelled`);
        if (!isHub(id)) return this.violation(`EVENT id ${id}: not 0, not an open request of the applet, not a Hub-range id`);
        this.nextServiceId(id, "stream EVENT");
        this.openOutbound(id);
        return;
      }
      case T.cancel:
        if (!this.serviceRequests.has(id)) this.violation(`CANCEL for ${id}, not an open request of the service`);
        return;
      case T.data: {
        this.maxData = Math.max(this.maxData, frame.payload.length);
        if (frame.payload.length > this.payloadLimit) this.violation("DATA over the payload limit");
        if (this.cancelled.has(id)) return this.violation(`DATA on request ${id}, which the applet cancelled`);
        if (!isHub(id) && !this.headed.has(id)) return this.violation(`DATA on the applet's request ${id} before any progress event (the head step)`);
        // A response stream opens at its first DATA frame; a body or event stream opened with its REQUEST or EVENT.
        if (!isHub(id) && !this.responses.has(id)) {
          this.responses.add(id);
          this.openOutbound(id);
        }
        if (!this.outboundStreams.has(id)) return this.violation(`DATA on ${id}, which is not an open stream toward the applet`);
        const credit = this.creditForService.get(id) ?? 0;
        if (frame.payload.length > credit) this.violation(`DATA of ${frame.payload.length} bytes past the applet's credit ${credit} on ${id}`);
        this.creditForService.set(id, credit - frame.payload.length);
        if ((frame.flags & 1) !== 0) this.outboundStreams.delete(id);
        return;
      }
      case T.credit:
        if (id === this.channel) {
          this.unitGrants.push(frame.value);
          this.unitsFromService += frame.value;
        } else {
          const outstanding = (this.creditFromService.get(id) ?? 0) + frame.value;
          this.creditFromService.set(id, outstanding);
          this.mostCreditFrom.set(id, Math.max(this.mostCreditFrom.get(id) ?? 0, outstanding));
          for (const wake of this.creditWaiters.get(id)?.splice(0) ?? []) wake();
        }
        return;
      case T.reset:
        this.outboundStreams.delete(id);
        return;
    }
  }

  private nextServiceId(id: number, what: string): void {
    if (id <= this.lastServiceId) this.violation(`${what} id 0x${id.toString(16)} is not past the last (0x${this.lastServiceId.toString(16)}): ids come from one counter, never reused`);
    this.lastServiceId = Math.max(this.lastServiceId, id);
  }
  private openOutbound(id: number): void {
    this.outboundStreams.add(id);
    if (this.outboundStreams.size > 4) this.violation("a fifth open stream toward the applet");
  }

  // --- waiting ---------------------------------------------------------------

  /** The first frame (received already or later) that matches. */
  waitFor(pred: (frame: MFrame) => boolean, timeoutMs = 3000, from = 0): Promise<MFrame> {
    for (let i = from; i < this.frames.length; i++) if (pred(this.frames[i]!)) return Promise.resolve(this.frames[i]!);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((w) => w.timer !== timer);
        reject(new Error(`no matching frame in ${timeoutMs} ms; frames: ${this.describe()}`));
      }, timeoutMs);
      this.waiters.push({ pred, resolve, reject, timer });
    });
  }
  /** The next frame that matches, received after this call. */
  next(pred: (frame: MFrame) => boolean, timeoutMs = 3000): Promise<MFrame> {
    return this.waitFor(pred, timeoutMs, this.frames.length);
  }
  reply(id: number, timeoutMs = 3000): Promise<MFrame> {
    return this.waitFor((f) => f.type === T.reply && f.id === id, timeoutMs);
  }
  describe(): string {
    return this.frames
      .slice(-12)
      .map((f) => `${f.type.toString(16)}/${f.flags}/${f.id.toString(16)}${f.json ? JSON.stringify(f.json) : f.type === T.data ? `[${f.payload.length}]` : `=${f.value}`}`)
      .join(" ");
  }
  /** Lets pending socket reads and microtasks run. */
  async settle(ms = 15): Promise<void> {
    await Bun.sleep(ms);
  }

  // --- the applet's frames ---------------------------------------------------

  send(bytes: Uint8Array): void {
    this.sock.send(bytes);
  }
  takeId(): number {
    return this.nextAppletId++;
  }
  /** A request of the applet; returns its id. */
  request(t: string, args: Record<string, unknown> = {}, options: { body?: boolean; id?: number } = {}): number {
    const id = options.id ?? this.takeId();
    this.appletRequests.add(id);
    this.send(jsonFrame(T.request, id, { t, ...args }, options.body ? 0x04 : 0));
    return id;
  }
  /** An event of the applet: id 0, or a fresh id when it opens a stream. Returns the id. Spends the units the service granted. */
  event(t: string, args: Record<string, unknown> = {}, options: { stream?: boolean; raw?: Uint8Array } = {}): number {
    const id = options.stream ? this.takeId() : 0;
    const payload = options.raw ?? enc.encode(JSON.stringify({ t, ...args }));
    this.unitsFromService -= units(payload.length);
    this.send(frameBytes(T.event, 0, id, payload));
    return id;
  }
  /** A progress event of the applet on one of the service's requests. */
  progress(id: number, t: string, args: Record<string, unknown> = {}): void {
    const payload = enc.encode(JSON.stringify({ t, ...args }));
    this.unitsFromService -= units(payload.length);
    this.send(frameBytes(T.event, 0, id, payload));
  }
  /** The applet's reply to the service's request (or one Hub makes: `timeout`, `busy`, `cancelled`, `too_large`). */
  replyTo(id: number, object: Record<string, unknown>): void {
    this.serviceRequests.delete(id);
    this.send(jsonFrame(T.reply, id, object));
  }
  replyError(id: number, code: string, message = code): void {
    this.serviceRequests.delete(id);
    this.send(jsonFrame(T.reply, id, { code, message }, 0x01));
  }
  /** DATA toward the service; throws when it would pass the service's credit, unless `force`. */
  data(id: number, bytes: Uint8Array, end = false, force = false): void {
    const credit = this.creditFromService.get(id) ?? 0;
    if (bytes.length > credit && !force) throw new Error(`the applet would send ${bytes.length} bytes past the service's credit ${credit} on ${id}`);
    this.creditFromService.set(id, credit - bytes.length);
    this.send(frameBytes(T.data, end ? 1 : 0, id, bytes));
  }
  /** Byte credit toward the service on a stream. */
  credit(id: number, amount: number): void {
    this.creditForService.set(id, (this.creditForService.get(id) ?? 0) + amount);
    this.send(frameBytes(T.credit, 0, id, u32(amount)));
  }
  /** Event units toward the service (the device's grant, passed through). */
  grantUnits(amount: number): void {
    this.unitsForService += amount;
    this.send(frameBytes(T.credit, 0, this.channel, u32(amount)));
  }
  /** The applet cancels its request; Hub has answered it `cancelled` and forwards the CANCEL. */
  cancel(id: number): void {
    this.appletRequests.delete(id);
    this.cancelled.add(id);
    this.send(frameBytes(T.cancel, 0, id));
  }
  reset(id: number, code: keyof typeof RESET_CODE): void {
    this.outboundStreams.delete(id);
    this.send(frameBytes(T.reset, 0, id, u16pad(RESET_CODE[code])));
  }
  /** Hub's close notice, then Hub closes the socket. */
  close(reason: string): void {
    this.closeNoticeSent = true;
    this.send(jsonFrame(T.event, this.channel, { t: "channel.close", reason }));
    this.sock.end();
  }
  closeSocket(): void {
    this.sock.end();
  }
  creditFrom(id: number): number {
    return this.creditFromService.get(id) ?? 0;
  }
  /** Waits until the service's credit on `id` is at least `bytes`. */
  async waitCredit(id: number, bytes = 1, timeoutMs = 3000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while ((this.creditFromService.get(id) ?? 0) < bytes) {
      if (Date.now() > deadline) throw new Error(`no credit of ${bytes} on ${id}; frames: ${this.describe()}`);
      await new Promise<void>((resolve) => {
        const list = this.creditWaiters.get(id) ?? [];
        list.push(resolve);
        this.creditWaiters.set(id, list);
        setTimeout(resolve, 50);
      });
    }
  }

  /** Reads a stream from the service as an applet that grants `window` bytes and refills it as bytes arrive. */
  readStream(id: number, window = 32 * 1024): Promise<Uint8Array> {
    return new Promise((resolve, reject) => {
      const parts: Uint8Array[] = [];
      let sinceGrant = 0;
      const done = (error?: Error) => {
        this.consumers.delete(id);
        if (error) return reject(error);
        const total = parts.reduce((n, p) => n + p.length, 0);
        const out = new Uint8Array(total);
        let at = 0;
        for (const p of parts) {
          out.set(p, at);
          at += p.length;
        }
        resolve(out);
      };
      // Frames of the stream received before this call.
      const earlier = this.frames.filter((f) => f.id === id && (f.type === T.data || f.type === T.reset));
      const take = (frame: MFrame) => {
        if (frame.type === T.reset) return done(new Error(`stream ${id} reset: ${frame.value}`));
        if (frame.type === T.reply) return;
        if (frame.type !== T.data) return;
        parts.push(frame.payload);
        sinceGrant += frame.payload.length;
        if ((frame.flags & 1) !== 0) return done();
        if (sinceGrant >= window / 2) {
          this.credit(id, sinceGrant);
          sinceGrant = 0;
        }
      };
      this.consumers.set(id, take);
      for (const f of earlier) take(f);
      this.credit(id, window);
    });
  }

  /** Sends `bytes` to the service on `id` within the service's credit, cut at the payload limit, END on the last frame. */
  async sendStream(id: number, bytes: Uint8Array, chunk = this.payloadLimit): Promise<void> {
    let at = 0;
    while (at < bytes.length) {
      await this.waitCredit(id, 1, 5000);
      const n = Math.min(chunk, this.creditFrom(id), bytes.length - at);
      const last = at + n === bytes.length;
      this.data(id, bytes.subarray(at, at + n), last);
      at += n;
    }
    if (bytes.length === 0) this.data(id, new Uint8Array(0), true);
  }
}

export interface MockHubOptions {
  app?: string;
  token?: string;
  /** Answer `service.hello` with `service.refused` and this message. */
  refuse?: string;
  /** The manifest's `requires`, as `host.ready` gives it. */
  requires?: string[];
  /** Answer `host.attach` with `host.refused` and this message. */
  refuseHost?: string;
}

/** Hub's service socket, with the applet behind each channel. */
export class MockHub {
  readonly dir: string;
  readonly socketPath: string;
  readonly token: string;
  readonly app: string;
  readonly violations: string[] = [];
  readonly hellos: Record<string, any>[] = [];
  /** The host connection's end, once the service attached it. */
  host: MockHost | null = null;
  private hostSocket: MockSocket | null = null;
  private hostTicket = hex32();
  private hostWaiters: (() => void)[] = [];
  private server: UnixSocketListener<MockSocket> | null = null;
  private control: MockSocket | null = null;
  private controlWaiters: ((sock: MockSocket) => void)[] = [];
  private tickets = new Map<string, { resolve: (ch: MockChannel) => void; reject: (e: Error) => void; offer: OfferFields; channel: number; messageLimit: number; refuse?: string }>();
  readonly channels: MockChannel[] = [];
  private generation = 41;
  private nextChannel = 7;
  readonly controlClosed: Promise<void>;
  private resolveControlClosed!: () => void;

  private constructor(private readonly options: MockHubOptions) {
    this.dir = mkdtempSync(join(tmpdir(), "pocket-hub-service-"));
    this.socketPath = join(this.dir, "service.sock");
    this.token = options.token ?? hex32();
    this.app = options.app ?? "dev.pocket-hub.test";
    this.controlClosed = new Promise((resolve) => (this.resolveControlClosed = resolve));
  }

  static async start(options: MockHubOptions = {}): Promise<MockHub> {
    const hub = new MockHub(options);
    hub.server = Bun.listen<MockSocket>({
      unix: hub.socketPath,
      socket: {
        open(socket) {
          const sock = new MockSocket(16_384, (why) => hub.violations.push(why));
          sock.socket = socket;
          socket.data = sock;
          sock.onFrames = (frames) => hub.firstFrames(sock, frames);
        },
        data(socket, chunk) {
          const sock = socket.data;
          const frames = sock.decoder.push(chunk);
          if (frames.length > 0) sock.onFrames(frames);
        },
        drain(socket) {
          socket.data.flush();
        },
        close(socket) {
          const sock = socket.data;
          if (sock.closed) return;
          sock.closed = true;
          sock.onClose();
        },
        error(socket) {
          const sock = socket.data;
          if (sock.closed) return;
          sock.closed = true;
          sock.onClose();
        },
      },
    });
    return hub;
  }

  /** The three variables the host starts a service with (§2). */
  env(): Record<string, string> {
    return { POCKET_SERVICE_SOCKET: this.socketPath, POCKET_SERVICE_TOKEN: this.token, POCKET_SERVICE_ID: this.app };
  }
  runOptions() {
    return { socket: this.socketPath, token: this.token, app: this.app };
  }

  private firstFrames(sock: MockSocket, frames: MFrame[]): void {
    const [first, ...rest] = frames;
    if (first === undefined) return;
    if (first.type !== T.handshake || first.json === null) {
      this.violations.push(`a connection that opens with type 0x${first.type.toString(16)}, not a HANDSHAKE`);
      return sock.end();
    }
    if (first.json.t === "service.hello") {
      this.hellos.push(first.json);
      const { protocol, token, app } = first.json;
      if (this.options.refuse !== undefined || protocol !== 3 || token !== this.token || app !== this.app) {
        sock.send(jsonFrame(T.handshake, 0, { t: "service.refused", message: this.options.refuse ?? "unknown token" }));
        return sock.end();
      }
      this.control = sock;
      sock.onFrames = (more) => {
        for (const f of more) this.violations.push(`control: unexpected frame 0x${f.type.toString(16)} from the service`);
      };
      sock.onClose = () => this.resolveControlClosed();
      sock.send(jsonFrame(T.handshake, 0, { t: "service.ready", app: this.app, release: "dev", host: this.hostTicket }));
      for (const wake of this.controlWaiters.splice(0)) wake(sock);
      if (rest.length > 0) sock.onFrames(rest);
      return;
    }
    if (first.json.t === "host.attach") {
      if (first.json.ticket !== this.hostTicket || this.options.refuseHost !== undefined) {
        sock.send(jsonFrame(T.handshake, 0, { t: "host.refused", message: this.options.refuseHost ?? "unknown or used ticket" }));
        return sock.end();
      }
      this.hostTicket = hex32();
      if (rest.length > 0) this.violations.push("host: frames before host.ready");
      sock.decoder.limit = 65_536;
      this.hostSocket = sock;
      const host = new MockHost((bytes) => sock.send(bytes), (why) => this.violations.push(why));
      this.host = host;
      sock.onFrames = (more) => host.frames(more);
      sock.send(jsonFrame(T.handshake, 0, { t: "host.ready", payloadLimit: 65_536, requires: this.options.requires ?? [], provides: ["store"] }));
      for (const wake of this.hostWaiters.splice(0)) wake();
      return;
    }
    if (first.json.t === "channel.attach") {
      const ticket = this.tickets.get(first.json.ticket);
      if (ticket === undefined) {
        sock.send(jsonFrame(T.handshake, 0, { t: "channel.refused", message: "unknown ticket" }));
        return sock.end();
      }
      this.tickets.delete(first.json.ticket);
      if (ticket.refuse !== undefined) {
        sock.send(jsonFrame(T.handshake, 0, { t: "channel.refused", message: ticket.refuse }));
        sock.end();
        return ticket.reject(new Error(`refused: ${ticket.refuse}`));
      }
      sock.decoder.limit = ticket.offer.payloadLimit;
      const channel = new MockChannel(sock, ticket.channel, ticket.offer.payloadLimit, ticket.messageLimit, ticket.offer, this.violations, 100);
      this.channels.push(channel);
      sock.send(jsonFrame(T.handshake, 0, { t: "channel.ready", channel: ticket.channel, messageLimit: ticket.messageLimit }));
      if (rest.length > 0) sock.onFrames(rest);
      return ticket.resolve(channel);
    }
    this.violations.push(`a first HANDSHAKE ${JSON.stringify(first.json)}`);
    sock.end();
  }

  /** Resolves once the service said hello and got `service.ready`. */
  waitControl(timeoutMs = 3000): Promise<void> {
    if (this.control !== null) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("no service.hello")), timeoutMs);
      this.controlWaiters.push(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  /** Resolves once the service attached its host connection. */
  waitHost(timeoutMs = 3000): Promise<MockHost> {
    if (this.host !== null) return Promise.resolve(this.host);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("no host.attach")), timeoutMs);
      this.hostWaiters.push(() => {
        clearTimeout(timer);
        resolve(this.host!);
      });
    });
  }

  /** Ends the host connection: part of the lease. */
  closeHost(): void {
    this.hostSocket?.end();
  }

  /** Sends `channel.offer` and resolves with the applet's end once the service attached. */
  offer(fields: Partial<OfferFields> = {}, options: { channel?: number; messageLimit?: number; refuse?: string; timeoutMs?: number } = {}): Promise<MockChannel> {
    if (this.control === null) throw new Error("no control connection");
    const payloadLimit = fields.payloadLimit ?? 16_384;
    const offer: OfferFields = {
      ticket: hex32(),
      device: "0123456789abcdef",
      deviceName: "Test 3DS",
      model: "new-3ds-xl",
      target: "3ds",
      app: this.app,
      hash: "fedcba9876543210",
      generation: (this.generation += 1),
      payloadLimit,
      ...fields,
    };
    const channel = options.channel ?? this.nextChannel++;
    const messageLimit = options.messageLimit ?? Math.min(8192, payloadLimit - 4);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("the service did not attach")), options.timeoutMs ?? 3000);
      this.tickets.set(offer.ticket, {
        resolve: (ch) => {
          clearTimeout(timer);
          resolve(ch);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
        offer,
        channel,
        messageLimit,
        refuse: options.refuse,
      });
      this.control!.send(jsonFrame(T.event, 0, { t: "channel.offer", ...offer }));
    });
  }

  /** `service.stop` on the control connection. */
  stop(): void {
    this.control?.send(jsonFrame(T.event, 0, { t: "service.stop" }));
  }
  /** Ends the service's lease: the control connection closes. */
  closeControl(): void {
    this.control?.end();
  }
  sendControl(bytes: Uint8Array): void {
    this.control?.send(bytes);
  }

  async shutdown(): Promise<void> {
    this.server?.stop(true);
    this.server = null;
    rmSync(this.dir, { recursive: true, force: true });
  }
}
