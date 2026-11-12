# Changes to the existing contracts, and the order of work

Proposal (see `README.md`). What changes in `docs/CONTROL-API.md` (§1) and
`docs/DAEMON-PROTOCOL.md` (§2), as diffs in words, and an order of work that
keeps `cargo test --workspace`, the SDK's tests, the app's tests and a real
3DS working after each step (§3). The names are the proposal's of
`README.md` §3; another choice changes the words, not the steps.

## 1. `docs/CONTROL-API.md`

**§1 Transport and hello.** Unchanged.

**§1 Errors**, the `kind` table:

| Row | Change |
| --- | --- |
| `daemon` | renamed **`service`**: "a service could not be installed, started or stopped". `data` gains `reason?`: `manifest` (the manifest breaks `MANIFEST.md`; `detail` names the field), `check` (the checking tool refused it; `detail` holds its first lines), `runtime` (Bun could not be fetched). `id?` stays |
| `store` | `reason` gains `key` (a release signed by another key than the one Hub pinned for the id, `TRUST.md` §7). `rejected` covers the new checks of `TRUST.md` §6 (the second signature, the service package's hash, a manifest that differs from the signed members) |
| new: **`consent`** | "the release runs its own programs on this computer, and the owner has not agreed". `data`: `{id, unrestricted: true}`. A client shows the dialog of `TRUST.md` §4 and calls again with `consent` |
| `not_found` | "no such device, service, app, sign-in or file" |

**§2 Objects.**

- **`Daemon` becomes `Service`**, one installed release:

  | Field | Type | Content |
  | --- | --- | --- |
  | `id` | string | the applet id |
  | `release` | string | the applet package's footer hash, or `dev` (`HOST-PROTOCOL.md` §8) |
  | `version` | string | the release's version (the applet's); `dev` for a development install |
  | `signed` | boolean | false for a development install |
  | `requires`, `hosts` | array | from the manifest |
  | `unrestricted` | boolean | `requires` names `native` |
  | `consented` | boolean | the owner agreed (always true for a portable service) |
  | `store` | object | `{bytes, maxBytes}` of the id's store |
  | `state`, `pid`, `since`, `channels`, `lastExit`, `error` | | as `Daemon` today |

- **`StoreApp`**: `services` is removed (companion services); `hosts` stays
  (the applet's); new `service`: `{requires, hosts, unrestricted} | null`,
  from the signed members, so the store page shows them before a download.
- **`Device.channels`**: each entry gains `service: "applet" | "system"`, so
  a client shows the applet's channel and not the system channel as the
  applet's computer side.

**§3 Methods.**

| Today | Proposed |
| --- | --- |
| `daemons.list` | `services.list` → `{services: [Service]}` |
| `daemons.install {path}` | `services.install {path, consent?}` → `Service`: a **development** release (`TRUST.md` §8). Progress `{stage, message}`; `stage` `runtime`, then `checking`, then `installing` |
| `daemons.remove {id, purge?}` | `services.remove {id, release?, purge?}`: without `release`, every release of the id. `purge` deletes the store, the native folder and the key pin |
| `daemons.start`, `.stop`, `.restart {id}` | `services.start`, `.stop`, `.restart {id, release?}` |
| `daemons.logs {id, tail?}` | `services.logs {id, tail?}` |
| `daemons.runtime {install?}` | `services.runtime {install?}` |
| `store.install {device, app}` | `store.install {device, app, consent?}`. **For a release with a service, Hub installs the service before it pushes the applet**, so the applet's first `channel.open` finds it. `consent: "unrestricted"` is required for a release whose service requires `native`; without it the error is `consent` and nothing is installed. Progress phases: `downloading`, then **`service`** (with `stage` `runtime` while Bun is fetched, `checking`, `installing`), then those of `devices.push` |
| new | **`services.consent {id, release}`**: the owner agrees to an unrestricted release that is installed and waiting (installed from the console's store page, `QUESTIONS.md` Q16) |
| new | **`signin.claim {id}`**, **`signin.finish {id, result}`**: below |

**Sign-ins.** `hubd` has no window; Pocket Hub's app shows it.

- Hub sends the event **`signin.requested`** `{id, service, title, kind:
  "web"|"form", web?: {url, until}, form?: {help, link, fields}, expiresAt}`
  to clients of kind `gui` subscribed to the topic `signin`. `until` is sent
  so the app knows which cookies to hand back, and nothing else.
- The first client to call `signin.claim {id}` shows it; another gets
  `busy`. **When no `gui` client claims it within 2 s, the service's call
  ends `unavailable`**: no fallback.
- The client calls `signin.finish {id, result}` with `{cookies}`, `{url}`,
  `{values}` or `{declined: true}`. Hub checks the result against the request
  (only the named cookies, a URL with the redirect's prefix, the fields and
  their patterns), hands it to the service, and sends `signin.ended {id}`.
- Cookie and form values pass through the control API on the local socket
  (mode 0600) and are never logged; "No link key in the control API" is
  unchanged (these are not link keys), and the same care is written beside
  it.

**§4 Events.**

| Today | Proposed |
| --- | --- |
| `daemon.changed {daemon}` | `service.changed {service}` |
| `daemon.removed {id}` | `service.removed {id, release}` |
| `daemon.log {id, at, stream, text}` | `service.log {id, release, at, stream, text}` |
| | `signin.requested`, `signin.ended` (topic `signin`) |

The overflow rule ("Hub sends `device.changed`, `daemon.changed` and
`store.changed` for everything again") names `service.changed`; `signin.*`
events are not dropped: an overflowing client loses a claim it has not made,
and the 2 s rule ends the call.

**§3 Store, last paragraph** ("An applet's daemon is not part of what
`store.install` fetches until the store publishes daemons") is replaced by
the `store.install` row above.

## 2. `docs/DAEMON-PROTOCOL.md` becomes `docs/SERVICE-PROTOCOL.md`

| Section today | What it becomes |
| --- | --- |
| intro | "A service is the code of one applet that runs on a host"; the three-part table names `sdk/service`, `crates/hub-services`, the relay, and **the capability providers** (`crates/hub-net` and the new image, text and media modules) |
| §1 The daemon package | **The service package**: `MANIFEST.md` §1 to §3 in full; the folders of `HOST-PROTOCOL.md` §8; "Hub provides the SDK" as `MANIFEST.md` §3 (Hub's copy always); the install over a running service unchanged, per release |
| §2 The process | the bootstrap, the three variables, the environment of portable and native services (`HOST-PROTOCOL.md` §2), and the sandbox (`TRUST.md` §5) once step two lands; exit codes, logs, the failed-start hold unchanged |
| §3 Frames | adds the host connection's payload limit (65,536) and id range (`HOST-PROTOCOL.md` §5) |
| §4 The control connection | renamed messages, `protocol` 2, `service.ready`'s `release`, `host` and `files`, no `data` (`HOST-PROTOCOL.md` §3) |
| §5 A channel connection | unchanged but for the close reason `service` (with the wire's rename) |
| new §6 The host connection | `HOST-PROTOCOL.md` §4 to §7 |
| new §7 Capabilities | `CAPABILITIES.md` §3 to §10: the SDK surface, the limits and the checks |

`AGENTS.md` "What decides a question" names `docs/SERVICE-PROTOCOL.md`;
"A daemon gets no device operation" becomes "A service gets no device
operation".

## 3. Order of work

Each step is one pull request that leaves the tree working. Steps 1 to 3
need nothing from PocketJS; step 4 waits for PocketJS's change to merge and
moves the pin.

| Step | Repository | Change | Why the tree still works |
| --- | --- | --- | --- |
| 0 | here, PocketJS | these documents and the specification proposal, confirmed | documents only |
| 1 | here | **local rename**: crate, SDK folder and package name, manifest file name and version key, variables, handshake names with `protocol` 2, control API methods, objects, events and error kind, the app, the CLI, the examples file regenerated. **Wire names stay** (`no_daemon`, close reason `daemon`, `link.channel`) | no behaviour changes; every test renamed with its subject; the SDK and Hub move together because Hub ships the SDK |
| 2 | here | the manifest's new fields, releases and folders, the bootstrap (no `serve()` in an entry), **the host connection with `store`**, the data folder removed; the two examples rewritten without Bun's interfaces (`sample` computes its CRC in JavaScript; `link-channel`'s HTTP fixture moves to the test harness) | the examples are the tests' services; a service without `requires` behaves as a daemon does today |
| 3 | here | **`net` for services** (`fetch`, `websocket`) and the destination policy keyed by service id and manifest `hosts` | applets keep today's `PublicDestinations` for `net.http` until step 7 |
| — | PocketJS | the implementation of the specification change: `device.accepts`, the system channel, `service` in `channel.open`, `hash` in `net.http`, the wire renames, the framework modules, the 3DS glue, the reference Hub and device, the vectors (its §12.4) | its own tests and the 3DS run |
| 4 | here | **pin moved**; the wire renames; `device.accepts` kept per connection; **the system service in-process** (`ChannelHost::open` answers `service: "system"` with an in-memory channel connection, so the relay's rules hold for it unchanged); **`image`, `text`, `media`** in Rust, served on the system channel and on the host connection | the reference device of the new pin is the tests' peer; a device on the old firmware is out of date anyway (no compatibility before release) |
| 5 | here | **`signin`**: the control API methods and events, the app's window and form | a service without `signin` is unaffected |
| 6 | applets | **the pilots** rewritten: GitHub (no service), X (portable), YouTube (native), installed as development releases (`services.install`) | the other applets keep failing as they do today (Hub answers `companion.pocket` with `unavailable`) |
| 7 | here | **the applet's hosts for `net.http`**: Hub learns each package's `applet.hosts` from the packages it installs and checks by `app` and `hash` | applets from this Mac's store keep working; `QUESTIONS.md` Q4 for the others |
| 8 | applets, then here | **signed releases**: the store's changes of `TRUST.md` §9, then Hub's checks of `TRUST.md` §6 and §7, `store.install` with a service, `consent` | releases without a service verify as today |
| 9 | here | **the checking tool** at install (step one of the sandbox) | development releases that fail it are refused with the reason |
| 10 | here | **`sandbox-exec`** for portable services (step two), after the measurement of `TRUST.md` §5 is repeated with a real service | native services are not enclosed |
| 11 | applets | the remaining applets | |

The direction's order is (2) the capabilities, (3) the pilots, (4) the
sandbox and signatures, (5) the rest. The table keeps it, puts the rename
first because every later diff is smaller with the final names, and places
the specification's implementation before step 4 because `image`, `text` and
`media` need the device's statement and the system channel.
