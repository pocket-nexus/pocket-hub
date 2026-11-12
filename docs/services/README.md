# Services and hosts: the proposed contracts

**Status: a proposal for the owner to confirm. Nothing here is implemented, and
nothing here changes `docs/CONTROL-API.md` or `docs/DAEMON-PROTOCOL.md`, which
stay the contracts of the code until each change below lands.** Where a page
says "Hub does", read "Hub would do once this is confirmed and built".

The companion change to the link specification is
[pocket-nexus/pocketjs#693](https://github.com/pocket-nexus/pocketjs/pull/693),
a proposal that edits `docs/POCKET-LINK.md`: new §7.9 "The system
channel", §7.10 "Proposed renames" and §12.4 (what an implementation
touches), and marked changes to §1, §6.1, §6.5, §7.1, §7.3, §7.6, §9 and
§11. This set and that
page use the same names, messages and limits; where they meet, the link page
decides the wire and this set decides what a service sees.

## Why a set of pages, and in what order to read them

Each page is one thing the owner confirms or sends back on its own: a change
to the capability list does not reopen the manifest, and the signing rules can
wait while the API is settled. When the set is confirmed, `CAPABILITIES.md`,
`MANIFEST.md` and `HOST-PROTOCOL.md` merge with today's
`DAEMON-PROTOCOL.md` into one contract (`docs/SERVICE-PROTOCOL.md`,
`MIGRATION.md` says how); the others are decisions and stay as records.

| Page | What it settles |
| --- | --- |
| `README.md` (this page) | why, the terms, and **the naming proposal** with every file a rename touches |
| `CAPABILITIES.md` | the seven capabilities as an API: SDK signatures, semantics, limits, what Hub checks, what "the framework uses it" means, and which existing applet needs each call |
| `MANIFEST.md` | `pocket.service.json`: every field, its checks, and how it relates to the applet's contract and to today's `pocket.daemon.json` |
| `HOST-PROTOCOL.md` | the new direction of the local protocol: a service asking Hub for a capability, frame by frame; what of today's protocol goes away |
| `TRUST.md` | the sandbox (the checking tool, the macOS profile, measured), `native`, the end of the control socket's exposure; identity and signing; what the store must add |
| `WALKTHROUGH.md` | GitHub, X and YouTube end to end: manifests, calls, and what the owner sees |
| `MIGRATION.md` | the changes to `CONTROL-API.md` and `DAEMON-PROTOCOL.md`, as diffs in words, and an order of work that keeps the tree working at every step |
| `QUESTIONS.md` | **every point the direction leaves open or that conflicts with existing code or the specification**, each with options and a recommendation |

## 1. Why

The UI loop of a device runs at 60 frames a second whatever happens. What a
device cannot do itself in that budget (decode an image, transcode a video,
rasterize a script it has no font for, keep an applet's back end with its
sign-ins) is done by another machine it is paired with, and content that is
missing because of the delay is a state the framework shows: loading, ready
or unavailable. PocketJS's `docs/OFFLOAD.md` states that the same request
contract could be served by a provider on the device itself
(`OffloadOps.local` in `contracts/spec/offload.ts` is that seam); it was never
built.

These pages turn four rules into contracts:

1. **A service depends only on a bounded set of host capabilities.** That is
   what lets a device strong enough run the same service code locally that a
   3DS reaches on a Mac. Today a daemon is a Bun program that can do anything
   its user can; nothing makes it movable.
2. **One capability list does three jobs**: it is the sandbox's permission
   table (a service gets what its manifest names, checked by Hub on every
   call), the test of whether a service can move to another host (the host
   provides every name it requires), and the base of the framework's general
   capabilities (an applet without a service uses `image`, `text` and `media`
   through the framework).
3. **Device differences live in the host's capabilities, never in a
   service.** The YouTube service of the earlier Hub knew the 3DS's H.264
   profile and PAL8 format; under this design a service asks `channel.media`
   for a stream and the host decides what the device at the other end of that
   channel can play, from facts the device states itself
   (`device.accepts`, link specification §7.9 as proposed).
4. **Hub is a service's only exit.** A portable service reaches the network,
   storage and the owner only through the SDK, and Hub does each thing after
   checking it against the manifest.

The connection layer, the process supervisor and the channel relay that exist
are kept: this is an addition of one connection kind (`HOST-PROTOCOL.md`) and
of capability providers in Hub, not a rewrite.

## 2. Terms

| Term | Meaning |
| --- | --- |
| **applet** | a Pocket applet package (`.pocket`) as today: the program a device runs |
| **service** | what `DAEMON-PROTOCOL.md` calls a daemon: the code of one applet that runs on a host, reached through the applet's channel |
| **host** | what runs services and implements capabilities. **Hub is the first host.** A device strong enough may later be its own host; nothing below names Hub where a host is meant |
| **capability** | one of seven named things a host provides (`net`, `store`, `signin`, `image`, `text`, `media`, `native`) |
| **portable service** | a service whose manifest does not require `native`: it runs on any host that provides what it requires, inside the sandbox |
| **unrestricted service** | a service that requires `native`: computers only, no sandbox, disclosed at install |
| **system service** | the service a host provides itself, with no applet code behind it: `image`, `text` and `media` for applets that have no service. Hub implements it in-process, in Rust |
| **system channel** | the second channel a device opens, to the system service (link specification §7.9 as proposed) |
| **release** | one published version of an applet: its package, and its service package when it has one, signed together (`TRUST.md`) |
| **owner** | the person whose computer and device these are. Owner-facing text never says service, host, capability, channel or native |

## 3. Naming proposal (needs the owner's choice)

The direction calls several names placeholders and asks that none keep "Hub"
or "Link": the link is only the transport when the service is remote, and a
device that hosts its own services has no Hub. Each row below is a proposal
with the alternatives considered; the owner picks.

| What | Today | Proposed | Alternatives | Reason for the proposal |
| --- | --- | --- | --- | --- |
| the applet's code on a host | daemon | **service** | | the direction's term |
| what runs services | Hub (as runner) | **host** | runtime, provider | the direction's term; "provider" is taken by `io.offload` |
| the manifest file | `pocket.daemon.json` | **`pocket.service.json`** | `service.json` | parallels `pocket.app.json` |
| the manifest's version key | `"daemon": 1` | **`"service": 1`** | `"version"` | parallels the file name; `version` would read as the release's version |
| the SDK's import specifier | `@pocket-hub/daemon` | **`@pocketjs/service`** | `@pocket/service`, `pocket-service` | the SDK is the service side of a platform contract every host implements; the `@pocketjs` scope says it belongs with the framework. Hub still ships it and links its copy into each package (the owner's standing decision); nothing is published to npm |
| socket variable | `POCKET_HUB_DAEMON_SOCKET` | **`POCKET_SERVICE_SOCKET`** | | |
| token variable | `POCKET_HUB_DAEMON_TOKEN` | **`POCKET_SERVICE_TOKEN`** | | |
| applet id variable | `POCKET_HUB_DAEMON_APP` | **`POCKET_SERVICE_ID`** | `POCKET_SERVICE_APP` | a service's id is its applet's id |
| data folder variable | `POCKET_HUB_DAEMON_DATA` | **removed** | | replaced by `store` (`HOST-PROTOCOL.md` §7) |
| the applet's capability to reach its service | `link.channel` | **`service.channel`** | `app.service` | names the behaviour, not the transport |
| the guest's host namespace | `globalThis.link.channel` | **`globalThis.channels.service`**, beside **`globalThis.channels.system`** | `globalThis.service` and `globalThis.system` | two channels with the same twelve operations; `system` alone would read as Pocket System |
| the framework module | `@pocketjs/framework/channel` | **`@pocketjs/framework/service`** | keep `channel` | the module is the applet's end of its service |
| `channel.open` refusal | `no_daemon` | **`no_service`** | | wire code: changes with the spec |
| `channel.close` reason | `daemon` | **`service`** | | wire code: changes with the spec |
| handshake messages | `daemon.hello`, `daemon.ready`, `daemon.refused`, `daemon.stop` | **`service.hello`, `service.ready`, `service.refused`, `service.stop`** | | local protocol; `protocol` becomes 2 so a stale SDK copy is refused with a clear message |
| control API | `daemons.*`, `Daemon`, `daemon.changed`, `daemon.removed`, `daemon.log`, error kind `daemon` | **`services.*`, `Service`, `service.changed`, `service.removed`, `service.log`, kind `service`** | | |
| Hub's folders | `<home>/daemons/`, `<home>/daemon-data/`, `<home>/daemon.sock` | **`<home>/services/`, `<home>/service-store/`, `<home>/service.sock`** | | |
| the SDK's error class | `LinkError` | **`ServiceError`** | | same members; used for channel and capability calls |
| Hub's crate and SDK folder | `crates/hub-daemons`, `sdk/daemon` | **`crates/hub-services`, `sdk/service`** | | |
| framework capabilities served by the system service | none | **`image.url`, `text.glyphs.deferred`, `media.url`** | `image.remote`, `text.glyphs.remote`, `media.remote` | "remote" is wrong when the system service runs on the device itself; the names say what the applet gets |
| the role name `companion` in the link's fields | `companion` | **unchanged** | | the specification keeps it on purpose (§1); renaming it is out of this proposal's scope |

### Blast radius in this repository

Counted with `git grep -i daemon` on the commit this proposal starts from;
every file is touched by the local rename, none by the wire rename until the
pin moves.

| Area | Files |
| --- | --- |
| Contracts and docs | `docs/DAEMON-PROTOCOL.md` (becomes `docs/SERVICE-PROTOCOL.md`), `docs/CONTROL-API.md`, `docs/control-api-examples.json` (regenerated), `README.md`, `AGENTS.md` |
| The SDK | `sdk/daemon/` → `sdk/service/`: `package.json`, `bun.lock`, `README.md`, `src/index.ts`, `src/serve.ts`, `src/channel.ts`, `src/connection.ts`, `src/frame.ts`, `src/streams.ts`, `src/errors.ts`; `test/mock-hub.ts`, `test/channel.test.ts`, `test/lifecycle.test.ts`, `test/examples.test.ts`, `test/frames.test.ts`, `test/helpers.ts`, `test/child.ts`, `test/fixtures/lifecycle-daemon.ts`; `examples/sample/` and `examples/link-channel/` (`daemon.ts`, `pocket.daemon.json`, `http-fixture.ts`) |
| The supervisor | `crates/hub-daemons/` → `crates/hub-services/`: `src/lib.rs`, `manifest.rs`, `registry.rs`, `supervisor.rs`, `instance.rs`, `channel.rs`, `config.rs`, `child.rs`, `logs.rs`; `tests/registry.rs`, `tests/supervisor.rs`, `tests/common/mod.rs`, `tests/fixtures/daemon.mjs`; the workspace `Cargo.toml` and `Cargo.lock` |
| The endpoint | `crates/hub-core/src/api.rs` (`ChannelOpenError::NoDaemon`, comments), `relay.rs`, `conn.rs`; `crates/hub-core/tests/channel.rs`, `tests/common/mod.rs`; `crates/pocket-link/src/frame.rs` (two comments) |
| The core and its clients | `crates/hubd/src/core.rs`, `handlers.rs`, `map.rs`, `sdk.rs`, `host.rs`, `server.rs`, `main.rs`, `lib.rs`, `Cargo.toml`; `crates/hubd/tests/e2e.rs`, `tests/common/mod.rs`; `crates/hub-api/src/types.rs`, `lib.rs`, `rpc.rs`; `crates/hub-cli/src/main.rs`, `output.rs`, `tests/cli.rs` |
| The app | `app/Sources/PocketHub/DaemonsView.swift`, `ContentView.swift`, `LogViews.swift`; `app/Sources/HubAppModel/HubModel.swift`, `UserFacingError.swift`, `Presentation.swift`; `app/Sources/HubClient/Models.swift`, `HubMethods.swift`, `HubClient.swift`, `Events.swift`, `HubError.swift`; `app/Sources/MockHubCore/MockHubCore.swift`, `MockScenario.swift`; `app/Tests/HubClientTests/DecodingTests.swift`, `ExamplesTests.swift`; `app/Tests/HubAppModelTests/HubModelTests.swift`; `app/Localization/Localizable.xcstrings`; `app/scripts/translations_zh_hans.py` |

### Blast radius in PocketJS

Counted on PocketJS `main` at `48ca98b4`. The link page lists the same files
in its §7.10.

| Name | Files |
| --- | --- |
| `link.channel` (capability, namespace, feature bit) | `contracts/spec/applet.ts`, `contracts/spec/platform-types.ts`, `hosts/3ds-dev/pocket.host.json`, `products/3ds/include/pocket_profile.h` (generated), `products/3ds/core/src/lib.rs` (`POCKET_GUEST_FEATURE_CHANNEL`), `products/3ds/include/pocket_core.h`, `products/3ds/src/link2_host.h`, `products/3ds/src/qjs.c`, `products/3ds/README.md`, `engine/host-services/link2_channel_qjs.{c,h}`, `engine/host-services/README.md`, `framework/src/channel.ts`, `tools/pocket-link/v2/hub.ts`, `tools/repack/3ds.ts`, `apps/link-channel/{pocket.app.json,daemon.ts,http-fixture.ts}`, `site/content/docs/app-contract.md`, `tests/e2e/azahar-link2.ts`, `tests/platform-contracts.test.ts`, `docs/POCKET-LINK.md` |
| `no_daemon`, close reason `daemon` | `contracts/spec/link2.ts` (`LINK2_CHANNEL_OPEN_ERROR`, `LINK2_CHANNEL_CLOSE`), `engine/host-services/link2.c`, `tools/pocket-link/v2/{device,hub,vectors}.ts`, `tests/vectors/pocket-link-2/channel.json`, `tests/pocket-link-2-client.test.ts`, `docs/POCKET-LINK.md` |
| "daemon" as the term (reference API, CLI flag `--daemon`, comments) | `tools/pocket-link/v2/{daemon,daemon-sample,cli,hub,device,peer,credit,requests,vectors}.ts`, `tools/pocket-link/protocol.ts`, `engine/host-services/link2.{c,h}`, `framework/src/channel.ts`, `apps/link-channel/{app.tsx,daemon.ts,http-fixture.ts}`, `tests/pocket-link-2{,-client,-reference}.test.ts`, `tests/pocket-link.test.ts`, `tests/fixtures/link2-client-harness.c`, `tests/e2e/azahar-link2.ts`, `docs/POCKET-LINK.md` |
| `pocket.daemon.json`, `@pocket-hub/daemon`, `POCKET_HUB_DAEMON_*` | none: these names exist only in this repository |

### Blast radius in pocket-nexus/applets

Every applet that uses `https://companion.pocket/<service>` is rewritten
anyway (no compatibility with that origin, a standing decision):
`lib/net.ts` (`COMPANION_ORIGIN`, `companionService`), `lib/store.ts`,
`lib/mock.ts`, `ui/companion.ts`, `ui/glyphs.ts`, `ui/setup.tsx`,
`kit/images.ts`, `kit/platform.tsx`, `tools/check.ts` (its `companionService`
scan), the `api.ts` of `github`, `weather`, `x`, `discord`, `homeassistant`,
`maps`, `podcasts`, `youtube`, `spotify`, `appstore`, the fixtures
`test/x-fixture.ts` and `test/discord-fixture.ts`, `spotify/mock/make.ts`,
and the docs `docs/applets.md`, `docs/X.md`, `docs/DISCORD.md`.
