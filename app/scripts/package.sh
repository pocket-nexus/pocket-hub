#!/bin/sh
# Builds the app for a release and packs it twice:
#
#   app/dist/Pocket-Hub-<version>-arm64.dmg     a disk image: the app beside a
#                                               link to /Applications on the
#                                               picture app/Artwork draws, the
#                                               way a Mac app is handed to people
#   app/dist/Pocket-Hub-<version>-arm64.zip     the app, as `ditto` packs one
#
# and prints their SHA-256. Both are made with what macOS has (`hdiutil`,
# `ditto`, Finder). docs/CONTRIBUTING.md "Releasing" has the steps around it
# (the tag, the release).
#
# Finder lays the disk image's window out, as only it writes a folder's
# .DS_Store: the script runs in a logged-in session, macOS asks once whether
# the terminal may control Finder, and the window shows for a few seconds. No
# other volume named Pocket Hub may be mounted meanwhile (an earlier disk
# image opened to look at): Finder finds a disk by its name.
#
# A release is a commit: the build refuses a working tree with changes, and
# says which commit it built. The version is the app's (MARKETING_VERSION),
# and the core and the command line inside must report the same one.
#
# The app is signed ad hoc, as every build is until there is a Developer ID:
# a copy that was downloaded has to be allowed once in System Settings ›
# Privacy & Security before macOS opens it.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
derived="$here/.build/release-derived-data"
app="$derived/Build/Products/Release/Pocket Hub.app"

cd "$here"
if [ -n "$(git status --porcelain)" ]; then
    echo "error: the working tree has changes: a release is built from a commit" >&2
    exit 1
fi
commit="$(git rev-parse HEAD)"
# Finder finds the disk image's volume by its name (below): checked before the
# build, so a release does not stop at its end.
volume="Pocket Hub"
if [ -e "/Volumes/$volume" ]; then
    echo "error: a volume named $volume is mounted: eject it first (Finder finds the disk image's by that name)" >&2
    exit 1
fi

# A clean build: nothing of an earlier one is packed.
rm -rf "$derived"
xcodebuild -quiet -project PocketHub.xcodeproj -scheme PocketHub -configuration Release \
    -destination "platform=macOS,arch=arm64" -derivedDataPath "$derived" build
# This copy is built to be packed, not opened here. Like every build of the app
# it claims pocket-hub: links, and Xcode registers what it builds with Launch
# Services, which may then open a link in it (scripts/check-strings.sh has the
# same step, and docs/CONTRIBUTING.md "Trying a link" why).
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -u "$app"

version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
for binary in "$app/Contents/MacOS/Pocket Hub" "$app/Contents/Helpers/pocket-hubd" "$app/Contents/Helpers/pocket-hub"; do
    archs="$(lipo -archs "$binary")"
    if [ "$archs" != arm64 ]; then
        echo "error: $binary is built for $archs, not for arm64 alone" >&2
        exit 1
    fi
done
for tool in pocket-hubd pocket-hub; do
    reported="$("$app/Contents/Helpers/$tool" --version)"
    if [ "$reported" != "$tool $version" ]; then
        echo "error: the app is version $version and its $tool says \"$reported\"" >&2
        exit 1
    fi
done
codesign --verify --deep --strict "$app"

mkdir -p "$here/dist"
archive="$here/dist/Pocket-Hub-$version-arm64.zip"
rm -f "$archive"
ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"

# The disk image: the app and a link to /Applications in a volume named after
# it, on the picture of app/Artwork/DMGBackground.tiff (drawn by
# dmg-background.swift there), compressed with lzfse. It is made writable,
# laid out by Finder, and compressed after.
image="$here/dist/Pocket-Hub-$version-arm64.dmg"
work="$(mktemp -d)"
mounted="$(mktemp -d)"
trap 'hdiutil detach -quiet "/Volumes/$volume" 2>/dev/null || true; hdiutil detach -quiet "$mounted" 2>/dev/null || true; rm -rf "$work" "$mounted"' EXIT
mkdir -p "$work/staging/.background"
ditto "$app" "$work/staging/Pocket Hub.app"
ln -s /Applications "$work/staging/Applications"
cp "$here/Artwork/DMGBackground.tiff" "$work/staging/.background/background.tiff"
hdiutil create -quiet -volname "$volume" -srcfolder "$work/staging" -fs HFS+ -format UDRW "$work/writable.dmg"
hdiutil attach -quiet -readwrite -noverify -noautoopen "$work/writable.dmg"
# The window is 640 x 400 points under a title bar of 28, the picture's size,
# with nothing but the icons in it. Finder places an item 45 points above the
# centre of its 128-point icon: placed at y 155, the icons stand at y 200, on
# the picture's way between them (measured). The item of the picture's
# folder goes out of the window, for one who shows hidden files. Finder keeps
# a window's size only once the window has opened again and been resized, so
# it is, twice.
osascript <<APPLESCRIPT
tell application "Finder"
    tell disk "$volume"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set pathbar visible of container window to false
        set the bounds of container window to {200, 120, 840, 548}
        set options to the icon view options of container window
        set arrangement of options to not arranged
        set icon size of options to 128
        set text size of options to 13
        set background picture of options to file ".background:background.tiff"
        set position of item "Pocket Hub.app" of container window to {160, 155}
        set position of item "Applications" of container window to {480, 155}
        set position of item ".background" of container window to {900, 900}
        close
        open
        delay 1
        set the bounds of container window to {200, 120, 830, 538}
        delay 1
        set the bounds of container window to {200, 120, 840, 548}
        delay 2
        close
    end tell
end tell
APPLESCRIPT
# What the file system logged of the build is not part of the image.
rm -rf "/Volumes/$volume/.fseventsd"
sync
hdiutil detach -quiet "/Volumes/$volume"
rm -f "$image"
hdiutil convert -quiet "$work/writable.dmg" -format ULFO -o "$image"
hdiutil verify -quiet "$image"
hdiutil attach -quiet -readonly -nobrowse -mountpoint "$mounted" "$image"
if [ ! -s "$mounted/.DS_Store" ] || [ ! -s "$mounted/.background/background.tiff" ]; then
    echo "error: the disk image has no window laid out by Finder" >&2
    exit 1
fi
codesign --verify --deep --strict "$mounted/Pocket Hub.app"
if [ "$("$mounted/Pocket Hub.app/Contents/Helpers/pocket-hubd" --version)" != "pocket-hubd $version" ]; then
    echo "error: the disk image does not hold the app just built" >&2
    exit 1
fi
hdiutil detach -quiet "$mounted"

echo "Pocket Hub $version, built from $commit"
shasum -a 256 "$image" "$archive"
