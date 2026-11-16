# Pocket Hub

Pocket Hub connects handheld devices that run
[Pocket applets](https://applets.pocket.nexus) to your Mac. Set a Nintendo 3DS
up once over Wi-Fi, and from then on Pocket Hub installs applets on it from the
store or from a `.pocket` file, keeps the 3DS's software up to date, takes
screenshots of both screens and shows the console's live log.

While a device is connected, Pocket Hub also does what the device cannot do by
itself: it makes the network requests its applets ask for, and runs the part of
an applet that lives on the Mac (its *daemon*).

The first supported device is the **Nintendo 3DS**, over Wi-Fi.

## What you need

- A Mac with Apple silicon, on macOS 15 or later. There is no Intel build.
- A Nintendo 3DS that opens the Homebrew Launcher, on the same Wi-Fi network as
  the Mac.
- For the first setup, [ftpd](https://github.com/mtheall/ftpd) on the 3DS, or a
  card reader for its SD card.

## Install

1. Download `Pocket-Hub-<version>-arm64.zip` from the
   [latest release](https://github.com/pocket-nexus/pocket-hub/releases/latest).
2. Unzip it and move **Pocket Hub** to Applications.
3. Open it. The app is not notarized yet, so macOS stops it the first time:
   choose **Open Anyway** in System Settings › Privacy & Security, then open it
   again.

Pocket Hub keeps an item in the menu bar. Closing the window, or quitting the
app, leaves Pocket Hub running in the background so devices stay connected;
**Quit and Stop Pocket Hub** in the menu bar item stops it altogether. Settings
can open Pocket Hub at login.

## Set up a 3DS

Choose **Add a Device…** (in the sidebar, or File › Add a Device…, ⌘N).

1. On the 3DS, open the Homebrew Launcher and start **ftpd**.
2. Pocket Hub looks for consoles running ftpd on the network. Choose yours, or
   type the address ftpd shows at the top of its screen (for example
   `192.168.1.57:5000`).
3. Pocket Hub copies the 3DS's software to the console and introduces it to
   this Mac. Keep ftpd open until it finishes.
4. On the 3DS, press START to leave ftpd, then open **Pocket Shell** from the
   Homebrew Launcher. It connects to the Mac by itself, now and every time you
   open it.

With the SD card instead: choose **Use the SD Card Instead…**, pick the card in
the Mac's card reader, then eject it, put it back in the 3DS and open Pocket
Shell.

A console can be set up with up to 8 computers. When it already has 8, open
Pocket Shell on the console and remove one in its settings.

## Applets

A device's **Applets** page lists what is installed on it. Each applet can be
opened, updated when the store has a newer version, or deleted from its …
menu; deleting an applet also deletes its data on the device. Below, **Available
in the Store** lists the applets that device can run, and Install puts one on
it. To install an applet you have as a file, drop the `.pocket` file on the
page.

The console's own store page lists the same applets and installs them through
Pocket Hub.

Each applet's page on [applets.pocket.nexus](https://applets.pocket.nexus) has
an Install button that opens Pocket Hub on that applet. A link from a website
only shows the applet; installing is always your click in Pocket Hub.

## The rest of the window

- **Screenshot**: both screens of the console as they are now.
- **Log**: what the console and its applets write, while you watch.
- **Details**: the device's model, software version, connection and address.
  When an update of the 3DS's software is available, the Applets page offers
  **Update Now**: the update goes over Wi-Fi and the 3DS restarts into it.
- **Advanced › Daemons**: the parts of applets that run on this Mac. A daemon
  starts when its applet needs it and stops when it is idle. Pocket Hub
  downloads the runtime daemons need ([Bun](https://bun.sh)) the first time.
- **Settings**: this Mac's name as your devices show it, the app's language
  (English or Simplified Chinese), and opening at login.

## Command line

The app carries `pocket-hub`, a command line that drives the same Pocket Hub as
the window:

```sh
alias pocket-hub="'/Applications/Pocket Hub.app/Contents/Helpers/pocket-hub'"

pocket-hub status                             # starts Pocket Hub when it is not running
pocket-hub setup 3ds                          # with ftpd open on the console
pocket-hub devices
pocket-hub store                              # what the store offers
pocket-hub store install dev.pocket-nexus.hn
pocket-hub device push demo.pocket            # an applet from a file
pocket-hub device shot out.png
pocket-hub device logs
pocket-hub device update                      # the 3DS's software
pocket-hub --help
```

`device` takes a device id first (`pocket-hub device 0123456789abcdef shot`);
with one device set up, the id can be left out. `--json` prints the raw
answers.

## Files and network

- Pocket Hub keeps its files in `~/Library/Application Support/nexus.pocket.hub`,
  and its log in `logs/hubd.log` there. Set `POCKET_HUB_HOME` to use another
  folder.
- Devices connect to it on TCP port 8628, and find it on the local network by
  the UDP beacons it sends to port 8627. When port 8628 is taken, Pocket Hub
  says so and does not start; it does not move to another port.

## Tested hardware

Pocket Hub has run on a New Nintendo 3DS XL over Wi-Fi: setup over ftpd,
connecting and reconnecting, installing and deleting applets (from the Mac and
from the console's own store page), screenshots, the log, applets' network
requests and daemons, and updating the 3DS's software. An Old 3DS, other
connections and other devices have not been tried yet.

## Building from source

Pocket Hub is a Rust core with a SwiftUI app.
[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) has how to build, test and change
it, and [sdk/daemon](sdk/daemon/README.md) is the SDK an applet's daemon is
written with.

## License

[GPL-3.0-or-later](LICENSE).
