#!/bin/sh
# Builds the app for a release and packs it twice:
#
#   app/dist/Pocket-Hub-<version>-arm64.dmg     a disk image: the app beside a
#                                               link to /Applications, the way
#                                               a Mac app is handed to people
#   app/dist/Pocket-Hub-<version>-arm64.zip     the app, as `ditto` packs one
#
# and prints their SHA-256. Both are made with what macOS has (`hdiutil`,
# `ditto`). docs/CONTRIBUTING.md "Releasing" has the steps around it (the tag,
# the release).
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
# it, compressed with lzfse. The image is mounted once, out of Finder's sight,
# and the app in it checked as the one above was.
image="$here/dist/Pocket-Hub-$version-arm64.dmg"
staging="$(mktemp -d)"
mounted="$(mktemp -d)"
trap 'hdiutil detach -quiet "$mounted" 2>/dev/null || true; rm -rf "$staging" "$mounted"' EXIT
ditto "$app" "$staging/Pocket Hub.app"
ln -s /Applications "$staging/Applications"
rm -f "$image"
hdiutil create -quiet -volname "Pocket Hub" -srcfolder "$staging" -fs HFS+ -format ULFO "$image"
hdiutil verify -quiet "$image"
hdiutil attach -quiet -readonly -nobrowse -mountpoint "$mounted" "$image"
codesign --verify --deep --strict "$mounted/Pocket Hub.app"
if [ "$("$mounted/Pocket Hub.app/Contents/Helpers/pocket-hubd" --version)" != "pocket-hubd $version" ]; then
    echo "error: the disk image does not hold the app just built" >&2
    exit 1
fi
hdiutil detach -quiet "$mounted"

echo "Pocket Hub $version, built from $commit"
shasum -a 256 "$image" "$archive"
