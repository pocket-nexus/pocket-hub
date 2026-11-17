# Services and the local service protocol

A **service** is the computer side of one applet (PocketJS `docs/POCKET-LINK.md`
§7.7): a module whose default export Pocket Hub loads into a Bun process of
its own when a device opens that applet's channel, and stops when it is idle.
An applet and its service agree on their own messages; Hub routes them and
reads none of their content. A service gets no device operation (no push, no
screenshot, no file access): it sees the channels of its own applet, and asks
its host for the capabilities its manifest requires (§6, §7), and nothing else
reaches it through Hub.

**A service runs inside an enclosure** (§2): its process reads its own
package and Hub's SDK, connects to Hub's service socket, and nothing else of
the machine; the capabilities below are its only way out, and Hub checks
each call. The enclosure stops a service; it does not stop a hostile program
the owner runs, which needs no Hub.

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
| The service SDK | `sdk/service` (TypeScript, runs on Bun) | what a service author imports, and the bootstrap that loads a service (§2) |
| The service host | `crates/service-host` | installs service packages, starts and stops their processes, owns the socket, matches tickets, answers a service's capability requests and keeps its store; answers the control API's service methods |
| The channel relay | `crates/hub-core` | joins a device's applet channel to one channel connection |

## 1. The service package

A service is distributed as its own artifact, apart from the applet's `.pocket`:
a folder (or a `.tar.gz` of it) with a manifest at its root. Installed from a
folder or a file on this computer, it is a **development release**.

```
pocket.service.json    the manifest
service.ts             the entry (any name; the manifest names it): its default export is the service (§2)
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
| `entry` | string | the module the host loads (§2) | a relative path of normal components inside the package, at most 1,024 bytes, naming a regular file reached through no link |
| `requires` | array of strings? | the capabilities the service uses | each of `net`, `store`, `image`, `text`, `media`, `native`; no repeats; order carries no meaning. **A name this host does not provide is refused**, so a service written for a capability is never run without it. **This host provides `store`** (§7). Absent or empty: a service that only computes for its applet |
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

No capability this host provides reaches a host, so `hosts` admits nothing
today: it is checked at install and shown in the control API's `Service`.

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

**Hub provides the SDK, and its copy always wins.** A service imports
`@pocketjs/service` and declares no dependency for it: Hub links the copy it
ships at `node_modules/@pocketjs/service` at install and again before each
start, replacing whatever the package brought there, so the SDK a service runs
always speaks the protocol of the Hub that started it. Hub writes its copy to
`<home>/sdk/service` at each of its own starts.

### Releases and folders

What is installed is a **release** of a service: `dev` for a development
install. Hub keeps:

| What | Where |
| --- | --- |
| a release, as installed | `<home>/services/<id>/<release>-<8 hex>/`, a folder name unique to that install |
| the id's releases, each naming its folder | `<home>/services/<id>/releases.json` |
| packages being installed | `<home>/services-staging/` |
| a development release's store (§7) | `<home>/service-store/<id>@dev/` |
| the store of the id's other releases (none installs yet) | `<home>/service-store/<id>/` |
| the data folder an earlier Pocket Hub gave a service | `<home>/service-data/<id>/`: given to no process, kept until the owner removes the service with `purge` |

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
again, which replaces it. Its `service-data/<id>/` is kept until a `purge`.

**Which release a channel starts.** A device's `channel.open {app, hash}`
starts the `dev` release of `app` when one is installed; an id with none (or
an earlier one) is `no_daemon` (link specification §7.1).

## 2. The process

Hub runs, with the release's folder as the working directory and inside
the enclosure (below):

```
bun --no-install run <home>/sdk/service/src/boot.ts ./<entry>
```

`boot.ts` is the SDK's bootstrap: it reads the three variables below, opens
the control connection (§4) and, after `service.ready`, the host connection
(§6), imports the entry, calls its **default export** with the host and serves
the service it returns. Service code calls none of this and reads no
environment. `--no-install` keeps Bun from fetching at run time a package the
lockfile did not pin.

```ts
// service.ts — the manifest's entry
import type { ServiceFactory } from "@pocketjs/service";

const service: ServiceFactory = (host) => ({
  attach(channel, info) {
    channel.on("visits", async () => {
      const visits = ((await host.store.getJson<number>("visits")) ?? 0) + 1;
      await host.store.setJson("visits", visits);
      return { visits };
    });
  },
});
export default service;
```

| Variable | Content |
| --- | --- |
| `POCKET_SERVICE_SOCKET` | path of Hub's service socket (a Unix domain socket) |
| `POCKET_SERVICE_TOKEN` | 32 lowercase hex digits, made for this process start |
| `POCKET_SERVICE_ID` | the applet id |

**The process gets these three, `TZ`, `LANG` and `LC_*`, and nothing else of
Hub's environment**: no `PATH`, `HOME`, proxy or certificate variable, since
it reaches no network, file or program of its own (below).

**The SDK's surface** (`sdk/service/src/index.ts`): the default export is a
`ServiceFactory`, `(host: Host) => Service | Promise<Service>`. `Host` has
`id` (the applet id), `log(line)` (a line of the service's log), and the
capabilities: **`store`** (§7) and `net` and `native`; an `AppletChannel` has,
beside its channel calls (§5), `image`, `text` and `media`. **Every member
exists whatever the manifest says**, so code that reads `host.net` does not
crash: on this host `net`, `native`, `image`, `text` and `media` are not
provided, no manifest can require them (§1), and every call of theirs fails
with `ServiceError` `not_required`. `Service` has `attach(channel, info)` (one
call per accepted channel), `stop()` (the host stops the service; the channels
close after it returns) and `ready` (when present, the channels offered before
it resolves are attached once it does). `LinkError` of the reference is
`ServiceError`: `code`, `message` and `extra`, for channel calls and capability
calls alike.

**Exit codes.** A service exits with 0 after `service.stop` and when its lease
ends, and with another code when it could not start or was refused: an entry
that cannot be loaded, a default export that is not a function, a factory that
throws.

The process's standard output and error are its log: Hub captures both, line by
line. **A service that exits is not restarted in a loop**: the next `channel.open`
starts it again, and a device that was refused `busy` retries after 2 s doubling
to 60 s (§7.1 of the link specification). **A process that ends without Hub
asking, before `service.ready` or within 10 s of its start, is a failed start**,
and so is one that attaches no host connection within 10 s of `service.ready`
(§6): Hub holds further starts off for 2 s, doubling to 60 s, and answers an
open inside the hold with `busy` at once, without a spawn. An install of the
package or its owner's own start clears the hold.

### The enclosure

**Every service process runs inside the enclosure, and nothing runs a
service outside it.** On macOS Hub starts the process as

```
/usr/bin/sandbox-exec -p <profile> <bun> --no-install run <sdk>/src/boot.ts ./<entry>
```

`sandbox-exec` applies the profile to itself and `exec`s Bun, so the process
Hub supervises, signals and stops (with its process group) is Bun's. The
profile is made for each start from **canonical** paths (a home reached
through a symbolic link, or with a space in its path, is named by its real
path; the socket path in `POCKET_SERVICE_SOCKET` is the canonical one, since
the profile names that path): Bun's executable, the release's folder, Hub's
copy of the SDK and the service socket, and nothing else of the home.
`<…>` marks what is generated:

```
(version 1)
(deny default)
(allow process-exec (literal <bun>))
(allow file-read* (literal <bun>))
(allow file-read* (subpath <release folder>) (subpath <sdk>))
(allow file-read-metadata (literal "/") <(literal …) for each folder above bun, the release folder, the sdk and the socket>)
(allow file-read-data (literal "/"))
(allow file-read* (subpath "/usr/lib") (subpath "/System/Library")
  (subpath "/usr/share/zoneinfo") (subpath "/usr/share/icu")
  (subpath "/private/var/db/timezone") (literal "/private/etc/localtime")
  (literal "/dev/urandom") (literal "/dev/random") (literal "/dev/null"))
(allow sysctl-read (sysctl-name-prefix "hw.") (sysctl-name-prefix "machdep.")
  (sysctl-name-prefix "kern.os") (sysctl-name "kern.version") (sysctl-name "kern.boottime")
  (sysctl-name "kern.argmax") (sysctl-name "kern.hostname") (sysctl-name "kern.maxfilesperproc")
  (sysctl-name "kern.usrstack64") (sysctl-name "sysctl.proc_translated")
  (sysctl-name "kern.secure_kernel") (sysctl-name "vm.footprint_suspend"))
(deny process-info* (target others))
(allow network-outbound (remote unix-socket (path-literal <service socket>)))
```

Paths are written as the profile language's strings, `"` and `\` escaped.

| A service can | A service cannot |
| --- | --- |
| read its own release's files (its `node_modules` included) and Hub's copy of the SDK | read, list or `stat` anything else of the home or the owner's folders: another service's package, any store's file, its own included (it reaches its store through the host connection only, §7) |
| connect to the service socket: its control, host and channel connections | connect to any other socket: Hub's control socket (`hubd.sock`), another Unix socket, TCP; resolve a name |
| use timers, `Intl` with a time zone, `crypto.subtle`, write its log (standard output and error, which Hub holds) | write any file, its own package included |
| call native code through `bun:ffi` (below) | start a program, by `spawn`, `exec` or `fork` |
| | list the machine's processes, or read another process's command line |

**Accepted leaks**, kept because the profile cannot close them without
breaking Bun, and asserted by the escape test (`tests/enclosure.rs`) so a
change in Bun or macOS that widens or closes one is seen:

- **a system library through `bun:ffi`**: native calls run, and every system
  call they make still meets the profile;
- **the command lines of the host and of its ancestors that run as the
  owner** (`KERN_PROCARGS2`), which the profile does not deny: the
  proposal measured them readable from a process started in its starter's
  process group. Measured here on macOS 27.2 with Bun 1.4.4: a process in a
  process group of its own, as Hub starts every service, reads none of them,
  its ancestors' included; the escape test asserts that, so a change that
  opens it is seen;
- the machine's name and its hardware and system version (`kern.hostname`,
  `hw.*`, `kern.os*`), and the metadata of the folders above Bun, the release,
  the SDK and the socket (that they exist).

**What it is for, and what not.** It stops a portable service, a package
that may come from someone the owner does not know, from reaching the
owner's files, the network, other programs and Pocket Hub's control socket.
**It does not stop a hostile program the owner runs**: such a program needs
no Hub to run code, and can read every store, which are files of the
owner's user. `sandbox-exec` is an interface Apple marks deprecated; it
remains in macOS 27.2, and the escape test would fail on the day it changes.

**When the enclosure cannot be set up** (no `sandbox-exec`, a path a profile
cannot name, `sandbox-exec` refusing the profile or unable to start Bun, which
it says on standard error before anything of the service's), nothing runs:
the owner's start is the control API's error `service` with
`reason: "enclosure"` and one sentence, a device's open is answered `busy`,
and the service is listed `failed` with that sentence. **No flag, variable or
build setting runs a service outside it.** On a system other than macOS no
enclosure is built yet, and no service starts there, with the same reason.

**The install is not enclosed.** The dependencies of a development package
are installed by `bun install --frozen-lockfile --ignore-scripts` run as Hub,
outside the enclosure, with the network: no install script runs, and what it
fetches is what the lockfile pins (§1).

## 3. Frames

Every connection on the service socket carries Pocket Link 2 frames: the 12-byte
header and the frame types of §4.2 and §4.3 of the link specification, in the
**local dialect** (`pocket_link::Dialect::Local`):

- no frame carries the `APP` flag or a channel id: the connection is the channel;
- a `REQUEST` may carry `BODY` (0x04) by itself;
- a `HANDSHAKE` frame (0x04, id 0, one JSON object of at most 1,024 bytes) is
  valid at any time and is used for the opening exchange of each connection;
  after `service.ready`, `channel.ready` or `host.ready` a receiver ignores one;
- there is no preamble, no `PING`/`PONG` and no `CLOSE` frame: a side that is
  done closes the socket.

The payload limit is **16,384** on the control connection, the device link's
negotiated limit on a channel connection (`payloadLimit` below), so Hub
forwards `DATA` frames in either direction without cutting them again, and
**65,536** on the host connection (§6).

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
| `service.hello` | service to Hub, `HANDSHAKE` | `{"t":"service.hello","protocol":3,"token":<POCKET_SERVICE_TOKEN>,"app":<applet id>}` |
| `service.ready` | Hub to service, `HANDSHAKE` | `{"t":"service.ready","app":<applet id>,"release":<release>,"host":<ticket>}`: `release` is the release the process runs (§1); `host` is 32 lowercase hex digits, good for one `host.attach` (§6) |
| `service.refused` | Hub to service, `HANDSHAKE` | `{"t":"service.refused","message":<text>}`: an unknown token, or a `protocol` other than **3** (1 is the exchange of Pocket Hub 0.2.2 and earlier; 2 had `service.ready` carry a data folder and no host connection). Hub closes |
| `channel.offer` | Hub to service, `EVENT` id 0 | below |
| `service.stop` | Hub to service, `EVENT` id 0 | `{"t":"service.stop"}`: finish and exit |

A protocol mismatch does not happen in practice: Hub links its own SDK into
every package before each start (§1).

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
- **Offers start once the host connection is attached (§6).** The service opens
  a channel connection for each offer within 10 s. Hub holds the device's
  `channel.open` meanwhile and answers `busy` at 20 s in all (§7.1). A device
  that cancels its open leaves the process running; it stops at its idle time
  like any other.
- **The service sends nothing on the control connection after its hello.** Hub
  ignores a valid frame there, and a malformed one ends the lease.
- **Hub sends `service.stop` when the process has had no channel for
  `idleSeconds`**, or when its owner stops it. A process still running 5 s
  later gets `SIGTERM`, and `SIGKILL` 5 s after that.
- **A stop with channels open** (the owner's, an upgrade, Hub quitting) sends
  each of them `channel.close` `stop` (§5) at the same moment as `service.stop`.
  The two can arrive in either order; the device is told `daemon` (the
  link's close reason, §5).
- **The control connection and the host connection are the process's lease.**
  When either closes, Hub closes the process's channels at once (the device is
  told `daemon`) and ends the process without a `service.stop`: `SIGTERM`, and
  `SIGKILL` 5 s later. A service whose control or host connection closes
  exits. **Hub signals the process group**, so a service's own children end
  with it.

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

## 6. The host connection

One per process, opened right after `service.ready`: on it the service asks
its host for a capability. **The service is the requester here**, as a device
is on its link; the host reads every frame, and nothing on it reaches a
device. It is a connection of its own so that a channel connection stays the
applet's, relayed with the device's limits, and the control connection stays
free for `channel.offer`.

```
service                                           Hub
  | -- HANDSHAKE host.attach -------------------> |
  | <-------------------- HANDSHAKE host.ready    |   or host.refused, then close
  |   requests, their bodies and responses        |
```

| Message | Direction | Object |
| --- | --- | --- |
| `host.attach` | service to Hub, `HANDSHAKE` | `{"t":"host.attach","ticket":<the host ticket of service.ready>}` |
| `host.ready` | Hub to service, `HANDSHAKE` | `{"t":"host.ready","payloadLimit":65536,"requires":[…],"provides":["store"]}`: the manifest's `requires`, and what this host provides |
| `host.refused` | Hub to service, `HANDSHAKE` | `{"t":"host.refused","message":<text>}`: an unknown ticket, or a used one (a second host connection for the process). Hub closes |

**After `host.attach` the service sends nothing until it has read
`host.ready`.** A process that sends no `host.attach` within 10 s of
`service.ready` is ended as a failed start (§2). **The host connection is
part of the lease** (§4).

**The SDK refuses locally** a capability `requires` does not name, with
`ServiceError` `not_required`, before a frame leaves; the host refuses it
again, since the SDK is code inside the process.

| Rule | Value |
| --- | --- |
| Payload limit | **65,536** bytes; one JSON object fits one frame |
| Ids of the service's requests | **`1`–`0x7fffffff`**, from one counter that only increases: an id at or below the last one ends the connection |
| Open requests of the service | at most **64** on the connection, and each capability's own bound (§7); one more is answered `busy` at once |
| Streams | at most one per request in each direction: the request's body toward the host, the response toward the service |
| Event units | none on this connection |
| Requests from the host | none in this version; a service answers one with `ERROR` |

**Credit, both ways, as §5.6 of the link specification.** Every stream starts
at credit 0 in its direction.

- **The response toward the service** exists from its head event on (the head
  step of §7.2): an `EVENT` on the request's id, then `DATA`. The SDK grants a
  window of **256 KiB** while it reads and refills it as it takes bytes.
- **The request's body toward the host** is granted by the host as it takes
  the bytes, **256 KiB** outstanding at most and never past the count the
  request declares; the last `DATA` carries `END`, and a body of 0 bytes is
  one empty `DATA` with `END`, which needs no credit.
- **`DATA` past the receiver's credit, a reused id, a `REPLY` (the host asks
  nothing), a `PING`, `PONG` or `CLOSE`, or a malformed frame ends the host
  connection**, and with it the process: a service whose SDK breaks the
  protocol is not trusted with the rest of it. `DATA` already in flight for a
  request that ended is dropped, within the credit it had.

**How a request ends.** `t` names the operation; the object's other members
are its arguments. **Every request ends with exactly one `REPLY`** unless the
connection closes first: `{…}` or, with `ERROR`, `{code, message}` (`message`
at most 240 bytes, a sentence for the service's log). The host checks, in
this order: the object's shape (`invalid`), the capability in `requires`
(`not_required`), the bounds (`busy`), the limits (`too_large`). **A store
call ends at 30 s** (`timeout`), unless its body is complete, when its commit
decides the reply; the SDK runs no timer of its own. **`CANCEL` from the
service** stops the work: a stream of the request still open gets `RESET`
`cancelled`, and the reply is `ERROR` `cancelled` at once, except for a write
that has started (§7). A `RESET` from the service on its body or on the value
it reads ends the request `cancelled` the same way.

A call of a capability this host does not provide (`net.fetch`,
`net.websocket`, `image.render`, `text.atlas`, `media.open`, `media.seek`,
`media.close`) is answered `not_required`; any other unknown `t`, `invalid`.

**The host logs each request on one line** (its log, not the service's): the
service, `t`, the outcome and the time. Never a key or a value.

## 7. `store`

```ts
type Opt = { signal?: AbortSignal };

interface Store {
  get(key: string, options?: Opt): Promise<Uint8Array | null>;
  getText(key: string, options?: Opt): Promise<string | null>;
  getJson<T = unknown>(key: string, options?: Opt): Promise<T | null>;
  set(key: string, value: ByteSource, options?: Opt & { bytes?: number }): Promise<void>;
  setJson(key: string, value: unknown, options?: Opt): Promise<void>;
  delete(key: string, options?: Opt): Promise<boolean>;        // whether it existed
  list(options?: Opt & { prefix?: string; after?: string; limit?: number }): Promise<{ keys: string[]; more: boolean }>;
  usage(options?: Opt): Promise<{ bytes: number; keys: number; maxBytes: number }>;
}
type ByteSource = Uint8Array | string | AsyncIterable<Uint8Array>; // an iterable needs `bytes`
```

**One store per release kind of an id**: a development release's
(`<id>@dev/`) and, once releases come from the store, the one their releases
share (`<id>/`). It is kept across installs and removals, deleted only when
the owner removes the service with `purge`. **A `set` is atomic and durable
before it resolves**: a reader sees the old value or the new one, never a
part, and a crash after the resolve keeps the new one. The last `set` of a
key wins; there is no transaction across keys. A missing key is no error:
`get` resolves `null`.

| Limit | Value |
| --- | --- |
| Key | 1 to **256** bytes of UTF-8, no control character. **A key with a lone surrogate is refused by the SDK** (`invalid`) before it is encoded, since it would encode to the bytes of another key; the host refuses a key that is not valid UTF-8 (`invalid`) |
| Value | at most **4 MiB** (`too_large`) |
| Whole store | `maxBytes`: the manifest's `store.maxBytes` of the release installed (default **16 MiB**, at most **256 MiB**). A write that would take `meta.bytes` past it is `quota`, and nothing is written |
| `list` page | at most **1,000** keys (the default), in byte order of the key |
| Open | **8** store calls per connection (`busy`) |

| `t` | Arguments | Body (service → host) | Head event, then the response stream (host → service) | Reply |
| --- | --- | --- | --- | --- |
| `store.get` | `{key}` | | `store.head` `{bytes}`, then the value; nothing when the key is absent | `{bytes}`, or `{absent:true}` |
| `store.set` | `{key, bytes}`, with `BODY` | `bytes` bytes | | `{}` once durable |
| `store.delete` | `{key}` | | | `{existed}` |
| `store.list` | `{prefix?, after?, limit?}` | | | `{keys, more}` |
| `store.usage` | `{}` | | | `{bytes, keys, maxBytes}` |

Errors beside §6's: `quota` (the write would pass the limit; nothing was
written) and `store_format` (below).

**On disk.** The format is kept across upgrades:

| Point | Rule |
| --- | --- |
| The file | one SQLite database per store, `store.sqlite` in the store's folder, made by exactly the statements below, in this order. A key is bytes (`/` included), so no key is mapped to a file name |
| Who writes | **only Hub's process**, through one connection per store guarded by one lock; two processes writing at once are two requests Hub serializes, so the quota is exact |
| A `set` | the value's whole body is received first; then one transaction replaces the row and updates `meta`, the quota checked inside it against `meta.bytes`; the reply follows the commit, which `synchronous = FULL` makes only once the write-ahead log is synced (below, "Durable") |
| The folder | mode 0700, the file 0600; the folder is synced (`fsync`) after the database and its write-ahead log are made |
| `list` | `after` is the last key returned (exclusive), keys in byte order; `more` says whether a later key exists at that moment; keys added or removed between pages show or not by their order |
| `CANCEL` | **before the body's last byte**: nothing is written and the reply is `cancelled`; **after it**: the transaction completes and the `set`'s own reply is sent. A `delete` is never cancelled once received |
| Disk use | `maxBytes` bounds **`meta.bytes`**, the sum of every key's and value's length, not the file: pages and the write-ahead log add to it. Hub keeps both small: `PRAGMA incremental_vacuum` after a commit that leaves more than a quarter of the pages free, `PRAGMA wal_checkpoint(TRUNCATE)` after one that leaves the log over 4 MiB. The control API's `Service.store` shows the file's size beside `bytes` |
| A limit below the usage | when a release's `maxBytes` is below `meta.bytes`, nothing is deleted: a `set` that would raise `meta.bytes` fails `quota`; a `set` that lowers it or leaves it as it is, and a `delete`, succeed |
| A newer format | a store whose `user_version` is above **1** was written by a newer Pocket Hub: a store call gets `ERROR` `store_format`, the control API lists the service with `reason: "store_format"` and refuses its owner's start with the error `service`, `reason: "store_format"`. The file is not touched: its header is read before it is opened |

```sql
PRAGMA auto_vacuum = INCREMENTAL;        -- before any table exists
PRAGMA journal_mode = WAL;
BEGIN;
CREATE TABLE entries (
  key   BLOB PRIMARY KEY NOT NULL,       -- the key's UTF-8 bytes, 1 to 256
  value BLOB NOT NULL                    -- at most 4 MiB
) WITHOUT ROWID;
CREATE TABLE meta (
  one   INTEGER PRIMARY KEY CHECK (one = 1),   -- exactly one row
  bytes INTEGER NOT NULL CHECK (bytes >= 0),   -- sum of length(key) + length(value)
  keys  INTEGER NOT NULL CHECK (keys >= 0)     -- count of entries
);
INSERT INTO meta (one, bytes, keys) VALUES (1, 0, 0);
PRAGMA user_version = 1;
COMMIT;
```

Each connection then sets `PRAGMA synchronous = FULL`, and on macOS
`PRAGMA checkpoint_fullfsync = ON` and `PRAGMA fullfsync = ON`. A `set` or
`delete` changes `entries` and the one `meta` row in one transaction.

**Durable: what a reply to `set` (or `delete`) guarantees on macOS.** The
reply is sent after the commit returned, and the commit returns after the
write-ahead log holding the transaction was synced with `F_FULLFSYNC`, which
asks the drive to write its cache to stable storage (`fsync` alone hands the
data to the drive and leaves it in the drive's cache).

| After the reply | The `set` is |
| --- | --- |
| **the service's process or Hub's process ends**, however (a crash, `SIGKILL`) | kept: the transaction is in the write-ahead log in the operating system's hands, and the next open replays it. **Tested**: 1,000 `SIGKILL`s of a process serving the host connection in the middle of a burst of sets; every acknowledged one was there, every one in flight whole or absent, `meta` equal to the entries |
| **the operating system crashes** (a kernel panic) | kept: the data reached the drive before the reply. By SQLite's documentation of `synchronous = FULL` in WAL mode and of `fullfsync`, not by test |
| **the power fails** | kept, as far as the drive honours `F_FULLFSYNC`, which the internal drives of Macs do. By SQLite's and Apple's documentation (`fcntl(2)`, `F_FULLFSYNC`), not by test: a `SIGKILL` cannot show a power loss |

A `set` replaced by the same bytes writes nothing and syncs nothing; it is
kept as well. On Linux `fsync` flushes the drive's cache itself, and the two
macOS pragmas do not apply. **The cost, measured** on an Apple-silicon Mac
(2,000 sets each): a 16-byte `set` takes a median 0.07 ms with `fsync` and
5.4 ms with `F_FULLFSYNC` (99th percentile 0.44 ms and 12.0 ms); a 64 KiB one
0.15 ms and 6.3 ms (0.72 ms and 17.8 ms). The store holds sign-in state,
written rarely: it pays the 5 ms. Values are not
encrypted beyond the file's mode. **The service's process cannot read the
file** (the enclosure, §2); other users cannot either; a program the owner
runs can.
