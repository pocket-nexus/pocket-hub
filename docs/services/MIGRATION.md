# Changes to the existing contracts, and the order of work

Proposal (see `README.md`). What changes in `docs/CONTROL-API.md` (§1) and
`docs/DAEMON-PROTOCOL.md` (§2), read at `1bc2074`, and an order of work (§3)
that says what each device and **an existing home or a resident core of
Pocket Hub 0.2.1** meet at each step. Step 1 is pull request #23
(`refactor/services-rename`, open); a stacked pull request
(`fix/app-replaces-older-core`) proposes how an app meets an older core.

## 1. `docs/CONTROL-API.md`

**Versions** (step 1 makes the control API's `protocol` 2, #23). Proposed by
the stacked pull request and **not yet ruled on by the owner**:

- `hub.hello` and `hub.shutdown` keep their shape in every protocol version,
  so any client can always say hello and ask a core to quit;
- a core that refuses a hello says its own `protocol` in the error's `data`,
  from protocol 2 on (0.2.1 sends none);
- **the app replaces an older core** (a lower protocol, or the same protocol
  and a lower `hub.version`): it asks it to shut down and starts its own; it
  **never replaces a newer one**, and tells the owner a newer Pocket Hub is
  running; **the command line never replaces a core**.

What that leaves open: a refusal without `protocol` in `data` is read as
"older than 2", which holds only because 0.2.1 is the one release before it;
two apps of different versions on one Mac replace each other's core in turn
until the older is quit; a `remote` client's versioning is not defined. **This
proposal adopts the rule for every later breaking change** (step 2's
`Service`, step 5's methods): each bumps `protocol`.

**§1 Who may call what** (`TRUST.md` §2): **consent-class methods**
(`services.consent`, and `consent` on an install) are taken only from the
app whose executable's code-directory hash the core pinned from its own
bundle, checked by audit token (`LOCAL_PEERTOKEN`); another client gets the
error kind `denied`; a core outside a bundle answers `denied` to every
client; a `remote` client and a `pocket-hub:` link never reach them. Every
other method stays open to every local client, as today.

**§1 Errors**, the `kind` table:

| Row | Change |
| --- | --- |
| `daemon` | renamed `service` in step 1 (#23). From step 2 `data` gains `reason?`: `manifest`, `check`, `runtime`, `enclosure` (it could not be set up; nothing runs unenclosed), `earlier` (a package of step 1's layout, kept and not run), and the service's own `unknown_setup` or `invalid_setup` |
| `store` | `reason` gains `key` |
| new: **`consent`** | the release runs its own programs and the owner has not allowed it; `data`: `{id, release}` |
| new: **`denied`** | a consent-class method from a client that is not the pinned app |

**§2 Objects** (from step 2): **`Service`** gains `release`, `signed`,
`requires`, `hosts`, `unrestricted`, `consented`, `setup`, `origins`, `store:
{bytes, maxBytes}`; a held release (§4 of `TRUST.md`) is `state: "held"`.
**`StoreApp`** keeps its name and its `services` member until step 9 (#23
leaves it), and gains `service: {requires, hosts, unrestricted, setup} | null`
and `runsOn: [{device, reason?}]` (below). **`Device.channels`** entries gain
`service: "applet" | "system"`. `Device.kind` and `Device.supports` are
unchanged: channels are opened by the device and need no entry.

**§3 Methods** (step 1's names, #23, then the additions):

| Step 1 (#23) | Later |
| --- | --- |
| `services.list`, `services.install {path}`, `services.remove {id, purge?}`, `services.start`, `.stop`, `.restart {id}`, `services.logs`, `services.runtime`; command line `pocket-hub services`, `pocket-hub service <install\|start\|stop\|restart\|logs\|remove>`, flag `--service-idle-seconds` | step 2: `release?` on start, stop, restart, remove; `purge` also deletes `service-data/<id>/`, the stores, the pin and the origins |
| `store.install {device, app}` (installs the applet only; it never installs a service, #23) | step 9: the release's service first; `consent: "unrestricted"` (consent-class); without it an unrestricted service is **installed and held** |
| | step 5: **`services.consent {id, release}`** (consent-class) |
| | step 5: **`services.setupGuide {id, release?}`** → `{path, release, signed, publisher}`; **`services.setup {id, message, origins?, release?}`** → `{reply, origins: [{origin, addresses}]}`; **`services.origins {id}`**, **`services.revokeOrigin {id, origin}`**; command line `pocket-hub service guide <id>`, `pocket-hub service setup <id> <file\|-> [--origin <url>…]` (`SETUP.md`) |
| `pairing.list`, `.answer`, `.decline` | unchanged |

**What a device is offered** (finding of the review: step 9 assumed a
mechanism): `store.apps {device}` and the link's catalog (§6.4) offer a
release to a device only when the device's target has a variant, its service
host provides every capability the release requires, and, where that host
has no measured enclosure, the release is the official key's. **A release
that fails is listed with `runsOn: [{device, reason}]`** (reasons
`capability`, `enclosure`) on the computer, and **marked in the link's
catalog row** on the console (a field the row would gain, `QUESTIONS.md` Q4),
never installed. A release waiting for consent is offered and held.

**`net.http` from a device** (from step 7, Q11): a request without `hash`
gets a sentence saying to update the 3DS's software; one whose package is not
in the index, a sentence saying to install the applet from this Mac.

**§4 Events**: step 1 renames `daemon.*` to `service.*` (#23); step 2 adds
`release` to `service.removed` and `service.log`.

## 2. `docs/DAEMON-PROTOCOL.md`, which step 1 makes `docs/SERVICE-PROTOCOL.md`

| Section after #23 | What step 2 and later make of it |
| --- | --- |
| intro | the parts: `sdk/service`, `crates/pocket-service-host`, the relay in `hub-core` (`DEVICES.md` §4); "a service gets no device operation" with its mechanism |
| §1 The service package | `MANIFEST.md` §1 to §3, the setup guide, the folders of `HOST-PROTOCOL.md` §8 and the rule for an earlier layout |
| §2 The process | the bootstrap, the variables, the environments, the enclosure (`TRUST.md` §5) |
| §3 Frames | the host connection's payload limit and id ranges |
| §4 The control connection | `protocol` 3, `service.ready`'s `release`, `host` and `files`, no `data` |
| §5 A channel connection | the close reason `service` with the pin move |
| new §6, §7 | the host connection (`HOST-PROTOCOL.md` §4 to §7) and the capabilities (`CAPABILITIES.md` §3 to §10) |

## 3. Order of work

| Step | Repository | Change | Existing home, older core | 3DS | Tab5 | Omarchy Pocket |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | here, PocketJS | these documents | | as today | links over Pocket Link 2; its applets' `companion.pocket` requests reach Hub as `net.http` and are answered `unavailable` | omarchy-pocketd answers `companion.pocket` |
| 1 | here | **#23: names only.** `pocket.service.json`, `@pocketjs/service`, `POCKET_SERVICE_*`, the handshake at `protocol` 2, the control API at `protocol` 2, the command line and flag of §1. New folders `services/`, `service-data/`, `services-staging/`, `service.sock`, `<home>/sdk/service`. **Nothing under the old names is read, moved or deleted.** Only Advanced › Services › Install and `pocket-hub service install` install a service; the store's install, deep links and setup never do | **an owner of 0.2.1 who never opened Advanced has nothing under the old names**; one who installed a daemon finds `daemons/` and `daemon-data/` left where they were, unread, and installs the service again. An app of this step meeting a 0.2.1 core replaces it (the stacked pull request) | as before | as before | as before |
| 2 | here | **the host parts into `crates/pocket-service-host`** (`DEVICES.md` §4); the manifest's new fields, releases, the bootstrap; **the host connection with `store`** (`CAPABILITIES.md` §5, its format); **the macOS enclosure, measured with the shipped Bun** (`TRUST.md` §5); the handshake at `protocol` 3; `service-data/` no longer given to any process; the control API's next `protocol` | a `services/<id>/` without `releases.json` is listed `failed`, reason `earlier`, and kept (`HOST-PROTOCOL.md` §8); `service-data/<id>/` stays until `purge` | services without capabilities behave as today | as before | as before |
| 3 | here | **`net` for services** | | as before | as before | as before |
| — | PocketJS | the specification's implementation and **a 3DS program release** | | | | |
| 4 | here | **pin moved** with that release; the wire renames; the system service; **`image`, `text`, `media`** with the host-wide bounds; the media listener; **the package index** (`<home>/packages.json`, so the system channel checks the applet's hosts; today Hub allows every public destination, `PublicDestinations` in `crates/hub-net/src/lib.rs`) | the index is built from the store cache and from later pushes | **an older program strands**: one channel, no system service; Pocket Hub offers the update | as before | as before |
| 5 | here | **the hardened runtime for the app**, **consent from the pinned app**, the setup guide and inlet, origins, **`native` for development installs** | | can be set up | as before | as before |
| — | pocket-m5 | the channel groups, namespaces, `device.accepts`, `applet.hosts` in its HTTP node; **a firmware release, flashed over USB by its owner** | | | **runs channels once flashed** | |
| 6 | applets | **the pilots**, as development releases; not published | | runs them | runs them once flashed | would strand if published |
| 7 | here | **`applet.hosts` enforced for every `net.http`** (the index exists since step 4); `hash` required | | a program without `hash` in `net.http` is told to update the 3DS's software | its own HTTP (Q10) | |
| 8 | omarchy-pocket, here | **`pocket-service-hostd`**, the enclosure measured per device, the local transport, the control path of `DEVICES.md` §4; `data.fs`, the host contract, the Store applet as the host's client | | | | **runs the pilots**; X cannot be set up there yet |
| 9 | applets, then here and omarchy-pocket | **signed releases**, `store.install` with services, consent, `runsOn` and the catalog mark; `applet.services` leaves the applet contract; **the rewritten applets published** | | installs them | installs them once flashed | installs them |
| 10 | here, omarchy-pocket | the source check at install | | | | |
| 11 | applets, omarchy-pocket | the remaining applets; Spotify on Omarchy Pocket | | | | |

**The strands**: a 3DS on an older program from step 4 (mended over the link);
a Tab5 until its owner flashes a firmware with channels (Hub cannot install
its software, Q42); Omarchy Pocket never, because nothing is published before
step 9. **An existing home** loses nothing silently: the old folders are left
unread in step 1, step 1's layout is listed and kept from step 2, and the old
data folder stays until the owner removes it.
