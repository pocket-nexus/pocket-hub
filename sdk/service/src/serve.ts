// The service process's side of docs/SERVICE-PROTOCOL.md §2, §4 and §5: the
// environment Hub starts it with, the control connection (`service.hello` →
// `service.ready`, then `channel.offer` and `service.stop`), and one channel
// connection per offer (`channel.attach` → `channel.ready`).
//
// `serve(service)` is the entry point of a service package: it reads the four
// POCKET_SERVICE_* variables and exits the process when Hub stops it or
// the control connection closes. `runService(service, options)` is the same
// without the environment and without `process.exit`, for tests and for
// hosting a service inside another Bun program.

import { AppletChannel, type ServiceInfo } from "./channel.ts";
import { Connection } from "./connection.ts";
import { asServiceError } from "./errors.ts";
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

/** What a service module gets from `serve` (the reference's `DaemonContext`). */
export interface ServiceContext {
  /** The applet id this service serves. */
  app: string;
  /** One line to the service's log: Hub captures standard output and error line by line (§2). */
  log(line: string): void;
  /** The service's own folder, mode 0700, kept across upgrades: `<home>/service-data/<id>/` (§1). */
  data: string;
}

/** The computer side of one applet (the reference's `Daemon`). */
export interface Service {
  /** When present, `serve` says `service.hello` only once it resolves; Hub holds the device's `channel.open` meanwhile. */
  ready?: Promise<void>;
  /** One call per accepted channel: the service's end of it. Handlers registered before it returns (or before its promise resolves) see the channel's first frames. */
  attach(channel: AppletChannel, info: ServiceInfo): void | Promise<void>;
  /** Hub stops the service (`service.stop`), or the control connection closed. The channels close after it returns. */
  stop?(): void | Promise<void>;
}

/** A module's default export may be a service or a function that makes one. */
export type ServiceFactory = (context: ServiceContext) => Service | Promise<Service>;

export interface RunOptions {
  /** Path of Hub's service socket (POCKET_SERVICE_SOCKET). */
  socket: string;
  /** 32 lowercase hex digits (POCKET_SERVICE_TOKEN). */
  token: string;
  /** The applet id (POCKET_SERVICE_ID). */
  app: string;
  /** The service's data folder (POCKET_SERVICE_DATA). */
  data: string;
  /** The context's `log`; standard output by default. */
  log?: (line: string) => void;
  /** The SDK's own diagnostics; standard error by default. */
  diag?: (line: string) => void;
  /** How long `channel.attach` may wait for its answer. Default 10 s. */
  attachTimeoutMs?: number;
}

export interface ServiceRun {
  /** Resolves with the exit code once the service is done: 0 after `service.stop` or the control connection closing, 1 after a refusal or a failure to start. */
  readonly done: Promise<number>;
  /** The channels open now. */
  channels(): AppletChannel[];
}

/** Protocol version of `service.hello` (§4). */
export const SERVICE_PROTOCOL = 2;

const HEX32 = /^[0-9a-f]{32}$/;

const defaultLog = (line: string) => void process.stdout.write(`${line}\n`);
const defaultDiag = (line: string) => void process.stderr.write(`[pocket-hub service] ${line}\n`);

async function makeService(service: Service | ServiceFactory, context: ServiceContext): Promise<Service> {
  const made = typeof service === "function" ? await service(context) : service;
  if (made === null || typeof made !== "object" || typeof made.attach !== "function") {
    throw new Error("no service: the default export is a service with attach(), or a function that returns one");
  }
  return made;
}

interface Offer extends ServiceInfo {
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
 * Runs a service against Hub's service socket: control connection, offers,
 * channel connections. Never throws; failures end `done` with 1.
 */
export function runService(serviceOrFactory: Service | ServiceFactory, options: RunOptions): ServiceRun {
  const log = options.log ?? defaultLog;
  const diag = options.diag ?? defaultDiag;
  const channels = new Set<AppletChannel>();
  const pending = new Set<Connection>();
  let finish!: (code: number) => void;
  const done = new Promise<number>((resolve) => (finish = resolve));
  let finished = false;
  let stopping = false;
  let service: Service | null = null;
  let control: Connection | null = null;

  const end = (code: number) => {
    if (finished) return;
    finished = true;
    control?.destroy();
    for (const connection of pending) connection.destroy();
    finish(code);
  };

  /** `service.stop`, or the lease ended: the service's stop, then every channel closes and is flushed. */
  const shutdown = async (why: string) => {
    if (stopping || finished) return;
    stopping = true;
    diag(`stopping: ${why}`);
    try {
      await service?.stop?.();
    } catch (error) {
      diag(`stop() failed: ${asServiceError(error).message}`);
    }
    const flushed: Promise<void>[] = [];
    for (const channel of [...channels]) {
      channel.close("stop");
    }
    for (const connection of channelConnections) flushed.push(new Promise((resolve) => connection.onClosed(resolve)));
    for (const connection of pending) connection.destroy();
    // Hub ends the process 5 s after `service.stop` (§4): a socket that does not close in 2 s is not waited for.
    await Promise.race([Promise.all(flushed), new Promise((resolve) => setTimeout(resolve, 2000).unref?.())]);
    end(0);
  };
  const channelConnections = new Set<Connection>();

  const attach = async (offer: Offer) => {
    if (stopping || finished || service === null) return;
    let connection: Connection;
    try {
      connection = await Connection.open(options.socket, offer.payloadLimit);
    } catch (error) {
      diag(`channel ${offer.generation}: cannot reach the service socket: ${asServiceError(error).message}`);
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
          // Hub stopped the service while this channel was being attached: it is not taken.
          if (stopping || finished) return connection.destroy();
          const { ticket: _ticket, payloadLimit: _limit, ...info } = offer;
          const opened = new AppletChannel(connection, id, info, offer.payloadLimit, limit, diag);
          channel = opened;
          channels.add(opened);
          channelConnections.add(connection);
          opened.onClose(() => channels.delete(opened));
          connection.onClosed(() => channelConnections.delete(connection));
          // The service's 256 event units go out right after `channel.ready` (§5), before any frame of the service's code.
          opened.start();
          // Frames behind `channel.ready` (in this read or later ones) wait until attach() has registered its handlers.
          opened.pause();
          opened.frames(frames.slice(i + 1));
          let attached: void | Promise<void>;
          try {
            attached = service!.attach(opened, { ...info });
          } catch (error) {
            diag(`attach() failed: ${asServiceError(error).message}`);
            opened.close("service");
            return;
          }
          const go = () => opened.release();
          if (attached instanceof Promise) {
            attached.then(go, (error: unknown) => {
              diag(`attach() failed: ${asServiceError(error).message}`);
              opened.close("service");
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
    const context: ServiceContext = { app: options.app, data: options.data, log };
    try {
      service = await makeService(serviceOrFactory, context);
      if (service.ready !== undefined) await service.ready;
    } catch (error) {
      diag(`the service failed to start: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
      return end(1);
    }
    try {
      control = await Connection.open(options.socket, CONTROL_PAYLOAD);
    } catch (error) {
      diag(`cannot reach Hub's service socket ${options.socket}: ${asServiceError(error).message}`);
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
            if (object.t === "service.ready" && !ready) {
              ready = true;
              if (object.app !== options.app) diag(`service.ready names ${String(object.app)}, not ${options.app}`);
              if (typeof object.data === "string" && object.data !== options.data) diag(`service.ready names the data folder ${object.data}, not ${options.data}`);
              for (const offer of offers.splice(0)) void attach(offer);
            } else if (object.t === "service.refused") {
              diag(`Hub refused the service: ${String(object.message ?? "")}`);
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
          } else if (object.t === "service.stop") {
            void shutdown("service.stop");
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
        // The control connection is the process's lease (§4): it closed, so the service ends.
        void shutdown("the control connection closed");
      },
    });
    let hello: Uint8Array;
    try {
      hello = encodeHandshake({ t: "service.hello", protocol: SERVICE_PROTOCOL, token: options.token, app: options.app });
    } catch (error) {
      diag(`service.hello does not fit ${MAX_HANDSHAKE} bytes: ${asServiceError(error).message}`);
      return end(1);
    }
    control.send(hello);
  };

  void start();
  return { done, channels: () => [...channels] };
}

/**
 * The entry point of a service package. Reads POCKET_SERVICE_SOCKET,
 * _TOKEN, _ID and _DATA, runs the service, and exits the process with 0
 * after `service.stop` or when the control connection closes, with 1 when Hub
 * refuses it or it fails to start. A rejected promise nobody handled is
 * logged and does not end the process: one handler's mistake must not close
 * every device's channel.
 */
export async function serve(serviceOrFactory: Service | ServiceFactory): Promise<never> {
  const env = process.env;
  const socket = env.POCKET_SERVICE_SOCKET;
  const token = env.POCKET_SERVICE_TOKEN;
  const app = env.POCKET_SERVICE_ID;
  const data = env.POCKET_SERVICE_DATA;
  if (!socket || !token || !app || !data) {
    defaultDiag("POCKET_SERVICE_SOCKET, _TOKEN, _ID and _DATA are not all set: this program is started by Pocket Hub");
    process.exit(1);
  }
  process.on("unhandledRejection", (reason: unknown) => {
    defaultDiag(`unhandled rejection: ${reason instanceof Error ? (reason.stack ?? reason.message) : String(reason)}`);
  });
  const run = runService(serviceOrFactory, { socket, token, app, data });
  const code = await run.done;
  process.exit(code);
}
