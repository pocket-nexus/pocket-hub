# Setting up an applet's service: the guide and the inlet

Proposal (see `README.md`). **Decided on 2026-11-17: `signin` is not a
capability, the setup happens outside the service host, and the important
confirmations are asked by the agent, not by the host.** An applet whose
service needs an account, a token or an address ships **a setup guide** that a
coding agent on the service host's machine follows with the owner. The host
owes the flow two things, argued below: it carries the guide and says where it
is (§1), and it takes the agent's result, origins included, and hands it to
the service (§2). §3 is the risk.

An applet whose setup nobody has done says so: "Not signed in to X. Set it up
on your Mac" ("on this device" on Omarchy Pocket).

## 1. The guide travels with the release

| Point | Rule | Why |
| --- | --- | --- |
| Where it lives | **in the service package**, as a folder `setup/` named by the manifest's `setup` field (`MANIFEST.md` §1) | the result lands in the service's store; an applet without a service has no service host to set up; the service package is what the second signature covers |
| Its form | **`SKILL.md`** and the files it names, in that folder; at most **1 MiB** and 64 files. **It carries every script it runs and fetches no instructions at run time** (the store refuses one that does, `TRUST.md` §7) | the form coding agents read as a skill; what the editor read is what the agent follows |
| Signed | by the service package's hash, under the release's second signature, which also states whether a guide is present | an agent follows it with the owner's rights |
| Where the host puts it | unpacked with the release, read-only: on Hub `<home>/services/<id>/<release>/setup/` | |
| How an agent finds it | **`services.setupGuide {id, release?}`** → `{path, release, signed, publisher}`, and `pocket-hub service guide <id>` (`pocket-service-host service guide <id>` on Omarchy Pocket), which prints them. `publisher` is "Pocket Nexus" for the official key, otherwise the publisher's own handle (`TRUST.md` §4). **The host checks the folder against the signed hash at each call** and refuses one that changed. **A development release's guide is returned only when `release: "dev"` is asked for**, so an agent never takes an unsigned guide for the store's | an agent can show the owner whose instructions these are before it follows them |
| Who runs it | **never the host** | |

## 2. The inlet: the agent hands its result to the service

**One call delivers the result**, origins included:
`services.setup {id, message, origins?, release?}` → `{reply, origins}`, and
`pocket-hub service setup <id> <file|-> [--origin <url>…]`.

| Point | Rule | Why |
| --- | --- | --- |
| Who may call it | any client of the control socket, including the command line; never a `remote` client | the agent works through the command line; an enclosed service cannot reach the socket |
| The message | one JSON object of at most **32 KiB**; the host starts the newest installed release (**only one whose consent is not missing**, `TRUST.md` §4) and sends it a `REQUEST setup` on its host connection (`HOST-PROTOCOL.md` §6); `Service.setup(message, channels)` checks it and writes its own store; its return value is the reply | the service owns its store's format |
| A message the service does not take | it throws `ServiceError("unknown_setup")` or `("invalid_setup")`; the call fails with the error `service` and that `reason`; a service with no `setup` handler is answered `unknown_setup` | |
| How the applet learns | the handler may `emit` on the open channels it is given | the host does not know what "done" means for a service |
| A development release | `release: "dev"` delivers to it and its own store | `TRUST.md` §8 |

**Origins.** A service may need an address its manifest cannot name: a Home
Assistant at `http://192.168.1.20:8123`. The agent passes it in `origins`; the
host enforces what is the host's:

- an origin is a scheme, a host (a name or an IP literal) and a port; it is
  **stored by the host** (`<home>/services/<id>/origins.json`), never in the
  service's store; `services.origins {id}` lists them and
  `services.revokeOrigin {id, origin}` removes one; removing the service with
  its data removes them;
- **never loopback, link-local (169.254.0.0/16, fe80::/10), unspecified,
  multicast, or one of this computer's own addresses**; a private address is
  allowed for that origin;
- **the pin is the addresses the name resolved to at that moment**; later
  connections go only to those addresses; **the call returns them**
  (`origins: [{origin, addresses}]`), so the agent shows the owner what they
  agree to: the addresses, not the name. A public name gets no wording of "on
  your network"; it is shown as what it resolved to;
- **behind a proxy the pin cannot be enforced** (the proxy resolves the name
  again, `crates/hub-net/src/proxy.rs`), so **a granted origin is always
  reached directly**, never through the system's proxy; where only the proxy
  can reach it, the request fails `connect`.

**Rejected, by the owner's rule** ("the setup happens outside the host; the
agent asks for the important confirmations"): a click in Pocket Hub's app to
allow a guide, and a click in the app to grant an origin. The host enforces
the limits above; the agent confirms with the owner.

## 3. The risk

**A setup guide is instructions an agent follows with the owner's rights**; a
guide from a publisher the owner does not trust is as powerful as `native`.
What stands between:

- **the store**: a version from a non-official key already waits for an
  editor (`TRUST.md` §7), who now reads the guide too; a guide that fetches
  instructions at run time is refused;
- **disclosure**: the store page and the install sheet say that the applet
  comes with setup instructions for a coding agent, and whose ("…written by
  Pocket Nexus", or "…by the publisher who calls themselves pocketfan"); **a
  console's own store page** shows the same in the catalog row (a field the
  link's catalog would gain, `QUESTIONS.md` Q4);
- **the host never runs a guide**: nothing happens until the owner starts an
  agent on it, and `services.setupGuide` names the publisher and whether the
  guide is signed.
