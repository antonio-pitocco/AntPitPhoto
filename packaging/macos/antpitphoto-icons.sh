#!/usr/bin/env bash
# AntPitPhoto: regenerate the app icon files from assets/app-icon/antpitphoto/antpitphoto.svg.
# Needs resvg (brew install resvg) and, for the .icns, macOS iconutil. The outputs are committed.
#
#   packaging/macos/antpitphoto-icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIR="$ROOT/assets/app-icon/antpitphoto"
SVG="$DIR/antpitphoto.svg"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
command -v resvg >/dev/null || { echo "error: resvg not found (brew install resvg)" >&2; exit 1; }

# Same grid as packaging/icons.sh: the SVG is the full-bleed 512 tile; macOS wants an 824/1024 body.
MAC="$TMP/macos.svg"
sed 's/viewBox="0 0 512 512"/viewBox="-62 -62 636 636"/' "$SVG" >"$MAC"
grep -q 'viewBox="-62 -62 636 636"' "$MAC" || { echo "error: unexpected viewBox in $SVG" >&2; exit 1; }
render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }

render "$SVG" 1024 "$DIR/antpitphoto-1024.png"
render "$MAC" 512 "$DIR/antpitphoto-macos-512.png"   # runtime Dock icon on macOS
render "$SVG" 256 "$DIR/antpitphoto-256.png"         # runtime window icon on Windows and Linux

SET="$TMP/antpitphoto.iconset"
mkdir -p "$SET"
for s in 16 32 128 256 512; do
  render "$MAC" "$s" "$SET/icon_${s}x${s}.png"
  render "$MAC" $((s * 2)) "$SET/icon_${s}x${s}@2x.png"
done
iconutil -c icns -o "$DIR/antpitphoto.icns" "$SET"
echo "icons written to $DIR"
