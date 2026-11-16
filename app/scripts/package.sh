#!/bin/sh
# Builds the app for a release and packs it:
#
#   app/dist/Pocket-Hub-<version>-arm64.zip     the app, as `ditto` packs one
#
# and prints the archive's SHA-256. docs/CONTRIBUTING.md "Releasing" has the
# steps around it (the tag, the release).
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
echo "Pocket Hub $version, built from $commit"
shasum -a 256 "$archive"
