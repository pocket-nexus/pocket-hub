# Open questions

Proposal (see `README.md`). Everything the direction leaves undecided, and
every place where it meets existing code or the specification and does not
fit as written. Each question has the options found and a recommendation; an
assumption not listed here is a bug to report against this page. Questions
marked **conflict** are where the accepted direction does not survive contact
with the code or the specification as it stands.

## Names

**Q1. The names of `README.md` §3.** Every row there is a proposal.
*Recommendation*: the proposed column; decide before step 1 of
`MIGRATION.md`, since the rename is the first change and every later diff
uses the result.

## Applets without a service

**Q2. Where an applet without a service keeps its state. Conflict.** The
direction's table gives `store` to services only, but GitHub keeps a token and
Weather its places through today's `store` service, and both become applets
without a service.
- (a) The device's `data.fs` (the 3DS host provides it; Notes uses it): the
  state lives with the applet, and works with no computer.
- (b) A framework form of `store` over the system channel, kept by Hub per
  applet id: the state follows the owner across devices paired with one Mac,
  and is lost to a device on another Mac.
- (c) A small service for each such applet: GitHub and Weather stop being
  examples of the first kind.

*Recommendation*: (a). It needs no capability, and GitHub's token already
crosses Hub in every request header; that it sits on the SD card in plain
text is stated in the applet's sign-in screen.

**Q3. How GitHub signs in without a service.** Today the owner types a
personal token on the console.
- (a) Keep typing a token on the console's keyboard.
- (b) GitHub's device flow from the applet's worker: the console shows a code
  and "github.com/login/device", the owner confirms on any browser, the
  worker polls for the token. Needs an OAuth app registered by Pocket Nexus
  (its client id is public).
- (c) `signin.web` through a service: GitHub becomes a portable service.

*Recommendation*: (b).

**Q4. How Hub applies an applet's hosts to `net.http`. Conflict.** Hub's
policy has a per-applet seam (`AppPolicy` in `crates/hub-net`) that allows
every public destination today. To apply `applet.hosts`, Hub must know the
package the request comes from, and `net.http` carries `app` but not `hash`
(L§6.5).
- (a) Add `hash` to `net.http`, as the runtime's assertion beside `app`
  (proposed in the specification change), and look the package up by it.
- (b) Look up the hosts by `app` alone from the newest package Hub knows for
  that id: wrong while two versions run.

And for a package Hub does not know (pushed from another computer):
- (i) refuse its requests with a sentence saying to install the applet from
  this Mac;
- (ii) allow every public destination for it, as today.

*Recommendation*: (a) with (i). (ii) is the fallback the owner's rules
forbid; note that (i) changes what a developer pushing from the PocketJS
reference CLI sees.

## Hosts and destinations

**Q5. Host lists that cannot be written down in advance. Conflict.** The
direction's fixed `hosts` fits X and Discord and does not fit three applets:
- YouTube's media addresses are per-server names under `googlevideo.com`;
- Podcasts fetches feeds, audio and art from wherever each podcast lives;
- Home Assistant talks to the owner's own instance, on a private address the
  destination policy refuses.

Options: (a) suffix wildcards `*.name` (`MANIFEST.md` §2, proposed); (b) an
entry meaning any public host, shown to the owner as "Can reach any website";
(c) owner-named origins (Q6); (d) make these services `native`.

*Recommendation*: (a) and (c) now. For Podcasts, (b) with the disclosure,
since (d) would give a feed reader every right on the computer; the owner
decides whether (b) exists at all.

**Q6. Owner-named origins and private addresses.** A `signin.form` field of
kind `origin` adds the origin the owner typed to the service's destinations,
private addresses included for it alone (`CAPABILITIES.md` §6). Options: as
proposed; or a separate prompt per origin; or no private destinations at all
(Home Assistant only through Nabu Casa's public URL). *Recommendation*: as
proposed: the owner typed it in Pocket Hub's own form on the Mac, which is
the consent.

**Q7. `signin.form`.** The direction's `signin` is a window. `form` is
added for tokens the owner pastes (Discord's bot token, Home Assistant's
token and URL). Options: keep `form`; drop it and let applets collect tokens
on the console. *Recommendation*: keep it. In both cases, **when no copy of
Pocket Hub's app is running, a sign-in fails with `unavailable`** and the
applet says to open Pocket Hub; `hubd` does not start the app (that would be
a second path nobody asked for). The owner may want the opposite.

## The device's facts and the system channel

**Q8. Where the device states what it takes. Conflict.** `channel.open`
carries `{app, hash}` and nothing about the device, and Hub cannot know from
a model name whether a 3DS has the H.264 decoder or whether its sound starts
(`MediaStatus.h264`, `audio`). Options:
- (a) an `EVENT device.accepts` once after `ready` and on change, per
  connection (proposed, L§7.9);
- (b) the same object in every `channel.open`;
- (c) a table of targets in Hub.

*Recommendation*: (a). (c) is the device knowledge in the wrong place, the
rule `README.md` §1 item 3 forbids.

**Q9. When the device opens the system channel, and the capability names.**
Options: (a) before boot when the plan has `image.url`,
`text.glyphs.deferred` or `media.url`, as the applet's channel opens for
`link.channel` today; (b) at the framework's first use. *Recommendation*:
(a): the same rule as the existing channel, and the open's latency falls
inside the three states. The three names are Q1's.

**Q10. The other text contracts.** PocketJS has `text.font`/`text.glyph`
(`contracts/spec/text.ts`), `text.layout` and its siblings
(`docs/TEXT-OFFLOAD.md`) and `font.*` (`docs/DYNAMIC_TEXT.md`), all over
`io.offload`. Options: move them onto the system service in this change;
later; never. *Recommendation*: later, unchanged in name and object, when an
applet needs shaping; this change carries `text.atlas`, which five applets
use today. Whether `io.offload`'s TCP 8741 transport then retires is the
specification's §11 question.

**Q11. Media.** (a) **The transcoder on the Mac**: ffmpeg (downloaded or
bundled; its licence matters) or AVFoundation for decoding and VideoToolbox
for H.264 encoding (no download; whether VideoToolbox gives one slice per
frame without B-frames, as `docs/MEDIA.md` requires, is not verified), with
PAL8 and ADPCM encoders of Hub's own. (b) **Inputs with separate sound and
picture** (most YouTube formats above 360p): a `feed` from a joining program
in the service, or two inputs to `media.open`. (c) **A pass-through form**
for a device that plays the source itself: needs a source other than the
PKMV stream; not proposed. (d) **Media on the link**: the specification keeps
the side TCP connection (decided); unchanged here. *Recommendation*:
AVFoundation and VideoToolbox, measured against the MVD decoder before
choosing; one input, with `feed` for joined streams.

## Releases and identity

**Q12. Two releases of one service at once.** A device on version 3 and
another on version 4: (a) one process per release, sharing the id's store
(proposed); (b) one release at a time, the second device refused `busy`
until the first closes. *Recommendation*: (a), with the rule stated to
authors that a store written by one release may be read by another.

**Q13. A development install.** Proposed: it serves every hash of its id and
takes precedence over store releases while installed (`TRUST.md` §8).
Alternative: it serves only hashes Hub pushed itself. *Recommendation*: as
proposed; it means a development install of X's service would serve any
applet that claims X's id, which is the developer's own choice on their own
computer.

**Q14. Hub's key pin.** Proposed: Hub pins the first key that signed a
release of an id (`TRUST.md` §7). Alternative: trust the store's record
alone. *Recommendation*: the pin; it costs a file, and it is what stops a
store that changed hands from changing the code on Macs that trusted it.

**Q15. The store release's package form.** (a) Bundled, no install at Hub
(proposed); (b) a lockfile with `bun install --frozen-lockfile
--ignore-scripts` at Hub. *Recommendation*: (a): nothing runs at install, and
the checking tool reads exactly what the signature covers.

**Q16. Consent for an unrestricted applet installed from the console.** The
console's store page installs over the link (L§6.4) with no window on the
Mac. Options: (a) install the applet, hold its service until the owner
allows it in Pocket Hub, and refuse its `channel.open` meanwhile with a new
code `consent` (retried at the applet's next start), so the console can say
"Allow YouTube in Pocket Hub on your Mac"; (b) refuse the install from the
console with a sentence; (c) show the disclosure on the console and take the
owner's yes there (a catalog row field marking unrestricted applets).
*Recommendation*: (a) now, and the catalog row field of (c) so the console's
store page can mark the applet before install. Both are link changes and are
listed in the specification proposal's §11.

**Q17. The second signature and Omarchy Pocket.** Proposed: releases with a
service keep `signature` (version 1) and add `signature2` (`TRUST.md` §6), so
Omarchy Pocket's devices keep verifying. Alternatives: a version-2 message
only (breaks those devices for such releases); sign the manifest's hash
instead of listing `requires` and hosts (loses the disclosure before
download). *Recommendation*: as proposed; the store's owner decides the
prefix (`omarchy-pocket-store/2`) and the line-break rule.

## The sandbox

**Q18. The mechanism of step two.** `sandbox-exec` (measured, deprecated),
App Sandbox (not tried), an embedded JavaScript engine with no host
interface, or Linux mechanisms for a Linux Hub (`TRUST.md` §5).
*Recommendation*: `sandbox-exec` now; the embedded engine as its planned
replacement, which also answers how a device would host the same services.

**Q19. The checking tool.** Where it runs (publisher, store, Hub at install)
and its list of allowed globals (`TRUST.md` §3) are part of the portability
contract. Open: `crypto.subtle` and `structuredClone` in the list (every host
must then provide them); whether `console` is allowed or `host.log` only.
*Recommendation*: the list as written; Hub runs the tool at install.

**Q20. A portable service's control-socket access until step two.** Accepted
by the owner (no separate patch). Listed so that nobody reads step one as a
fix: until step two, any service can drive Hub's control API.

## Limits and storage

**Q21. The guest's hand-over budget with two channels. Conflict.** L§7.3
gives each channel 16 messages or 8 KiB of JSON and 16 KiB of stream bytes a
frame; with two channels the guest could take twice that in one frame, which
the budget exists to prevent. Options: (a) one budget per guest, shared by
its channels in turn; (b) per channel, as today. *Recommendation*: (a), in
the specification change.

**Q22. Secrets in the store.** Values are files of mode 0600 in Hub's 0700
home (`CAPABILITIES.md` §5). Options: as proposed; a `secret` flag that puts
the value in the macOS Keychain (Keychain prompts and entitlements for
`hubd` not investigated). *Recommendation*: as proposed now; revisit when Hub
is signed and notarized.

**Q23. A native service's files folder.** The direction replaces the data
folder by `store`, but a native service keeps programs (`yt-dlp`) that do not
fit a key-value store. Proposed: `host.native.files`, a 0700 folder for
native services only. *Recommendation*: as proposed.

**Q24. YouTube's manifest.** The direction writes `["media", "native"]`;
thumbnails need `image` too, or the service encodes for the 3DS itself
(`WALKTHROUGH.md` §3). *Recommendation*: `["image", "media", "native"]`.

**Q25. The numbers chosen here without measurement**: the host connection's
payload limit (65,536), `net` limits (16 KiB URL, 16 MiB body, 64 MiB
response, 300 s, 16 fetches, 8 MiB WebSocket message), store limits (4 MiB
value, 16 MiB default, 256 MiB maximum), image (16 MiB, 50 megapixels, 1,024
a side), text (2,048 codepoints, 1 MiB), media (10 minutes paused). Each has
a reason beside it and none was measured. *Recommendation*: keep them until
a pilot meets one.

## The applet contract

**Q26. `applet.services` and the store's `services` member.** Proposed:
removed from the applet contract with the companion origin; an applet
declares `service.channel` exactly when its release has a service
(`MANIFEST.md` §4). The specification's applet allowlist and
`docs/APPLETS.md` change with it, and `APPLET_MODULES` must admit the
framework's service, image and glyph modules (today it lists no `channel`
module, so an applet under `tools/applet-check.ts` cannot import
`@pocketjs/framework/channel` at all). *Recommendation*: as proposed, in the
specification change.

**Q27. Discord's gateway and the idle stop.** A service stops 30 s after its
last channel closes (`idleSeconds`), and its gateway WebSocket with it; the
applet catches up from history when it opens. Options: as is; a longer
`idleSeconds` for Discord (up to a day, disclosed by nothing). *Recommendation*:
as is.

**Q28. Maps' `/locate`.** No capability gives a position. Options: an IP
geolocation service reached with `net` (approximate; another host to
declare); a `location` capability (CoreLocation on a Mac; an eighth
capability); drop the button. *Recommendation*: IP geolocation with `net`;
an eighth capability needs the owner.

**Q29. Spotify and the App Store applet** are built for Omarchy Pocket
(`rg-ds-plus`) only, where omarchy-pocketd answers their services on the
device. Whether Omarchy Pocket becomes a host of this contract is outside
this proposal. *Recommendation*: leave both out of the pilots and the
"remaining applets" step until they target a 3DS.

**Q30. The SDK examples.** `examples/sample` uses `Bun.hash.crc32` and
`examples/link-channel` starts an HTTP server with `Bun.serve`; neither passes
the checking tool. *Recommendation*: rewrite `sample` without Bun's
interfaces; move the HTTP fixture into the test harness (`MIGRATION.md` step 2).

## Wire and protocol

**Q31. New `channel.open` refusals.** Proposed in the specification change:
`no_service` (renamed), `capability` (the host does not provide a capability
the service requires; `missing` lists them; not retried during the run),
and `consent` (Q16). *Recommendation*: all three.

**Q32. The local protocol's version.** Proposed: `protocol` 2 with the
renamed handshake messages, and Hub's SDK copy always linked
(`HOST-PROTOCOL.md` §3). *Recommendation*: as proposed.

**Q33. A device that hosts its own services.** Not built, and nothing here
may block it. What this proposal leaves to that design: how a device chooses
between its own host and Hub for a service (its own when it provides every
capability the release requires, Hub otherwise, is the obvious rule), and how
the device learns a release's `requires` before it opens (the signed catalog
members of `TRUST.md` §6 carry them). Nothing in the wire names Hub where the
system service or a service is meant; the SDK names no Bun interface.

**Q34. The role name `companion`** in the link's fields stays
(L§1 keeps it on purpose). *Recommendation*: leave it.

**Q35. Error words for the owner.** New control API kinds and reasons
(`consent`, `service` with `manifest`, `check`, `runtime`; `store` with
`key`) need sentences in English and Simplified Chinese in the app
(`AGENTS.md`). *Recommendation*: written with step 1 to step 8 as each kind
lands.
