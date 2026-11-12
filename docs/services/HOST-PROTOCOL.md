# The local protocol's new direction: a service asks its host

Proposal (see `README.md`). Today the local protocol (`DAEMON-PROTOCOL.md`)
has a control connection that Hub talks on and a channel connection per
applet run; **a daemon never asks Hub for anything**. This page adds the
direction "a service asks its host for a capability" as **one more connection
kind on the same socket, in the same local dialect**, and says what of today's
protocol goes away. Section numbers `D§n` are `DAEMON-PROTOCOL.md`'s; `L§n`
are the link specification's.

## 1. Three connections

| Connection | Opened by | When | Carries | Lease |
| --- | --- | --- | --- | --- |
| control | the service | first, once per process | Hub's `channel.offer` and `service.stop` (as D§4) | **the process's lease** (as D§4) |
| channel | the service | once per `channel.offer` (as D§5) | one applet run's frames, relayed to the device unread | one channel |
| **host** | the service | **once per process, right after `service.ready`** | the service's capability requests and Hub's answers | part of the lease: when it closes, Hub ends the process as when the control connection closes |

**Why a connection of its own, and not requests on the control connection or
on a channel connection.** A channel connection is relayed to the device
frame by frame with the device's limits (LINK_APP_MSG, 8 requests, 4
streams); a capability request there would have Hub read applet traffic it
promises not to read (L§7.2) and would spend the applet's bounds. The control
connection carries `channel.offer`, which the service must act on within 10 s
(D§4); behind a megabyte of `DATA` from a fetch it would wait. A host
connection keeps both as they are, and its frames are Hub's to read.

A channel capability (`image`, `text`, `media`) still names its channel: the
request carries the channel's **generation** (L§7.4), which is unique for the
whole Hub, so Hub finds the channel and its device's statement (`EVENT
device.accepts`, L§7.9 as proposed) without the service ever handling a
device fact.

## 2. The process

Hub runs, with the package folder as the working directory:

```
bun --no-install run <home>/sdk/service/src/boot.ts ./<entry>
```

`boot.ts` is part of the SDK Hub ships: it reads the environment, opens the
connections, imports the entry and serves its default export, and exits as
D§2 says. Service code calls none of this (`CAPABILITIES.md` §2).

| Variable | Content |
| --- | --- |
| `POCKET_SERVICE_SOCKET` | the path of Hub's service socket (today `POCKET_HUB_DAEMON_SOCKET`) |
| `POCKET_SERVICE_TOKEN` | 32 lowercase hex digits, made for this start (today `POCKET_HUB_DAEMON_TOKEN`) |
| `POCKET_SERVICE_ID` | the applet id (today `POCKET_HUB_DAEMON_APP`) |

**A portable service gets these three, `TZ`, `LANG` and `LC_*`, and nothing
else**: no `HOME`, `PATH`, proxy or certificate variable, because it reaches
no network, file or program of its own (`TRUST.md` §2). Run that way under
the sandbox profile of `TRUST.md` §5, Bun started and served a module
(measured, `TRUST.md` §5). **A native service gets today's list** (D§2: `PATH`
with Bun's folder first, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TMPDIR`, the
locale, the proxy and certificate variables) beside the three.

## 3. The control connection

As D§4, with these changes:

| Message | Change |
| --- | --- |
| `service.hello` (was `daemon.hello`) | `{"t":"service.hello","protocol":2,"token":…,"app":…}`. **`protocol` is 2**: a process that says 1 is refused with a sentence naming the SDK Hub expects |
| `service.ready` (was `daemon.ready`) | `{"t":"service.ready","app":…,"release":<release>,"host":<ticket>,"files"?:<path>}`. **`data` is gone.** `release` names the release the process runs (§8). `host` is 32 lowercase hex digits, valid for one `host.attach` within 10 s. `files` is present for a native service only (`CAPABILITIES.md` §10) |
| `service.refused`, `service.stop` (were `daemon.*`) | renamed, unchanged |
| `channel.offer` | unchanged |

Each object stays within the 1,024 bytes of a `HANDSHAKE` (D§3); that is why
the manifest's `hosts` do not travel here (§4).

## 4. Opening the host connection

```
service                                            Hub
  | -- HANDSHAKE host.attach -------------------> |
  | <------------------- HANDSHAKE host.ready     |   or host.refused, then close
  |   requests, events, streams, both ways        |
```

| Message | Direction | Object |
| --- | --- | --- |
| `host.attach` | service to Hub | `{"t":"host.attach","ticket":<the host ticket of service.ready>}` |
| `host.ready` | Hub to service | `{"t":"host.ready","payloadLimit":65536,"requires":[…]}` |
| `host.refused` | Hub to service | `{"t":"host.refused","message":<text>}`: an unknown or used ticket, a second host connection for the process. Hub closes |

`requires` is the manifest's list, so the SDK refuses a capability the
manifest lacks before a frame leaves (`CAPABILITIES.md` §3). The manifest's
`hosts` are not sent: the SDK does not check a URL; Hub does.

**After `host.attach` the service sends nothing until it has read
`host.ready`.** A service that sends no `host.attach` within 10 s of
`service.ready` is ended as a failed start (D§2).

## 5. Frames, ids, sizes and credit

The host connection uses the local dialect of D§3: the 12-byte header, the
frame types of L§4.2 and L§4.3, no `APP` flag and no channel id, `REQUEST` with
`BODY` alone, no preamble, `PING`, `PONG` or `CLOSE`.

| Rule | Value | Reason |
| --- | --- | --- |
| Payload limit | **65,536** bytes; one JSON object fits one frame | a request names a URL of up to 16 KiB and headers of up to 16 KiB (`CAPABILITIES.md` §4); the device's 16 KiB comes from its memory, not from a Unix socket |
| Ids of the service's requests | **`1`–`0x7fffffff`**, from one counter per connection, never reused on it | the service is the requester here, as a device is on the link; Hub sends no request in this version |
| Hub's standalone events | id **0** | |
| Open requests of the service | at most **64** on the connection, and each capability's bound (`CAPABILITIES.md`); one more is answered `busy` at once | |
| Streams | at most one per request in each direction: the request's body toward Hub, the response toward the service | |
| Event units | **none on this connection**: Hub's only standalone event is `media.state`, at most one per change of a session, and a service holds at most 2 sessions | the event window of L§5.6 bounds a peer that can outrun the guest; nothing here can |

**Credit, both ways, as L§5.6.** Every stream starts at credit 0 in its
direction.

- **The response toward the service** exists from its head event on (the head
  step of L§7.2, kept so a response is always handed over before its bytes).
  The SDK grants a window of **256 KiB** while a `read()` is pending and
  refills it as its code takes bytes, as `StreamReader` does on a channel
  today; a response nobody reads holds 256 KiB at Hub and makes the call end
  at its own time limit.
- **The request's body toward Hub** is granted by Hub as it consumes the
  bytes, **256 KiB** outstanding at most. A body declared with `bytes` must
  end at exactly that count.
- **`DATA` past the receiver's credit, a reused id, or a malformed frame
  closes the host connection**, and with it the process (§1): a service whose
  SDK breaks the protocol cannot be trusted with the rest of it.

## 6. The requests

`t` names the operation; the object's other members are its arguments.
Long operations send progress events on the request's id **at least every
20 s**, so the request never reaches the 30 s stall of L§5.5.

| `t` | Arguments | Body (service → Hub) | Head event, then the response stream (Hub → service) | Reply |
| --- | --- | --- | --- | --- |
| `net.fetch` | `{url, method, headers, bytes, timeoutMs, maxBytes, redirect}` | `bytes` bytes when above 0 | `net.head` `{status, url, headers, truncated?}`, then the body | `{bytes}` after the body's `END` |
| `net.websocket` | `{url, headers, protocols}` | messages toward the server as records (below), until `END` | `net.open` `{protocol, headers}`, then messages from the server as records | `{code, reason}` once the socket closed |
| `store.get` | `{key}` | | `store.head` `{bytes}`, then the value; nothing when the key is absent | `{bytes}`, or `{absent:true}` |
| `store.set` | `{key, bytes}` | `bytes` bytes | | `{}` once durable |
| `store.delete` | `{key}` | | | `{existed}` |
| `store.list` | `{prefix?, after?, limit?}` | | | `{keys, more}` |
| `store.usage` | `{}` | | | `{bytes, keys, maxBytes}` |
| `signin.web` | `{title, url, until, timeoutMs}` | | progress `signin.wait` `{}` while the window is open | `{cookies}` or `{url}` |
| `signin.form` | `{title, help?, link?, fields, timeoutMs}` | | progress `signin.wait` `{}` | `{values}` |
| `image.render` | `{channel, source, width, height, fit?, round?, background?}`; `source` is `{url, headers?}` or `{bytes:<n>}` | the source's `n` bytes, for a bytes source | `image.head` `{width, height, format, bytes}`, then the IMG entry | `{bytes}` |
| `text.atlas` | `{channel, px, bold?, text}` | | `text.head` `{bytes, glyphs, missing}`, then the atlas | `{bytes}` |
| `media.open` | `{channel, source, at?}`; `source` is `{url, headers?}` or `{feed:true}` | | | `{session, source:{host, port, token}, codec, width, height, fps, durationMs, audio}` |
| `media.feed` | `{session}` | the input's bytes, until `END` | | `{bytes}` once the host consumed them |
| `media.seek` | `{session, at}` | | | as `media.open`, with a new `source` |
| `media.close` | `{session}` | | | `{}` |

| Event | Direction | Object |
| --- | --- | --- |
| `media.state` | Hub to service, id 0 | `{"t":"media.state","session":<n>,"state":"ended"\|"error"\|"closed","error"?:<text>}`; `closed` when the channel the session belongs to closed |

**A WebSocket message is one record** in either stream: a u32 little-endian
length of what follows, a u8 kind (`1` text in UTF-8, `2` binary, `8` close
with a u16 code and a UTF-8 reason), then the payload. A record never spans
the end of a stream. The service closes with a kind-8 record and `END`; Hub
completes the closing handshake and replies. Credit on the two streams is the
backpressure: a service that stops reading stops Hub reading the socket.

**The `channel` argument** is the generation of a channel open on one of this
process's channel connections. Any other number, including a generation that
belongs to another service, is answered `closed`, so a service learns nothing
about channels that are not its own.

## 7. What Hub checks, and how a request ends

Every request is attributed to the process, its release and its applet id
(§8). Before it does anything Hub checks, in order: the object's shape
(`invalid`), the capability in the manifest's `requires` (`denied`), the
`channel` (`closed`), the bound (`busy`), the destination against the
manifest's `hosts` and the destination policy (`denied`), the limits
(`too_large`), the device's statement for a channel capability
(`unavailable`). The checks and the codes are `CAPABILITIES.md`'s.

- **Every request ends with exactly one `REPLY`** unless the connection
  closes first: the capability's own time limit, or 30 s for one that has
  none (`store.*`). The SDK runs no timer of its own, as for a daemon's
  channel requests today (D§5).
- **`CANCEL` from the service**: Hub stops the work, sends `RESET`
  `cancelled` for a stream still open, and replies `ERROR` `cancelled` within
  1 s. Cancelling `media.open` before its reply closes the session it was
  making; a session that exists is ended with `media.close`.
- **An error** is `REPLY` `ERROR` `{code, message, …}`: `message` at most 240
  bytes, a sentence for the service's log; extra members as
  `CAPABILITIES.md` names them (`status` for `http`).
- **Hub logs each request on one line**, as it logs `net.http` today
  (`crates/hub-net` `Line`): the service, `t`, the target without
  credentials, query values or fragment, the outcome and the time. Never a
  header, a cookie, a body, a store value or a form value.

## 8. Releases and folders

| Path | Content |
| --- | --- |
| `<home>/service.sock` | the socket (today `daemon.sock`), mode 0600 |
| `<home>/services/<id>/<release>/` | one installed release's package. `<release>` is **the footer hash of the applet package of that release** (16 hex, what `channel.open` carries), or `dev` for a local development install |
| `<home>/services/<id>/releases.json` | each installed release: `{release, version, applet: <sha256>, service: <sha256>, key, signed, installedAt}` |
| `<home>/services/<id>/owner.json` | the publisher key Hub pinned for the id (`TRUST.md` §7) |
| `<home>/service-store/<id>/` | the store, shared by the id's releases (`CAPABILITIES.md` §5) |
| `<home>/service-files/<id>/` | a native service's own folder (`CAPABILITIES.md` §10) |
| `<home>/sdk/service/` | Hub's copy of the SDK, written at each start (today `<home>/sdk/daemon`) |

**One process per running release**: a device on version 3 of an applet and
another on version 4 each get their own release's service at the same time,
both with the id's one store (`QUESTIONS.md` Q12). Hub keeps a release while
a paired device reports its hash (`runtime.apps`) and removes the others.
`channel.open {app, hash}` starts the release whose folder is `hash`, or the
`dev` release when one is installed (`TRUST.md` §8).

## 9. What goes away, and what stays

**Goes away from `DAEMON-PROTOCOL.md`:**

| Today | Why |
| --- | --- |
| `POCKET_HUB_DAEMON_DATA`, `daemon.ready`'s `data`, `DaemonContext.data`, `<home>/daemon-data/<id>/` | replaced by `store`; a portable service writes no file. No data is migrated: no daemon outside this repository's examples exists |
| the manifest's `version` and `<home>/daemons/<id>/<version>/`, `current.json` | releases (§8): the version is the applet's |
| `bun install --frozen-lockfile` for every package with a lockfile | a store release is bundled; a local install runs it with `--ignore-scripts` (`MANIFEST.md` §3) |
| "a package that brings its own copy [of the SDK] keeps it" | Hub's copy always (`MANIFEST.md` §3) |
| `bun --no-install run ./<entry>` and `serve()` in the entry | the bootstrap of §2 |
| the inherited environment for every process | portable services get three variables and the locale (§2) |
| "The daemon sends nothing on the control connection after its hello" with nothing to ask for | kept as a rule; the asking moved to the host connection |

**Stays as it is**: the socket's mode and the ticket scheme, the control
connection as the lease, `channel.offer`, the channel connection and every
rule of D§5 (ids, credit, the head step, the bounds Hub enforces, `CANCEL`,
the two outcomes Hub makes, close), the timings and the start backoff of D§2,
`idleSeconds`, the process group signals, and "a process Hub starts inherits
none of Hub's descriptors".
