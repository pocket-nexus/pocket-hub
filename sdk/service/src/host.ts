// The host connection at the service's end (docs/SERVICE-PROTOCOL.md §6), the
// one capability this host provides on it, `store` (§7), and the `Host` a
// service's factory receives (§2).
//
// One connection per process, opened right after `service.ready` with its
// `host` ticket. The service is the requester here: its requests take ids
// 1–0x7fffffff from one counter that never goes back. Bytes move as streams
// under credit both ways, with the channel's stream handles: a `store.set`
// body goes out only as the host grants, and a `store.get` value is granted
// 256 KiB at a time while it is read.

import { Connection } from "./connection.ts";
import { native, net, type Native, type Net } from "./capabilities.ts";
import { type Json, ServiceError } from "./errors.ts";
import {
  FLAG_BODY,
  FLAG_END,
  FLAG_ERROR,
  FRAME,
  type Frame,
  ID_DEVICE_MAX,
  MAX_CREDIT,
  STREAM_WINDOW,
  encodeCancel,
  encodeCredit,
  encodeHandshake,
  encodeJson,
  encodeReset,
  parseJson,
  readU16,
  readU32,
  resetName,
} from "./frame.ts";
import { type StreamOwner, StreamReader, StreamWriter } from "./streams.ts";

/** The payload limit of the host connection (§6). */
export const HOST_PAYLOAD = 65_536;

/** What a call accepts as bytes: a whole buffer, text (UTF-8), or chunks produced as the host takes them. An iterable is read only as fast as the host grants credit. */
export type ByteSource = Uint8Array | string | AsyncIterable<Uint8Array>;

type Opt = { signal?: AbortSignal };

/** The service's own key-value storage (§7): one store per applet id on this host, kept across installs. */
export interface Store {
  /** The value, or null when the key is absent. */
  get(key: string, options?: Opt): Promise<Uint8Array | null>;
  getText(key: string, options?: Opt): Promise<string | null>;
  getJson<T = unknown>(key: string, options?: Opt): Promise<T | null>;
  /** Atomic and durable before it resolves. `bytes` is required with an iterable value. */
  set(key: string, value: ByteSource, options?: Opt & { bytes?: number }): Promise<void>;
  setJson(key: string, value: unknown, options?: Opt): Promise<void>;
  /** Whether the key existed. */
  delete(key: string, options?: Opt): Promise<boolean>;
  /** Keys in byte order, at most `limit` (1,000), after `after`; `more` says whether a later one exists. */
  list(options?: Opt & { prefix?: string; after?: string; limit?: number }): Promise<{ keys: string[]; more: boolean }>;
  usage(options?: Opt): Promise<{ bytes: number; keys: number; maxBytes: number }>;
}

/** What a service gets from its host (§2). */
export interface Host {
  /** The applet id this service serves. */
  readonly id: string;
  /** One line to the service's log. */
  log(line: string): void;
  readonly net: Net;
  readonly store: Store;
  /** Present on every host; its members fail with not_required unless the manifest requires "native". */
  readonly native: Native;
}

interface Pending {
  resolve: (outcome: { reply: Json; value: Uint8Array | null }) => void;
  reject: (error: ServiceError) => void;
  /** The response stream, from its head event on. */
  reader: StreamReader | null;
  collected: Promise<Uint8Array> | null;
  /** The request's body. */
  writer: StreamWriter | null;
  done: boolean;
}

const utf8 = new TextEncoder();
const utf8Text = new TextDecoder();

function concat(chunks: Uint8Array[]): Uint8Array {
  if (chunks.length === 1) return chunks[0]!;
  const out = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0));
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.length;
  }
  return out;
}

/** The host connection, after `host.ready`. */
export class HostLink {
  private nextId = 1;
  private readonly calls = new Map<number, Pending>();
  private closedListeners: (() => void)[] = [];
  private roomWaiters: (() => void)[] = [];
  private isClosed = false;
  private readonly owner: StreamOwner;
  /** @internal */ outboundBytes = 0;

  private constructor(
    private readonly connection: Connection,
    /** The manifest's `requires`, as `host.ready` gave it. */
    readonly requires: string[],
    private readonly diag: (line: string) => void,
  ) {
    const link = this;
    this.owner = {
      payloadLimit: HOST_PAYLOAD,
      send: (bytes, written) => this.connection.send(bytes, written),
      sendCredit: (id, amount) => amount > 0 && this.connection.send(encodeCredit(id, amount)),
      sendReset: (id, code) => this.connection.send(encodeReset(id, code)),
      readerDone: () => {},
      writerDone: () => {},
      get outboundBytes() {
        return link.outboundBytes;
      },
      set outboundBytes(value: number) {
        link.outboundBytes = value;
      },
      outboundBound: STREAM_WINDOW,
      released: () => {
        for (const wake of link.roomWaiters.splice(0)) wake();
      },
      waitRoom: () => new Promise<void>((resolve) => link.roomWaiters.push(resolve)),
    };
  }

  /**
   * Opens the host connection with the `host` ticket of `service.ready` and
   * waits for `host.ready` (§6). Rejects on `host.refused` or a close.
   */
  static async attach(socket: string, ticket: string, diag: (line: string) => void, timeoutMs = 10_000): Promise<HostLink> {
    const connection = await Connection.open(socket, HOST_PAYLOAD);
    return new Promise((resolve, reject) => {
      let link: HostLink | null = null;
      const timer = setTimeout(() => {
        connection.destroy();
        reject(new Error("no host.ready"));
      }, timeoutMs);
      connection.listen({
        frames: (frames) => {
          if (link !== null) return link.frames(frames);
          for (let i = 0; i < frames.length; i++) {
            const frame = frames[i]!;
            const object = frame.type === FRAME.handshake ? parseJson(frame.payload) : null;
            if (object?.t === "host.refused") {
              clearTimeout(timer);
              reject(new Error(`the host refused the host connection: ${String(object.message ?? "")}`));
              return connection.destroy();
            }
            if (object?.t !== "host.ready") continue;
            clearTimeout(timer);
            const requires = Array.isArray(object.requires) ? object.requires.filter((name): name is string => typeof name === "string") : [];
            link = new HostLink(connection, requires, diag);
            resolve(link);
            return link.frames(frames.slice(i + 1));
          }
        },
        broken: (error) => {
          diag(`the host connection broke: ${error.message}`);
          connection.destroy();
        },
        closed: () => {
          clearTimeout(timer);
          if (link === null) reject(new Error("the host connection closed before host.ready"));
          else link.ended();
        },
      });
      // After host.attach the service sends nothing until host.ready (§6).
      connection.send(encodeHandshake({ t: "host.attach", ticket }));
    });
  }

  get closed(): boolean {
    return this.isClosed;
  }

  onClosed(listener: () => void): void {
    if (this.isClosed) listener();
    else this.closedListeners.push(listener);
  }

  destroy(): void {
    this.connection.destroy();
  }

  private ended(): void {
    if (this.isClosed) return;
    this.isClosed = true;
    for (const [id, call] of this.calls) this.settle(id, call, new ServiceError("closed", "the host connection closed"));
    for (const listener of this.closedListeners.splice(0)) listener();
  }

  /** Ends the connection: the host broke the protocol. */
  private broken(why: string): void {
    this.diag(`the host connection: ${why}`);
    this.connection.destroy();
  }

  private settle(id: number, call: Pending, outcome: ServiceError | { reply: Json; value: Uint8Array | null }): void {
    if (call.done) return;
    call.done = true;
    this.calls.delete(id);
    if (outcome instanceof ServiceError) {
      call.reader?.dead(outcome);
      call.writer?.dead(outcome);
      call.reject(outcome);
    } else {
      call.writer?.dead(new ServiceError("transfer", "the request ended"));
      call.resolve(outcome);
    }
  }

  private frames(frames: Frame[]): void {
    for (const frame of frames) {
      if (this.isClosed) return;
      const call = this.calls.get(frame.id);
      switch (frame.type) {
        case FRAME.reply: {
          if (call === undefined) continue;
          const object = parseJson(frame.payload) ?? {};
          if ((frame.flags & FLAG_ERROR) !== 0) {
            const { code, message, ...extra } = object;
            this.settle(frame.id, call, new ServiceError(typeof code === "string" ? code : "internal", typeof message === "string" ? message : String(code), extra));
          } else if (call.collected !== null) {
            // The value's bytes came before the reply (§6); the outcome waits until they are read.
            call.collected.then(
              (value) => this.settle(frame.id, call, { reply: object, value }),
              (error: unknown) => this.settle(frame.id, call, error instanceof ServiceError ? error : new ServiceError("transfer", String(error))),
            );
          } else {
            this.settle(frame.id, call, { reply: object, value: null });
          }
          break;
        }
        case FRAME.event: {
          // A head event opens the response stream (the head step, §6).
          if (call === undefined || frame.id === 0 || call.reader !== null) continue;
          const reader = new StreamReader(this.owner, frame.id);
          call.reader = reader;
          call.collected = (async () => {
            const chunks: Uint8Array[] = [];
            for (let chunk = await reader.read(); chunk !== null; chunk = await reader.read()) chunks.push(chunk);
            return concat(chunks);
          })();
          call.collected.catch(() => {});
          break;
        }
        case FRAME.data: {
          const reader = call?.reader;
          if (reader === undefined || reader === null) {
            if (frame.payload.length > 0) return this.broken(`DATA on ${frame.id}, which has no response stream`);
            continue;
          }
          if (!reader.data(frame.payload, (frame.flags & FLAG_END) !== 0)) return this.broken(`DATA past the credit granted on ${frame.id}`);
          break;
        }
        case FRAME.credit:
          if (call?.writer && !call.writer.grant(readU32(frame.payload, 0), MAX_CREDIT)) return this.broken(`CREDIT past ${MAX_CREDIT} on ${frame.id}`);
          break;
        case FRAME.reset: {
          const code = resetName(readU16(frame.payload, 0)) ?? "cancelled";
          const error = new ServiceError("transfer", `reset: ${code}`, { reset: code });
          call?.writer?.dead(error);
          call?.reader?.dead(error);
          break;
        }
        case FRAME.request:
          // The host asks nothing of a service in this version.
          this.connection.send(encodeJson(FRAME.reply, frame.id, { code: "invalid", message: "this service answers no request on its host connection" }, HOST_PAYLOAD, FLAG_ERROR));
          break;
        default:
          break;
      }
    }
  }

  /**
   * One request: its REQUEST (with BODY when `body` is given), the body under
   * the host's credit, the response stream after its head event, and the
   * REPLY. Aborting `signal` sends CANCEL; the call then settles with the
   * reply that follows (`cancelled`, or a write's own outcome).
   */
  call(t: string, args: Json, options: { body?: { bytes: number; source: Uint8Array | AsyncIterable<Uint8Array> }; signal?: AbortSignal } = {}): Promise<{ reply: Json; value: Uint8Array | null }> {
    if (this.isClosed) return Promise.reject(new ServiceError("closed", "the host connection closed"));
    if (options.signal?.aborted) return Promise.reject(new ServiceError("cancelled", "the call was aborted before it was sent"));
    if (this.nextId > ID_DEVICE_MAX) return Promise.reject(new ServiceError("busy", "this connection's request ids are spent"));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const call: Pending = { resolve, reject, reader: null, collected: null, writer: null, done: false };
      this.calls.set(id, call);
      try {
        this.connection.send(encodeJson(FRAME.request, id, { t, ...args }, HOST_PAYLOAD, options.body ? FLAG_BODY : 0));
      } catch (error) {
        this.calls.delete(id);
        return reject(new ServiceError("too_large", error instanceof Error ? error.message : String(error)));
      }
      const signal = options.signal;
      if (signal !== undefined) {
        const abort = () => {
          if (!call.done && !this.isClosed) this.connection.send(encodeCancel(id));
        };
        signal.addEventListener("abort", abort, { once: true });
        const forget = () => signal.removeEventListener("abort", abort);
        const settled = { resolve: call.resolve, reject: call.reject };
        call.resolve = (value) => {
          forget();
          settled.resolve(value);
        };
        call.reject = (error) => {
          forget();
          settled.reject(error);
        };
      }
      if (options.body !== undefined) {
        const writer = new StreamWriter(this.owner, id);
        call.writer = writer;
        void this.feed(writer, options.body);
      }
    });
  }

  /** Writes a body as the host grants; an iterable must give exactly `bytes`. */
  private async feed(writer: StreamWriter, body: { bytes: number; source: Uint8Array | AsyncIterable<Uint8Array> }): Promise<void> {
    try {
      if (body.source instanceof Uint8Array) {
        await writer.writeAll(body.source);
      } else {
        let count = 0;
        for await (const chunk of body.source) {
          if (writer.done) return;
          count += chunk.length;
          if (count > body.bytes) {
            writer.close("length");
            return;
          }
          await writer.writeAll(chunk);
        }
        if (count !== body.bytes) {
          writer.close("length");
          return;
        }
      }
      writer.end();
    } catch {
      // The stream ended under it: the request's reply says how.
    }
  }
}

const notRequired = () => new ServiceError("not_required", "the manifest's requires does not name store");

/** A key or prefix as the SDK must check it (§7): a string with a lone surrogate would encode to the bytes of another key. */
function checkKey(key: unknown, name: string): string {
  if (typeof key !== "string") throw new ServiceError("invalid", `${name} is not a string`);
  if (!key.isWellFormed()) throw new ServiceError("invalid", `${name} holds a lone surrogate`);
  return key;
}

/** `store` on the host connection, refused locally when the manifest does not require it (§3 of the capabilities: before a frame leaves). */
export function makeStore(link: HostLink | null): Store {
  const required = link !== null && link.requires.includes("store");
  const call = async (t: string, args: Json, options: Opt | undefined, body?: { bytes: number; source: Uint8Array | AsyncIterable<Uint8Array> }) => {
    if (!required || link === null) throw notRequired();
    return link.call(t, args, { signal: options?.signal, body });
  };
  const store: Store = {
    async get(key, options) {
      const { reply, value } = await call("store.get", { key: checkKey(key, "key") }, options);
      return reply.absent === true ? null : (value ?? new Uint8Array(0));
    },
    async getText(key, options) {
      const value = await store.get(key, options);
      return value === null ? null : utf8Text.decode(value);
    },
    async getJson<T = unknown>(key: string, options?: Opt): Promise<T | null> {
      const text = await store.getText(key, options);
      return text === null ? null : (JSON.parse(text) as T);
    },
    async set(key, value, options) {
      checkKey(key, "key");
      let source: Uint8Array | AsyncIterable<Uint8Array>;
      let bytes: number;
      if (typeof value === "string") {
        source = utf8.encode(value);
        bytes = source.length;
      } else if (value instanceof Uint8Array) {
        source = value;
        bytes = value.length;
      } else if (value !== null && typeof value === "object" && Symbol.asyncIterator in value) {
        if (!Number.isSafeInteger(options?.bytes) || (options?.bytes ?? -1) < 0) throw new ServiceError("invalid", "an iterable value needs its length as `bytes`");
        source = value;
        bytes = options!.bytes!;
      } else {
        throw new ServiceError("invalid", "a value is a Uint8Array, a string or an async iterable of Uint8Array");
      }
      await call("store.set", { key, bytes }, options, { bytes, source });
    },
    async setJson(key, value, options) {
      const text = JSON.stringify(value);
      if (text === undefined) throw new ServiceError("invalid", "the value has no JSON form");
      await store.set(key, text, { signal: options?.signal });
    },
    async delete(key, options) {
      const { reply } = await call("store.delete", { key: checkKey(key, "key") }, options);
      return reply.existed === true;
    },
    async list(options) {
      const args: Json = {};
      if (options?.prefix !== undefined) args.prefix = checkKey(options.prefix, "prefix");
      if (options?.after !== undefined) args.after = checkKey(options.after, "after");
      if (options?.limit !== undefined) args.limit = options.limit;
      const { reply } = await call("store.list", args, options);
      return { keys: Array.isArray(reply.keys) ? (reply.keys as string[]) : [], more: reply.more === true };
    },
    async usage(options) {
      const { reply } = await call("store.usage", {}, options);
      return { bytes: Number(reply.bytes ?? 0), keys: Number(reply.keys ?? 0), maxBytes: Number(reply.maxBytes ?? 0) };
    },
  };
  return Object.freeze(store);
}

/** The `Host` of §2. */
export function makeHost(id: string, log: (line: string) => void, link: HostLink | null): Host {
  return Object.freeze({ id, log, net, native, store: makeStore(link) });
}
