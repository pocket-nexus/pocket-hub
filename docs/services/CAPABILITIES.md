# The capabilities, as an API

Proposal (see `README.md`). Seven capabilities, each with the SDK surface a
service imports from `@pocketjs/service`, what it does, its limits, what Hub
checks on every call, what "the framework uses it without a service" means,
and which existing applet needs each call. §11 lists the existing applets'
needs, taken from their use of `https://companion.pocket/<service>`; §12 says
where an existing PocketJS contract is reused and where this departs from it.

## 1. Where each capability hangs

| Capability | Hangs on | Why there | Used by | Hosts that provide it |
| --- | --- | --- | --- | --- |
| `net` | `host.net` | the result does not depend on the device | services | all |
| `store` | `host.store` | idem; the store belongs to the service, not to a device | services | all |
| `signin` | `host.signin` | idem; the window opens on the host's machine | services | all |
| `image` | `channel.image` | the output is the format **the device at the other end of that channel** takes | services; applets through the framework | all |
| `text` | `channel.text` | the atlas is drawn at that device's density, in the atlas format it loads | services; applets through the framework | all |
| `media` | `channel.media` | the stream is in a codec and size **that device** plays (H.264 on a New 3DS, PAL8 on an Old 3DS) | services; applets through the framework | all |
| `native` | `host.native` | arbitrary code: Bun's own interfaces, external programs | services | computers only |

A channel capability knows its device from the device's own statement
(`EVENT device.accepts`, link specification §7.9 as proposed): the format,
codecs and density the device takes. **Hub keeps no table of device models**;
a device that has not stated them gets `unavailable` (`QUESTIONS.md` Q8).

## 2. The service module

```ts
// service.ts — the manifest's entry. Its default export is the service.
import type { ServiceFactory } from "@pocketjs/service";

const service: ServiceFactory = (host) => ({
  attach(channel, info) {
    channel.on("timeline", async (args, call) => { /* … */ });
  },
});
export default service;
```

```ts
export type ServiceFactory = (host: Host) => Service | Promise<Service>;

export interface Service {
  /** When present, the host answers the device's channel.open only once it resolves. */
  ready?: Promise<void>;
  /** One call per accepted channel: one applet run on one device. */
  attach(channel: AppletChannel, info: ChannelInfo): void | Promise<void>;
  /** The host stops the service. The channels close after it returns. */
  stop?(): void | Promise<void>;
}

export interface ChannelInfo {
  device: string;        // the device id, authenticated by the link's handshake
  deviceName: string;
  model: string;
  target: string;
  app: string;           // the applet id, asserted by the device's runtime
  hash: string;          // 16 hex: the running package's footer hash
  generation: number;    // the channel's generation (link §7.4)
}

export interface Host {
  /** The applet id this service serves. */
  readonly id: string;
  /** One line to the service's log. */
  log(line: string): void;
  readonly net: Net;
  readonly store: Store;
  readonly signin: SignIn;
  /** null unless the manifest requires "native". */
  readonly native: Native | null;
}
```

**The host loads the module and calls its default export; service code calls
no `serve()` and reads no environment.** Today a daemon ends with
`if (import.meta.main) await serve(daemon)` and `serve` reads
`POCKET_HUB_DAEMON_*`: both are Hub's process model leaking into service
code, and a device that hosts the same service has neither. The host's own
bootstrap (`HOST-PROTOCOL.md` §2) does that part.

`AppletChannel` keeps every member it has today (`on`, `request`, `emit`,
`emitStream`, `onEvent`, `state`, `onClose`, `close`, the call and stream
handles, `messageLimit`, the head step, the bounds of 8 requests and 4
streams) and gains three:

```ts
interface AppletChannel {
  // … as today (sdk/daemon/README.md "API") …
  readonly image: Images;   // §7
  readonly text: Glyphs;    // §8
  readonly media: Media;    // §9
}
```

`LinkError` (the SDK's error class today) is renamed **`ServiceError`**: the
same `code`, `message` and `extra`, for channel calls and capability calls
alike.

## 3. Rules every capability shares

**A capability the manifest does not require is refused by the SDK before a
frame leaves**, with `ServiceError("denied")`; Hub refuses it again, because
the SDK is code inside the process (`TRUST.md` §1). Every member of `Host` and
of the channel exists whatever the manifest says, so code that reads
`host.net` does not crash on another host; it fails its call.

**Every call takes an optional `AbortSignal`.** Aborting sends `CANCEL`; the
call rejects with `cancelled`. Hub stops the work and answers within 1 s.

**Bytes flow as streams under credit, both ways** (`HOST-PROTOCOL.md` §5):

```ts
/** What a call accepts as bytes: a whole buffer, text (UTF-8), or chunks
 *  produced as the host takes them. An iterable is read only as fast as the
 *  host grants credit, so a slow host slows the producer and nothing piles up. */
export type ByteSource = Uint8Array | string | AsyncIterable<Uint8Array>;

/** Bytes toward the service. Credit is granted only while a read() is pending,
 *  up to a 256 KiB window: a body nobody reads holds 256 KiB at most. */
export interface ByteReader {
  read(max?: number): Promise<Uint8Array | null>;   // null at the end
  readAll(limit: number): Promise<Uint8Array>;       // too_large past limit
  cancel(): void;                                    // RESET, the rest is discarded
}
```

Common error codes (each capability adds its own):

| Code | Meaning |
| --- | --- |
| `denied` | the manifest does not require the capability, or names no host the URL matches |
| `invalid` | an argument is malformed |
| `too_large` | an argument or a result is over its limit |
| `busy` | a bound of the table below is reached; nothing was started |
| `unavailable` | the host cannot do it now: no window can be shown, the device has not stated what it takes, the device takes nothing of this kind |
| `closed` | the channel the call named has closed |
| `timeout` | the call's own time limit passed |
| `cancelled` | the service aborted the call |
| `internal` | anything else; the message says what |

## 4. `net`: HTTP and WebSocket to the declared hosts

```ts
export interface Net {
  fetch(url: string, init?: FetchInit): Promise<FetchResponse>;
  websocket(url: string, init?: SocketInit): Promise<NetSocket>;
}

export type NetMethod = "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS";

export interface FetchInit {
  method?: NetMethod;                  // default GET
  headers?: Record<string, string>;
  body?: ByteSource;
  /** The body's length, required when body is an iterable. */
  bytes?: number;
  timeoutMs?: number;                  // default 30,000; at most 300,000
  maxBytes?: number;                   // default 4 MiB; at most 64 MiB
  redirect?: "follow" | "manual";      // default follow, at most 3 hops
  signal?: AbortSignal;
}

export interface FetchResponse {
  status: number;
  url: string;                         // the final URL after redirects
  headers: Record<string, string>;     // lower-case names; set-cookie joined with "\n"
  truncated: boolean;                  // headers were dropped to fit (as net.head)
  body: ByteReader;
  bytes(): Promise<Uint8Array>;
  text(): Promise<string>;
  json<T = unknown>(): Promise<T>;
}

export interface SocketInit {
  headers?: Record<string, string>;
  protocols?: string[];
  signal?: AbortSignal;
}

export interface NetSocket {
  readonly protocol: string;
  /** Resolves when the host has taken the message under credit. */
  send(message: string | Uint8Array): Promise<void>;
  /** The next message, or null once the socket closed. */
  receive(): Promise<string | Uint8Array | null>;
  close(code?: number, reason?: string): void;
  readonly closed: Promise<{ code: number; reason: string }>;
}
```

**Semantics.** Hub performs the request with its own HTTP and TLS stack, as it
does `net.http` for a device (link specification §6.5), under **one
destination policy shared with the applet's `net.http`**: every hop resolved
and its address checked (no loopback, private, link-local, CGNAT, multicast,
unique-local or reserved address), redirects followed by Hub, the system's
proxy settings. The difference between the two surfaces is whose host list
applies: the service's manifest `hosts` here, the applet's `applet.hosts`
for `net.http`. Hub keeps no cookie jar: a service that holds a session sends
its `cookie` header itself, from its store. A WebSocket's ping and pong are
Hub's; the service sees messages and the close.

| Limit | Value | Reason |
| --- | --- | --- |
| URL | `http:`/`https:` (`ws:`/`wss:` for `websocket`), at most **16 KiB** encoded | web APIs put JSON in query strings (X's operation variables); the device's 2 KiB comes from its 16 KiB frame, which a local connection does not have |
| Headers | at most **64**, the encoded object at most **16 KiB**; `host`, `content-length`, `expect` and hop-by-hop headers are Hub's (as `net.http` today) | |
| Request body | at most **16 MiB** | uploads of a picture or a file |
| Response | `maxBytes` default **4 MiB**, at most **64 MiB**; past it the service keeps the prefix and the call ends `response_too_large`, as `net.http` | a feed or a page, not a video (`media` takes URLs itself) |
| Time | `timeoutMs` from the call to the body's end; default **30 s**, at most **300 s** | |
| Redirects | at most **3**, each hop checked | as `net.http` |
| Open | **16** fetches and **4** sockets per service | |
| WebSocket message | at most **1 MiB** sent, **8 MiB** received | a gateway's first message for a bot in several servers is large; not measured |

**Hub checks**: `net` in `requires`; the URL's host matches an entry of the
manifest's `hosts` (`MANIFEST.md` §2) at the first hop and at every redirect
hop; the destination policy at every connect; the limits. Errors: the
`NET_ERROR` codes of `contracts/spec/net.ts` (`dns`, `connect`, `tls`,
`timeout`, `redirect`, `response_too_large`, `protocol`, `other`, …), and
`denied` where `net.http` answers `invalid_request` for a host the policy
refuses (a service is told plainly that its manifest is the reason).

**The framework does not use `net`.** An applet's own HTTP stays `net.http`
from its worker (`io.http`), under the same policy with `applet.hosts`.

**Who needs it**: X (x.com, api.x.com, abs.twimg.com), Discord (discord.com
and its gateway, over `websocket`), Home Assistant (the owner's own instance,
over HTTP and its WebSocket API), Maps (a tile server and a geocoder),
Podcasts (a directory search and any feed's host), Spotify if it moves to
Hub.

## 5. `store`: the service's own key-value storage

```ts
export interface Store {
  get(key: string, options?: { signal?: AbortSignal }): Promise<Uint8Array | null>;
  getText(key: string): Promise<string | null>;
  getJson<T = unknown>(key: string): Promise<T | null>;
  set(key: string, value: ByteSource, options?: { bytes?: number; signal?: AbortSignal }): Promise<void>;
  setJson(key: string, value: unknown): Promise<void>;
  delete(key: string): Promise<boolean>;            // whether it existed
  list(options?: { prefix?: string; after?: string; limit?: number }): Promise<{ keys: string[]; more: boolean }>;
  usage(): Promise<{ bytes: number; keys: number; maxBytes: number }>;
}
```

**Semantics.** One store per applet id on a host, shared by every release and
every process of that id, kept across upgrades, deleted only when the owner
removes the applet's service and asks for its data to go (`services.remove`
with `purge`). **A `set` is atomic and durable before it resolves**: a reader
sees the old value or the new one, never a part, and a crash after the
resolve keeps the new one. The last `set` of a key wins; there are no
transactions across keys. Tokens and sessions live here: X's `auth_token`
and `ct0`, Discord's bot token, Home Assistant's URL and token.

| Limit | Value |
| --- | --- |
| Key | 1 to **256** bytes of UTF-8, no control character |
| Value | at most **4 MiB** |
| Whole store | the manifest's `store.maxBytes`: default **16 MiB**, at most **256 MiB** |
| `list` page | at most **1,000** keys, in byte order of the key |
| Open | **8** calls per service |

**Hub checks**: `store` in `requires`; the limits. Error `quota`: the write
would pass `maxBytes`, and nothing was written. A missing key is no error:
`get` resolves `null`.

**Where it lives on Hub**: `<home>/service-store/<id>/`, mode 0700, files
0600, in Hub's home and nowhere a sandboxed service can read
(`TRUST.md` §2). Values are not encrypted beyond that; whether secrets go to
the macOS Keychain is `QUESTIONS.md` Q22.

**The framework does not use `store`.** An applet without a service keeps its
state on the device (`data.fs`): GitHub's token and Weather's places
(`QUESTIONS.md` Q2, which records that the direction's table does not cover
these two applets).

**Who needs it**: X, Discord, Home Assistant, Maps (the saved view could stay
on the device), Podcasts (subscriptions and positions), YouTube.

## 6. `signin`: the host asks the owner

```ts
export interface SignIn {
  /** Opens a window on the host's machine at `url` and waits until the
   *  condition holds, the owner closes it, or the time runs out. */
  web(options: {
    title: string;                     // "Sign in to X": the window's title
    url: string;                       // https; its host in the manifest's hosts
    until:
      | { cookies: { host: string; names: string[] } }   // these cookies are set for host
      | { redirect: string };                             // a navigation starts with this URL
    timeoutMs?: number;                // default and at most 15 minutes
    signal?: AbortSignal;
  }): Promise<{ cookies: Record<string, string> } | { url: string }>;

  /** Asks the owner for values in a form on the host's machine. */
  form(options: {
    title: string;                     // "Set up Discord"
    help?: string;                     // one paragraph shown above the fields
    link?: { label: string; url: string };   // "Open the Discord developer portal"
    fields: Array<{
      name: string;
      label: string;
      kind: "text" | "secret" | "origin";
      pattern?: string;                // checked by the host before the form closes
      maxLength?: number;              // default 512; at most 4,096
    }>;
    timeoutMs?: number;
    signal?: AbortSignal;
  }): Promise<Record<string, string>>;
}
```

**Semantics.** A sign-in happens where the host runs: on Hub, a window on the
Mac. **The window is shown by Pocket Hub's app**, which is a client of the
control API (`MIGRATION.md` §1): `hubd` has no window of its own. `web` uses
a fresh web data store for each sign-in that shares nothing with Safari or
with another service, shows the page's host in the window so the owner sees
where they type a password, and hands the service only what `until` names:
the named cookies' values, or the URL of the redirect (which is not loaded).
`form` shows the fields and checks each `pattern`; a field of kind `origin`
takes a URL, and the origin the owner names becomes a destination this
service may reach, **private addresses included for that origin alone**,
because a Home Assistant instance is on the owner's own network
(`QUESTIONS.md` Q5 and Q6).

The device shows the waiting state. The applet asks its service to sign in;
the service calls `signin.web`; the applet says, in the owner's words, "Sign
in to X on your Mac". The call sends progress at least every 20 s
(`HOST-PROTOCOL.md` §6), so a 15-minute wait trips no stall timer.

| Limit | Value |
| --- | --- |
| Time | at most **15 minutes** (X's sign-in window today) |
| At once | **one** sign-in per host machine; another call waits up to 10 s, then `busy` |
| Cookies | at most **8** names; each value at most **4 KiB** |
| Form | at most **8** fields; `title` 64 characters, `help` 512, `label` 64 |

**Hub checks**: `signin` in `requires`; the `url` and the cookie `host` match
the manifest's `hosts`. Errors: `declined` (the owner closed the window or
the form), `timeout`, `unavailable` (**no app is running that can show the
window**: Hub does not fall back to anything; the sentence says to open
Pocket Hub), `busy`.

`form` is this proposal's addition to the direction's `signin`
(`QUESTIONS.md` Q7): a Discord bot token is 70 characters and a Home
Assistant token over 180, and the alternative is typing them on a 3DS's
keyboard.

**The framework does not use `signin`.** An applet without a service that
needs an account uses a flow it can run with `net.http` and its own screen
(GitHub's device flow, `QUESTIONS.md` Q3).

**Who needs it**: X (`web`, cookies `auth_token` and `ct0` on `x.com`),
Discord (`form`: the bot token), Home Assistant (`form`: an `origin` and a
token), Spotify if it moves to Hub (`web` with `redirect`, OAuth).

## 7. `image`: a network image in this device's texture format

```ts
export interface Images {
  render(source: ImageSource, box: ImageBox, options?: { signal?: AbortSignal }): Promise<RenderedImage>;
}

export type ImageSource =
  | { url: string; headers?: Record<string, string> }   // fetched by the host under net's policy
  | { bytes: ByteSource; length?: number };             // bytes the service already holds

export interface ImageBox {
  width: number;                 // logical pixels of the device's surface, 1 to 1,024
  height: number;
  fit?: "contain" | "cover" | "fill";   // default contain
  round?: boolean;               // a circle cut from the box (avatars)
  background?: string;           // "#rrggbb" behind a contained image; transparent by default
}

export interface RenderedImage {
  /** One IMG entry, the layout uploadImgEntry takes (PocketJS framework/compiler/pak.ts). */
  entry: Uint8Array;
  width: number;                 // the entry's pixels
  height: number;
  format: "rgb565" | "rgba4444" | "rgba8888" | "t8";
}
```

**Semantics.** The host decodes the source, scales it into the box at the
device's density, applies `fit`, `round` and `background`, and encodes **one
IMG entry in a format the device stated it takes**, choosing among them
itself (an opaque photo as RGB565, a round avatar with alpha, a picture of
few colours as T8 with PackBits when that is smaller). The service sends the
entry to its applet in its own message, typically a response stream after a
head event; the applet calls `uploadImgEntry`. The service never learns the
device's pixel format, and a device added later needs no service change.

| Limit | Value |
| --- | --- |
| Input formats | **JPEG, PNG, GIF (first frame), WebP**: the set every host must decode, so a service does not depend on the Mac's wider set; others are `unsupported_input` |
| Input | at most **16 MiB**; at most **50 megapixels** decoded (a decompression bomb is refused before it is decoded) |
| URL source | as `net.fetch`: the host in the manifest's `hosts`, the destination policy |
| Box | 1 to 1,024 logical pixels a side; the output at most the device's stated largest texture side |
| Output | one IMG entry, at most side × side × 4 + 8 bytes |
| Open | **8** per service across its channels |

**Hub checks**: `image` in `requires`; the channel named is open and belongs
to this process; the URL against `hosts`; the device stated image formats
(else `unavailable`). Errors: `unsupported_input`, `http` (the source answered
a status outside 2xx; `extra.status`), the `NET_ERROR` codes of the fetch,
`too_large`.

**The framework uses it without a service.** An applet declares the
capability `image.url` and lists the image hosts in `applet.hosts`; the
framework's image module asks the system service over the system channel
(`image.render`, link specification §7.9 as proposed), and Hub checks the URL
against **the applet's** hosts, taken from the package identified by the
system channel's `app` and `hash`. The applet sees a three-state resource
(PocketJS `ResourceState`: pending, ready with a `TextureResource`, error with
a code) and draws it with `ResourceImage`, which exist in
`framework/src/resource*.ts` today.

**Who needs it**: GitHub (avatars from avatars.githubusercontent.com, through
the framework), X (avatars 32 × 32 round and media pictures from
pbs.twimg.com, through its service), Discord (avatars and server icons from
its CDN), YouTube (thumbnails from i.ytimg.com, 128 × 64 today), Maps (tiles,
256 × 256 RGB565 today), Podcasts (cover art from any feed's host), Spotify
(covers).

## 8. `text`: glyphs the device lacks

```ts
export interface Glyphs {
  atlas(request: {
    px: number;                  // the face's size in logical pixels, 6 to 64
    bold?: boolean;
    text: string;                // the codepoints wanted; at most 2,048
    signal?: AbortSignal;
  }): Promise<{
    /** The font atlas ui.loadFontAtlas takes, the format agent.glyphs carries (link §6.6). */
    atlas: Uint8Array;
    glyphs: number;
    /** Codepoints of `text` no font of the host has. */
    missing: string;
  }>;
}
```

**Semantics.** The host rasterizes printable ASCII and the requested
codepoints with its own fonts (on a Mac, CoreText and the system's fonts) at
the device's density, into one atlas. **A codepoint keeps its advance in
every atlas of the same `px` and `bold`**, so text laid out with one atlas
keeps its layout when a later atlas replaces it; `ui/glyphs.ts` in the
applets depends on this today.

| Limit | Value |
| --- | --- |
| Request | at most **2,048** codepoints |
| Atlas | at most **1 MiB**, at most **4,096** glyphs |
| Open | **2** per service |

**Hub checks**: `text` in `requires`; the channel; the device stated an atlas
format and density (else `unavailable`).

**The framework uses it without a service.** An applet declares
`text.glyphs.deferred`; the framework's glyph module asks the system service
(`text.atlas`) and loads the atlas into the applet's slot, with the state of
`ui/glyphs.ts` (which codepoints a loaded atlas covers, a revision when one
loads). This is what five applets do today through the `glyphs` service.

**Who needs it**: GitHub, X, Discord, Spotify through the framework. A
service needs `channel.text` only when it lays out text itself (none of the
existing applets does), so X's manifest does not require it
(`WALKTHROUGH.md`).

**Not carried in this phase**: PocketJS has three other text contracts over
`io.offload` (`text.font`/`text.glyph` in `contracts/spec/text.ts` and
`docs/TEXT_RESOURCES.md`; `text.layout` and the others in
`docs/TEXT-OFFLOAD.md`; `font.open`/`font.glyphs` in
`docs/DYNAMIC_TEXT.md`). Their method names and objects could ride the system
channel unchanged later; `QUESTIONS.md` Q10.

## 9. `media`: a stream this device can play

```ts
export interface Media {
  open(source: MediaInput, options?: { at?: number; signal?: AbortSignal }): Promise<MediaSession>;
}

export type MediaInput =
  | { url: string; headers?: Record<string, string> }   // fetched by the host under net's policy
  | { feed: AsyncIterable<Uint8Array> };                // bytes the service produces (a pipe from a program)

export interface MediaSession {
  /** What the applet hands its player: PocketJS MediaSource {host, port, token}. */
  readonly source: { host: string; port: number; token: string };
  readonly codec: "h264" | "pal8" | "rgb565";
  readonly width: number;        // the picture at the top-left of the media texture
  readonly height: number;
  readonly fps: number;
  readonly durationMs: number | null;   // null when live or unknown
  readonly audio: boolean;
  /** A new source from `at`; the previous one ends. Not for a feed. */
  seek(at: number): Promise<MediaSession>;
  close(): void;
  readonly ended: Promise<{ reason: "end" | "error" | "closed"; error?: string }>;
}
```

**Semantics.** The host opens the input, decodes it, and produces the stream
of PocketJS `docs/MEDIA.md` for **the device at the other end of the
channel**: the codec from the device's stated codecs (H.264 where the device
has the decoder, PAL8 otherwise, RGB565 for a host that uploads whole
frames), the picture fitted in the device's stated maximum (512 × 256 on a
3DS), audio as 22,050 Hz stereo IMA ADPCM. **Transcode or pass through is the
host's choice**; with today's `MediaSource` the device takes only the PKMV
stream, so on a 3DS it is always a transcode. The service passes `source` and
the picture facts to its applet, which opens its player (`mediaPlayer`,
`createMediaService`). **The media bytes go from Hub to the device on the side
TCP connection the link specification keeps (§1), not through the service
and not on the channel.** Pause is the device's (`MediaOps.paused`: the
producer's credits stop).

| Limit | Value |
| --- | --- |
| Sessions | **one per channel** (a new `open` ends the previous, as a new source cancels the previous generation on the device); **2** per service |
| Tickets | one use, expire after **60 s** unused (`docs/MEDIA.md`) |
| Paused | a stream whose consumer takes nothing for **10 minutes** ends with `reason: "end"` |
| Input | URL as `net.fetch`; a feed under credit |

**Hub checks**: `media` in `requires`; the channel; the URL against `hosts`;
the device stated codecs (else `unavailable`). Errors: `unsupported_input`,
`http`, the `NET_ERROR` codes.

**The framework uses it without a service.** An applet declares `media.url`
and `media.playback`; the framework asks the system service (`media.open`,
`media.seek`, `media.close`, the `media.state` event) and gives the existing
`createMediaService` its client. The URL is checked against the applet's
hosts.

**Who needs it**: YouTube (through its service, from the address `yt-dlp`
resolves), Podcasts (episode audio from any feed's host).

## 10. `native`: arbitrary code

```ts
export interface Native {
  /** A folder of this service's own, mode 0700, kept across upgrades:
   *  where a program the service fetches (yt-dlp) lives. */
  readonly files: string;
}
```

**Semantics.** A service whose manifest requires `native` may import `bun:*`
and `node:*` modules, start programs, open sockets and files, and use Bun's
own `fetch`. **It runs with every right of the owner's account and outside the
sandbox** (`TRUST.md` §4). The SDK's other capabilities still work and Hub
still checks them, but for a native service those checks bind only the SDK's
calls: the service can reach anything by its own means, which is what the
owner is told at install. **Device-dependent work still goes through the
channel**: YouTube resolves an address with `yt-dlp` and hands it to
`channel.media`, so the service never encodes for a device.

**Hosts that provide it**: computers. A host without it refuses the channel
(`channel.open` error `capability`, link specification §7.1 as proposed) and
the applet says, in the owner's words, that it needs a computer.

**Who needs it**: YouTube.

## 11. The existing applets' needs

From each applet's use of `https://companion.pocket/<service>` in
pocket-nexus/applets at `3edefb5`. "Through the framework" means the applet
uses the capability with no service.

| Applet | Uses today | Becomes | Calls under this proposal |
| --- | --- | --- | --- |
| Hacker News | no service; `net.http` to hacker-news.firebaseio.com | no service | `net.http` only |
| Weather | `store` `/get`, `/set` (places) | no service | `net.http` to open-meteo; places in `data.fs` (Q2) |
| GitHub | `store` (token), `images` `/fetch?url&w&h&round` (avatars), `glyphs` `/atlas` | no service | `net.http` to api.github.com; `image.url` (avatars); `text.glyphs.deferred`; token in `data.fs` (Q2), sign-in by device flow (Q3) |
| X | `x` `/timeline?kind&cursor`, `/post/<id>`, `/{like,repost,…}?id`, `/reply`, `/avatar?u`, `/media?u&w&h`; `glyphs` | portable service | service: `net.fetch` (x.com, api.x.com, abs.twimg.com), `store` (session, operation ids), `signin.web` (cookies), `channel.image.render` (avatars, media); applet: `text.glyphs.deferred` |
| Discord | `discord` `/guilds`, `/guilds/<id>/channels`, `/channels/<id>/messages?before`, `?after&wait` (long poll), POST messages, `/<kind>/<owner>/<hash>` (pictures); `glyphs` | portable service | service: `net.fetch` and `net.websocket` (gateway), `store` (token), `signin.form` (token), `channel.image.render`; the long poll becomes events the service emits; applet: `text.glyphs.deferred` |
| Home Assistant | `homeassistant` `/setup` (URL and token sent from the device), `/states`, `/call`, `/forget` | portable service | service: `signin.form` (an `origin` and a token), `store`, `net.fetch` to that origin (private address, Q5) |
| Maps | `maps` `/tile?z&x&y` (256 × 256 RGB565), `/geocode?q`, `/locate`; `store` (view) | portable service | service: `net.fetch` (a tile server, a geocoder), `channel.image.render` (tiles); `/locate` has no capability (Q28); the view in `data.fs` |
| Podcasts | `podcasts` `/search?q`, `/episodes?feed`, `/play?url&at&duration&codec`, `/stop`, `/art?url&size`; `store` | portable service with any host (Q5) | service: `net.fetch`, `channel.media.open`, `channel.image.render`, `store`; the codec is no longer the applet's choice |
| YouTube | `youtube` `/feed?topic`, `/search?q`, `/play?id&codec&at`, `/stop`, `/thumb?id` | unrestricted service | service: `native` (yt-dlp), `channel.media.open`, `channel.image.render`; the codec is no longer the applet's choice |
| Spotify | `spotify` (sign-in, player, library), `images`, `glyphs`; built for Omarchy Pocket (`rg-ds-plus`) only | out of Hub's scope until it targets a 3DS | if it moves: `signin.web` with `redirect`, `net.fetch`, `store`, `channel.image.render` |
| App Store | `appstore` (Omarchy Pocket's system store) | out of scope | a device's store page on Hub is the link's `catalog` and `install` (link §6.4) |
| Camera, Photos, Notes | nothing | no service | unchanged |

Two applets today pass a codec in their request (`youtube` `codec=h264|pal8`,
`podcasts` `codec=pal8`): the applet chose the device's format. Under this
proposal neither can, which is the rule of `README.md` §1 item 3.

## 12. What is reused from PocketJS, and where this departs

| Existing contract | Reused as | Departure, and why |
| --- | --- | --- |
| `contracts/spec/net.ts` (`NET_METHODS`, `NET_ERROR`) and link §6.5 (header rules, 3 redirects, destination policy) | `net.fetch` | larger limits (body 16 MiB, response 64 MiB, URL 16 KiB, 300 s): the device's limits come from the device's memory and frame, which a host's service does not share; `denied` for a host the manifest does not name |
| `docs/MEDIA.md` (`MediaSource`, the PKMV wire, codecs `h264`/`pal8`/`rgb565`, one-use 60 s tickets, at most 4 tickets and streams, the side TCP connection) | `channel.media` output and the system service's `media.open` | the codec is chosen by the host from the device's statement, not by the applet or service |
| `framework/src/media-service.ts` (`createServiceClient`, `createMediaService`) | the framework's media over the system channel, through a client adapter | none on the applet's side |
| `ui.loadFontAtlas` format, carried by `agent.glyphs` (link §6.6); the applets' `ui/glyphs.ts` request `{px, bold, slot, density, text}` | `text.atlas` | density comes from the device's statement, not from the applet; the atlas holds the same cells |
| IMG entry (`uploadImgEntry`, `framework/compiler/pak.ts`) | `image.render` output | none |
| `framework/src/resource-state.ts`, `resource.ts` (`ResourceState`, `ResourceImage`), `resource-cache.ts` (`createResourceScheduler`) | the framework's three states | "unavailable" is `ResourceState`'s `error` with a code; no fourth shape |
| `contracts/spec/text.ts`, `docs/TEXT-OFFLOAD.md`, `docs/DYNAMIC_TEXT.md` (over `io.offload`) | not in this phase | Q10 |
| `docs/OFFLOAD.md` `httpResources`, `sqliteQueries` | not reused | a service uses `net` and `store` |
| `docs/APPLETS.md` "A service's owner setup" (Field, Vendor tool, Prompt; `/_setup`) | Field → `signin.form`; Prompt → the applet's own screen; Vendor tool → a native service's own business | the `/_setup` HTTP shape goes with `companion.pocket` |
