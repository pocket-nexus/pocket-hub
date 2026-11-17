# Open questions

Proposal (see `README.md`). What is left open, with options and a
recommendation each. **Conflict** marks a place where a decision or the
direction does not fit the code or the specification as it stands.

## Decided

**On 2026-11-17, by the owner:**

- **Names**: `service`, `host`, `@pocketjs/service`, `service.channel` and the
  names derived from them (`README.md` §3). Not decided: the three names that
  keep "link" (Q1).
- **Omarchy Pocket's host is Hub's host, factored out** into
  `crates/pocket-host` and run there as `pocket-hostd`; there is no second
  implementation (`DEVICES.md` §4).
- **Consent to an unrestricted service is the owner's own click in Pocket
  Hub's app** (in the shell on Omarchy Pocket); never the command line, a
  remote client or a `pocket-hub:` link (`TRUST.md` §2).
- **A device with no measured enclosure** runs services of the official key
  unenclosed and does not run a third-party portable service that requires
  `store`; measuring each device is an implementation step (`DEVICES.md` §6).
- **`signin` is not a capability.** A release ships a setup guide that a
  coding agent on the host's machine follows with the owner, outside the
  host; six capabilities remain (`SETUP.md`).

**Found answered on `1bc2074`**: this repository is public under
GPL-3.0-or-later, so the host crate's visibility is settled
(`DEVICES.md` §4 on Omarchy Pocket's MIT); the Tab5 already speaks Pocket
Link 2 and pairs by a code it shows, so the earlier questions about its
pairing and its move to version 2 are gone.

**Earlier, on 2026-11-12**: Omarchy Pocket becomes a host and every device
takes the new path; the control socket is closed in this work; the LAN
attacker without the key is accepted until link encryption; decoding is
performance-first.

## 1. First rank

**Q1. Can "link" leave the JavaScript surface?** `@pocketjs/framework/link`,
`globalThis.link` and the capability `link.companion` bundle four concerns:

| Concern | Exports today | Who uses it |
| --- | --- | --- |
| a wall clock to trust | `companionNow()`, `linkStatus().clock` | **applets, and only this**: `lib/net.ts` and `kit/platform.tsx` re-export it; `hn/app.tsx`, `weather/model.ts`, `x/app.tsx`, `podcasts/views.tsx`, `spotify/model.ts`, `spotify/signin.tsx` call it |
| the applet catalog, install, remove | `requestCatalog`, `installApp`, `removeApp`, `onLinkEvent` (catalog and install events), `LinkCatalogApp` | the launcher (pocket-shell `shells/3ds/shell/system.ts`, `store.tsx`) for the 3DS and the Tab5; **Omarchy Pocket's Store applet does the same through another API** (`companion.pocket/appstore`) |
| whether the service host is reachable, its name | `linkStatus().state`, `.companion`, `.features`, `linkHost()` | the launcher (`system.ts`, `pages.tsx`); the applets' "not linked" words (`ui/companion.ts`) |
| pairing with a computer | `pairCompanion`, `forgetCompanion`, `isPairingUri`, and the Tab5's `pairOpen`, `pairAsk`, `pairClose` (link §3.11), `linkStatus().paired`, `.pairing`, `.device` | the launcher (`system.ts`, `scanner.tsx`) |

Three of the four hold on a device with no link. **Proposed split**:

| Concern | Module, host namespace, capability | 3DS | Tab5 | Omarchy Pocket |
| --- | --- | --- | --- | --- |
| clock | `wallClock()` in `@pocketjs/framework/clock` → `{utc, offsetMinutes, source}`; no capability (every runtime has a best source) | the paired Hub's clock (`ready.clock`) | Hub's, or SNTP | the system clock |
| catalog | `@pocketjs/framework/apps`, `globalThis.apps`, capability `system.apps` (launchers only) | the link's `catalog` and `install` (§6.4) | the same | its service host over the local connection, with the same two groups (link §7.11) |
| service host | `serviceHost()` in `@pocketjs/framework/service` → `{state, local, name}`; comes with `service.channel` | the link's state, Hub's name | the same | `local: true` while `pocket-hostd` runs |
| pairing | `@pocketjs/framework/pairing`, `globalThis.pairing`, capability `system.pairing` (launchers only): `paired()`, `forget(id)`, and **the device-initiated way** `open()`, `ask(hub)`, `close()`, `state()` | list and forget (Hub pairs it over ftpd or the card, outside the link) | list, forget, and the code of §3.11 | **not provided** |

Pairing by a code (§3.11) is the one part that is the device's own act, and it
is the strongest reason for a `pairing` module of its own: it has a state
(the code, its time, its tries, the Hubs heard) that the other three concerns
never need.

**What stays "link"**: the protocol's name and its page, `contracts/spec/link2.ts`,
the C client (`engine/host-services/link2*.c`, `products/3ds/src/link2_host.c`),
the labels of §3.5 (`pocket-link/2/…`), the pairing folder on a card
(`link/companions/`). None is seen by an applet's author or an owner.
Version 1's `globalThis.link` stays for programs on version 1.

**Blast radius**: PocketJS `framework/src/link.ts` (split into `apps.ts`,
`pairing.ts`; `clock.ts` and `service.ts` gain one export each),
`contracts/spec/link.ts` (version 2's types move to `apps.ts` and
`pairing.ts`), `engine/host-services/link2_guest.{c,h}` (two namespaces and
the clock), `products/3ds/src/qjs.c`, `contracts/spec/{platform-types,applet}.ts`
(`link.companion` becomes `system.apps` and `system.pairing`; the applet
module list drops `link`), `tools/applet-check.ts`, `hosts/3ds-dev/pocket.host.json`,
`docs/APPLETS.md`, `docs/POCKET-LINK.md` §7.10; pocket-shell
`shells/3ds/shell/{system.ts,store.tsx,pages.tsx,scanner.tsx}`; pocket-m5
`components/pocket_system/src/qjs.c` (its `pairOpen`, `pairAsk`, `pairClose`)
and `pocket.host.json`; omarchy-pocket `host/pocket.host.json` and the Store
applet; applets `lib/net.ts`, `kit/platform.tsx`, the six callers above,
`appstore/`, `ui/companion.ts`.

*Recommendation*: **split**, with these names. Omarchy Pocket's Store applet
and pocket-shell then share one catalog API, an applet asks for a clock
without naming a transport, and the one transport-specific piece says what it
is. Not decided.

**Q2. Two meanings of "host".** PocketJS uses "host" for the device's native
runtime: the host contract (`hosts/3ds-dev/pocket.host.json`), `hostAbi` in
`hello`, the host namespace (`globalThis.link.channel`), "the host hands the
guest…" throughout link §7.3. This proposal uses it for what runs services,
and `docs/OFFLOAD.md` calls that role a "provider". **Proposed**: `host` stays
the SDK's identifier (`host.net`, `host.store`), where a service has no other
host to confuse it with; in PocketJS's specification and code "host" alone
keeps the runtime's meaning and the other side is written **"service host"**
(applied in link §7.9 to §7.11); in these pages "host" is the service host
and the device's program is its "runtime", with "host contract" and "host
namespace" written in full. **Where these pages were ambiguous** before this
revision: `README.md`'s naming row "the guest's host namespace";
`DEVICES.md`'s "Host contract" rows and "its host contract states raster
density 1"; `CAPABILITIES.md` §11's "host contract" for a worker's limits;
`WALKTHROUGH.md` "Needs Bun on its host"; link §7.9 and §7.11 as first
drafted ("the host keeps no table of models", "A host on the device"). Each
now says which. *Recommendation*: as proposed.

**Q3. The setup guide's risk rule.** A guide is instructions an agent follows
with the owner's rights, as powerful as `native` (`SETUP.md` §4). Options:
(a) treat a guide from a non-official key as `native`: the store's editor
reads it, the install sheet names it and the publisher, and the owner's click
is required; (b) disclose it on the install sheet and nothing more; (c) allow
guides from the official key alone. *Recommendation*: (a).

**Q4. Granting a private origin is consent-class.** `services.grantOrigin`
widens what an enclosed service can reach (a Home Assistant at
`http://192.168.1.20:8123`); the agent asks with `services.requestOrigin`, and
the owner clicks in the app (`SETUP.md` §3). Options: consent-class, as
proposed; any non-service client, like `services.setup`. *Recommendation*:
consent-class. Not ruled on.

**Q5. The control socket's other privileged methods.** Consent is decided.
Still proposed (`TRUST.md` §2): the peer check refusing services and their
descendants; `services.install` of a development release from identified
clients only; `pairing.answer` (which gives a device a link key) is open to
any local client today and could join the identified class. *Recommendation*:
the peer check and identified development installs; leave `pairing.answer`
open, since the owner types the code the device shows.

**Q6. Where the device states what it takes.** `channel.open` says nothing
about the device, and only the device knows whether a 3DS has the H.264
decoder. Options: (a) `EVENT device.accepts` per connection (link §7.9);
(b) the same object in every `channel.open`; (c) a table of targets in the
host. *Recommendation*: (a).

**Q7. Versions, and what an existing home meets.** Pocket Hub 0.2.1 is
released, so homes with `<home>/daemons/`, `<home>/daemon-data/` and
`daemon.sock` exist, and a resident core of that version may be running when
a newer app starts. Proposed (`MIGRATION.md` §3): `protocol` 2 for both the
control API and the local protocol; the first core of step 1 moves
`daemons/` and `daemon-data/` to their new names once, logged; from step 2 a
package installed under the old manifest is listed `failed` ("installed by an
earlier Pocket Hub; install it again") and kept until the owner removes it; a
newer app that meets an older core (refused at `hub.hello`) asks it to shut
down with the old protocol's `hub.shutdown` and starts its own. *Recommendation*:
as proposed.

**Q8. `data.fs` on Omarchy Pocket**, so an applet without a service keeps its
state the same way on all three devices (`DEVICES.md` §7). *Recommendation*:
yes.

**Q9. The setup inlet.** `services.setup` is open to any client that is not a
service (`SETUP.md` §2): the agent works through the command line. So any
program of the owner's user can hand a service a session of its choosing (X
signed in to another account). Options: as proposed (a same-user program can
already read the stores); make it consent-class (the agent's flow then stops
for a click). *Recommendation*: as proposed.

**Q10. Who applies `applet.hosts` on the Tab5.** Its worker's HTTP goes
through its own stack. Options: (a) the firmware applies `applet.hosts` and
the destination policy in its HTTP node; (b) it sends every request to Hub as
`net.http`. *Recommendation*: (a).

## 2. Second rank: blocks a later step

**Q11. `applet.hosts` for `net.http` on Hub.** Add `hash` to `net.http` (link
§6.5 as proposed); Hub keeps `<home>/packages.json`, `(app, footer hash) →
{version, hosts, source}`, written when it pushes or installs a package or
verifies one in its cache. A package not in it is refused with a sentence
saying to install the applet from this Mac; a request without `hash` (an
older 3DS program) with one saying to update the 3DS's software. Options
(ii) and (iv) of the earlier revision (every public destination) are the
fallback the owner's rules forbid. *Recommendation*: as proposed.

**Q12. Host lists that cannot be written in advance.** Suffix wildcards
`*.name`, refused when `name` is a public suffix (Public Suffix List, private
section included); an entry for any public host, shown as "Can reach any
website", for Podcasts if the owner wants it; private addresses only by Q4.
*Recommendation*: wildcards now; decide "any host" with Podcasts.

**Q13. Text.** `text.atlas` takes its codepoints as a body and returns
`omitted`; `channel.text` for services is named and not implemented until a
service needs it. *Recommendation*: as written.

**Q14. When the system channel opens, and the glyph capability.** Before boot
when the plan has `image.url`, `text.glyphs.deferred` or `media.url`; a new id
`text.glyphs.deferred` rather than `text.glyphs.streamed` or
`text.glyphs.runtime`. *Recommendation*: as written.

**Q15. Media.** AVFoundation and VideoToolbox on a Mac (one slice per frame
not verified), ffmpeg on Linux; `video: false` and `durationMs` for Podcasts;
sound and picture apart (format 18 for the pilot); the side connection's
listener on a port the OS assigns, a failed bind stopping the start; no
pass-through form. *Recommendation*: as written.

**Q16. Two releases at once, and the store's size.** One process per release
sharing the id's store, whose limit is the newest release's; a release that
raises it says so in its update's text. *Recommendation*: as written.

**Q17. A development install** serves every hash of its id, takes precedence
while installed, has its own store, and receives `services.setup` with
`release: "dev"`. *Recommendation*: as written.

**Q18. Key pins and lost keys.** Pins for every id; a rotation statement
signed by the old key; removal for a lost key. *Recommendation*: as written.

**Q19. The store release's form**: bundled, no install at the host.
*Recommendation*: as written.

**Q20. Installs from a console.** A release with a service installed from a
device's store page gets its service first; an unrestricted one is held until
the owner allows it in the app, `channel.open` answering `consent` meanwhile,
with a catalog row field marking such applets. *Recommendation*: as written.

**Q21. The second signature** keeps the first for Omarchy Pocket's current
devices and adds the service package's hash, `requires`, both host lists and
whether a setup guide is present. *Recommendation*: as written; the store's
owner decides the prefix.

**Q22. The macOS enclosure**: `sandbox-exec` (measured, deprecated), with an
embedded JavaScript engine as its planned replacement. *Recommendation*: as
written.

**Q23. The source check's allowed globals** (`crypto.subtle`,
`structuredClone`, `console`). *Recommendation*: as written.

**Q24. What a same-user program can still do** with the enclosure in place:
read the stores, call unprivileged control methods, call `services.setup`.
Listed so the enclosure is not read as covering more than services.

**Q25. The guest's hand-over budget with two channels. Conflict.** Link §7.3's
budgets are each channel's in the C client. *Recommendation*: one budget per
guest, in the specification change.

**Q26. Secrets in the store**: files of mode 0600 now; the Keychain or the
Secret Service later.

**Q27. A native service's files and vendor tools.** `host.native.files`;
`docs/APPLETS.md`'s vendor tool (terms, SHA-256 pin, refresh) named in the
manifest and kept by a `host.native.tool` call. *Recommendation*: before
YouTube or Spotify is released.

**Q28. YouTube's manifest**: `["image", "media", "native"]`.

**Q29. Limits sized to the applets**, none measured (`CAPABILITIES.md`).

## 3. Can wait for the step named

**Q30. `applet.services` and the store's `services` member** leave with
step 9. **Q31.** GitHub signs in by its device flow. **Q32.** Discord's gateway
stops with its service. **Q33.** Maps' `/locate` by IP geolocation through
`net`. **Q34.** The SDK examples rewritten without Bun's interfaces.
**Q35.** `channel.open` refusals `no_service`, `capability` (with `missing`),
`consent`; an older C client takes each as "not during this applet run".
**Q36.** A service host on a weak device: chooses its own host when it provides
every capability a release requires. **Q37.** The role name `companion` in
the link's fields stays. **Q38.** Owner-facing words for each new control API
kind, in English and Simplified Chinese. **Q39.** The pin move ships with a
3DS program release. **Q40.** Omarchy Pocket's Store applet becomes a client
of its service host (and of `system.apps` under Q1). **Q41.** Spotify becomes
an unrestricted service on Omarchy Pocket. **Q42.** Whether Hub ever installs
a Tab5's software (`Device.supports` has no `program` today), which decides how
a Tab5 gets the firmware with channels.
