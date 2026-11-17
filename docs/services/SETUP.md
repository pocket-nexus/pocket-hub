# Setting up an applet's service: the guide, the inlet, the origin

Proposal (see `README.md`). **Decided on 2026-11-17: `signin` is not a
capability.** An applet whose service needs an account or a token (X's
session, Discord's bot token, Home Assistant's address and token) ships **a
setup guide**: instructions that a coding agent on the service host's own
machine follows to do the setup with the owner (open a browser, have the
owner confirm, obtain the session or the token). **That happens outside the
service host and is not its responsibility.** The host owes the flow three
small things, each argued below: it carries the guide and says where it is
(§1), it takes the agent's result and hands it to the service (§2), and it
lets the owner grant a service one private address (§3). §4 is the risk.

An applet whose setup nobody has done says so, in its owner's words: "Not
signed in to X. Set it up on your Mac" ("on this device" on Omarchy Pocket).

## 1. The guide travels with the release

| Point | Decision proposed | Why |
| --- | --- | --- |
| Where it lives | **in the service package**, as a folder `setup/` beside `pocket.service.json`, named by the manifest's `setup` field (`MANIFEST.md` §1) | the setup's result is the service's (it lands in the service's store); an applet without a service has no service host to set up (GitHub signs in on the device by its device flow); and the service package is what the second signature already covers by hash |
| Its form | a folder an agent can read: **`SKILL.md`**, with any files it names (scripts, pictures, a JSON schema of the result) inside the same folder; at most **1 MiB** and 64 files | the form coding agents read as a skill; one entry file says where to start |
| Signed | yes: it is inside the service package, whose SHA-256 the release's second signature covers (`TRUST.md` §6); the signed message also states whether a guide is present, so the install sheet can say so before a download | an agent follows it with the owner's rights (§4) |
| Where the host puts it | unpacked with the release, **read-only**: on Hub `<home>/services/<id>/<release>/setup/`; on Omarchy Pocket the same under its host's folder | nothing else to install |
| How an agent finds it | the control API method **`services.setupGuide {id}`** → `{path, release}`, and the command **`pocket-hub service guide <id>`** (on Omarchy Pocket, `pocket-host service guide <id>`) which prints the path | an agent works through the command line; a path is all it needs |
| Who runs it | **never the host.** The host does not read, run or interpret the guide | the guide is for an agent the owner chose to run |

## 2. The inlet: the agent hands its result to the service

The agent ends with a result (cookies, a token, an address). **It delivers one
`setup` message to the service; the service checks it and writes its own
store.** The host does not write into the store for it, so the store's format
stays the service's own.

| Point | Decision proposed | Why |
| --- | --- | --- |
| The call | control API **`services.setup {id, message}`** → `{reply}`; the command **`pocket-hub service setup <id> <file \| ->`** reads the message as JSON | the agent works through the command line |
| Who may call it | any client of the control socket **except a service or a descendant of one** (the peer check, `TRUST.md` §2), and never a `remote` client; not consent-class | the agent is a program of the owner's user, like the command line; an enclosed service cannot reach the control socket at all |
| How it reaches the service | the host starts the service's newest installed release if it is not running (as for a channel), and sends it a `REQUEST` **`setup`** on its host connection (`HOST-PROTOCOL.md` §6, the one request the host sends); the SDK hands it to `Service.setup(message)` (`CAPABILITIES.md` §2); the service's return value is the reply | one path, in the protocol a service already speaks |
| Size | the message is one JSON object of at most **32 KiB** | a session's cookies or a token fit many times over; a larger result belongs in a file the guide tells the agent to read into fields |
| A message the service does not recognise | the service throws `ServiceError("unknown_setup", "…")`, or `invalid_setup` for one it recognises and refuses; `services.setup` fails with the error `service` and `reason` set to that code; a service with no `setup` handler is answered `unknown_setup` by the SDK | the agent learns at once that its result was not taken, and why |
| A development release | `services.setup {id, message, release: "dev"}` delivers to it and its own store | `TRUST.md` §8 |
| How the applet learns it is done | the service's `setup` handler may `emit` an event on its open channels (the SDK gives it the list); otherwise the applet's next request finds the store filled | the host does not know what "done" means for a service |

## 3. Private origins

`signin.form`'s field of kind `origin` was where an owner named a private
address (Home Assistant's). With `signin` gone, it needs a home of its own:

| Point | Decision proposed |
| --- | --- |
| The request | control API **`services.requestOrigin {id, origin}`** (and `pocket-hub service origin <id> <origin>`): any client but a service; creates a pending request that Pocket Hub's app shows (event `service.originRequested`), with the applet's name and publisher |
| The grant | **`services.grantOrigin {id, origin}`: consent-class**, only from Pocket Hub's app by the owner's click (`TRUST.md` §2), because it widens what an enclosed service can reach. On Omarchy Pocket, the owner's click in the shell |
| The rules | the origin is a scheme, a host (a name, or an IP literal) and a port: `http://192.168.1.20:8123` is allowed. **Stored by the host** (`<home>/services/<id>/origins.json`), never in the service's store. **Private addresses allowed for it; loopback, link-local (169.254.0.0/16, fe80::/10), unspecified and multicast never.** A name is pinned to the addresses it resolved to when granted and checked again at each connect, so a later answer of 127.0.0.1 is refused |
| Revocation | `services.revokeOrigin {id, origin}` from any client but a service (it only narrows); `services.origins {id}` lists them; removing the service with its data removes them |

Not ruled on by the owner: `QUESTIONS.md` Q4.

## 4. The risk

**A setup guide is instructions that an agent follows with the owner's
rights.** A guide from a publisher the owner does not trust is as powerful as
`native`: it can tell the agent to download and run anything. Proposed:

- **The store**: a release that carries a guide, from a key other than the
  official one, waits for an editor who reads the guide, as for a native
  release (`TRUST.md` §7); the store page says the applet comes with setup
  instructions for an agent.
- **The install sheet** says, in the owner's words: "X comes with setup
  instructions for a coding agent on your Mac, written by Pocket Nexus. An
  agent that follows them acts with your rights." For a non-official
  publisher, the owner's click is required, as for `native`.
- The host never runs a guide, so nothing happens until the owner starts an
  agent on it.

`QUESTIONS.md` Q3.
