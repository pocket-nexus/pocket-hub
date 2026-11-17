// One applet channel at the service's end: one channel connection to Hub
// (docs/SERVICE-PROTOCOL.md §5), carrying the frames of §7.2 of the link
// specification in the local dialect.
//
// The API mirrors the reference service API (vendor/pocketjs/tools/
// pocket-link/v2/peer.ts `AppletChannel`), so a service written for the
// reference Hub ports by changing its imports: `on`, `request`, `emit`,
// `emitStream`, `onEvent`, `state`, `onClose`, `close`, and the call and
// stream handles. The bookkeeping is the service side of the protocol
// document: ids from the Hub range, event units both ways, the head step,
// the message limit, and the bounds of 8 open requests and 4 outbound
// streams, all checked here before a frame is queued, so Hub never has to
// refuse one.

import { ServiceError, type Json, asServiceError } from "./errors.ts";
import {
  CHANNEL_REQUESTS,
  CHANNEL_STREAMS,
  ERROR_MESSAGE_BYTES,
  EVENT_WINDOW_SERVICE,
  FLAG_BODY,
  FLAG_END,
  FLAG_ERROR,
  FRAME,
  type Frame,
  ID_HUB_FIRST,
  ID_HUB_LAST,
  MAX_CREDIT,
  type ResetName,
  STREAM_WINDOW,
  encodeCancel,
  encodeCredit,
  encodeJsonBytes,
  encodeReset,
  encodedPrefix,
  eventUnits,
  idRange,
  jsonEncode,
  parseJson,
  readU16,
  readU32,
  resetName,
  validAppName,
  validErrorCode,
} from "./frame.ts";
import { type StreamOwner, StreamReader, StreamWriter } from "./streams.ts";
import { type Glyphs, type Images, type Media, glyphs, images, media } from "./capabilities.ts";

/** What the host vouches for about a channel (`channel.offer`, SERVICE-PROTOCOL.md §4). */
export interface ChannelInfo {
  /** The device id, authenticated by the link's handshake. */
  device: string;
  /** The device's display name, model and build target. */
  deviceName: string;
  model: string;
  target: string;
  /** The applet id, asserted by the device's runtime. */
  app: string;
  /** 16 hex: the running package's footer hash. */
  hash: string;
  /** The channel's generation (§7.4): larger than every one Hub gave before. */
  generation: number;
}

/** A request the service sent. */
export interface Call {
  /** The request's id (Hub range); 0 for a call refused before it was sent. */
  readonly id: number;
  /** The reply's value, or a ServiceError: the applet's error, one Hub made (`timeout`, `cancelled`, `busy`, `too_large`), `closed` when the channel ended, or a local refusal. */
  readonly reply: Promise<Json>;
  /** Sends CANCEL. The reply settles at the REPLY that follows: `cancelled`, or the applet's reply if it crossed the CANCEL. */
  cancel(): void;
  /** The request's body stream, when `body: true` was given. */
  readonly body: StreamWriter | null;
  /** The response stream, from the first progress event on (the head step, §7.2). */
  readonly response: StreamReader | null;
}

/** A request from the applet, as its handler sees it. */
export interface AppletCall {
  readonly t: string;
  readonly args: Json;
  /** True once the applet cancelled the request (Hub has answered it `cancelled`) or the channel closed. */
  readonly cancelled: boolean;
  /** Runs when the request is cancelled or its channel closes; at once (in a microtask) if that already happened. */
  onCancel(listener: () => void): void;
  /** A progress event on the request. False when the applet's event window has no room for it, or the request is over; try again later. */
  progress(t: string, args?: Json): boolean;
  /** The stream for response bytes. Throws before a progress event (the head step, §7.2), and `busy` when 4 streams toward the applet are open. */
  respond(): StreamWriter;
  /** The request's body stream, when the applet sent one; null otherwise (or when 4 inbound streams were open and it was refused). */
  readonly body: StreamReader | null;
}

export type AppletHandler = (args: Json, call: AppletCall) => Json | void | Promise<Json | void>;
export type AppletEventHandler = (args: Json, stream?: StreamReader) => void | Promise<void>;
export type ProgressHandler = (t: string, args: Json, stream?: StreamReader) => void;

/** @internal What the channel needs of its connection. */
export interface ChannelLink {
  send(bytes: Uint8Array, written?: () => void): void;
  /** Drops what is queued and not yet in the socket. */
  discardQueued(): void;
  /** Closes the socket once the queue is written. */
  end(): void;
}

interface OutCall {
  id: number;
  t: string;
  resolve: (value: Json) => void;
  reject: (error: ServiceError) => void;
  onProgress: ProgressHandler | null;
  body: StreamWriter | null;
  response: StreamReader | null;
  /** The head step was refused (4 inbound streams), or the applet reset the id before it. */
  responseGone: ServiceError | null;
  cancelled: boolean;
  /** The REPLY arrived; the outcome may wait behind response bytes being read. */
  replied: boolean;
  settled: boolean;
}

class IncomingCall {
  cancelled = false;
  replied = false;
  head = false;
  out: StreamWriter | null = null;
  body: StreamReader | null = null;
  /** CREDIT for the response that came before `respond()`. */
  earlyCredit = 0;
  /** RESET of the id that came before `respond()`. */
  earlyReset: ServiceError | null = null;
  readonly cancels: (() => void)[] = [];
  constructor(
    readonly id: number,
    readonly t: string,
    readonly args: Json,
  ) {}
}

/** Bytes of events and stream data the service may have queued toward Hub at once. */
const OUTBOUND_BOUND = STREAM_WINDOW;

const isPlainObject = (value: unknown): value is Json =>
  value !== null && typeof value === "object" && !Array.isArray(value) && !(value instanceof Uint8Array);

function withoutT(args: Json): Json {
  if (!("t" in args)) return args;
  const { t: _t, ...rest } = args;
  return rest;
}

/**
 * One applet channel, the service's end. A channel object exists from
 * `channel.ready` to its close; `service.attach(channel, info)` receives it.
 */
export class AppletChannel {
  private status: "open" | "closed" = "open";
  private reason: string | null = null;
  private nextId = ID_HUB_FIRST;
  /** Units the applet granted and the service has not spent (events and progress toward the applet). */
  private unitsOut = 0;
  /** Units the service granted and the applet has not spent. */
  private unitsIn = 0;
  /** Units of events that left the service's queue, not yet regranted. */
  private owed = 0;
  private regrantQueued = false;
  private readonly handlers = new Map<string, AppletHandler>();
  private readonly eventHandlers = new Map<string, AppletEventHandler>();
  private readonly outCalls = new Map<number, OutCall>();
  private readonly inCalls = new Map<number, IncomingCall>();
  /** Inbound streams by id: request bodies, responses, event streams. At most 4 open. */
  private readonly readers = new Map<number, StreamReader>();
  /** Outbound streams by id, until their END is queued. At most 4 open. */
  private readonly writers = new Map<number, StreamWriter>();
  /** Writers whose END is queued and not yet written: CREDIT and RESET still find them. */
  private readonly ending = new Map<number, StreamWriter>();
  /** Replies held behind response bytes the service is reading. */
  private readonly trailers = new Map<OutCall, Json | ServiceError>();
  private readonly closeListeners: ((reason: string) => void)[] = [];
  private roomWaiters: (() => void)[] = [];
  private paused: Frame[] | null = null;
  /** @internal */ outboundBytes = 0;
  /** @internal */ readonly outboundBound = OUTBOUND_BOUND;
  /** Counters a test or a status page reads. */
  readonly stats = { eventsIn: 0, eventsOut: 0, unitsRegranted: 0 };
  /** A network image in this device's format. Not provided by this host: `render` rejects with `not_required`. */
  readonly image: Images = images;
  /** Glyphs this device lacks. Not provided by this host: `atlas` rejects with `not_required`. */
  readonly text: Glyphs = glyphs;
  /** A stream this device can play. Not provided by this host: `open` rejects with `not_required`. */
  readonly media: Media = media;
  private readonly owner: StreamOwner;

  /** @internal */
  constructor(
    private readonly link: ChannelLink,
    /** The channel's id on this connection (`channel.ready`): CREDIT on it carries event units, `channel.close` travels on it. */
    readonly id: number,
    readonly info: ChannelInfo,
    /** The payload limit of the channel connection (`channel.offer`). */
    readonly payloadLimit: number,
    /** LINK_APP_MSG: the largest JSON object of the channel (`channel.ready`). */
    readonly messageLimit: number,
    private readonly diag: (line: string) => void,
  ) {
    // What the stream handles need of the channel, bound once; frames for a closed channel are not sent.
    const channel = this;
    this.owner = {
      payloadLimit,
      send: (bytes, written) => this.link.send(bytes, written),
      sendCredit: (sid, amount) => {
        if (this.status === "open" && amount > 0) this.link.send(encodeCredit(sid, amount));
      },
      sendReset: (sid, code) => {
        if (this.status === "open") this.link.send(encodeReset(sid, code));
      },
      readerDone: (reader) => {
        if (this.readers.get(reader.id) === reader) this.readers.delete(reader.id);
      },
      writerDone: (writer) => {
        if (this.writers.get(writer.id) !== writer) return;
        this.writers.delete(writer.id);
        if (!writer.done) {
          this.ending.set(writer.id, writer);
          void writer.finished.then(
            () => this.ending.delete(writer.id),
            () => this.ending.delete(writer.id),
          );
        }
      },
      get outboundBytes() {
        return channel.outboundBytes;
      },
      set outboundBytes(value: number) {
        channel.outboundBytes = value;
      },
      outboundBound: OUTBOUND_BOUND,
      released: () => this.wakeRoom(),
      waitRoom: () => this.waitRoom(),
    };
  }

  /** The applet id. */
  get app(): string {
    return this.info.app;
  }
  /** The running package's footer hash. */
  get hash(): string {
    return this.info.hash;
  }
  /** The channel's generation (§7.4). */
  get generation(): number {
    return this.info.generation;
  }
  /** The device id. */
  get device(): string {
    return this.info.device;
  }

  /** `open` or `closed`, with the close reason. */
  state(): { state: "open" | "closed"; reason: string | null } {
    return { state: this.status, reason: this.reason };
  }
  /** Runs once when the channel closes, whoever closed it, with the reason. */
  onClose(listener: (reason: string) => void): void {
    if (this.status === "closed") queueMicrotask(() => this.runListener(() => listener(this.reason ?? "closed")));
    else this.closeListeners.push(listener);
  }
  /** Event units the applet granted and the service has not spent. */
  get units(): number {
    return this.unitsOut;
  }

  // --- the service's calls ---------------------------------------------------

  /**
   * Sends a request to the applet. Refused at once, with nothing sent, when
   * the channel is closed (`no_channel`), `t` is not 1–64 bytes of
   * [a-z0-9._-] (`malformed`), the object is over `messageLimit`
   * (`too_large`), 8 requests of the service are open (`busy`), or a body is
   * asked for while 4 streams toward the applet are open (`busy`).
   */
  request(t: string, args: Json = {}, options: { body?: boolean; onProgress?: ProgressHandler } = {}): Call {
    const refuse = (error: ServiceError): Call => {
      const reply = Promise.reject(error);
      reply.catch(() => {});
      return { id: 0, reply, cancel() {}, body: null, response: null };
    };
    if (this.status !== "open") return refuse(new ServiceError("no_channel", "the channel is closed"));
    if (!validAppName(t)) return refuse(new ServiceError("malformed", "an operation name is 1 to 64 bytes of [a-z0-9._-]"));
    let json: Uint8Array;
    try {
      json = jsonEncode({ t, ...withoutT(args) });
    } catch (error) {
      return refuse(new ServiceError("malformed", `the arguments are not JSON: ${asServiceError(error).message}`));
    }
    if (json.length > this.messageLimit) return refuse(new ServiceError("too_large", `a request of ${json.length} bytes is over the channel's ${this.messageLimit}`));
    if (this.outCalls.size >= CHANNEL_REQUESTS) return refuse(new ServiceError("busy", `${CHANNEL_REQUESTS} requests of the service are open`));
    if (options.body === true && this.writers.size >= CHANNEL_STREAMS) return refuse(new ServiceError("busy", `${CHANNEL_STREAMS} streams toward the applet are open`));
    const id = this.takeId();
    let resolve!: (value: Json) => void;
    let reject!: (error: ServiceError) => void;
    const reply = new Promise<Json>((ok, no) => {
      resolve = ok;
      reject = no;
    });
    reply.catch(() => {});
    const call: OutCall = {
      id,
      t,
      resolve,
      reject,
      onProgress: options.onProgress ?? null,
      body: null,
      response: null,
      responseGone: null,
      cancelled: false,
      replied: false,
      settled: false,
    };
    this.outCalls.set(id, call);
    this.link.send(encodeJsonBytes(FRAME.request, id, json, this.payloadLimit, options.body === true ? FLAG_BODY : 0));
    if (options.body === true) call.body = this.newWriter(id, false);
    return {
      id,
      reply,
      cancel: () => this.cancelOut(call),
      body: call.body,
      get response() {
        return call.response;
      },
    };
  }

  /** The handler of the applet's request `t`; its return value is the reply, a thrown ServiceError the error reply. */
  on(t: string, handler: AppletHandler): void {
    this.handlers.set(t, handler);
  }

  /**
   * Sends an event to the applet. False, with nothing sent or queued, when
   * the channel is closed, the applet's event window has too few units for
   * it, or 256 KiB are already queued toward Hub; try again later. Throws a
   * ServiceError for an event that can never be sent: `malformed` for a bad
   * `t` or arguments that are not JSON, `too_large` over `messageLimit`.
   */
  emit(t: string, args: Json = {}): boolean {
    return this.sendEvent(t, args, false) !== null;
  }

  /** An event that opens a stream toward the applet; null where `emit` gives false, or while 4 streams toward the applet are open. */
  emitStream(t: string, args: Json = {}): StreamWriter | null {
    if (this.status === "open" && this.writers.size >= CHANNEL_STREAMS) {
      // Checked first so a bad `t` still throws below only when there is room.
      this.checkEvent(t, args);
      return null;
    }
    const id = this.sendEvent(t, args, true);
    if (id === null) return null;
    return this.newWriter(id, true);
  }

  /**
   * The handler of the applet's event `t`. For an event that opens a stream,
   * a handler declared with two parameters gets the stream; a stream event
   * with no handler, or with a handler of one parameter, is answered
   * `RESET refused` (§7.2) and the handler (if any) gets the arguments alone.
   */
  onEvent(t: string, handler: AppletEventHandler): void {
    this.eventHandlers.set(t, handler);
  }

  /**
   * Ends the channel from the service's side (§7.5): `channel.close` goes out
   * behind the frames already queued, then the socket closes. Every pending
   * call ends with `closed`. The device is told `daemon`, the link's close
   * reason (§7.5), whatever the reason here.
   */
  close(reason: string = "service"): void {
    if (this.status !== "open") return;
    this.link.send(encodeJsonBytes(FRAME.event, this.id, jsonEncode({ t: "channel.close", reason }), this.payloadLimit));
    this.ended(reason, "service");
  }

  // --- the connection's side ------------------------------------------------

  /** @internal Right after `channel.ready`: the service's grant of 256 event units (SERVICE-PROTOCOL.md §5). */
  start(): void {
    this.unitsIn = EVENT_WINDOW_SERVICE;
    this.link.send(encodeCredit(this.id, EVENT_WINDOW_SERVICE));
  }

  /** @internal Holds frames until `release()` (while an async `attach` registers its handlers). */
  pause(): void {
    if (this.paused === null) this.paused = [];
  }
  /** @internal */
  release(): void {
    const held = this.paused;
    this.paused = null;
    if (held !== null && held.length > 0) this.frames(held);
  }

  /** @internal Frames Hub sent on the channel connection after `channel.ready`. */
  frames(frames: Frame[]): void {
    if (this.paused !== null) {
      this.paused.push(...frames);
      return;
    }
    for (const frame of frames) {
      if (this.status !== "open") return;
      try {
        this.frame(frame);
      } catch (error) {
        this.diag(`channel ${this.id}: ${asServiceError(error).message}`);
        this.violation("a frame the service could not read");
      }
    }
  }

  /** @internal The socket closed without a `channel.close` (Hub went away). */
  connectionClosed(): void {
    this.ended("connection", "socket");
  }

  /** @internal Hub's frames broke the local dialect's rules. */
  broken(why: string): void {
    this.diag(`channel ${this.id}: ${why}`);
    this.violation(why);
  }

  // --- frames from Hub -------------------------------------------------------

  private frame(frame: Frame): void {
    switch (frame.type) {
      case FRAME.request:
        return this.onRequest(frame);
      case FRAME.reply:
        return this.onReply(frame);
      case FRAME.event:
        return this.onEventFrame(frame);
      case FRAME.cancel:
        return this.onCancel(frame.id);
      case FRAME.data:
        return this.onData(frame);
      case FRAME.credit:
        return this.onCredit(frame);
      case FRAME.reset:
        return this.onReset(frame);
      default:
        // HANDSHAKE after `channel.ready`, PING, PONG, CLOSE: no meaning on a channel connection.
        return;
    }
  }

  private onRequest(frame: Frame): void {
    const id = frame.id;
    if (idRange(id) !== "device" || this.inCalls.has(id) || this.readers.has(id)) {
      return this.violation(`REQUEST id ${id} is not a fresh id of the applet`);
    }
    const object = parseJson(frame.payload);
    if (frame.payload.length > this.messageLimit) return this.replyError(id, new ServiceError("too_large", `a request of ${frame.payload.length} bytes`));
    if (object === null || typeof object.t !== "string") return this.replyError(id, new ServiceError("malformed", "not a JSON object with a string t"));
    if (this.inCalls.size >= CHANNEL_REQUESTS) return this.replyError(id, new ServiceError("busy", `${CHANNEL_REQUESTS} requests of the applet are open`));
    const handler = this.handlers.get(object.t);
    if (handler === undefined) return this.replyError(id, new ServiceError("unknown_message", `no handler for ${object.t}`));
    const args = withoutT(object);
    const call = new IncomingCall(id, object.t, args);
    this.inCalls.set(id, call);
    if ((frame.flags & FLAG_BODY) !== 0) {
      if (this.readers.size >= CHANNEL_STREAMS) this.owner.sendReset(id, "refused");
      else call.body = this.newReader(id);
    }
    const api = this.callApi(call);
    // The handler starts in a microtask: the request's body frames read in this turn already have their reader.
    Promise.resolve()
      .then(() => handler(args, api))
      .then(
        (value) => this.replyTo(call, value),
        (error) => this.failCall(call, error),
      );
  }

  private callApi(call: IncomingCall): AppletCall {
    const channel = this;
    return {
      t: call.t,
      args: call.args,
      get cancelled() {
        return call.cancelled;
      },
      onCancel(listener) {
        if (call.cancelled) queueMicrotask(() => channel.runListener(listener));
        else call.cancels.push(listener);
      },
      progress: (t, more = {}) => this.progress(call, t, more),
      respond: () => this.respond(call),
      get body() {
        return call.body;
      },
    };
  }

  private progress(call: IncomingCall, t: string, args: Json): boolean {
    const json = this.checkEvent(t, args);
    if (this.status !== "open" || call.replied || call.cancelled) return false;
    const cost = eventUnits(json.length);
    if (cost > this.unitsOut) return false;
    this.unitsOut -= cost;
    this.stats.eventsOut += 1;
    call.head = true;
    this.link.send(encodeJsonBytes(FRAME.event, call.id, json, this.payloadLimit));
    return true;
  }

  private respond(call: IncomingCall): StreamWriter {
    if (call.out !== null) return call.out;
    if (call.cancelled || this.status !== "open") throw new ServiceError("cancelled", "the request is over");
    if (call.replied) throw new ServiceError("internal", "respond() after the reply");
    if (!call.head) throw new ServiceError("internal", "respond() before a progress event: the head step (§7.2) comes first");
    if (this.writers.size >= CHANNEL_STREAMS) throw new ServiceError("busy", `${CHANNEL_STREAMS} streams toward the applet are open`);
    const writer = this.newWriter(call.id, false);
    call.out = writer;
    if (call.earlyReset !== null) writer.dead(call.earlyReset);
    else if (call.earlyCredit > 0) {
      const credit = call.earlyCredit;
      call.earlyCredit = 0;
      if (!writer.grant(credit, MAX_CREDIT)) this.violation("a grant over 16 MiB outstanding");
    }
    return writer;
  }

  private async replyTo(call: IncomingCall, value: Json | void): Promise<void> {
    if (call.replied || call.cancelled) return;
    if (call.out !== null && !call.out.done) {
      call.out.end();
      try {
        await call.out.finished;
      } catch {
        // A reset response: the request still ends with its REPLY (§5.6).
      }
    }
    if (call.replied || call.cancelled || this.status !== "open") return;
    const object = value ?? {};
    if (!isPlainObject(object)) return this.failCall(call, new ServiceError("internal", "a reply is one JSON object"));
    let json: Uint8Array;
    try {
      json = jsonEncode(object);
    } catch (error) {
      return this.failCall(call, new ServiceError("internal", `the reply is not JSON: ${asServiceError(error).message}`));
    }
    if (json.length > this.messageLimit) return this.failCall(call, new ServiceError("too_large", `a reply of ${json.length} bytes is over the channel's ${this.messageLimit}`));
    this.finishCall(call);
    this.link.send(encodeJsonBytes(FRAME.reply, call.id, json, this.payloadLimit));
  }

  private failCall(call: IncomingCall, error: unknown): void {
    if (call.replied || call.cancelled || this.status !== "open") return;
    const e = asServiceError(error);
    if (!(error instanceof ServiceError)) this.diag(`handler of ${call.t} failed: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
    this.finishCall(call);
    call.out?.dead(e);
    this.replyError(call.id, e);
  }

  private finishCall(call: IncomingCall): void {
    call.replied = true;
    if (this.inCalls.get(call.id) === call) this.inCalls.delete(call.id);
  }

  private replyError(id: number, error: ServiceError): void {
    const code = validErrorCode(error.code) ? error.code : "internal";
    const message = encodedPrefix(code === error.code ? error.message : `${error.code}: ${error.message}`, ERROR_MESSAGE_BYTES);
    let json = jsonEncode({ ...error.extra, code, message });
    // An error object with more than fits: its own fields alone.
    if (json.length > this.messageLimit) json = jsonEncode({ code, message });
    this.link.send(encodeJsonBytes(FRAME.reply, id, json, this.payloadLimit, FLAG_ERROR));
  }

  private onReply(frame: Frame): void {
    const call = this.outCalls.get(frame.id);
    if (call === undefined || call.replied) return;
    let outcome: Json | ServiceError;
    const object = parseJson(frame.payload);
    if (frame.payload.length > this.messageLimit) outcome = new ServiceError("too_large", `a reply of ${frame.payload.length} bytes`);
    else if ((frame.flags & FLAG_ERROR) !== 0) {
      const { code, message, ...extra } = object ?? {};
      outcome = new ServiceError(typeof code === "string" ? code : "internal", typeof message === "string" ? message : "", extra);
    } else outcome = object ?? new ServiceError("malformed", "a reply that is not a JSON object");
    call.replied = true;
    // The request's place is free at its REPLY, as Hub counts it.
    this.outCalls.delete(call.id);
    if (call.body !== null && !call.body.done) call.body.dead(outcome instanceof ServiceError ? outcome : new ServiceError("closed", "the request ended"));
    const reader = call.response;
    // The REPLY is the trailer (§7.3): it waits behind response bytes the service is still reading.
    if (reader !== null && !reader.done && reader.heldBytes > 0) return this.holdReply(call, reader, outcome);
    this.settle(call, outcome);
  }

  private holdReply(call: OutCall, reader: StreamReader, outcome: Json | ServiceError): void {
    this.trailers.set(call, outcome);
    reader.onDrained = () => this.settle(call, outcome);
    // A reader that stops reading does not hold the reply: one turn without a read() hands it over.
    let reads = reader.reads;
    const check = () => {
      if (call.settled) return;
      if (reader.reads === reads || reader.heldBytes === 0 || reader.done) return this.settle(call, outcome);
      reads = reader.reads;
      setImmediate(check);
    };
    setImmediate(check);
  }

  private settle(call: OutCall, outcome: Json | ServiceError): void {
    if (call.settled) return;
    call.settled = true;
    this.trailers.delete(call);
    if (call.response !== null) call.response.onDrained = null;
    this.outCalls.delete(call.id);
    if (outcome instanceof ServiceError) {
      call.body?.dead(outcome);
      call.response?.dead(outcome);
      call.reject(outcome);
    } else call.resolve(outcome);
  }

  private cancelOut(call: OutCall): void {
    if (call.cancelled || call.replied || call.settled || this.status !== "open") return;
    call.cancelled = true;
    this.link.send(encodeCancel(call.id));
    // The requester discards the id's events and bytes after CANCEL (§5.4); the reply still comes.
    const stopped = new ServiceError("cancelled", "the service cancelled the request");
    call.body?.dead(stopped);
    call.response?.dead(stopped);
  }

  private onCancel(id: number): void {
    const call = this.inCalls.get(id);
    if (call === undefined) return;
    // Hub has answered the applet `cancelled` (SERVICE-PROTOCOL.md §5): no REPLY, nothing more for the id.
    this.inCalls.delete(id);
    this.endCall(call, new ServiceError("cancelled", "the applet cancelled the request"));
  }

  private endCall(call: IncomingCall, error: ServiceError): void {
    if (call.replied || call.cancelled) return;
    call.cancelled = true;
    call.out?.dead(error);
    call.body?.dead(error);
    for (const listener of call.cancels.splice(0)) this.runListener(listener);
  }

  private onEventFrame(frame: Frame): void {
    const id = frame.id;
    const object = parseJson(frame.payload);
    const t = object !== null && typeof object.t === "string" ? object.t : null;
    const bytes = frame.payload.length;
    // `channel.close` on the channel's id (§5, "Close"). It costs no units.
    if (id === this.id && id !== 0) {
      if (t === "channel.close") this.ended(typeof object?.reason === "string" ? object.reason : "closed", "hub");
      return;
    }
    // Every other event of the channel costs units (§5.6), counted before anything else.
    const cost = eventUnits(bytes);
    if (cost > this.unitsIn) return this.violation(`an event of ${cost} units past the ${this.unitsIn} granted`);
    this.unitsIn -= cost;
    this.stats.eventsIn += 1;
    const usable = object !== null && t !== null && bytes <= this.messageLimit;
    const args = usable ? withoutT(object) : {};
    if (id === 0) {
      // A standalone event.
      this.unitsLeft(cost);
      if (usable) this.deliverEvent(t, args, undefined);
      return;
    }
    const out = this.outCalls.get(id);
    if (out !== undefined) {
      // Progress of one of the service's requests (§5.3, step 1).
      this.unitsLeft(cost);
      if (!usable || out.cancelled || out.replied) return;
      if (out.response === null && out.responseGone === null) {
        // The head step: the response stream is handed over at the first progress event (§7.2).
        if (this.readers.size >= CHANNEL_STREAMS) {
          this.owner.sendReset(id, "refused");
          out.responseGone = new ServiceError("transfer", "reset: refused", { reset: "refused" });
        } else out.response = this.newReader(id);
      }
      const stream = out.response ?? undefined;
      try {
        out.onProgress?.(t, args, stream);
      } catch (error) {
        this.diag(`onProgress of ${out.t} failed: ${asServiceError(error).message}`);
      }
      return;
    }
    if (idRange(id) === "hub") {
      // Late progress of a request the service released (§5.3, step 3): ignored, its units return.
      this.unitsLeft(cost);
      return;
    }
    if (this.inCalls.has(id) || this.readers.has(id)) return this.violation(`an event on id ${id}, which is in use`);
    // A fresh id of the applet: the event opens a stream toward the service.
    this.unitsLeft(cost);
    if (!usable) return this.owner.sendReset(id, "refused");
    const handler = this.eventHandlers.get(t);
    if (handler === undefined || handler.length < 2 || this.readers.size >= CHANNEL_STREAMS) {
      // A stream nobody takes is refused at once, so the applet does not hold its bytes (§7.2).
      this.owner.sendReset(id, "refused");
      if (handler !== undefined) this.deliverEvent(t, args, undefined);
      return;
    }
    this.deliverEvent(t, args, this.newReader(id));
  }

  private deliverEvent(t: string, args: Json, stream: StreamReader | undefined): void {
    const handler = this.eventHandlers.get(t);
    if (handler === undefined) return;
    try {
      const result = stream === undefined ? handler(args) : handler(args, stream);
      if (result instanceof Promise) result.catch((error: unknown) => this.diag(`event handler of ${t} failed: ${asServiceError(error).message}`));
    } catch (error) {
      this.diag(`event handler of ${t} failed: ${asServiceError(error).message}`);
    }
  }

  private onData(frame: Frame): void {
    const reader = this.readers.get(frame.id);
    if (reader === undefined) return;
    if (!reader.data(frame.payload, (frame.flags & FLAG_END) !== 0)) this.violation(`DATA past the credit granted on stream ${frame.id}`);
  }

  private onCredit(frame: Frame): void {
    const amount = readU32(frame.payload, 0);
    if (frame.id === this.id) {
      this.unitsOut += amount;
      return;
    }
    const writer = this.writers.get(frame.id) ?? this.ending.get(frame.id);
    if (writer !== undefined) {
      if (!writer.grant(amount, MAX_CREDIT)) this.violation("a grant over 16 MiB outstanding");
      return;
    }
    // The applet may grant the response before `respond()` (§5.6): kept for the writer.
    const call = this.inCalls.get(frame.id);
    if (call !== undefined && call.out === null) call.earlyCredit += amount;
  }

  private onReset(frame: Frame): void {
    const name = resetName(readU16(frame.payload, 0)) ?? "unknown";
    const error = new ServiceError("transfer", `reset by the applet: ${name}`, { reset: name });
    const id = frame.id;
    this.readers.get(id)?.dead(error);
    (this.writers.get(id) ?? this.ending.get(id))?.dead(error);
    const incoming = this.inCalls.get(id);
    if (incoming !== undefined && incoming.out === null) incoming.earlyReset = error;
    const out = this.outCalls.get(id);
    if (out !== undefined && out.response === null) out.responseGone = error;
  }

  // --- units, ids, streams ---------------------------------------------------

  /** An event left the service's queue: its units are owed back, in one CREDIT per turn (§5.6). */
  private unitsLeft(cost: number): void {
    this.owed += cost;
    if (this.regrantQueued) return;
    this.regrantQueued = true;
    queueMicrotask(() => {
      this.regrantQueued = false;
      if (this.status !== "open" || this.owed === 0) return;
      const units = this.owed;
      this.owed = 0;
      this.unitsIn += units;
      this.stats.unitsRegranted += units;
      this.link.send(encodeCredit(this.id, units));
    });
  }

  /** Validates an outgoing event and returns its JSON. Throws `malformed` or `too_large`. */
  private checkEvent(t: string, args: Json): Uint8Array {
    if (!validAppName(t)) throw new ServiceError("malformed", "an operation name is 1 to 64 bytes of [a-z0-9._-]");
    let json: Uint8Array;
    try {
      json = jsonEncode({ t, ...withoutT(args) });
    } catch (error) {
      throw new ServiceError("malformed", `the arguments are not JSON: ${asServiceError(error).message}`);
    }
    if (json.length > this.messageLimit) throw new ServiceError("too_large", `an event of ${json.length} bytes is over the channel's ${this.messageLimit}`);
    return json;
  }

  /** Sends an event (id 0, or a fresh id when it opens a stream). Returns the id, or null when there is no room. */
  private sendEvent(t: string, args: Json, stream: boolean): number | null {
    const json = this.checkEvent(t, args);
    if (this.status !== "open") return null;
    if (this.outboundBytes > 0 && this.outboundBytes + json.length > OUTBOUND_BOUND) return null;
    const cost = eventUnits(json.length);
    if (cost > this.unitsOut) return null;
    this.unitsOut -= cost;
    this.stats.eventsOut += 1;
    const id = stream ? this.takeId() : 0;
    const bytes = json.length;
    this.outboundBytes += bytes;
    this.link.send(encodeJsonBytes(FRAME.event, id, json, this.payloadLimit), () => {
      this.outboundBytes -= bytes;
      this.wakeRoom();
    });
    return id;
  }

  /** The service's ids: one counter per channel connection, from 0x80000001, never reused (SERVICE-PROTOCOL.md §5). */
  private takeId(): number {
    if (this.nextId === this.id) this.nextId += 1;
    if (this.nextId > ID_HUB_LAST) throw new ServiceError("busy", "the channel connection used all its ids");
    return this.nextId++;
  }

  private newReader(id: number): StreamReader {
    const reader = new StreamReader(this.owner, id, STREAM_WINDOW);
    this.readers.set(id, reader);
    return reader;
  }

  private newWriter(id: number, giveUp: boolean): StreamWriter {
    const writer = new StreamWriter(this.owner, id, { giveUp });
    this.writers.set(id, writer);
    return writer;
  }

  private waitRoom(): Promise<void> {
    if (this.status !== "open") return Promise.resolve();
    return new Promise((resolve) => this.roomWaiters.push(resolve));
  }
  private wakeRoom(): void {
    if (this.roomWaiters.length === 0) return;
    const waiters = this.roomWaiters;
    this.roomWaiters = [];
    for (const wake of waiters) wake();
  }

  private runListener(listener: () => void): void {
    try {
      listener();
    } catch (error) {
      this.diag(`a listener failed: ${asServiceError(error).message}`);
    }
  }

  /** Hub's frames broke a rule: the service closes the channel, as Hub does for the service's (SERVICE-PROTOCOL.md §5). */
  private violation(why: string): void {
    if (this.status !== "open") return;
    this.diag(`channel ${this.id} closed: ${why}`);
    this.close("overflow");
  }

  /**
   * The channel ended: `channel.close` from Hub, the service's own close, or
   * the socket closing. Every pending call ends with `closed`, every stream
   * is dropped, and the close listeners run once.
   */
  private ended(reason: string, by: "hub" | "service" | "socket"): void {
    if (this.status !== "open") return;
    this.status = "closed";
    this.reason = reason;
    // Hub sends nothing of the channel after its close notice and takes nothing after it (§5 "Close").
    if (by === "hub") this.link.discardQueued();
    this.link.end();
    const error = new ServiceError("closed", `the channel closed: ${reason}`);
    for (const [call, outcome] of [...this.trailers]) this.settle(call, outcome);
    for (const call of [...this.outCalls.values()]) this.settle(call, error);
    for (const call of [...this.inCalls.values()]) this.endCall(call, error);
    this.inCalls.clear();
    for (const reader of [...this.readers.values()]) reader.dead(error);
    for (const writer of [...this.writers.values(), ...this.ending.values()]) writer.dead(error);
    this.wakeRoom();
    for (const listener of this.closeListeners.splice(0)) this.runListener(() => listener(reason));
  }
}
