#!/usr/bin/env bash
#
# Proves a macOS build is actually installable, rather than merely built.
#
# The sibling-project incident this exists to catch: a build passed
# `codesign --verify` and still could not be opened by anyone but its author.
# So verifying the signature is not the test. This mounts the dmg the way a
# person would, copies the app out, marks it quarantined exactly as a browser
# download does, and then asks macOS the same question it asks itself at
# double-click time — plus Apple's own pre-distribution checker, which names
# the development-certificate problem outright in one command.
#
# Usage: scripts/verify-macos.sh [--dmg <path>] [--app <path>] [--require-notarized]
set -uo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
PRODUCT="Halation"
DMG=""
APP=""
REQUIRE_NOTARIZED=0

while [ $# -gt 0 ]; do
  case "$1" in
    --dmg) DMG="$2"; shift 2 ;;
    --app) APP="$2"; shift 2 ;;
    --require-notarized) REQUIRE_NOTARIZED=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Searched, not hard-coded: a --target build lands under target/<triple>/release.
[ -z "$APP" ] && APP="$(/usr/bin/find "$REPO_ROOT/target" -maxdepth 5 -path "*/release/bundle/macos/$PRODUCT.app" -print -quit 2>/dev/null)"
[ -z "$DMG" ] && DMG="$(/usr/bin/find "$REPO_ROOT/target" -maxdepth 5 -path "*/release/bundle/dmg/*.dmg" -print -quit 2>/dev/null)"

if [ -z "$APP" ] || [ ! -d "$APP" ]; then
  echo "No $PRODUCT.app found — build one first." >&2
  exit 1
fi

FAILURES=0
pass() { printf '  \033[32mPASS\033[0m  %s\n' "$1"; }
fail() { printf '  \033[31mFAIL\033[0m  %s\n' "$1"; FAILURES=$((FAILURES + 1)); }
note() { printf '  ----  %s\n' "$1"; }

echo "App: $APP"
[ -n "$DMG" ] && echo "Dmg: $DMG"

echo
echo "Signature"
if codesign --verify --deep --strict --verbose=2 "$APP" >/dev/null 2>&1; then
  pass "signature is intact and covers every nested binary"
else
  fail "codesign --verify rejected the bundle"
fi

SIGN_INFO="$(codesign --display --verbose=4 "$APP" 2>&1)"
AUTHORITY="$(printf '%s\n' "$SIGN_INFO" | awk -F'=' '/^Authority=/ {print $2; exit}')"
case "$AUTHORITY" in
  "Developer ID Application"*) pass "signed by $AUTHORITY" ;;
  "Apple Development"*)
    # The failure this whole script exists for, named exactly.
    fail "signed by a DEVELOPMENT certificate ($AUTHORITY) — Gatekeeper will refuse this"
    ;;
  *) fail "unexpected signing authority: ${AUTHORITY:-none}" ;;
esac

if printf '%s\n' "$SIGN_INFO" | grep -q "flags=.*runtime"; then
  pass "hardened runtime is enabled"
else
  fail "hardened runtime is missing — Apple rejects notarization without it"
fi

# Halation's entitlements file is deliberately EMPTY: a Tauri app is a Rust
# binary plus a system webview, and the web content runs in Apple's own
# out-of-process service, so it needs no JIT / unsigned-memory / library-
# validation exceptions. Assert that nothing has crept in — every entitlement
# is something Apple scrutinises at notarization, and one added speculatively
# is pure downside.
ENTS="$(codesign --display --entitlements - --xml "$APP" 2>/dev/null | plutil -convert xml1 -o - - 2>/dev/null)"
UNEXPECTED="$(printf '%s\n' "$ENTS" | grep -oE 'com\.apple\.security\.[a-z.-]+' | sort -u)"
if [ -z "$UNEXPECTED" ]; then
  pass "no entitlements claimed (correct for a Tauri app)"
else
  fail "unexpected entitlements — justify each before shipping:"
  printf '%s\n' "$UNEXPECTED" | sed 's/^/        /'
fi

echo
echo "Notarization"
if xcrun stapler validate "$APP" >/dev/null 2>&1; then
  pass "a notarization ticket is stapled to the app"
elif [ "$REQUIRE_NOTARIZED" = "1" ]; then
  fail "no stapled ticket on the app — a copy taken off the dmg needs the network"
else
  note "no stapled ticket on the app (required to publish)"
fi

if [ -n "$DMG" ] && [ -f "$DMG" ]; then
  if xcrun stapler validate "$DMG" >/dev/null 2>&1; then
    pass "the dmg carries its own ticket"
  elif [ "$REQUIRE_NOTARIZED" = "1" ]; then
    fail "the dmg has no ticket — Tauri signs the dmg but never submits it"
  else
    note "the dmg has no ticket"
  fi

  # Apple's own checker. One command, and it names the development-certificate
  # problem outright instead of leaving it to be discovered by a user.
  if command -v syspolicy_check >/dev/null 2>&1; then
    if syspolicy_check distribution "$DMG" 2>&1 | grep -q "passed"; then
      pass "syspolicy_check: passes Apple's pre-distribution checks"
    elif [ "$REQUIRE_NOTARIZED" = "1" ]; then
      fail "syspolicy_check rejects the dmg:"
      syspolicy_check distribution "$DMG" 2>&1 | sed -n '1,14p' | sed 's/^/        /'
    else
      note "syspolicy_check rejects the dmg (expected before notarization)"
    fi
  fi
fi

echo
echo "Install and open"
STAGE="$(mktemp -d)"
cleanup() {
  [ -n "${MOUNTED:-}" ] && hdiutil detach "$MOUNTED" -quiet >/dev/null 2>&1
  rm -rf "$STAGE" "${MOUNTED:-}"
}
trap cleanup EXIT

SOURCE_APP="$APP"
if [ -n "$DMG" ] && [ -f "$DMG" ]; then
  # Quarantined before mounting, so this is the download path and not the
  # local-file path Gatekeeper treats more kindly.
  cp "$DMG" "$STAGE/download.dmg"
  xattr -w com.apple.quarantine "0083;00000000;Safari;" "$STAGE/download.dmg" 2>/dev/null

  DMG_ASSESS="$(spctl --assess --type open --context context:primary-signature -vv "$STAGE/download.dmg" 2>&1)"
  if printf '%s\n' "$DMG_ASSESS" | grep -q "accepted"; then
    pass "Gatekeeper accepts the downloaded dmg"
  elif [ "$REQUIRE_NOTARIZED" = "1" ]; then
    fail "Gatekeeper refuses the dmg: $(printf '%s\n' "$DMG_ASSESS" | tail -1)"
  else
    note "Gatekeeper refuses the dmg: $(printf '%s\n' "$DMG_ASSESS" | tail -1)"
  fi

  MOUNTED="$(mktemp -d)"
  if hdiutil attach "$STAGE/download.dmg" -nobrowse -readonly -mountpoint "$MOUNTED" -quiet; then
    pass "the dmg mounts"
    DMG_APP="$(/usr/bin/find "$MOUNTED" -maxdepth 2 -name "$PRODUCT.app" -print -quit)"
    if [ -n "$DMG_APP" ]; then
      SOURCE_APP="$DMG_APP"
    else
      fail "no $PRODUCT.app inside the dmg"
    fi
  else
    MOUNTED=""
    fail "the dmg would not mount"
  fi
fi

cp -R "$SOURCE_APP" "$STAGE/$PRODUCT.app"
INSTALLED="$STAGE/$PRODUCT.app"
xattr -w com.apple.quarantine "0083;00000000;Safari;" "$INSTALLED" 2>/dev/null

ASSESS="$(spctl --assess --type execute --verbose=4 "$INSTALLED" 2>&1)"
if printf '%s\n' "$ASSESS" | grep -q "accepted"; then
  pass "Gatekeeper accepts a quarantined copy of the app"
  GATEKEEPER_OK=1
else
  GATEKEEPER_OK=0
  REASON="$(printf '%s\n' "$ASSESS" | tail -1)"
  if [ "$REQUIRE_NOTARIZED" = "1" ]; then
    fail "Gatekeeper rejects a quarantined copy: $REASON"
  else
    note "Gatekeeper rejects a quarantined copy: $REASON"
  fi
fi

# Being allowed to launch and actually launching are different questions: a
# bundle can pass assessment and still die on a bad entitlement. Run this one
# without the quarantine flag so it stays a launch test.
LAUNCH_COPY="$STAGE/launch/$PRODUCT.app"
mkdir -p "$STAGE/launch"
cp -R "$SOURCE_APP" "$LAUNCH_COPY"
xattr -cr "$LAUNCH_COPY" 2>/dev/null

EXECUTABLE_NAME="$(/usr/libexec/PlistBuddy -c "Print :CFBundleExecutable" \
  "$LAUNCH_COPY/Contents/Info.plist" 2>/dev/null)"
BINARY="$LAUNCH_COPY/Contents/MacOS/$EXECUTABLE_NAME"
if [ -z "$EXECUTABLE_NAME" ] || [ ! -x "$BINARY" ]; then
  fail "no executable at Contents/MacOS/${EXECUTABLE_NAME:-<unset>}"
else
  "$BINARY" >"$STAGE/run.log" 2>&1 &
  APP_PID=$!
  SETTLED=0
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    sleep 1
    if kill -0 "$APP_PID" 2>/dev/null; then SETTLED=$((SETTLED + 1)); else break; fi
  done

  if kill -0 "$APP_PID" 2>/dev/null; then
    pass "the app starts and stays running (${SETTLED}s)"
    kill "$APP_PID" 2>/dev/null
    wait "$APP_PID" 2>/dev/null
  else
    fail "the app exited on launch"
    sed -n '1,20p' "$STAGE/run.log" | sed 's/^/        /'
  fi
fi

echo
if [ "$FAILURES" -eq 0 ]; then
  if [ "${GATEKEEPER_OK:-0}" = "1" ]; then
    echo "Ready to distribute: a downloaded copy opens with no warning."
  else
    echo "Builds and runs, but is not distributable until it is notarized."
  fi
  exit 0
fi

echo "$FAILURES check(s) failed."
exit 1
