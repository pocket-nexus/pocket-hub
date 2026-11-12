# Three applets end to end

Proposal (see `README.md`). The three kinds of applets, each with its
manifests, the calls it makes, and what the owner sees. The pilots of the
direction's order are these three. Sizes are computed from the formats, not
measured.

| Kind | Pilot | Sandbox | Needs Bun |
| --- | --- | --- | --- |
| no service | GitHub | the applet's (its manifest's capabilities and hosts) | no |
| portable service | X | the service's (`TRUST.md` §1) | yes |
| unrestricted service | YouTube | none, disclosed | yes |

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

`applet.services` (`store`, `images`, `glyphs` today) is gone. `image.url`
and `text.glyphs.deferred` are enhancements: on a host with no system
service the applet still runs, with initials for avatars and ASCII for other
scripts.

| Step | Device | Link | Hub |
| --- | --- | --- | --- |
| start | the host opens the system channel because the plan has `image.url` | `channel.open {app, hash, service:"system"}` | the system service accepts it in-process; no process, no Bun |
| sign-in (`QUESTIONS.md` Q3) | the worker asks github.com for a device code and shows it with "github.com/login/device"; it polls for the token and keeps it in `data.fs` | `net.http` to github.com | Hub checks `github.com` against the applet's hosts (`QUESTIONS.md` Q4) |
| a page | the worker asks the API with the token | `net.http` GET `https://api.github.com/…` | as today, under the applet's hosts |
| an avatar | `images.image(() => ({ url, width: 24, height: 24, round: true }))` returns `pending` | `REQUEST image.render {url, width:24, height:24, round:true}` on the system channel | the URL's host is in the applet's hosts (from the package of `app` and `hash`); fetch, decode, 24 × 24 at density 1, alpha for the circle: RGBA4444 from the device's stated formats |
| | the state becomes `ready`; `ResourceImage` draws it | `image.head {width:24, height:24, format:"rgba4444", bytes:1160}`, 1,160 bytes, `REPLY {bytes:1160}` | |
| a name in Chinese | the glyph module asks for the codepoints on the page | `REQUEST text.atlas {px:13, text:"…"}` | rasterizes ASCII and the codepoints at the device's density |
| | loads the atlas into slot 21 | `text.head {bytes, glyphs, missing}`, the atlas, `REPLY` | |

**What the owner sees at install**: as today, the applet and "Reaches
api.github.com, github.com and avatars.githubusercontent.com". Nothing about
a computer, and nothing is downloaded but the applet.

## 2. X: a portable service

`pocket.app.json`: `requires` gains `service.channel` (today `link.channel`),
`enhances` gains `text.glyphs.deferred`; `applet` has no `services` and no
`hosts`: everything the applet shows comes from its service.

`pocket.service.json`:

```json
{
  "service": 1,
  "id": "dev.pocket-nexus.x",
  "entry": "service.js",
  "requires": ["image", "net", "signin", "store"],
  "hosts": ["abs.twimg.com", "api.x.com", "pbs.twimg.com", "x.com"]
}
```

`abs.twimg.com` is added to the direction's example: the service reads the
operation ids of x.com's web API from x.com's client scripts there
(`docs/X.md` in the applets). `text` is not required: the applet takes its
glyphs from the system service itself.

| Step | Applet | Service | Hub |
| --- | --- | --- | --- |
| start | the host opens two channels: its own and the system channel | | release `(dev.pocket-nexus.x, hash)` is installed: starts the process under the profile, `service.hello` → `service.ready {host}` → `host.attach` → `host.ready`, then `channel.offer` → `channel.attach` → `channel.ready`, and the device's `REPLY {generation}` |
| first page | `request("timeline", {kind:"following"})` | `host.store.getJson("session")` is `null`: throws `ServiceError("signed-out")` | relays the error reply |
| | shows "Not signed in to X" and a button "Sign In on Your Mac" | | |
| sign-in | `request("signin")`; shows "Finish signing in on your Mac" | `host.signin.web({title:"Sign in to X", url:"https://x.com/i/flow/login", until:{cookies:{host:"x.com", names:["auth_token","ct0"]}}})`; sends `call.progress("signin.wait")` every 20 s, so the applet's request does not stall (L§5.5) | checks `x.com` against the hosts; sends the control API event `signin.requested` to the app, which opens the window; returns the two cookies when x.com sets them |
| | | `host.store.setJson("session", {authToken, ct0, at})`; replies `{}` | the store, durable before the reply |
| operation ids | | once a day: `host.net.fetch("https://abs.twimg.com/…/main.<hash>.js")`, kept in the store | |
| the timeline | `request("timeline", …)` again | `host.net.fetch("https://x.com/i/api/graphql/<op>/HomeLatestTimeline?variables=…&features=…", {headers:{authorization, cookie, "x-csrf-token"}})`, reduces the answer to a page | checks `x.com`; the destination policy; logs the URL without its query values |
| | reads the page from the response stream | the page is over LINK_APP_MSG (8 KiB): `call.progress("timeline.head", {bytes})`, `call.respond()`, the bytes, `return {cursor}` | relays under the applet's credit |
| an avatar | `request("avatar", {url})` | `channel.image.render({url}, {width:32, height:32, round:true})`; head, the entry (32 × 32 RGBA4444, 2,056 bytes), reply | checks `pbs.twimg.com`; formats from the device's statement |
| | `uploadImgEntry` | | |
| glyphs | the framework's glyph module, on the system channel | | as GitHub |

**What the owner sees at install**: the applet, "Reaches abs.twimg.com,
api.x.com, pbs.twimg.com and x.com. Keeps your X sign-in on this Mac." When
Bun is not on the Mac yet, the install's progress says Pocket Hub is getting
it ready (today's `runtime` stage, `MIGRATION.md` §1). At the first sign-in,
a window titled "Sign in to X" opens on the Mac, showing x.com as the page's
host.

**On a host that is a device** (later): the same `service.js` runs there if
that host provides `image`, `net`, `signin` and `store`; the sign-in window
opens on that device.

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

**`image` is added to the direction's `["media", "native"]`**: thumbnails
are pictures for the device, and without `channel.image` the service would
decode and encode them for the 3DS itself, which is the counter-example the
direction names. `hosts` binds only the SDK's calls (`TRUST.md` §4):
`*.googlevideo.com` for the media URLs it hands `channel.media`,
`i.ytimg.com` for thumbnails. `yt-dlp`'s own traffic does not pass Hub.

| Step | Applet | Service | Hub |
| --- | --- | --- | --- |
| start | as X | `host.native.files` holds no `yt-dlp`: fetches the release with Bun's own `fetch`, checks it, keeps it there; the applet's first request gets progress meanwhile | no profile: the process runs with the owner's rights and today's environment, and `files` in `service.ready` |
| a list | `request("feed", {topic:"tech"})` | runs `yt-dlp --flat-playlist -J "ytsearch20:…"` and replies with the items | |
| a thumbnail | `request("thumb", {id})` | `channel.image.render({url:"https://i.ytimg.com/vi/<id>/mqdefault.jpg"}, {width:128, height:64, fit:"cover"})` | checks `i.ytimg.com`; an entry in the device's format |
| play | `request("play", {id, at})` (no codec: the applet no longer chooses one) | `yt-dlp -g -f 18 <id>` gives one address with sound and picture; `channel.media.open({url, headers}, {at})` | checks the name against `*.googlevideo.com`; reads the device's statement: H.264 on a New 3DS, PAL8 on a console without the decoder; fits the picture in 512 × 256; ADPCM sound; issues a ticket |
| | opens `mediaPlayer` with `source`, sizes its picture from `width` and `height` | replies `{source, codec, width, height, fps, durationMs, title}` | the stream goes from Hub to the console on the side TCP connection |
| seek | `request("seek", {at})` | `session.seek(at)`; replies with the new source | a new ticket; the old stream ends |
| stop | `request("stop")` | `session.close()` | |

Format 18 is the single-file format `yt-dlp` lists for most videos; a video
without one needs sound and picture joined before `channel.media` takes them
(a `feed` from a joining program, or a second input; `QUESTIONS.md` Q11).

**What the owner sees at install, in Pocket Hub**: on the store page, the mark
"Runs its own programs on your Mac"; on Install, the dialog of `TRUST.md` §4:

> **YouTube runs its own programs on this Mac**
>
> Other applets can do only what Pocket Hub allows. YouTube runs programs of
> its own, with the same access to your files, accounts and network that you
> have, and Pocket Hub cannot limit or watch what they do. Install it only if
> you trust Pocket Nexus, who published it.
>
> [Cancel] [Install]

**From the console's store page**, the applet installs and says "Allow
YouTube in Pocket Hub on your Mac" until the owner allows it there
(`QUESTIONS.md` Q16).

**On a host that does not provide `native`** (a device hosting its own
services, later), the channel is refused with `capability` and the applet
says "YouTube needs a computer".
