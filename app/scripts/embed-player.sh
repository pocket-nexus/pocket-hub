#!/bin/sh
# Build phase of the PocketHub target (PocketHub.xcodeproj): copies the
# applet store's player into Contents/Resources/Player, where Try It loads it
# (AppletPlayer.swift). The player is pocket-nexus/applets'
# store/site/public/play as built there (`bun store/build-player.ts`): the
# frame's page, the player and PocketJS's core in WebAssembly, from the
# submodule vendor/applets at the commit this repository pins. A change to
# the player lands in pocket-nexus/applets first and this repository moves
# its pin.
set -eu

player="$SRCROOT/../vendor/applets/store/site/public/play"
for file in index.html player.js pocketjs.wasm; do
    if [ ! -f "$player/$file" ]; then
        echo "error: $player/$file is missing: check out the submodules (git submodule update --init vendor/applets)" >&2
        exit 1
    fi
done

out="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/Player"
mkdir -p "$out"
cp "$player/index.html" "$player/player.js" "$player/pocketjs.wasm" "$out/"
