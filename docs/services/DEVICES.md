# Three devices, three layers

Proposal (see `README.md`). The applets of pocket-nexus/applets are built from
one source for three devices, and **every one of them takes the new path**: no
device keeps `https://companion.pocket/<service>` (the owner's decision). This
page says where the service host is for each device, what is identical
everywhere and what differs, how Omarchy Pocket's service host is built, and
what the Tab5 needs. Facts about the other programs are from
pocket-nexus/omarchy-pocket and pocket-nexus/pocket-m5 at their `main` on
2026-11-17, and from this repository at `1bc2074`.

**Words.** "Host" on these pages is the service host, as in the SDK's `host`.
The device's own native program is its **runtime**; PocketJS's "host
contract" (`pocket.host.json`) and "host namespace" (`globalThis.…`) are
always written out in full, because they belong to the runtime
(`QUESTIONS.md` Q2).

## 1. The three layers

| Layer | Contract | Identical on | Differs by |
| --- | --- | --- | --- |
| **(a) applet ↔ service**: the applet's channel and the system channel | link specification §7, §7.9 and §7.11 (as proposed) | every device: the guest sees the same namespace, modules, limits and three states | the transport under it: **the link** when the service host is another machine, **a local connection** when it is on the device (§2) |
| **(b) service ↔ host**: the SDK and the local protocol | `CAPABILITIES.md` §2 and §3, `HOST-PROTOCOL.md` | every host: **one service bundle runs on any host that provides what its manifest requires** | nothing a service sees |
| **(c) the capabilities** | `CAPABILITIES.md` §4 to §10 | every host's API and limits | which capabilities a host provides (§3) and how it implements them |

## 2. The devices

| | Nintendo 3DS | M5Stack Tab5 | Omarchy Pocket |
| --- | --- | --- | --- |
| Hardware | New or Old 3DS: 64 to 256 MB, QuickJS guest | ESP32-P4, a 720 × 1280 panel turned to 640 × 360, microSD card, USB-C | an Anbernic RG DS Plus (RK3568, Linux); also an AYN Thor (Android, the Linux root under proot) and a ClockworkPi uConsole (Raspberry Pi OS, the root under chroot) |
| Runtime | Pocket Shell (PocketJS `products/3ds`) | pocket-m5's firmware (`programs/m5stack-tab5`), with pocket-shell's launcher | omarchy-pocket-host (Rust) and omarchy-pocketd (Bun) |
| **Where its service host is** | Hub on a paired Mac | Hub on a paired Mac | **on the device** |
| The link today | Pocket Link 2 over Wi-Fi | **Pocket Link 2 over Wi-Fi** (`components/pocket_system`: `link2*.c`); it lists `apps`, `runtime`, `runtime.remove`, `catalog`, `install`, `net.http`, and **opens no applet channel** | none: same machine |
| Pairing | Hub writes the pairing over ftpd or on the SD card (link §3.2) | **by a code it shows** (link §3.11; Hub's `pairing.answer`), or Hub writes it onto its card (§3.2) | none |
| What Hub may ask of it (`Device.supports`, `crates/hub-core/src/family.rs`) | `push`, `screenshot`, `logs`, `program` | **none**: no push, screenshot, log or program install; its software is flashed over USB by its owner | not a Hub device |
| Channels, under the proposal | the link | **the link, once its firmware lists `channel` and `system`** and binds the channel namespaces (the C client it shares with the 3DS already holds a channel table) | a local connection (below) |
| The worker's HTTP | `net.http` over the link; Hub performs it and applies `applet.hosts` | **its own HTTP stack** (`pocket_http_esp`) for every URL but `companion.pocket`, which goes to Hub as `net.http`; Hub never sees the rest, so **the firmware must apply `applet.hosts`** (`QUESTIONS.md` Q10) | today an `appNet` line on omarchy-pocketd's bridge socket (`host/src/net.rs`, `daemon/companion.ts`); under the proposal `net.http` on the local connection |
| `data.fs` | yes | yes (on the card, or the `storage` partition) | **no** (`host/pocket.host.json`) |
| Installs | from Hub's catalog over the link (§6.4), or pushed by Hub | from Hub's catalog over the link (its own store page), or an install QR code | the Store applet, through omarchy-pocketd's `appstore` service (`daemon/appstore.ts`) |
| Media side connection | TCP to Hub | TCP to Hub | TCP to 127.0.0.1 |

**`Device.supports` and services.** `supports` lists what Hub may *ask* of a
device. Channels and the system channel are opened by the device, so they need
no entry there; a Tab5 gets them when its firmware lists the groups `channel`
and `system` (link §6.1) and Hub lists them back. Nothing in Hub's family table
changes for services.

**The local connection on Omarchy Pocket** (link §7.11 as proposed): a Unix
domain socket the service host listens on, at
`$XDG_RUNTIME_DIR/pocket-host/channels.sock`, mode 0600, owned by the Omarchy
user. omarchy-pocket-host opens **one connection per applet window** (each
window is a Runtime of its own, `host/src/children.rs`), sends `local.hello`
with the package's `app` and `hash`, and then carries that window's applet
channel, its system channel and its worker's `net.http` with the link's
frames. No beacon, pairing, preamble or keepalive: the socket's mode admits
the peer. The shell keeps its own bridge to omarchy-pocketd
(`$XDG_RUNTIME_DIR/omarchy-pocket/bridge.sock`, `daemon/protocol.ts`) for its
system calls.

**What omarchy-pocketd does for applets today**, and what replaces each:

| Today (`daemon/companion.ts`, `daemon/appstore.ts`) | Under the proposal |
| --- | --- |
| answers `companion.pocket/<service>` with the earlier Pocket Hub's TypeScript providers (a submodule `vendor/pocket-hub`; not read here, under this repository's rule) and its own `spotify` | each applet's service, run by the service host on the device |
| `images` by ffmpeg (an avatar about 0.57 s, `docs/omarchy-pocket.md`) | `image` (`CAPABILITIES.md` §7) |
| `glyphs` from its own baker (`daemon/glyphs.ts`, at density 2) | `text` through the system service |
| every other URL through PocketJS's `fetch.ts` policy, and the store's isolation | `net.http` on the local connection; a service's `store` |
| YouTube and Podcasts with yt-dlp and ffmpeg on the device | YouTube as an unrestricted service, `media` (§5) |
| owner setup files copied from the Mac (X's `session.json`, Discord's token) | the setup guide, followed by the coding agent on the device (`SETUP.md`) |

## 3. Which host provides which capability

| Capability | Hub (macOS) | Omarchy Pocket's host (Linux aarch64) | A future host on a weak device |
| --- | --- | --- | --- |
| `net` | yes | yes | yes |
| `store` | yes | yes | yes |
| `image` | yes | yes | yes |
| `text` | through the system service; for services later | idem | yes |
| `media` | yes: H.264, PAL8 or RGB565 for the device at the other end | yes: RGB565 frames over 127.0.0.1, as its services produce today with ffmpeg | as it can |
| `native` | yes | **yes** (§5) | no |

**Implementations shared by both hosts** (§4): `net` and `store` are Rust
already (`crates/hub-net`) or plain files; **`image` uses pure-Rust decoders**
(JPEG, PNG, GIF and WebP through the `image` crate family), memory-safe at
little cost; **`text` uses PocketJS's own Rust text engine** (`engine/text`,
`docs/TEXT-OFFLOAD.md`) with the fonts each machine has (a Mac's system font
files; Omarchy Pocket's JetBrains Mono Nerd Font and wqy-microhei); **`media`
stays per platform**: AVFoundation and VideoToolbox on a Mac, ffmpeg on
Linux. **A host decodes in its own process** (the owner's decision: decoding
is performance-first), so bytes from the network reach the decoders inside
the host; that is why the memory-safe decoders are preferred.

## 4. Omarchy Pocket's host: one host, factored out of Hub

**Decided on 2026-11-17.** Hub's host parts (the service supervisor, the
service socket with its three connections, the capability providers, the
system service, the enclosure launcher, the channel relay, the store client
for releases, the setup inlet) become a crate, `crates/pocket-host`, that
`pocket-hubd` embeds on macOS and that Omarchy Pocket runs as a program of its
own, **`pocket-hostd`**, under the Omarchy user's systemd, built for Linux
aarch64, with its command line `pocket-host`. Omarchy Pocket's architecture
rests on it; there is no second implementation.

**What it costs**: a Linux enclosure (§6); the portable capability
implementations of §3; CI on Linux aarch64; the local transport's endpoint in
`hub-core` (the relay without admission); Bun for linux-arm64 on the device
(omarchy-pocketd already runs on Bun 1.4.2).

**Rejected alternative**: omarchy-pocketd implementing the contract again in
TypeScript: a second implementation of every rule, kept equal by tests alone,
and image decoding by spawning ffmpeg as today.

**`pocket-hostd` is a process of its own**, not code linked into
omarchy-pocket-host: a decoder's failure does not take the display program
down, and the licences stay apart. This repository is GPL-3.0-or-later
(`LICENSE`, `Cargo.toml`); omarchy-pocket declares MIT (its `Cargo.toml`
`license`). A GPL program that Omarchy Pocket ships beside its MIT programs and
talks to over a socket is an aggregate, which the GPL allows, with the GPL's
duty to offer `pocket-hostd`'s source, which is public here. **Linking the
crate into omarchy-pocket-host would make that program GPL**; the separate
process avoids the question rather than raising it. Not a lawyer's reading.

omarchy-pocketd keeps its system duties (the bridge, terminals, Omarchy's
command line, notifications, Wi-Fi) and gives up `companion.ts`'s services and
the service half of `appstore.ts`.

## 5. `native` on Omarchy Pocket

**Omarchy Pocket's host provides `native`.** `native` is for hosts that are
general-purpose computers the owner controls. Omarchy Pocket is one: the owner
has a terminal, SSH, Omarchy's command line and a coding agent on it, and
makes and runs applets there; YouTube's search with yt-dlp already runs on the
device (9.3 s, `docs/omarchy-pocket.md`). The disclosure is the same, in the
device's words ("YouTube runs its own programs on this device…"), and the
consent is the owner's click in the shell. On the AYN Thor the service runs
inside the Linux root under proot, with that root's rights.

## 6. The enclosure on Linux

A portable service gets the rules of `TRUST.md` §1. Candidate mechanisms:
**bubblewrap** (user, mount, network and PID namespaces) where unprivileged
user namespaces are allowed; **Landlock and seccomp** where not. Measuring the
mechanism on each device, the AYN Thor under proot included, is an
implementation step, not a question.

**Decided on 2026-11-17, for a device with no measured enclosure**: only
services whose release is published with the official key run there, and they
run unenclosed. **A service whose release is not published with the official
key does not run at all, whatever it requires**, and its applet says the
device does not support it yet. This is a stated rule, not a fallback: no
third-party code runs unenclosed on such a device.

- **The refusal**: `channel.open` answers **`enclosure`**, a refusal of its
  own (link §7.1 as proposed), on the link and on the local connection alike:
  "this service host cannot enclose this daemon, and its release is not the
  official key's". It is not `capability` (nothing the service requires is
  missing) and not `rejected` (the applet cannot tell its owner why). Retry:
  not during this applet run; the applet's framework turns it into "This
  device does not support YouTube yet."
- **Before that point**: the device's service host is the one that installs
  releases (`QUESTIONS.md` Q40), so its catalog **offers such an applet,
  marked** "Not supported on this device yet", and **refuses its install**
  with the same sentence. It is listed, not hidden, so the owner sees why.
- **An unrestricted (`native`) third-party service** is unenclosed everywhere,
  with the owner's consent. Read with the owner's sentence ("third-party ones
  do not run"), **it does not run on such a device either, consent or not**;
  this is the coordinator's reading, confirmed as the rule here, with a
  one-line check in `QUESTIONS.md` Q43.
- **A development install** is not published with the official key, so under
  the rule as stated **it does not run on such a device**, refused the same
  way. That stops applet development on a device whose enclosure is not yet
  measured, Omarchy Pocket among them until it is; `QUESTIONS.md` Q44 asks
  whether the owner's own code is meant to be included.

## 7. Where an applet without a service keeps its state

The 3DS and the Tab5 provide `data.fs`; Omarchy Pocket does not.
*Recommendation*: **Omarchy Pocket's host contract gains `data.fs`**, so GitHub's
token and Weather's places live in `data.fs` on all three devices. Cost:
omarchy-pocket-host implements `contracts/spec/fs.ts` (it can compile
PocketJS's `engine/host-services/fs.c`), with each app's tree under
`~/.local/share/omarchy-pocket/apps/<id>/`; Notes then builds for Omarchy
Pocket too. `QUESTIONS.md` Q8.

## 8. The Tab5's path

The Tab5 already links to this Hub over Pocket Link 2 and pairs by the code it
shows. What its firmware still needs for this proposal, without designing it:

| Area | Change in pocket-m5 | Open |
| --- | --- | --- |
| Channels | list `channel` and `system` in `hello`; bind `globalThis.channels.service` and `.system` (PocketJS's `link2_channel_qjs` binding takes a channel index); open the system channel when the plan asks for it | the memory of two channels on the P4, unmeasured |
| `device.accepts` | state its texture formats, density (its host contract states raster density 1 for the 640 × 360 surface; its README says the shell is drawn "at 2x") and media codecs (`components/pocket_media`) | the facts are the firmware's |
| Its worker's HTTP | apply `applet.hosts` and the destination policy (link §6.5) in its HTTP node, since Hub never sees those requests; `companion.pocket` stops going to Hub | `QUESTIONS.md` Q10 |
| Host contract | `service.channel`, `image.url`, `text.glyphs.deferred`, `media.url` | |
| Updates | its software is flashed over USB by its owner (`Device.supports` has no `program`), so **a Tab5 gets channels only when its owner flashes a firmware that has them** | whether Hub ever installs its software |
