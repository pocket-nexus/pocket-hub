# Working in this repository

Pocket Hub is the computer side of Pocket Link 2: a resident core
(`pocket-hubd`), a command line and a macOS app that are its clients, and the
daemons it runs for applets. `docs/CONTRIBUTING.md` has the layout.

## What decides a question

- **The link**: PocketJS `docs/POCKET-LINK.md`, in `vendor/pocketjs`. Its
  vectors and reference device are the tests' peers. Nothing under `vendor/`
  is edited here: a change to the protocol or to the reference lands in
  [pocket-nexus/pocketjs](https://github.com/pocket-nexus/pocketjs) first and
  this repository moves its pin.
- **Between the core and its clients**: `docs/CONTROL-API.md`. The server, the
  command line and the app implement that page; a change starts there, and
  `docs/control-api-examples.json` is regenerated from the end-to-end test.
- **Between the core and daemons**: `docs/DAEMON-PROTOCOL.md`.
- **Between a web page and the app**: `docs/DEEP-LINKS.md`, the `pocket-hub:`
  links the applet store's website opens Hub with. The website's side is in
  [pocket-nexus/applets](https://github.com/pocket-nexus/applets)
  (`store/site/src/hub.ts`) and changes with that page. A link shows; it
  never changes a device.
- **Measure before naming a cause.** A stall on a device was first blamed on
  the wrong step from reading code; the timing run found the real one.

This repository shares nothing with the earlier Pocket Hub (now
`pocket-nexus/pocket-hub-obsolete`): do not read it or port from it, and do not
touch its folders on a machine (`~/Library/Application Support/Pocket Hub`,
`~/Library/Logs/Pocket Hub`).

## Rules that are easy to break

- **No fallback nobody asked for.** When something Hub needs is unavailable (a
  port, a home folder, a release source), it fails with one precise line. Do
  not add a second path that hides the failure.
- **The owner never has to learn what "Pocket Shell", a runtime or a pairing
  file is.** "Pocket Shell" appears in owner-facing text in one role only: the
  name of the thing to open on the console. Everything about installing,
  updating or troubleshooting says "the 3DS's software".
- **No link key in a log line, an error, a status answer or the control API.**
- **A daemon gets no device operation**, and a process Hub starts inherits none
  of Hub's descriptors.
- Owner-facing sentences from the core are English; the app localizes from
  `kind`, `step` and `reason` (English and Simplified Chinese are both filled
  in for every string).
- Comments say the mechanism and the reason, and cite the section of the
  specification they follow. No stub presented as done.

## The loop

```sh
cargo test --workspace && cargo clippy --workspace --all-targets
(cd sdk/daemon && bun test)
(cd app && export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer && swift test &&
  xcodebuild -quiet -scheme PocketHub -destination "platform=macOS,arch=$(uname -m)" -derivedDataPath build build)
```

`swift test` covers the app's libraries; the SwiftUI app is the Xcode
project's target and only `xcodebuild` (or Xcode) compiles it.

Tests use temp homes and ephemeral ports (`--link-port 0`), never TCP 8628 or
the default home: a core for a real device may be running there. Stop only
processes a test started, by pid.

**A `pocket-hub:` link opened by the system goes to whichever build Launch
Services picks, started with the default home.** In the middle of a run
against the mock core, `open pocket-hub://…` once started a second copy of
the app, and with it a core in the default home that the owner's real device
linked to. To try a link against a mock core or a temp home, hand it to the
build that is running there: `open -a "<path>/Pocket Hub.app" "pocket-hub://…"`,
or the `link <url>` debug command.

`hub-core`'s tests run the reference device under Bun; one of them
(`screenshot_png_and_the_unsolicited_one`) fails rarely until the pin includes
PocketJS #689.

Changes go through a pull request named with Conventional Commits
(`feat(core): …`, `fix(setup): …`).
