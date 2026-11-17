# Open questions

Proposal (see `README.md`). What is decided, then what is left, with options
and a recommendation each. **Conflict** marks a place where a decision does
not fit the code or the specification as it stands.

## Decided

**By the owner, on 2026-11-17:**

- **Names**: `service`, `host`, `@pocketjs/service`, `service.channel` and the
  names derived from them (`README.md` §3). Open: the three names that keep
  "link" (Q1) and the service host's crate and program names (Q2).
- **Omarchy Pocket's service host is Hub's, factored out**; no second
  implementation (`DEVICES.md` §4).
- **Consent to an unrestricted service is the owner's own click in Pocket
  Hub's app** (the launcher on Omarchy Pocket); never the command line, a
  remote client or a link (`TRUST.md` §2).
- **On a device with no measured enclosure only services published with the
  official key run**, unenclosed; any other does not run there, whatever it
  requires (`channel.open` refusal `enclosure`, `DEVICES.md` §6).
- **`signin` is not a capability; the setup happens outside the host, and the
  important confirmations are asked by the agent, not by the host**: no click
  in the app for a setup guide or for a private origin (`SETUP.md`).

**Ruled on 2026-11-17 by the coordinator, on the review's evidence**: no check
of whether a control client descends from a service (`TRUST.md` §2 says why).

**Found answered on `1bc2074`**: the repository is public under
GPL-3.0-or-later; the Tab5 speaks Pocket Link 2 and pairs by a code.

**Earlier, on 2026-11-12**: Omarchy Pocket becomes a host and every device
takes the new path; the control socket is closed in this work; the LAN
attacker without the key is accepted until link encryption; decoding is
performance-first.

## 1. First rank

**Q1. Can "link" leave the JavaScript surface?** `@pocketjs/framework/link`,
`globalThis.link` and the capability `link.companion` bundle four concerns:

| Concern | Exports today | Who uses it |
| --- | --- | --- |
| a wall clock to trust | `companionNow()`, `linkStatus().clock` | **applets, and only this**: `lib/net.ts` re-exports it; `kit/platform.tsx` wraps it as `now()`, which `kit/page.ts` calls; `hn/app.tsx`, `weather/model.ts`, `x/app.tsx`, `podcasts/views.tsx`, `spotify/model.ts`, `spotify/signin.tsx` call it |
| the applet catalog, install, remove | `requestCatalog`, `installApp`, `removeApp`, `onLinkEvent`, `LinkCatalogApp` | the launcher (pocket-shell `shells/3ds/shell/system.ts`, `store.tsx`) for the 3DS and the Tab5; Omarchy Pocket's Store applet does the same through `companion.pocket/appstore` |
| whether the service host is reachable | `linkStatus().state`, `.companion`, `.features`, `.error`, `linkHost()`, `LINK_STATE` | the launcher (`system.ts`, `pages.tsx`); the applets' words (`ui/companion.ts`) |
| pairing with a computer | `pairCompanion`, `forgetCompanion`, `isPairingUri`, `LINK_PAIRING`, the Tab5's `pairOpen`, `pairAsk`, `pairClose` (link §3.11), `linkStatus().paired`, `.pairing`, `.pair`, `.device` | the launcher (`system.ts`, `scanner.tsx`) |

**The end state proposed**:

| Concern | Proposed | 3DS | Tab5 | Omarchy Pocket | PSP |
| --- | --- | --- | --- | --- | --- |
| clock | `wallClock()` → `{utc, offsetMinutes, source}` in a module of its own, **`@pocketjs/framework/wall-clock`** (not `clock`, whose `framework/src/clock.ts` is the virtual frame clock); no capability | the paired Hub's clock | Hub's, or SNTP | the system clock | its own clock |
| catalog | `@pocketjs/framework/apps`, `globalThis.apps`, capability `system.apps` (launchers only) | the link's `catalog`, `install` | the same | its service host over the local connection | version 1's catalog until the PSP moves (link §11) |
| service host status | `serviceHost()` → `{state, name}` beside the applet's channel in `@pocketjs/framework/service` (whose file is named apart from `service-client.ts` and `services.ts`) | the link's state | the same | ready while the host runs | none |
| pairing | `@pocketjs/framework/pairing`, `globalThis.pairing`, capability `system.pairing` (launchers only): list, forget, and the code of §3.11 | list, forget | list, forget, the code | not provided | version 1's |

The local/remote flag has one name, the channel state's `local`
(`channelState().local`, link §7.11). **What keeps "link"**: the protocol and
its page, `contracts/spec/link2.ts`, the C client, the labels of §3.5, the
pairing folder on a card; none is seen by an applet's author or an owner.

**Options**: (a) keep the three names; (b) the whole split at once; (c)
**move only the clock now**, so "link" is gone from an applet author's view
(the clock is the one export applets use), and split the launcher's surface
(apps, pairing, service host status) when Omarchy Pocket's Store moves to the
shared host (`MIGRATION.md` step 8). *Recommendation*: **(c)**, with the end
state above unchanged.

**Blast radius of the end state**: PocketJS `framework/src/link.ts`,
`contracts/spec/link.ts`, `engine/host-services/link2_guest.{c,h}`,
`products/3ds/src/qjs.c`, `contracts/spec/{platform-types,applet}.ts`
(`link.companion`; `APPLET_SYSTEM_LINK_EXPORTS`), `tools/applet-check.ts`,
`hosts/3ds-dev/pocket.host.json`, `hosts/psp/pocket.host.json`,
`hosts/psp-runtime/pocket.host.json`, `platforms/psp/io/src/link.rs`,
`programs/psp/src/service_bindings.rs`, `platforms/sim/link.ts`,
`programs/sim/console-3ds.ts`, `tools/repack/3ds.ts`,
`site/content/docs/app-contract.md`, `site/content/changelog.md`,
`docs/APPLETS.md`, `docs/POCKET-LINK.md` §7.10, `tests/applet.test.ts`,
`tests/platform-contracts.test.ts`, `tests/repack-3ds.test.ts`,
`tests/e2e/azahar-link2.ts`, `tests/fixtures/plans/portable-psp.plan.json`;
pocket-shell `shells/3ds/shell/{system.ts,store.tsx,pages.tsx,scanner.tsx}`
and `shells/3ds/test/shell-sim.test.ts`; pocket-m5 `components/pocket_system/src/qjs.c`
and its host contract; omarchy-pocket's host contract and the Store applet;
applets `lib/net.ts`, `kit/platform.tsx`, `kit/page.ts`, the six callers,
`appstore/`, `ui/companion.ts`. Under (c), the first change touches only the
clock's lines of these.

**Q2. Two meanings of "host", and the names that follow.** PocketJS's "host"
is the device's runtime (host contract, `hostAbi`, link §7.3). Proposed:
`host` stays the SDK's identifier; in PocketJS's prose and code "host" alone
keeps the runtime's meaning and the other side is "service host" (applied in
link §7.9 to §7.11); on these pages "host" is the service host and "runtime"
the device's program. **The crate and program names collide too**:
`pocket-host` beside PocketJS's `pocket.host.json` and Omarchy Pocket's
`omarchy-pocket-host`. *Recommendation*: **`crates/pocket-service-host`,
`pocket-service-hostd`, command line `pocket-service-host`**, and Hub's own
`ChannelHost` trait (`crates/hub-core/src/api.rs`, implemented by `hubd`'s
`DaemonHost` in `crates/hubd/src/host.rs`) renamed **`ServiceConnector`** when
the code moves (`DEVICES.md` §4). The owner confirms.

**Q3. What consent binds.** (a) one release: every update asks again; (b) the
applet's id and its publisher's key: asked again when the key changes or when
an update newly requires `native`. *Recommendation*: **(b)**: the dialog asks
the owner to trust a publisher, and a click on every release tells the owner
nothing they can judge (`TRUST.md` §4).

**Q4. The console's catalog row.** A console's own store page shows what the
computer's store page shows only if the link's catalog row (link §6.4) carries
it: that a release cannot run on that device (`capability`, `enclosure`), that
it needs the owner's consent on the computer, that it comes with setup
instructions and whose. Options: one field `notes` with those values; three
fields; nothing (the console learns at `channel.open`). *Recommendation*: one
field, in the specification change.

**Q5. Other consent-class methods.** Consent is the only one. Candidates the
review raised: installing a development release, `pairing.answer`,
`services.setup`. *Recommendation*: none; the owner's rule makes the agent
the one who confirms a setup, a development install of native code is held
until consent, and a code typed in Pocket Hub is the owner's act already.

**Q6. Where the device states what it takes**: `EVENT device.accepts` per
connection (link §7.9). *Recommendation*: as proposed.

**Q7. Versions.** The stacked pull request's rule (`MIGRATION.md` §1), and
a `protocol` bump at every breaking step of the control API and of the
service handshake (2 at step 1, 3 at step 2). *Recommendation*: adopt it.

**Q8. `data.fs` on Omarchy Pocket** (`DEVICES.md` §7). *Recommendation*: yes.

**Q9. The setup inlet is open to any local program** (`SETUP.md` §2), origins
included, so a program of the owner's user can hand a service a session or an
address of its choosing. *Recommendation*: as proposed; such a program needs
no Hub to act as the owner (`TRUST.md` §2).

**Q10. Who applies `applet.hosts` on the Tab5**, whose HTTP is its own.
*Recommendation*: the firmware, in its HTTP node.

## 2. Second rank

**Q11. `applet.hosts` for `net.http` on Hub**: the package index from step 4
(the system channel needs it), `hash` required from step 7; a package not in
the index, or a request without `hash`, is refused with a sentence.
**Q12.** Wildcards refused on a public suffix; "any host" decided with
Podcasts. **Q13.** `text.atlas` with a body and `omitted`; `channel.text` for
services later. **Q14.** The system channel opens when the applet's guest
starts, as its own channel does; `text.glyphs.deferred` is a new id.
**Q15.** Media per platform; format 18 for the YouTube pilot; the listener on
an OS-assigned port. **Q16.** One process per release, one store, the newest
release's limit, a raise shown in the update's text. **Q17.** A development
release has its own store and takes precedence while installed. **Q18.** Pins
for every id; a rotation statement; removal for a lost key. **Q19.** A store
release is bundled. **Q20.** A release with a service installed from a
console gets its service first; an unrestricted one is held until consent.
**Q21.** The second signature, with the guide line. **Q22.** `sandbox-exec`
now, an embedded engine later. **Q23.** The source check's globals. **Q24.**
What a same-user program can still do (`TRUST.md` §2). **Q25. Conflict.** The
per-frame hand-over budget per guest. **Q26.** Secrets in files of mode 0600.
**Q27.** A native service's files and vendor tools. **Q28.** YouTube's
manifest `["image", "media", "native"]`. **Q29.** Limits, none measured.

## 3. Can wait

**Q30** `applet.services` leaves at step 9. **Q31** GitHub's device flow.
**Q32** Discord's gateway stops with its service. **Q33** Maps' `/locate` by
IP geolocation. **Q34** The SDK examples rewritten. **Q35** Refusals
`no_service`, `capability`, `consent`, `enclosure`. **Q36** A service host on
a weak device. **Q37** `companion` stays. **Q38** Owner-facing words.
**Q39** The pin move with a 3DS release. **Q40** Omarchy Pocket's Store applet
as a client of its service host. **Q41** Spotify on Omarchy Pocket. **Q42**
Whether Hub ever installs a Tab5's software.

**Q43.** On a device with no measured enclosure, does the rule also stop a
third-party unrestricted service the owner consented to? Read as yes
(`DEVICES.md` §6); say if the owner meant otherwise.

**Q44.** On such a device, does a development install (the owner's own code)
run? Under the rule as stated, no. *Recommendation*: let it run unenclosed
after the owner's click, labelled Development; otherwise applets cannot be
made on Omarchy Pocket until its enclosure is measured.
