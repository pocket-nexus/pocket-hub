# Changes to the existing contracts, and the order of work

Proposal (see `README.md`). What changes in `docs/CONTROL-API.md` (§1) and
`docs/DAEMON-PROTOCOL.md` (§2), read at `1bc2074`, as diffs in words, and an
order of work (§3) that keeps the tests working after each step, says what
each device runs at each step, and says what **an existing home and a resident
core of Pocket Hub 0.2.1** meet, since Pocket Hub is released.

## 1. `docs/CONTROL-API.md`

**§1 Hello.** `protocol` becomes **2** with the renamed methods. A core
refuses a client that says 1 with `-32602` and a sentence; a client that is
refused by a core that speaks 1 asks it to quit (below).

**§1 Who may call what** (new, `TRUST.md` §2):

- On each accept the core takes the peer's process id (`LOCAL_PEERPID`). **A
  connection from a process the core started as a service, or from one of
  its descendants, is refused** before `hub.hello`.
- **Consent-class methods are taken from Pocket Hub's app alone**: the peer's
  code signature carries `pocket-hubd`'s team identifier and the app's own
  signing identifier, which the command line in the same bundle does not
  carry. Decided for `services.consent` and `consent` on an install; proposed
  for `services.grantOrigin`. Another client gets the error kind `denied`.
  **A client of kind `remote` never gets them**, and a `pocket-hub:` link
  never calls them (`docs/DEEP-LINKS.md`).
- "A client on the local socket may call every method" becomes "every method
  but the consent-class ones".

**§1 Errors**, the `kind` table:

| Row | Change |
| --- | --- |
| `daemon` | renamed **`service`**. `data` gains `reason?`: `manifest` (the field in `detail`), `check` (the source check's first lines), `runtime` (Bun could not be fetched), `enclosure` (the enclosure could not be set up; nothing runs unenclosed), `earlier` (a package installed by an earlier Pocket Hub, kept and not run), and the service's own `unknown_setup` or `invalid_setup` for `services.setup` |
| `store` | `reason` gains `key` (another key than the one pinned); `rejected` covers the checks of `TRUST.md` §6 |
| new: **`consent`** | the release runs its own programs and the owner has not agreed; `data`: `{id, release}` |
| new: **`denied`** | a consent-class method from a client that is not the app |
| `not_found` | "no such device, service, app, request or file" |

**§2 Objects.**

- **`Daemon` becomes `Service`**, one installed release: `id`, `release` (the
  applet package's footer hash, or `dev`), `version`, `signed`, `requires`,
  `hosts`, `unrestricted`, `consented`, `setup` (whether a guide is present),
  `origins` (granted), `store: {bytes, maxBytes}`, and `state`, `pid`,
  `since`, `channels`, `lastExit`, `error` as today.
- **`StoreApp`**: `services` removed; `hosts` stays; new `service: {requires,
  hosts, unrestricted, setup} | null` from the signed members.
- **`Device.channels`**: each entry gains `service: "applet" | "system"`.
- **`Device.kind` and `Device.supports`** (`3ds`, `tab5`; `push`,
  `screenshot`, `logs`, `program`) are unchanged: channels are opened by the
  device and need no entry in `supports` (`DEVICES.md` §2).

**§3 Methods.**

| Today | Proposed |
| --- | --- |
| `daemons.list` | `services.list` → `{services: [Service]}` |
| `daemons.install {path}` | **`services.install {path, consent?}`** → `Service`: a development release (`TRUST.md` §8); from identified clients if Q5 is accepted. Progress `{stage}`: `runtime`, `checking`, `installing` |
| `daemons.remove {id, purge?}` | `services.remove {id, release?, purge?}`; `purge` deletes the stores, the native folder, the granted origins, the key pin and an earlier version's data folder |
| `daemons.start`, `.stop`, `.restart {id}` | `services.start`, `.stop`, `.restart {id, release?}` |
| `daemons.logs`, `daemons.runtime` | `services.logs`, `services.runtime` |
| `store.install {device, app}` | **`store.install {device, app, consent?}`**: a release with a service gets its service before the applet is pushed; `consent: "unrestricted"` (consent-class) for one that requires `native`, else the error `consent` and nothing installed. Phases `downloading`, `service`, then `devices.push`'s. For a family whose `supports` has no `push` (the Tab5), the device installs from its own store page and Hub installs the service when the device's `install` asks for the applet (Q20) |
| new | **`services.consent {id, release}`** (consent-class): the owner allows an unrestricted release that waits |
| new | **`services.setupGuide {id, release?}`** → `{path, release}` (`SETUP.md` §1) |
| new | **`services.setup {id, message, release?}`** → `{reply}` (`SETUP.md` §2); not from a service |
| new | **`services.requestOrigin {id, origin}`**, **`services.grantOrigin {id, origin}`** (consent-class, Q4), **`services.revokeOrigin {id, origin}`**, **`services.origins {id}`** (`SETUP.md` §3) |
| `pairing.list`, `pairing.answer`, `pairing.decline` | unchanged (`QUESTIONS.md` Q5 on `pairing.answer`) |

The command line gains `pocket-hub service guide <id>`, `pocket-hub service
setup <id> <file|->`, `pocket-hub service origin <id> <origin>` (a request,
never a grant). Consent has one shape everywhere: `consent: "unrestricted"`
on an install, or `services.consent` for a release already installed.

**`net.http` from a device** (Q11): a request without `hash` gets a sentence
saying to update the 3DS's software; one whose package is not in the index,
a sentence saying to install the applet from this Mac.

**§4 Events.** `daemon.changed` → `service.changed {service}`;
`daemon.removed` → `service.removed {id, release}`; `daemon.log` →
`service.log {id, release, at, stream, text}`; new `service.originRequested
{id, origin, applet}`. The overflow rule names `service.changed` beside
`device.changed`, `pairing.changed` and `store.changed`.

## 2. `docs/DAEMON-PROTOCOL.md` becomes `docs/SERVICE-PROTOCOL.md`

| Section today | What it becomes |
| --- | --- |
| intro | "A service is the code of one applet that runs on a service host"; the parts: `sdk/service`, the host (`crates/pocket-host`), the relay; "a service gets no device operation" with its mechanism (`TRUST.md` §2) |
| §1 The daemon package | **The service package**: `MANIFEST.md` §1 to §3, the setup guide, the folders of `HOST-PROTOCOL.md` §8 |
| §2 The process | the bootstrap, the variables, the environments, **the enclosure** (`TRUST.md` §5) |
| §3 Frames | the host connection's payload limit and id ranges |
| §4 The control connection | renamed messages, `protocol` 2, `service.ready`'s `release`, `host` and `files`, no `data` |
| §5 A channel connection | unchanged but for the close reason `service` |
| new §6 The host connection | `HOST-PROTOCOL.md` §4 to §7, with the `setup` request |
| new §7 Capabilities | `CAPABILITIES.md` §3 to §10 |

`AGENTS.md` names `docs/SERVICE-PROTOCOL.md` under "What decides a question";
"A daemon gets no device operation" becomes "A service gets no device
operation".

## 3. Order of work

Each step is one pull request that leaves its repository's tree working.
"Strands" names a device that loses something at that step.

| Step | Repository | Change | Existing home, older core | 3DS | Tab5 | Omarchy Pocket |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | here, PocketJS | these documents and the specification proposal | | as today | as today: links over Pocket Link 2, pairs by a code; its applets' `companion.pocket` requests reach Hub as `net.http` and are answered `unavailable` | as today: omarchy-pocketd answers `companion.pocket` |
| 1 | here | **local rename**: crate, SDK, manifest, variables, handshake (`protocol` 2), control API (`protocol` 2), app, command line. Wire names stay | **the first core moves `<home>/daemons/` to `services/`, `daemon-data/` to `service-data/` and drops `daemon.sock`, once, with a log line**; no format changes in this step, so an installed daemon runs as before. **An app of this step that meets a resident 0.2.1 core** (refused at `hub.hello`) asks it to quit with a protocol-1 `hub.shutdown`, waits for its socket to go, and starts its own core; an older app meeting a newer core is refused with a sentence to update | as before | as before | as before |
| 2 | here | **the host parts into `crates/pocket-host`**; the manifest's new fields, releases, the bootstrap; **the host connection with `store`**; **the macOS enclosure** and **the peer check** (`TRUST.md` §2 (a), (b)); the data folder no longer given; the examples rewritten | **a package installed under the old manifest is listed `failed`, reason `earlier`** ("installed by an earlier Pocket Hub; install it again") and kept until removed; `service-data/<id>/` is kept and never read as a store; `services.remove` with `purge` deletes it | services without capabilities behave as daemons today | as before | as before |
| 3 | here | **`net` for services**, the destination policy keyed by service id | | as before | as before | as before |
| — | PocketJS | the specification's implementation: `device.accepts`, the system channel, the local transport, `hash` in `net.http`, the wire renames, the framework modules, the 3DS glue, the reference, the vectors; **a 3DS program release** | | | | |
| 4 | here | **pin moved** with that release; the wire renames; the system service in-process; **`image`, `text`, `media`**; the media listener | | **an older program strands**: one channel, no system service; Pocket Hub offers the update (Q39) | as before: its firmware opens no channel | as before |
| 5 | here | **consent-class methods and the identified app**, **the setup inlet and guide location**, **granted origins**, **`native` for development installs** | | can be set up for a portable service | as before | as before |
| — | pocket-m5 | the channel groups, the channel namespaces, `device.accepts`, `applet.hosts` in its HTTP node (`DEVICES.md` §8); **a firmware release, flashed over USB by the owner** | | | **runs channels once flashed**; a Tab5 not flashed keeps today's firmware and gets nothing new | |
| 6 | applets | **the pilots** (GitHub, X, YouTube), installed on the Mac as development releases; **not published** | | runs them | runs them once flashed | would strand if published: the store keeps today's versions |
| 7 | here | **`applet.hosts` for `net.http`**: the package index, `hash` required (Q11) | `<home>/packages.json` is built from the store cache at first start | older programs are told to update | the Tab5's HTTP is its own (Q10) | |
| 8 | omarchy-pocket, here | **Omarchy Pocket's host**: `pocket-hostd` for Linux aarch64, the enclosure measured per device, the local transport; in omarchy-pocket the device program's local connections, `data.fs`, the host contract, the Store applet as the host's client; omarchy-pocketd drops its companion services | | | | **runs the pilots on the device**; X cannot be set up there yet (no browser in its shell) |
| 9 | applets, then here and omarchy-pocket | **signed releases** with services and guides; `consent`; `applet.services` leaves the applet contract; **the rewritten applets are published** | | installs them | installs them once flashed | installs them |
| 10 | here, omarchy-pocket | **the source check** at install | | | | |
| 11 | applets, omarchy-pocket | the remaining applets; Spotify as an unrestricted service on Omarchy Pocket | | | | |

**The strands**: a 3DS on an older program at step 4 (an update over the link
mends it); **a Tab5 until its owner flashes a firmware with channels**, since
Hub cannot install its software (`Device.supports` has no `program`; Q42);
Omarchy Pocket never, because the rewritten applets are published only at
step 9, after its host exists. **An existing home** loses nothing silently:
installed daemons run through step 1, are listed and kept from step 2, and
their data folders stay until the owner removes them.

The direction's order ((2) capabilities, (3) pilots, (4) sandbox and
signatures, (5) the rest) is kept, with the enclosure and the control
socket's closing moved to step 2, before `store` holds anything, and the
second service host built before anything is published.
