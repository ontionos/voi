#!/usr/bin/env bash
# The README's pictures, made on a Mac without any window going on screen:
#
#   ./scripts/readme-pictures.sh [out-dir]
#
# Builds Voi.app, has it draw its windows into PNGs at Retina with
# `VOI_SHOT_SHOWCASE=1` (an English call, an English dictation, a meter moving
# like a voice; Settings from a demo folder, so no key and nobody's settings),
# then lays them over a generated macOS-style backdrop the way the window
# server would: the glass over a blurred, darkened backdrop with a soft shadow,
# Settings rounded with its window shadow. Needs ImageMagick (`magick`).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/target/readme-pictures}"
S="$(mktemp -d)"; trap 'rm -rf "$S"' EXIT
R="$S/macshots"; O="$OUT"; T="$S/tmp"
mkdir -p "$O" "$T" "$S/demo"

"$ROOT/scripts/bundle.sh" >/dev/null
echo '{ "sounds": false, "languages": ["en", "uk", "de", "es", "pl"] }' > "$S/demo/settings.json"
(cd "$S" && env -u SONIOX_API_KEY VOI_DATA_DIR="$S/demo" VOI_SHOT_SHOWCASE=1 \
    "$ROOT/target/Voi.app/Contents/MacOS/Voi" --screenshots "$R" >/dev/null)

# A wallpaper of soft colour, made here rather than borrowed.
magick -size 2560x1440 gradient:'#0a1440'-'#2a0b4a' \
 -fill '#ff2d78' -draw "ellipse 1650,900 820,520 0,360" \
 -fill '#ff8a1f' -draw "ellipse 2250,1250 700,460 0,360" \
 -fill '#2aa8ff' -draw "ellipse 520,380 900,560 0,360" \
 -fill '#7a3cff' -draw "ellipse 1150,250 760,420 0,360" \
 -fill '#00d1c1' -draw "ellipse 300,1300 640,420 0,360" \
 -fill '#ffd23f' -draw "ellipse 2400,350 380,300 0,360" \
 -blur 0x220 -modulate 105,125 -resize 200% "$T/bg2x.png"

glass() { # name x y  (where on the 2x backdrop)
  local n=$1 x=$2 y=$3 W H P=160
  W=$(magick identify -format %w "$R/$n.png"); H=$(magick identify -format %h "$R/$n.png")
  local CW=$((W+2*P)) CH=$((H+2*P))
  magick "$T/bg2x.png" -crop ${CW}x${CH}+$x+$y +repage "$T/$n-bg.png"
  # Everything at +P+P on that larger canvas.
  magick "$R/$n.png" -alpha extract -threshold 85% -background black -gravity center -extent ${CW}x${CH} "$T/$n-mask.png"
  magick "$R/$n-content.png" -background none -gravity center -extent ${CW}x${CH} "$T/$n-content.png"
  magick "$T/$n-bg.png" -blur 0x55 -modulate 62,85 \( -size ${CW}x${CH} xc:'rgba(18,18,22,0.38)' \) -compose over -composite "$T/$n-frost.png"
  magick "$T/$n-frost.png" "$T/$n-mask.png" -alpha off -compose CopyOpacity -composite "$T/$n-frost-cut.png"
  magick "$T/$n-mask.png" -background black -alpha shape -channel A -evaluate multiply 0.55 +channel \( +clone -blur 0x28 \) -delete 0 "$T/$n-shadow.png"
  magick "$T/$n-bg.png" \( "$T/$n-shadow.png" -geometry +0+14 \) -compose over -composite          "$T/$n-frost-cut.png" -compose over -composite          "$T/$n-content.png" -compose over -composite "$T/$n-full.png"
  local bw bh bx by
  read -r bw bh bx by < <(magick "$T/$n-mask.png" -trim -format '%w %h %[fx:page.x] %[fx:page.y]' info:) || true
  local m=88
  magick "$T/$n-full.png" -crop $((bw+2*m))x$((bh+2*m))+$((bx-m))+$((by-m)) +repage "$O/$n.png"
}
window() { # name x y
  local n=$1 x=$2 y=$3 W H m=140
  W=$(magick identify -format %w "$R/$n.png"); H=$(magick identify -format %h "$R/$n.png")
  magick "$T/bg2x.png" -crop $((W+2*m))x$((H+2*m))+$x+$y +repage "$T/$n-bg.png"
  magick -size ${W}x${H} xc:black -fill white -draw "roundrectangle 0,0,$((W-1)),$((H-1)),30,30" "$T/$n-round.png"
  magick "$R/$n.png" "$T/$n-round.png" -alpha off -compose CopyOpacity -composite "$T/$n-rounded.png"
  magick "$T/$n-rounded.png" \( +clone -background black -shadow 60x34+0+26 \) +swap -background none -layers merge +repage "$T/$n-shadowed.png"
  magick "$T/$n-bg.png" "$T/$n-shadowed.png" -gravity center -geometry +0+0 -compose over -composite "$O/$n.png"
}
glass hud-open 2700 60
glass dictation-listening 1300 1450
window settings-general 300 200
window settings-dictation 2600 1100
for f in hud-open dictation-listening settings-general settings-dictation; do magick "$O/$f.png" -depth 8 "$O/$f.png"; done
echo "pictures in $O: hud-open (calls), dictation-listening (typing), settings-general, settings-dictation"
