#!/usr/bin/env bash
# AntPitPhoto (AntPitLab's build of LightCraft): native for this Mac only (Apple silicon), ad-hoc
# signed (the app is for this machine; other Macs' Gatekeeper refuses it), installed in
# /Applications/AntPitPhoto.app with ditto, plus `antpitphoto-cli` in ~/bin. Bundle id
# it.antpitlab.antpitphoto, its own icon (assets/app-icon/antpitphoto/). The interface starts in
# Italian (LSEnvironment LIGHTCRAFT_LANGUAGE=it; Edit > Language still changes it).
# See docs/antpitphoto.md.
#
#   CARGO_TARGET_DIR=/path/with/space/lightcraft-target packaging/macos/antpitphoto-local.sh [--skip-build] [--no-install]
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"
SKIP_BUILD=0
INSTALL=1
for a in "$@"; do
  case "$a" in
    --skip-build) SKIP_BUILD=1 ;;
    --no-install) INSTALL=0 ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done
export MACOSX_DEPLOYMENT_TARGET=11.0
SHORT_VERSION="${VERSION%%-*}"
WORK="$CARGO_TARGET_DIR/antpitlab-package"
NAME=AntPitPhoto
BUNDLE_ID=it.antpitlab.antpitphoto
APP="$WORK/$NAME.app"

if [ "$SKIP_BUILD" = 0 ]; then
  (cd "$ROOT" && cargo build --release --locked -p lightcraft -p lightcraft-cli --features lightcraft/heif,lightcraft-cli/heif)
fi

rm -rf "$WORK"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Licenses"
cp "$CARGO_TARGET_DIR/release/lightcraft" "$APP/Contents/MacOS/$NAME"
cp "$ROOT/assets/app-icon/antpitphoto/antpitphoto.icns" "$APP/Contents/Resources/$NAME.icns"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/${LIGHTCRAFT_BUILD_SHA:-unknown}/g" "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy \
  -c "Set :CFBundleDisplayName $NAME" -c "Set :CFBundleName $NAME" -c "Set :CFBundleExecutable $NAME" \
  -c "Set :CFBundleIdentifier $BUNDLE_ID" -c "Set :CFBundleIconFile $NAME" \
  -c "Set :UTExportedTypeDeclarations:0:UTTypeIdentifier $BUNDLE_ID.document" \
  -c "Set :UTExportedTypeDeclarations:0:UTTypeIconFile $NAME" \
  -c "Set :UTImportedTypeDeclarations:1:UTTypeIdentifier $BUNDLE_ID.qoi" \
  -c "Set :NSAppleEventsUsageDescription $NAME asks Photos to import the photos you export when you choose Add to Apple Photos in the Export dialog." \
  -c "Set :NSHumanReadableCopyright AntPitPhoto by AntPitLab, based on LightCraft (github.com/storytold/lightcraft). Copyright © the LightCraft authors. MIT OR Apache-2.0." \
  -c "Add :LSEnvironment dict" -c "Add :LSEnvironment:LIGHTCRAFT_LANGUAGE string it" "$APP/Contents/Info.plist"
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")" = "$BUNDLE_ID" ]
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"
copy_docs "$APP/Contents/Resources/Licenses"
cp "$ROOT/docs/antpitphoto.md" "$APP/Contents/Resources/Licenses/"
xattr -cr "$APP"

codesign --force --sign - --timestamp=none --options runtime --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/$NAME"
codesign --force --sign - --timestamp=none --options runtime --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

CLI="$WORK/antpitphoto-cli"
cp "$CARGO_TARGET_DIR/release/lightcraft-cli" "$CLI"
codesign --force --sign - --timestamp=none --options runtime "$CLI"
"$CLI" --version

if [ "$INSTALL" = 1 ]; then
  if pgrep -f "/Applications/$NAME.app/" >/dev/null; then
    echo "error: $NAME is running; quit it first" >&2
    exit 1
  fi
  rm -rf "/Applications/$NAME.app"
  ditto "$APP" "/Applications/$NAME.app"
  codesign --verify --strict --deep "/Applications/$NAME.app"
  # the earlier AntPitLab build was installed as LightCraft.app: AntPitPhoto takes its place
  # (only that build, recognisable by its LSEnvironment, and never while it is open)
  if [ -d /Applications/LightCraft.app ] && [ "$(defaults read /Applications/LightCraft.app/Contents/Info LSEnvironment 2>/dev/null | grep -c LIGHTCRAFT_LANGUAGE)" = 1 ]; then
    if pgrep -f "/Applications/LightCraft.app/" >/dev/null; then
      echo "note: /Applications/LightCraft.app is open: left in place, remove it once it is closed"
    else
      rm -rf /Applications/LightCraft.app
    fi
  fi
  DEST="$HOME/.local/share/antpitphoto-cli/$VERSION"
  mkdir -p "$DEST" "$HOME/bin"
  ditto "$CLI" "$DEST/antpitphoto-cli"
  ln -sfn "$DEST/antpitphoto-cli" "$HOME/bin/antpitphoto-cli"
  echo "installed /Applications/$NAME.app and $HOME/bin/antpitphoto-cli -> $DEST/antpitphoto-cli"
fi
