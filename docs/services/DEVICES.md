# Three devices, three layers

Proposal (see `README.md`). The applets of pocket-nexus/applets are built from
one source for three devices, and **every one of them takes the new path**: no
device keeps `https://companion.pocket/<service>` (the owner's decision). This
page says where the host is for each device, what is identical everywhere and
what differs, who implements the host on Omarchy Pocket, and what the Tab5
needs. Facts about the two other programs are from pocket-nexus/omarchy-pocket
and pocket-nexus/pocket-m5 at their `main` on 2026-11-12.

## 1. The three layers

Each layer is stated without Hub; Hub is one implementation of the host.

| Layer | Contract | Identical on | Differs by |
| --- | --- | --- | --- |
| **(a) applet ↔ service**: the applet's channel and the system channel | link specification §7, §7.9 and §7.11 (as proposed) | every device: the guest sees the same namespace, modules, limits and three states | the transport under it: **the link** when the host is another machine, **a local connection** when the host is on the device (§2) |
| **(b) service ↔ host**: the SDK and the local protocol | `CAPABILITIES.md` §2 and §3, `HOST-PROTOCOL.md` | every host: **one service bundle runs on any host that provides what its manifest requires** | nothing a service sees; the host's process model and folders are its own |
| **(c) the capabilities** | `CAPABILITIES.md` §4 to §10 | every host's API and limits | **which capabilities a host provides** (§3) and how it implements them |

## 2. The devices

| | Nintendo 3DS | M5Stack Tab5 | Omarchy Pocket |
| --- | --- | --- | --- |
| Hardware | New or Old 3DS: 64 to 256 MB, QuickJS guest | ESP32-P4, a 720 × 1280 panel turned to 640 × 360, microSD card, USB-C | an Anbernic RG DS Plus (RK3568, Linux); also an AYN Thor (Android, the Linux root under proot) and a ClockworkPi uConsole (Raspberry Pi OS, the root under chroot) |
| Program | Pocket Shell (PocketJS `products/3ds`) | pocket-m5's firmware (`programs/m5stack-tab5`) | omarchy-pocket-host (Rust) and omarchy-pocketd (Bun) |
| **Where its host is** | Hub on a paired Mac | Hub on a paired Mac | **on the device** |
| Channels travel on | Pocket Link 2 over Wi-Fi | Pocket Link 2, which **its firmware does not speak yet**: it compiles version 1's `link.c`, `link_http.c` and `link_proto.c` (`components/pocket_system/CMakeLists.txt`) | **a local connection** (below) |
| The worker's HTTP | `net.http` over the link; Hub performs it and applies `applet.hosts` | **its own HTTP stack** (`pocket_http_esp`); `companion.pocket` alone goes over Link 1 today (`components/pocket_system` "Companion services"). Hub never sees the rest, so **the firmware must apply `applet.hosts`** (Q8) | today an `appNet` line on omarchy-pocketd's bridge socket (`host/src/net.rs`, `daemon/companion.ts`); under the proposal `net.http` on the local connection, performed by the host on the device |
| `data.fs` | yes | yes (on the card) | **no** (`host/pocket.host.json`; the applets' `README.md` leaves Notes out of Omarchy builds for it) |
| Pairing | Hub writes the pairing files over ftpd or on the SD card (link §3.2) | today a QR code of Link 1's `pocketlink://pair`, read by its camera; under version 2: Q9 | none: same machine |
| Installs | from the Mac's catalog over the link (link §6.4) | from the Mac's catalog over Link 1, or an install QR code (`pocketlink://install?…`, `components/pocket_system` "Installs from a code") | the Store applet, through omarchy-pocketd's `appstore` service (`daemon/appstore.ts`) |
| Media side connection | TCP to Hub | TCP to Hub | TCP to 127.0.0.1 |

**The local connection on Omarchy Pocket** (link §7.11 as proposed): a Unix
domain socket the host listens on, at
`$XDG_RUNTIME_DIR/pocket-host/channels.sock`, mode 0600, owned by the Omarchy
user. omarchy-pocket-host opens **one connection per applet window** (each
window is a Runtime of its own, `host/src/children.rs`), sends `local.hello`
with the package's `app` and `hash`, and then carries that window's applet
channel, its system channel and its worker's `net.http` with the link's
frames. No beacon, pairing, preamble or keepalive: the socket's mode admits
the peer. Today the same machine already joins its two programs this way:
the shell talks to omarchy-pocketd over `$XDG_RUNTIME_DIR/omarchy-pocket/bridge.sock`,
one JSON object per line (`daemon/protocol.ts`); the proposal keeps that bridge
for the shell's own system calls and gives applets the link's frames instead.

**What omarchy-pocketd does for applets today**, and what replaces each:

| Today (`daemon/companion.ts`, `daemon/appstore.ts`) | Under the proposal |
| --- | --- |
| answers `companion.pocket/<service>` with the earlier Pocket Hub's TypeScript providers (a submodule `vendor/pocket-hub`; not read here, under this repository's rule) and its own `spotify` | each applet's service, run by the host on the device |
| `images` by ffmpeg (an avatar about 0.57 s, measured in `docs/omarchy-pocket.md`) | `image` (`CAPABILITIES.md` §7) |
| `glyphs` from its own baker (`daemon/glyphs.ts`, PocketJS `bakeSlot` at density 2) | `text` through the system service |
| every other URL through PocketJS's `fetch.ts` policy, and the store's isolation (`applet.hosts`, prefixed `store` names) | `net.http` on the local connection under the shared policy; a service's `store` |
| YouTube and Podcasts with yt-dlp and ffmpeg on the device, media on 127.0.0.1 | YouTube as an unrestricted service, `media` (§5) |
| owner setup files copied from the Mac (X's `session.json`, Discord's token) | `signin` on the device (Q6) |

## 3. Which host provides which capability

| Capability | Hub (macOS) | Omarchy Pocket's host (Linux aarch64) | A future host on a weak device |
| --- | --- | --- | --- |
| `net` | yes | yes | yes |
| `store` | yes | yes | yes |
| `signin` | yes: Pocket Hub's app shows a web view and forms | `form`: in the shell. **`web`: no web view exists on the device today** (the shell is a PocketJS program on GBM/EGL; Omarchy's desktop session is not installed); Q6 | per device |
| `image` | yes | yes | yes |
| `text` | through the system service; for services later (Q15) | idem | yes |
| `media` | yes: H.264, PAL8 or RGB565 for the device at the other end | yes: for the device itself, RGB565 frames over 127.0.0.1 as its services produce today with ffmpeg; a pass-through needs a source form `MediaSource` does not have | as it can |
| `native` | yes | **yes** (§5) | no |

**Implementations that must then be portable** (when one implementation
serves both hosts, §4): `net` and `store` are Rust already (`crates/hub-net`)
or plain files; **`image` uses pure-Rust decoders** (JPEG, PNG, GIF and WebP
through the `image` crate family) on both, which are memory-safe at little
cost and are not ImageIO; **`text` uses PocketJS's own Rust text engine**
(`engine/text`, `docs/TEXT-OFFLOAD.md`: COSMIC Text, Harfrust, Swash, fonts
loaded explicitly, no system font discovery) with the fonts each host has (a
Mac's system font files; Omarchy Pocket's JetBrains Mono Nerd Font and
wqy-microhei, which its baker uses today), not CoreText; **`media` stays per
platform**: AVFoundation and VideoToolbox on a Mac, ffmpeg on Linux, which
Omarchy Pocket's image already carries. **A host decodes in its own process**
(the owner's decision: a host may itself be a weak device, and a helper
process per picture costs it); bytes from the network therefore reach the
decoders inside the host, which is why the memory-safe decoders are
preferred where they cost nothing.

## 4. Who implements the host on Omarchy Pocket

| Option | What it is | What it costs |
| --- | --- | --- |
| **A. One host, factored out of Hub** | Hub's host parts (the service supervisor, the service socket with its three connections, the capability providers, the system service, the enclosure launcher, the channel relay, the store client for releases) become a crate, `crates/pocket-host`, that `pocket-hubd` embeds on macOS and that Omarchy Pocket runs as a program of its own, `pocket-hostd`, under the Omarchy user's systemd, built for Linux aarch64 | a Linux enclosure (§6); the portable capability implementations of §3; CI on Linux aarch64; the local transport's endpoint in `hub-core` (the relay without admission); Bun for linux-arm64 on the device (omarchy-pocketd already runs on Bun 1.4.2); the sign-in window per platform |
| **B. omarchy-pocketd implements the contract** | the host protocol, the capabilities, the policy, the signature checks and the enclosure launcher, again, in TypeScript inside omarchy-pocketd | a second implementation of every rule in this set, kept equal to Hub's by tests alone; image decoding by spawning ffmpeg as today; two places to fix each finding |

*Recommendation*: **A.** The contract exists so a service runs unchanged on
any host; two implementations would make that a matter of keeping them
alike, and the owner said from the start that the Rust core and the service
management should be portable. `pocket-hostd` is a process of its own rather
than part of omarchy-pocket-host so a decoder's failure does not take the
display program down; the local socket costs microseconds a message.
omarchy-pocketd keeps its system duties (the bridge, terminals, Omarchy's
CLI, notifications, Wi-Fi) and gives up `companion.ts`'s services and the
service half of `appstore.ts`. `QUESTIONS.md` Q2.

## 5. `native` on Omarchy Pocket

**Recommendation: Omarchy Pocket's host provides `native`.** `native` is for
hosts that are general-purpose computers the owner controls. Omarchy Pocket
is one: the owner has a terminal, SSH, Omarchy's command line and a coding
agent on it, and makes and runs applets there (`docs/applets.md` in
omarchy-pocket); YouTube's search with yt-dlp already runs on the device
(9.3 s, measured in `docs/omarchy-pocket.md`). The disclosure is the same, in
the device's words ("YouTube runs its own programs on this device…"), and
the owner gives the consent on the device, where the shell shows it. The
alternative, refusing `native` there, would make YouTube "need a computer" on
a device that is one. On the AYN Thor the service runs inside the Linux root
under proot, with that root's rights.

## 6. The enclosure on Linux

A portable service on Omarchy Pocket gets the rules of `TRUST.md` §1: it reads
Bun, its package and the SDK, connects to the host's service socket, and
nothing else. Mechanisms:

| Mechanism | What it gives | Where it works |
| --- | --- | --- |
| **bubblewrap** (user, mount, network and PID namespaces) | an empty network namespace, read-only binds of Bun, the package, the SDK and the system libraries, the service socket bound in, no other files; the closest match to the macOS profile | where unprivileged user namespaces are allowed: to be checked on the RG DS Plus's Rockchip 6.1 kernel and on Raspberry Pi OS |
| **Landlock and seccomp** | Landlock for files (read-only paths); seccomp to refuse `socket()` other than `AF_UNIX`, `execve`, `ptrace` | Landlock needs a 5.13 or later kernel built with it (not checked on either device); seccomp is everywhere |
| neither | | the AYN Thor: the root runs under proot, which uses `ptrace` itself; namespaces are not available to an Android app, and nesting seccomp under proot is untried |

*Recommendation*: bubblewrap where it runs, Landlock and seccomp where it does
not, measured on each device as the macOS profile was before step 2 ships;
on the AYN Thor, until a mechanism is measured, **the host does not run
portable services that require `store` or `signin`** rather than run them
unenclosed (Q5).

## 7. Where an applet without a service keeps its state

The direction gives `store` to services only; GitHub's token and Weather's
places belong to applets without a service. Per device: the 3DS and the Tab5
provide `data.fs`; Omarchy Pocket does not. *Recommendation*: **Omarchy
Pocket's host contract gains `data.fs`**, so one answer works on all three:
the applet keeps its state in `data.fs` everywhere. Cost: omarchy-pocket-host
implements `contracts/spec/fs.ts` (PocketJS's `engine/host-services/fs.c` is
the C implementation the 3DS and the Tab5 compile; the Rust host program can
compile it or port it) with each app's tree under
`~/.local/share/omarchy-pocket/apps/<id>/`; Notes then builds for Omarchy
Pocket too. On the 3DS the token lies on the SD card in plain text, as it
crosses Hub in every request header anyway. `QUESTIONS.md` Q3.

## 8. The Tab5's path

The Tab5 is a weak device like the 3DS, whose host is Hub. What must change,
without designing its firmware:

| Area | Change in pocket-m5 | Open |
| --- | --- | --- |
| The link | PocketJS's version-2 C client (`link2.c`, `link2_proto.c`, its BSD-socket platform `link2_bsd.c`; ESP-IDF's lwIP has BSD sockets) in place of `link.c`, `link_http.c`, `link_proto.c`; `companion.c` and its `net.stream` transport removed | the link's memory on the P4, unmeasured |
| Pairing | Link 1's QR pairing (`pocketlink://pair`) goes, as version 2 retires it (link §1 "What version 2 retires"). **The Tab5 keeps `data.fs` on its microSD card, so Hub's card setup** (`setup.card`, link §3.2) fits: the owner puts the card in the Mac, Hub writes `device.txt` and `companions/` | a Tab5 without a card (its `storage` partition); a USB path (Q9) |
| The first install of its firmware | today `idf.py flash`. Hub's setup has a per-family installer (`crates/hub-setup`, `setup.install {kind}`); a kind `tab5` that flashes a pocket-m5 release over USB through the ESP32-P4's ROM bootloader | updates afterwards over the link: an OTA path in the firmware for `runtime.native.install`, or USB again (Q9) |
| A carrier | Wi-Fi TCP as the 3DS; USB later fits the endpoint's carrier abstraction (`crates/hub-core/src/carrier.rs`) | not needed for the first step |
| Its worker's HTTP | the firmware applies `applet.hosts` and the destination policy (link §6.5) in its HTTP node, since Hub never sees those requests | Q8 |
| `device.accepts` | its formats, density (its host contract states raster density 1 for the 640 × 360 surface, which the firmware's README calls "at 2x"), and media codecs from its own player (`components/pocket_media`) | the facts are the firmware's to state |
| Host contract | `service.channel`, `image.url`, `text.glyphs.deferred`, `media.url`; `link.companion` stays | |

**Until its firmware speaks version 2, a Tab5 has no back end for any applet
that used `companion.pocket`**: Hub refuses version 1 (link §1). That is the
cost of taking the new path everywhere, stated in `MIGRATION.md` §3.
