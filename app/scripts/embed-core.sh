#!/bin/sh
# Build phase of the PocketHub target (PocketHub.xcodeproj): builds the core
# and the command line with cargo, in the profile matching the configuration
# (Debug: dev, Release: release), and puts them in Contents/Helpers, where
# the app looks for the core first (HubClient's CoreLauncher).
#
# cargo runs with a clean environment: Xcode exports SDKROOT,
# MACOSX_DEPLOYMENT_TARGET and the like, which build scripts watch, and the
# shared target/ would otherwise be rebuilt each time a build alternates
# between Xcode and a terminal. cargo is where rustup puts it.
#
# The helpers are signed here with the app's identity: Xcode signs the bundle
# after this phase but does not sign executables a script copies into it.
# The target has user script sandboxing off so this can run cargo on the
# repository.
set -eu

case "$CONFIGURATION" in
    Debug) profile=debug; release_flag="" ;;
    Release) profile=release; release_flag="--release" ;;
    *) echo "error: no cargo profile for the configuration $CONFIGURATION" >&2; exit 1 ;;
esac

repo_root="$(cd "$SRCROOT/.." && pwd)"
cargo="$HOME/.cargo/bin/cargo"
if [ ! -x "$cargo" ]; then
    echo "error: $cargo not found: install Rust with rustup (https://rustup.rs)" >&2
    exit 1
fi

cd "$repo_root"
env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    "$cargo" build $release_flag -p pocket-hubd -p pocket-hub-cli

helpers="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Helpers"
mkdir -p "$helpers"
for tool in pocket-hubd pocket-hub; do
    cp "$repo_root/target/$profile/$tool" "$helpers/$tool"
    if [ "${CODE_SIGNING_ALLOWED:-NO}" = YES ]; then
        codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" --timestamp=none "$helpers/$tool"
    fi
done
