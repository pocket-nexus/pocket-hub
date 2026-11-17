# Services and the local service protocol

A **service** is the computer side of one applet (PocketJS `docs/POCKET-LINK.md`
§7.7): a Bun process that Pocket Hub starts when a device opens that applet's
channel and stops when it is idle. An applet and its service agree on their own
messages; Hub routes them and reads none of their content. A service gets no
device operation (no push, no screenshot, no file access): it sees the channels
of its own applet and nothing else.

A service never learns which carrier a device is on. Wi-Fi, USB, UART and
Bluetooth all end in the same channel connection described here.

**The link specification still calls a service a *daemon*.** Its wire names
keep that word until PocketJS changes them and this repository moves its pin:
the `channel.open` refusal `no_daemon` (§7.1) and the `channel.close` reason
`daemon` (§7.5) are what the device is sent, and this page names them as the
wire does.

This page is the contract between three parts of this repository:

| Part | Where | Role |
| --- | --- | --- |
| The service SDK | `sdk/service` (TypeScript, runs on Bun) | what a service author imports |
| The service host | `crates/service-host` | installs service packages, starts and stops their processes, owns the socket, matches tickets; answers the control API's service methods |
| The channel relay | `crates/hub-core` | joins a device's applet channel to one channel connection |

## 1. The service package

A service is distributed as its own artifact, apart from the applet's `.pocket`:
a folder (or a `.tar.gz` of it) with a manifest at its root. Installed from a
folder or a file on this computer, it is a **development release**.

```
pocket.service.json    the manifest
service.ts             the entry (any name; the manifest names it)
package.json           optional
bun.lock               optional (or bun.lockb); with package.json, Hub runs `bun install --frozen-lockfile --ignore-scripts`
```

A `.tar.gz` holds the package at its root or in one top-level folder. A package
holds files, folders and links that stay inside it, at most **512 MiB** and
**100,000** entries. An install from a folder skips `.git`, and `node_modules`
when the package has a lockfile.

### The manifest

`pocket.service.json`, at most **16 KiB**:

| Field | Type | Content | Check |
| --- | --- | --- | --- |
| `service` | integer | the manifest version, **1** | any other value is refused, with the version named, before any other field is read |
| `id` | string | the applet's manifest id; a service serves exactly this applet | 1 to 128 bytes of `[A-Za-z0-9._-]`, not `.` or `..` |
| `entry` | string | the file Bun runs | a relative path of normal components inside the package, at most 1,024 bytes, naming a regular file reached through no link |
| `requires` | array of strings? | the capabilities the service uses | each of `net`, `store`, `image`, `text`, `media`, `native`; no repeats; order carries no meaning. **A name this host does not provide is refused**, so a service written for a capability is never run without it. **This host provides none yet**: only a service that requires nothing installs. Absent or empty: a service that only computes for its applet |
| `hosts` | array of strings? | the destinations the service's capabilities may reach | at most **32**, no repeats; each a host entry (below) |
| `store` | object? | `{"maxBytes": <integer>}`, the size of the service's store | only with `store` in `requires`; `maxBytes` 1 to 268,435,456, default 16 MiB |
| `idleSeconds` | integer? | how long the process stays up with no channel open | 1 to 86,400; default **30** |

**A manifest with a field this table does not name is refused**, so a package
written for a later manifest version is not half understood. That includes the
`version` of the manifest before this one: **a service has no version of its
own**; what is installed is a release (below).

Each refusal is one sentence that names its field, and the control API's
error `service` with `reason: "manifest"` carries the field in `detail`
(CONTROL-API.md §1, Errors). The sentences:

| Field | Refused when | Sentence |
| --- | --- | --- |
| (the file) | missing; over 16 KiB; not JSON; not an object | `pocket.service.json is missing` / `… is over 16384 bytes` / `… is not JSON: <why>` / `… is not a JSON object` |
| `service` | absent, not an integer; another version | `` `service` must be the integer 1 `` / `manifest version 2 is not supported (this Hub reads version 1)` |
| any other | not in the table | ``pocket.service.json names `version`, a field this Hub does not read`` |
| `id`, `entry`, `requires`, `hosts`, `store`, `idleSeconds` | of the wrong type | `` `requires` must be an array of strings `` (the type the table says) |
| `id` | outside its rule | `id "a/b": 1 to 128 bytes of A-Z a-z 0-9 . _ -, and not . or ..` |
| `entry` | not a relative path inside; not a file there | `entry "../d.ts" is not a relative path inside the package` / `entry "d.ts" is not a file in the package` |
| `requires` | not a capability; repeated; not provided | `requires names "signin", which is not a capability (net, store, image, text, media, native)` / `requires names "net" twice` / `requires "net", which this Hub does not provide yet` |
| `hosts` | more than 32; not an entry; repeated; a wildcard over a public suffix | `hosts has 33 entries, more than 32` / `hosts entry "X.com" is not a lower-case host name, or `*.` and one` / `hosts names "x.com" twice` / `hosts entry "*.co.uk" covers co.uk, a public suffix` |
| `store` | without `store` in `requires`; `maxBytes` out of range | ``` `store` is allowed only with "store" in requires ``` / `store.maxBytes 0 is not between 1 and 268435456` |
| `idleSeconds` | out of range | `idleSeconds 0 is not between 1 and 86400` |

### Host entries

| Entry | Matches |
| --- | --- |
| `name.tld` | that name exactly: `x.com` matches `x.com`, not `api.x.com` |
| `*.name.tld` | any name that ends in `.name.tld`, with at least one label before it, not `name.tld` itself: `*.googlevideo.com` matches `rr3---sn-q4f.googlevideo.com` |

A name is lower-case labels of `[a-z0-9-]`, none starting or ending with `-`,
at least two of them, at most 253 bytes, as an applet's `hosts` writes them.
No address (a name whose last label is all digits, such as `192.0.2.1`), no
port, no scheme.

**A wildcard is refused when `name.tld` is a public suffix** of the Public
Suffix List, its private section included: `*.co.uk`, `*.github.io` and
`*.workers.dev` would admit every customer of a registry or a hosting
platform. The host carries a dated copy of the list,
`crates/service-host/data/public_suffix_list.dat` as publicsuffix.org serves it
(its header names its version and commit), applies the list's own algorithm
(its exception rules prevail, then the longest rule, then the implicit `*`),
and compares names as ASCII, the list's Unicode rules converted by IDNA. The
copy changes with this repository, never at run time.

This host gives a service no capability yet, so `hosts` admits nothing today:
it is checked at install and shown in the control API's `Service`.

### Dependencies

**A local install's dependencies are installed with
`bun install --frozen-lockfile --ignore-scripts`**: only what the lockfile pins,
and **no dependency's install script**, which would otherwise run at install
time with the owner's rights. A package with `package.json` and no lockfile
gets nothing installed and must ship what it needs. Dependencies that will not
install (a lockfile that does not match `package.json`, a package that cannot
be fetched) refuse the install with the control API's error `service`,
`reason: "dependencies"`, and the end of `bun install`'s output.

A dependency that needs its install script (to build a native addon, or to
download a program) is installed without what the script would have made. The
install succeeds; the service fails when it loads what is missing, with the
dependency's own error in its log, and its state is `failed`. Such a package
ships what the script makes, or does without the dependency.

**Hub provides the SDK.** A service imports `@pocketjs/service` and declares no
dependency for it: when a package's `node_modules` holds no `@pocketjs/service`
of its own, Hub links the copy it ships there, so the SDK a service runs always
speaks the protocol of the Hub that started it. A package that brings its own
copy keeps it.

### Releases and folders

What is installed is a **release** of a service: `dev` for a development
install. Hub keeps:

| What | Where |
| --- | --- |
| a release, as installed | `<home>/services/<id>/<release>-<8 hex>/`, a folder name unique to that install |
| the id's releases, each naming its folder | `<home>/services/<id>/releases.json` |
| the service's own files, mode 0700, kept across installs | `<home>/service-data/<id>/` |
| packages being installed | `<home>/services-staging/` |

`<release>` is `dev`, or 16 lower-case hex digits; nothing installs a release
of the second form yet.

**How a release folder appears.** A package is unpacked and checked, and its
dependencies installed, under `services-staging/`. `services/<id>/` appears
only by renaming into place a staged folder that already holds
`releases.json` and the first release, so a crash never leaves an id without
`releases.json`.

**Installing a release again cannot lose it.** `releases.json` names each
release's folder, and a folder's name is unique (`<release>-<8 hex>`, random
per install). An install renames the staged release into a fresh folder under
`services/<id>/`, then replaces `releases.json` by writing a temporary file
and renaming it, then removes the old folder; a process running from the old
folder keeps the files it has open. At every instant `releases.json` names a
complete folder, the old one or the new one. **At Hub's next start**, the
staging folder is removed, so is any folder `releases.json` does not name, and
a listed release whose folder is gone is dropped from it. A folder name that
does not follow `<release>-<8 hex>` is never read.

**An install over a running service** stops the process at once when it has
no channel; with channels open it keeps serving them on the release it
started from, and the new one applies at its next start.

**What an earlier Pocket Hub left.** One rule, read once per id: a folder
`services/<id>/` holding `releases.json` is of this layout; any other (a
`<version>/` folder and `current.json`) is **earlier**. An earlier id is listed
as one `Service` in state `failed` with `reason: "earlier"` and its sentence
("Installed by an earlier Pocket Hub; install it again."), and **nothing under
it is read, run or deleted** until the owner removes it, or installs the id
again, which replaces it. Its `service-data/<id>/` is kept as any other's.

**Which release a channel starts.** A device's `channel.open {app, hash}`
starts the `dev` release of `app` when one is installed; an id with none (or
an earlier one) is `no_daemon` (link specification §7.1).

## 2. The process

Hub runs `bun --no-install run ./<entry>` with the package folder as the working
directory: `./` keeps Bun from taking the name for a `package.json` script, and
`--no-install` from fetching at run time a package the lockfile did not pin.
The process gets these variables:

| Variable | Content |
| --- | --- |
| `POCKET_SERVICE_SOCKET` | path of Hub's service socket (a Unix domain socket) |
| `POCKET_SERVICE_TOKEN` | 32 lowercase hex digits, made for this process start |
| `POCKET_SERVICE_ID` | the applet id |
| `POCKET_SERVICE_DATA` | the service's data folder; `service.ready` repeats it (§4) |

**Exit codes.** A service exits with 0 after `service.stop` and when its control
connection closes, and with another code when it could not start or was
refused.

**The rest of the environment is not Hub's own.** Beside the four, the process
gets `PATH` (Bun's folder first), `HOME`, `USER`, `LOGNAME`, `SHELL`, `TMPDIR`,
`TZ`, `LANG`, `LANGUAGE`, `LC_*`, the proxy variables in either case, and
`SSL_CERT_FILE`, `SSL_CERT_DIR` and `NODE_EXTRA_CA_CERTS`.

The process's standard output and error are its log: Hub captures both, line by
line. **A service that exits is not restarted in a loop**: the next `channel.open`
starts it again, and a device that was refused `busy` retries after 2 s doubling
to 60 s (§7.1 of the link specification). **A process that ends without Hub
asking, before `service.ready` or within 10 s of its start, is a failed start**:
Hub holds further starts off for 2 s, doubling to 60 s, and answers an open
inside the hold with `busy` at once, without a spawn. An install of the package
or its owner's own start clears the hold.

## 3. Frames

Every connection on the service socket carries Pocket Link 2 frames: the 12-byte
header and the frame types of §4.2 and §4.3 of the link specification, in the
**local dialect** (`pocket_link::Dialect::Local`):

- no frame carries the `APP` flag or a channel id: the connection is the channel;
- a `REQUEST` may carry `BODY` (0x04) by itself;
- a `HANDSHAKE` frame (0x04, id 0, one JSON object of at most 1,024 bytes) is
  valid at any time and is used for the opening exchange of each connection;
  after `service.ready` or `channel.ready` a receiver ignores one;
- there is no preamble, no `PING`/`PONG` and no `CLOSE` frame: a side that is
  done closes the socket.

The payload limit is **16,384** on the control connection, and the device
link's negotiated limit on a channel connection (`payloadLimit` below), so Hub
forwards `DATA` frames in either direction without cutting them again.

## 4. The control connection

One per process, opened first and held for the process's life.

```
service                                           Hub
  | -- HANDSHAKE service.hello -----------------> |
  | <----------------- HANDSHAKE service.ready    |   or service.refused, then close
  | <----------------- EVENT channel.offer        |   once per channel a device opens
  | <----------------- EVENT service.stop         |
```

| Message | Direction | Object |
| --- | --- | --- |
| `service.hello` | service to Hub, `HANDSHAKE` | `{"t":"service.hello","protocol":2,"token":<POCKET_SERVICE_TOKEN>,"app":<applet id>}` |
| `service.ready` | Hub to service, `HANDSHAKE` | `{"t":"service.ready","app":<applet id>,"data":<data folder>}` |
| `service.refused` | Hub to service, `HANDSHAKE` | `{"t":"service.refused","message":<text>}`: an unknown token, or a `protocol` other than **2** (1 is the exchange of Pocket Hub 0.2.2 and earlier). Hub closes |
| `channel.offer` | Hub to service, `EVENT` id 0 | below |
| `service.stop` | Hub to service, `EVENT` id 0 | `{"t":"service.stop"}`: finish and exit |

`channel.offer`:

| Field | Type | Content |
| --- | --- | --- |
| `ticket` | string | 32 lowercase hex digits, for one channel connection |
| `device` | string | the device id, authenticated by the link's handshake |
| `deviceName`, `model`, `target` | string | the device's display name, model and build target |
| `app` | string | the applet id, asserted by the device's runtime |
| `hash` | string | 16 hex: the running package's footer hash |
| `generation` | integer | the channel's generation (§7.4): larger than every one Hub gave before |
| `payloadLimit` | integer | the payload limit of the channel connection |

- **`service.hello` arrives within 8 s of the process's start, and the first
  frame of any connection within 10 s of its accept**; Hub closes a connection
  that sends neither.
- **The service opens a channel connection for each offer within 10 s.** Hub
  holds the device's `channel.open` meanwhile and answers `busy` at 20 s in all
  (§7.1). A device that cancels its open leaves the process running; it stops
  at its idle time like any other.
- **The service sends nothing on the control connection after its hello.** Hub
  ignores a valid frame there, and a malformed one ends the lease.
- **Hub sends `service.stop` when the process has had no channel for
  `idleSeconds`**, or when its owner stops it. A process still running 5 s
  later gets `SIGTERM`, and `SIGKILL` 5 s after that.
- **A stop with channels open** (the owner's, an upgrade, Hub quitting) sends
  each of them `channel.close` `stop` (§5) at the same moment as `service.stop`.
  The two can arrive in either order; the device is told `daemon` (the
  link's close reason, §5).
- **The control connection is the process's lease.** When it closes, Hub closes
  the process's channels at once (the device is told `daemon`) and ends the
  process without a `service.stop`: `SIGTERM`, and `SIGKILL` 5 s later. A service
  whose control connection closes exits. **Hub signals the process group**, so
  a service's own children end with it.

## 5. A channel connection

One per accepted channel: one applet run on one device.

```
service                                           Hub
  | -- HANDSHAKE channel.attach ----------------> |
  | <----------------- HANDSHAKE channel.ready    |   or channel.refused, then close
  |   the channel's frames, both ways             |
  | <-- EVENT channel.close (either side) ------> |   then the socket closes
```

| Message | Direction | Object |
| --- | --- | --- |
| `channel.attach` | service to Hub, `HANDSHAKE` | `{"t":"channel.attach","ticket":<ticket>}` |
| `channel.ready` | Hub to service, `HANDSHAKE` | `{"t":"channel.ready","channel":<u32>,"messageLimit":<integer>}` |
| `channel.refused` | Hub to service, `HANDSHAKE` | `{"t":"channel.refused","message":<text>}`: an unknown or expired ticket. Hub closes |

**After `channel.attach` the service sends nothing until it has read
`channel.ready`.** A ticket is 16 random bytes, travels on the control
connection alone and opens one connection; Hub does not check which process
presents it, so the socket's mode (0600) is what keeps another user's process
out.

`channel` is the channel's id on the device's link, **an id of the device range
(`1`–`0x7fffffff`)**; no request or stream on this connection has it. `CREDIT`
uses it for event units and `channel.close` carries it (below). `messageLimit`
is LINK_APP_MSG for the device's link: `min(8,192, payloadLimit − 4)`.

After `channel.ready` the connection carries the channel's frames of §7.2 of the
link specification, with the meaning they have on the wire:

| Frame | Id | Notes |
| --- | --- | --- |
| `REQUEST` (`BODY` when the requester's bytes follow) | a fresh id of its sender | one JSON object with string `t`; ends with one `REPLY` |
| `REPLY` (`ERROR` for `{"code","message"}`) | the request's id | queued after every `EVENT` and `DATA` of the request |
| `EVENT` | **0**: a standalone event. **An open request's id**: its progress. **A fresh id of its sender**: the event opens a stream from sender to receiver. **`channel`**: `channel.close` | one JSON object with string `t` |
| `CANCEL` | the request's id | from the requester |
| `DATA` (`END` on the last frame of a direction) | the stream's id: a request's id, or the id of the event that opened it | bytes within the receiver's credit |
| `CREDIT` | a stream's id: **bytes**. **`channel`**: **event units** (§5.6) | u32, at least 1 |
| `RESET` | the stream's id | u16 code, u16 0 |

**Ids.** The applet's ids arrive in the device's range (`1`–`0x7fffffff`), as on
the wire. **The service allocates the ids of its own requests and of the streams
its events open from the Hub range (`0x80000001`–`0xffffffff`), from one counter
that starts again at each channel connection**, and never reuses one on a
connection. Hub maps them to its own ids on the device's link; the service never
sees those.

**Credit is the service's and the applet's, passed through.** Hub forwards
`CREDIT` frames both ways and adds none of its own, with one exception: **when
Hub drops an event of either side, it returns that event's units to its sender
with a `CREDIT` on `channel`** (a progress event for a request already
cancelled or released, an event over `messageLimit`, a late event of a released
id). The receiver never saw the event, so it would never regrant them, and the
sender's window would shrink with every cancelled request.

- Stream bytes start at credit 0 in each direction. The service grants what it
  can take on a stream it reads (256 KiB outstanding is the recommended window,
  refilled as its code consumes bytes) and sends `DATA` only within the credit
  the applet granted.
- Events start at 0 units in each direction; an event costs one unit per
  started 512 bytes of its JSON object, and progress events count. **The service
  grants 256 units with `CREDIT` on `channel` right after `channel.ready`** and
  regrants an event's units when that event leaves its queue, for any reason.
  It sends an event only when it holds the units for it: the applet grants 48.

**What Hub enforces on the service's frames**, since it answers to the device
for them:

| Rule | Hub's answer to the service |
| --- | --- |
| A JSON object over `messageLimit` | a request gets `REPLY` `ERROR` `too_large`; an event is dropped, and `RESET` `refused` answers the stream it opens |
| A ninth open request of the service | `REPLY` `ERROR` `busy` |
| A fifth open stream toward the applet | `RESET` `refused`. A stream toward the applet counts from its opening frame (a `REQUEST` with `BODY`, a stream-opening `EVENT`, the first `DATA` of a response) until its `END` or a `RESET`; for a fifth `REQUEST` with `BODY`, Hub forwards the `REQUEST` and answers `RESET` `refused` for its body **to both sides**: `RESET` abandons a stream in both directions, and the applet would otherwise wait on a body that never comes |
| `DATA` on the applet's request before any progress event on it (the head step, §7.2) | `RESET` `refused` for that id |
| `DATA` past the applet's credit, an event past the applet's units, a malformed frame | Hub closes the channel: `channel.close` `overflow` to the service, `daemon` to the device |

**What Hub does on the service's behalf:**

- **A `CANCEL` from the applet is answered by Hub** (§5.4): the device gets
  `REPLY` `ERROR` `cancelled` at once, and the service gets the `CANCEL` so it
  stops the work. The service sends no `REPLY` for that request; anything it
  still sends for the id is dropped.
- **Hub makes two outcomes of the service's own requests**: `REPLY` `ERROR`
  `timeout` when 30 s pass without a frame for the request (§5.5), and `REPLY`
  `ERROR` `cancelled` when the applet's host sends none within 10 s of the
  service's `CANCEL`. **Every `REQUEST` of the service therefore ends with
  exactly one `REPLY` on its channel connection, unless the channel closes
  first**; the service runs no timer of its own for them.

**Close.** `EVENT` `channel.close` with id `channel` and `{"t":"channel.close",
"reason":<reason>}` ends the channel in both directions; the sender closes the
socket after it. From Hub the reason is the device's (`exit`, `reset`,
`overflow`, `feature`) or Hub's own: `link` (the device's connection ended),
`policy`, `stop` (the service is being stopped), `overflow` (a rule above was
broken). **Hub sends one close notice per channel, never two, and nothing of the
channel after it.** A service that closes a channel sends `channel.close` with
any reason, or closes the socket; the device is told `daemon` either way.
