#!/usr/bin/env bash
# Build Voi.app into target/, signed and ready to run.
#
# Everything both `install.sh` and `dmg.sh` need, and nothing either of them
# does afterwards. Sourced rather than run, so the caller gets $STAGE back.
#
# macOS grants microphone and system-audio permission per *code-signed bundle*,
# keyed on Info.plist usage strings. A bare binary has neither, so TCC cannot
# prompt and silently denies — capture appears to run and records pure silence.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE="$ROOT/target/Voi.app"

# Who this build is, in one place.
#
# The marketing version comes from Cargo.toml, so there is one number to bump
# and no way for the bundle and the crates to disagree about it. The build
# number is the commit count — it only ever goes up, which is what makes
# "newer" answerable — and the commit itself is recorded beside it, because
# the version alone cannot tell two builds of the same version apart, and
# almost every build is one of those.
VERSION=$(awk -F'"' '/^version = /{print $2; exit}' "$ROOT/Cargo.toml")
# The build number: commits since config/build-base, which carries the count
# of the history before this one, so builds only ever go up.
BUILD=$(( $(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 0) + $(tr -d '[:space:]' < "$ROOT/config/build-base") ))
COMMIT=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)
# A build made from uncommitted work is not the commit it claims to be.
git -C "$ROOT" diff --quiet HEAD 2>/dev/null || COMMIT="$COMMIT+"
BUILT=$(date -u +%Y-%m-%d)
echo "▸ Voi $VERSION (build $BUILD, $COMMIT)"

echo "▸ building"
# The compiler writes the paths it built from into the binary — this
# machine's home folder, and so the user's name, among them. A published
# build names neutral ones instead. (The last match wins, so the home folder
# comes first and the paths inside it after.)
REMAP="--remap-path-prefix=$HOME=/home --remap-path-prefix=$HOME/.cargo=/cargo"
REMAP="$REMAP --remap-path-prefix=$HOME/.rustup=/rustup --remap-path-prefix=$ROOT=/voi"
(cd "$ROOT" && VOI_BUILD="$BUILD" RUSTFLAGS="${RUSTFLAGS:-} $REMAP" cargo build --release)

APP_BIN="$ROOT/target/release/voi-app"
CLI_BIN="$ROOT/target/release/voi"
[ -x "$APP_BIN" ] && [ -x "$CLI_BIN" ] || { echo "build produced no binaries" >&2; exit 1; }

echo "▸ assembling bundle"
rm -rf "$STAGE"
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources"

cat > "$STAGE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>              <string>Voi</string>
    <key>CFBundleDisplayName</key>       <string>Voi</string>
    <key>CFBundleIdentifier</key>        <string>is.mana.voi</string>
    <key>CFBundleExecutable</key>        <string>Voi</string>
    <key>CFBundleIconFile</key>          <string>Voi</string>
    <key>CFBundlePackageType</key>       <string>APPL</string>
    <!-- Short string is what a person reads; CFBundleVersion is what
         "is this newer" is decided on, so it counts rather than describes. -->
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key>           <string>$BUILD</string>
    <key>VoiCommit</key>                 <string>$COMMIT</string>
    <key>VoiBuiltOn</key>                <string>$BUILT</string>
    <key>LSMinimumSystemVersion</key>    <string>14.4</string>
    <!-- Menu-bar only: no Dock icon, no app menu. -->
    <key>LSUIElement</key>               <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Voi transcribes your side of a call.</string>
    <key>NSAudioCaptureUsageDescription</key>
    <string>Voi transcribes what the other participants say.</string>
</dict>
PLIST
echo '</plist>' >> "$STAGE/Contents/Info.plist"

# Both are copied under names that differ by more than case — see the note in
# apps/voi-app/Cargo.toml.
cp "$APP_BIN" "$STAGE/Contents/MacOS/Voi"
cp "$CLI_BIN" "$STAGE/Contents/MacOS/voi-cli"

echo "▸ sounds"
# The two notes dictation makes. In the bundle rather than beside the source,
# because NSSound looks for them in Resources and an app handed to somebody
# else has no source tree to look in.
cp "$ROOT/assets/sounds/start.mp3" "$ROOT/assets/sounds/end.mp3" "$STAGE/Contents/Resources/"

echo "▸ icon"
# Generated, not committed: the shape lives in the script's numbers.
python3 "$ROOT/scripts/make_icon.py" "$ROOT/target/Voi.iconset" >/dev/null
cp "$ROOT/target/Voi.icns" "$STAGE/Contents/Resources/Voi.icns"

echo "▸ signing"
# With a real identity if this Mac has one, ad-hoc otherwise.
#
# Not about Gatekeeper — a development certificate does not satisfy that
# either. It is about permissions surviving a rebuild. macOS remembers
# Microphone, Screen Recording and especially Accessibility against the app's
# *signature*, and an ad-hoc signature is a hash of the binary: every rebuild
# is a different app as far as the system is concerned, so every rebuild starts
# from nothing while the old entry sits in the list looking enabled. Signed with
# a certificate, the identity stays the same and so does the permission.
IDENTITY="${VOI_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(Developer ID Application[^"]*\)".*/\1/p' | head -1)}"
if [ -z "$IDENTITY" ]; then
    IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null \
        | sed -n 's/.*"\(Apple Development[^"]*\)".*/\1/p' | head -1)
fi
if [ -n "$IDENTITY" ]; then
    echo "  as $IDENTITY"
    # No hardened runtime: it enforces entitlements, and a hardened app
    # without `com.apple.security.device.audio-input` is denied the microphone
    # outright. It is required for notarising, and that is when to add the
    # entitlements to go with it — together, or not at all.
    #
    # A secure timestamp, though, yes. A signature otherwise carries only the
    # signing Mac's own clock reading, which proves nothing, so when the
    # certificate expires there is no evidence the signing happened while it
    # was valid and the signature expires with it. `--timestamp` has Apple's
    # timestamp server countersign the moment instead, and the signature
    # outlives the certificate. It costs one round trip and works with a
    # free-account certificate.
    if ! codesign --force --sign "$IDENTITY" --timestamp "$STAGE" 2>/dev/null; then
        echo "  ⚠ no timestamp server — signing without one; this build's"
        echo "    signature stops verifying when the certificate expires"
        codesign --force --sign "$IDENTITY" --timestamp=none "$STAGE"
    fi
else
    echo "  ad-hoc — permissions will need granting again after each rebuild"
    codesign --force --sign - --timestamp=none "$STAGE"
fi
