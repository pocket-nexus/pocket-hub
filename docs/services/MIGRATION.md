# Changes to the existing contracts, and the order of work

Proposal (see `README.md`). What changes in `docs/CONTROL-API.md` (§1) and
`docs/DAEMON-PROTOCOL.md` (§2), as diffs in words, and an order of work
(§3) that keeps `cargo test --workspace`, the SDK's tests and the app's tests
working after each step and says what each of the three devices runs at each
step. The names are the proposal's (`README.md` §3).

## 1. `docs/CONTROL-API.md`

**§1 Hello.** `protocol` becomes **2** with the renamed methods; a client
that says 1 is refused with `-32602` and a sentence (`QUESTIONS.md` Q10).

**§1 Who may call what** (new, `TRUST.md` §2):

- On each accept Hub takes the peer's process id (`LOCAL_PEERPID`). **A
  connection from a process Hub started as a service, or from a descendant of
  one, is refused** before `hub.hello`.
- **The privileged methods** (`signin.claim`, `signin.finish`,
  `services.consent`, `services.install`, and `consent` on `store.install`)
  are answered only for a client whose executable carries the same code
  signing identity as `pocket-hubd` (Pocket Hub's app and command line in
  the same bundle); another client gets the error kind `denied`. Until
  step 5 brings the identification, `services.install` stays open as
  `daemons.install` is today; no unrestricted service exists before it.
  **A client of kind `remote` never gets them.**
- "A client on the local socket may call every method" becomes "every
  method but the privileged ones".

**§1 Errors**, the `kind` table:

| Row | Change |
| --- | --- |
| `daemon` | renamed **`service`**: "a service could not be installed, started or stopped". `data` gains `reason?`: `manifest` (the field in `detail`), `check` (the source check's first lines in `detail`), `runtime` (Bun could not be fetched), `enclosure` (the OS enclosure could not be set up; no fallback to running it unenclosed) |
| `store` | `reason` gains `key` (another key than the one pinned, `TRUST.md` §7); `rejected` covers the checks of `TRUST.md` §6 |
| new: **`consent`** | "the release runs its own programs on this computer, and the owner has not agreed". `data`: `{id, release}`. A client shows the dialog of `TRUST.md` §4 and calls again with `consent: "unrestricted"` |
| new: **`denied`** | a privileged method from a client Hub does not identify as its own |
| `not_found` | "no such device, service, app, sign-in or file" |

**§2 Objects.**

- **`Daemon` becomes `Service`**, one installed release: `id`, `release`
  (the applet package's footer hash, or `dev`), `version`, `signed`,
  `requires`, `hosts`, `unrestricted`, `consented`, `store: {bytes,
  maxBytes}`, and `state`, `pid`, `since`, `channels`, `lastExit`, `error` as
  today.
- **`StoreApp`**: `services` removed; `hosts` stays (the applet's); new
  `service: {requires, hosts, unrestricted} | null` from the signed members.
- **`Device.channels`**: each entry gains `service: "applet" | "system"`.
- **`Device.kind`**: `3ds`, and `tab5` once its firmware links (step 11).

**§3 Methods.**

| Today | Proposed |
| --- | --- |
| `daemons.list` | `services.list` → `{services: [Service]}` |
| `daemons.install {path}` | **`services.install {path, consent?}`** (privileged) → `Service`: a development release (`TRUST.md` §8). `consent: "unrestricted"` for one that requires `native`. Progress `{stage}`: `runtime`, `checking`, `installing` |
| `daemons.remove {id, purge?}` | `services.remove {id, release?, purge?}`: without `release`, every release of the id; `purge` deletes the stores, the native folder, the owner-named origins and the key pin |
| `daemons.start`, `.stop`, `.restart {id}` | `services.start`, `.stop`, `.restart {id, release?}` |
| `daemons.logs {id, tail?}` | `services.logs {id, tail?}` |
| `daemons.runtime {install?}` | `services.runtime {install?}` |
| `store.install {device, app}` | **`store.install {device, app, consent?}`**: for a release with a service, Hub installs the service before it pushes the applet; `consent: "unrestricted"` (privileged) for one that requires `native`, else the error `consent` and nothing installed. Phases `downloading`, `service` (`runtime`, `checking`, `installing`), then `devices.push`'s |
| new | **`services.consent {id, release}`** (privileged): the owner allows an unrestricted release installed from a console's store page (`QUESTIONS.md` Q22) |
| new | **`services.origins {id}`**, **`services.removeOrigin {id, origin}`**: the owner-named origins (`CAPABILITIES.md` §6) |
| new | **`signin.claim {id}`**, **`signin.finish {id, result}`** (privileged), below |

**Consent has one shape everywhere**: the string `"unrestricted"` in the
`consent` member of `store.install` and `services.install`, and the method
`services.consent` for a release already installed.

**Sign-ins.** Hub sends `signin.requested {id, service, applet: {title,
icon, author}, kind: "web"|"form", web?: {url, until}, form?: {title, help,
link, fields}, expiresAt}` to `gui` clients subscribed to `signin`. The first
identified client to call `signin.claim` shows it; another gets `busy`.
**When no identified client claims it within 2 s, the service's call ends
`unavailable`.** `signin.finish {id, result}` carries `{cookies}`, `{url}`,
`{values}` or `{declined: true}`; Hub checks it against the request (the
named cookies only, the redirect's prefix, the fields and their patterns),
hands it over, and sends `signin.ended {id}`. Values are never logged.

**`net.http` from a device** (Q11): a request without `hash` is answered
with an error whose sentence says to update the 3DS's software; one whose
package is not in Hub's index (`<home>/packages.json`) with a sentence saying
to install the applet from this Mac.

**§4 Events.** `daemon.changed {daemon}` → `service.changed {service}`;
`daemon.removed {id}` → `service.removed {id, release}`; `daemon.log` →
`service.log {id, release, at, stream, text}`; new `signin.requested`,
`signin.ended` (topic `signin`). The overflow rule names `service.changed`.

**§3 Store, last paragraph** is replaced by the `store.install` row.

## 2. `docs/DAEMON-PROTOCOL.md` becomes `docs/SERVICE-PROTOCOL.md`

| Section today | What it becomes |
| --- | --- |
| intro | "A service is the code of one applet that runs on a host"; the parts: `sdk/service`, the host (`crates/pocket-host`: supervisor, capability providers, system service, enclosure), the relay; "a service gets no device operation" with its mechanism (`TRUST.md` §2) |
| §1 The daemon package | **The service package**: `MANIFEST.md` §1 to §3; the folders of `HOST-PROTOCOL.md` §8 |
| §2 The process | the bootstrap, the three variables, the environments, **the enclosure** (`TRUST.md` §5); exit codes, logs, the failed-start hold unchanged |
| §3 Frames | adds the host connection's payload limit and id range |
| §4 The control connection | renamed messages, `protocol` 2, `service.ready`'s `release`, `host` and `files`, no `data` |
| §5 A channel connection | unchanged but for the close reason `service` |
| new §6 The host connection | `HOST-PROTOCOL.md` §4 to §7 |
| new §7 Capabilities | `CAPABILITIES.md` §3 to §10 |

`AGENTS.md` names `docs/SERVICE-PROTOCOL.md`; "A daemon gets no device
operation" becomes "A service gets no device operation".

## 3. Order of work

Each step is one pull request that leaves its repository's tree working.
"Strands" names a device that loses something at that step.

| Step | Repository | Change | 3DS | Tab5 | Omarchy Pocket |
| --- | --- | --- | --- | --- | --- |
| 0 | here, PocketJS | these documents and the specification proposal, confirmed | as today | as today: version 1 only, which this Hub does not speak | as today: omarchy-pocketd answers `companion.pocket` |
| 1 | here | **local rename**: crate, SDK, manifest, variables, handshake (`protocol` 2), control API (`protocol` 2), app, command line, examples file. Wire names stay | as before | as before | as before |
| 2 | here | **the host parts into `crates/pocket-host`**; the manifest's new fields, releases, folders, the bootstrap; **the host connection with `store`**; **the macOS enclosure for every portable service** and **the peer check on the control socket** (`TRUST.md` §2 (a), (b)); the data folder removed; the two examples rewritten (Q36) | services without capabilities behave as daemons today | as before | as before |
| 3 | here | **`net` for services**, the destination policy keyed by service id and manifest `hosts` | applets keep `net.http` as today | as before | as before |
| — | PocketJS | the specification's implementation: `device.accepts`, the system channel, **the local transport** (§7.11), `hash` in `net.http`, the wire renames, the framework modules, the 3DS glue, the reference, the vectors; **a 3DS program release** | | | |
| 4 | here | **pin moved** with that program release; the wire renames; the system service in-process (the relay's `ChannelHost` answers `service: "system"` with an in-memory channel connection); **`image`, `text`, `media`**: pure-Rust image decoding, PocketJS's text engine, AVFoundation and VideoToolbox; the media listener | **strands a console on an older program**: one channel, no system service, no `device.accepts`; Pocket Hub offers the update (Q41) | as before | as before |
| 5 | here | **identified clients and the privileged methods**, then **`signin`** (control API, the app's window and form, owner-named origins), then **`native` for development installs** (`files`, the unrestricted environment, the consent dialog) | can sign in to a portable service | as before | as before |
| 6 | applets | **the pilots rewritten** (GitHub, X, YouTube), installed on the Mac as development releases; **not published** | runs the pilots | — | **would be stranded if published**: a store version carries every device's variant, and Omarchy Pocket has no host yet; so the store keeps today's versions |
| 7 | here | **the applet's hosts for `net.http`**: the package index, `hash` required (Q11) | applets from this Mac keep working; older programs are told to update | — | — |
| 8 | omarchy-pocket, here | **Omarchy Pocket's host**: `pocket-hostd` built for Linux aarch64, the Linux enclosure measured per device (`DEVICES.md` §6), the local transport's endpoint; in omarchy-pocket: the device program's local connections, `data.fs`, the host contract, the Store applet as the host's client; omarchy-pocketd drops its companion services | | | **runs the pilots on the device**; YouTube as native; X without `signin.web` until Q6 |
| 9 | applets, then here and omarchy-pocket | **signed releases**: the store's changes (`TRUST.md` §7), then the hosts' checks, `store.install` with a service, `consent`; `applet.services` leaves the applet contract; **the rewritten applets are published** | installs them from the store | **stranded**: no version-2 firmware yet; the store's new versions do not run on its version-1 firmware | installs them from the store |
| 10 | here, omarchy-pocket | **the source check** at install on both hosts | | | |
| 11 | pocket-m5, here | **the Tab5**: version 2's client, the card pairing, a `tab5` installer over USB in `crates/hub-setup`, its HTTP node applying `applet.hosts`, a firmware release (`DEVICES.md` §8) | | **runs the applets through Hub** | |
| 12 | applets | the remaining applets; Spotify as an unrestricted service on Omarchy Pocket (Q43) | | | |

**Where the strands are, and why they are accepted**: the 3DS on an older
program at step 4 (an update over the link mends it); the Tab5 from step 0 to
step 11 (it cannot link to this Hub today, and the new applets need
version 2); Omarchy Pocket is never stranded, because the rewritten applets
are published only at step 9, after its host exists. The direction's order
((2) capabilities, (3) pilots, (4) sandbox and signatures, (5) the rest) is
kept, with two moves the owner decided: **the enclosure and the control
socket's closing come in step 2**, before `store` and `signin` hold anything,
and the second device host is built before anything is published.
