# Applets end to end

Proposal (see `README.md`). The three kinds of applets, each with its
manifests, the calls it makes, and what the owner sees, on the three devices
of `DEVICES.md`; the pilots of the direction's order are GitHub, X and
YouTube (§1 to §3). Discord and Home Assistant (§4, §5) show the setup flow of
`SETUP.md` with a pasted token and a private address. Sizes
are computed from the formats, not measured.

| Kind | Pilot | Enclosure | Needs Bun on its service host |
| --- | --- | --- | --- |
| no service | GitHub | the applet's (its manifest's capabilities and hosts) | no |
| portable service | X | the service's (`TRUST.md` §1) | yes |
| unrestricted service | YouTube | none, disclosed | yes |

**The applet's code is the same on the three devices**; what differs is
where its host is: Hub on the Mac for a 3DS or a Tab5, the host on the device
for Omarchy Pocket.

## 1. GitHub: no service

`pocket.app.json`, the parts that change:

```json
{
  "id": "dev.pocket-nexus.github",
  "engine": { "capabilities": {
    "requires": ["text.glyphs.baked", "input.buttons", "net.http", "data.fs"],
    "enhances": ["image.url", "text.glyphs.deferred", "input.analog.left", "system.agent", "display.auxiliary", "input.touch.auxiliary"]
  } },
  "applet": {
    "version": 1,
    "hosts": ["api.github.com", "github.com", "avatars.githubusercontent.com"],
    "store": { "maxBytes": 65536 }
  }
}
```

`applet.services` (`store`, `images`, `glyphs` today) is gone. `data.fs` is
required, which Omarchy Pocket's host contract gains (`DEVICES.md` §7).
`image.url` and `text.glyphs.deferred` are enhancements: without a system
service the applet still runs, with initials for avatars and ASCII for other
scripts.

| Step | Device | Transport | Host |
| --- | --- | --- | --- |
| start | the device opens the system channel because the plan has `image.url` | `channel.open {app, hash, service:"system"}` on the link (3DS, Tab5) or the local connection (Omarchy Pocket) | the system service accepts it in its own process; no Bun |
| sign-in (`QUESTIONS.md` Q31) | the worker asks github.com for a device code and shows it with "github.com/login/device"; it polls for the token and keeps it in `data.fs` | the worker's HTTP | the 3DS's goes through Hub, Omarchy Pocket's through its host, both checking `github.com` against the applet's hosts; the Tab5's own stack checks it (`QUESTIONS.md` Q10) |
| a page | the worker asks the API with the token | the worker's HTTP | as above |
| an avatar | `images.image(() => ({ url, width: 32, height: 32, round: true }))` returns `pending` | `REQUEST image.render {url, width:32, height:32, round:true}` on the system channel | the URL's host is in the applet's hosts (from the package of `app` and `hash`); fetch, decode in the host's process, 32 × 32 at density 1, alpha for the circle: RGBA4444 from the device's stated formats |
| | the state becomes `ready`; `ResourceImage` draws the 32 × 32 content | `image.head {width:32, height:32, contentWidth:32, contentHeight:32, format:"rgba4444", bytes:2056}`, 2,056 bytes, `REPLY {bytes:2056}` | |
| a name in Chinese | the glyph module asks for the codepoints on the page | `REQUEST text.atlas {px:13, bytes}` with the codepoints as its body | rasterizes ASCII and the codepoints at the device's density (2 on Omarchy Pocket's windows) |
| | loads the atlas into slot 21 | `text.head {bytes, glyphs, missing, omitted}`, the atlas, `REPLY` | |

At density 2 the same avatar is a 64 × 64 texture of 8,200 bytes.

**What the owner sees at install**: as today, the applet and "Reaches
api.github.com, github.com and avatars.githubusercontent.com". Nothing about
a computer.

## 2. X: a portable service, set up by an agent

`pocket.app.json`: `requires` gains `service.channel` (today `link.channel`),
`enhances` gains `text.glyphs.deferred`; `applet` has no `services` and no
`hosts`: everything the applet shows comes from its service.

`pocket.service.json`:

```json
{
  "service": 1,
  "id": "dev.pocket-nexus.x",
  "entry": "service.js",
  "requires": ["image", "net", "store"],
  "hosts": ["abs.twimg.com", "api.x.com", "pbs.twimg.com", "x.com"],
  "setup": "setup/SKILL.md"
}
```

`docs/X.md` in the applets says the service reads the operation ids of
x.com's web API from x.com's client scripts; **that those scripts are served
from `abs.twimg.com` is an assumption, not checked here**. The guide
(`setup/SKILL.md`) tells an agent to open a browser at x.com's login page,
have the owner sign in there, read the `auth_token` and `ct0` cookies, and
deliver them with `pocket-hub service setup dev.pocket-nexus.x -`. The host
never opens the browser and never sees the password.

| Step | Applet | Agent, on the service host's machine | Service | Host |
| --- | --- | --- | --- | --- |
| start | the device opens two channels: its own and the system channel | | | release `(dev.pocket-nexus.x, hash)` is installed: starts the process inside the enclosure, `service.hello` → `service.ready {host}` → `host.attach` → `host.ready`, then `channel.offer` → `channel.attach`, and answers `channel.open` |
| first page | `request("timeline", {kind:"following"})` | | `host.store.getJson("session")` is `null`: throws `ServiceError("signed-out")` | relays the error reply |
| | shows "Not signed in to X. Set it up on your Mac" | | | |
| setup | | the owner asks an agent to set X up; it runs `pocket-hub service guide dev.pocket-nexus.x`, reads `SKILL.md`, opens a browser at x.com; the owner signs in and confirms; it reads the two cookies | | `services.setupGuide` prints the guide's folder |
| | | `pocket-hub service setup dev.pocket-nexus.x -` with `{"authToken":"…","ct0":"…"}` | `setup(message, channels)`: checks the two fields, `host.store.setJson("session", …)`, `channel.emit("signed-in")` on each open channel, returns `{ok:true}` | `services.setup` starts the release if it is not running and sends `REQUEST setup` on its host connection; returns the reply to the agent |
| | hears `signed-in`, asks again | | | |
| the timeline | `request("timeline", …)` | | `host.net.fetch("https://x.com/i/api/graphql/<op>/HomeLatestTimeline?…", {headers:{authorization, cookie, "x-csrf-token"}})`, reduces it to a page, sends it after a head event (the page is over LINK_APP_MSG) | checks `x.com`, the destination policy; logs the URL without its query values |
| an avatar | `request("avatar", {url})` | | `channel.image.render({url}, {width:32, height:32, round:true})`: a 32 × 32 RGBA4444 entry, 2,056 bytes at density 1, with its content size | checks `pbs.twimg.com`; formats from `device.accepts` |
| glyphs | the framework's glyph module on the system channel | | | as GitHub |

**Per device**: on a **3DS or a Tab5** the agent runs on the Mac and the
applet says "Set it up on your Mac". On **Omarchy Pocket** the agent is the one
on the device (Codex, which the owner already runs there), the command is
`pocket-host service setup …`, and the browser is whatever the agent can open
there; **the device has no browser in its shell today, so X cannot be set up
on Omarchy Pocket for now**, which the owner accepted.

**What the owner sees at install**: the applet, "Reaches abs.twimg.com,
api.x.com, pbs.twimg.com and x.com. Comes with setup instructions for a
coding agent on your Mac, written by Pocket Nexus."

## 3. YouTube: an unrestricted service

`pocket.app.json`: `requires` gains `service.channel`, `enhances` keeps
`media.playback`.

`pocket.service.json`:

```json
{
  "service": 1,
  "id": "dev.pocket-nexus.youtube",
  "entry": "service.js",
  "requires": ["image", "media", "native"],
  "hosts": ["*.googlevideo.com", "i.ytimg.com"]
}
```

**`image` is added to the direction's `["media", "native"]`**: thumbnails are
pictures for the device, and without `channel.image` the service would encode
them for the 3DS itself. `hosts` binds only the SDK's calls (`TRUST.md` §4):
`*.googlevideo.com` (not a public suffix) for the media URLs it hands
`channel.media`, `i.ytimg.com` for thumbnails. `yt-dlp`'s own traffic does
not pass the host.

| Step | Applet | Service | Host |
| --- | --- | --- | --- |
| start | as X | `host.native.files` holds no `yt-dlp`: fetches the release with Bun's own `fetch`, checks it, keeps it there (`QUESTIONS.md` Q27); the applet's first request gets progress meanwhile | no enclosure: the process runs with the owner's rights and today's environment, and `files` in `service.ready` |
| a list | `request("feed", {topic:"tech"})` | runs `yt-dlp --flat-playlist -J "ytsearch20:…"`, replies with the items | |
| a thumbnail | `request("thumb", {id})` | `channel.image.render({url:"https://i.ytimg.com/vi/<id>/mqdefault.jpg"}, {width:128, height:64, fit:"cover"})` | checks `i.ytimg.com`; a 128 × 64 texture in the device's format |
| play | `request("play", {id, at})` (no codec: the applet no longer chooses one) | `yt-dlp -g -f 18 <id>` gives one address with sound and picture; `channel.media.open({url, headers}, {at})` | checks the name against `*.googlevideo.com`; reads the device's statement: H.264 on a New 3DS, PAL8 on a console without the decoder, RGB565 on Omarchy Pocket; fits the picture in the device's maximum; ADPCM sound; a ticket |
| | opens `mediaPlayer` with `source`, sizes its picture from `width` and `height` | replies `{source, codec, width, height, fps, durationMs, title}` | the stream goes on the side TCP connection: to the console from the Mac, or to 127.0.0.1 on Omarchy Pocket |
| seek | `request("seek", {at})` | `session.seek(at)`; replies with the new source | a new ticket; the old stream ends |
| stop | `request("stop")` | `session.close()` | |

Format 18 is the single-file format `yt-dlp` lists for most videos; a video
without one needs sound and picture joined first (`QUESTIONS.md` Q15).

**What the owner sees at install, in Pocket Hub**: on the store page, "Runs
its own programs on your Mac"; on Install, the dialog of `TRUST.md` §4:

> **YouTube runs its own programs on this Mac**
>
> Other applets can do only what Pocket Hub allows. YouTube runs programs of
> its own, with the same access to your files, accounts and network that you
> have, and Pocket Hub cannot limit or watch what they do. Install it only if
> you trust Pocket Nexus, who published it.
>
> [Cancel] [Install]

**On Omarchy Pocket** the same text with "on this device", in the shell, and
the service runs on the device (its host provides `native`, `DEVICES.md` §5),
as YouTube's search with yt-dlp runs there today. **From a console's store
page**, the applet installs and says "Allow YouTube in Pocket Hub on your
Mac" until the owner allows it there (`QUESTIONS.md` Q20).

## 4. Discord: a portable service with a pasted token

`pocket.service.json`: `requires ["image", "net", "store"]`, `hosts
["cdn.discordapp.com", "discord.com", "gateway.discord.gg"]`, `setup`.
The guide walks the owner through the Discord developer portal as
`docs/DISCORD.md` in the applets does today (create an application, reset the
bot token, turn on Message Content Intent, invite the bot), with the agent
opening each page and the owner confirming each step, and ends with
`pocket-hub service setup dev.pocket-nexus.discord -` and `{"token":"…"}`.

| Step | Applet | Agent | Service | Host |
| --- | --- | --- | --- | --- |
| not set up | "Discord is not set up. Set it up on your Mac" | | `host.store.get("token")` is `null` | |
| setup | | the guide, the portal, the token | `setup(message)`: checks the token with Discord (`host.net.fetch` of `/users/@me`, refusing a user token as today), stores it, opens the gateway with `host.net.websocket`, emits `ready` | delivers the message; the WebSocket lasts until closed |
| messages | lists servers and channels; new messages arrive as events, not by the long poll of today | | gateway events become `channel.emit("message", …)` | |

**Per device**: the 3DS and the Tab5 through the Mac's agent; Omarchy Pocket
through the agent on the device, where the portal opens in whatever browser
that agent has, or the owner pastes the token into the agent's terminal.

## 5. Home Assistant: a token and a private address

`pocket.service.json`: `requires ["net", "store"]`, `hosts []`, `setup`: the
service reaches only what the owner grants. The owner's own instance is a
**granted origin**; so is a Home Assistant Cloud address
(`https://<id>.ui.nabu.casa`), whose suffix a wildcard could not name if it
is a public suffix (not checked). The guide has the agent ask the owner for the instance's address,
check that it answers, have the owner create a long-lived token in Home
Assistant's profile page, then:

| Step | Agent | Owner | Service | Host |
| --- | --- | --- | --- | --- |
| the address | `pocket-hub service origin dev.pocket-nexus.homeassistant http://192.168.1.20:8123` | sees "Home Assistant asks to reach 192.168.1.20:8123 on your network" in Pocket Hub and clicks Allow (consent-class, `QUESTIONS.md` Q4) | | `services.requestOrigin`, then `services.grantOrigin` from the app: stored, pinned, private allowed, loopback and link-local never |
| the token | `pocket-hub service setup dev.pocket-nexus.homeassistant -` with `{"url":"http://192.168.1.20:8123","token":"…"}` | | `setup`: checks the URL is a granted origin by calling `/api/` with the token, stores both | delivers |
| use | | | `host.net.fetch` and `host.net.websocket` to the origin | checks the origin against the grants |

Today the applet sends the address and token from the console's keyboard
(`homeassistant/api.ts` `/setup`); under this flow nothing is typed on the
device. **Per device**: on Omarchy Pocket the owner's click is in the shell,
and the agent is the one on the device.
