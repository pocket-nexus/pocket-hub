#!/bin/sh
# Builds the app for a release and packs it twice:
#
#   app/dist/Pocket-Hub-<version>-arm64.dmg     a disk image: the app beside a
#                                               link to /Applications on the
#                                               picture app/Artwork draws, the
#                                               way a Mac app is handed to people
#   app/dist/Pocket-Hub-<version>-arm64.zip     the app, as `ditto` packs one
#   app/dist/appcast.xml                        the feed installed copies of
#                                               the app update themselves from:
#                                               it names the disk image, and
#                                               both are signed with the
#                                               release key
#
# and prints their SHA-256. The archives are made with what macOS has
# (`hdiutil`, `ditto`, Finder, `codesign`, `notarytool`, `stapler`), the feed
# with Sparkle's generate_appcast. docs/CONTRIBUTING.md "Releasing" has the
# steps around it (the tag, the release, the credentials), and
# .github/workflows/release.yml runs it for a tag.
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
# A release is signed with the Developer ID of TypeSafe Limited, which
# publishes Pocket Hub, and notarized by Apple, so a downloaded copy opens
# like any other app. Every executable carries the hardened runtime and a
# secure timestamp (both required by notarization); the app is notarized and
# its ticket stapled to it before it is packed, and the disk image is signed,
# notarized and stapled in turn, so Gatekeeper finds the ticket without
# asking Apple. The certificate and its private key are in a keychain, and
# notarytool's credentials in the keychain profile named below
# (`xcrun notarytool store-credentials`); both are checked before the build.
# A build signed otherwise is not a release, so there is no ad hoc way
# through this script: builds for development stay ad hoc (the project's own
# signing settings).
#
# The release key is Sparkle's EdDSA key (AppUpdater.swift): installed copies
# take an update only when it and the feed are signed with it (SUPublicEDKey
# in Info.plist). It comes in SPARKLE_PRIVATE_KEY (the release workflow's
# secret of that name), is checked before the build too, and is taken out of
# the environment at once: what this script starts (Xcode, its build phases,
# notarytool) never sees it, and it reaches generate_appcast on its standard
# input alone.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
derived="$here/.build/release-derived-data"
app="$derived/Build/Products/Release/Pocket Hub.app"
team="9FDAZCRAN3"
identity="Developer ID Application: TypeSafe Limited ($team)"
notary_profile="pocket-hub-notary"
release_key="${SPARKLE_PRIVATE_KEY:-}"
unset SPARKLE_PRIVATE_KEY

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
# The signature and the notarization, checked before the build for the same
# reason. `notarytool history` asks Apple, so it proves the credentials work.
if ! security find-identity -v -p codesigning | grep -qF "\"$identity\""; then
    echo "error: no valid \"$identity\" with its private key in the keychain: a release is signed with it" >&2
    exit 1
fi
if ! xcrun notarytool history --keychain-profile "$notary_profile" >/dev/null 2>&1; then
    echo "error: notarytool's keychain profile \"$notary_profile\" is missing or Apple refuses it: store it with xcrun notarytool store-credentials (docs/CONTRIBUTING.md \"Releasing\")" >&2
    exit 1
fi
if [ -z "$release_key" ]; then
    echo "error: SPARKLE_PRIVATE_KEY is not set: the appcast is signed with the release key (docs/CONTRIBUTING.md \"Releasing\")" >&2
    exit 1
fi

work="$(mktemp -d)"
mounted="$(mktemp -d)"
trap 'hdiutil detach -quiet "/Volumes/$volume" 2>/dev/null || true; hdiutil detach -quiet "$mounted" 2>/dev/null || true; rm -rf "$work" "$mounted"' EXIT

# notarize <file>: hands the file to Apple's notary service and waits for its
# verdict (minutes, most often). A file the service does not accept stops
# the release with the service's log, which names each problem it found.
notarize() {
    xcrun notarytool submit "$1" --keychain-profile "$notary_profile" --wait \
        --output-format plist >"$work/notary.plist" 2>"$work/notary.err" || true
    status="$(plutil -extract status raw -o - "$work/notary.plist" 2>/dev/null || true)"
    if [ "$status" != Accepted ]; then
        id="$(plutil -extract id raw -o - "$work/notary.plist" 2>/dev/null || true)"
        echo "error: Apple's notary service did not accept $(basename "$1"): ${status:-no answer}" >&2
        cat "$work/notary.err" >&2
        if [ -n "$id" ]; then
            xcrun notarytool log "$id" --keychain-profile "$notary_profile" >&2 || true
        fi
        exit 1
    fi
    echo "Notarized $(basename "$1") ($(plutil -extract id raw -o - "$work/notary.plist"))"
}

# A clean build: nothing of an earlier one is packed. Xcode signs the app and
# embed-core.sh the helpers with the Developer ID given here, the hardened
# runtime being on in the project. A plain build adds the entitlement that
# lets a debugger attach (get-task-allow), which notarization refuses; only
# an archive leaves it out by itself, so it is turned off here.
rm -rf "$derived"
xcodebuild -quiet -project PocketHub.xcodeproj -scheme PocketHub -configuration Release \
    -destination "platform=macOS,arch=arm64" -derivedDataPath "$derived" \
    CODE_SIGN_IDENTITY="$identity" OTHER_CODE_SIGN_FLAGS=--timestamp \
    CODE_SIGN_INJECT_BASE_ENTITLEMENTS=NO build
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
# What notarization requires of each executable, checked here so a release
# stops with one line instead of after an upload. Sparkle's programs are
# signed by the build phase sign-sparkle.sh.
sparkle="$app/Contents/Frameworks/Sparkle.framework/Versions/B"
for code in "$app/Contents/Helpers/pocket-hubd" "$app/Contents/Helpers/pocket-hub" \
    "$sparkle/Autoupdate" "$sparkle/Updater.app" "$sparkle" "$app"; do
    signature="$(codesign --display --verbose=2 "$code" 2>&1)"
    if ! printf '%s\n' "$signature" | grep -qxF "Authority=$identity" ||
        ! printf '%s\n' "$signature" | grep -q '^Timestamp=' ||
        ! printf '%s\n' "$signature" | grep -q '^CodeDirectory .*(runtime)'; then
        echo "error: $code is not signed by $identity with the hardened runtime and a secure timestamp" >&2
        exit 1
    fi
done
if codesign --display --entitlements - --xml "$app" 2>/dev/null | grep -q get-task-allow; then
    echo "error: the app lets a debugger attach (get-task-allow): notarization refuses it" >&2
    exit 1
fi

# The app is notarized as a zip of itself, and the ticket stapled to it: both
# archives below hold the stapled app.
ditto -c -k --keepParent "$app" "$work/notarize.zip"
notarize "$work/notarize.zip"
xcrun stapler staple -q "$app"
xcrun stapler validate -q "$app"

mkdir -p "$here/dist"
archive="$here/dist/Pocket-Hub-$version-arm64.zip"
rm -f "$archive"
ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"

# The disk image: the app and a link to /Applications in a volume named after
# it, on the picture of app/Artwork/DMGBackground.tiff (drawn by
# dmg-background.swift there), compressed with lzfse. It is made writable,
# laid out by Finder, and compressed after.
image="$here/dist/Pocket-Hub-$version-arm64.dmg"
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
# The disk image is signed, notarized and stapled itself: Gatekeeper checks
# the image a person opens before the app in it.
codesign --sign "$identity" --timestamp "$image"
notarize "$image"
xcrun stapler staple -q "$image"
xcrun stapler validate -q "$image"
hdiutil verify -quiet "$image"
hdiutil attach -quiet -readonly -nobrowse -mountpoint "$mounted" "$image"
if [ ! -s "$mounted/.DS_Store" ] || [ ! -s "$mounted/.background/background.tiff" ]; then
    echo "error: the disk image has no window laid out by Finder" >&2
    exit 1
fi
codesign --verify --deep --strict "$mounted/Pocket Hub.app"
xcrun stapler validate -q "$mounted/Pocket Hub.app"
if [ "$("$mounted/Pocket Hub.app/Contents/Helpers/pocket-hubd" --version)" != "pocket-hubd $version" ]; then
    echo "error: the disk image does not hold the app just built" >&2
    exit 1
fi
hdiutil detach -quiet "$mounted"

# The feed installed copies read (Info.plist's SUFeedURL: the appcast.xml of
# the latest release, so this file goes on the release with the archives).
# Its update is the disk image, the file a person downloads: signed with the
# Developer ID, it is also what Sparkle accepts a new release key from, were
# this one lost (Sparkle, "Rotating signing keys"; the app verifies an update
# before it opens it, SUVerifyUpdateBeforeExtraction). generate_appcast reads
# the version and the minimum macOS from the app in the image, names the
# image at the address the release gives it, and signs the image and the feed
# with the release key, given on its standard input by printf, a builtin, so
# the key is in no program's arguments. It works in a folder
# holding the image alone, so the feed lists this release only and makes no
# delta from an earlier one. A key that is not the one Info.plist names
# leaves the update unsigned, which the check after it reports.
feed="$work/feed"
mkdir "$feed"
cp "$image" "$feed/"
printf '%s' "$release_key" |
    "$derived/SourcePackages/artifacts/sparkle/Sparkle/bin/generate_appcast" --ed-key-file - \
        --download-url-prefix "https://github.com/pocket-nexus/pocket-hub/releases/download/v$version/" \
        --link "https://hub.pocket.nexus" "$feed"
if ! grep -q "sparkle:edSignature=" "$feed/appcast.xml"; then
    echo "error: the appcast's update is not signed: SPARKLE_PRIVATE_KEY is not the key Info.plist names (SUPublicEDKey)" >&2
    exit 1
fi
if ! grep -q "<!-- sparkle-signatures:" "$feed/appcast.xml"; then
    echo "error: the appcast itself is not signed, and the app takes only a signed one (SURequireSignedFeed)" >&2
    exit 1
fi
cp "$feed/appcast.xml" "$here/dist/appcast.xml"

echo "Pocket Hub $version, built from $commit, signed by $identity and notarized"
shasum -a 256 "$image" "$archive" "$here/dist/appcast.xml"
