// The reference device of Pocket Link 2 (vendor/pocketjs/tools/pocket-link/v2/device.ts),
// driven by Hub's integration tests: one JSON config as argv[2], commands as
// JSON lines on stdin, events as JSON lines on stdout. Nothing here changes
// the reference; it only answers the runtime operations with fixed data and
// gives the applet channel a few handlers the tests call.

import { LinkDevice, type GuestChannel } from "../../../../vendor/pocketjs/tools/pocket-link/v2/device.ts";
import { type Call, LinkError, type StreamReader } from "../../../../vendor/pocketjs/tools/pocket-link/v2/peer.ts";
import { packageFooter } from "../../../../vendor/pocketjs/tools/pocket-link/v2/offer.ts";
import { POCKET_SECTION, decodeIdentity, decodePocketPackage, findSection } from "../../../../vendor/pocketjs/contracts/spec/pocket-package.ts";

type Json = Record<string, unknown>;

const config = JSON.parse(process.argv[2]) as {
  dir: string;
  hub: string;
  address: string;
  port: number;
  name?: string;
  model?: string;
  target?: string;
  hostAbi?: number;
  features?: string[];
  maxPayload?: number;
  autoConnect?: boolean;
};

const out = (event: Json) => process.stdout.write(`${JSON.stringify(event)}\n`);

/** The byte pattern both sides generate: byte i of a stream. */
const pattern = (length: number): Uint8Array => {
  const bytes = new Uint8Array(length);
  for (let i = 0; i < length; i++) bytes[i] = (i * 7 + 13) & 0xff;
  return bytes;
};
/** FNV-1a 32 over a stream, carried across chunks. */
const fnv = (hash: number, bytes: Uint8Array): number => {
  let h = hash;
  for (let i = 0; i < bytes.length; i++) {
    h ^= bytes[i];
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h >>> 0;
};
const FNV_START = 0x811c9dc5;

const readAll = async (stream: StreamReader): Promise<{ bytes: number; sum: number }> => {
  let bytes = 0;
  let sum = FNV_START;
  for (;;) {
    const chunk = await stream.read();
    if (chunk === null) return { bytes, sum };
    bytes += chunk.length;
    sum = fnv(sum, chunk);
  }
};

let generation = 1;
/** The installed set the device reports (§6.2 runtime.apps), by app id. */
const installed = new Map<string, { id: string; output: string; version: string; hash: string }>();

/** A real package's identity for this device's target and its manifest's
 *  version, or null for the byte patterns the other tests push. */
const identityOf = (body: Uint8Array): { id: string; output: string; title: string; version: string } | null => {
  try {
    const pkg = decodePocketPackage(body, { skipHashCheck: true });
    const variant = pkg.variants.find((v) => v.target === (config.target ?? "3ds-dev"));
    const section = variant === undefined ? null : findSection(variant, POCKET_SECTION.identity);
    if (section === null) return null;
    const manifest = JSON.parse(new TextDecoder().decode(pkg.manifest)) as { version?: unknown };
    return { ...decodeIdentity(section), version: typeof manifest.version === "string" ? manifest.version : "" };
  } catch {
    return null;
  }
};
const surfaces = [
  { name: "top", width: 400, height: 240 },
  { name: "bottom", width: 320, height: 240 },
];

const device = new LinkDevice({
  dir: config.dir,
  name: config.name ?? "Test 3DS",
  model: config.model ?? "New Nintendo 3DS XL",
  target: config.target ?? "3ds-dev",
  hostAbi: config.hostAbi ?? 7,
  features: config.features,
  maxPayload: config.maxPayload,
  beaconPort: null,
  runtime: {
    status: () => ({
      phase: "booted",
      target: config.target ?? "3ds-dev",
      hostAbi: config.hostAbi ?? 7,
      slot: "0000000000000001",
      address: "127.0.0.1",
      generation,
      active: "00000000000000aa",
      lastGood: "00000000000000aa",
      running: "00000000000000aa",
      frame: 1234,
      app: { id: "dev.demo", output: "demo", title: "Demo" },
    }) as never,
    push: async (request, body) => {
      if (body[0] === 0xee) throw new LinkError("rejected", "the package is not admitted", { phase: "rejected" });
      out({ ev: "pushed", bytes: body.length, hash: request.hash, sum: fnv(FNV_START, body) });
      generation += 1;
      // A real package is committed in its app's slot and reported (§6.2).
      const app = identityOf(body);
      if (app !== null) {
        installed.set(app.id, { id: app.id, output: app.output, version: app.version, hash: request.hash });
        device.setApps([...installed.values()]);
        return { generation, message: "ok", app: { id: app.id, output: app.output, title: app.title } as never };
      }
      return { generation, message: "ok", app: { id: "dev.demo", output: "demo", title: "Demo" } as never };
    },
    nativeInstall: async (request, body) => {
      out({ ev: "native", name: request.name, launch: request.launch, bytes: body.length, sum: fnv(FNV_START, body) });
      return { phase: request.launch ? "launching" : "installed", path: `sdmc:/3ds/${request.name}` };
    },
    launch: async (request) => {
      out({ ev: "launch", ...request });
      if (request.native !== undefined) return { phase: "launching", name: request.native, path: `sdmc:/3ds/${request.native}` };
      if (request.app === "missing") throw new LinkError("not_found", "no such output");
      return { phase: "switching", output: request.app };
    },
    // The reference has refused the launcher's row, the running applet
    // (`status().app`, "dev.demo") and an id it does not hold; it drops the
    // row and reports the set after this (§6.2 runtime.remove).
    remove: async ({ id }) => {
      out({ ev: "removed", id });
      installed.delete(id);
    },
    screenshot: () => ({
      frame: 99,
      surfaces: surfaces.map((surface, index) => {
        const bytes = surface.width * surface.height * 3;
        const pixels = new Uint8Array(bytes);
        for (let j = 0; j < bytes; j++) pixels[j] = (j + index * 3) & 0xff;
        return { ...surface, bytes, pixels };
      }),
    }),
  },
  onLinked: (hub) => out({ ev: "linked", hub: hub.id, name: hub.name, clock: device.hub?.clock ?? null, limit: device.peer?.limit ?? null }),
  onUnlinked: (info) => out({ ev: "unlinked", ...info }),
});

device.onCatalogChanged = () => out({ ev: "catalog.changed" });

let guest: GuestChannel | null = null;
const calls = new Map<string, Call>();

const wireGuest = (channel: GuestChannel) => {
  channel.onOpen = (g) => out({ ev: "channel-open", generation: g });
  channel.on("echo", (args) => ({ echo: args }));
  channel.on("upload", async (_args, call) => {
    if (call.body === null) return { bytes: -1 };
    return await readAll(call.body);
  });
  channel.on("download", async (args, call) => {
    call.progress("head", { bytes: args.bytes });
    const stream = call.respond();
    await stream.writeAll(pattern(Number(args.bytes)));
    return { bytes: args.bytes };
  });
  channel.on("hang", (_args, call) => {
    call.onCancel(() => out({ ev: "applet-cancelled" }));
    return new Promise<Json>(() => {});
  });
  channel.on("named", (_args, call) => {
    // Progress named like system events: the applet's own names (§5.3 step 1).
    call.progress("log", { text: "mine" });
    call.progress("features", {});
    return { named: true };
  });
  channel.onEvent("note", (args) => out({ ev: "ch-event", t: "note", args }));
  channel.onEvent("blob", (args, stream?: StreamReader) => {
    if (stream === undefined) return out({ ev: "ch-stream", t: "blob", args, bytes: -1 });
    void readAll(stream).then(
      (got) => out({ ev: "ch-stream", t: "blob", args, ...got }),
      (error: LinkError) => out({ ev: "ch-stream", t: "blob", args, error: error.code }),
    );
  });
  channel.onEvent("nostream", (args) => out({ ev: "ch-event", t: "nostream", args }));
  // The link-channel example daemon's own request and events (pocket-hubd's end-to-end test).
  channel.on("applet.sum", (args) => {
    out({ ev: "applet-request", t: "applet.sum", args });
    return { sum: Number(args.a ?? 0) + Number(args.b ?? 0) };
  });
  channel.onEvent("tick", (args) => out({ ev: "ch-event", t: "tick", args }));
};

const command = async (cmd: Json) => {
  const tag = cmd.tag as string | undefined;
  switch (cmd.cmd) {
    case "connect":
      device.connectTo(config.hub, config.address, config.port);
      return;
    case "stop":
      device.stop();
      return;
    case "log":
      out({ ev: "logged", sent: device.log((cmd.level as never) ?? "info", (cmd.source as never) ?? "host", String(cmd.text ?? ""), Number(cmd.frame ?? 0)) });
      return;
    case "status":
      device.statusChanged();
      return;
    case "apps": {
      const count = Number(cmd.count ?? 1);
      const big = cmd.big === true;
      const rows: { id: string; output: string; version: string; hash: string; home?: boolean }[] = Array.from({ length: count }, (_, i) => ({
        id: big ? `dev.app.${String(i).padStart(2, "0")}.${"x".repeat(128 - 11)}` : `dev.app.${i}`,
        output: big ? `o${String(i).padStart(2, "0")}${"y".repeat(61)}` : `app${i}`,
        version: big ? "1".repeat(32) : "1.0.0",
        hash: i.toString(16).padStart(16, "0"),
      }));
      // With `home`, the launcher's own row first, at that version: the
      // device's program as the real Pocket Shell reports it (§6.2 `home`).
      if (typeof cmd.home === "string") rows.unshift({ id: "dev.pocket-nexus.shell", output: "home", version: cmd.home, hash: "00000000000000aa", home: true });
      installed.clear();
      for (const row of rows) installed.set(row.id, row);
      device.setApps(rows);
      return;
    }
    case "shot":
      out({ ev: "shot-sent", sent: device.sendScreenshot() !== null });
      return;
    case "features":
      device.setFeatures(cmd.list as string[]);
      return;
    case "open":
      guest = device.openChannel(String(cmd.app), String(cmd.hash ?? "00000000000000aa"));
      wireGuest(guest);
      return;
    case "request": {
      if (guest === null) return out({ ev: "reply", tag, ok: false, code: "no_guest" });
      let bytes = 0;
      let sum = FNV_START;
      let reading: Promise<void> = Promise.resolve();
      const progress: string[] = [];
      const call = guest.request(String(cmd.t), (cmd.args as Json) ?? {}, {
        body: Number(cmd.body ?? 0) > 0 || cmd.emptyBody === true,
        onProgress: (t, args, stream) => {
          progress.push(t);
          out({ ev: "progress", tag, t, args, stream: stream !== undefined });
          if (stream !== undefined && cmd.read !== false && progress.length === 1) {
            reading = (async () => {
              for (;;) {
                const chunk = await stream.read();
                if (chunk === null) return;
                bytes += chunk.length;
                sum = fnv(sum, chunk);
              }
            })().catch(() => {});
          }
        },
      });
      calls.set(tag ?? "", call);
      if (call.body !== null) {
        const n = Number(cmd.body ?? 0);
        void (n > 0 ? call.body.writeAll(pattern(n)) : Promise.resolve()).then(() => call.body?.end(), () => {});
      }
      call.reply.then(
        async (value) => {
          await reading;
          out({ ev: "reply", tag, ok: true, value, bytes, sum, progress });
        },
        async (error: LinkError) => {
          await reading;
          out({ ev: "reply", tag, ok: false, code: error.code, message: error.message, bytes, progress });
        },
      );
      return;
    }
    case "cancel":
      calls.get(String(cmd.tag))?.cancel();
      return;
    case "emit":
      out({ ev: "emitted", tag, ok: guest?.emit(String(cmd.t), (cmd.args as Json) ?? {}) ?? false });
      return;
    case "emitStream": {
      const writer = guest?.emitStream(String(cmd.t), (cmd.args as Json) ?? {}) ?? null;
      out({ ev: "emitted", tag, ok: writer !== null });
      if (writer === null) return;
      const n = Number(cmd.bytes ?? 0);
      // With `chunk`, the bytes go in pieces of at most that size. The
      // reference peer's `writeAll` waits on its writer's own drains when
      // the stream-opening event's bytes still count toward the channel's
      // outbound bound (64 KiB): a first piece that does not fit beside them
      // waits for a wake-up the event's write never gives. Pieces of 32 KiB
      // always fit beside the event.
      const chunk = Number(cmd.chunk ?? 0);
      const bytes = pattern(n);
      const sending =
        chunk > 0
          ? (async () => {
              for (let at = 0; at < n; at += chunk) await writer.writeAll(bytes.subarray(at, Math.min(n, at + chunk)));
            })()
          : writer.writeAll(bytes);
      void sending.then(
        () => {
          writer.end();
          return writer.finished.then(() => out({ ev: "stream-sent", tag }));
        },
        () => {},
      ).catch((error: LinkError) => out({ ev: "stream-failed", tag, code: error.code }));
      return;
    }
    case "exit":
      guest?.exit();
      return;
    case "reset":
      guest?.reset();
      return;
    case "state":
      out({
        ev: "state",
        tag,
        state: guest?.state() ?? null,
        generations: guest?.generations ?? [],
        units: guest?.channel?.units ?? null,
        phase: device.status.phase,
        inUse: device.peer ? ["runtime", "apps", "channel", "catalog"].filter((f) => device.peer?.inUse(f)) : [],
      });
      return;
    case "catalog":
      device.catalog().then(
        (rows) => out({ ev: "catalog", tag, ok: true, rows }),
        (error: LinkError) => out({ ev: "catalog", tag, ok: false, code: error.code }),
      );
      return;
    case "page": {
      // One catalog request as written, outside the reference's fetch (§6.4):
      // a cursor naming no row, a cursor without its revision.
      const peer = device.peer;
      if (peer === null) return out({ ev: "page", tag, ok: false, code: "unlinked" });
      const object: Json & { t: string } = { t: "catalog" };
      if (cmd.after !== undefined) object.after = cmd.after;
      if (cmd.rev !== undefined) object.rev = cmd.rev;
      peer.request(object, { feature: "catalog" }).reply.then(
        (value) => out({ ev: "page", tag, ok: true, value }),
        (error: LinkError) => out({ ev: "page", tag, ok: false, code: error.code, message: error.message }),
      );
      return;
    }
    case "install": {
      // The reference device's install (§6.4): install.head, the stream under
      // its 64 KiB window, the reply. The footer it reports is computed from
      // the bytes received.
      const started = Date.now();
      device.install(String(cmd.app)).then(
        ({ hash, file }) => {
          const footer = packageFooter(file);
          out({ ev: "install", tag, ok: true, hash, bytes: file.length, stored: footer?.stored ?? null, computed: footer?.computed ?? null, ms: Date.now() - started });
        },
        (error: LinkError) => out({ ev: "install", tag, ok: false, code: error.code, message: error.message, ms: Date.now() - started }),
      );
      return;
    }
    case "http": {
      // A worker's net.http as the reference device's `http()` sends it
      // (device.ts: the body stream, the 8 KiB window granted with the
      // request), through `peer.request` so a test can also cancel it, slow
      // its reader, misdeclare its body, or send it where `http()` would
      // refuse locally (a payload limit under 12,288).
      const peer = device.peer;
      if (peer === null) return out({ ev: "http", tag, ok: false, code: "unlinked" });
      const body = typeof cmd.body === "number" ? pattern(cmd.body) : typeof cmd.body === "string" ? new TextEncoder().encode(cmd.body) : new Uint8Array(0);
      const object: Json & { t: string } = { t: "net.http", app: (cmd.app as string) ?? "dev.demo", url: cmd.url as string, method: (cmd.method as string) ?? "GET", headers: (cmd.headers as Json) ?? {}, bytes: Number(cmd.declared ?? body.length) };
      if (cmd.timeoutMs !== undefined) object.timeoutMs = cmd.timeoutMs as number;
      if (cmd.maxBytes !== undefined) object.maxBytes = cmd.maxBytes as number;
      const slow = Number(cmd.slowMs ?? 0);
      let head: Json | null = null;
      let bytes = 0;
      let sum = FNV_START;
      const kept: Uint8Array[] = [];
      let keptBytes = 0;
      const started = Date.now();
      let call: Call | null = null;
      call = peer.request(object, {
        feature: "net.http",
        body: true,
        response: {
          window: Number(cmd.window ?? 8192),
          onData: (chunk) => {
            bytes += chunk.length;
            sum = fnv(sum, chunk);
            if (keptBytes < 65536) {
              kept.push(chunk.slice(0, 65536 - keptBytes));
              keptBytes += Math.min(chunk.length, 65536 - keptBytes);
            }
            if (cmd.progressEvery !== undefined && Math.floor((bytes - chunk.length) / Number(cmd.progressEvery)) !== Math.floor(bytes / Number(cmd.progressEvery))) out({ ev: "http-progress", tag, bytes });
            if (slow > 0) return new Promise<void>((resolve) => setTimeout(resolve, slow));
          },
        },
        grantNow: true,
        onProgress: (t, args) => {
          if (t !== "net.head") return;
          head = args;
          out({ ev: "http-head", tag, status: args.status });
          if (cmd.cancelOnHead === true) {
            call?.cancel();
            out({ ev: "http-cancelled", tag });
          }
        },
      });
      calls.set(tag ?? "", call);
      if (cmd.cancelAfterMs !== undefined) setTimeout(() => call?.cancel(), Number(cmd.cancelAfterMs));
      const sent = Number(cmd.send ?? body.length);
      if (sent > 0) void call.body?.writeAll(body.subarray(0, sent)).then(() => call?.body?.end(), () => {});
      else call.body?.end();
      const text = () => {
        const all = new Uint8Array(keptBytes);
        let at = 0;
        for (const chunk of kept) {
          all.set(chunk, at);
          at += chunk.length;
        }
        return new TextDecoder().decode(all);
      };
      call.reply.then(
        (value) => out({ ev: "http", tag, ok: true, value, head, bytes, sum, text: text(), ms: Date.now() - started }),
        (error: LinkError) => out({ ev: "http", tag, ok: false, code: error.code, message: error.message, head, bytes, sum, text: text(), ms: Date.now() - started }),
      );
      return;
    }
    case "quit":
      device.stop();
      setTimeout(() => process.exit(0), 50);
      return;
  }
};

let pending = "";
process.stdin.on("data", (chunk: Buffer) => {
  pending += chunk.toString("utf8");
  let at = pending.indexOf("\n");
  while (at >= 0) {
    const line = pending.slice(0, at).trim();
    pending = pending.slice(at + 1);
    if (line.length > 0) void command(JSON.parse(line) as Json).catch((error) => out({ ev: "error", message: String(error) }));
    at = pending.indexOf("\n");
  }
});
process.stdin.on("end", () => {
  device.stop();
  setTimeout(() => process.exit(0), 50);
});

device.start(20);
out({ ev: "ready", device: device.id });
if (config.autoConnect !== false) device.connectTo(config.hub, config.address, config.port);
