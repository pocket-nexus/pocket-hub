// The daemon process's side of docs/DAEMON-PROTOCOL.md §2, §4 and §5: the
// environment Hub starts it with, the control connection (`daemon.hello` →
// `daemon.ready`, then `channel.offer` and `daemon.stop`), and one channel
// connection per offer (`channel.attach` → `channel.ready`).
//
// `serve(daemon)` is the entry point of a daemon package: it reads the four
// POCKET_HUB_DAEMON_* variables and exits the process when Hub stops it or
// the control connection closes. `runDaemon(daemon, options)` is the same
// without the environment and without `process.exit`, for tests and for
// hosting a daemon inside another Bun program.

import { AppletChannel, type DaemonInfo } from "./channel.ts";
import { Connection } from "./connection.ts";
import { asLinkError } from "./errors.ts";
import {
  CONTROL_PAYLOAD,
  FRAME,
  type Frame,
  MAX_HANDSHAKE,
  MIN_PAYLOAD,
  PAYLOAD_CEILING,
  appMessageLimit,
  encodeHandshake,
  parseJson,
} from "./frame.ts";

/** What a daemon module gets from `serve` (the reference's `DaemonContext`). */
export interface DaemonContext {
  /** The applet id this daemon serves. */
  app: string;
  /** One line to the daemon's log: Hub captures standard output and error line by line (§2). */
  log(line: string): void;
  /** The daemon's own folder, mode 0700, kept across upgrades: `<home>/daemon-data/<id>/` (§1). */
  data: string;
}

/** The computer side of one applet (the reference's `Daemon`). */
export interface Daemon {
  /** When present, `serve` says `daemon.hello` only once it resolves; Hub holds the device's `channel.open` meanwhile. */
  ready?: Promise<void>;
  /** One call per accepted channel: the daemon's end of it. Handlers registered before it returns (or before its promise resolves) see the channel's first frames. */
  attach(channel: AppletChannel, info: DaemonInfo): void | Promise<void>;
  /** Hub stops the daemon (`daemon.stop`), or the control connection closed. The channels close after it returns. */
  stop?(): void | Promise<void>;
}

/** A module's default export may be a daemon or a function that makes one. */
export type DaemonFactory = (context: DaemonContext) => Daemon | Promise<Daemon>;

export interface RunOptions {
  /** Path of Hub's daemon socket (POCKET_HUB_DAEMON_SOCKET). */
  socket: string;
  /** 32 lowercase hex digits (POCKET_HUB_DAEMON_TOKEN). */
  token: string;
  /** The applet id (POCKET_HUB_DAEMON_APP). */
  app: string;
  /** The daemon's data folder (POCKET_HUB_DAEMON_DATA). */
  data: string;
  /** The context's `log`; standard output by default. */
  log?: (line: string) => void;
  /** The SDK's own diagnostics; standard error by default. */
  diag?: (line: string) => void;
  /** How long `channel.attach` may wait for its answer. Default 10 s. */
  attachTimeoutMs?: number;
}

export interface DaemonRun {
  /** Resolves with the exit code once the daemon is done: 0 after `daemon.stop` or the control connection closing, 1 after a refusal or a failure to start. */
  readonly done: Promise<number>;
  /** The channels open now. */
  channels(): AppletChannel[];
}

/** Protocol version of `daemon.hello` (§4). */
export const DAEMON_PROTOCOL = 1;

const HEX32 = /^[0-9a-f]{32}$/;

const defaultLog = (line: string) => void process.stdout.write(`${line}\n`);
const defaultDiag = (line: string) => void process.stderr.write(`[pocket-hub daemon] ${line}\n`);

async function makeDaemon(daemon: Daemon | DaemonFactory, context: DaemonContext): Promise<Daemon> {
  const made = typeof daemon === "function" ? await daemon(context) : daemon;
  if (made === null || typeof made !== "object" || typeof made.attach !== "function") {
    throw new Error("no daemon: the default export is a daemon with attach(), or a function that returns one");
  }
  return made;
}

interface Offer extends DaemonInfo {
  ticket: string;
  payloadLimit: number;
}

function parseOffer(object: Record<string, unknown>): Offer | null {
  const { ticket, device, deviceName, model, target, app, hash, generation, payloadLimit } = object;
  if (typeof ticket !== "string" || !HEX32.test(ticket)) return null;
  if (typeof device !== "string" || typeof app !== "string" || typeof hash !== "string") return null;
  if (typeof generation !== "number" || !Number.isInteger(generation) || generation < 0) return null;
  if (typeof payloadLimit !== "number" || !Number.isInteger(payloadLimit) || payloadLimit < MIN_PAYLOAD || payloadLimit > PAYLOAD_CEILING) return null;
  return {
    ticket,
    device,
    deviceName: typeof deviceName === "string" ? deviceName : "",
    model: typeof model === "string" ? model : "",
    target: typeof target === "string" ? target : "",
    app,
    hash,
    generation,
    payloadLimit,
  };
}

/**
 * Runs a daemon against Hub's daemon socket: control connection, offers,
 * channel connections. Never throws; failures end `done` with 1.
 */
export function runDaemon(daemonOrFactory: Daemon | DaemonFactory, options: RunOptions): DaemonRun {
  const log = options.log ?? defaultLog;
  const diag = options.diag ?? defaultDiag;
  const channels = new Set<AppletChannel>();
  const pending = new Set<Connection>();
  let finish!: (code: number) => void;
  const done = new Promise<number>((resolve) => (finish = resolve));
  let finished = false;
  let stopping = false;
  let daemon: Daemon | null = null;
  let control: Connection | null = null;

  const end = (code: number) => {
    if (finished) return;
    finished = true;
    control?.destroy();
    for (const connection of pending) connection.destroy();
    finish(code);
  };

  /** `daemon.stop`, or the lease ended: the daemon's stop, then every channel closes and is flushed. */
  const shutdown = async (why: string) => {
    if (stopping || finished) return;
    stopping = true;
    diag(`stopping: ${why}`);
    try {
      await daemon?.stop?.();
    } catch (error) {
      diag(`stop() failed: ${asLinkError(error).message}`);
    }
    const flushed: Promise<void>[] = [];
    for (const channel of [...channels]) {
      channel.close("stop");
    }
    for (const connection of channelConnections) flushed.push(new Promise((resolve) => connection.onClosed(resolve)));
    for (const connection of pending) connection.destroy();
    // Hub ends the process 5 s after `daemon.stop` (§4): a socket that does not close in 2 s is not waited for.
    await Promise.race([Promise.all(flushed), new Promise((resolve) => setTimeout(resolve, 2000).unref?.())]);
    end(0);
  };
  const channelConnections = new Set<Connection>();

  const attach = async (offer: Offer) => {
    if (stopping || finished || daemon === null) return;
    let connection: Connection;
    try {
      connection = await Connection.open(options.socket, offer.payloadLimit);
    } catch (error) {
      diag(`channel ${offer.generation}: cannot reach the daemon socket: ${asLinkError(error).message}`);
      return;
    }
    pending.add(connection);
    const timer = setTimeout(() => {
      diag(`channel ${offer.generation}: no answer to channel.attach`);
      connection.destroy();
    }, options.attachTimeoutMs ?? 10_000);
    let channel: AppletChannel | null = null;
    connection.listen({
      frames: (frames: Frame[]) => {
        if (channel !== null) return channel.frames(frames);
        for (let i = 0; i < frames.length; i++) {
          const frame = frames[i]!;
          if (frame.type !== FRAME.handshake) {
            diag(`channel ${offer.generation}: frame 0x${frame.type.toString(16)} before channel.ready`);
            return connection.destroy();
          }
          const object = parseJson(frame.payload);
          if (object?.t === "channel.refused") {
            diag(`channel ${offer.generation} refused: ${String(object.message ?? "")}`);
            return connection.destroy();
          }
          if (object?.t !== "channel.ready") continue;
          const id = object.channel;
          if (typeof id !== "number" || !Number.isInteger(id) || id <= 0 || id > 0xffff_ffff || id === 0x8000_0000) {
            diag(`channel ${offer.generation}: channel.ready without a valid channel id`);
            return connection.destroy();
          }
          const limit = typeof object.messageLimit === "number" && Number.isInteger(object.messageLimit) && object.messageLimit > 0
            ? Math.min(object.messageLimit, appMessageLimit(offer.payloadLimit))
            : appMessageLimit(offer.payloadLimit);
          clearTimeout(timer);
          pending.delete(connection);
          // Hub stopped the daemon while this channel was being attached: it is not taken.
          if (stopping || finished) return connection.destroy();
          const { ticket: _ticket, payloadLimit: _limit, ...info } = offer;
          const opened = new AppletChannel(connection, id, info, offer.payloadLimit, limit, diag);
          channel = opened;
          channels.add(opened);
          channelConnections.add(connection);
          opened.onClose(() => channels.delete(opened));
          connection.onClosed(() => channelConnections.delete(connection));
          // The daemon's 256 event units go out right after `channel.ready` (§5), before any frame of the daemon's code.
          opened.start();
          // Frames behind `channel.ready` (in this read or later ones) wait until attach() has registered its handlers.
          opened.pause();
          opened.frames(frames.slice(i + 1));
          let attached: void | Promise<void>;
          try {
            attached = daemon!.attach(opened, { ...info });
          } catch (error) {
            diag(`attach() failed: ${asLinkError(error).message}`);
            opened.close("daemon");
            return;
          }
          const go = () => opened.release();
          if (attached instanceof Promise) {
            attached.then(go, (error: unknown) => {
              diag(`attach() failed: ${asLinkError(error).message}`);
              opened.close("daemon");
            });
          } else go();
          return;
        }
      },
      broken: (error) => {
        if (channel !== null) channel.broken(error.message);
        else connection.destroy();
      },
      closed: () => {
        clearTimeout(timer);
        pending.delete(connection);
        channel?.connectionClosed();
      },
    });
    connection.send(encodeHandshake({ t: "channel.attach", ticket: offer.ticket }));
  };

  const start = async () => {
    const context: DaemonContext = { app: options.app, data: options.data, log };
    try {
      daemon = await makeDaemon(daemonOrFactory, context);
      if (daemon.ready !== undefined) await daemon.ready;
    } catch (error) {
      diag(`the daemon failed to start: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
      return end(1);
    }
    try {
      control = await Connection.open(options.socket, CONTROL_PAYLOAD);
    } catch (error) {
      diag(`cannot reach Hub's daemon socket ${options.socket}: ${asLinkError(error).message}`);
      return end(1);
    }
    let ready = false;
    const offers: Offer[] = [];
    control.listen({
      frames: (frames) => {
        for (const frame of frames) {
          if (finished) return;
          const object = frame.type === FRAME.handshake || frame.type === FRAME.event ? parseJson(frame.payload) : null;
          if (object === null) continue;
          if (frame.type === FRAME.handshake) {
            if (object.t === "daemon.ready" && !ready) {
              ready = true;
              if (object.app !== options.app) diag(`daemon.ready names ${String(object.app)}, not ${options.app}`);
              if (typeof object.data === "string" && object.data !== options.data) diag(`daemon.ready names the data folder ${object.data}, not ${options.data}`);
              for (const offer of offers.splice(0)) void attach(offer);
            } else if (object.t === "daemon.refused") {
              diag(`Hub refused the daemon: ${String(object.message ?? "")}`);
              return end(1);
            }
            continue;
          }
          if (frame.id !== 0) continue;
          if (object.t === "channel.offer") {
            const offer = parseOffer(object);
            if (offer === null) {
              diag("a channel.offer without its fields");
              continue;
            }
            if (ready) void attach(offer);
            else offers.push(offer);
          } else if (object.t === "daemon.stop") {
            void shutdown("daemon.stop");
          }
        }
      },
      broken: (error) => {
        diag(`the control connection broke: ${error.message}`);
        control?.destroy();
      },
      closed: () => {
        if (finished) return;
        if (!ready) return end(1);
        // The control connection is the process's lease (§4): it closed, so the daemon ends.
        void shutdown("the control connection closed");
      },
    });
    let hello: Uint8Array;
    try {
      hello = encodeHandshake({ t: "daemon.hello", protocol: DAEMON_PROTOCOL, token: options.token, app: options.app });
    } catch (error) {
      diag(`daemon.hello does not fit ${MAX_HANDSHAKE} bytes: ${asLinkError(error).message}`);
      return end(1);
    }
    control.send(hello);
  };

  void start();
  return { done, channels: () => [...channels] };
}

/**
 * The entry point of a daemon package. Reads POCKET_HUB_DAEMON_SOCKET,
 * _TOKEN, _APP and _DATA, runs the daemon, and exits the process with 0
 * after `daemon.stop` or when the control connection closes, with 1 when Hub
 * refuses it or it fails to start. A rejected promise nobody handled is
 * logged and does not end the process: one handler's mistake must not close
 * every device's channel.
 */
export async function serve(daemonOrFactory: Daemon | DaemonFactory): Promise<never> {
  const env = process.env;
  const socket = env.POCKET_HUB_DAEMON_SOCKET;
  const token = env.POCKET_HUB_DAEMON_TOKEN;
  const app = env.POCKET_HUB_DAEMON_APP;
  const data = env.POCKET_HUB_DAEMON_DATA;
  if (!socket || !token || !app || !data) {
    defaultDiag("POCKET_HUB_DAEMON_SOCKET, _TOKEN, _APP and _DATA are not all set: this program is started by Pocket Hub");
    process.exit(1);
  }
  process.on("unhandledRejection", (reason: unknown) => {
    defaultDiag(`unhandled rejection: ${reason instanceof Error ? (reason.stack ?? reason.message) : String(reason)}`);
  });
  const run = runDaemon(daemonOrFactory, { socket, token, app, data });
  const code = await run.done;
  process.exit(code);
}
