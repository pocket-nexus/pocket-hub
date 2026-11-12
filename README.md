# Pocket Hub

Pocket Hub runs on a computer and stands between physical devices, the
[Pocket Applets](https://github.com/pocket-nexus/applets) they run, and the
tools that make applets. A device only has to reach Hub: Hub puts the runtime
on a new device, keeps the link to it, carries applets to it, performs the
network requests the device cannot make itself, and runs the computer side of
an applet (its daemon) beside it.

Hub and devices speak **Pocket Link 2**. The specification is PocketJS's
[`docs/POCKET-LINK.md`](https://github.com/pocket-nexus/pocketjs/blob/main/docs/POCKET-LINK.md),
pinned here as the submodule `vendor/pocketjs`, whose test vectors and reference
device this repository's tests run against.

The first device is the **Nintendo 3DS** over Wi-Fi. The app is for macOS; the
core is portable Rust.

## How it is built

```
            Pocket Hub.app (SwiftUI)        pocket-hub (command line)
                      \                          /
                       \   control API: JSON-RPC, one object per line,
                        \  on a Unix socket     /        docs/CONTROL-API.md
                         v                     v
                  +---------------------------------+
                  |           pocket-hubd           |   the resident core:
                  |  devices · store · daemons      |   one per home folder
                  +---------------------------------+
                     |                          |
     Pocket Link 2   |                          |  local daemon protocol
     (TCP 8628,      |                          |  docs/DAEMON-PROTOCOL.md
      UDP beacons)   v                          v
                  devices                 one Bun process per applet's daemon
```

**`pocket-hubd` owns every device connection.** The app and the command line
are clients of one control API and hold no device logic, so a device is never
held by two processes and a later remote client (Pocket Studio's agents) is
one more client of the same surface.

**A daemon is the computer side of one applet**: a Bun process Hub starts when
a device opens that applet's channel and stops when it is idle. Applet and
daemon exchange requests, events and credited byte streams whose content is
theirs; Hub relays them and reads none of it. Neither learns which carrier the
device is on.

| Path | What it is |
| --- | --- |
| `crates/pocket-link` | The Pocket Link 2 wire: beacon, preamble, frames and their streaming decoder, handshake proofs, pairing files and the setup as a plan of storage operations. No I/O |
| `crates/hub-store` | Hub's own files: its identity, one record per paired device, the channel generation |
| `crates/hub-core` | The link endpoint: carriers, beacons, the handshake, requests, streams and credit, runtime operations, the device mutex, applet channels relayed to daemons |
| `crates/hub-setup` | The first install: finding a console's ftpd, its software and the pairing written onto its storage, software releases |
| `crates/hub-daemons` | The Bun runtime (downloaded on demand), daemon packages, their processes and the daemon socket |
| `crates/hub-net` | `net.http`: a device's HTTP requests performed by Hub, with the destination policy |
| `crates/hub-applet-store` | The applet store's client, and the link's `catalog` and `install` served from it |
| `crates/hub-api` | The control API's types and client, shared by the two binaries |
| `crates/hubd` | `pocket-hubd`, the resident core and the control API's server |
| `crates/hub-cli` | `pocket-hub`, the command line |
| `sdk/daemon` | The TypeScript SDK a daemon is written with; Hub ships it to every daemon |
| `app` | The macOS app: `PocketHub.xcodeproj` (the app target, sources in `app/PocketHub`) and a Swift package with its libraries, a mock core and the tests |
| `docs` | The two contracts, and real examples of every control API message |

## Building and running

Rust (the version in `Cargo.toml`), [Bun](https://bun.sh) for the tests that run
the reference device, and Xcode for the app.

```sh
git clone --recurse-submodules https://github.com/pocket-nexus/pocket-hub.git
cd pocket-hub
cargo build -p pocket-hubd -p pocket-hub-cli

target/debug/pocket-hub status                 # starts the core when none runs
target/debug/pocket-hub setup 3ds              # with ftpd open on the console
target/debug/pocket-hub devices
target/debug/pocket-hub store                  # the applets for the paired device
target/debug/pocket-hub store install dev.pocket-nexus.hn
target/debug/pocket-hub device shot out.png
target/debug/pocket-hub --help
```

The core keeps its files in `~/Library/Application Support/nexus.pocket.hub`
(`$POCKET_HUB_HOME` overrides it) and listens for devices on TCP 8628. **It does
not fall back when that port or its home is unusable**: it says why and exits.

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
(`app/scripts/embed-core.sh`). After changing owner-facing text, run
`app/scripts/check-strings.sh`: it brings the String Catalog in line with the
source and fails when a string has no Simplified Chinese.

## Tests

```sh
cargo test --workspace       # the vectors, the reference device under Bun, the real binaries end to end
cargo clippy --workspace --all-targets
(cd sdk/daemon && bun test)
(cd app && swift test)       # the app's libraries, against the mock core
```

The workspace suite needs no network and no device. A few tests that reach
GitHub or the live applet store are marked `#[ignore]`.

`docs/control-api-examples.json` is written by the end-to-end test
(`POCKET_HUB_UPDATE_EXAMPLES=1 cargo test -p pocket-hubd --test e2e`) and
checked by it otherwise; the app's tests decode every object in it.

## What has run on hardware

On a New Nintendo 3DS XL, through the command line: the first install over
ftpd, linking and relinking, status, screenshots, logs, installing applets
from the store on the computer and from the console's own store page,
`net.http`, an applet channel with its daemon, and updating the console's
software over the link. The app has shown and driven the same console. An Old
3DS, other carriers and other devices have not been tried.
