# Working on Pocket Hub

This page is for changing Pocket Hub itself. Using it is in the
[README](../README.md); writing an applet's service is in
[`sdk/service`](../sdk/service/README.md).

## How it is built

Pocket Hub stands between physical devices, the
[Pocket Applets](https://github.com/pocket-nexus/applets) they run, and the
tools that make applets. A device only has to reach Hub: Hub puts the runtime
on a new device (a 3DS) or pairs one that carries its own (a Tab5), keeps the
link to it, carries applets to it, performs the network requests the device
cannot make itself, and runs the computer side of an applet (its service)
beside it.

Hub and devices speak **Pocket Link 2**. The specification is PocketJS's
[`docs/POCKET-LINK.md`](https://github.com/pocket-nexus/pocketjs/blob/main/docs/POCKET-LINK.md),
pinned here as the submodule `vendor/pocketjs`, whose test vectors and reference
device this repository's tests run against. The app is for macOS on Apple
silicon; the core is portable Rust.

```
            Pocket Hub.app (SwiftUI)        pocket-hub (command line)
                      \                          /
                       \   control API: JSON-RPC, one object per line,
                        \  on a Unix socket     /        docs/CONTROL-API.md
                         v                     v
                  +---------------------------------+
                  |           pocket-hubd           |   the resident core:
                  |  devices · store · services     |   one per home folder
                  +---------------------------------+
                     |                          |
     Pocket Link 2   |                          |  local service protocol
     (TCP 8628,      |                          |  docs/SERVICE-PROTOCOL.md
      UDP beacons)   v                          v
                  devices                 one Bun process per applet's service
```

**`pocket-hubd` owns every device connection.** The app and the command line
are clients of one control API and hold no device logic, so a device is never
held by two processes and a later remote client is one more client of the same
surface.

**A device is one of two families, and Hub's core says what each takes.** A
Nintendo 3DS is set up through its storage and takes everything Hub offers. An
M5Stack Tab5 pairs on the link by a code it shows (link specification §3.11),
installs applets from its own store page, and takes no pushed package,
screenshot, log or software update from Hub. A link's feature list cannot say
this (one group covers several requests), so the table is
`crates/hub-core/src/family.rs`, each Device carries its answer (`supports`,
`CONTROL-API.md` §2), and the command line and the app hold no table of their
own.

**The applet store's website opens the app on an applet.** Each applet's page
at [applets.pocket.nexus](https://applets.pocket.nexus) has a button that is
a `pocket-hub://store/app/<id>` link (`docs/DEEP-LINKS.md`): the app comes to
the front on that applet's page in its Store, and the owner chooses a device
and installs it from there. A link only shows; it never installs by itself.

**An applet's page tries the applet on the Mac**, on the store's player: the
one the store's website runs (pocket-nexus/applets `store/play`, built into
`store/site/public/play`), pinned here as the submodule `vendor/applets` and
copied into the app by `app/scripts/embed-player.sh`. It runs the package's
Omarchy Pocket variant on PocketJS's core in WebAssembly in a sandboxed web
view and answers every request from the version's sample data
(`store.sample`), so nothing reaches a device or an account. A change to the
player lands in pocket-nexus/applets first and this repository moves its
pin.

**A service is the computer side of one applet**: a Bun process Hub starts when
a device opens that applet's channel and stops when it is idle. Applet and
service exchange requests, events and credited byte streams whose content is
theirs; Hub relays them and reads none of it. Neither learns which carrier the
device is on.

| Path | What it is |
| --- | --- |
| `crates/pocket-link` | The Pocket Link 2 wire: beacon, preamble, frames and their streaming decoder, handshake proofs, pairing files and the setup as a plan of storage operations, the messages and HMACs of a pairing by a code. No I/O |
| `crates/hub-store` | Hub's own files: its identity, one record per paired device, the channel generation |
| `crates/hub-core` | The link endpoint: carriers, beacons, the handshake, a device's request to be paired by the code it shows, requests, streams and credit, runtime operations, the device mutex, applet channels relayed to services; the device families and what each takes (`family.rs`) |
| `crates/hub-setup` | The first install of a 3DS: finding a console's ftpd, its software and the pairing written onto its storage, software releases |
| `crates/hub-services` | The Bun runtime (downloaded on demand), service packages, their processes and the service socket |
| `crates/hub-net` | `net.http`: a device's HTTP requests performed by Hub, with the destination policy |
| `crates/hub-applet-store` | The applet store's client (the catalog, verified packages, icons, and an app's details as the store reads them out of its package), and the link's `catalog` and `install` served from it |
| `crates/hub-api` | The control API's types and client, shared by the two binaries |
| `crates/hubd` | `pocket-hubd`, the resident core and the control API's server |
| `crates/hub-cli` | `pocket-hub`, the command line |
| `sdk/service` | The TypeScript SDK a service is written with; Hub ships it to every service |
| `app` | The macOS app: `PocketHub.xcodeproj` (the app target, sources in `app/PocketHub`) and a Swift package with its libraries, a mock core and the tests |
| `docs` | The contracts (the control API, the service protocol, the links that open the app), and real examples of every control API message |
| `site` | [hub.pocket.nexus](https://hub.pocket.nexus), the page that hands a visitor the app: built against the latest release, served by Cloudflare |

## Contracts

- **The link**: PocketJS `docs/POCKET-LINK.md`, in `vendor/pocketjs`. Its
  vectors and reference device are the tests' peers. Nothing under `vendor/`
  is edited here: a change to the protocol or to the reference lands in
  [pocket-nexus/pocketjs](https://github.com/pocket-nexus/pocketjs) first and
  this repository moves its pin.
- **Between the core and its clients**: [`CONTROL-API.md`](CONTROL-API.md). The
  server, the command line and the app implement that page; a change starts
  there, and `docs/control-api-examples.json` is regenerated from the
  end-to-end test.
- **Between the core and services**: [`SERVICE-PROTOCOL.md`](SERVICE-PROTOCOL.md).
- **Between a web page and the app**: [`DEEP-LINKS.md`](DEEP-LINKS.md), the
  `pocket-hub:` links the applet store's website opens Hub with. The website's
  side is in [pocket-nexus/applets](https://github.com/pocket-nexus/applets)
  (`store/site/src/hub.ts`) and changes with that page. A link shows; it never
  changes a device.

## Building

Rust (the version in `Cargo.toml`), [Bun](https://bun.sh) for the tests that run
the reference device, and Xcode for the app.

```sh
git clone --recurse-submodules https://github.com/pocket-nexus/pocket-hub.git
cd pocket-hub
cargo build -p pocket-hubd -p pocket-hub-cli
target/debug/pocket-hub status                 # starts the core when none runs
```

The app: open `app/PocketHub.xcodeproj` in Xcode and run the PocketHub scheme,
or from a terminal:

```sh
cd app
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
xcodebuild -scheme PocketHub -configuration Release -destination "platform=macOS,arch=$(uname -m)" \
    -derivedDataPath build build     # build/Build/Products/Release/Pocket Hub.app
```

A build of the app also builds the core and the command line with cargo, in
the matching profile, into the app's `Contents/Helpers`
(`app/scripts/embed-core.sh`). cargo builds them for the Mac it runs on, and
the script stops a build whose app is for another architecture: the Release
configuration is arm64 alone.

## Tests

Run all of it before a pull request:

```sh
cargo test --workspace && cargo clippy --workspace --all-targets
(cd sdk/service && bun test)
(cd app && export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer && swift test &&
  xcodebuild -quiet -scheme PocketHub -destination "platform=macOS,arch=$(uname -m)" -derivedDataPath build build)
(cd site && bun install && bun test && bun run typecheck)     # when site/ changed
```

`cargo test` runs the vectors, the reference device under Bun and the real
binaries end to end; it needs no network and no device. A few tests that reach
GitHub or the live applet store are marked `#[ignore]`. `swift test` covers the
app's libraries against the mock core; the SwiftUI app is the Xcode project's
target and only `xcodebuild` (or Xcode) compiles it.

**Tests use temp homes and ephemeral ports (`--link-port 0`), never TCP 8628 or
the default home**: a core for a real device may be running there. Stop only
processes a test started, by pid.

`docs/control-api-examples.json` is written by the end-to-end test
(`POCKET_HUB_UPDATE_EXAMPLES=1 cargo test -p pocket-hubd --test e2e`) and
checked by it otherwise; the app's tests decode every object in it.

### Trying a link

**A `pocket-hub:` link opened by the system goes to whichever build Launch
Services picks, started with the default home**, so `open pocket-hub://…`
during a run against the mock core can start another copy of the app with a
core in the default home. Hand the link to the build under test instead:
`open -a "<path>/Pocket Hub.app" "pocket-hub://…"`, or the `link <url>` debug
command (`app/PocketHub/DebugCommands.swift`, debug builds only). For the same
reason `check-strings.sh` and `package.sh` take what they build out of Launch
Services.

## Rules that are easy to break

- **No fallback nobody asked for.** When something Hub needs is unavailable (a
  port, a home folder, a release source), it fails with one precise line. Do
  not add a second path that hides the failure.
- **The owner never has to learn what "Pocket Shell", a runtime or a pairing
  file is.** "Pocket Shell" appears in owner-facing text in one role only: the
  name of the thing to open on the console. Everything about installing,
  updating or troubleshooting says "the 3DS's software".
- **No link key in a log line, an error, a status answer or the control API.**
- **A service gets no device operation**, and a process Hub starts inherits none
  of Hub's descriptors.
- Owner-facing sentences from the core are English; the app localizes from
  `kind`, `step` and `reason`.
- Comments say the mechanism and the reason, and cite the section of the
  specification they follow.

## The app's words

**The app's words are in one file**, the String Catalog
`app/Localization/Localizable.xcstrings`: a key is the English sentence as the
source writes it, and the catalog holds its Simplified Chinese. Every string
has both. After changing owner-facing text, run `app/scripts/check-strings.sh`.
It asks the compiler which strings the source uses, brings the catalog in line
with them, and fails naming each string that has no Simplified Chinese yet and
each one the source no longer uses. A translation is written into the catalog
itself, in Xcode's String Catalog editor or in the file.

## Pull requests

Changes go through a pull request named with
[Conventional Commits](https://www.conventionalcommits.org) (`feat(core): …`,
`fix(setup): …`).

## Releasing

A release is a tag on `main` and the app built from it, signed with the
Developer ID of TypeSafe Limited and notarized by Apple. Pushing the tag is
enough: `.github/workflows/release.yml` builds it with `app/scripts/package.sh`
and attaches the disk image and the zip to a **draft** release of the tag. The
notes are written on the draft, and publishing it is the release:

```sh
git checkout main && git pull
git tag -a v<version> -m "Pocket Hub v<version>" && git push origin v<version>
# the release workflow drafts the release; write its notes, then publish it
```

The same build runs on a Mac that holds the credentials (below):

```sh
app/scripts/package.sh                 # app/dist/Pocket-Hub-<version>-arm64.dmg and .zip, and their SHA-256
gh release create v<version> --verify-tag --title "Pocket Hub v<version>" \
    app/dist/Pocket-Hub-<version>-arm64.dmg app/dist/Pocket-Hub-<version>-arm64.zip
```

The version is one number in three places, changed together: `version` of
`[workspace.package]` in `Cargo.toml`, `version` in `sdk/service/package.json`
and `MARKETING_VERSION` in the Xcode project. A tag that is not `v` and that
version stops the workflow. `package.sh` refuses a working tree with changes,
builds the Release configuration from nothing, and checks that every
executable in the app is arm64 and that the core and the command line report
the app's version. It packs the app twice: a disk image (the app beside a
link to Applications, which is what a person downloads) and a zip (the same
app, for what unpacks it by itself). Finder lays the disk image's window out
on the picture `app/Artwork/dmg-background.swift` draws
(`app/Artwork/DMGBackground.tiff`): the script runs in a logged-in session,
macOS asks once whether the terminal may control Finder, and no other volume
named Pocket Hub may be mounted.

**Signing and notarization.** `package.sh` signs the app and both helpers with
`Developer ID Application: TypeSafe Limited (9FDAZCRAN3)`, with the hardened
runtime (on in the project; Xcode leaves it out of an ad hoc signature, so a
development build runs without it) and a secure timestamp, and
without the entitlement that lets a debugger attach. It sends the app to
Apple's notary service, staples the ticket to it, packs it, then signs,
notarizes and staples the disk image. A rejection stops it with the service's
log. Builds for development stay signed ad hoc. The script checks two things
in the keychain before it builds:

- the certificate with its private key. It expires on 1 February 2027, with the
  authority that issued it; a release after that needs a new one from the
  team's account (Certificates, Identifiers & Profiles, Developer ID
  Application). Releases signed before then stay valid.
- notarytool's profile `pocket-hub-notary`, an App Store Connect API key of
  the team (Users and Access › Integrations › Team Keys, the Developer role at
  least):

  ```sh
  xcrun notarytool store-credentials pocket-hub-notary \
      --key AuthKey_<key id>.p8 --key-id <key id> --issuer <issuer id>
  ```

### The release runner

The release workflow runs on a self-hosted runner of this repository with
the label `pocket-hub-release`, never on a machine of GitHub's: it is a Mac
that holds the certificate and the notarytool profile above in its login
keychain, and they never leave it (the workflow takes no secret). A pull
request that changes `package.sh`, `embed-core.sh` or the workflow runs the
whole build there too, notarization included, and keeps the archives as the
run's artifact. A pull request from a fork never runs there.

The runner runs as a launchd agent of the owner logged in on that Mac, which
is what unlocks the keychain and lets Finder lay out the disk image; the
first run asks once whether the runner may control Finder. It needs Xcode at
`/Applications/Xcode.app` and Rust from rustup. To set one up, in a folder of
its own (the runner's release from
[actions/runner](https://github.com/actions/runner/releases), unpacked there):

```sh
./config.sh --url https://github.com/pocket-nexus/pocket-hub --labels pocket-hub-release \
    --token "$(gh api -X POST repos/pocket-nexus/pocket-hub/actions/runners/registration-token -q .token)"
printf %s "$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" > .path
./svc.sh install && ./svc.sh start
```

`.path` is the runner's `PATH`; `config.sh` writes the one of the shell it ran
in, which the line after it replaces with a plain one (`gh` and `git` from
Homebrew, cargo from rustup).

Two more settings, both found when the runner hung:

- **The launchd agent must run in the owner's session.** `svc.sh install`
  writes `SessionCreate` true into the agent's plist
  (`~/Library/LaunchAgents/actions.runner.pocket-nexus-pocket-hub.<name>.plist`),
  which gives the runner a security session of its own where the login
  keychain is locked, and notarytool then finds no profile. Set it to false
  before `./svc.sh start`:
  `plutil -replace SessionCreate -bool false <the plist>`.
- **A Mac that reaches GitHub through a proxy gives the runner that proxy** in
  its `.env` (`https_proxy=…`, `http_proxy=…`, `no_proxy=…`, and the same in
  capitals); the runner hands it to each step, and `embed-core.sh` hands it
  on to cargo.

Publishing the release deploys [hub.pocket.nexus](https://hub.pocket.nexus)
again (`.github/workflows/site.yml`): its Download button links the release's
archive itself, so the page names the version, the size and the SHA-256 of the
one it offers. The page's build takes the latest release that is not a
pre-release, and refuses one whose tag is not `v<version>` or that lacks the
archive of a platform `site/src/release.ts` lists (today macOS alone, the
disk image `Pocket-Hub-<version>-arm64.dmg`); the page that is live then stays
as it was.

## The download page

`site/` is [hub.pocket.nexus](https://hub.pocket.nexus). Most visitors come
from the applet store's website, whose Install button opens the app and does
nothing on a computer without it, so the page is the download first, then the
devices Hub connects to. It is a Vue app built with Vite, as the store's
website is, in English and Simplified Chinese, with the store's look. Its build
asks GitHub for the latest release first (`site/src/release.ts`) and writes the
archive's own address into the page, so the button is the download and not a
page to find it on; Cloudflare serves the built folder (`site/wrangler.jsonc`).

A platform is added in `site/src/release.ts` (its archive's name) and in the
page's words (its download); a device in `site/src/devices.ts`, with a drawing
of it.

```sh
cd site
bun install
bun run dev              # the page with hot reload; GITHUB_TOKEN, when set, is sent to GitHub
bun run build            # site/dist
bunx wrangler dev        # serves site/dist with its headers on http://localhost:8787
```

A change to `site/` that reaches `main` is deployed by the same workflow. The
deploy needs the repository secrets `CLOUDFLARE_API_TOKEN` (Workers Scripts
and Workers Routes, edit) and `CLOUDFLARE_ACCOUNT_ID`.
