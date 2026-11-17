# @pocketjs/service

The computer side of a Pocket applet. An applet runs on a handheld; its
**service** is a Bun program that Pocket Hub starts when the applet opens its
channel and stops when it is idle. The two exchange requests with replies,
events, cancellation and credited byte streams; the messages are yours, Hub
routes them and reads none of their content. The service never learns whether
the device is on Wi-Fi, USB, UART or Bluetooth.

This package is the service side of [`docs/SERVICE-PROTOCOL.md`](../../docs/SERVICE-PROTOCOL.md).
It has no runtime dependencies and runs on Bun without a build step. Its names
mirror the PocketJS reference service API (`tools/pocket-link/v2/peer.ts`,
`hub.ts`, `daemon.ts`) with the reference's word *daemon* read as *service*
and its `LinkError` as `ServiceError`, so a service written for the reference
Hub ports by changing its imports, those names and adding the `serve` line.

## A minimal service

```ts
// service.ts
import { type ServiceFactory, ServiceError, serve } from "@pocketjs/service";

const service: ServiceFactory = ({ log, data }) => ({
  attach(channel, info) {
    log(`device ${info.deviceName} opened generation ${info.generation}`);
    channel.on("echo", (args) => ({ echo: args }));
    channel.on("file", (args) => {
      if (typeof args.name !== "string") throw new ServiceError("invalid", "name is a string");
      return { path: `${data}/${args.name}` };
    });
    channel.onEvent("note", (args) => void channel.emit("note.seen", { text: String(args.text) }));
    channel.onClose((reason) => log(`closed: ${reason}`));
  },
});

export default service;
if (import.meta.main) await serve(service);
```

```json
// pocket.service.json
{ "service": 1, "id": "com.example.my-applet", "entry": "service.ts" }
```

`id` is the applet's manifest id; a service serves that applet only. The
optional `idleSeconds` (default 30) is how long Hub keeps the process with no
channel open.

## Package layout

```
pocket.service.json    the manifest (SERVICE-PROTOCOL.md §1)
service.ts             the entry the manifest names; calls serve()
package.json, bun.lock optional; Hub runs `bun install --frozen-lockfile` when both are there
```

Hub runs `bun run <entry>` in the package folder with
`POCKET_SERVICE_SOCKET`, `_TOKEN`, `_ID` and `_DATA`. Standard output and
error are the service's log, line by line. `context.data` is a folder for the
service's own files, kept across upgrades.

Working examples, each a complete package: [`examples/sample`](examples/sample)
(the reference `daemon-sample.ts`) and [`examples/link-channel`](examples/link-channel)
(the computer side of `dev.pocket-nexus.link-channel`).

## API

```ts
serve(service: Service | ServiceFactory): Promise<never>
runService(service: Service | ServiceFactory, options: RunOptions): ServiceRun   // same, without env and process.exit

type ServiceFactory = (context: ServiceContext) => Service | Promise<Service>
interface ServiceContext { app: string; data: string; log(line: string): void }
interface Service {
  ready?: Promise<void>;                                              // hello waits for it
  attach(channel: AppletChannel, info: ServiceInfo): void | Promise<void>;
  stop?(): void | Promise<void>;                                      // service.stop, or the lease ended
}
interface ServiceInfo { device; deviceName; model; target; app; hash: string; generation: number }

class AppletChannel {
  readonly id, info, app, hash, generation, device, payloadLimit, messageLimit, units, stats
  on(t, handler: (args: Json, call: AppletCall) => Json | void | Promise<Json | void>): void
  request(t, args?, { body?: boolean, onProgress?: (t, args, stream?) => void }?): Call
  emit(t, args?): boolean
  emitStream(t, args?): StreamWriter | null
  onEvent(t, handler: (args: Json, stream?: StreamReader) => void | Promise<void>): void
  state(): { state: "open" | "closed"; reason: string | null }
  onClose(listener: (reason: string) => void): void
  close(reason = "service"): void
}
interface AppletCall { t; args; cancelled; onCancel(fn); progress(t, args?): boolean; respond(): StreamWriter; body: StreamReader | null }
interface Call { id; reply: Promise<Json>; cancel(): void; body: StreamWriter | null; response: StreamReader | null }
class StreamReader { id; received; finished: Promise<number>; heldBytes; done; read(max?): Promise<Uint8Array | null>; close(code?) }
class StreamWriter { id; sent; buffered; creditFree; done; finished: Promise<void>; write(bytes): boolean; writeAll(bytes): Promise<void>; end(); close(code?) }
class ServiceError extends Error { code: string; extra: Json }
```

A handler's return value is the reply (`{}` for none). Throwing a
`ServiceError(code, message)` sends that error; any other exception sends
`internal` and is logged. An applet request whose `t` has no handler is
answered `unknown_message`.

## Rules that surprise people

**The head step.** A response stream exists only after a progress event:
call `call.progress("x.head", {bytes})` first, then `call.respond()`.
`respond()` before any progress event throws, before a byte reaches the wire:
the applet is handed the stream at the first progress event and grants credit
only once it reads it, so bytes sent without a head would wait at credit 0
until the request times out. The reply goes out after the stream's END, as its
trailer.

```ts
channel.on("tile", async (args, call) => {
  const bytes = await load(args);
  while (!call.progress("tile.head", { bytes: bytes.length })) await Bun.sleep(5);
  const out = call.respond();
  await out.writeAll(bytes);
  return { rev: "r42" };          // sent after END
});
```

**`emit` and `progress` return false; nothing is queued.** Every event and
progress event costs one unit per started 512 bytes of JSON from a window the
applet grants (48 units on a device, regranted as the applet takes events). At
zero units `emit` returns false and sends nothing; nothing is dropped behind
your back and nothing piles up without bound. Keep the event and try again
later (`while (!channel.emit(...)) await Bun.sleep(5)`), or drop it yourself
if it is stale. A paused applet slows its service this way and loses nothing.
`emit` throws (`too_large`, `malformed`) only for an event that could never be
sent.

**Credit, both ways.** Stream bytes travel only within the receiver's grant.
The service grants a 256 KiB window on a stream it reads, once `read()` is
called and refilled as `read()` takes bytes; a stream you never read is never
granted anything, and its sender waits. Toward the applet, `write` queues
bytes and they leave as the applet grants credit (32 KiB per stream on a
device, granted while the applet reads); `writeAll` waits for room. A reply
sent while the response stream still waits for credit waits with it.

**Sizes.** A JSON message is at most `channel.messageLimit` bytes
(`min(8,192, payload limit − 4)`; 1,020 on the smallest link). Larger content
is a stream. A request over the limit fails with `too_large` without being
sent.

**Bounds.** At most 8 requests of the service open per channel (a ninth fails
at once with `busy`) and 4 streams toward the applet (a fifth `emitStream`
returns null, a body or `respond()` fails with `busy`). Toward the service the
same bounds hold: a ninth applet request is answered `busy`, a fifth inbound
stream is reset `refused`.

**Cancel.** When the applet cancels, Hub answers it at once; the handler sees
`call.cancelled` and its `onCancel` listeners, and anything it returns or
writes for that request is dropped. `call.cancel()` on the service's own
request sends CANCEL; its `reply` settles with the REPLY that follows
(`cancelled`, or the applet's reply if it crossed the cancel).

**Close.** When a channel closes (the applet exits, the device disconnects,
Hub stops the service, or `channel.close()`), every pending call ends with
`ServiceError("closed")`, handlers' `onCancel` listeners run, and `onClose` runs
once. `service.stop` calls `stop()`, closes every channel with reason `stop`,
and exits with 0; the process also exits when Hub's control connection
closes. Hub sends `SIGTERM` 5 s after `service.stop`.

**Errors stay in their request.** A throwing request handler fails that
request; a throwing event handler, `onProgress` or `onClose` is logged. Under
`serve`, an unhandled promise rejection is logged and the process keeps
running. An exception thrown from a timer callback of your own is not caught:
it ends the process, and every device's channel with it.

## Development

```
bun install            # dev dependencies only: typescript, @types/bun
bun test               # mock Hub, in process and as child processes
bunx tsc --noEmit -p . # strict type check of src, tests and examples
```

`test/mock-hub.ts` is a frame-level mock of Hub's service socket that plays the
applet and records every rule of SERVICE-PROTOCOL.md §5 a service's frames
break; use it to test your own service.
