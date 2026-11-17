# The capabilities, as an API

Proposal (see `README.md`). Six capabilities (decided on 2026-11-17: `signin`
is not one; `SETUP.md`), each with the SDK surface a
service imports from `@pocketjs/service`, what it does, its limits, what the host
checks on every call, what "the framework uses it without a service" means,
and which existing applet needs each call. §11 lists the existing applets'
needs, taken from their use of `https://companion.pocket/<service>`; §12 says
where an existing PocketJS contract is reused and where this departs from it.

**Every host offers the same API**: Hub on a Mac for the 3DS and the Tab5,
and the host on Omarchy Pocket for itself (`DEVICES.md`). Which capabilities
each host provides is `DEVICES.md` §3; where an implementation differs by
host, the section says so.

## 1. Where each capability hangs

| Capability | Hangs on | Why there | Used by | Hosts that provide it |
| --- | --- | --- | --- | --- |
| `net` | `host.net` | the result does not depend on the device | services | all |
| `store` | `host.store` | idem; the store belongs to the service, not to a device | services | all |
| `image` | `channel.image` | the output is the format **the device at the other end of that channel** takes | services; applets through the framework | all |
| `text` | `channel.text` | the atlas is drawn at that device's density, in the atlas format it loads | applets through the framework; services once implemented (Q13) | all |
| `media` | `channel.media` | the stream is in a codec and size **that device** plays (H.264 on a New 3DS, PAL8 on an Old 3DS) | services; applets through the framework | all, for a device that can open a TCP connection to its host (§9) |
| `native` | `host.native` | arbitrary code: Bun's own interfaces, external programs | services | general-purpose computers the owner controls: Hub, Omarchy Pocket (`DEVICES.md` §5) |

A channel capability knows its device from the device's own statement
(`EVENT device.accepts`, link specification §7.9 as proposed): the formats,
codecs and density the device takes. **No host keeps a table of device models**
(`QUESTIONS.md` Q6).

**Decoding happens in the host's own process.** A host may itself be a weak
device, and a helper process per picture would cost it (the owner's
decision: decoding is performance-first). So bytes from the network reach
the decoders inside the host, `hubd` included, which holds every device's
link key. Two things follow: **memory-safe decoders are preferred where they
cost nothing** (the pure-Rust image path of `DEVICES.md` §3, on every host),
and **a URL is the only input form**: the image `{bytes}` source and the
media `feed` of the first revision are dropped, since no applet needs them.

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
  /** One setup message an agent delivered through the host (SETUP.md §2).
   *  Check it, write the store, return the reply; throw
   *  ServiceError("unknown_setup") or ServiceError("invalid_setup") to refuse.
   *  `channels` are the channels open now, to tell their applets. */
  setup?(message: Json, channels: AppletChannel[]): Json | void | Promise<Json | void>;
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
frame leaves**, with `ServiceError("not_required")`; the host refuses it again,
because the SDK is code inside the process (`TRUST.md` §1). Every member of
`Host` and of the channel exists whatever the manifest says, so code that
reads `host.net` does not crash on another host; it fails its call.

**Every call of every capability takes an optional `AbortSignal`** as
`signal` in its last argument, the store's included. Aborting sends `CANCEL`;
the call rejects with `cancelled`. The host stops the work and answers within 1 s.

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

Common error codes, each with one meaning (each capability adds its own):

| Code | Meaning |
| --- | --- |
| `not_required` | the manifest's `requires` does not name the capability |
| `denied` | the destination is refused: its host matches no entry of the manifest's `hosts` (nor an origin granted to the service, `SETUP.md` §3), or the destination policy refuses its address |
| `invalid` | an argument is malformed |
| `too_large` | an argument or a result is over its limit |
| `busy` | a bound of the tables below is reached; nothing was started |
| `unavailable` | the host lacks something now that a later call may find: the device's link is down, the media listener is not bound |
| `not_accepted` | the device's `device.accepts` names nothing of this kind (or the device has stated nothing) |
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
  maxBytes?: number;                   // default 1 MiB; at most 4 MiB
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

**Semantics.** The host performs the request with its own HTTP and TLS stack, as Hub
does `net.http` for a device (link specification §6.5), under **one
destination policy shared with the applet's `net.http`**: every hop resolved
and its address checked (no loopback, private, link-local, CGNAT, multicast,
unique-local or reserved address, except a granted origin's private
address, `SETUP.md` §3), redirects followed by the host, the system's proxy settings. The
difference between the two surfaces is whose host list applies: the
service's manifest `hosts` here, the applet's `applet.hosts` for `net.http`.
The host keeps no cookie jar: a service that holds a session sends its `cookie`
header itself, from its store. **A WebSocket lasts until either side closes
it**, with no time limit; its ping and pong are the host's, and the service sees
messages and the close.

Limits are sized to what the applets send (`QUESTIONS.md` Q29): no applet
uploads a file; X's and Discord's posts and Home Assistant's calls are small
JSON; the largest answers are X's timeline pages.

| Limit | Value | Reason |
| --- | --- | --- |
| URL | `http:`/`https:` (`ws:`/`wss:` for `websocket`), at most **16 KiB** encoded | web APIs put JSON in query strings (X's operation variables; their length not measured); the device's 2 KiB comes from its 16 KiB frame, which a local connection does not have |
| Headers | at most **64**, the encoded object at most **16 KiB**; `host`, `content-length`, `expect` and hop-by-hop headers are the host's (as `net.http` today) | |
| Request body | at most **256 KiB** | a post, a message, a call |
| Response | `maxBytes` default **1 MiB**, at most **4 MiB**; past it the service keeps the prefix and the call ends `response_too_large`, as `net.http` | a timeline page or a feed; `media` takes media URLs itself |
| Time | `timeoutMs` from the call to the body's end; default **30 s**, at most **300 s**. None for a WebSocket | |
| Redirects | at most **3**, each hop checked | as `net.http` |
| Open | **16** fetches and **4** sockets per service | |
| WebSocket message | at most **64 KiB** sent, **1 MiB** received | a gateway's first message for a bot in a few servers; not measured |

**The host checks**: `net` in `requires`; the URL's host matches an entry of the
manifest's `hosts` (`MANIFEST.md` §2) or a granted origin, at the first
hop and at every redirect hop; the destination policy at every connect; the
limits. Errors: the `NET_ERROR` codes of `contracts/spec/net.ts` (`dns`,
`connect`, `tls`, `timeout`, `redirect`, `response_too_large`, `protocol`,
`other`, …), and `denied` where `net.http` answers `invalid_request` for a
destination the policy refuses (a service is told plainly that its manifest
or the policy is the reason).

**The framework does not use `net`.** An applet's own HTTP stays `net.http`
from its worker (`io.http`), under the same policy with `applet.hosts`.

**Who needs it**: X (x.com, api.x.com, and the host of x.com's client scripts),
Discord (discord.com and its gateway, over `websocket`), Home Assistant (the
owner's own instance, over HTTP and its WebSocket API), Maps (a tile server
and a geocoder), Podcasts (a directory search and any feed's host), Spotify
on Omarchy Pocket (Q41).

## 5. `store`: the service's own key-value storage

```ts
type Opt = { signal?: AbortSignal };

export interface Store {
  get(key: string, options?: Opt): Promise<Uint8Array | null>;
  getText(key: string, options?: Opt): Promise<string | null>;
  getJson<T = unknown>(key: string, options?: Opt): Promise<T | null>;
  set(key: string, value: ByteSource, options?: Opt & { bytes?: number }): Promise<void>;
  setJson(key: string, value: unknown, options?: Opt): Promise<void>;
  delete(key: string, options?: Opt): Promise<boolean>;     // whether it existed
  list(options?: Opt & { prefix?: string; after?: string; limit?: number }): Promise<{ keys: string[]; more: boolean }>;
  usage(options?: Opt): Promise<{ bytes: number; keys: number; maxBytes: number }>;
}
```

**Semantics.** One store per applet id on a host, shared by every store
release and every process of that id, kept across upgrades, deleted only when
the owner removes the applet's service and asks for its data to go
(`services.remove` with `purge`). **A development install has a store of its
own** (`TRUST.md` §8). **A `set` is atomic and durable before it resolves**: a
reader sees the old value or the new one, never a part, and a crash after the
resolve keeps the new one. The last `set` of a key wins; there are no
transactions across keys. Tokens and sessions live here: X's `auth_token`
and `ct0`, Discord's bot token, Home Assistant's token (its address is a granted origin, `SETUP.md` §3).

| Limit | Value |
| --- | --- |
| Key | 1 to **256** bytes of UTF-8, no control character |
| Value | at most **4 MiB** |
| Whole store | the `store.maxBytes` of **the newest release installed**, which every running release of the id writes under: default **16 MiB**, at most **256 MiB** (`QUESTIONS.md` Q16) |
| `list` page | at most **1,000** keys, in byte order of the key |
| Open | **8** calls per service |

**The host checks**: `store` in `requires`; the limits. Error `quota`: the write
would pass the limit, and nothing was written. A missing key is no error:
`get` resolves `null`.

**Where it lives on Hub**: `<home>/service-store/<id>/` (on Omarchy Pocket,
the host's own folder, `DEVICES.md`), mode 0700, files
0600, in Hub's home, which an enclosed service cannot read (`TRUST.md` §5).
A native service and any other process of the owner's user can read it
(`TRUST.md` §2, `QUESTIONS.md` Q24). Values are not encrypted beyond the
file mode (`QUESTIONS.md` Q26).

**The framework does not use `store`.** An applet without a service keeps its
state on the device (`data.fs`), GitHub's token and Weather's places among it
(`QUESTIONS.md` Q8, which records that the direction's table does not cover
these two applets and what this costs on Omarchy Pocket).

**Who needs it**: X, Discord, Home Assistant, Maps (the saved view could stay
on the device), Podcasts (subscriptions and positions).

## 6. Sign-in is not a capability

**Decided on 2026-11-17.** A service that needs an account or a token gets it
from **a setup guide** its release ships: a coding agent on the service host's
machine follows it with the owner, outside the host, and hands the result to
the service through the host's **setup inlet**, which calls the service's
`setup` handler (§2). A private address the service must reach (a Home
Assistant instance) is **granted by the owner's click** as an origin. All
three are in `SETUP.md`. An applet whose setup nobody has done says so: "Not
signed in to X. Set it up on your Mac."

## 7. `image`: a network image in this device's texture format

```ts
export interface Images {
  render(source: { url: string; headers?: Record<string, string> }, box: ImageBox,
         options?: { signal?: AbortSignal }): Promise<RenderedImage>;
}

export interface ImageBox {
  width: number;                 // logical pixels of the device's surface, 1 to 512
  height: number;
  fit?: "contain" | "cover" | "fill";   // default contain
  round?: boolean;               // a circle cut from the box (avatars)
  background?: string;           // "#rrggbb" behind a contained image; transparent by default
}

export interface RenderedImage {
  /** One IMG entry, the layout uploadImgEntry takes (PocketJS framework/compiler/pak.ts). */
  entry: Uint8Array;
  /** The texture's sides: powers of two, at most the device's stated maxSide (≤ 512). */
  width: number;
  height: number;
  /** The picture, at the texture's top-left corner: what the applet draws. */
  contentWidth: number;
  contentHeight: number;
  format: "rgb565" | "rgba4444" | "rgba8888" | "t8";
}
```

**Semantics.** The host fetches the URL under `net`'s policy, decodes it in
the host (§1), scales it into the box at the device's density, applies
`fit`, `round` and `background`, and places it at the top-left corner of
**a texture whose sides are powers of two, no larger than the device's
stated `maxSide`**: `uploadImgEntry` refuses any other size
(`framework/compiler/pak.ts`: "pow2, <= TEX_MAX_DIM", 512 in
`contracts/spec/spec.ts`). It encodes **one IMG entry in a format the
device stated it takes**, choosing among them itself (an opaque photo as
RGB565, a round avatar with alpha, a picture of few colours as T8 with
PackBits when that is smaller). The service sends the entry and the content
size to its applet in its own message, typically a response stream after a
head event; the applet calls `uploadImgEntry` and draws the content
rectangle. The service never learns the device's pixel format.

| Limit | Value |
| --- | --- |
| Input formats | **JPEG, PNG, GIF (first frame), WebP**: the set every host must decode; others are `unsupported_input` |
| Input | at most **16 MiB**; at most **50 megapixels** decoded (a decompression bomb is refused before it is decoded) |
| Source | a URL, as `net.fetch`: the host in the manifest's `hosts`, the destination policy |
| Box | 1 to 512 logical pixels a side; the texture at most the device's `maxSide` |
| Output | one IMG entry; 512 × 512 RGBA8888, the largest, is 1 MiB |
| Open | **8** per service across its channels |

**The host checks**: `image` in `requires`; the channel named is open and belongs
to this process; the URL against `hosts`; the device stated image formats
(else `not_accepted`). Errors: `unsupported_input`, `http` (the source
answered a status outside 2xx; `extra.status`), the `NET_ERROR` codes of the
fetch, `too_large`.

**The framework uses it without a service.** An applet declares the
capability `image.url` and lists the image hosts in `applet.hosts`; the
framework's image module asks the system service over the system channel
(`image.render`, link specification §7.9 as proposed), and the host checks the URL
against **the applet's** hosts, taken from the package identified by the
system channel's `app` and `hash`. The applet sees a three-state resource
(PocketJS `ResourceState`: pending, ready with a `TextureResource`, error with
a code) and draws it with `ResourceImage`, both in `framework/src/resource*.ts`
today.

**Who needs it**: GitHub (avatars from avatars.githubusercontent.com, through
the framework), X (avatars 32 × 32 round and media pictures from
pbs.twimg.com, through its service), Discord (avatars and server icons from
its CDN), YouTube (thumbnails from i.ytimg.com, 128 × 64 today), Maps (tiles,
256 × 256 RGB565 today), Podcasts (cover art from any feed's host), Spotify
(covers).

## 8. `text`: glyphs the device lacks

**Through the framework now; for services, named and not implemented until a
service needs it** (no existing service lays out text; `QUESTIONS.md` Q13).
A manifest that requires `text` is refused at install until then
(`MANIFEST.md` §6). The surface it will have:

```ts
export interface Glyphs {
  atlas(request: {
    px: number;                  // the face's size in logical pixels, 6 to 64
    bold?: boolean;
    text: string;                // the codepoints wanted; at most 4,096
    signal?: AbortSignal;
  }): Promise<{
    /** The font atlas ui.loadFontAtlas takes, the format agent.glyphs carries (link §6.6). */
    atlas: Uint8Array;
    glyphs: number;
    missing: string;             // codepoints no font of the host has
    omitted: string;             // codepoints that did not fit this atlas: ask again for fewer
  }>;
}
```

**Semantics.** The host rasterizes printable ASCII and as many requested
codepoints as fit **4,096 glyphs and 1 MiB at that `px`** with its own fonts
(on a Mac, CoreText and the system's fonts) at the device's density, into one
atlas; `omitted` names the rest. **A codepoint keeps its advance in every
atlas of the same `px` and `bold`**, so text laid out with one atlas keeps
its layout when a later atlas replaces it; the applets' `ui/glyphs.ts`
depends on this today.

**The framework uses it without a service.** An applet declares
`text.glyphs.deferred` (not `text.glyphs.runtime` or `text.glyphs.streamed`,
whose meanings differ: `QUESTIONS.md` Q14); the framework's glyph module
asks the system service (`text.atlas`, its codepoints as the request's body,
since 4,096 of them do not fit LINK_APP_MSG) and loads the atlas into the
applet's slot, with the state of `ui/glyphs.ts` (which codepoints a loaded
atlas covers, a revision when one loads). This is what GitHub, X, Discord and
Spotify do today through the `glyphs` service.

**Not carried in this phase**: PocketJS has three other text contracts over
`io.offload` (`text.font`/`text.glyph` in `contracts/spec/text.ts` and
`docs/TEXT_RESOURCES.md`; `text.layout` and the others in
`docs/TEXT-OFFLOAD.md`; `font.open`/`font.glyphs` in `docs/DYNAMIC_TEXT.md`).
Their method names and objects could ride the system channel unchanged later.

## 9. `media`: a stream this device can play

```ts
export interface Media {
  open(source: { url: string; headers?: Record<string, string> },
       options?: { at?: number; video?: boolean; durationMs?: number; signal?: AbortSignal }): Promise<MediaSession>;
}

export interface MediaSession {
  /** What the applet hands its player: PocketJS MediaSource {host, port, token}. */
  readonly source: { host: string; port: number; token: string };
  readonly codec: "h264" | "pal8" | "rgb565";
  readonly width: number;        // the picture at the top-left of the media texture
  readonly height: number;
  readonly fps: number;          // 0 for sound alone
  readonly durationMs: number | null;   // null when live or unknown
  readonly audio: boolean;
  /** A new source from `at`; the previous one ends. */
  seek(at: number, options?: { signal?: AbortSignal }): Promise<MediaSession>;
  close(): void;
  readonly ended: Promise<{ state: "ended" | "error" | "closed"; error?: string }>;
}
```

**Semantics.** The host fetches the URL under `net`'s policy, decodes it in
the host (§1), and produces the stream of PocketJS `docs/MEDIA.md` for
**the device at the other end of the channel**: the codec from the device's
stated codecs (H.264 where the device has the decoder, PAL8 otherwise,
RGB565 for a host that uploads whole frames), the picture fitted in the
device's stated maximum (512 × 256 on a 3DS), sound as 22,050 Hz stereo IMA
ADPCM. `video: false` asks for sound alone (a podcast); `durationMs` gives
the length the applet knows when the source does not state it. **Transcode
or pass through is the host's choice**; with today's `MediaSource` the
device takes only the PKMV stream, so on a 3DS it is always a transcode. The
service passes `source` and the stream's facts to its applet, which opens
its player (`mediaPlayer`; `createMediaService` in
`framework/src/media-service.ts`). Pause is the device's (`MediaOps.paused`:
the producer's credits stop).

**The media bytes go from the host to the device on the side TCP connection the
link specification keeps (§1), not through the service and not on the
channel.** The host's listener for it binds at start on a port the OS assigns
(the source carries it), so there is no fixed port to conflict and nothing to
fall back from; a bind that fails stops the host's start with one line. It exists
on the TCP carrier alone, so a device on another carrier states no `media`
and gets `not_accepted` (`QUESTIONS.md` Q15).

| Limit | Value |
| --- | --- |
| Sessions | **one per channel** (a new `open` ends the previous, as a new source cancels the previous generation on the device); **2** per service |
| Tickets | one use, expire after **60 s** unused (`docs/MEDIA.md`) |
| Paused | a stream whose consumer takes nothing for **10 minutes** ends with state `ended` |
| Input | a URL, as `net.fetch` |

**The host checks**: `media` in `requires`; the channel; the URL against `hosts`;
the device stated codecs (else `not_accepted`). Errors: `unsupported_input`,
`http`, the `NET_ERROR` codes.

**The framework uses it without a service.** An applet declares `media.url`
and `media.playback`; the framework asks the system service (`media.open`,
`media.seek`, `media.close`, the `media.state` event) and gives the existing
`createMediaService` its client. The URL is checked against the applet's
hosts.

**Who needs it**: YouTube (through its service, from the address `yt-dlp`
resolves), Podcasts (episode sound, `video: false`, with the duration the
feed states).

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
enclosure** (`TRUST.md` §4). The SDK's other capabilities still work and the host
still checks them, but for a native service those checks bind only the SDK's
calls: the service can reach anything by its own means, which is what the
owner is told at install. **Device-dependent work still goes through the
channel**: YouTube resolves an address with `yt-dlp` and hands it to
`channel.media`, so the service never encodes for a device. How a vendor's
program (`yt-dlp`) is fetched, consented to and pinned is
`QUESTIONS.md` Q27.

**Hosts that provide it**: general-purpose computers the owner controls:
Hub and Omarchy Pocket (`DEVICES.md` §5). A host without it refuses the channel
(`channel.open` error `capability`, link specification §7.1 as proposed) and
the applet says, in the owner's words, that it needs a computer.

**Who needs it**: YouTube.

## 11. The existing applets' needs

From each applet's use of `https://companion.pocket/<service>` in
pocket-nexus/applets at `3edefb5`. The calls are the same on all three
devices (`DEVICES.md`). "Through the framework"
means the applet uses the capability with no service.

| Applet | Uses today | Becomes | Calls under this proposal |
| --- | --- | --- | --- |
| Hacker News | no service; `net.http` to hacker-news.firebaseio.com | no service | `net.http` only |
| Weather | `store` `/get`, `/set` (places) | no service | `net.http` to open-meteo; places in `data.fs` (Q8) |
| GitHub | `store` (token), `images` `/fetch?url&w&h&round` (avatars), `glyphs` `/atlas` | no service | `net.http` to api.github.com; `image.url` (avatars); `text.glyphs.deferred`; token in `data.fs` (Q8), sign-in by device flow (Q31) |
| X | `x` `/timeline?kind&cursor`, `/post/<id>`, `/{like,repost,…}?id`, `/reply`, `/avatar?u`, `/media?u&w&h`; `glyphs` | portable service | service: `net.fetch` (x.com, api.x.com, the scripts' host), `store` (session, operation ids), the setup inlet (the session's cookies, `SETUP.md`), `channel.image.render` (avatars, media); applet: `text.glyphs.deferred` |
| Discord | `discord` `/guilds`, `/guilds/<id>/channels`, `/channels/<id>/messages?before`, `?after&wait` (long poll), POST messages, `/<kind>/<owner>/<hash>` (pictures); `glyphs` | portable service | service: `net.fetch` and `net.websocket` (gateway), `store` (token), the setup inlet (the bot token), `channel.image.render`; the long poll becomes events the service emits; applet: `text.glyphs.deferred` |
| Home Assistant | `homeassistant` `/setup` (URL and token sent from the device), `/states`, `/call`, `/forget` | portable service | service: the setup inlet (a token) and a granted origin (`SETUP.md` §3), `store`, `net.fetch` to that origin (a private address, Q4) |
| Maps | `maps` `/tile?z&x&y` (256 × 256 RGB565), `/geocode?q`, `/locate`; `store` (view) | portable service | service: `net.fetch` (a tile server, a geocoder), `channel.image.render` (tiles); `/locate` has no capability (Q33); the view in `data.fs` |
| Podcasts | `podcasts` `/search?q`, `/episodes?feed`, `/play?url&at&duration&codec`, `/stop`, `/art?url&size`; `store` | portable service with any host (Q12) | service: `net.fetch`, `channel.media.open` (`video: false`, `durationMs`), `channel.image.render`, `store`; the codec is no longer the applet's choice |
| YouTube | `youtube` `/feed?topic`, `/search?q`, `/play?id&codec&at`, `/stop`, `/thumb?id` | unrestricted service | service: `native` (yt-dlp), `channel.media.open`, `channel.image.render`; the codec is no longer the applet's choice |
| Spotify | `spotify` (sign-in by a pairing code, player, library; a Rust program of its own that is the device's Spotify Connect speaker), `images`, `glyphs`; built for Omarchy Pocket alone | **unrestricted service on Omarchy Pocket's host** (it runs a program and plays through PipeWire) | service: `native`, `net.fetch`, `store`, `channel.image.render`; applet: `text.glyphs.deferred` (Q41) |
| App Store | `appstore` (Omarchy Pocket's own store client) | Omarchy Pocket's host installs releases on the device (Q40) | on Hub, a device's store page is the link's `catalog` and `install` (link §6.4) |
| Camera, Photos, Notes | nothing | no service | unchanged |

Two applets today pass a codec in their request (`youtube` `codec=h264|pal8`,
`podcasts` `codec=pal8`): the applet chose the device's format. Under this
proposal neither can, which is the rule of `README.md` §1 item 3.

The applets declaring the `glyphs` service are GitHub, X, Discord and Spotify
(four; the first revision said five).

## 12. What is reused from PocketJS, and where this departs

| Existing contract | Reused as | Departure, and why |
| --- | --- | --- |
| `contracts/spec/net.ts` (`NET_METHODS`, `NET_ERROR`) and link §6.5 (header rules, 3 redirects, destination policy) | `net.fetch` | the URL limit (16 KiB) and the response limit (4 MiB) differ from the device's, which come from the device's frame and memory; `denied` for a destination the manifest or policy refuses |
| `docs/MEDIA.md` (`MediaSource`, the PKMV wire, codecs `h264`/`pal8`/`rgb565`, one-use 60 s tickets, at most 4 tickets and streams, the side TCP connection) | `channel.media` output and the system service's `media.open` | the codec is chosen by the host from the device's statement, not by the applet or service |
| `framework/src/media-service.ts` (`createMediaService`) and `framework/src/service-client.ts` (`createServiceClient`) | the framework's media over the system channel, through a client adapter | none on the applet's side |
| `ui.loadFontAtlas` format, carried by `agent.glyphs` (link §6.6); the applets' `ui/glyphs.ts` request `{px, bold, slot, density, text}` | `text.atlas` | density comes from the device's statement; the codepoints travel as a body; `omitted` reports what did not fit |
| IMG entry (`uploadImgEntry`, `framework/compiler/pak.ts`): power-of-two sides at most TEX_MAX_DIM | `image.render` output | the content size travels beside it, since the picture sits in a padded texture |
| `framework/src/resource-state.ts`, `resource.ts` (`ResourceState`, `ResourceImage`), `resource-cache.ts` (`createResourceScheduler`) | the framework's three states | "unavailable" is `ResourceState`'s `error` with a code; no fourth shape |
| `contracts/spec/text.ts`, `docs/TEXT-OFFLOAD.md`, `docs/DYNAMIC_TEXT.md` (over `io.offload`) | not in this phase | Q13 |
| `docs/OFFLOAD.md` `httpResources`, `sqliteQueries` | not reused | a service uses `net` and `store` |
| `docs/APPLETS.md` "A service's owner setup" (Field, Vendor tool, Prompt; `/_setup`) | Field and Prompt → the setup guide and its inlet (`SETUP.md`); Vendor tool → open (`QUESTIONS.md` Q27) | the `/_setup` HTTP shape goes with `companion.pocket` |
