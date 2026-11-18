#!/bin/sh
# Build phase of the PocketHub target (PocketHub.xcodeproj), after Xcode has
# embedded Sparkle.framework, the package the app updates itself with
# (AppUpdater.swift): signs the programs inside the framework with the app's
# identity, then the framework.
#
# Xcode signs a framework it embeds but not the programs inside it: Autoupdate
# and Updater.app keep the ad hoc signature they come with (measured: after a
# build their code directories are the package's own). Notarization accepts
# an executable only signed with the Developer ID, the hardened runtime and a
# secure timestamp, so they are signed here, as embed-core.sh signs the
# helpers, inside out as Sparkle's documentation signs it by hand
# ("Sandboxing with Sparkle"): a signature seals what is inside it, so the
# framework comes last, and Xcode signs the app after this phase. Nothing of
# the signatures they come with is kept, as there: Autoupdate's carries a
# com.apple.application-identifier entitlement (measured), which is no part
# of a Developer ID signature without a provisioning profile. A development
# build is signed ad hoc, as the rest of it is.
#
# The phase names the framework's seal (_CodeSignature/CodeResources) as its
# output: without it, an incremental build ran this phase and left the app's
# own signature as it was, sealing the framework before this phase signed it
# again (measured: "nested code is modified or invalid").
#
# The framework's XPC services are taken out first. They are for an app in
# the sandbox (Sparkle, "Removing XPC Services"); Pocket Hub is not in one,
# and Sparkle starts Autoupdate itself (SUEnableInstallerLauncherService is
# not set). Out, they are no executables to sign.
set -eu

framework="$TARGET_BUILD_DIR/$FRAMEWORKS_FOLDER_PATH/Sparkle.framework"
if [ ! -d "$framework/Versions/B" ]; then
    echo "error: $framework/Versions/B is missing: this phase runs after Xcode embeds Sparkle" >&2
    exit 1
fi
rm -rf "$framework/Versions/B/XPCServices"

if [ "${CODE_SIGNING_ALLOWED:-NO}" != YES ]; then
    exit 0
fi
# The hardened runtime and the timestamp as embed-core.sh decides them.
runtime=""
if [ "${ENABLE_HARDENED_RUNTIME:-NO}" = YES ]; then
    runtime="--options=runtime"
fi
timestamp="--timestamp"
if [ "$EXPANDED_CODE_SIGN_IDENTITY" = "-" ]; then
    timestamp="--timestamp=none"
fi
for code in "$framework/Versions/B/Autoupdate" "$framework/Versions/B/Updater.app" "$framework/Versions/B"; do
    codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" $runtime "$timestamp" "$code"
done
