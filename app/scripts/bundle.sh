#!/bin/sh
# Assembles "Pocket Hub.app" from the SwiftPM build products.
#
#   app/scripts/bundle.sh [debug|release]      (default: release)
#
# Output: app/build/Pocket Hub.app (or $OUT/Pocket Hub.app), ad-hoc signed.
#
#   Contents/MacOS/Pocket Hub          the SwiftUI app
#   Contents/Resources/*.lproj         the String Catalog compiled (en, zh-Hans)
#   Contents/Helpers/pocket-hubd       the core, copied from the repository's
#   Contents/Helpers/pocket-hub        target/release (else target/debug) when built
#   Contents/Info.plist                bundle id nexus.pocket.hub
#
# The app looks for the core in Contents/Helpers first (after $POCKET_HUBD).
# The command line is shipped beside it but the app does not install it
# anywhere (no symlink, no prompt).
# Xcode's toolchain is required: the macOS SDK's SwiftUI macros and
# xcstringstool are not in the Command Line Tools. xcode-select is not changed;
# DEVELOPER_DIR is set for this script only.
set -eu

config="${1:-release}"
case "$config" in debug|release) ;; *) echo "usage: $0 [debug|release]" >&2; exit 2 ;; esac

app_root="$(cd "$(dirname "$0")/.." && pwd)"
repo_root="$(cd "$app_root/.." && pwd)"
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
out="${OUT:-$app_root/build}"
version="${VERSION:-0.1.0}"
build_number="${BUILD_NUMBER:-1}"
bundle="$out/Pocket Hub.app"

cd "$app_root"
echo "==> swift build -c $config --product PocketHub"
swift build -c "$config" --product PocketHub
bin="$(swift build -c "$config" --show-bin-path)"

echo "==> assembling $bundle"
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources" "$bundle/Contents/Helpers"
cp "$bin/PocketHub" "$bundle/Contents/MacOS/Pocket Hub"

# SwiftPM resource bundles, should a target gain resources.
for resources in "$bin"/*.bundle; do
    [ -e "$resources" ] && cp -R "$resources" "$bundle/Contents/Resources/"
done

# The String Catalog, compiled into the app's own lproj folders so SwiftUI's
# default lookup (Bundle.main) finds every key of every module.
xcrun xcstringstool compile "$app_root/Localization/Localizable.xcstrings" -o "$bundle/Contents/Resources"

cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>Pocket Hub</string>
    <key>CFBundleExecutable</key><string>Pocket Hub</string>
    <key>CFBundleIdentifier</key><string>nexus.pocket.hub</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleLocalizations</key><array><string>en</string><string>zh-Hans</string></array>
    <key>CFBundleName</key><string>Pocket Hub</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$build_number</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key><string>15.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>NSSupportsAutomaticTermination</key><false/>
    <key>NSSupportsSuddenTermination</key><false/>
</dict>
</plist>
PLIST
plutil -lint "$bundle/Contents/Info.plist" >/dev/null

# The core and the command line, when the Rust workspace has built them.
for tool in pocket-hubd pocket-hub; do
    copied=""
    for profile in release debug; do
        candidate="$repo_root/target/$profile/$tool"
        if [ -x "$candidate" ]; then
            cp "$candidate" "$bundle/Contents/Helpers/$tool"
            copied="$profile"
            break
        fi
    done
    if [ -n "$copied" ]; then
        echo "    Helpers/$tool from target/$copied"
    else
        echo "    Helpers/$tool: not built yet (cargo build -p ... at the repository root); the app will report the core missing"
    fi
done

echo "==> signing (ad hoc)"
for helper in "$bundle/Contents/Helpers/"*; do
    [ -e "$helper" ] && codesign --force --sign - --timestamp=none "$helper"
done
codesign --force --sign - --timestamp=none "$bundle"
codesign --verify --strict --verbose=1 "$bundle"
echo "==> $bundle"
