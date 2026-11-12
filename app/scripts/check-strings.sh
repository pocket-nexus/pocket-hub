#!/bin/sh
# Brings Localization/Localizable.xcstrings in line with the source and checks
# that every key has English and Simplified Chinese.
#
# The keys come from the compiler itself (-emit-localized-strings), so every
# Text("…"), Button("…"), String(localized: "…") and other LocalizedStringKey
# use is found, interpolations included, exactly as SwiftUI will look it up.
# Keys no longer in the source are marked stale; fill-catalog.py lists them.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
scratch="$here/.build/strings-build"

cd "$here"
# A clean scratch build: an incremental one would not re-emit the strings of
# unchanged files, and would keep those of deleted ones.
rm -rf "$scratch"
swift build --product PocketHub --scratch-path "$scratch" \
    -Xswiftc -emit-localized-strings >/dev/null
find "$scratch" -name '*.stringsdata' -print0 | xargs -0 xcrun xcstringstool sync Localization/Localizable.xcstrings --stringsdata
python3 scripts/fill-catalog.py
