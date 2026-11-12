# The control API

`pocket-hubd` is Pocket Hub's resident core: it owns every device connection,
the daemons and the store of paired devices. **Everything else is a client of
this API**: the SwiftUI app, the `pocket-hub` command line, and later a relay
that lets Pocket Studio's agents reach a device. One surface for all of them
means a client can do nothing the others cannot, and a device is never held by
two processes.

## 1. Transport

- A Unix domain socket at `<home>/hubd.sock`, mode 0600. `<home>` is
  `$POCKET_HUB_HOME`, else `~/Library/Application Support/nexus.pocket.hub` on
  macOS and `$XDG_STATE_HOME/pocket-hub` (else `~/.local/state/pocket-hub`)
  elsewhere. **The folder is this Hub's alone**: `~/Library/Application
  Support/Pocket Hub` and `~/Library/Logs/Pocket Hub` belong to the earlier
  Pocket Hub, which may be installed beside this one, and nothing here reads or
  writes them. Hub's own log is `<home>/logs/hubd.log`.
- **A home whose `hubd.sock` path is longer than 100 bytes is refused**: a
  socket address cannot hold it, and the core says so and does not start. There
  is no second location for a client to look in.
- **Hub writes one connection in order, and an answer reflects every event
  written before it.** A client that subscribes and then lists therefore drops
  the state events it read before the list's answer, and applies those after.
- **JSON-RPC 2.0, one object per line**: UTF-8, no line breaks inside an
  object, each ended by `\n`. No batches.
- Field names are lower camel case. Times are epoch milliseconds. Ids and
  hashes are the lowercase hex strings the link uses. **No link key ever
  appears in an answer or an event.**
- Bytes do not travel in this version: a client and `pocket-hubd` share a file
  system, so a package to push or a screenshot to keep is named by an absolute
  path. A remote client will need a blob transfer; the methods below take
  `path` in an object so a `blob` member can stand beside it later.

### Hello

A client's first request is `hub.hello`; any other request before it is
answered with the error `-32600` and the connection stays open, and a
notification before it is ignored. A second `hello` is `-32600`; another
`protocol` or an unknown `client.kind` is `-32602`. A line that is not JSON is
answered `-32700`; a line over 1 MiB is answered `-32600` and the connection is
closed.

```
→ {"jsonrpc":"2.0","id":1,"method":"hub.hello","params":{"protocol":1,"client":{"kind":"cli","name":"pocket-hub","version":"0.1.0"}}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":1,"hub":{"id":"fedcba9876543210","name":"Studio Mac","version":"0.1.0"}}}
```

`client.kind` is `gui`, `cli` or `remote`. **The client's kind and name are its
identity**: they label the device mutex's holder (§3) and are where a remote
client's limits will attach. A client on the local socket may call every
method.

### Starting and stopping the core

`pocket-hubd` with no argument is the resident core: it stays in the
foreground of whatever started it, so a client that starts it detaches it (its
own session, no terminal). **One core runs per home**: a second `pocket-hubd`
that finds a live one answering on the socket exits with status 0 at once, so
two clients starting it together are both served by the one that won. The
answer to `hub.shutdown` is written before the socket closes; **the core is
gone when the client's connection has ended and `hubd.sock` no longer exists**,
which is what a client that has to know waits for.

**A core that cannot start says why and exits; it does not fall back.** When
the link port (TCP 8628) is taken, the core does not move to another port: it
writes one line that names the port and exits with a status other than 0. The
same holds for a home it cannot use. The line goes to standard error and to
`<home>/logs/hubd.log`, and a client that started the core shows it to the
owner. A port other than 8628 is used only when the core is started with one
on purpose (`--link-port`).

### Progress, events and cancelling

- **A long call answers when it ends.** While it runs, Hub sends notifications
  `{"jsonrpc":"2.0","method":"progress","params":{"id":<the request's id>, …}}`
  with the members the method defines.
- `{"jsonrpc":"2.0","method":"cancel","params":{"id":<a request's id>}}` asks
  Hub to stop that call; the call then ends with the error `cancelled`. A
  `cancel` for an id that names no open call is ignored.
- After `hub.subscribe`, Hub sends `{"jsonrpc":"2.0","method":"event","params":
  {"topic":<topic>, …}}` (§4). A later `hub.subscribe` replaces the topic list.
  **A client that reads too slowly loses no state**: when its event queue
  overflows, Hub sends `device.changed`, `daemon.changed` and `store.changed`
  for everything again; only `device.log` and `daemon.log` lines are lost.
- A client that disconnects cancels its open calls and releases what it held.

### Errors

`error.code` is JSON-RPC's for a malformed call (`-32600`, `-32601`, `-32602`)
and **`1`** for every failure of the operation itself, whose `error.data.kind`
says which:

| `kind` | Meaning | More in `data` |
| --- | --- | --- |
| `not_found` | no such device, daemon, app or file | |
| `not_linked` | the device is paired and not connected now | |
| `busy` | the device is held by a state-changing operation | `holder` (below) |
| `link` | the device answered with an error, or the link failed | `code` (the link's error code, or `closed`, `timeout`), `phase` when the device gave one |
| `cancelled` | the client cancelled the call | |
| `setup` | a setup step failed | `step`: the `stage` it failed in (§3 Setup). `reason?`: `unreachable` (the console did not answer), `companions_full` (the device already holds 8 computers), `release_unavailable` (the program could not be fetched), `storage` (a read or a write on the device failed), `card` (the folder is not a card of that device). `detail?`: the failing step in the setup's own words, for a log. `device?` and `confirmed: false` when a key was stored for the device and its final readback did not return: that device is then listed with `setupUnconfirmed` |
| `invalid` | an argument is unusable (not a `.pocket`, a bad name) | |
| `daemon` | a daemon could not be installed, started or stopped | `id?` |
| `release` | the release of a device's program could not be used | `reason`: `unreachable` (the release source did not answer, or not with a release), `not_found` (no such tag), `rejected` (the file fails its checksum or is not a program of that device) |
| `store` | the applet store could not be used | `reason`: `unreachable` (no answer, or not the store's answer), `rejected` (a downloaded file does not match its hash, its publisher's signature does not verify, or the package is not what its catalog entry says) |
| `internal` | anything else | |

`error.message` is one sentence for the owner, **in English**; so are a
progress `message` and an install report's `next`. A client that speaks another
language words its own text from `kind`, `step`, `reason` and the report's
fields, and shows Hub's sentence as detail.

**A holder** is `{kind, name, method}`: the kind and name the holding client
gave in its hello, and the method that holds the device (`devices.push`). It
is the same object in a Device's `busy` and in the error `busy`.

## 2. Objects

**Device** — one paired device, linked or not:

| Field | Type | Content |
| --- | --- | --- |
| `id` | string | the device id |
| `kind` | string | the device family, `3ds` |
| `name`, `model`, `target` | string | from the last handshake; empty before the first |
| `hostAbi` | integer | |
| `lastAddress` | string or null | |
| `lastSeen` | integer or null | |
| `setupUnconfirmed` | boolean | a setup wrote a key and could not read it back; a link or a new setup settles it |
| `duplicateId` | boolean | two consoles run one card image (link specification §2.4) |
| `link` | object or null | `{carrier, peer, since, payloadLimit, features}`; `carrier` is `tcp`, `usb`, `uart` or `ble`, for display only; `features` are the groups both sides list (link specification §6.1). null when not linked |
| `status` | object or null | the last `runtime.status` (link specification §6.2) |
| `apps` | array or null | the installed applets as the device reports them: `{id, output, version, hash, home?}`. A row has no title; a client shows `output` until a catalog gives it one |
| `busy` | object or null | `{holder, since, launching}` while a state-changing operation holds the device; `holder` as in §1. `launching` is true while the device restarts into a program it was told to start (up to 60 s, until it links again) |
| `program` | object | the device's own program: `{version, latest, update}`. `version` is what the device reports for it (the `home` row of `apps`), null before the first report or when the device reports none. `latest` is the newest release Hub knows, null until Hub has looked: it looks when a device links, unless it began a look less than 6 hours before (a failed one included), and a device that stays linked is not looked at again until it links again; a failed look leaves the value as it was. `update` is true when `latest` orders above `version`: both must be numbers joined by dots (`2026.11.11`), compared part by part; a version in any other form is never older or newer |
| `channels` | array | open applet channels: `{app, generation, since}` |

**Daemon** — one installed daemon:

| Field | Type | Content |
| --- | --- | --- |
| `id` | string | the applet id it serves |
| `version` | string | |
| `state` | string | `stopped`, `starting`, `running`, `stopping`, `failed` |
| `pid` | integer or null | |
| `since` | integer or null | when it entered this state |
| `channels` | integer | channels open on it |
| `lastExit` | object or null | `{code, signal, at}`: `code` an integer or null, `signal` a name such as `SIGTERM` or null |
| `error` | string or null | why it is `failed` |

**StoreApp** — one applet the store offers, at its newest listed version:

| Field | Type | Content |
| --- | --- | --- |
| `id` | string | the applet's manifest id |
| `title`, `summary`, `author`, `version` | string | as published |
| `official` | boolean | published with the store's official key |
| `targets` | array | the device targets the package has a variant for (`3ds-dev`, …) |
| `bytes` | integer | the package's size |
| `icon` | string or null | absolute path of the applet's icon (a PNG) in Hub's cache; null until Hub has fetched it |
| `cached` | boolean | Hub holds the verified package. A device's own store page lists an applet only once this is true |
| `services`, `hosts` | array | the services and hosts the applet declares it reaches |
| `installed` | object or null | only in an answer for a device (absent without `device`): `{version, hash}` of the copy that device holds, null when it holds none |
| `update` | boolean | only in an answer for a device: the store's version orders above the version the device holds (`major.minor.patch`). An equal version is no update whatever its hash |

## 3. Methods

Methods that change a device's state take the device mutex (link specification
§8.3): one at a time per device. A second caller waits up to 10 s and then gets
`busy` with the holder.

### Hub

| Method | Params | Result |
| --- | --- | --- |
| `hub.hello` | `{protocol, client}` | `{protocol, hub: {id, name, version}}` |
| `hub.status` | | `{id, name, version, home, startedAt, carriers: [{kind, port?}], bun: {version, installed}}` |
| `hub.setName` | `{name}` | `{name}`. Devices see it at their next link |
| `hub.subscribe` | `{topics?}` | `{}`. Without `topics`, every topic |
| `hub.shutdown` | | `{}`. Hub closes its links with `going_away`, stops its daemons and exits |

### Devices

| Method | Params | Result | Mutex |
| --- | --- | --- | --- |
| `devices.list` | | `{devices: [Device]}` | |
| `devices.get` | `{device}` | `Device` | |
| `devices.forget` | `{device}` | `{}`. The record is deleted and the link closed with `forgotten`: `device.unlinked` with that code comes before `device.removed` | |
| `devices.status` | `{device}` | the device's fresh `runtime.status` | |
| `devices.push` | `{device, path}` | `{app: {id, output, title}, hash, generation}`. Installs a `.pocket`, or replaces the running guest when it is that app. Progress: `{phase, done, total}` | yes |
| `devices.launch` | `{device, app}` (an installed applet's output name or id) | `{output}` | yes |
| `devices.screenshot` | `{device, path?}` | `{path, width, height, frame}`. A PNG of both screens; without `path` Hub writes into its cache folder, where it keeps the newest 50 files | |
| `devices.logs` | `{device, on}` | `{}`. Lines arrive as `device.log` events. **Hub keeps a device's logs on while at least one connected client has asked for them**: it turns them on again after the device links again, and off when the last such client turns them off or disconnects. A client may ask while the device is not linked; the lines start when it links. Hub keeps no history: a client sees the lines written while it listens | |
| `devices.installProgram` | `{device, program?, launch?}` | `{phase, name, version}`. Replaces the device's own program over the link. `program` is as in Setup (below): `{"release":"latest"}` by default, a release tag, or `{"path": <a local file>}`. **The file always takes the place of the program the device runs** (for `3ds`: `pocket-shell.3dsx`), whatever the local file is called, so an update never leaves a second copy beside the first. `launch` defaults to true: the device restarts into the new program and links again by itself; `Device.busy.launching` is true meanwhile. The device is held from the start, the download included, so a call while it restarts is `busy`, not `not_linked`. An update for `latest` asks the release source and fails with the error `release` when it does not answer; it never sends an older build in its place. An update notes the build it installed for the device as a setup does, so a later setup can tell the program is in place. `name` in the result is the name the program has on the device. Progress: `{phase, done, total}`, `phase` being `downloading` when Hub fetches a release and then those below | yes |

`device` is a device id, or `"*"` when exactly one device is paired: with none
paired `"*"` is `not_found`, with several it is `invalid`.

`devices.push` and `devices.installProgram` report `{phase, done, total}`:
`phase` is `sending` until the device's first event and then the device's own
(`receiving`, `received`, `staged`); `done` and `total` are the bytes Hub has
written. `devices.launch` takes `app` as an id of the device's last apps report
and sends its output name, or else sends `app` as an output name.

### Setup

The first install on a device, outside the link (link specification §3.2).

| Method | Params | Result |
| --- | --- | --- |
| `setup.discover` | `{kind}` | `{candidates: [{address, port, banner, ftpd}]}`. For `3ds`: every host on the local networks that answers like an FTP server on ftpd's port; `ftpd` is true when the banner is ftpd's own, and a client offers those first. It answers within about 2 s |
| `setup.install` | `{kind, host, port?, program?}` | an install report. Progress: `{stage, done?, total?, message}` |
| `setup.card` | `{kind, path, program?}` | an install report, for an SD card mounted at `path` |

`program` is `{"release":"latest"}` (the default), `{"release": <a release tag>}`
or `{"path": <a local file>}`.
`stage` is `connecting`, `reading`, `downloading`, `uploading`, `pairing` or
`done`.

The install report: `{device, pairing, program: {action, version, runtime},
confirmed, next}`.

| Field | Type | Content |
| --- | --- | --- |
| `device` | string | the device id |
| `pairing` | string | `key` (a new key was written), `kept` (the pairing was in place) or `address` (only Hub's address was rewritten) |
| `program.action` | string | `installed`, or `kept` when the device already held it |
| `program.version` | string or null | the program's release version |
| `program.runtime` | string or null | the PocketJS commit the program was built from |
| `confirmed` | boolean | always true in a report: every write was read back |
| `next` | string | one sentence telling the owner what to do on the device |

**A setup that fails is the error `setup`, never a report.** When the failure
came after Hub stored a key for the device (a write whose readback did not
return), the error carries `device` and `confirmed: false`, and the device is
listed with `setupUnconfirmed` until it links or a later setup settles it (link
specification §3.2).

### Daemons

| Method | Params | Result |
| --- | --- | --- |
| `daemons.list` | | `{daemons: [Daemon]}` |
| `daemons.install` | `{path}` (a folder or a `.tar.gz`) | `Daemon`. Progress: `{stage, message}`; `stage` is `runtime` while Hub downloads Bun for it, then `installing` |
| `daemons.remove` | `{id, purge?}` | `{}`. The daemon's data folder stays unless `purge` is true |
| `daemons.start`, `daemons.stop`, `daemons.restart` | `{id}` | `Daemon` |
| `daemons.logs` | `{id, tail?}` | `{lines: [{at, stream, text}]}`; `tail` defaults to 200 |
| `daemons.runtime` | `{install?}` | `{version, installed, path}`: the Bun that Hub runs daemons on. With `install: true` Hub downloads Bun now. Progress: `{done, total}` |

### Store

The applet store (applets.pocket.nexus). Hub is its client: it keeps the
catalog, downloads packages and icons into its cache, **checks each package
against the SHA-256 the store names and the publisher's signature**, and carries
packages to devices. The same catalog answers a device's own store page over
the link (`catalog` and `install`, link specification §6.4), so the owner sees
one offer on the computer and on the device.

| Method | Params | Result | Mutex |
| --- | --- | --- | --- |
| `store.apps` | `{device?, refresh?}` | `{apps: [StoreApp], fetchedAt}`. With `device`: the applets that have a variant for that device's target, each with `installed` and `update`; a device that has never linked has no known target and is offered none. Hub answers from the catalog it holds and fetches it again when it is older than 10 minutes or `refresh` is true. **When that fetch fails the answer is the error `store`, not the older catalog** | |
| `store.install` | `{device, app}` | as `devices.push`. Hub downloads the applet's package when its cache lacks it, checks it, and installs it on the device; an applet with no variant for the device's target is `not_found`. Progress: `{phase, done, total}`, `phase` being `downloading` (once, with `done` equal to `total`, when the package was cached) and then those of `devices.push`. The device is held from the start, the download included, with the method `store.install` | yes |

**Hub fetches the packages of its paired devices' targets ahead of need**, one
at a time after each catalog fetch, because a device's catalog row needs the
package's own hash and the store has no route that gives it. Until a package is
there (`cached`), the device's store page does not list that applet; the
computer's does.

The store carries applet packages alone today. An applet's daemon is not part
of what `store.install` fetches until the store publishes daemons.

## 4. Events

| Topic | Members | When |
| --- | --- | --- |
| `device.changed` | `device: Device` | anything in a Device changed: linked, unlinked, status, apps, busy, channels |
| `device.removed` | `id` | forgotten |
| `device.unlinked` | `id`, `code`, `reason` | the link ended, with the link's close code |
| `device.log` | `id`, `level`, `source`, `text`, `frame`, `dropped?` | a log line while logs are on |
| `device.screenshot` | `id`, `path`, `width`, `height`, `frame` | the device sent a screenshot unasked |
| `daemon.changed` | `daemon: Daemon` | a daemon was installed or changed state |
| `daemon.removed` | `id` | |
| `daemon.log` | `id`, `at`, `stream`, `text` | a line of a daemon's output |
| `hub.changed` | `hub: {id, name, version}` | the name changed |
| `store.changed` | `fetchedAt` | the catalog Hub holds, or what it has cached of it (packages, icons), changed; a client calls `store.apps` again |

`device.changed` carries the whole Device, so a client that subscribes and then
calls `devices.list` holds the truth without merging fields.
