# Deep links

A deep link opens Pocket Hub's app on one thing. Something outside Hub carries
it, today the applet store's website on each applet's page, and the app acts
on it. This page is the whole contract between the two: the website builds
exactly the links of §1, and the app does exactly what §2 says.

The core takes no part. A link reaches the app, which is a client of the
control API like any other (`docs/CONTROL-API.md`), and nothing here adds a
method to it.

## 1. The link

```
pocket-hub://store/app/<id>
```

| Part | Value |
| --- | --- |
| scheme | `pocket-hub`. The app is registered for it with the system (`CFBundleURLTypes` in `app/PocketHub/Info.plist`) |
| host | `store`: the part of Hub the link opens. It is the only one today |
| path | `/app/<id>`: one applet of the store |

`<id>` is the applet's id as the store lists it, which is its manifest's:
`dev.pocket-nexus.hn`. The store publishes only ids of the shape
`[a-z0-9]+(\.[a-z0-9][a-z0-9-]*){2,5}`, so an id needs no percent-encoding
and a link carries it as it is.

The scheme and the host compare without case (RFC 3986 §3.1, §3.2.2); the
path does not. A link has no user, password or port. **A query and a fragment
are reserved**: a link of today has neither, and Hub ignores both, so a later
link that adds one still opens on a Hub that predates it.

The website's page of an applet is `https://applets.pocket.nexus/app/<id>`;
its link into Hub is the same path under `pocket-hub://store`.

## 2. What Hub does with it

Hub comes to the front with its window, starting first when it was not
running, and then (a window that is already open stays as it is):

1. **It finds the applet in the store's catalog** (`store.apps` without a
   device, `docs/CONTROL-API.md` §3 "Store"). An id the catalog Hub holds
   does not list is asked for once more with `refresh`: the applet may have
   been published since Hub last fetched. An id that is still not there is
   said so, and nothing else happens.
2. **It chooses a device**, among the paired devices whose target the applet
   has a variant for: one that is linked before one that is not, and the one
   the Store installs on before another. The window goes to the Store page,
   installing on that device; with no device that can run the applet, the
   device the Store installs on stays as it was.
3. **It opens the applet's sheet**: what the applet is, and the one action
   that fits the device: Install, Update when the device holds an older
   version, Open when it holds this one. With several devices that can run
   the applet, the sheet chooses between them. With no device paired, or
   none that can run it, the sheet says so in place of the action. **For a
   device that takes no applet from the computer** (an M5Stack Tab5: its
   `supports` has no `push`, `docs/CONTROL-API.md` §2) the sheet has no
   Install and no Update, and says where that device installs the applet:
   its own store page.

**A link shows; it never installs.** Any page can carry a link, and a browser
can be told to stop asking before it follows one, so what a link does by
itself is bounded by what is safe to happen unasked: a window and a sheet.
Installing is the owner's click in that sheet, on the device the sheet names.
It is not the Return key's either: a link can bring the sheet up while the
owner is typing somewhere else, so Install is never the sheet's default
button.

A link carries an id and nothing else. Hub takes the applet's title, its
package and where to fetch it from its own catalog, never from the link, and
an id is only ever compared with the catalog's.

A link that arrives while the core is starting, or cannot be reached, waits
for it; a newer link replaces one that is still waiting.

**A link Hub does not understand opens nothing.** Another host, a path of
another shape, or an id outside §1's grammar: Hub comes to the front and
says that it cannot open the link.

**On a computer without Hub, a link opens nothing either**, and the browser
tells the page nothing of it: no app claims the scheme. So the page that
carries a link says, beside it, where Hub is to be had:
`https://hub.pocket.nexus/`, the download page (`site/`, docs/CONTRIBUTING.md
"The download page"). That page takes nothing from the page that sent the
visitor: once Hub is installed, the same link opens it.

## 3. Where it is implemented

| | |
| --- | --- |
| The grammar | `app/Sources/HubAppModel/DeepLink.swift` |
| The catalog lookup and the choice of a device | `app/Sources/HubAppModel/StoreModel.swift` |
| Receiving a link, and following it | `AppDelegate` and `AppState` in `app/PocketHub/AppState.swift` |
| The sheet | `StoreAppDetails` in `app/PocketHub/StoreSection.swift` |
| The website's side | `store/site/src/hub.ts` in [pocket-nexus/applets](https://github.com/pocket-nexus/applets) |
| Where a computer without Hub gets it | `site/`, served at `https://hub.pocket.nexus/` |

A change to §1 or §2 starts on this page, and reaches the website's side in
the same change.
