#!/usr/bin/env bash
# Package Voi.app as a disk image to hand to somebody.
#
# The usual shape: a small window with the app on the left, a shortcut to
# /Applications on the right, and an arrow between them. Dragging one onto the
# other is the whole installation.
#
# What is *not* in it: the Soniox key. It lives in the Application Support
# folder, never in the bundle — which is the point of keeping it there. Whoever
# gets this types their own into Settings on first run.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/target/Voi.dmg}"
BG="$ROOT/target/dmg-background.tiff"

# dmg-window.py imports the drawing next door for its numbers; without this
# Python leaves a __pycache__ in scripts/ for the privilege.
export PYTHONDONTWRITEBYTECODE=1

# shellcheck source=bundle.sh
source "$ROOT/scripts/bundle.sh"

echo "▸ background"
python3 "$ROOT/scripts/make_dmg_bg.py" "$BG" >/dev/null

# The window's arrangement is a .DS_Store — a Finder file, normally written by
# opening the image and dragging the icons where you want them. dmgbuild writes
# it directly instead, which is both repeatable and silent. It is a build tool,
# not a dependency of the app, so it lives in a virtualenv under target/ and is
# installed once.
TOOLS="$ROOT/target/dmg-tools"
if [ ! -x "$TOOLS/bin/dmgbuild" ]; then
    echo "▸ fetching dmgbuild"
    python3 -m venv "$TOOLS" >/dev/null 2>&1 || true
    [ -x "$TOOLS/bin/pip" ] && "$TOOLS/bin/pip" install -q dmgbuild >/dev/null 2>&1 || true
fi

rm -f "$OUT"

if [ -x "$TOOLS/bin/dmgbuild" ]; then
    echo "▸ making $OUT"
    "$TOOLS/bin/dmgbuild" \
        -s "$ROOT/scripts/dmg-window.py" \
        -D app="$STAGE" \
        -D background="$BG" \
        -D icon="$ROOT/target/Voi.icns" \
        -D here="$ROOT/scripts" \
        "Voi" "$OUT"
else
    # No network, or no venv: still produce something installable, just plain.
    echo "▸ making $OUT (unstyled — dmgbuild unavailable)"
    ROOM="$(mktemp -d)"
    trap 'rm -rf "$ROOM"' EXIT
    cp -R "$STAGE" "$ROOM/Voi.app"
    ln -s /Applications "$ROOM/Applications"
    hdiutil create -volname "Voi" -srcfolder "$ROOM" -ov -quiet -format UDZO "$OUT"
fi

# The signature has to survive being copied into the image, or the app is
# refused on arrival for a reason nobody can see. Checked against the copy
# inside, not the one we built. -nobrowse so nothing opens on screen.
echo "▸ checking the copy inside"
MOUNT="$(mktemp -d)"
hdiutil attach "$OUT" -nobrowse -quiet -mountpoint "$MOUNT"
codesign --verify --deep --strict "$MOUNT/Voi.app" \
    && echo "  signature intact" \
    || echo "  ⚠ the app inside the image is not properly signed"
# And what is inside carries no Soniox key: the one this Mac uses, read from
# its config now, or anything shaped like one. On a hit the image is deleted,
# so there is nothing left to hand over by mistake.
echo "▸ scanning the copy inside for a key"
cargo build --release -q -p voi-scan --manifest-path "$ROOT/Cargo.toml"
if ! "$ROOT/target/release/voi-scan" "$MOUNT/Voi.app"; then
    hdiutil detach "$MOUNT" -quiet
    rmdir "$MOUNT"
    rm -f "$OUT"
    echo "  ✗ a key is in the build — image deleted" >&2
    exit 1
fi
hdiutil detach "$MOUNT" -quiet
rmdir "$MOUNT"

SIZE=$(du -h "$OUT" | cut -f1)
echo
echo "Built $OUT ($SIZE) — Voi $VERSION, build $BUILD, $COMMIT."
echo
echo "On another Mac: open it, drag Voi onto Applications, launch it."
echo "The first launch is refused — this is signed for development, not"
echo "notarised by Apple. System Settings ▸ Privacy & Security has an"
echo "\"Open Anyway\" for it; after that it opens like anything else."
echo
echo "It then asks for Microphone, System Audio Recording and Accessibility,"
echo "and needs a Soniox API key pasted into Settings."
