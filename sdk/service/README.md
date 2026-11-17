# @pocketjs/service

The computer side of a Pocket applet. An applet runs on a handheld; its
**service** is a module Pocket Hub loads into a Bun process of its own when
the applet opens its channel, and stops when it is idle. The two exchange
requests with replies, events, cancellation and credited byte streams; the
messages are yours, Hub routes them and reads none of their content. The
service never learns whether the device is on Wi-Fi, USB, UART or Bluetooth.
It asks its host for what it needs: this host gives it a **store**.

This package is the service side of [`docs/SERVICE-PROTOCOL.md`](../../docs/SERVICE-PROTOCOL.md).
It has no runtime dependencies and runs on Bun without a build step. Hub links
its own copy into every package it starts: a service declares no dependency on
it. The channel's names mirror the PocketJS reference service API
(`tools/pocket-link/v2/peer.ts`) with its `LinkError` as `ServiceError`.

**A service runs with its owner's rights**: it is a program of the owner's
account, and what the SDK gives it does not limit what its own code can do.

## A minimal service

```ts
// service.ts — the manifest's entry. Its default export is the service.
import { type ServiceFactory, ServiceError } from "@pocketjs/service";

const service: ServiceFactory = (host) => ({
  attach(channel, info) {
    host.log(`device ${info.deviceName} opened generation ${info.generation}`);
    channel.on("echo", (args) => ({ echo: args }));
    channel.on("remember", async (args) => {
      if (typeof args.text !== "string") throw new ServiceError("invalid", "text is a string");
      await host.store.set("note", args.text);
      return {};
    });
    channel.on("recall", async () => ({ text: await host.store.getText("note") }));
    channel.onClose((reason) => host.log(`closed: ${reason}`));
  },
});

export default service;
```

```json
// pocket.service.json
{ "service": 1, "id": "com.example.my-applet", "entry": "service.ts", "requires": ["store"] }
```

`id` is the applet's manifest id; a service serves that applet only.
`requires` names the capabilities the service uses (this host provides
`store`); `store.maxBytes` sizes the store (default 16 MiB); `idleSeconds`
(default 30) is how long Hub keeps the process with no channel open.

## Package layout

```
pocket.service.json    the manifest (SERVICE-PROTOCOL.md §1)
service.ts             the entry the manifest names: its default export is a ServiceFactory
package.json, bun.lock optional; Hub runs `bun install --frozen-lockfile --ignore-scripts` when both are there
```

Hub runs its bootstrap, `bun --no-install run <sdk>/src/boot.ts ./<entry>`,
in the package folder with `POCKET_SERVICE_SOCKET`, `_TOKEN` and `_ID` and the
locale, and nothing else of its environment. The bootstrap imports the entry
and calls its default export with the host; your code calls nothing to start.
Standard output and error are the service's log, line by line.

Working examples, each a complete package: [`examples/sample`](examples/sample)
(the reference `daemon-sample.ts`) and [`examples/link-channel`](examples/link-channel)
(the computer side of `dev.pocket-nexus.link-channel`).

## API

```ts
type ServiceFactory = (host: Host) => Service | Promise<Service>   // the entry's default export
interface Service {
  ready?: Promise<void>;                                            // channels offered meanwhile attach once it resolves
  attach(channel: AppletChannel, info: ChannelInfo): void | Promise<void>;
  stop?(): void | Promise<void>;                                    // service.stop, or the lease ended
}
interface ChannelInfo { device; deviceName; model; target; app; hash: string; generation: number }

interface Host {
  readonly id: string;              // the applet id
  log(line: string): void;          // a line of the service's log
  readonly store: Store;            // SERVICE-PROTOCOL.md §7
  readonly net: Net;                // not provided by this host: calls fail `not_required`
  readonly native: Native;          // not provided by this host: `files` throws `not_required`
}
interface Store {
  get(key, { signal }?): Promise<Uint8Array | null>
  getText(key, { signal }?): Promise<string | null>
  getJson<T>(key, { signal }?): Promise<T | null>
  set(key, value: Uint8Array | string | AsyncIterable<Uint8Array>, { signal, bytes }?): Promise<void>   // durable when it resolves
  setJson(key, value, { signal }?): Promise<void>
  delete(key, { signal }?): Promise<boolean>
  list({ prefix, after, limit, signal }?): Promise<{ keys: string[]; more: boolean }>
  usage({ signal }?): Promise<{ bytes: number; keys: number; maxBytes: number }>
}

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
  readonly image: Images; readonly text: Glyphs; readonly media: Media   // not provided by this host: calls fail `not_required`
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

**The store** keeps keys of 1 to 256 bytes of UTF-8 (no control character;
a key with a lone surrogate is refused before it is sent) and values of at
most 4 MiB, under the manifest's `store.maxBytes` for all of them (`quota`).
A `set` resolves once the value is durable; a reader sees the old value or
the new one. `list` pages in byte order: pass the last key as `after`.
Aborting a call's `signal` cancels it: a `set` cancelled before its last byte
writes nothing and rejects `cancelled`; one whose bytes are all sent completes
and resolves. A manifest without `store` in `requires` gets `not_required`
from every call, before anything is sent.

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
and exits with 0; the process also exits when Hub's control or host
connection closes. Hub sends `SIGTERM` 5 s after `service.stop`.

**Errors stay in their request.** A throwing request handler fails that
request; a throwing event handler, `onProgress` or `onClose` is logged. An
unhandled promise rejection is logged and the process keeps running. An exception thrown from a timer callback of your own is not caught:
it ends the process, and every device's channel with it.

## Development

```
bun install            # dev dependencies only: typescript, @types/bun
bun test               # mock Hub, in process and as child processes
bunx tsc --noEmit -p . # strict type check of src, tests and examples
```

`test/mock-hub.ts` is a frame-level mock of Hub's service socket that plays the
applet, and with `test/mock-host.ts` the host and its store; it records every
rule of SERVICE-PROTOCOL.md §5 and §6 a service's frames break. Use it to test
your own service.
