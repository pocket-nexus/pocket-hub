# Open questions

Proposal (see `README.md`). Everything the direction leaves undecided, and
every place where it meets existing code or the specification and does not
fit as written. Each question has the options found and a recommendation; an
assumption not listed here is a bug to report against this page. **Conflict**
marks a place where the accepted direction does not survive contact with the
code or the specification as it stands.

**Order.** §1 blocks the first implementation steps (`MIGRATION.md` steps 1
to 3: the rename, the host connection with `store` and the enclosure, `net`);
§2 blocks a later step; §3 can wait for the step named.

**Changed in this revision** (the owner's four decisions and an independent
review): the numbers are new again. **Decided and removed**: whether Omarchy
Pocket and the Tab5 keep `companion.pocket` (no: Omarchy Pocket becomes a
host, the Tab5 moves to Pocket Link 2); the LAN attacker without the key
(accepted until link encryption, `TRUST.md` §9); a helper process for
decoding (no: decoding is performance-first, in the host). **New**: Q2 (who
implements Omarchy Pocket's host), Q5 (the Linux enclosure), Q6 (sign-in on
Omarchy Pocket), Q8 and Q9 (the Tab5), Q42 (installs on Omarchy Pocket), Q43
(Spotify). **Changed recommendations**: Q3 (one answer for all three
devices: `data.fs` on Omarchy Pocket), Q4 (the control socket closed in this
work, from step 2), Q11 (firmware without `hash`, the package index), Q12
(the Public Suffix List), Q13 (where owner-named origins live), Q14 (the
frame and the pattern dialect), Q15 (`text.atlas` body and `omitted`), Q17
(sound alone, the media listener), Q18 (which size limit applies), Q19 (a
development install's own store), Q20 (pins for every id, lost keys), Q22
(console installs of a release with a service), Q23 (the applet-hosts line
is for display), Q24 (the enclosure from step 2), Q26 (what remains possible
without the enclosure), Q29 (vendor tools), Q31 (smaller limits).

## 1. First rank: blocks the first implementation steps

**Q1. The names of `README.md` §3.** Every row there is a proposal. Two rows
keep "link": the capability `link.companion`, `globalThis.link` and
`@pocketjs/framework/link` are the console's link itself (pairing, status,
catalog, installs), where "link" names the thing and not where an applet's
code runs. Options: keep them; rename them with the rest.
*Recommendation*: the proposed column, and keep the three "link" names;
decide before step 1, since every later diff uses the result.

**Q2. Who implements the host on Omarchy Pocket.** (A) Hub's host parts
factored into `crates/pocket-host`, which `pocket-hubd` embeds and Omarchy
Pocket runs as `pocket-hostd` on Linux aarch64; (B) omarchy-pocketd
implements the contract again in TypeScript. Costs in `DEVICES.md` §4: (A)
needs a Linux enclosure, portable capability implementations (pure-Rust image
decoders, PocketJS's Rust text engine, media per platform) and Linux CI;
(B) is a second implementation of every rule, kept equal by tests.
*Recommendation*: **(A)**, as a separate process from omarchy-pocket-host;
it decides how steps 2 to 4 are written (the host parts land in a crate of
their own from step 2).

**Q3. Where an applet without a service keeps its state.** The direction
gives `store` to services only; GitHub's token and Weather's places belong to
applets without one. The 3DS and the Tab5 provide `data.fs`; Omarchy
Pocket's host contract does not. Options: (a) **Omarchy Pocket gains
`data.fs`** and every device keeps the state there; (b) a framework form of
`store` over the system channel (the state then lives with the host: on the
Mac for a 3DS, which a device paired with another Mac does not see); (c) a
small service for each such applet. *Recommendation*: **(a)**. Cost: the
Omarchy host program implements `contracts/spec/fs.ts` (it can compile
PocketJS's `engine/host-services/fs.c`), and on the 3DS a token lies on the SD
card in plain text (it crosses Hub in every request header anyway).
`DEVICES.md` §7.

**Q4. Closing the control socket's exposure. Conflict** (today any daemon can
call every control method, `README.md`'s rule "a daemon gets no device
operation" notwithstanding). Proposed, in this work and before the first
capability that holds anything (`TRUST.md` §2):
- (a) **the OS enclosure for portable services from step 2**, the step in
  which `store` first exists: a sandboxed service cannot reach `hubd.sock`,
  files or the network;
- (b) **in `pocket-hubd`, on each accept, the peer's process**
  (`LOCAL_PEERPID` on macOS, `SO_PEERCRED` on Linux): a control connection
  from a process Hub started as a service, or from one of its descendants,
  is refused; and **the privileged methods** (finishing a sign-in, consent
  to an unrestricted release, installing a development release) are taken
  only from a client Hub identifies as its own app (on macOS, the peer's code
  signature matches `hubd`'s own; on Linux, an executable owned by root in
  the host's installed path), **never from a `remote` client**;
- (c) a development release has its own store.

What remains, said plainly: without (a), a determined process of the owner's
user can read every store (files of that user) and call the unprivileged
methods; (b) does not stop a process that controls the app itself (a
debugger on an app without the hardened runtime). **A native service is
unsandboxed by definition**: it can detach from Hub's process tree, read
every store and drive the control API's unprivileged methods; that is what
its disclosure says. *Recommendation*: (a), (b) and (c) as written.

**Q5. The enclosure on Linux.** On Omarchy Pocket: bubblewrap where
unprivileged user namespaces are allowed, Landlock and seccomp where not; on
the AYN Thor, where the root runs under proot, neither is measured
(`DEVICES.md` §6). Options there: (a) do not run portable services that
require `store` or `signin` until a mechanism is measured; (b) run them
unenclosed with a notice; (c) seccomp alone. *Recommendation*: measure on the
RG DS Plus and the uConsole before step 2 ships there, and (a) on the Thor:
(b) would be the fallback the owner's rules forbid.

**Q6. `signin` on Omarchy Pocket. Conflict.** The direction lists `signin` for
every host, and the device has no web view (its shell is a PocketJS program
on GBM/EGL; Omarchy's desktop session is not installed). `form` can be drawn
by the shell; `web` cannot today. Options: (a) a WPE WebKit view on EGL in
the shell (untried on the Mali-G52); (b) the host provides `form` alone, and
a service that needs `web` (X) states it, so its manifest can be refused
there (`requires` would name `signin.web` and `signin.form` apart); (c) sign
in on a Mac and copy the session, as owners do today (`docs/omarchy-pocket.md`
"Companion services"). *Recommendation*: (b) now, with the two names, so a
host can provide one; try (a) before X's release on Omarchy Pocket.

**Q7. Where the device states what it takes. Conflict.** `channel.open`
carries `{app, hash}` and nothing about the device, and a host cannot know
from a model name whether a 3DS has the H.264 decoder or whether its sound
starts. Options: (a) an `EVENT device.accepts` per connection (link §7.9);
(b) the same object in every `channel.open`; (c) a table of targets in the
host. *Recommendation*: (a). (c) puts device knowledge in the wrong place.

**Q8. Who applies `applet.hosts` on the Tab5.** Its worker's HTTP goes
through the firmware's own stack, which Hub never sees. Options: (a) the
firmware applies `applet.hosts` and link §6.5's destination policy in its
HTTP node, reading the hosts from the package it runs; (b) the Tab5 sends
its HTTP over the link as the 3DS does (`net.http`), so Hub applies them,
at the cost of every request crossing the Mac. *Recommendation*: (a); the
policy is a PocketJS rule each host applies where the request is performed.

**Q9. The Tab5's pairing and first install.** It has no ftpd; its firmware is
flashed with `idf.py` today. Options for the pairing: (a) Hub's card setup
(`setup.card`), since the Tab5 keeps `data.fs` on a microSD card; (b) over
USB. For the first install: (c) a `tab5` installer in Hub's setup that flashes
a pocket-m5 release over USB through the ESP32-P4's ROM bootloader; for
updates afterwards, (d) an OTA path in the firmware for
`runtime.native.install` or (e) USB again. *Recommendation*: (a), (c) and (d),
designed in pocket-m5 and `crates/hub-setup` (`DEVICES.md` §8).

**Q10. Protocol versions.** The local protocol's `protocol` becomes 2 with the
renamed handshake (`HOST-PROTOCOL.md` §3). The control API's methods are
renamed too: with `hub.hello` at protocol 1, a client from before the rename
gets "method not found" instead of a refusal that says why.
*Recommendation*: both at 2 in step 1; an older client is refused at
`hub.hello` with `-32602` and a sentence.

## 2. Second rank: blocks a later step

**Q11. How Hub applies an applet's hosts to `net.http`. Conflict.** Blocks
step 7. Hub's per-applet seam (`AppPolicy` in `crates/hub-net`) allows every
public destination today; to apply `applet.hosts` Hub must know the package,
and `net.http` carries `app` but not `hash`.
- Add `hash` to `net.http` (link §6.5 as proposed).
- **The index**: Hub keeps `<home>/packages.json`, `(app, footer hash) →
  {version, hosts, source}`, written when it pushes a package
  (`devices.push`, `store.install`, a console's `install`) or verifies one in
  its store cache; it survives restarts. The host on Omarchy Pocket keeps the
  same for the packages it installs.
- **A package not in the index**: (i) refuse with a sentence saying to install
  the applet from this Mac; (ii) every public destination, as today.
- **A request without `hash`** (a 3DS program from before the change): (iii)
  refuse with "Update the 3DS's software in Pocket Hub"; (iv) treat it as (ii).

*Recommendation*: `hash` with (i) and (iii); (ii) and (iv) are the fallback
the owner's rules forbid.

**Q12. Host lists that cannot be written down in advance. Conflict.** YouTube's
media addresses are per-server names under `googlevideo.com`; Podcasts
fetches from wherever each podcast lives; Home Assistant talks to the owner's
own instance. Options: (a) suffix wildcards `*.name`, **refused when `name` is
a public suffix of the Public Suffix List, its private section included**
(`*.co.uk`, `*.github.io`, `*.workers.dev`); (b) an entry meaning any public
host, shown as "Can reach any website"; (c) owner-named origins (Q13); (d)
make these services `native`. *Recommendation*: (a) and (c); for Podcasts,
(b) with its disclosure if the owner wants (b) at all. Hosts and the store
carry a dated copy of the list.

**Q13. Owner-named origins.** A `signin.form` field of kind `origin` adds the
origin the owner typed to the service's destinations. Proposed: kept by the
host (`<home>/services/<id>/origins.json` on Hub), never in the service's
store; a scheme, a host (name or address literal) and a port, so
`http://192.168.1.20:8123` is expressible; **private addresses allowed for
it, loopback, link-local (169.254.0.0/16, fe80::/10), unspecified and
multicast never**; a name is pinned to the addresses it resolved to when
saved and checked again at each connect, so DNS rebinding to 127.0.0.1 is
refused; listed for the owner to remove, and removed with the service's data.
*Recommendation*: as proposed; meaningful only with Q4(b) (the form's values
must come from the host's own app).

**Q14. `signin.form`, and the frame of every sign-in.** The service writes
`title`, `help`, `label` and `link`, so it could ask for "your Mac password".
Proposed: the host frames every window and form with the applet's name, icon
and publisher ("Development" for a development install) and, under a secret
field, "This goes to X, not to Pocket Hub or macOS"; `link.url` and `web.url`
are `https:` only; `pattern` uses the Rust `regex` crate's syntax (no
backreferences or lookaround, linear time), checked by the host's core, not
by the app. When no copy of the app is running, a sign-in fails with
`unavailable`; the host does not start the app. *Recommendation*: as proposed.

**Q15. Text.** `text.atlas` sends its codepoints as a request body (up to
4,096, which do not fit LINK_APP_MSG) and returns `omitted` for those that
did not fit the atlas. `channel.text` for services stays among the seven
names and is not implemented until a service needs it; a release that
requires it is refused at install until then. The other PocketJS text
contracts over `io.offload` move later, unchanged in name and object.
*Recommendation*: as written.

**Q16. When the device opens the system channel, and the glyph capability.**
(a) before boot when the plan has `image.url`, `text.glyphs.deferred` or
`media.url`; (b) at the framework's first use. And (c) a new id
`text.glyphs.deferred`; (d) give `text.glyphs.streamed` (a font archive's
cells over `io.offload`) this meaning; (e) serve `text.glyphs.runtime` (the
host extends atlases as text renders, no state the applet sees).
*Recommendation*: (a) and (c).

**Q17. Media.** The transcoder per platform (a Mac: AVFoundation and
VideoToolbox, whose one-slice-per-frame output is not verified, or ffmpeg;
Linux: ffmpeg); sound alone (`video: false`) and a `durationMs` hint, for
Podcasts (proposed); inputs with sound and picture apart (most YouTube
formats above 360p: not covered with the `feed` dropped; options: a second
input to `media.open`, format 18 only); **the side connection's listener**:
bound at the host's start on a port the OS assigns, carried in the source, so
no fixed port conflicts; a bind that fails stops the start with one line; it
serves TCP only, so a device on another carrier states no `media`; a
pass-through form for a device that plays the source itself is not proposed.
*Recommendation*: AVFoundation and VideoToolbox on a Mac, measured first;
format 18 for the YouTube pilot; the listener as proposed.

**Q18. Two releases at once, and the store's size.** (a) one process per
release, sharing the id's store (proposed); (b) one release at a time. The
store's limit is the newest installed release's `store.maxBytes`. **A release
that raises it**: (c) silently; (d) shown in the update's text ("Keeps up to
64 MB on this Mac"). *Recommendation*: (a) and (d).

**Q19. A development install.** Serves every hash of its id, takes
precedence over store releases while installed, and **has a store of its
own** (`<id>@dev`), so one under X's id cannot read X's session; installing
it is a privileged method (Q4). *Recommendation*: as proposed.

**Q20. Key pins and lost keys.** The host pins the key of the first signed
release it installs **for every id, with or without a service**. Open: a
publisher who loses or rotates a key. Options: the pin blocks until the owner
removes the applet; a rotation statement signed by the old key naming the
new; the official key countersigns a rotation. *Recommendation*: the rotation
statement, removal for a lost key.

**Q21. The store release's package form.** (a) bundled, no install at the
host; (b) a lockfile with `bun install --frozen-lockfile --ignore-scripts`.
*Recommendation*: (a).

**Q22. Installs from a console.** (1) A release with a service installed from
the console's store page (link §6.4): Hub installs the service before it
streams the applet, as `store.install` does. (2) An unrestricted one: no
window on the Mac at that moment. Options: (a) install, hold the service
until the owner allows it in Pocket Hub, and refuse `channel.open` with
`consent` meanwhile; (b) refuse the install with a sentence; (c) a catalog
row field so the console's store page marks such applets. *Recommendation*:
(1); (a) with (c).

**Q23. The second signature.** Releases with a service keep `signature`
(version 1) and add `signature2`, so Omarchy Pocket's devices keep verifying
until their host checks the second. The applet package's SHA-256 already
covers its manifest and `applet.hosts`; the second message's applet-hosts
line is for showing them before a download, and the host checks it against
the package. *Recommendation*: as proposed; the store's owner decides the
prefix.

**Q24. The enclosure on macOS, and its place.** `sandbox-exec` (measured,
deprecated), App Sandbox (not tried), an embedded JavaScript engine with no
host interface. **From step 2**, the first step with a capability.
*Recommendation*: `sandbox-exec` from step 2; the embedded engine as its
planned replacement, which also fits a weak device that hosts services.

**Q25. The source check.** Where it runs and its list of allowed globals are
part of the portability contract. Open: `crypto.subtle` and `structuredClone`
in the list; `console` or `host.log` only. *Recommendation*: the list as
written; every host runs it at install.

**Q26. What a same-user process can still do.** With Q4 in place: a native
service or any other program of the owner's user can read every store (files
of that user), call the control API's unprivileged methods (devices
included), and attach to the app if its hardened runtime allows. Listed so
that nobody reads the enclosure as covering more than services.

**Q27. The guest's hand-over budget with two channels. Conflict.** Link §7.3's
per-frame budgets are each channel's in the C client today; with two
channels a frame could take twice as much. *Recommendation*: one budget per
guest, shared in turn, in the specification change.

**Q28. Secrets in the store.** Files of mode 0600 in the host's 0700 folder.
Options: as proposed; the macOS Keychain or the Secret Service on Linux.
*Recommendation*: as proposed now.

**Q29. A native service's files and vendor tools.** Proposed:
`host.native.files`, a 0700 folder. `docs/APPLETS.md`'s **Vendor tool** (the
owner consents to the vendor's terms; a SHA-256 pin; a refresh schedule)
would otherwise become the service's own business. Options: leave it there; a
`host.native.tool({url, sha256, terms})` call that shows the vendor's terms
in the host's frame and keeps the tool; the tool named in the manifest for
the store's editor. *Recommendation*: the manifest entry and the host call,
before YouTube or Spotify is released.

**Q30. YouTube's manifest.** The direction writes `["media", "native"]`;
thumbnails need `image` too. *Recommendation*: `["image", "media", "native"]`.

**Q31. Limits sized to the applets**: `net` body 256 KiB, response 4 MiB
(1 MiB default), WebSocket message 64 KiB sent and 1 MiB received, URL
16 KiB; store value 4 MiB, 16 MiB per store by default, 256 MiB at most;
image source 16 MiB and 50 megapixels; atlas 4,096 glyphs and 1 MiB; media
paused 10 minutes; host connection payload 65,536. None measured.
*Recommendation*: keep them until a pilot meets one.

## 3. Can wait for the step named

**Q32. `applet.services` and the store's `services` member** (step 9). Removed
from the applet contract; an applet declares `service.channel` exactly when
its release has a service; `APPLET_MODULES` admits the service, image and
glyph modules (it lists no channel module today).

**Q33. How GitHub signs in without a service** (step 6): its device flow from
the worker (an OAuth app registered by Pocket Nexus), rather than a token
typed on a console.

**Q34. Discord's gateway and the idle stop.** A service stops 30 s after its
last channel closes, its WebSocket with it. *Recommendation*: as is.

**Q35. Maps' `/locate`.** No capability gives a position. *Recommendation*:
IP geolocation through `net`; an eighth capability needs the owner.

**Q36. The SDK examples** (step 2). `sample` uses `Bun.hash.crc32`,
`link-channel` starts `Bun.serve`. *Recommendation*: rewrite `sample`; move
the HTTP fixture into the test harness.

**Q37. New `channel.open` refusals**: `no_service`, `capability` (with
`missing`), `consent`. An older C client takes any as "not during this applet
run". *Recommendation*: all three.

**Q38. A host on a weak device** (later than Omarchy Pocket): how a device
chooses between its own host and a remote one (its own when it provides
every capability the release requires), and how it learns `requires` before
it opens (the signed catalog members). Nothing in the wire or the SDK names
Hub.

**Q39. The role name `companion`** in the link's fields stays.

**Q40. Error words for the owner**, in English and Simplified Chinese, as
each control API kind lands.

**Q41. Program releases for the pin move** (step 5). The specification's
implementation reaches a 3DS through a release of its program; a console on
an older program keeps one channel (no system channel, no `device.accepts`)
and Pocket Hub offers the update. The Tab5 has no version-2 firmware at all
until pocket-m5 builds one. *Recommendation*: move the pin with a 3DS program
release.

**Q42. Installs on Omarchy Pocket** (step 8). The Store applet installs
packages through omarchy-pocketd's `appstore` service today. Under Q2(A) the
host on the device installs releases (applet and service, the checks of
`TRUST.md` §7), and the Store applet becomes a client of it, with the native
disclosure in the shell. *Recommendation*: as described, the store client
factored with the host.

**Q43. Spotify on Omarchy Pocket** (step 10). Its service runs a Rust program
(`omarchy-pocket-spotify`) that plays through PipeWire: an unrestricted
service there, with a vendor tool (Soloist) under Q29. *Recommendation*: port
it after YouTube.
