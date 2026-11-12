#!/bin/sh
# Brings Localization/Localizable.xcstrings in line with the source, then
# checks it.
#
# The catalog is the one place the app's words are kept. A key is the English
# sentence as the source writes it, and the catalog holds its Simplified
# Chinese; a string with a count also has its English plural forms there, as
# variations. A translation is written into the catalog itself, in Xcode's
# String Catalog editor or in the file.
#
# The keys come from the compiler itself (SWIFT_EMIT_LOC_STRINGS), so every
# Text("…"), Button("…"), String(localized: "…") and other LocalizedStringKey
# use is found, interpolations included, exactly as SwiftUI will look it up.
# The app and the package's libraries are compiled with it alike: the
# libraries' String(localized:) are looked up in the app's catalog too
# (Bundle.main). Ordinary builds leave it off, so Xcode never syncs the
# catalog from the app target's strings alone and marks the libraries' stale.
#
# The sync adds each new key with no translation and marks stale each key the
# source no longer uses. The catalog's test then says which keys still need
# their Simplified Chinese and which stale ones to delete, and fails until
# neither is left.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
derived="$here/.build/strings-derived-data"

cd "$here"
# A clean build: an incremental one would not re-emit the strings of
# unchanged files, and would keep those of deleted ones.
rm -rf "$derived"
xcodebuild -quiet -project PocketHub.xcodeproj -scheme PocketHub -configuration Debug \
    -destination "platform=macOS,arch=$(uname -m)" -derivedDataPath "$derived" \
    SWIFT_EMIT_LOC_STRINGS=YES build
# This build is compiled for its strings, not to be opened. Like every build
# of the app it claims pocket-hub: links (Info.plist, docs/DEEP-LINKS.md), and
# Xcode registers what it builds with Launch Services, which may then open a
# link in this copy, with the default home, instead of the app the developer
# runs. So it is taken out of Launch Services again.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -u "$derived/Build/Products/Debug/Pocket Hub.app"
find "$derived/Build/Intermediates.noindex" -name '*.stringsdata' -print0 |
    xargs -0 xcrun xcstringstool sync Localization/Localizable.xcstrings --stringsdata
swift test --filter LocalizationTests
