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
# after this phase but does not sign executables a script copies into it. They
# are signed as Xcode signs the app: with the hardened runtime when the target
# has it (ENABLE_HARDENED_RUNTIME, which Xcode turns off for an ad hoc
# signature: a development build has none), and with a secure timestamp when the
# identity is a certificate rather than ad hoc ("-"). Notarization accepts an
# executable only with both (package.sh signs a release with the Developer ID).
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

# cargo built the helpers for this Mac; Xcode builds the app for $ARCHS. They
# have to be the same. A Release build used to be a universal app around
# arm64 helpers: on an Intel Mac it would open and then fail to start its
# core. The app is built for arm64 alone now (ARCHS of the Release
# configuration), and a build whose two halves differ stops here.
for tool in pocket-hubd pocket-hub; do
    built="$(lipo -archs "$repo_root/target/$profile/$tool")"
    if [ "$built" != "$ARCHS" ]; then
        echo "error: $tool is built for $built and the app for $ARCHS: build Pocket Hub on a Mac of the app's architecture" >&2
        exit 1
    fi
done

helpers="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Helpers"
mkdir -p "$helpers"
for tool in pocket-hubd pocket-hub; do
    cp "$repo_root/target/$profile/$tool" "$helpers/$tool"
    if [ "${CODE_SIGNING_ALLOWED:-NO}" = YES ]; then
        runtime=""
        if [ "${ENABLE_HARDENED_RUNTIME:-NO}" = YES ]; then
            runtime="--options=runtime"
        fi
        timestamp="--timestamp"
        if [ "$EXPANDED_CODE_SIGN_IDENTITY" = "-" ]; then
            timestamp="--timestamp=none"
        fi
        codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" $runtime "$timestamp" "$helpers/$tool"
    fi
done
