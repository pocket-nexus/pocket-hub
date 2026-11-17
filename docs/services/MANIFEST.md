# The service manifest

Proposal (see `README.md`). `pocket.service.json` at the root of a service
package says which applet the service belongs to, what it runs, and **which
capabilities and hosts it may use**: the line its host draws around it, on every host (Hub, or the host on
Omarchy Pocket).

```json
{
  "service": 1,
  "id": "dev.pocket-nexus.x",
  "entry": "service.js",
  "requires": ["image", "net", "store"],
  "hosts": ["abs.twimg.com", "api.x.com", "pbs.twimg.com", "x.com"],
  "store": { "maxBytes": 16777216 },
  "setup": "setup/SKILL.md"
}
```

## 1. Fields

| Field | Type | Content | Check |
| --- | --- | --- | --- |
| `service` | integer | the manifest version, **1** | any other value: refused, with the version named, before any other field is read (as `pocket.daemon.json`'s `daemon` today) |
| `id` | string | the applet's manifest id | 1 to 128 bytes of `[A-Za-z0-9._-]`, not `.` or `..` (the folder rule of today); **equal to the `id` of the applet package of the same release** (`TRUST.md` §6) |
| `entry` | string | the module the host loads; its default export is the service (`CAPABILITIES.md` §2) | a relative path of normal components inside the package, at most 1,024 bytes, naming a regular file reached through no link (today's rule) |
| `requires` | array of strings | the capabilities the service uses | each of `net`, `store`, `image`, `text`, `media`, `native` (six; `signin` is not one, `SETUP.md`); no repeats; order carries no meaning (the host sorts it where it signs or shows it). **A name the host does not know, or does not provide, is refused**, so a service written for a later capability is not run without it; `text` is refused until hosts implement it for services (`CAPABILITIES.md` §8). May be empty: a service that only computes for its applet |
| `hosts` | array of strings | the destinations the service's `net` and the URL sources of its `image` and `media` may reach | at most **32**; each a lower-case DNS name as `applet.hosts` writes them (`^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$`, at most 253 bytes), or such a name behind `*.` (§2); no repeats; no address, no port, no scheme. **May be empty**: a service with `net` and no hosts reaches only the origins its setup passed (`SETUP.md` §2; Home Assistant) |
| `store` | object? | `{ "maxBytes": <integer> }`: the store's size | only with `store` in `requires`; 1 to 268,435,456 (256 MiB); default 16 MiB. The id's releases share one store, whose limit is the newest installed release's (`QUESTIONS.md` Q16) |
| `idleSeconds` | integer? | how long a process stays up with no channel open | 1 to 86,400; default **30** (unchanged) |
| `setup` | string? | the setup guide's entry, `setup/SKILL.md` (`SETUP.md` §1) | a relative path inside the package to a regular file named `SKILL.md`; its folder holds at most 64 files and 1 MiB; the host never runs it |

**A manifest with a field this table does not name is refused**, and so is
one over **16 KiB** (both as today): a field the host does not know may change what
the package means.

**No `version` field.** A service is part of a release, and the release's
version is the applet's (`pocket.app.json` `version`, signed by the store,
`TRUST.md` §6). With a version of its own, the two could disagree; without
one, they cannot.

## 2. Host entries

| Entry | Matches | Example |
| --- | --- | --- |
| `name.tld` | that name exactly | `x.com` matches `x.com`, not `api.x.com` |
| `*.name.tld` | any name that ends in `.name.tld`, with at least one label before it; not `name.tld` itself. **Refused when `name.tld` is a public suffix** of the Public Suffix List, its private section included: `*.co.uk`, `*.github.io` and `*.workers.dev` would admit every customer of a registry or a hosting platform | `*.googlevideo.com` matches `rr3---sn-q4f.googlevideo.com` |

A URL's host is compared after lower-casing and IDNA conversion; an address
written as a host (`http://192.0.2.1/`) matches no entry. Any port is allowed,
as `net.http` allows it today. **The destination policy applies after the
match**: a name that resolves to a private address is refused whatever the
manifest says. A destination the manifest cannot express (a Home Assistant
instance at `http://192.168.1.20:8123`: an address literal, a port, a private
address) is reachable only as an **origin the setup's agent passed** with its
result, which the host pins and keeps outside the manifest and the service's
store (`SETUP.md` §2).

The wildcard is this proposal's addition (`QUESTIONS.md` Q12): YouTube's media
addresses are on per-server names under `googlevideo.com`, and without it a
service would list names it cannot know. A further form, any public host
(for Podcasts, whose feeds and enclosures are anywhere), is not in the table;
`QUESTIONS.md` Q12 sets out the choice. Hosts and the store carry a dated
copy of the Public Suffix List.

## 3. The package

| Kind | Content | Limit |
| --- | --- | --- |
| **A store release** | a `.tar.gz` holding, at its root or in one top-level folder, `pocket.service.json` and the module files the entry imports; **no `package.json`, lockfile or `node_modules`**: the publisher bundles dependencies into the entry (`bun build --target=browser --format=esm --external @pocketjs/service`) | at most **16 MiB** packed (the store's publish limit) and 512 MiB, 100,000 entries unpacked (today's limits) |
| **A local development install** | a folder or a `.tar.gz` as today, and `package.json` with a lockfile allowed; the host runs `bun install --frozen-lockfile --ignore-scripts` for it | today's limits |

A store release is bundled because **the host would otherwise run a dependency's
install scripts outside any sandbox at install time**, and because the
source check (`TRUST.md` §3) then reads one module graph that the signature
covers whole (`QUESTIONS.md` Q19). The bundler is not a check: tried with Bun
1.4.2, `--target=browser` replaced `node:fs` with an empty object and bundled a
polyfill of `node:path` without a warning, so a forbidden import is caught by
the source check, not by the build. A native service builds with
`--target=bun`.

**The host provides the SDK.** `@pocketjs/service` is never part of a package: the host
links its own copy into the package's `node_modules` before each start, so
the SDK a service runs speaks the protocol of the host that runs it. Today a
package that brings its own copy keeps it; under this proposal the host's copy
always wins, because a bundled release cannot carry one and a stale copy
would speak a protocol the host refuses.

## 4. How it relates to the applet's own contract

| Applet contract (PocketJS `contracts/spec/applet.ts`, `pocket.app.json`) | Relation |
| --- | --- |
| `id` | must equal the manifest's `id`; checked at install of a release |
| `version` | the release's version; the service has none of its own |
| capability `service.channel` (today `link.channel`) | **the applet declares it exactly when its release has a service.** A host refuses a release with a service whose applet does not declare it (the service would never be reached) and the store refuses it at publish. An applet that declares it without a service gets `no_service` from its host, as today's `no_daemon` |
| `applet.services` (companion services at `https://companion.pocket/<name>`) | **removed from the applet contract** (link specification §7.10 as proposed): the origin is gone, and an applet's computer side is its service, named by its id. The store's `services` member and the control API's `StoreApp.services` go with it |
| `applet.hosts` | the applet's own destinations: its worker's `net.http`, and the URLs it hands the framework for `image.url` and `media.url`. **Separate from the service's `hosts`**; an applet with a service often needs none of its own. The owner sees both at install, in one list |
| capability `net.http` | the applet's own HTTP; independent of the service's `net` |
| capabilities `image.url`, `text.glyphs.deferred`, `media.url` | the framework's use of the system service; independent of the service's `image`, `text`, `media` |

## 5. Compared with today's `pocket.daemon.json`

| `pocket.daemon.json` | `pocket.service.json` |
| --- | --- |
| `"daemon": 1` | `"service": 1` |
| `id` | `id`, now also checked against the applet package of the same release |
| `version` (SemVer, names a folder) | removed; the release's version is the applet's, and the host names the folder by the release (`HOST-PROTOCOL.md` §8) |
| `entry` (the program Bun runs; calls `serve`) | `entry` (the module the host loads; its default export is the service) |
| `idleSeconds` | unchanged |
| | `requires`, `hosts`, `store`: new |

## 6. What the host does with it

| When | Check | Refusal (control API, `MIGRATION.md` §1) |
| --- | --- | --- |
| install, every kind | §1 and §2 | `service` with `reason: "manifest"` and the field |
| install of a store release | `id`, `requires` and `hosts` equal what the release's signature covers, and the signed applet-hosts line equals the package's `applet.hosts` (`TRUST.md` §6); the applet declares `service.channel` | `store` with `reason: "rejected"` |
| install, every kind | a wildcard entry's suffix is not a public suffix (§2); every name in `requires` is one this host provides | `service` with `reason: "manifest"` |
| install of a release that requires `native` | the owner agreed (`TRUST.md` §4) | `consent` |
| install of a portable service | the source check passes (`TRUST.md` §3) | `service` with `reason: "check"` and its first lines |
| each start | the process gets `requires` in `host.ready` (`HOST-PROTOCOL.md` §4), so the SDK refuses locally what the host would refuse | |
| each capability call | `requires`, `hosts`, `store.maxBytes` (`CAPABILITIES.md`) | the call's error |
