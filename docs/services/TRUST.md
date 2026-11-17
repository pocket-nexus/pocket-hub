# Sandbox, identity and signing

Proposal (see `README.md`). What a service can do on the machine that hosts
it (§1 to §5), which code a service host agrees to run for which applet (§6
to §8), and what is covered (§9). "The host" is the service host: Hub on a
Mac, `pocket-service-hostd` on Omarchy Pocket (`DEVICES.md`). Each statement
about the enclosure says whether it is **measured** (with the versions) or
**on paper**.

## 1. The rules

| Rule | Portable service | Native service |
| --- | --- | --- |
| R1. Its only exit is the host's service socket | yes | no |
| R2. It reads its own package, the host's copy of the SDK and Bun's installation, and the metadata of their folders' ancestors; it writes nothing | yes | no |
| R3. Every capability call is checked by the host against the release's manifest and the destination policy | yes | for the SDK's calls only |
| R4. It starts no program and gets no environment beyond `HOST-PROTOCOL.md` §2 | yes | no |
| R5. It gets no device operation: the host's control socket is out of its reach | yes, by R1 | no, and the owner is told (§4) |
| R6. The host logs each capability call without secrets | yes | for the SDK's calls |
| R7. A process the host starts inherits none of its descriptors (today's rule) | yes | yes |

**The SDK is code inside the process**, so the host repeats every check (R3).
**Two mechanisms, one boundary**: the *source check* (§3) makes intent
visible and is not a boundary; the *enclosure* (§5) is the boundary, **on
from step 2**, the first step in which `store` exists.

## 2. The control socket, and consent

Today a daemon runs as the owner's user and can call every control method.
This work closes that:

| Layer | Mechanism | What it stops | Status |
| --- | --- | --- | --- |
| **(a) the enclosure** (§5) | a portable service connects to the service socket alone | every portable service reaching `hubd.sock`, files or the network | macOS: **measured** (§5). Linux: **on paper**: seccomp cannot filter `connect()` by path and Landlock before its Unix-socket scoping does not limit pathname sockets, so "sees no other socket" holds only with a mount and network namespace (bubblewrap), unmeasured |
| **(b) consent from the app's own click** | below | a helper acting for the owner consenting on the owner's behalf | on paper |
| **(c) a development release's own store** (§8) | `<id>@dev` | a development install under X's id reading X's session | on paper |

**There is no check of whether a control client descends from a service.** A
portable service cannot reach the control socket at all (a), and an
unrestricted one escapes such a check by forking twice, so it would stop
nothing.

**Consent** (decided on 2026-11-17: the owner's own click in Pocket Hub's
app) is the one consent-class method family: `services.consent`, and
`consent` on an install. How the core knows the caller is the app:

- **The core pins the code-directory hash of the app's executable in its own
  bundle** (`pocket-hubd` runs from `Contents/Helpers`; the app's executable
  is `Contents/MacOS/Pocket Hub`), computed when the core starts, and **checks
  the caller by its audit token** (`LOCAL_PEERTOKEN` on the accepted socket,
  then the code-signing API on that token), never by a process id, which a
  caller can race. A team or signing identifier would prove nothing: the app
  is signed ad hoc today (`CODE_SIGN_IDENTITY = "-"` in
  `app/PocketHub.xcodeproj`, `app/scripts/package.sh`).
- **The hardened runtime is required from the step that brings consent**
  (`MIGRATION.md` step 5), so a debugger or an injected library cannot drive
  the app's process without the owner's own system prompt.
- **The app's debug command pipe never reaches a consent dialog's answer**;
  only a click in the dialog sends `consent`.
- **A core that is not in a bundle** (run from a build folder, or by the
  command line alone) has no app to pin: **no client can consent**, and the
  call answers `denied`. Tests reach the path through a constructor of the
  core's library that takes the expected hash, which `pocket-hubd`'s
  `main` never calls; no flag, variable or file of the shipped binary sets
  it.
- **On Omarchy Pocket** every applet's runtime runs inside the shell's own
  process (`host/src/children.rs` in omarchy-pocket), so the process cannot
  tell the launcher from an applet. The runtime tells them apart: the
  consenting call is accepted by omarchy-pocket-host only from a realm whose
  plan holds a system capability only the launcher may declare
  (`system.services`, as `system.apps` would be, `QUESTIONS.md` Q1), and
  omarchy-pocket-host is the only client of `pocket-service-hostd`'s control
  socket that sends it (`DEVICES.md` §4).

**What this protects, said honestly**: it keeps consent with the owner's own
click as against a helper acting for the owner: a coding agent, the command
line, a remote client, a `pocket-hub:` link. **It does not stop a hostile
program already running as the owner**: such a program needs no Hub to run
code, and can read every store (files of the owner's user) and call every
other control method.

## 3. The source check

`pocket-service check <package>`, run by the publisher, by the store when it
can, and **by the host at the install of every portable service**. It
inspects every module reachable from `entry` after Bun's transpiler removed
types and comments.

| Rule | Reason |
| --- | --- |
| Imports relative and inside the package, or `@pocketjs/service` | a store release is bundled |
| No `bun:*`, `node:*` or bare Node built-in | the ways to files, programs and sockets |
| No `require`, dynamic `import()`, `import.meta` beyond `import.meta.url` | the checked graph is the one that runs |
| No `eval`, no `Function` constructor | idem |
| No free `Bun`, `process`, `fetch`, `WebSocket`, `XMLHttpRequest`, `EventSource`, `Worker`, `navigator`, `Deno`; no computed member of `globalThis`, `self`, `window` | Bun's globals reach the network and programs without an import |

Allowed globals: ECMAScript's built-ins, `TextEncoder`, `TextDecoder`, `URL`,
`URLSearchParams`, the timers, `queueMicrotask`, `structuredClone`, `atob`,
`btoa`, `console`, `performance.now`, `AbortController`, `AbortSignal`,
`crypto.getRandomValues`, `crypto.subtle`, `crypto.randomUUID`: also what
every host must provide. **It does not catch** a global reached without its
name; the measurement of §5 reached `node:fs` and `bun:ffi` through
`Function("return import('…')")`, which the check refuses only when it is
written that plainly.

## 4. Unrestricted services

A release whose manifest requires `native` runs with the owner's rights,
outside the enclosure, on a host that provides `native` (Hub; the host on
Omarchy Pocket).

**One rule: no path starts a release whose consent is missing.**

| Path | With consent missing |
| --- | --- |
| `channel.open` from a device | refused with `consent` (link §7.1 as proposed) |
| `services.start`, `services.restart` | the error `consent` |
| `services.setup` (which starts a release to deliver its message) | the error `consent`; nothing is delivered |
| `store.install` and `services.install` without `consent` (the command line, a remote client, a console's own store page) | **installed and held**: the files are in place and the release waits; nothing runs until the owner allows it in the app, which then shows it as waiting |
| any install with `consent: "unrestricted"` | taken from the app alone (§2) |

**What consent binds** is the owner's to decide (`QUESTIONS.md` Q3): (a)
**one release**, so every update asks again; (b) **the applet's id and its
publisher's key**, asked again when the key changes or when an update newly
requires `native`. *Recommendation*: (b). The dialog asks the owner to trust
a publisher, and a click on every release tells the owner nothing they can
judge.

**The dialog.** Title "**YouTube runs its own programs on this Mac**" ("on
this device" on Omarchy Pocket), text "Other applets can do only what Pocket
Hub allows. YouTube runs programs of its own, with the same access to your
files, accounts and network that you have, and Pocket Hub cannot limit or
watch what they do. Install it only if you trust {publisher}." Buttons
"Cancel" and "Install"; **Install is not the default button**, for the reason
`docs/DEEP-LINKS.md` §2 gives for the Store page: a window can come up while
the owner is typing elsewhere.

**Who the owner is told to trust**: `{publisher}` is "Pocket Nexus" only when
the entry's key is the official key this host knows. Otherwise it is the
entry's `author` handle, written as the publisher's own claim ("signed by the
publisher who calls themselves pocketfan"), with the key's fingerprint under
it. **The applet's title is not signed** (the signed message covers id,
version, hashes, author and summary, §6), so the dialog shows the applet's
id under its title.

A native release from a non-official key waits for the store's editor, as
every non-official version already does (§7); what is new is that a
service's code can be read in that review.

## 5. The enclosure

**macOS, measured on 2026-11-17** with Bun **1.4.4** (the version Hub pins)
and Bun 1.4.2, on macOS 27.2: a real SDK service (the SDK's `serve`, `attach`,
a request handler) run by `sandbox-exec` against the SDK's mock Hub, which
offered it a channel and sent it requests. The service tried what a hostile
service would, reaching `node:fs` and `bun:ffi` through
`Function("return import('…')")`. Every path handed to the profile is
canonical (`/private/tmp`, not `/tmp`), and the service's socket path in its
environment is the canonical one.

| Tried | Profile of revision 3 (global metadata, all sysctls) | This profile (below) |
| --- | --- | --- |
| the SDK's hello, a channel, a request and its reply | worked | worked |
| `stat` of the home folder, of `/private/etc/hosts` | **allowed** | denied |
| read `~/.zshrc`, list the home folder | denied | denied |
| load `libSystem` through `bun:ffi` and call it | **allowed** | **allowed** (accepted: every system call it makes still meets the profile) |
| `sysctl` `KERN_PROC_ALL` (every process on the machine) | **allowed** (650 KB) | denied |
| `sysctl` `KERN_PROCARGS2` of the parent process | **allowed**: its command line, no environment | **allowed**: the parent is the host, whose command line holds no secret; denied for pid 1 and for another of the owner's processes |
| `KERN_PROCARGS2` of another process | not tried | denied |
| TCP, start a program | denied | denied |
| `Intl` with a time zone, `crypto.subtle` | worked | worked |
| the working directory reached through a symbolic link (`/tmp/…`), with the profile's paths canonical | worked | worked |

```
(version 1)
(deny default)
(allow process-exec (literal (param "BUN")))
(allow file-read* (literal (param "BUN")))
(allow file-read* (subpath (param "PACKAGE")) (subpath (param "SDK")))
(allow file-read-metadata (literal "/") <each ancestor of BUN, PACKAGE, SDK and SOCKET, as literals>)
(allow file-read-data (literal "/"))
(allow file-read* (subpath "/usr/lib") (subpath "/System/Library")
  (subpath "/usr/share/zoneinfo") (subpath "/usr/share/icu")
  (subpath "/private/var/db/timezone") (literal "/private/etc/localtime")
  (literal "/dev/urandom") (literal "/dev/random") (literal "/dev/null"))
(allow sysctl-read (sysctl-name-prefix "hw.") (sysctl-name-prefix "machdep.")
  (sysctl-name-prefix "kern.os") (sysctl-name "kern.version") (sysctl-name "kern.boottime")
  (sysctl-name "kern.argmax") (sysctl-name "kern.hostname") (sysctl-name "kern.maxfilesperproc")
  (sysctl-name "kern.usrstack64") (sysctl-name "sysctl.proc_translated")
  (sysctl-name "kern.secure_kernel") (sysctl-name "vm.footprint_suspend"))
(deny process-info* (target others))
(allow network-outbound (remote unix-socket (path-literal (param "SOCKET"))))
```

**Accepted leaks, with what they expose**: `bun:ffi` and native calls inside
the profile (nothing the profile denies); the parent's command line; the
machine's name and its hardware and OS version (`hw.*`, `kern.os*`,
`kern.hostname`); the metadata of the listed ancestors (that the folders
exist). **Measured as failing**: `(deny process-info*)` without a target
makes Bun 1.4.4 crash at start. The sysctl list is what Bun ran with, not a
minimum; it was not reduced further. Not measured: Bun's first run with no
transpiler cache on a slow disk, an older macOS.

`sandbox-exec` is marked deprecated and remains in macOS 27.2. Alternatives:
the same engine from a launcher (same deprecation); App Sandbox (not tried);
an embedded JavaScript engine with no host interface, which would also fit a
weak device that hosts services (`QUESTIONS.md` Q22).

**Linux (Omarchy Pocket), on paper**: bubblewrap (an empty network namespace,
a mount namespace with read-only binds of Bun, the package, the SDK and the
system libraries, the service socket bound in) where unprivileged user
namespaces are allowed; elsewhere Landlock and seccomp, which leave
pathname sockets reachable and so do not meet R1 alone. **On a device with no
measured enclosure** (decided): only services published with the official
key run, unenclosed; any other, portable or native, does not run there
(`channel.open` refusal `enclosure`, `DEVICES.md` §6).

## 6. The signed release

**Today**, Hub checks each catalog entry with the publisher's ed25519 key
(`crates/hub-applet-store/src/entry.rs`):

```rust
pub const SIGNED_PREFIX: &str = "omarchy-pocket-store/1";

pub fn signed_message(&self) -> String {
    [SIGNED_PREFIX, &self.id, &self.version, &self.package, &self.icon, &self.author, &self.summary].join("\n")
}
```

with `verify_strict`, and ids under `dev.pocket-nexus.` from the official key
only. The package's SHA-256 covers the applet's manifest, `applet.hosts`
included; the catalog's `hosts` member is what the store read from that
package (`store/src/index.ts`). **Not covered: the title**, which the store
takes from the package and lists beside the signed fields. Line breaks: the
store checks `author` as a handle and only trims and length-checks `summary`,
which is the last line of the first message, so it parses one way.

**Proposed**: a release with a service carries `signature2` over:

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
<1 if the service package carries a setup guide, else 0>
```

`summary` is not last here, so the store refuses a line break in any field.
The first signature stays for verifiers that know only it. The host checks
at install: both signatures; the reserved prefix; the pinned key (§7); both
packages' hashes; the manifest against the signed lines, the applet-hosts line
against the package; `service.channel` declared; for `native`, consent (§4);
for a portable service, the source check. **At `channel.open {app, hash}`**
it starts the release whose applet package has that footer hash. The footer
hash is FNV-1a64 and `app` and `hash` are the device's assertion: the
binding keeps an honest runtime from running another applet's service and is
no defence against a peer that lies (§9).

## 7. Key ownership, and the store

**Pins**: the host pins the key of the first signed release it installs for
every id and refuses another key (`store`, `reason: "key"`); removing the
applet with its data drops the pin; lost or rotated keys are Q18.

**The store already holds every version from a non-official key as pending
for an editor** and limits how many a key may have waiting
(`store/src/index.ts`); the changes for it (pocket-nexus/applets `store/`):

1. `POST /v1/publish` takes an optional service package; the Worker checks
   `MANIFEST.md` §1 and §2 (the Public Suffix List rule), the `id`, and
   `service.channel`.
2. R2 keeps it by SHA-256; `GET /v1/services/<sha256>`, listed only.
3. Entries gain `service` and `signature2`.
4. The publisher's tools sign the second message; the Worker verifies both.
5. No line break in any field.
6. The editor's review of a pending version now **reads the service's code
   and its setup guide** (`store/admin.ts` fetches and unpacks a service
   package; the pending list marks native versions and versions with a
   guide); **a guide carries every script it runs and fetches no
   instructions at run time**, or it is refused.
7. The app's page shows the capabilities, both host lists, the native mark,
   and that the applet comes with setup instructions for an agent, from
   whom.
8. `services` is dropped once `applet.services` leaves the applet contract.
9. `backfill` sets `service: null` on existing versions.
10. The source check, when the Worker can run it.

## 8. Development installs

`services.install {path}` installs the id's **development** release:
unsigned, labelled "Development", serving every package hash of its id while
installed, taking precedence over store releases, with **a store of its own**
(`service-store/<id>@dev/`). An unrestricted development release is installed
and held until the owner's click (§4).

## 9. What is covered, and what is not

| Threat | Covered by |
| --- | --- |
| A store applet reaching hosts it did not declare | R3; the signed host lists |
| A portable service reading files, starting programs, opening sockets, calling the control API | the enclosure (§5): measured on macOS, on paper on Linux |
| A helper acting for the owner (an agent, the command line, a remote client, a link) consenting to native code | consent from the app's own click (§2) |
| A development install under another applet's id reading its session | its own store (§8) |
| The store changing an applet's code, hosts or capabilities | the publisher's signature (§6) |
| The store handing an id to another key | the pin (§7) |
| A setup guide from an untrusted publisher | it runs only when the owner starts an agent on it; the store's editor reads it; the store page and install sheet say it is there and whose. An agent that follows it acts with the owner's rights (`SETUP.md` §4) |
| **A hostile program running as the owner** | **not covered**: it needs no Hub to run code; it can read the stores, hand a service a setup message, and call every control method but consent |
| **An active attacker on the LAN without the key** | **not covered in phase one, by decision.** It can open channels asserting any applet id (link §3.9) and drive X's, Discord's or Home Assistant's service with the owner's stored session; link encryption (§3.10) is the remedy, later. Omarchy Pocket's local connection does not cross the LAN |
| A device that holds the link key and lies about `app` or `hash` | not covered: it holds the pairing |
| A native service | not covered: disclosed (§4) |
| A malformed picture or stream from the network | decoded in the host's own process; memory-safe decoders preferred; a host-wide decode budget (`CAPABILITIES.md` §1) |
