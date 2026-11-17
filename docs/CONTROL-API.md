# The control API

`pocket-hubd` is Pocket Hub's resident core: it owns every device connection,
the services and the store of paired devices. **Everything else is a client of
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

**The protocol is 2.** Version 1 is the API of Pocket Hub 0.2.2 and earlier,
before the methods, objects and events of services took their names (§3, §4).
A client that says 1 is refused with `-32602` and one sentence that names both
versions, and a core of version 1 refuses a client that says 2 the same way;
neither side reads the other's names.

```
→ {"jsonrpc":"2.0","id":1,"method":"hub.hello","params":{"protocol":2,"client":{"kind":"cli","name":"pocket-hub","version":"0.1.0"}}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":2,"hub":{"id":"fedcba9876543210","name":"Studio Mac","version":"0.1.0"}}}
```

**`hub.hello` and `hub.shutdown` are frozen.** Every protocol version, past and
future, keeps these two methods with these shapes: `hub.hello` takes
`{protocol, client: {kind, name, version}}` and answers `{protocol, hub: {id,
name, version}}`, and `hub.shutdown` takes `{}` and answers `{}` (§3). So a
client of one version can always learn which core it has met and ask a core of
another version to stop. **No other method is ever called in another
version's dialect.**

**A core that refuses a hello for its protocol says its own**: from protocol 2
on, the `-32602` error carries `data: {protocol}`, the version the core speaks.
A core of Pocket Hub 0.2.2 and earlier sends no `data`; a refusal without it
means protocol 1, the only version before the field existed. A client reads the
number, never the sentence.

```
→ {"jsonrpc":"2.0","id":1,"method":"hub.hello","params":{"protocol":1,"client":{"kind":"gui","name":"Pocket Hub","version":"0.2.2"}}}
← {"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"This Hub speaks protocol 2, not 1.","data":{"protocol":2}}}
```

`client.kind` is `gui`, `cli` or `remote`. **The client's kind and name are its
identity**: they label the device mutex's holder (§3) and are where a remote
client's limits will attach. A client on the local socket may call every
method.

### Starting and stopping the core

`pocket-hubd` with no argument is the resident core: it stays in the
foreground of whatever started it, so a client that starts it detaches it (its
own session, no terminal). **The core also ends at `SIGINT` and `SIGTERM`**,
shutting down as it does for `hub.shutdown`, so a client starts it with no
signal blocked: a process inherits the signal mask of the thread that starts
it, and a core started with `SIGTERM` blocked never sees one. **One core runs
per home**: a second `pocket-hubd`
that finds a live one answering on the socket exits with status 0 at once, so
two clients starting it together are both served by the one that won. The
answer to `hub.shutdown` is written before the socket closes; **the core is
gone when the client's connection has ended and `hubd.sock` no longer exists**,
which is what a client that has to know waits for.

**A resident core outlives the app**, so an updated app meets the core of the
version it replaced. **The app takes over from an older core, and only from
an older one**, when it connects:

- When the core speaks a lower protocol (its refusal's `data.protocol`, or 1
  without it), or the same protocol and its `hub.version` orders below the
  version of the core the app carries (as its `pocket-hubd --version` says it;
  SemVer precedence), the app says hello in the core's protocol, calls
  `hub.shutdown`, waits until the core is gone (above) for up to 20 s, and
  starts its own core. Equal versions: nothing happens.
- **A newer core is never replaced.** One that speaks a higher protocol is
  left running and the app tells the owner that a newer Pocket Hub is already
  running on this Mac and this copy is older; one of the same protocol and a
  higher version is used as it is, since it speaks this app's protocol. Two
  installed copies therefore never take turns ending each other's core: the
  newest one's runs.
- When the older core is not gone after 20 s, the app says so and does nothing
  else (no signal, no removing the socket), until the owner asks it to try
  again. When the app has no core of its own to start, it replaces nothing.
- **Quit and Stop stops whatever core is there**, older or newer, through the
  two frozen methods.

**The command line never replaces a core.** Against a core of another protocol
it says so in one line and does nothing; `pocket-hub shutdown` stops a core of
any version through the two frozen methods.

`hub.shutdown` does not wait for a device operation: the core closes each link
with `going_away` as soon as its answer is written. A push, an install or an
update of the 3DS's software under way then ends unfinished on the device,
which keeps what it had (it writes into a staging file and commits only a
whole, checked transfer: link specification §6.2 `runtime.push` and
`runtime.native.install`, §6.4 `install`) and links again to the core that
follows (§2.4).

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
  overflows, Hub sends `device.changed`, `service.changed`, `pairing.changed`
  and `store.changed` for everything again; only `device.log` and `service.log`
  lines are lost.
- A client that disconnects cancels its open calls and releases what it held.

### Errors

`error.code` is JSON-RPC's for a malformed call (`-32600`, `-32601`, `-32602`;
of these only a hello refused for its protocol has `data`, `{protocol}`, Hello
above) and **`1`** for every failure of the operation itself, whose
`error.data.kind` says which:

| `kind` | Meaning | More in `data` |
| --- | --- | --- |
| `not_found` | no such device, service, app or file | |
| `not_linked` | the device is paired and not connected now | |
| `busy` | the device is held by a state-changing operation | `holder` (below) |
| `link` | the device answered with an error, or the link failed | `code` (the link's error code, or `closed`, `timeout`; `unsupported` also for an operation the device's `supports` does not name, §2), `phase` and `reason` when the device gave them (`devices.remove`: `reason` is `running` or `home`) |
| `cancelled` | the client cancelled the call | |
| `setup` | a setup step failed | `step`: the `stage` it failed in (§3 Setup). `reason?`: `unreachable` (the console did not answer), `companions_full` (the device already holds 8 computers), `release_unavailable` (the program could not be fetched), `storage` (a read or a write on the device failed), `card` (the folder is not a card of that device). `detail?`: the failing step in the setup's own words, for a log. `device?` and `confirmed: false` when a key was stored for the device and its final readback did not return: that device is then listed with `setupUnconfirmed` |
| `invalid` | an argument is unusable (not a `.pocket`, a bad name) | |
| `service` | a service could not be installed, started or stopped | `id?` |
| `release` | the release of a device's program could not be used | `reason`: `unreachable` (the release source did not answer, or not with a release), `not_found` (no such tag), `rejected` (the file fails its checksum or is not a program of that device) |
| `store` | the applet store could not be used | `reason`: `unreachable` (no answer, or not the store's answer), `rejected` (a downloaded file does not match its hash, its publisher's signature does not verify, or the package is not what its catalog entry says) |
| `pairing` | a pairing by a code did not complete (§3 Pairing) | `reason`: `wrong_code` (the device did not take the code: it is not the one it shows), `bad_proof` (the device did not prove that it holds the key Hub sent), `gone` (the device left, or its request ended, before it answered). Nothing was stored in any of them |
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
| `kind` | string | the device family: `3ds` (a Nintendo 3DS) or `tab5` (an M5Stack Tab5); empty for a device of a family Hub does not know. A setup writes it (§3 Setup); a pairing by a code takes it from the device's target (§3 Pairing) |
| `supports` | array | what Hub can ask of a device of this family, linked or not: `push` (`devices.push`, `store.install`), `screenshot` (`devices.screenshot`), `logs` (`devices.logs`), `program` (`devices.installProgram`). `3ds`: all four. `tab5`: none; its applets come from its own store page, which Hub serves, and its software is not installed by Hub. A family Hub does not know: all but `program`. **A client offers an operation only when this list names it**; a call of one it does not name is the error `link` with `code` `unsupported`, and nothing is sent to the device. A link's feature list cannot say this: one group covers several requests (link specification §6.1) |
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
| `program` | object | the device's own program: `{version, latest, update}`. `version` is what the device reports for it (the `home` row of `apps`), null before the first report or when the device reports none. `latest` is the newest release Hub knows, null until Hub has looked and always null for a device whose `supports` lacks `program`: it looks when a device links, unless it began a look less than 6 hours before (a failed one included), and a device that stays linked is not looked at again until it links again; a failed look leaves the value as it was. `update` is true when `latest` orders above `version`. Pocket Shell's versions were dates up to `2026.11.12` and are SemVer after it, and **every dated version orders below every SemVer one**: a date (three dotted numbers whose first is a year, 2000 or more: `2026.11.11`) compares part by part as numbers, a SemVer version (`0.1.1`, `0.3.0-rc.1`) by SemVer precedence; a version in any other form is never older or newer |
| `channels` | array | open applet channels: `{app, generation, since}` |

**PairRequest** — one device that asks to be paired by the code it shows
(§3 Pairing) and waits for that code:

| Field | Type | Content |
| --- | --- | --- |
| `device` | string | the device id it names. **Nothing has proved it yet**: the id is the request's name, not an identity |
| `kind` | string | the family of its target, as in a Device; empty for one Hub does not know |
| `name`, `model`, `target` | string | as the device says them |
| `address` | string | where the request came from: the device's IP address on a network carrier |
| `since` | integer | when Hub read the request |
| `expires` | integer | when Hub closes it: 5 minutes after `since`, the time the device keeps its code |

**Service** — one installed service:

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
| `devices` | array or null | the devices the package was built for, in the store's order: `{target, name}` as `StoreDetails` names them. Null until Hub holds the details of this version: it reads every app's after each catalog fetch, and `store.changed` follows when they change |
| `sample` | boolean | the store has sample data for this version: the store's player runs it on that data (`store.sample`) |
| `bytes` | integer | the package's size |
| `icon` | string or null | absolute path of the applet's icon (a PNG) in Hub's cache; null until Hub has fetched it |
| `cached` | boolean | Hub holds the verified package. A device's own store page lists an applet only once this is true |
| `services`, `hosts` | array | what the applet declares it reaches: the `companion.pocket` services of the link specification (§11), which are not Hub's services, and hosts |
| `installed` | object or null | only in an answer for a device (absent without `device`): `{version, hash}` of the copy that device holds, null when it holds none |
| `update` | boolean | only in an answer for a device: the store's version orders above the version the device holds (`major.minor.patch`). An equal version is no update whatever its hash |

**StoreDetails** — what an applet's package says about the applet, as the
store reads it out of the package's manifest and directory (the store's
`GET /v1/details/<sha256>`). No client reads a manifest of its own. The
publisher's signature does not cover them: they are the store's reading.

| Field | Type | Content |
| --- | --- | --- |
| `id`, `version` | string | the applet, and the version these are of: the newest listed, the one `store.apps` gives |
| `devices` | array | the devices the package was built for, in the store's order: `{target, name, hostAbi}`. `name` is the store's name for the device (`Nintendo 3DS`, `Omarchy Pocket`), the same in every language; the target itself for a device the store has no name for. `hostAbi`: an integer, or null when the store names no build |
| `needs` | array | the capabilities it needs of a device, by id (PocketJS `contracts/spec/applet.ts`): `input.buttons`, `net.http`, … |
| `uses` | array | the capabilities it uses where a device has them |
| `surface` | object or null | the surface it lays itself out for, null when the package names none: `{touch, size}`. `touch` true: a touch surface of its own, not the device's screen. `size`: `{min: [width, height], max: [width, height]}` in points, or null |
| `framework` | string or null | what its interface is written with (`solid`) |
| `api` | integer or null | the version of the applet API it is written for |

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
| `hub.shutdown` | | `{}`. Hub closes its links with `going_away`, stops its services and exits |

### Devices

| Method | Params | Result | Mutex |
| --- | --- | --- | --- |
| `devices.list` | | `{devices: [Device]}` | |
| `devices.get` | `{device}` | `Device` | |
| `devices.forget` | `{device}` | `{}`. The record is deleted and the link closed with `forgotten`: `device.unlinked` with that code comes before `device.removed` | |
| `devices.status` | `{device}` | the device's fresh `runtime.status` | |
| `devices.push` | `{device, path}` | `{app: {id, output, title}, hash, generation}`. Installs a `.pocket`, or replaces the running guest when it is that app. Progress: `{phase, done, total}` | yes |
| `devices.launch` | `{device, app}` (an installed applet's output name or id) | `{output}` | yes |
| `devices.remove` | `{device, app}` (an installed applet's id or output name) | `{}`. **Deletes the applet and its data on the device** (link specification §6.2 `runtime.remove`); the device's next `runtime.apps` report no longer lists it, and `device.changed` follows. Only a device whose `link.features` lists `runtime.remove` can do it; one whose program predates it fails with `link` `unsupported`. The device refuses the running applet and its home screen: `link` with `code: rejected` and `reason` `running` or `home`. An applet the device does not hold is `not_found`; an install of it in progress is `link` `busy` | yes |
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

The first install on a device, outside the link (link specification §3.2),
for a device whose storage Hub reaches. `kind` is `3ds`, the one family set up
this way; a device that shows a code is paired on the link instead (Pairing,
below).

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

### Pairing

A device that has a screen and no storage Hub can write through is paired on
the link, by a code it shows (link specification §3.11): **the device's owner
opens a pairing on the device and chooses this computer there, and Hub holds
the device's request until its owner enters the 6 digits the device shows.**
An M5Stack Tab5 pairs this way. No setup comes before it and no software is
installed: the pairing it leaves is the one a setup leaves, and the device
links by itself from then on.

| Method | Params | Result |
| --- | --- | --- |
| `pairing.list` | | `{requests: [PairRequest]}`: the requests that wait for a code, oldest first |
| `pairing.answer` | `{device, code}` | `Device`: the device, now paired. `code` is the 6 digits the device shows, as ASCII digits. Hub mints a link key and sends it to the device under the code; the answer comes when the device has proved that it holds the key, and **the device's record is stored then and not before**. A `code` that is not 6 digits is `invalid`; no request of that device waiting for a code is `not_found`; anything else is the error `pairing` |
| `pairing.decline` | `{device}` | `{}`. The device is told that this computer's owner dismissed the request (the link's refusal `declined`). No request of that device waiting for a code is `not_found` |

`device` is the device id of a request, or `"*"` when exactly one request
waits: with none `"*"` is `not_found`, with several it is `invalid`.

- **Hub keeps one request for a device id**, a later one replacing the one
  before, and closes a request 5 minutes after it came. A request leaves the
  list when it is answered, declined, replaced or closed, and when the device
  goes away; `pairing.changed` carries the list after each change.
- **A wrong code stores nothing.** The error is `pairing` with `reason`
  `wrong_code`, and a pairing Hub already held for that device id stays as it
  was. The device takes 3 wrong codes before it drops its code; while it has
  tries left it asks again within a second, which is a new request in the
  list.
- **While a code is on its way to the device the request is not in the
  list**: a second `pairing.answer` for it is `not_found`.
- After a pairing, `device.changed` carries the device (`kind` from its
  target, `name` and `model` as its request said them), and again when it
  links.
- **Hub sets no device up this way and has no program to install on it**:
  `setup.discover`, `setup.install` and `setup.card` take the kind `3ds`
  alone.

### Services

| Method | Params | Result |
| --- | --- | --- |
| `services.list` | | `{services: [Service]}` |
| `services.install` | `{path}` (a folder or a `.tar.gz`) | `Service`. Progress: `{stage, message}`; `stage` is `runtime` while Hub downloads Bun for it, then `installing` |
| `services.remove` | `{id, purge?}` | `{}`. The service's data folder stays unless `purge` is true |
| `services.start`, `services.stop`, `services.restart` | `{id}` | `Service` |
| `services.logs` | `{id, tail?}` | `{lines: [{at, stream, text}]}`; `tail` defaults to 200 |
| `services.runtime` | `{install?}` | `{version, installed, path}`: the Bun that Hub runs services on. With `install: true` Hub downloads Bun now. Progress: `{done, total}` |

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
| `store.details` | `{app}` | `StoreDetails` of the applet's newest listed version. Hub asks the store for the details of the package its catalog names (fetched again past 10 minutes, as for `store.apps`), keeps them, and reads them again after each catalog fetch; a read that fails drops them. An applet the catalog does not list is `not_found`; a store that does not answer them is the error `store` | |
| `store.sample` | `{app}` | `{package, mock}`: absolute paths in Hub's cache of the applet's verified package (every variant) and of its sample data, a JSON document checked against the SHA-256 the catalog names. What the store's player needs to run the applet on a computer, on that data alone (pocket-nexus/applets `store/play/player.ts`). Hub downloads what its cache lacks. Progress: `{phase, done, total}`, `phase` being `downloading` for the package. An applet the catalog does not list, or with no sample data (`sample` false), is `not_found`. The store's reviewer adds sample data: the publisher's signature does not cover it | |
| `store.install` | `{device, app}` | as `devices.push`. Hub downloads the applet's package when its cache lacks it, checks it, and installs it on the device; an applet with no variant for the device's target is `not_found`. Progress: `{phase, done, total}`, `phase` being `downloading` (once, with `done` equal to `total`, when the package was cached) and then those of `devices.push`. The device is held from the start, the download included, with the method `store.install` | yes |

**Hub fetches the packages of its paired devices' targets ahead of need**, one
at a time after each catalog fetch, because a device's catalog row needs the
package's own hash and the store has no route that gives it. Until a package is
there (`cached`), the device's store page does not list that applet; the
computer's does.

The store carries applet packages alone today. An applet's service is not part
of what `store.install` fetches until the store publishes services.

## 4. Events

| Topic | Members | When |
| --- | --- | --- |
| `device.changed` | `device: Device` | anything in a Device changed: linked, unlinked, status, apps, busy, channels |
| `device.removed` | `id` | forgotten |
| `device.unlinked` | `id`, `code`, `reason` | the link ended, with the link's close code |
| `device.log` | `id`, `level`, `source`, `text`, `frame`, `dropped?` | a log line while logs are on |
| `device.screenshot` | `id`, `path`, `width`, `height`, `frame` | the device sent a screenshot unasked |
| `pairing.changed` | `requests: [PairRequest]` | the requests that wait for a code changed (§3 Pairing): the whole list, as `pairing.list` answers it |
| `service.changed` | `service: Service` | a service was installed or changed state |
| `service.removed` | `id` | |
| `service.log` | `id`, `at`, `stream`, `text` | a line of a service's output |
| `hub.changed` | `hub: {id, name, version}` | the name changed |
| `store.changed` | `fetchedAt` | the catalog Hub holds, or what it has cached of it (packages, icons), changed; a client calls `store.apps` again |

`device.changed` carries the whole Device, so a client that subscribes and then
calls `devices.list` holds the truth without merging fields.
