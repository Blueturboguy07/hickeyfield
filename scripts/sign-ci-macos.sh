#!/usr/bin/env bash
# Sign, notarize and package the unsigned Hickeyfield.app that ci.yml makes (artifact
# Hickeyfield-macos-arm64-app, file Hickeyfield-macos-arm64.app.zip). Nothing opens a window:
# codesign, notarytool, stapler, hdiutil and spctl only, so it is safe on a Mac someone is using.
# scripts/publik-release.sh calls it; it is the "sign the CI app" path of docs/updates.md.
#
# Usage: scripts/sign-ci-macos.sh <Hickeyfield-macos-arm64.app.zip> <out dir>
# Env:   NOTARY_PROFILE (notarytool keychain profile, default "publik"),
#        APPLE_SIGNING_IDENTITY (default: the Developer ID below),
#        TAURI_SIGNING_PRIVATE_KEY(+_PASSWORD) or the "publik-update-key" keychain item, and
#        TAURI_CLI (scripts/updater-pack-macos.sh; SKIP_UPDATER=1 skips the update bundle).
# Out:   <out dir>/hickeyfield-<version>-macos-arm64.dmg (signed, notarized, stapled; the app in it
#        too), <out dir>/Hickeyfield_<version>_aarch64.app.tar.gz + .sig (the same stapled app, for
#        the updater; see docs/updates.md).
set -euo pipefail
ZIP="${1:?unsigned app zip}"
OUT="${2:?out dir}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Mann Bellani (R5R3ZS54LV)}"
PROFILE="${NOTARY_PROFILE:-publik}"
case "$IDENTITY" in "Developer ID Application:"*) ;; *) echo "Refusing: '$IDENTITY' is not a Developer ID Application identity." >&2; exit 1;; esac
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d)"
MNT=""
trap '[ -z "$MNT" ] || hdiutil detach "$MNT" -quiet 2>/dev/null; rm -rf "$WORK"' EXIT

notarize() {
  local file="$1" log="$WORK/notary.log"
  xcrun notarytool submit "$file" --keychain-profile "$PROFILE" --wait 2>&1 | tee "$log"
  grep -q "status: Accepted" "$log" || { echo "Notarization of $(basename "$file") was not accepted." >&2; exit 1; }
}

ditto -x -k "$ZIP" "$WORK/x"
APP="$WORK/x/Hickeyfield.app"
[ -d "$APP" ] || { echo "No Hickeyfield.app in $ZIP" >&2; exit 1; }
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
MAIN="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")"
echo "==> Hickeyfield $VERSION: signing with $IDENTITY"
# Inner code first: every Mach-O file in the bundle other than the main executable (the FFmpeg
# sidecar Contents/MacOS/ffmpeg), as the Tauri bundler signs externalBin. Then the app itself.
find "$APP/Contents" -type f ! -path "$APP/Contents/MacOS/$MAIN" -print0 | while IFS= read -r -d '' f; do
  if file -b "$f" | grep -q "Mach-O"; then
    echo "    inner code: ${f#"$APP/"}"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$f"
  fi
done
# Same as the Tauri bundler: hardened runtime, the (empty) entitlements, a timestamp.
codesign --force --options runtime --timestamp --entitlements "$ROOT/src-tauri/Entitlements.plist" --sign "$IDENTITY" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

echo "==> Notarizing the app"
ditto -c -k --keepParent "$APP" "$WORK/Hickeyfield.zip"
notarize "$WORK/Hickeyfield.zip"
rm -f "$WORK/Hickeyfield.zip"
xcrun stapler staple "$APP"

# The update bundle is the stapled app itself, so an update keeps the Developer ID identity
# (and the TCC permissions bound to it).
if [ "${SKIP_UPDATER:-0}" != 1 ]; then
  echo "==> Update bundle"
  "$ROOT/scripts/updater-pack-macos.sh" "$APP" "$OUT"
fi

echo "==> Building the dmg"
STAGE="$WORK/stage"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/Hickeyfield.app"
ln -s /Applications "$STAGE/Applications"
DMG="$OUT/hickeyfield-${VERSION}-macos-arm64.dmg"
rm -f "$DMG"
hdiutil create -volname "Hickeyfield" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE" "$WORK/x"
codesign --force --sign "$IDENTITY" --timestamp "$DMG"
echo "==> Notarizing the dmg"
notarize "$DMG"
xcrun stapler staple "$DMG"

echo "==> Checks (no launch)"
xcrun stapler validate "$DMG"
spctl -a -vv -t open --context context:primary-signature "$DMG"
MNT="$WORK/mnt"
mkdir -p "$MNT"
hdiutil attach -nobrowse -noautoopen -readonly -mountpoint "$MNT" "$DMG" >/dev/null
xcrun stapler validate "$MNT/Hickeyfield.app"
spctl -a -vv -t exec "$MNT/Hickeyfield.app"
codesign -dvv "$MNT/Hickeyfield.app" 2>&1 | grep -E "Authority=Developer ID Application|flags=|TeamIdentifier"
hdiutil detach "$MNT" -quiet
MNT=""
shasum -a 256 "$DMG"
echo "Dmg: $DMG"
