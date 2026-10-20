#!/usr/bin/env bash
# Build Voi.app and install it to /Applications.
#
# The bundle itself is `bundle.sh`; this is what to do with it on this Mac —
# the key, the copy into /Applications, and the CLI on the PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${VOI_DEST:-/Applications}/Voi.app"
SUPPORT="$HOME/Library/Application Support/voi"

# shellcheck source=bundle.sh
source "$ROOT/scripts/bundle.sh"

echo "▸ storing config"
mkdir -p "$SUPPORT"
if [ ! -f "$SUPPORT/config.env" ]; then
    if [ -f "$ROOT/.env" ] && grep -q '^SONIOX_API_KEY=' "$ROOT/.env"; then
        (umask 077; grep '^SONIOX_API_KEY=' "$ROOT/.env" > "$SUPPORT/config.env")
        chmod 600 "$SUPPORT/config.env"
        echo "  copied Soniox key from .env"
    else
        (umask 077; echo 'SONIOX_API_KEY=' > "$SUPPORT/config.env")
        echo "  ⚠ no key found — put it in $SUPPORT/config.env"
    fi
else
    echo "  keeping existing $SUPPORT/config.env"
fi

echo "▸ installing to $DEST"
# Quit a running copy first; the bundle cannot be replaced while in use.
pkill -x Voi 2>/dev/null && sleep 1 || true
rm -rf "$DEST"
mkdir -p "$(dirname "$DEST")"
cp -R "$STAGE" "$DEST"

# Make the CLI reachable from a shell, best-effort.
for d in /usr/local/bin "$HOME/.local/bin"; do
    if [ -d "$d" ] && [ -w "$d" ]; then
        ln -sf "$DEST/Contents/MacOS/voi-cli" "$d/voi"
        echo "▸ linked CLI at $d/voi"
        break
    fi
done

echo
echo "Installed. Launch it with:  open -a Voi"
echo "The overlay sits in the top-right corner: click the red dot to record,"
echo "hover for the live transcript, right-click its header for the sessions folder."
echo "Hold the right ⌘ anywhere to dictate into whatever you are typing in;"
echo "put the pointer on that window to throw one away instead."
echo "macOS asks for Microphone, System Audio Recording and Accessibility."
