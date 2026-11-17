// One Unix-socket connection to Hub's service socket (docs/SERVICE-PROTOCOL.md
// §3): frames in, frames out. Frames leave in the order they are queued
// (link specification §5.7, §8.1); a frame's `written` callback runs once
// every byte of it is in the socket, which is what credit and the per-stream
// "one DATA frame queued at a time" rule key on.

import type { Socket } from "bun";
import { type Frame, FrameDecoder, FrameError } from "./frame.ts";

export interface ConnectionEvents {
  frames(frames: Frame[]): void;
  /** The decoder refused what Hub sent: the connection is closed after this. */
  broken(error: FrameError): void;
  /** The socket closed, by either side. Called once. */
  closed(): void;
}

interface Outgoing {
  bytes: Uint8Array;
  written?: () => void;
}

export class Connection {
  private socket: Socket<undefined> | null = null;
  private readonly queue: Outgoing[] = [];
  /** Bytes of the head of the queue already in the socket. */
  private offset = 0;
  private ending = false;
  private isClosed = false;
  private broken = false;
  private closedTold = false;
  private events: ConnectionEvents | null = null;
  private readonly early: Uint8Array[] = [];
  private closedListeners: (() => void)[] = [];
  readonly decoder: FrameDecoder;

  private constructor(limit: number) {
    this.decoder = new FrameDecoder(limit);
  }

  /** Opens a connection to the service socket at `path`. Frames read before `listen` are kept for it. */
  static async open(path: string, limit: number): Promise<Connection> {
    const connection = new Connection(limit);
    connection.socket = await Bun.connect<undefined>({
      unix: path,
      socket: {
        data: (_socket, chunk) => connection.received(chunk),
        drain: () => connection.flush(),
        close: () => connection.ended(),
        error: () => connection.ended(),
        end: () => connection.ended(),
      },
    });
    return connection;
  }

  get closed(): boolean {
    return this.isClosed;
  }

  listen(events: ConnectionEvents): void {
    this.events = events;
    for (const chunk of this.early.splice(0)) this.received(chunk);
    if (this.isClosed && !this.closedTold) {
      this.closedTold = true;
      events.closed();
    }
  }

  onClosed(listener: () => void): void {
    if (this.isClosed) listener();
    else this.closedListeners.push(listener);
  }

  /** Queues one encoded frame. Nothing is queued once the connection is ending. */
  send(bytes: Uint8Array, written?: () => void): void {
    if (this.isClosed || this.ending) return;
    this.queue.push({ bytes, written });
    if (this.queue.length === 1) this.flush();
  }

  /** Drops every frame not yet in the socket (a channel Hub has closed takes nothing more, §7.5). */
  discardQueued(): void {
    if (this.queue.length === 0) return;
    const head = this.offset > 0 ? this.queue[0] : undefined;
    this.queue.length = 0;
    // A frame half in the socket must be completed, or the next byte Hub reads is a broken header.
    if (head !== undefined) this.queue.push(head);
  }

  /** Closes the socket once every queued frame is written. */
  end(): void {
    if (this.isClosed) return;
    this.ending = true;
    if (this.queue.length === 0) this.socket?.end();
  }

  /** Closes the socket now. */
  destroy(): void {
    if (this.isClosed) return;
    this.socket?.terminate();
    this.ended();
  }

  private flush(): void {
    const socket = this.socket;
    if (socket === null || this.isClosed) return;
    while (this.queue.length > 0) {
      const head = this.queue[0]!;
      const rest = this.offset === 0 ? head.bytes : head.bytes.subarray(this.offset);
      const n = socket.write(rest);
      if (n < 0) return this.destroy();
      if (n < rest.length) {
        this.offset += n;
        return;
      }
      this.offset = 0;
      this.queue.shift();
      head.written?.();
    }
    if (this.ending) socket.end();
  }

  private received(chunk: Uint8Array): void {
    if (this.events === null) {
      this.early.push(chunk.slice());
      return;
    }
    // Bytes that arrived before the socket closed are still read: a `channel.close` may be among them.
    if (this.broken) return;
    let frames: Frame[];
    try {
      frames = this.decoder.push(chunk);
    } catch (error) {
      // Reading stops here; the owner decides how the connection ends (a channel says `channel.close` first).
      this.broken = true;
      this.events.broken(error instanceof FrameError ? error : new FrameError("protocol", String(error)));
      return;
    }
    if (frames.length > 0) this.events.frames(frames);
  }

  private ended(): void {
    if (this.isClosed) return;
    this.isClosed = true;
    this.queue.length = 0;
    if (this.events !== null) {
      this.closedTold = true;
      this.events.closed();
    }
    for (const listener of this.closedListeners.splice(0)) listener();
  }
}
