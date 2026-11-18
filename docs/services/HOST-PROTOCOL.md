# The local protocol's new direction: a service asks its host

Proposal (see `README.md`). Today the local protocol (`DAEMON-PROTOCOL.md`)
has a control connection that Hub talks on and a channel connection per
applet run; **a daemon never asks Hub for anything**. This page adds the
direction "a service asks its host for a capability" as **one more connection
kind on the same socket, in the same local dialect**, and says what of today's
protocol goes away. Section numbers `D§n` are `DAEMON-PROTOCOL.md`'s; `L§n`
are the link specification's.

**This is layer (b) of `DEVICES.md` §1, the same on every host**: Hub on a Mac
and the host on Omarchy Pocket speak it identically, so one service bundle
runs on either. "The host" below is whichever runs the service; §8 gives
each host's paths.

## 1. Three connections

| Connection | Opened by | When | Carries | Lease |
| --- | --- | --- | --- | --- |
| control | the service | first, once per process | the host's `channel.offer` and `service.stop` (as D§4) | **the process's lease** (as D§4) |
| channel | the service | once per `channel.offer` (as D§5) | one applet run's frames, relayed to the device unread | one channel |
| **host** | the service | **once per process, right after `service.ready`** | the service's capability requests and the host's answers | part of the lease: when it closes, the host ends the process as when the control connection closes |

**Why a connection of its own, and not requests on the control connection or
on a channel connection.** A channel connection is relayed to the device
frame by frame with the device's limits (LINK_APP_MSG, 8 requests, 4
streams); a capability request there would have Hub read applet traffic it
promises not to read (L§7.2) and would spend the applet's bounds. The control
connection carries `channel.offer`, which the service must act on within 10 s
(D§4); behind a megabyte of `DATA` from a fetch it would wait. A host
connection keeps both as they are, and its frames are the host's to read.

A channel capability (`image`, `text`, `media`) still names its channel: the
request carries the channel's **generation** (L§7.4), which the host
assigns from one counter for all its channels, so the host finds the channel and its device's statement (`EVENT
device.accepts`, L§7.9 as proposed) without the service ever handling a
device fact.

## 2. The process

The host runs, with the package folder as the working directory, and inside
the enclosure for a portable service (`TRUST.md` §5):

```
bun --no-install run <sdk>/src/boot.ts ./<entry>
```

`boot.ts` is part of the SDK the host ships: it reads the environment, opens the
connections, imports the entry and serves its default export, and exits as
D§2 says. Service code calls none of this (`CAPABILITIES.md` §2).

| Variable | Content |
| --- | --- |
| `POCKET_SERVICE_SOCKET` | the path of the host's service socket (today `POCKET_HUB_DAEMON_SOCKET`) |
| `POCKET_SERVICE_TOKEN` | 32 lowercase hex digits, made for this start (today `POCKET_HUB_DAEMON_TOKEN`) |
| `POCKET_SERVICE_ID` | the applet id (today `POCKET_HUB_DAEMON_APP`) |

**A portable service gets these three, `TZ`, `LANG` and `LC_*`, and nothing
else**: no `HOME`, `PATH`, proxy or certificate variable, because it reaches
no network, file or program of its own (`TRUST.md` §1). **Measured on
2026-11-17** (Bun 1.4.4 and 1.4.2, macOS 27.2): a real SDK service run that
way under the profile of `TRUST.md` §5 said its hello, took a channel and
answered requests; its socket path must be the canonical one. **A native service gets today's list** (D§2: `PATH`
with Bun's folder first, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TMPDIR`, the
locale, the proxy and certificate variables) beside the three.

## 3. The control connection

As D§4, with these changes. **The handshake's `protocol` changes at every
step that breaks it**: 1 is Pocket Hub's up to 0.2.2; **2 is step 1's** (pull request
#23: the messages renamed, nothing else); **3 is step 2's** (`service.ready`'s
members below, the host connection). A process that says another number is
refused with a sentence naming the SDK the host expects. From step 2 on a
mismatch cannot happen in practice: the host links its own copy of the SDK into
every package before each start and a store release cannot carry one
(`MANIFEST.md` §3). In step 1 it can: a development package that brought its
own copy keeps it (today's rule), and is refused.

| Message | Change |
| --- | --- |
| `service.hello` (was `daemon.hello`) | `{"t":"service.hello","protocol":3,"token":…,"app":…}` |
| `service.ready` (was `daemon.ready`) | `{"t":"service.ready","app":…,"release":<release>,"host":<ticket>,"files"?:<path>}`. **`data` is gone.** `release` names the release the process runs (§8). `host` is 32 lowercase hex digits, valid for one `host.attach` within 10 s. `files` is present for a native service only (`CAPABILITIES.md` §10) |
| `service.refused`, `service.stop` (were `daemon.*`) | renamed, unchanged |
| `channel.offer` | unchanged |

Each object stays within the 1,024 bytes of a `HANDSHAKE` (D§3); that is why
the manifest's `hosts` do not travel here (§4).

## 4. Opening the host connection

```
service                                           host
  | -- HANDSHAKE host.attach -------------------> |
  | <------------------- HANDSHAKE host.ready     |   or host.refused, then close
  |   requests, events, streams, both ways        |
```

| Message | Direction | Object |
| --- | --- | --- |
| `host.attach` | service to host | `{"t":"host.attach","ticket":<the host ticket of service.ready>}` |
| `host.ready` | host to service | `{"t":"host.ready","payloadLimit":65536,"requires":[…],"provides":[…]}` |
| `host.refused` | host to service | `{"t":"host.refused","message":<text>}`: an unknown or used ticket, a second host connection for the process. The host closes |

`requires` is the manifest's list, so the SDK refuses a capability the
manifest lacks before a frame leaves (`CAPABILITIES.md` §3); `provides` is
what this host provides, which always holds `requires` (a release that
requires more is refused at install). The manifest's `hosts` are not sent:
the SDK does not check a URL; the host does.

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
| Ids of the service's requests | **`1`–`0x7fffffff`**, from one counter per connection, **strictly increasing** | the service is the requester here, as a device is on the link |
| Ids of the host's requests | **`0x80000001`–`0xffffffff`**: the host sends one kind, `setup` (§6) | |
| The host's standalone events | id **0** | |
| Open requests of the service | at most **64** on the connection, and each capability's bound (`CAPABILITIES.md`); one more is answered `busy` at once | |
| Streams | at most one per request in each direction: the request's body toward the host, the response toward the service | |
| Event units | **none on this connection**: the host's only standalone event is `media.state`, at most one per change of a session, and a service holds at most 2 sessions | the event window of L§5.6 bounds a peer that can outrun the guest; nothing here can |

**Credit, both ways, as L§5.6.** Every stream starts at credit 0 in its
direction.

- **The response toward the service** exists from its head event on (the head
  step of L§7.2, kept so a response is always handed over before its bytes).
  The SDK grants a window of **256 KiB** while a `read()` is pending and
  refills it as its code takes bytes, as `StreamReader` does on a channel
  today; a response nobody reads holds 256 KiB at the host and makes the call
  end at its own time limit.
- **The request's body toward the host** is granted by the host as it consumes the
  bytes, **256 KiB** outstanding at most. A body declared with `bytes` must
  end at exactly that count.
- **`DATA` past the receiver's credit, a reused id, or a malformed frame
  closes the host connection**, and with it the process (§1): a service whose
  SDK breaks the protocol cannot be trusted with the rest of it.

## 6. The requests

`t` names the operation; the object's other members are its arguments.
**There is no stall timer on this connection, on either side** (§7); no
request here sends progress events.

| `t` | Arguments | Body (service → host) | Head event, then the response stream (host → service) | Reply |
| --- | --- | --- | --- | --- |
| `net.fetch` | `{url, method, headers, bytes, timeoutMs, maxBytes, redirect}` | `bytes` bytes when above 0 | `net.head` `{status, url, headers, truncated?}`, then the body | `{bytes}` after the body's `END` |
| `net.websocket` | `{url, headers, protocols}` | messages toward the server as records (below), until `END` | `net.open` `{protocol, headers}`, then messages from the server as records | `{code, reason}` once the socket closed |
| `store.get` | `{key}` | | `store.head` `{bytes}`, then the value; nothing when the key is absent | `{bytes}`, or `{absent:true}` |
| `store.set` | `{key, bytes}` | `bytes` bytes | | `{}` once durable |
| `store.delete` | `{key}` | | | `{existed}` |
| `store.list` | `{prefix?, after?, limit?}` | | | `{keys, more}` |
| `store.usage` | `{}` | | | `{bytes, keys, maxBytes}` |
| `image.render` | `{channel, url, headers?, width, height, fit?, round?, background?}` | | `image.head` `{width, height, contentWidth, contentHeight, format, bytes}`, then the IMG entry | `{bytes}` |
| `text.atlas` (for services once one needs it, `CAPABILITIES.md` §8) | `{channel, px, bold?, bytes}` | the codepoints as UTF-8, at most 16 KiB | `text.head` `{bytes, glyphs, missing, omitted}`, then the atlas | `{bytes}` |
| `media.open` | `{channel, url, headers?, at?, video?, durationMs?}` | | | `{session, source:{host, port, token}, codec, width, height, fps, durationMs, audio}` |
| `media.seek` | `{session, at}` | | | as `media.open`, with a new `source` |
| `media.close` | `{session}` | | | `{}` |

**The one request the host sends**: `REQUEST` **`setup`**, host to service,
`{"t":"setup","message":<the agent's JSON object, at most 32 KiB>}`, when an
agent calls `services.setup` (`SETUP.md` §2). The SDK calls `Service.setup`
and replies with its return value, or `ERROR` `unknown_setup` /
`invalid_setup`; a service without a handler is answered `unknown_setup` by
the SDK. The host waits 30 s for the reply.

| Event | Direction | Object |
| --- | --- | --- |
| `media.state` | host to service, id 0 | `{"t":"media.state","session":<n>,"state":"ended"\|"error"\|"closed","error"?:<text>}`; `closed` when the channel the session belongs to closed |

**A WebSocket message is one record** in either stream: a u32 little-endian
length of what follows, a u8 kind (`1` text in UTF-8, `2` binary, `8` close
with a u16 code and a UTF-8 reason), then the payload. A record never spans
the end of a stream. The service closes with a kind-8 record and `END`; the
host completes the closing handshake and replies. Credit on the two streams
is the backpressure: a service that stops reading stops the host reading the
socket.

**The `channel` argument** is the generation of a channel open on one of this
process's channel connections. Any other number, including a generation that
belongs to another service, is answered `closed`, so a service learns nothing
about channels that are not its own.

## 7. What the host checks, and how a request ends

Every request is attributed to the process, its release and its applet id
(§8). Before it does anything the host checks, in order: the object's shape
(`invalid`), the capability in the manifest's `requires` (`not_required`),
the `channel` (`closed`), the bound (`busy`), the destination against the
manifest's `hosts`, the origins granted to the service and the destination policy
(`denied`), the limits (`too_large`), the device's statement for a channel
capability (`not_accepted`). The checks and the codes are `CAPABILITIES.md`'s.

- **Every request ends with exactly one `REPLY`** unless the connection
  closes first. The host ends a request at its capability's own time limit
  (`net.fetch`'s `timeoutMs`), at 30 s for a store call, and **never for a request that lasts until it is closed**: a
  WebSocket ends when either side closes it. The SDK runs no timer of its
  own, as for a daemon's channel requests today (D§5). The 30 s stall of
  L§5.5 belongs to the channel between the service and its applet, which a
  service keeps alive with its own progress events.
- **`CANCEL` from the service**: the host stops the work, sends `RESET`
  `cancelled` for a stream still open, and replies `ERROR` `cancelled` within
  1 s. Cancelling `media.open` before its reply closes the session it was
  making; a session that exists is ended with `media.close`.
- **An error** is `REPLY` `ERROR` `{code, message, …}`: `message` at most 240
  bytes, a sentence for the service's log; extra members as
  `CAPABILITIES.md` names them (`status` for `http`).
- **The host logs each request on one line**, as it logs `net.http` today
  (`crates/hub-net` `Line`): the service, `t`, the target without
  credentials, query values or fragment, the outcome and the time. Never a
  header, a cookie, a body, a store value or a form value.

## 8. Releases and folders

The layout is the host's; a service sees none of it. **One name per folder**,
with the step that makes it (`MIGRATION.md` §3):

| What | Hub (`<home>`) | Born | Omarchy Pocket's host (proposed) |
| --- | --- | --- | --- |
| the service socket, mode 0600 | `<home>/service.sock` | step 1 (was `daemon.sock`) | `$XDG_RUNTIME_DIR/pocket-service-host/service.sock` |
| the local transport's socket (L§7.11) | none | | `$XDG_RUNTIME_DIR/pocket-service-host/channels.sock` |
| the control socket | `<home>/hubd.sock` (unchanged) | | `$XDG_RUNTIME_DIR/pocket-service-host/control.sock` (`DEVICES.md` §4) |
| packages being installed | `<home>/services-staging/` | step 1 | under the host's folder |
| an installed package | `<home>/services/<id>/<version>/` and `current.json` (step 1); **`<home>/services/<id>/<release>/` and `releases.json`** (step 2) | step 1, then step 2 | `~/.local/share/omarchy-pocket/service-host/services/<id>/<release>/` |
| the old data folder under its new name | `<home>/service-data/<id>/` | step 1; **no longer given to any process from step 2**, kept until the owner removes the service with `purge` | none |
| the store (`CAPABILITIES.md` §5) | `<home>/service-store/<id>/`, and `<id>@dev/` for a development release | step 2 | the same, under the host's folder |
| the pinned key, granted origins | `<home>/services/<id>/owner.json`, `origins.json` | step 2, step 5 | the same |
| a release's setup guide, read-only | `<home>/services/<id>/<release>/setup/` | step 5 | the same |
| a native service's folder | `<home>/service-files/<id>/` | step 5 | the same |
| the SDK's copy, written at each start | `<home>/sdk/service/` | step 1 (was `sdk/daemon`) | the same |
| the package index (`QUESTIONS.md` Q11) | `<home>/packages.json` | step 4 | the same |

**How a release folder appears.** `services/<id>/` appears only by renaming
into place a staged folder (under `services-staging/`) that already holds
`releases.json` and the first release, so a crash cannot leave a new install
without `releases.json` (which the rule below would read as earlier).

**Replacing a release without a window that loses it** (the `dev` release
installed again, the same footer hash installed again). As built in #27, the
old folder moves aside before the new one is renamed in, and a crash between
the two renames loses the release, which the owner installs again. **Proposed
instead**: `releases.json` names each release's folder (`{release, folder,
…}`), and a folder's name is unique (`<release>-<8 hex>`). An install renames
the staged release to a new folder name, then replaces `releases.json` by a
temporary file and a rename, then removes the old folder. At every instant
`releases.json` names a complete folder, the old or the new one. At start: a
folder that `releases.json` does not name is removed, and **a listed release
whose folder is gone is dropped from `releases.json`** with a log line (as
#27 does).

**How step 2 recognises what step 1 left.** One rule, read once per id at the
host's start: **a folder `services/<id>/` holding `releases.json` is of step 2;
any other is earlier.** An earlier id is listed as one `Service` in state
`failed`, reason `earlier`, `release` null ("installed by an earlier Pocket
Hub; install it again"); nothing under it is read or run. **It is deleted only
by the owner's own action**: removing the service, or installing it again,
which replaces it.
No `<version>/` folder or `current.json` is read beside `releases.json`.

`<release>` is **the footer hash of the applet package of that release** (16
hex, what `channel.open` carries), or `dev` for a development install.

**One process per running release**: a device on version 3 of an applet and
another on version 4 each get their own release's service. Built in step 2 as
one process per id, which is the same while `dev` is an id's only release;
**the difference becomes real at step 9**, when two signed releases of one id
can be installed at once. Each gets both with the id's
one store, whose limit is the newest installed release's `store.maxBytes`
(`QUESTIONS.md` Q16). The host keeps a release while a device it serves runs
or holds its hash and removes the others. `channel.open {app, hash}` starts
the release whose folder is `hash`, or the `dev` release when one is
installed (`TRUST.md` §8), and none whose consent is missing (`TRUST.md` §4).

## 9. What goes away, and what stays

**Goes away from `DAEMON-PROTOCOL.md`:**

| Today | Why |
| --- | --- |
| `POCKET_HUB_DAEMON_DATA`, `daemon.ready`'s `data`, `DaemonContext.data`, `<home>/daemon-data/<id>/` | replaced by `store` (`CAPABILITIES.md` §5); a portable service writes no file. An existing home's folders meet the rule of `MIGRATION.md` §3: kept, listed, never read as a store |
| the manifest's `version` and `<home>/daemons/<id>/<version>/`, `current.json` | releases (§8): the version is the applet's |
| `bun install --frozen-lockfile` for every package with a lockfile | a store release is bundled; a local install runs it with `--ignore-scripts` (`MANIFEST.md` §3) |
| "a package that brings its own copy [of the SDK] keeps it" | the host's copy always (`MANIFEST.md` §3) |
| `bun --no-install run ./<entry>` and `serve()` in the entry | the bootstrap of §2 |
| the inherited environment for every process | portable services get three variables and the locale (§2) |
| "The daemon sends nothing on the control connection after its hello" with nothing to ask for | kept as a rule; the asking moved to the host connection |

**Stays as it is**: the socket's mode and the ticket scheme, the control
connection as the lease, `channel.offer`, the channel connection and every
rule of D§5 (ids, credit, the head step, the bounds the host enforces,
`CANCEL`, the two outcomes the host makes, close), the timings and the start
backoff of D§2, `idleSeconds`, the process group signals, and "a process the
host starts inherits none of its descriptors". On Omarchy Pocket the channel
connection's far side is the local transport (L§7.11) instead of the link;
the service cannot tell.
