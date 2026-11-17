# Sandbox, identity and signing

Proposal (see `README.md`). Two questions: what a service can do on the
computer or device that hosts it (§1 to §5), and which code a host agrees to
run for which applet (§6 to §8); §9 says what is covered and what is not.
"The host" is Hub on a Mac or the host on Omarchy Pocket (`DEVICES.md`);
where they differ the text says so.

## 1. The rules

| Rule | Portable service | Native service |
| --- | --- | --- |
| R1. Its only exit is the host's service socket: the control, channel and host connections | yes | no |
| R2. It reads its own package, the host's copy of the SDK and Bun's installation; it writes nothing | yes | no |
| R3. Every capability call is checked by the host against the release's manifest (`requires`, `hosts`, `store.maxBytes`) and the destination policy | yes | for the SDK's calls only |
| R4. It starts no program and gets no environment beyond `HOST-PROTOCOL.md` §2 | yes | no |
| R5. It gets no device operation: the host's control socket is out of its reach | yes | no, and the owner is told (§4) |
| R6. The host logs each capability call without secrets (`HOST-PROTOCOL.md` §7) | yes | for the SDK's calls |
| R7. A process the host starts inherits none of its descriptors (today's rule) | yes | yes |

**The SDK is code inside the process**, so nothing the SDK refuses is
refused for good: the host repeats every check (R3). The SDK checks first so
that a mistake fails at once with a clear code.

**Two mechanisms, one boundary.** The *source check* (§3) refuses a service
that imports anything but the SDK; it makes intent visible and catches
mistakes, and **it is not a boundary**. The *enclosure* (§5) is the operating
system's, and is the boundary. **The enclosure is on from the first step in
which a capability exists** (`MIGRATION.md` step 2, where `store` arrives),
not after the pilots: a service holding a session before it would be
readable by any other service.

## 2. The control socket

Today a daemon runs as the owner's user and can connect to `hubd.sock` and
call every method, `devices.installProgram` included, although "a daemon gets
no device operation" is a rule of this repository. The owner decided to close
this as part of this work. Four layers:

| Layer | Mechanism | What it stops |
| --- | --- | --- |
| **(a) the enclosure** (§5) | a portable service can connect to the service socket alone; on macOS the profile names one socket path, on Linux the service sees no other socket | every portable service, from step 2: it cannot reach `hubd.sock`, any file or the network |
| **(b) the peer check in `pocket-hubd`** | on each accept, the peer's process id (`LOCAL_PEERPID` on macOS, `SO_PEERCRED` on Linux); **a control connection from a process the host started as a service, or from one of its descendants, is refused** | a service that escaped (a) driving the control API |
| **(c) consent-class methods: the owner's own click** | **decided on 2026-11-17 for consent to an unrestricted service** (`services.consent`, and `consent` on an install); proposed for granting a private origin (`services.grantOrigin`, `QUESTIONS.md` Q4). Taken only from Pocket Hub's app itself: on macOS the peer's code signature carries `hubd`'s team identifier **and the app's own signing identifier**, which the command line in the same bundle does not have; on Omarchy Pocket, the shell's program (an executable owned by root in the installed path). **Never from the command line, a `remote` client or a `pocket-hub:` link** (`docs/DEEP-LINKS.md`: a link shows, it never changes a device) | an agent, a script or a remote client consenting to native code or widening a service's reach on the owner's behalf |
| **(d) a development release's own store** (§8) | `<id>@dev`; installing one is proposed for identified clients (the app or the command line, `QUESTIONS.md` Q5) | a development install under X's id reading X's session |

**What a determined process of the owner's user can still do** without (a),
and what (b) to (d) do not stop: hand a service a setup message of its
choosing (`SETUP.md` §2, `QUESTIONS.md` Q9), read every store (they are files of that user),
call the unprivileged control methods (the devices' operations included), and
control the app itself where its hardened runtime allows a debugger. **A
native service is unsandboxed by definition**: it can leave the host's
process tree (and so the descendant check), read every store, and drive the
unprivileged control methods. That is what its disclosure says (§4).

## 3. The source check

A command shipped with the SDK, `pocket-service check <package>`, and the same
check as a library, run by the publisher (pocket-nexus/applets `bun run
check`), by the store when it can (§7), and **by the host at the install of
every portable service**, which refuses one that fails (control API error
`service`, `reason: "check"`, with the first lines).

It inspects every module reachable from the manifest's `entry`, after Bun's
transpiler has removed types and comments (the method of PocketJS
`tools/applet-check.ts`).

| Rule | Reason |
| --- | --- |
| Imports are relative and stay inside the package, or are `@pocketjs/service` | a store release is bundled (`MANIFEST.md` §3) |
| No `bun:*`, `node:*` or bare Node built-in | the ways to files, programs and sockets |
| No `require`, no dynamic `import()`, no `import.meta` beyond `import.meta.url` | the module graph checked is the one that runs |
| No `eval`, no `Function` constructor | idem |
| No free reference to `Bun`, `process`, `fetch`, `WebSocket`, `XMLHttpRequest`, `EventSource`, `Worker`, `navigator`, `Deno`, and no computed member of `globalThis`, `self` or `window` | Bun's globals reach the network and programs without an import |

**Allowed globals**: ECMAScript's built-ins, `TextEncoder`, `TextDecoder`,
`URL`, `URLSearchParams`, `setTimeout`, `clearTimeout`, `setInterval`,
`clearInterval`, `queueMicrotask`, `structuredClone`, `atob`, `btoa`,
`console` (to the service's log), `performance.now`, `AbortController`,
`AbortSignal`, `crypto.getRandomValues`, `crypto.subtle`,
`crypto.randomUUID`. This list is also **what every host must provide**.

It fails with one line per violation, `<file>:<line>:<column> <rule>: <what>`,
exit status 1. **It does not catch** a global reached without its name
(`[].constructor.constructor("return Bun")()`); hence the enclosure.

## 4. Native services

A release whose manifest requires `native` runs **with the owner's rights and
outside the enclosure**, on a host that provides `native`: Hub, and the host
on Omarchy Pocket (`DEVICES.md` §5). **The host says so before installing it,
and installs nothing without the owner's yes**, given by the owner's own click
in the host's app (§2 (c)).

| Where | What the owner sees |
| --- | --- |
| Pocket Hub's store page | a mark: "Runs its own programs on your Mac" |
| Installing, in Pocket Hub | title "**YouTube runs its own programs on this Mac**", text "Other applets can do only what Pocket Hub allows. YouTube runs programs of its own, with the same access to your files, accounts and network that you have, and Pocket Hub cannot limit or watch what they do. Install it only if you trust Pocket Nexus, who published it." Buttons "Install" and "Cancel" |
| Installing on Omarchy Pocket | the same text with "on this device", in the shell |
| Installing from a console's store page | the applet installs; its programs wait until the owner allows them in Pocket Hub on the Mac, and the console says "Allow YouTube in Pocket Hub on your Mac" (`QUESTIONS.md` Q20) |
| A development install that requires `native` | the same dialog with "This applet is not from the store" in place of the publisher |
| A host without `native` | the applet says it needs a computer (`channel.open` error `capability`) |

The publisher's name is the store entry's `author`, which the signature
covers (§6). **A native release published with a key other than the official
one waits for an editor who reads the service's code** (§7); a service's
code, unlike a `.pocket`, can be read.

## 5. The enclosure

**macOS.** The host starts a portable service as `sandbox-exec -f <profile>
-D … bun …`, with this profile:

```
(version 1)
(deny default)
(allow process-exec (literal (param "BUN")))
(allow file-read* (literal (param "BUN")))
(allow file-read* (subpath (param "PACKAGE")) (subpath (param "SDK")))
(allow file-read-metadata)
(allow file-read-data (literal "/"))
(allow file-read* (subpath "/usr/lib") (subpath "/System/Library")
  (subpath "/usr/share/zoneinfo") (subpath "/usr/share/icu")
  (subpath "/private/var/db/timezone") (literal "/private/etc/localtime")
  (literal "/dev/urandom") (literal "/dev/random") (literal "/dev/null"))
(allow sysctl-read)
(allow network-outbound (remote unix-socket (path-literal (param "SOCKET"))))
```

**Measured on 2026-11-12 on macOS 27.2 with Bun 1.4.2** (Hub pins 1.4.4; not
repeated with it), a TypeScript module that imports a sibling module and a
module from the SDK folder, run with an empty environment:

| Operation | Result |
| --- | --- |
| import a relative module and the SDK's | worked |
| `crypto.getRandomValues`, `crypto.subtle.digest` | worked |
| `Intl.DateTimeFormat` with a time zone | worked **only after `/usr/share/icu` was allowed** |
| timers | worked |
| connect to the allowed Unix socket | worked |
| connect to another Unix socket | denied (`network-outbound`) |
| read a file outside the package | denied (`EPERM`) |
| write a file | denied (`file-write-create`) |
| start a program | denied (`process-fork`) |
| TCP to a public address | denied (`network-outbound`) |
| JIT | worked: Bun's signature carries `com.apple.security.cs.allow-jit` |
| Bun's transpiler cache | its writes are denied and Bun runs without it |
| Bun reads `/` and lists each folder above the entry and the working directory | `/` had to be allowed; the others are denied and Bun continues |

Not covered yet: a real service with the SDK's connections under load, Bun
1.4.4, an older macOS. They are measured before step 2 ships.

`sandbox-exec` and its profile language are marked deprecated and remain in
current macOS (used above on 27.2); build tools such as Bazel and Nix use them
(from their documentation, not checked here). Alternatives: the same engine
called as a function from a launcher (same deprecation); App Sandbox (a copy
of Bun signed by the host, the socket in an app group container; not tried);
**an embedded JavaScript engine with no host interface** (V8 through
`deno_core`, or JavaScriptCore), where the enclosure is the absence of
interfaces and which would fit a weak device that hosts services.
*Recommendation*: `sandbox-exec` from step 2, the embedded engine as its
planned replacement (`QUESTIONS.md` Q22).

**Linux (Omarchy Pocket).** bubblewrap where unprivileged user namespaces are
allowed (an empty network namespace, read-only binds of Bun, the package, the
SDK and the system libraries, the service socket bound in); Landlock and
seccomp where not; measured on each device as an implementation step. **On a
device with no measured enclosure** (decided on 2026-11-17): only services
published with the official key run, unenclosed; **any other service, portable
or native, does not run there**, its applet saying the device does not
support it yet (`channel.open` refusal `enclosure`, `DEVICES.md` §6;
development installs: `QUESTIONS.md` Q44).

## 6. The signed release

**Today**, Hub checks each catalog entry with the publisher's ed25519 key
(`crates/hub-applet-store/src/entry.rs`):

```rust
pub const SIGNED_PREFIX: &str = "omarchy-pocket-store/1";

pub fn signed_message(&self) -> String {
    [SIGNED_PREFIX, &self.id, &self.version, &self.package, &self.icon, &self.author, &self.summary].join("\n")
}
```

checked with `verify_strict` against the entry's `key`, with ids under
`dev.pocket-nexus.` accepted from the official key only. **The package's
SHA-256 covers the applet's manifest**, `applet.hosts` included; the catalog's
`hosts` member, which the message does not cover, is what the store read from
that package at publish (`store/src/index.ts` in the applets). **Line breaks**:
the store checks `author` as a handle of 2 to 30 lower-case letters, digits
or dashes, and only trims and length-checks `summary` (`store/src/index.ts`), so a summary can
hold a line feed; in the first message `summary` is the last line, so the
message still parses one way.

**Proposed**: a release with a service carries a second signature,
`signature2`, over this message (lines joined by `\n`, no final newline):

```
omarchy-pocket-store/2
<id>
<version>
<package sha256>
<icon sha256>
<author>
<summary>
<applet hosts, sorted, joined by ",">
<service package sha256>
<service requires, sorted, joined by ",">
<service hosts, sorted, joined by ",">
<1 if the service package carries a setup guide (SETUP.md), else 0>
```

- **The service package's SHA-256 binds the code to the applet**; its
  manifest is inside it.
- **`requires` and both host lists are written out** so the store page, the
  install dialog and a console show them before a download. The applet-hosts
  line is for that display: the package's hash already covers the hosts, and
  the host checks the line against the package (`MANIFEST.md` §6).
- **`summary` is not last here, so it may not hold a line break**: the store
  refuses one at publish for both messages.
- **The first signature stays**, for verifiers that know only it (Omarchy
  Pocket's devices until their host checks the second); a release without a
  service carries `signature` alone.

The entry gains `signature2` and `service: {package, bytes, requires, hosts}
| null`.

**The host checks at install** of a release with a service, before anything
runs:

1. `signature` and `signature2` verify with the entry's `key`; an entry with
   `service` and no `signature2` is refused;
2. the reserved prefix: `dev.pocket-nexus.` from the official key only;
3. the key is the one pinned for the id, or the first the host sees (§7);
4. the applet package matches its SHA-256 and is what the entry says, and its
   `applet.hosts` equal the signed line;
5. the service package matches its SHA-256; its manifest's `id`, `requires`
   and `hosts` equal the signed members; the applet declares
   `service.channel`;
6. for `native`: the owner's consent (§4); for a portable service: the
   source check (§3);
7. the release is recorded under the applet package's footer hash.

**At `channel.open {app, hash}`** the host starts the service of the release
whose applet package has that footer hash. Applet and service are always of
one release, so neither declares which versions of the other it works with.
**What this binding is worth**: the footer hash is FNV-1a64
(`contracts/spec/pocket-package.ts`), which anyone can match with a package
of their own, and `app` and `hash` are the device runtime's assertion. The
binding keeps an honest runtime from running another applet's service; it is
no defence against a peer that lies (§9).

## 7. Key ownership, and what the store must add

**Pins.** The store's rule stays (the first key to publish an id owns it).
**The host pins the key of the first signed release it installs for every id,
with or without a service** (`<home>/services/<id>/owner.json` on Hub), and
refuses a release of that id from another key (control API error `store`,
`reason: "key"`). The pin goes when the owner removes the applet with its
data. A lost or rotated key is `QUESTIONS.md` Q18.

**The store's changes**, for pocket-nexus/applets (`store/`), not code:

1. `POST /v1/publish` takes an optional service package (`MANIFEST.md` §3) in
   the 16 MiB of a publish; the Worker refuses one that breaks `MANIFEST.md`
   §1 and §2 (the Public Suffix List rule included), whose `id` is not the
   package's, or whose applet does not declare `service.channel`.
2. R2 keeps it by SHA-256; `GET /v1/services/<sha256>` serves it, listed only.
3. Entries gain `service` and `signature2` in every route that lists them.
4. The publisher's tools (`store/admin.ts`, `pocket-applet publish` on Omarchy
   Pocket) sign the second message; the Worker verifies both.
5. No line break in any field of either message.
6. A version whose service requires `native`, or that carries a setup guide
   (`SETUP.md` §4), from a non-official key, waits
   for an editor who reads its code: `store/admin.ts` fetches and unpacks a
   service package; the pending list marks native versions.
7. The app's page shows the capabilities in words, both host lists, the
   native mark, and that the applet comes with setup instructions for an
   agent, from whom.
8. `services` (companion services) is dropped once `applet.services` leaves
   the applet contract.
9. `backfill` sets `service: null` on existing versions.
10. The source check, when the Worker can run it; else the editor runs it.

## 8. Local development installs

`services.install {path, consent?}` installs a folder or an archive as the
id's **development** release: unsigned, labelled "Development", serving every
package hash of its id while installed, taking precedence over the id's store
releases, and **with a store of its own** (`<home>/service-store/<id>@dev/`),
so a development install under X's id cannot read X's session. Installing one
is proposed for identified clients (§2 (d)). The source check runs for it as for any
portable service.

## 9. What is covered, and what is not

| Threat | Covered by |
| --- | --- |
| A store applet reaching hosts it did not declare | the host's checks (R3); the signed host lists |
| A portable service reading files, starting programs, opening sockets, calling the control API | the enclosure, from step 2 (§5), and the peer check (§2) |
| Another local program, the command line, a remote client or a link consenting to native code or granting an origin | consent-class methods taken from the app alone (§2 (c)) |
| A setup guide from an untrusted publisher | it runs only when the owner starts an agent on it; a non-official guide is reviewed by the store's editor and needs the owner's click at install (`SETUP.md` §4, `QUESTIONS.md` Q3). An agent that follows a guide acts with the owner's rights |
| A local program handing a service a session of its choosing | not covered: `services.setup` is open to the command line (`QUESTIONS.md` Q9) |
| A development install under another applet's id reading its session | its own store (§8) |
| The store changing an applet's code, hosts or capabilities | the publisher's signature (§6) |
| The store handing an id to another key | the host's pin on a host that installed it before (§7) |
| **An active attacker on the LAN without the key** | **not covered in phase one, by decision.** After a real handshake it can open applet channels asserting any applet id (link §3.9): it can drive X's, Discord's or Home Assistant's service **with the owner's stored session** (read the timeline, post, send messages, call the house's services), make a service ask for a sign-in, and fetch within any applet's hosts through the system channel. Link encryption (§3.10) is the remedy, later. Omarchy Pocket's local connection does not cross the LAN |
| A device that holds the link key and lies about `app` or `hash` | not covered: it holds the pairing (link §3.9) |
| A same-user program that is not a service | not covered beyond §2 (b) to (d): it can read the stores and call unprivileged methods (`QUESTIONS.md` Q24) |
| A native service | not covered: disclosed (§4) |
| A malformed picture or stream from the network | decoded in the host's own process (the owner's decision: decoding is performance-first, `CAPABILITIES.md` §1); memory-safe decoders where they cost nothing (`DEVICES.md` §3) |
