// Byte streams of an applet channel at the daemon's end (link specification
// §5.6, §7.2, §7.3; docs/DAEMON-PROTOCOL.md §5 "Credit is the daemon's and
// the applet's, passed through"). Every direction starts at credit 0 and is
// granted by its receiver; Hub adds no credit of its own, so the daemon
// grants what it reads and sends only what the applet granted.
//
// The handles keep the names and behaviour of the reference
// (vendor/pocketjs/tools/pocket-link/v2/peer.ts `StreamReader`,
// `StreamWriter`): `read()` resolves to a chunk or null at the end;
// `write`/`writeAll`/`end`/`finished` on the sending side; `close(code)`
// abandons a stream with RESET on either side.

import { LinkError } from "./errors.ts";
import { type ResetName, STREAM_STALL_MS, STREAM_WINDOW, encodeData } from "./frame.ts";

/** What a stream handle needs of its channel. @internal */
export interface StreamOwner {
  readonly payloadLimit: number;
  /** Queues an encoded frame; `written` runs when its bytes are in the socket. */
  send(bytes: Uint8Array, written?: () => void): void;
  sendCredit(id: number, amount: number): void;
  sendReset(id: number, code: ResetName): void;
  /** The reader is over (END handed out, or dead): it no longer counts toward the 4 inbound streams. */
  readerDone(reader: StreamReader): void;
  /** The writer's END is queued, or it is dead: it no longer counts toward the 4 outbound streams. */
  writerDone(writer: StreamWriter): void;
  /** Bytes queued toward the socket, events and stream bytes of the channel together. */
  outboundBytes: number;
  readonly outboundBound: number;
  /** Bytes left the queue: wakes writers that wait for room. */
  released(bytes: number): void;
  waitRoom(): Promise<void>;
}

/**
 * The receiving end of one stream direction: the body of the applet's
 * request, the response to one of the daemon's requests (from its head
 * event on), or a stream an applet's event opened.
 *
 * **Credit follows consumption.** The window (256 KiB, the size the link
 * specification recommends for Hub's side, §5.6) is granted once a `read()`
 * is made and refilled as `read()` takes bytes: a CREDIT goes out when a
 * quarter of the window is free, or when nothing is in flight and nothing is
 * held. A stream nobody reads is granted nothing, so its sender waits on its
 * own side, as the device's host does for its guest (§7.3).
 */
export class StreamReader {
  private readonly held: Uint8Array[] = [];
  private heldCount = 0;
  private outstanding = 0;
  private ended = false;
  private settled = false;
  private failure: LinkError | null = null;
  private pending: { max: number; resolve: (bytes: Uint8Array | null) => void; reject: (error: LinkError) => void } | null = null;
  private finish!: (bytes: number) => void;
  private abort!: (error: LinkError) => void;
  /** Resolves with the byte count at END, once the bytes are read; rejects when the stream is reset or its channel closes. */
  readonly finished: Promise<number>;
  /** Bytes received so far. */
  received = 0;
  /** @internal `read()` calls so far: the reply of a request waits behind bytes being read. */
  reads = 0;
  /** @internal Runs when the held bytes reach 0 or the stream is over. */
  onDrained: (() => void) | null = null;

  /** @internal */
  constructor(
    private readonly owner: StreamOwner,
    readonly id: number,
    private readonly window: number = STREAM_WINDOW,
  ) {
    this.finished = new Promise<number>((resolve, reject) => {
      this.finish = resolve;
      this.abort = reject;
    });
    this.finished.catch(() => {});
  }

  /** Bytes received and not yet read. */
  get heldBytes(): number {
    return this.heldCount;
  }
  /** True once the stream ended, was reset, or its channel closed. */
  get done(): boolean {
    return this.settled;
  }

  /**
   * Up to `max` bytes (at most the window), or null at the end. One read is
   * pending per stream at a time; it completes as soon as any bytes are
   * there, with fewer than `max` when fewer came. Rejects with a LinkError
   * (`transfer` for a RESET, `closed` when the channel closed).
   */
  read(max?: number): Promise<Uint8Array | null> {
    if (this.pending !== null) return Promise.reject(new LinkError("internal", "one read is pending per stream"));
    if (this.failure !== null) return Promise.reject(this.failure);
    if (this.ended && this.heldCount === 0) return Promise.resolve(null);
    this.reads += 1;
    const size = Math.max(1, Math.min(max ?? this.window, this.window));
    if (this.heldCount > 0) {
      const out = this.take(size);
      this.grant();
      // After the caller's `await` continuation is queued: a reply held behind these bytes settles after them.
      queueMicrotask(() => this.drained());
      return Promise.resolve(out);
    }
    return new Promise((resolve, reject) => {
      this.pending = { max: size, resolve, reject };
      this.grant();
    });
  }

  /**
   * Abandons the stream: RESET `cancelled` (or the code given) toward the
   * applet, unless its END already came. Bytes held are dropped.
   */
  close(code: ResetName = "cancelled"): void {
    if (this.settled) return;
    if (!this.ended) this.owner.sendReset(this.id, code);
    this.dead(new LinkError("transfer", `reset: ${code}`, { reset: code }));
  }

  /** @internal A DATA frame of this stream. False when its bytes pass the credit granted. */
  data(bytes: Uint8Array, end: boolean): boolean {
    if (this.settled || this.ended) return true;
    if (bytes.length > this.outstanding) return false;
    this.outstanding -= bytes.length;
    this.received += bytes.length;
    if (bytes.length > 0) {
      this.held.push(bytes);
      this.heldCount += bytes.length;
    }
    if (end) this.ended = true;
    if (this.pending !== null && this.heldCount > 0) {
      const { max, resolve } = this.pending;
      this.pending = null;
      resolve(this.take(max));
    }
    if (this.ended && this.heldCount === 0) this.settleEnd();
    else this.drained();
    return true;
  }

  /** @internal The stream is over without END: a RESET, a cancel, or its channel closed. */
  dead(error: LinkError): void {
    if (this.settled) return;
    this.settled = true;
    this.failure = error;
    this.held.length = 0;
    this.heldCount = 0;
    this.owner.readerDone(this);
    this.abort(error);
    if (this.pending !== null) {
      const { reject } = this.pending;
      this.pending = null;
      reject(error);
    }
    this.drained();
  }

  private take(max: number): Uint8Array {
    const n = Math.min(max, this.heldCount);
    const first = this.held[0]!;
    let out: Uint8Array;
    if (first.length === n) {
      out = first;
      this.held.shift();
    } else {
      out = new Uint8Array(n);
      let filled = 0;
      while (filled < n) {
        const head = this.held[0]!;
        const piece = Math.min(head.length, n - filled);
        out.set(head.subarray(0, piece), filled);
        filled += piece;
        if (piece === head.length) this.held.shift();
        else this.held[0] = head.subarray(piece);
      }
    }
    this.heldCount -= n;
    if (this.ended && this.heldCount === 0) queueMicrotask(() => this.settleEnd());
    return out;
  }

  private grant(): void {
    if (this.ended || this.settled) return;
    const due = this.window - this.outstanding - this.heldCount;
    if (due <= 0) return;
    if (due < this.window / 4 && (this.outstanding > 0 || this.heldCount > 0)) return;
    this.outstanding += due;
    this.owner.sendCredit(this.id, due);
  }

  private drained(): void {
    if (this.heldCount === 0 || this.settled) this.onDrained?.();
  }

  private settleEnd(): void {
    if (this.settled) return;
    this.settled = true;
    this.owner.readerDone(this);
    this.finish(this.received);
    if (this.pending !== null) {
      const { resolve } = this.pending;
      this.pending = null;
      resolve(null);
    }
    this.drained();
  }
}

/**
 * The sending end of one stream direction: a response (after the head
 * event), the body of the daemon's request, or a stream the daemon's event
 * opened.
 *
 * Bytes wait here until the applet grants credit; **no DATA frame passes the
 * credit granted, and none is longer than the channel's payload limit.** At
 * most one DATA frame of a stream is in the write queue at a time, so a
 * stream's chunks do not hold the queue (§8.1). `write` refuses ("full",
 * false) once the channel's queued bytes reach 256 KiB; `writeAll` waits for
 * room.
 */
export class StreamWriter {
  private readonly chunks: Uint8Array[] = [];
  private pendingBytes = 0;
  private credit = 0;
  private ending = false;
  private queued = false;
  private settled = false;
  private counted = true;
  private stall: ReturnType<typeof setTimeout> | null = null;
  private finish!: () => void;
  private abort!: (error: LinkError) => void;
  /** Resolves when the END frame is in the socket; rejects when the stream is reset or its channel closes. */
  readonly finished: Promise<void>;
  /** Bytes of this stream handed to the socket. */
  sent = 0;

  /** @internal */
  constructor(
    private readonly owner: StreamOwner,
    readonly id: number,
    private readonly options: { giveUp?: boolean } = {},
  ) {
    this.finished = new Promise<void>((resolve, reject) => {
      this.finish = resolve;
      this.abort = reject;
    });
    this.finished.catch(() => {});
  }

  /** Bytes written and not yet in a DATA frame. */
  get buffered(): number {
    return this.pendingBytes;
  }
  /** Credit granted and not yet claimed by written bytes. */
  get creditFree(): number {
    return Math.max(0, this.credit - this.pendingBytes);
  }
  /** True once END is in the socket, or the stream was reset or its channel closed. */
  get done(): boolean {
    return this.settled;
  }

  /**
   * Takes `bytes` (copied) to send as credit allows. False ("full") when the
   * channel's queue would pass its bound, or once the stream ended; nothing
   * is taken then.
   */
  write(bytes: Uint8Array): boolean {
    if (this.settled || this.ending) return false;
    if (bytes.length === 0) return true;
    const queued = this.owner.outboundBytes;
    if (queued > 0 && queued + bytes.length > this.owner.outboundBound) return false;
    this.chunks.push(bytes.slice());
    this.pendingBytes += bytes.length;
    this.owner.outboundBytes += bytes.length;
    this.pump();
    return true;
  }

  /** Writes every byte, waiting for room. Rejects with `transfer` when the stream ends under it. */
  async writeAll(bytes: Uint8Array): Promise<void> {
    const step = this.owner.outboundBound;
    for (let at = 0; at < bytes.length; ) {
      if (this.settled) throw new LinkError("transfer", "the stream ended");
      const piece = bytes.subarray(at, Math.min(bytes.length, at + step));
      if (this.write(piece)) at += piece.length;
      else if (this.ending) throw new LinkError("transfer", "the stream was ended");
      else await this.owner.waitRoom();
    }
  }

  /** No more bytes: the last DATA frame carries END. */
  end(): void {
    if (this.settled || this.ending) return;
    this.ending = true;
    this.pump();
  }

  /** Abandons the stream with RESET (§5.6); bytes not yet in a frame are dropped. */
  close(code: ResetName = "cancelled"): void {
    if (this.settled) return;
    this.owner.sendReset(this.id, code);
    this.dead(new LinkError("transfer", `reset: ${code}`, { reset: code }));
  }

  /** @internal A CREDIT frame for this stream. False when it raises the credit past 16 MiB. */
  grant(bytes: number, max: number): boolean {
    if (this.settled) return true;
    if (this.credit + bytes > max) return false;
    this.credit += bytes;
    this.clearStall();
    this.pump();
    return true;
  }

  /** @internal The stream is over without END: a RESET, a cancel, the request ended, or its channel closed. */
  dead(error: LinkError): void {
    if (this.settled) return;
    this.settled = true;
    this.clearStall();
    this.chunks.length = 0;
    const dropped = this.pendingBytes;
    this.pendingBytes = 0;
    this.owner.outboundBytes -= dropped;
    this.uncount();
    this.abort(error);
    this.owner.released(dropped);
  }

  private uncount(): void {
    if (!this.counted) return;
    this.counted = false;
    this.owner.writerDone(this);
  }

  private pump(): void {
    if (this.queued || this.settled) return;
    if (this.pendingBytes === 0) {
      if (!this.ending) return;
      this.queued = true;
      this.uncount();
      this.owner.send(encodeData(this.id, new Uint8Array(0), true, this.owner.payloadLimit), () => this.complete());
      return;
    }
    const n = Math.min(this.pendingBytes, this.credit, this.owner.payloadLimit);
    if (n === 0) {
      this.armStall();
      return;
    }
    const out = new Uint8Array(n);
    let filled = 0;
    while (filled < n) {
      const head = this.chunks[0]!;
      const piece = Math.min(head.length, n - filled);
      out.set(head.subarray(0, piece), filled);
      filled += piece;
      if (piece === head.length) this.chunks.shift();
      else this.chunks[0] = head.subarray(piece);
    }
    this.pendingBytes -= n;
    this.credit -= n;
    const last = this.ending && this.pendingBytes === 0;
    if (last) this.uncount();
    this.queued = true;
    this.owner.send(encodeData(this.id, out, last, this.owner.payloadLimit), () => {
      this.sent += n;
      this.owner.outboundBytes -= n;
      this.owner.released(n);
      if (last) return this.complete();
      this.queued = false;
      this.pump();
    });
  }

  private complete(): void {
    if (this.settled) return;
    this.settled = true;
    this.clearStall();
    this.finish();
  }

  // §5.5: the sender of an event's stream gives up after 30 s without a CREDIT while it holds bytes.
  private armStall(): void {
    if (!this.options.giveUp || this.stall !== null) return;
    this.stall = setTimeout(() => {
      this.stall = null;
      if (this.pendingBytes > 0 && this.credit === 0) this.close("cancelled");
    }, STREAM_STALL_MS);
    (this.stall as { unref?: () => void }).unref?.();
  }
  private clearStall(): void {
    if (this.stall === null) return;
    clearTimeout(this.stall);
    this.stall = null;
  }
}
