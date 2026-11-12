# Services and hosts: the proposed contracts

**Status: a proposal for the owner to confirm. Nothing here is implemented, and
nothing here changes `docs/CONTROL-API.md` or `docs/DAEMON-PROTOCOL.md`, which
stay the contracts of the code until each change below lands.** Where a page
says "Hub does", read "Hub would do once this is confirmed and built".

The companion change to the link specification is
[pocket-nexus/pocketjs#693](https://github.com/pocket-nexus/pocketjs/pull/693),
a proposal that edits `docs/POCKET-LINK.md`: new §7.9 "The system channel",
§7.10 "Proposed renames", §7.11 "A host on the device, and the three devices"
and §12.4 (what an implementation touches), and marked changes to §1, §5.3,
§6.1, §6.5, §7.1, §7.3, §7.6, §9 and §11. This set and that page use the
same names, messages and limits; the link page decides the wire, this set
decides what a service sees and what a host does.

**Revision 3** (2026-11-12) folds in the owner's four decisions (Omarchy
Pocket becomes a host and every device takes the new path; the control
socket is closed in this work; the LAN attacker without the key is accepted
until link encryption; decoding is performance-first) and an independent
review. `QUESTIONS.md` lists what changed at its top.

## The picture

The applets of pocket-nexus/applets are built from one source for three
devices. **Every one takes the new path; none keeps `companion.pocket`.**

| | Nintendo 3DS | M5Stack Tab5 | Omarchy Pocket |
| --- | --- | --- | --- |
| Where its host is | Hub on a paired Mac | Hub on a paired Mac | on the device |
| Its channels travel on | Pocket Link 2 | Pocket Link 2, once its firmware speaks it | a local Unix socket (link §7.11) |
| Its worker's HTTP | through Hub | its own stack, which applies `applet.hosts` itself | through the host on the device |
| `native` | on the Mac | on the Mac | on the device |

**Identical everywhere**: what an applet sees of its service and of the
system service (layer a); the SDK and the protocol between a service and its
host, so one service bundle runs on any host that provides what it requires
(layer b); the capabilities' API and limits (layer c). **What differs**: the
transport under the channels, which capabilities each host provides
(Omarchy Pocket has no web view for `signin.web` today), how a host
implements them, and how each device installs and pairs. `DEVICES.md` has
the detail.

## Why a set of pages, and in what order to read them

Each page is one thing the owner confirms or sends back on its own. When the
set is confirmed, `CAPABILITIES.md`, `MANIFEST.md` and `HOST-PROTOCOL.md`
merge with today's `DAEMON-PROTOCOL.md` into one contract
(`docs/SERVICE-PROTOCOL.md`, `MIGRATION.md` says how); the others are
decisions and stay as records.

| Page | What it settles |
| --- | --- |
| `README.md` (this page) | the picture, why, the terms, and **the naming proposal** with every file a rename touches |
| `DEVICES.md` | the three devices and three layers; which host provides which capability; **who implements Omarchy Pocket's host**; `native` there; the Linux enclosure; state without a service; the Tab5's path |
| `CAPABILITIES.md` | the seven capabilities as an API: SDK signatures, semantics, limits, what the host checks, what "the framework uses it" means, and which existing applet needs each call |
| `MANIFEST.md` | `pocket.service.json`: every field, its checks, and how it relates to the applet's contract and to today's `pocket.daemon.json` |
| `HOST-PROTOCOL.md` | layer (b): a service asking its host for a capability, frame by frame; each host's folders; what of today's protocol goes away |
| `TRUST.md` | the enclosure (macOS measured, Linux proposed), **closing the control socket**, `native`, the source check; identity and signing; what the store must add; what is covered |
| `WALKTHROUGH.md` | GitHub, X and YouTube end to end on the three devices |
| `MIGRATION.md` | the changes to `CONTROL-API.md` and `DAEMON-PROTOCOL.md`, and an order of work that says what each device runs at each step |
| `QUESTIONS.md` | every point left open, ordered so the questions that block the first steps come first |

## 1. Why

The UI loop of a device runs at 60 frames a second whatever happens. What a
device cannot do itself in that budget (decode an image, transcode a video,
rasterize a script it has no font for, keep an applet's back end with its
sign-ins) is done by a host (another machine it is paired with, or the
device itself when it is strong enough), and content that is missing because
of the delay is a state the framework shows: loading, ready or unavailable.
PocketJS's `docs/OFFLOAD.md` states that the same request contract could be
served by a provider on the device itself (`OffloadOps.local` in
`contracts/spec/offload.ts` is that seam); Omarchy Pocket is where this
proposal builds it.

These pages turn four rules into contracts:

1. **A service depends only on a bounded set of host capabilities.** That is
   what lets Omarchy Pocket run the same service code locally that a 3DS
   reaches on a Mac. Today a daemon is a Bun program that can do anything its
   user can.
2. **One capability list does three jobs**: the enclosure's permission table
   (a service gets what its manifest names, checked by the host on every
   call), the test of whether a service can run on a host (the host provides
   every name it requires), and the base of the framework's general
   capabilities (an applet without a service uses `image`, `text` and `media`
   through the framework).
3. **Device differences live in the host's capabilities, never in a
   service.** The earlier YouTube service knew the 3DS's H.264 profile and
   PAL8 format; here a service asks `channel.media` for a stream and the host
   decides what the device at the other end can play, from facts the device
   states itself (`device.accepts`, link §7.9).
4. **The host is a service's only exit.** A portable service reaches the
   network, storage and the owner only through the SDK, and the host does
   each thing after checking it against the manifest.

The connection layer, the process supervisor and the channel relay that exist
are kept and become the host's (`DEVICES.md` §4).

## 2. Terms

| Term | Meaning |
| --- | --- |
| **applet** | a Pocket applet package (`.pocket`): the program a device runs |
| **service** | what `DAEMON-PROTOCOL.md` calls a daemon: the code of one applet that runs on a host, reached through the applet's channel |
| **host** | what runs services and implements capabilities: **Hub** on a Mac for the 3DS and the Tab5; **the host on Omarchy Pocket** for that device |
| **capability** | one of seven named things a host provides (`net`, `store`, `signin`, `image`, `text`, `media`, `native`) |
| **portable service** | a service whose manifest does not require `native`: it runs on any host that provides what it requires, inside the enclosure |
| **unrestricted service** | a service that requires `native`: hosts that are general-purpose computers the owner controls, no enclosure, disclosed at install |
| **system service** | the service a host provides itself, with no applet code behind it: `image`, `text` and `media` for applets |
| **system channel** | the second channel a device opens, to the system service (link §7.9) |
| **local connection** | the transport of a device's channels to a host on the same machine: a Unix socket with the link's frames (link §7.11) |
| **release** | one published version of an applet: its package, and its service package when it has one, signed together (`TRUST.md` §6) |
| **owner** | the person whose computer and devices these are. Owner-facing text never says service, host, capability, channel or native |

## 3. Naming proposal (needs the owner's choice)

The direction asks that no placeholder name keep "Hub" or "Link": the link is
only the transport when the host is remote, and Omarchy Pocket has no Hub.

| What | Today | Proposed | Alternatives | Reason |
| --- | --- | --- | --- | --- |
| the applet's code on a host | daemon | **service** | | the direction's term |
| what runs services | Hub (as runner) | **host** | runtime, provider | "provider" is taken by `io.offload` |
| the manifest file | `pocket.daemon.json` | **`pocket.service.json`** | `service.json` | parallels `pocket.app.json` |
| the manifest's version key | `"daemon": 1` | **`"service": 1`** | `"version"` | `version` would read as the release's version |
| the SDK's import specifier | `@pocket-hub/daemon` | **`@pocketjs/service`** | `@pocket/service`, `pocket-service` | the service side of a contract every host implements; each host ships and links its copy; nothing is published to npm |
| socket variable | `POCKET_HUB_DAEMON_SOCKET` | **`POCKET_SERVICE_SOCKET`** | | |
| token variable | `POCKET_HUB_DAEMON_TOKEN` | **`POCKET_SERVICE_TOKEN`** | | |
| applet id variable | `POCKET_HUB_DAEMON_APP` | **`POCKET_SERVICE_ID`** | `POCKET_SERVICE_APP` | a service's id is its applet's id |
| data folder variable | `POCKET_HUB_DAEMON_DATA` | **removed** | | replaced by `store` (`CAPABILITIES.md` §5) |
| the applet's capability to reach its service | `link.channel` | **`service.channel`** | `app.service` | names the behaviour, not the transport |
| the guest's host namespace | `globalThis.link.channel` | **`globalThis.channels.service`**, beside **`globalThis.channels.system`** | `globalThis.service`, `globalThis.system` | `system` alone would read as Pocket System |
| the framework module | `@pocketjs/framework/channel` | **`@pocketjs/framework/service`** | keep `channel` | |
| `channel.open` refusal | `no_daemon` | **`no_service`** | | wire code |
| `channel.close` reason | `daemon` | **`service`** | | wire code |
| handshake messages | `daemon.hello`, `.ready`, `.refused`, `.stop` | **`service.hello`, `.ready`, `.refused`, `.stop`** | | `protocol` becomes 2 |
| control API | `daemons.*`, `Daemon`, `daemon.changed`, `.removed`, `.log`, kind `daemon` | **`services.*`, `Service`, `service.*`, kind `service`** | | `hub.hello`'s protocol becomes 2 (`QUESTIONS.md` Q10) |
| Hub's folders | `<home>/daemons/`, `<home>/daemon-data/`, `<home>/daemon.sock` | **`<home>/services/`, `<home>/service-store/`, `<home>/service.sock`** | | |
| the SDK's error class | `LinkError` | **`ServiceError`** | | same members |
| Hub's crate and SDK folder | `crates/hub-daemons`, `sdk/daemon` | **`crates/hub-services`**, the host parts moving to **`crates/pocket-host`** (`DEVICES.md` §4); **`sdk/service`** | | |
| framework capabilities served by the system service | none | **`image.url`, `text.glyphs.deferred`, `media.url`** | `image.remote`, … | "remote" is wrong when the host is on the device |
| the console's link itself: capability `link.companion`, `globalThis.link`, `@pocketjs/framework/link` | as is | **unchanged** | rename them too | they name the link itself (pairing, status, catalog), not where an applet's code runs (`QUESTIONS.md` Q1) |
| the role name `companion` in the link's fields | `companion` | **unchanged** | | the specification keeps it on purpose (§1) |

### Blast radius in this repository

**Generated with `git grep -il daemon` at `806022f`, excluding `Cargo.lock`:
101 files**, every one touched by the local rename; none by the wire rename
until the pin moves.

| Area | Files |
| --- | --- |
| Top level and docs | `AGENTS.md`, `README.md`, `Cargo.toml`, `docs/CONTROL-API.md`, `docs/DAEMON-PROTOCOL.md` (becomes `SERVICE-PROTOCOL.md`), `docs/control-api-examples.json` (regenerated) |
| The SDK, `sdk/daemon/` → `sdk/service/` | `README.md`, `package.json`, `bun.lock`; `src/{channel,connection,errors,frame,index,serve,streams}.ts`; `test/{channel.test,child,examples.test,frames.test,helpers,lifecycle.test,mock-hub}.ts`, `test/fixtures/lifecycle-daemon.ts`; `examples/link-channel/{daemon.ts,http-fixture.ts,pocket.daemon.json}`, `examples/sample/{daemon.ts,pocket.daemon.json}` |
| The supervisor, `crates/hub-daemons/` | `Cargo.toml`; `src/{archive,channel,child,config,instance,lib,logs,manifest,registry,runtime,supervisor,util,wire}.rs`; `tests/{registry,runtime,supervisor}.rs`, `tests/common/mod.rs`, `tests/fixtures/daemon.mjs`, `tests/fixtures/with-lock/pocket.daemon.json` |
| The endpoint, `crates/hub-core/` | `src/{api,carrier,conn,endpoint,lib,ops,reading,relay}.rs`; `tests/channel.rs`, `tests/common/mod.rs`, `tests/harness/device.ts`; and `crates/pocket-link/src/frame.rs` |
| The core and its clients | `crates/hubd/` `Cargo.toml`, `src/{core,handlers,host,lib,main,map,sdk,server}.rs`, `tests/e2e.rs`, `tests/common/mod.rs`; `crates/hub-api/src/{lib,rpc,types}.rs`; `crates/hub-cli/src/{main,output}.rs`, `tests/cli.rs` |
| The app | `app/Sources/PocketHub/{AppState,ContentView,DaemonsView,DebugCommands,LogViews,SettingsView}.swift`; `app/Sources/HubAppModel/{HubModel,Presentation,UserFacingError}.swift`; `app/Sources/HubClient/{Events,HubClient,HubError,HubMethods,Models}.swift`; `app/Sources/MockHubCore/{MockHubCore,MockScenario}.swift`; `app/Sources/mock-hubd/main.swift`; `app/Tests/HubAppModelTests/HubModelTests.swift`; `app/Tests/HubClientTests/{DecodingTests,ExamplesTests,HubClientTests}.swift`; `app/Localization/Localizable.xcstrings`; `app/scripts/translations_zh_hans.py` |

### Blast radius in PocketJS

At `48ca98b4`; the link page lists the same in its §7.10.

| Name | Files |
| --- | --- |
| `link.channel` | `contracts/spec/{applet,platform-types}.ts`, `hosts/3ds-dev/pocket.host.json`, `products/3ds/include/{pocket_profile,pocket_core}.h`, `products/3ds/core/src/lib.rs`, `products/3ds/src/{link2_host.h,qjs.c}`, `products/3ds/README.md`, `engine/host-services/link2_channel_qjs.{c,h}`, `engine/host-services/README.md`, `framework/src/channel.ts`, `tools/pocket-link/v2/hub.ts`, `tools/repack/3ds.ts`, `apps/link-channel/{pocket.app.json,daemon.ts,http-fixture.ts}`, `site/content/docs/app-contract.md`, `tests/e2e/azahar-link2.ts`, `tests/platform-contracts.test.ts`, `docs/POCKET-LINK.md` |
| `no_daemon`, close reason `daemon` | `contracts/spec/link2.ts`, `engine/host-services/link2.c`, `tools/pocket-link/v2/{device,hub,vectors}.ts`, `tests/vectors/pocket-link-2/channel.json`, `tests/pocket-link-2-client.test.ts`, `docs/POCKET-LINK.md` |
| "daemon" as a term | `tools/pocket-link/v2/{daemon,daemon-sample,cli,hub,device,peer,credit,requests,vectors}.ts`, `tools/pocket-link/protocol.ts`, `engine/host-services/link2.{c,h}`, `framework/src/channel.ts`, `apps/link-channel/{app.tsx,daemon.ts,http-fixture.ts}`, `tests/pocket-link-2{,-client,-reference}.test.ts`, `tests/pocket-link.test.ts`, `tests/fixtures/link2-client-harness.c`, `tests/e2e/azahar-link2.ts` |
| `applet.services`, `companion.pocket` | `contracts/spec/{applet,link}.ts`, `docs/{APPLETS,WORKER,POCKET-LINK}.md`, `site/content/docs/{app-contract,applets}.md`, `tools/pocket-link/catalog.ts`, `tools/pocket-link/v2/cli.ts` |
| `pocket.daemon.json`, `@pocket-hub/daemon`, `POCKET_HUB_DAEMON_*` | none: these exist only in this repository |

### Blast radius in the other repositories

| Repository | What changes |
| --- | --- |
| pocket-nexus/applets | every applet that uses `companion.pocket` is rewritten (no compatibility, a standing decision): `lib/{net,store,mock}.ts`, `ui/{companion,glyphs}.ts`, `ui/setup.tsx`, `kit/{images,platform}.ts*`, `tools/check.ts`, the `api.ts` of `github`, `weather`, `x`, `discord`, `homeassistant`, `maps`, `podcasts`, `youtube`, `spotify`, `appstore`, `test/{x,discord}-fixture.ts`, `spotify/mock/make.ts`, `docs/{applets,X,DISCORD}.md`, the `README.md` table |
| pocket-nexus/omarchy-pocket | `daemon/companion.ts` and the service half of `daemon/appstore.ts` give way to the host (`DEVICES.md` §4); `host/src/{net,children}.rs` (the local connection); `host/pocket.host.json` (the new capabilities and `data.fs`); the `vendor/pocket-hub` submodule goes |
| pocket-nexus/pocket-m5 | `components/pocket_system` (`link.c`, `link_http.c`, `link_proto.c` and `companion.c` → version 2's client), its HTTP node (`applet.hosts`), `programs/m5stack-tab5/pocket.host.json` (`DEVICES.md` §8) |
