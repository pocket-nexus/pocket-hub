# Services and hosts: the proposed contracts

**Status: a proposal for the owner to confirm. Nothing here is implemented, and
nothing here changes `docs/CONTROL-API.md` or `docs/DAEMON-PROTOCOL.md`, which
stay the contracts of the code until each change below lands.** Where a page
says "Hub does", read "Hub would do once this is confirmed and built". The
pages are read against this repository at `1bc2074` (Pocket Hub 0.2.1), its
PocketJS pin `89e1043` and its applets pin `a629fda`.

The companion change to the link specification is
[pocket-nexus/pocketjs#693](https://github.com/pocket-nexus/pocketjs/pull/693),
a proposal on PocketJS `main` at `2072729`: new §7.9 "The system channel",
§7.10 "Proposed renames" (with the link's guest surface split by concern),
§7.11 "A service host on the device, and the three devices" and §12.4, and
marked changes to §1, §5.3, §6.1, §6.5, §7.1, §7.3, §7.6, §9 and §11. The
link page decides the wire; this set decides what a service sees and what a
service host does.

**Revision 4** (2026-11-17) applies the owner's answers (`QUESTIONS.md`
"Decided": the names, one host for Omarchy Pocket, consent as the owner's
click, the rule for a device with no measured enclosure, **`signin` replaced
by a setup guide**) and rebases on a `main` that moved: the Tab5 now links
over Pocket Link 2 and pairs by a code, and Pocket Hub has released users.

## The picture

| | Nintendo 3DS | M5Stack Tab5 | Omarchy Pocket |
| --- | --- | --- | --- |
| Where its service host is | Hub on a paired Mac | Hub on a paired Mac | on the device (`pocket-hostd`) |
| Its channels travel on | Pocket Link 2 | Pocket Link 2, once its firmware opens channels | a local Unix socket (link §7.11) |
| Pairing | Hub writes it (ftpd or SD card) | by a code it shows, or on its card | none |
| Its worker's HTTP | through Hub | its own stack, which applies `applet.hosts` | through the service host on the device |
| `native` | on the Mac | on the Mac | on the device |
| An applet's setup | an agent on the Mac follows the guide | an agent on the Mac | the agent on the device |

**Identical everywhere**: what an applet sees of its service and of the system
service (layer a); the SDK and the protocol between a service and its host
(layer b), so one service bundle runs on any host that provides what it
requires; the six capabilities' API and limits (layer c). **What differs**:
the transport, how a host implements a capability (media is per platform),
how a device pairs and installs, and where the setup's agent runs.
`DEVICES.md` has the detail.

## The pages

| Page | What it settles |
| --- | --- |
| `README.md` (this page) | the picture, why, the terms, the naming, and every file a rename touches |
| `DEVICES.md` | the three devices and three layers; which host provides what; Omarchy Pocket's host (decided: one host); `native` there; the Linux enclosure; state without a service; the Tab5 |
| `CAPABILITIES.md` | the six capabilities as an API, and which existing applet needs each call |
| `SETUP.md` | the setup guide, the inlet that hands its result to the service, granted origins, and the guide's risk |
| `MANIFEST.md` | `pocket.service.json` |
| `HOST-PROTOCOL.md` | layer (b), frame by frame, and each host's folders |
| `TRUST.md` | the enclosure, closing the control socket, consent, `native`, the source check, signing, the store's changes, what is covered |
| `WALKTHROUGH.md` | GitHub, X, YouTube, Discord and Home Assistant on the three devices |
| `MIGRATION.md` | the control API and protocol diffs, and an order of work that says what each device and an existing home meet at each step |
| `QUESTIONS.md` | what is decided, and what is left, the first rank first |

When the set is confirmed, `CAPABILITIES.md`, `MANIFEST.md`, `SETUP.md` §1 and
§2 and `HOST-PROTOCOL.md` merge with today's `DAEMON-PROTOCOL.md` into
`docs/SERVICE-PROTOCOL.md`; the others stay as records.

## 1. Why

The UI loop of a device runs at 60 frames a second whatever happens. What a
device cannot do itself in that budget (decode an image, transcode a video,
rasterize a script it has no font for, keep an applet's back end with its
accounts) is done by a service host, another machine it is paired with or the
device itself when it is strong enough, and content missing because of the
delay is a state the framework shows: loading, ready or unavailable.
PocketJS's `docs/OFFLOAD.md` states that the same request contract could be
served on the device itself (`OffloadOps.local`); Omarchy Pocket is where this
proposal builds it.

1. **A service depends only on a bounded set of host capabilities**, so the
   same service code runs on Omarchy Pocket and on a Mac for a 3DS.
2. **One capability list does three jobs**: the enclosure's permission table,
   the test of whether a service can run on a host, and the base of the
   framework's general capabilities.
3. **Device differences live in the host's capabilities, never in a
   service**: the device states what it takes (`device.accepts`).
4. **The host is a service's only exit.**

Accounts are not a capability: an agent the owner runs does the setup outside
the host, from a guide the release ships (`SETUP.md`).

## 2. Terms

| Term | Meaning |
| --- | --- |
| **applet** | a Pocket applet package (`.pocket`) |
| **service** | the code of one applet that runs on a service host (today a daemon) |
| **service host**, "host" on these pages | what runs services and implements capabilities: Hub on a Mac for the 3DS and the Tab5, `pocket-hostd` on Omarchy Pocket. The SDK's object is `host` |
| **runtime** | the device's own native program (Pocket Shell on a 3DS, the Tab5's firmware, omarchy-pocket-host). PocketJS calls it "host" in "host contract", "host namespace" and `hostAbi`; these pages write those out in full (`QUESTIONS.md` Q2) |
| **capability** | one of six: `net`, `store`, `image`, `text`, `media`, `native` |
| **portable service**, **unrestricted service** | without `native`, enclosed; with `native`, computers only, disclosed |
| **system service**, **system channel** | the service a host provides itself, and the device's second channel to it |
| **setup guide** | instructions in a release for a coding agent on the host's machine (`SETUP.md`) |
| **release** | one published version of an applet: its package and its service package, signed together |
| **owner** | the person whose computer and devices these are. Owner-facing text never says service, host, capability, channel or native |

## 3. Naming

**Decided on 2026-11-17**: every row below but the last two.

| What | Today | Name |
| --- | --- | --- |
| the applet's code on a host | daemon | **service** |
| what runs services | Hub (as runner) | **host** (in prose that could mean the runtime: **service host**) |
| the manifest file, its version key | `pocket.daemon.json`, `"daemon": 1` | **`pocket.service.json`**, **`"service": 1`** |
| the SDK's import specifier, error class | `@pocket-hub/daemon`, `LinkError` | **`@pocketjs/service`**, **`ServiceError`** |
| the variables | `POCKET_HUB_DAEMON_SOCKET`, `_TOKEN`, `_APP`, `_DATA` | **`POCKET_SERVICE_SOCKET`**, **`_TOKEN`**, **`POCKET_SERVICE_ID`**; `_DATA` removed |
| the applet's capability to reach its service | `link.channel` | **`service.channel`** |
| the guest's channel namespaces | `globalThis.link.channel` | **`globalThis.channels.service`**, **`globalThis.channels.system`** |
| the framework module | `@pocketjs/framework/channel` | **`@pocketjs/framework/service`** |
| wire codes | `no_daemon`, close reason `daemon` | **`no_service`**, **`service`** |
| handshake messages | `daemon.hello`, `.ready`, `.refused`, `.stop` | **`service.hello`**, … (`protocol` 2) |
| control API | `daemons.*`, `Daemon`, `daemon.*` events, kind `daemon` | **`services.*`**, **`Service`**, **`service.*`**, kind **`service`** (`protocol` 2) |
| Hub's folders | `<home>/daemons/`, `daemon-data/`, `daemon.sock` | **`<home>/services/`**, **`service-store/`**, **`service.sock`** |
| Hub's crates and SDK folder | `crates/hub-daemons`, `sdk/daemon` | **`crates/hub-services`** with the host parts in **`crates/pocket-host`**; **`sdk/service`** |
| framework capabilities of the system service | none | **`image.url`**, **`text.glyphs.deferred`**, **`media.url`** |
| the console's link: `link.companion`, `globalThis.link`, `@pocketjs/framework/link` | as is | **not decided**: a split by concern is proposed (`QUESTIONS.md` Q1) |
| the role name `companion` in the link's fields | `companion` | unchanged |

### Blast radius in this repository

**Generated with `git grep -il daemon` at `1bc2074`, excluding `Cargo.lock`:
101 files**, every one touched by the local rename. Another branch is doing
that rename (`MIGRATION.md` step 1); this list is the proposal's, not that
branch's diff.

| Area | Files |
| --- | --- |
| Top level and docs | `AGENTS.md`, `README.md`, `Cargo.toml`, `docs/CONTRIBUTING.md`, `docs/CONTROL-API.md`, `docs/DAEMON-PROTOCOL.md`, `docs/control-api-examples.json` |
| The SDK, `sdk/daemon/` | `README.md`, `package.json`, `bun.lock`; `src/{channel,connection,errors,frame,index,serve,streams}.ts`; `test/{channel.test,child,examples.test,frames.test,helpers,lifecycle.test,mock-hub}.ts`, `test/fixtures/lifecycle-daemon.ts`; `examples/link-channel/{daemon.ts,http-fixture.ts,pocket.daemon.json}`, `examples/sample/{daemon.ts,pocket.daemon.json}` |
| The supervisor, `crates/hub-daemons/` | `Cargo.toml`; `src/{archive,channel,child,config,instance,lib,logs,manifest,registry,runtime,supervisor,util,wire}.rs`; `tests/{registry,runtime,supervisor}.rs`, `tests/common/mod.rs`, `tests/fixtures/daemon.mjs`, `tests/fixtures/with-lock/pocket.daemon.json` |
| The endpoint | `crates/hub-core/src/{api,carrier,conn,endpoint,lib,ops,reading,relay}.rs`, `tests/channel.rs`, `tests/common/mod.rs`, `tests/harness/device.ts`; `crates/pocket-link/src/frame.rs` |
| The core and its clients | `crates/hubd/` `Cargo.toml`, `src/{core,handlers,host,lib,main,map,sdk,server}.rs`, `tests/e2e.rs`, `tests/common/mod.rs`; `crates/hub-api/src/{lib,rpc,types}.rs`; `crates/hub-cli/src/{main,output}.rs`, `tests/cli.rs` |
| The app | `app/PocketHub/{AppState,ContentView,DaemonsView,DebugCommands,LogViews,SettingsView}.swift`; `app/Sources/HubAppModel/{HubModel,Presentation,UserFacingError}.swift`; `app/Sources/HubClient/{Events,HubClient,HubError,HubMethods,Models}.swift`; `app/Sources/MockHubCore/{MockHubCore,MockScenario}.swift`; `app/Sources/mock-hubd/main.swift`; `app/Tests/HubAppModelTests/HubModelTests.swift`; `app/Tests/HubClientTests/{DecodingTests,ExamplesTests,HubClientTests}.swift`; `app/Localization/Localizable.xcstrings` |

### Blast radius elsewhere

| Repository | What changes |
| --- | --- |
| pocket-nexus/pocketjs | link §7.10's lists: `link.channel` (applet and platform contracts, the 3DS's host contract, profile and feature bit, `qjs.c`, the channel binding, the framework module, the reference, `tools/repack/3ds.ts`, `apps/link-channel`, the site's app-contract page, two tests); `no_daemon` and `daemon` (`link2.ts`, `link2.c`, the reference, the vectors, a test); "daemon" as a term (the reference API and CLI, comments); `applet.services` and `companion.pocket` (`contracts/spec/{applet,link}.ts`, `docs/{APPLETS,WORKER,POCKET-LINK}.md`, `site/content/docs/{app-contract,applets}.md`, `tools/pocket-link/catalog.ts`, `tools/pocket-link/v2/cli.ts`); the link split's list in `QUESTIONS.md` Q1 |
| pocket-nexus/applets | every applet that uses `companion.pocket`, rewritten (no compatibility): `lib/{net,store,mock}.ts`, `ui/{companion,glyphs}.ts`, `ui/setup.tsx`, `kit/{images,platform}`, `tools/check.ts`, the `api.ts` of `github`, `weather`, `x`, `discord`, `homeassistant`, `maps`, `podcasts`, `youtube`, `spotify`, `appstore`, the fixtures, `docs/{applets,X,DISCORD}.md`; each service's `setup/SKILL.md`, new |
| pocket-nexus/pocket-shell | `shells/3ds/shell/{system.ts,store.tsx,pages.tsx,scanner.tsx}` under Q1 |
| pocket-nexus/omarchy-pocket | `daemon/companion.ts` and the service half of `daemon/appstore.ts` give way to `pocket-hostd`; `host/src/{net,children}.rs`; `host/pocket.host.json` (`data.fs`, the new capabilities); the `vendor/pocket-hub` submodule goes |
| pocket-nexus/pocket-m5 | `components/pocket_system` (the channel groups and namespaces, `device.accepts`, `applet.hosts` in its HTTP node, `companion.c` gone), `programs/m5stack-tab5/pocket.host.json` |
