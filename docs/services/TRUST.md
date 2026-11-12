# Sandbox, identity and signing

Proposal (see `README.md`). Two questions: what a service can do on the
owner's computer (§1 to §5), and which code Hub agrees to run for which applet
(§6 to §10).

## 1. The rules

| Rule | Portable service | Native service |
| --- | --- | --- |
| R1. Its only exit is Hub's service socket: the control, channel and host connections | yes | no |
| R2. It reads its own package, Hub's copy of the SDK and Bun's installation; it writes nothing | yes | no |
| R3. Every capability call is checked by Hub against the release's manifest (`requires`, `hosts`, `store.maxBytes`) and the destination policy | yes | for the SDK's calls only |
| R4. It starts no program and gets no environment beyond `HOST-PROTOCOL.md` §2 | yes | no |
| R5. It gets no device operation: Hub's control socket is out of its reach | yes, from step two | no, and the owner is told (§4) |
| R6. Hub logs each capability call without secrets (`HOST-PROTOCOL.md` §7) | yes | for the SDK's calls |
| R7. A process Hub starts inherits none of Hub's descriptors (today's rule) | yes | yes |

**The SDK is code inside the process**, so nothing the SDK refuses is
refused for good: Hub repeats every check (R3). The SDK checks first only so
that a mistake fails at once with a clear code.

**Two steps, as the direction says.** Step one fixes the SDK's interface and
adds a checking tool that refuses a service importing anything but the SDK
(§3); it makes intent visible and catches mistakes, and **it is not a
boundary**. Step two encloses the process with the operating system (§5),
which is the boundary.

## 2. What a portable service can reach

| Resource | How | Checked by |
| --- | --- | --- |
| the network | `host.net` | Hub: `hosts`, the destination policy |
| its storage | `host.store` | Hub: the quota; the store is in Hub's home, which the service cannot read |
| the owner | `host.signin` | Hub: `hosts`; the owner sees the window or form |
| its applet | the channel | Hub relays; the device's bounds (L§7.6) |
| a device's formats | `channel.image`, `.text`, `.media` | Hub: the channel is the service's own |
| files, programs, other sockets, other services, Hub's control API | nothing | the OS profile (§5) |

## 3. Step one: the checking tool

A command shipped with the SDK, `pocket-service check <package>`, the same
check as a library, run in three places: by the publisher (pocket-nexus/applets
`bun run check`), by the store when it can (§9), and **by Hub at the install
of every portable service**, which refuses one that fails (control API error
`service`, `reason: "check"`, with the first lines).

**What it inspects**: every module reachable from the manifest's `entry`,
after Bun's transpiler has removed types and comments (the method of PocketJS
`tools/applet-check.ts`, so a word in a comment does not count).

| Rule | Reason |
| --- | --- |
| Imports are relative and stay inside the package, or are `@pocketjs/service` | a store release is bundled (`MANIFEST.md` §3), so any other import is a mistake or a way out |
| No `bun:*`, `node:*` or bare Node built-in (`fs`, `child_process`, `net`, …) | those are the ways to files, programs and sockets |
| No `require`, no dynamic `import()`, no `import.meta` beyond `import.meta.url` | the module graph that was checked is the one that runs |
| No `eval`, no `Function` constructor | idem |
| No free reference to `Bun`, `process`, `fetch`, `WebSocket`, `XMLHttpRequest`, `EventSource`, `Worker`, `navigator`, `Deno`, and no computed member of `globalThis`, `self` or `window` | Bun's globals reach the network and programs without an import |

**Allowed globals**: ECMAScript's built-ins, `TextEncoder`, `TextDecoder`,
`URL`, `URLSearchParams`, `setTimeout`, `clearTimeout`, `setInterval`,
`clearInterval`, `queueMicrotask`, `structuredClone`, `atob`, `btoa`,
`console` (its lines go to the service's log), `performance.now`,
`AbortController`, `AbortSignal`, `crypto.getRandomValues`, `crypto.subtle`,
`crypto.randomUUID`. This list is also **what every host must provide**, so it
is part of the portability contract.

**How it fails**: one line per violation, `<file>:<line>:<column>
<rule>: <what>`, exit status 1.

**What it does not catch**: anything that reaches a global without naming it,
`[].constructor.constructor("return Bun")()` among them. A service that wants
to pass it can. That is why step two exists, and why the tool's verdict is
never called a sandbox.

## 4. Native services

A release whose manifest requires `native` runs **with the owner's rights and
outside the profile of §5**: it can read the owner's files, start programs,
use the network directly, and connect to Hub's control socket and call every
method there (`devices.installProgram` included). Hub still checks the SDK's
calls, which binds nothing the service does by its own means. **Hub says so
before installing it, and installs nothing without the owner's yes.**

| Where | What the owner sees |
| --- | --- |
| Pocket Hub's store page | a mark on the applet: "Runs its own programs on your Mac" |
| Installing, in Pocket Hub | a dialog, title "**YouTube runs its own programs on this Mac**", text "Other applets can do only what Pocket Hub allows. YouTube runs programs of its own, with the same access to your files, accounts and network that you have, and Pocket Hub cannot limit or watch what they do. Install it only if you trust Pocket Nexus, who published it." Buttons "Install" and "Cancel" |
| Installing from the console's store page | the applet installs; it does not start its programs until the owner allows it in Pocket Hub on the Mac, and says so on the console: "Allow YouTube in Pocket Hub on your Mac" (`QUESTIONS.md` Q16) |
| A host that does not provide `native` | the applet says it needs a computer (`channel.open` error `capability`, L§7.1 as proposed) |

The publisher's name in the dialog is the store entry's `author`, which the
signature covers (§6). **A native release published with a key other than the
official one requires an editor's review that reads the service's code**
(§9). The store already holds every non-official version for an editor; what
is new is that a service's code, unlike a `.pocket`, can be read, so the
review reads it.

## 5. Step two: the OS sandbox on macOS

Hub starts a portable service as `sandbox-exec -f <profile> -D … bun …`,
with this profile:

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
| `Intl.DateTimeFormat` with a time zone | worked **only after `/usr/share/icu` was allowed** (Bun reads `icudt*.dat` there; without it: "failed to initialize DateTimeFormat") |
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

**What this does not cover yet**: a real service with the SDK's connections
and streams under load, Bun 1.4.4, an older macOS, and Bun's behaviour
when a folder it lists is denied in a case not met here. These are measured
before step two lands.

**The control socket.** The profile names one socket path, Hub's service
socket. `<home>/hubd.sock` is not it, so from step two **a portable service
cannot connect to the control API**, which today any daemon can, since it
runs as the owner's user (the owner chose not to patch this separately). Until
step two lands, the exposure stays; a native service keeps it for good (§4).

**The deprecated interface.** `sandbox-exec` and its profile language are
marked deprecated by Apple and undocumented, and remain in current macOS
(used above on 27.2); build tools such as Bazel and Nix use them (from their
documentation, not checked here). A macOS release could remove or
change them. Alternatives, from nearest to farthest:

| Option | What it takes | Risk |
| --- | --- | --- |
| `sandbox_init_with_parameters` from a small launcher Hub ships | the same engine and profile, called as a function | the same deprecation |
| App Sandbox | a copy of Bun signed by Hub with the sandbox entitlement; the service socket inside an app group container | not tried; App Sandbox limits Unix sockets to the container, and Hub would re-sign a third-party binary |
| **An embedded JavaScript engine with no host interface** (V8 through `deno_core`, or JavaScriptCore) running portable services in a Hub child process | the engine exposes only the SDK's operations, so the sandbox is the absence of interfaces; it works the same on Linux and on a device that hosts services | effort; no npm compatibility beyond what a bundle carries |
| Linux: Landlock and seccomp, or bubblewrap | for a Hub on Linux | none on macOS |

*Recommendation*: ship step two with `sandbox-exec` (measured to work), keep
the profile this small, run the measurement above in CI on each macOS, and
plan the embedded engine as its replacement (`QUESTIONS.md` Q18).

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
`dev.pocket-nexus.` accepted from the official key only. **The applet's
`services` and `hosts` members are not covered**: the store could change
what an applet declares without the publisher's key.

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
```

- **The service package's SHA-256 binds the code to the applet**; the
  manifest is inside that package, so its `id`, `entry` and every field are
  covered.
- **`requires` and both host lists are signed as well** so that the store
  page, Pocket Hub's install dialog and the console show what an applet will
  reach, and whether it is native, **before anything is downloaded**, from
  members the publisher's key covers. Hub checks after the download that the
  manifest says the same (`MANIFEST.md` §6).
- **The first signature stays.** The store's catalog also serves Omarchy
  Pocket's devices, which check `omarchy-pocket-store/1` at install; a
  release keeps `signature` over the first message so they go on working,
  and adds `signature2`. A release without a service carries `signature`
  alone, as today.
- **No field may hold a line break**; the store refuses one at publish. The
  first message has the same weakness if the store does not already refuse
  one (not checked).

The entry gains, beside `signature2`: `service: {package, bytes, requires,
hosts} | null`. The release's version is the entry's `version`
(`MANIFEST.md` §1).

**Hub checks at install** of a release with a service, before anything runs:

1. `signature` and `signature2` verify with the entry's `key` (`verify_strict`);
   an entry with `service` and no `signature2` is refused;
2. the reserved prefix: `dev.pocket-nexus.` from the official key only (as today);
3. the key is the one Hub pinned for the id, or the first it sees (§7);
4. the applet package matches its SHA-256 and is what the entry says (as today);
5. the service package matches its SHA-256; its manifest's `id`, `requires`
   and `hosts` equal the entry's signed members; the applet declares
   `service.channel`;
6. for `native`: the owner's consent (§4); for a portable service: the
   checking tool (§3);
7. Hub records the release under the applet package's footer hash
   (`HOST-PROTOCOL.md` §8).

**At `channel.open {app, hash}`** Hub starts the service of the release whose
applet package has that footer hash, and no other. **The applet and its
service are always of one release**, so neither declares which versions of
the other it works with. A device running a package Hub does not know (pushed
from elsewhere, or a different build under the same id) gets `no_service`,
unless a development install exists (§8). An applet built by someone else
under X's id, on an honest runtime, therefore never reaches X's service or
its sign-in.

## 7. Key ownership

The store's rule stays: the first key to publish an id owns it, and a later
version must come from the same key. Hub cannot read the store's record; it
knows only the official key. **Hub pins the key of the first signed release
it installs for an id** (`<home>/services/<id>/owner.json`) and refuses a
release of that id from another key (control API error `store`, `reason:
"key"`), so a store that changed an id's owner could not change what runs on
a Mac that already trusts the first. The pin goes when the owner removes the
applet's service with its data. `QUESTIONS.md` Q14.

## 8. Local development installs

`services.install {path}` (today `daemons.install`) installs a folder or an
archive as the id's `dev` release: **unsigned, labelled "Development" in
Pocket Hub, and serving every package hash of its id** while it is installed,
because a developer pushes a new build at every change. It takes precedence
over the id's store releases until it is removed. The checking tool runs for
it as for any portable service; a local install that requires `native` gets
the dialog of §4 with "This applet is not from the store" in place of the
publisher. `QUESTIONS.md` Q13.

## 9. What the store must add

A list of changes for pocket-nexus/applets (`store/`, its Worker and site), not
code:

1. **`POST /v1/publish` takes an optional service package** beside the
   `.pocket`: a `.tar.gz` as `MANIFEST.md` §3, in the 16 MiB of the publish.
   The Worker reads its `pocket.service.json` and refuses one that breaks
   `MANIFEST.md` §1 and §2, whose `id` is not the package's, or whose applet
   does not declare `service.channel`.
2. **R2 keeps it by SHA-256**, and `GET /v1/services/<sha256>` serves it, with
   the same listed-only rule as packages.
3. **Entries gain `service: {package, bytes, requires, hosts} | null` and
   `signature2`** (§6), in `/v1/apps`, `/v1/apps/<id>`, `/v1/apps/<id>/versions`
   and `/v1/catalog`.
4. **The publisher's tool signs the second message**; the Worker verifies both
   signatures at publish.
5. **No field may contain a line break**, for both messages.
6. **A version whose service requires `native`, from a key other than the
   official one, waits for an editor who reads the service's code**:
   `store/admin.ts` gains a command to fetch and unpack a service package, and
   the pending list marks native versions.
7. **The app's page shows** the service's capabilities in words, both host
   lists, and the native mark of §4.
8. **The `services` member** (companion services) is dropped once
   `applet.services` leaves the applet contract; until then it is ignored.
9. The admin `backfill` fills the new members for existing versions
   (`service: null`).
10. The checking tool of §3, when the Worker can run it (it needs a
    JavaScript parser, not Bun); otherwise the editor runs it for non-official
    versions.

## 10. What this protects, and what it does not

| Threat | Covered by |
| --- | --- |
| A store applet reaching hosts it did not declare | Hub's checks (R3), signed host lists (§6) |
| A portable service reading the owner's files, starting programs, calling Hub's control API | step two (§5); before it, nothing but the checking tool |
| Another applet, on an honest runtime, using X's service and sign-in | the release binding at `channel.open` (§6) |
| The store changing an applet's code, hosts or capabilities | the publisher's signature (§6); Hub's key pin (§7) |
| The store handing an id to another key | Hub's key pin on a Mac that installed it before (§7) |
| **A device that holds the link key and lies about `app` or `hash`** | **not covered**: L§3.9; such a device already holds the pairing |
| **A native service** | **not covered**: disclosed (§4) |
| **A local development install** | **not covered**: the owner's own code (§8) |
