#!/usr/bin/env bash
# Live update proof on a macOS GitHub runner (.github/workflows/live-update-proof.yml).
# Never run it on a desk Mac: it writes TCC grants, installs into /Applications and starts
# Hickeyfield. Copied from GSpot's proof (Blueturboguy07/gspot, tests/live-proof/proof-macos.sh).
#
# Usage: tests/live-proof/proof-macos.sh <from_tag> <expect_version> <out dir>
#
# 1. Downloads the dmg of release <from_tag> from GitHub (the asset publik's listing links), marks
#    it quarantined as a browser does, checks that Gatekeeper accepts it, and installs the app the
#    way a user does (Finder copies it from the mounted dmg to /Applications).
# 2. Records the app's designated requirement and writes real TCC grants for its bundle id:
#    camera and microphone in the user database, screen capture and accessibility in the system
#    database, each with the csreq made from that requirement.
# 3. Starts the app with `open` (no test hook, no feed override: the release asks the live feed
#    https://publikhq.com/api/updates/hickeyfield/latest.json). Sign-in uses a local stand-in for
#    publik's API (fake-publik.py, "unclaimed"), so nobody signs in, the app stays on its
#    "Sign in with publik" question with its main window hidden, the camera, the microphone and
#    screen capture stay unused, and publik's database is not touched.
# 4. <expect_version> = the version of <from_tag>: the app must find no update and stay as it is.
#    Higher: the app must download and install it by itself and ask; the proof answers "Restart"
#    (then the app must run the new version), installs <from_tag> again and answers "Later" (the
#    app keeps running; after a quit and reopen it runs the new version).
# 5. "Check for Updates…" in the app menu is clicked (System Events) and the app's answer is read.
# 6. After it: the designated requirement is unchanged, Gatekeeper accepts the app, and every TCC
#    row is still there with its csreq satisfied by the installed app (the check tccd makes).
# shellcheck disable=SC2024 # sudo runs the reader; its output goes to the runner user's files
set -uo pipefail
FROM_TAG="${1:?from_tag}"
EXPECT="${2:?expect_version}"
OUT="${3:?out dir}"
FROM="${FROM_TAG#v}"
EXPECT="${EXPECT#v}"
if ! [[ "$FROM" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$EXPECT" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "from_tag must be vX.Y.Z and expect_version X.Y.Z" >&2
  exit 2
fi
REPO="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY}"
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d)"
APP=/Applications/Hickeyfield.app
BUNDLE_ID=ai.hickeyfield.studio
EXE=hickeyfield-tauri
# pgrep -x cannot find the app: macOS cuts a process name to 16 characters ("hickeyfield-taur").
# So the executable path is matched at the start of the command line (which a shell running
# pgrep never has).
PROC_RE='^/.*/Hickeyfield\.app/Contents/MacOS/hickeyfield-tauri( |$)'
FEED=https://publikhq.com/api/updates/hickeyfield/latest.json
DMG="$WORK/dl/hickeyfield-${FROM}-macos-arm64.dmg"
MNT=/Volumes/Hickeyfield-proof
PORT=47333
MODE=update
[ "$FROM" = "$EXPECT" ] && MODE=no-update
USER_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
SYS_DB="/Library/Application Support/com.apple.TCC/TCC.db"
SERVICES=(kTCCServiceCamera kTCCServiceMicrophone kTCCServiceScreenCapture kTCCServiceAccessibility)

PASS=0
FAILS=0
RESULTS="$OUT/results.md"
printf '| Check | Result | Detail |\n|---|---|---|\n' > "$RESULTS"
row() { printf '| %s | %s | %s |\n' "$1" "$2" "$(printf '%s' "${3:-}" | tr '|\n' '/ ' | cut -c1-300)" >> "$RESULTS"; }
pass() { PASS=$((PASS + 1)); echo "PASS  $1 ${2:+($2)}"; row "$1" PASS "${2:-}"; }
fail() { FAILS=$((FAILS + 1)); echo "FAIL  $1 ${2:+($2)}"; row "$1" "**FAIL**" "${2:-}"; }
note() { echo "NOTE  $1 ${2:+($2)}"; row "$1" note "${2:-}"; }
check() { local name="$1" detail="$2"; shift 2; if "$@"; then pass "$name" "$detail"; else fail "$name" "$detail"; fi; }
log() { echo "[$(date -u +%H:%M:%S)] $*"; }
SHOTS=0
shot() { SHOTS=$((SHOTS + 1)); screencapture -x "$OUT/$(printf '%02d' "$SHOTS")-$1.png" 2>/dev/null || true; alerts_dump > "$OUT/$(printf '%02d' "$SHOTS")-$1.alerts.txt" 2>&1; }
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null; }
cdhash() { codesign -dvvv "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
dr() { codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => //p'; }
app_pid() { pgrep -f "$PROC_RE" | head -1; }
app_running() { pgrep -f "$PROC_RE" >/dev/null; }
gatekeeper_ok() { local a; a="$(spctl -a -vv -t "$1" ${2:+--context "$2"} "$3" 2>&1)"; echo "$a" >> "$OUT/spctl.txt"; echo "$a" | grep -q ": accepted" && echo "$a" | grep -q "source=Notarized Developer ID"; }
# Runs a command for at most $1 seconds (124 on a timeout).
limit() {
  local t="$1" p i
  shift
  "$@" & p=$!
  for ((i = 0; i < t; i++)); do kill -0 "$p" 2>/dev/null || { wait "$p"; return $?; }; sleep 1; done
  kill "$p" 2>/dev/null
  return 124
}
# Waits up to $1 tries (one a second) for a command to succeed.
wait_for() {
  local t="$1" i
  shift
  for ((i = 0; i < t; i++)); do "$@" && return 0; sleep 1; done
  return 1
}
# System Events (the runner lets osascript use accessibility to read and press another app's buttons).
ui() { limit 30 osascript -e "tell application \"System Events\" to $1" 2>> "$OUT/osascript.log"; }
windows_of() { ui "if exists process \"$1\" then count windows of process \"$1\"" | tr -d '[:space:]'; }
has_window() { [ "$(windows_of "$1")" -gt 0 ] 2>/dev/null; }
texts_of() { ui "tell process \"$1\" to get value of every static text of window 1"; }
press() { ui "tell process \"$1\" to click button \"$2\" of window 1" >/dev/null; }
# The app's questions are UserNotificationCenter windows (ui.applescript has the details).
UI_SCRIPT="$HERE/ui.applescript"
alerts_dump() { limit 60 osascript "$UI_SCRIPT" dump 2>> "$OUT/osascript.log"; }
alert_find() { limit 60 osascript "$UI_SCRIPT" find "$1" 2>> "$OUT/osascript.log"; }
alert_press() { limit 60 osascript "$UI_SCRIPT" press "$1" "$2" 2>> "$OUT/osascript.log"; }
has_alert() { local t; t="$(alert_find "$1")" && [ -n "$t" ]; }
# True only when System Events could look and found no such window.
no_alert() { local t; t="$(alert_find "$1")" && [ -z "$t" ]; }
# Three tries: System Events sometimes answers "not allowed assistive access" for a moment after an alert closes.
click_check_for_updates() {
  local r i
  for i in 1 2 3; do
    r="$(limit 60 osascript "$UI_SCRIPT" menu "$BUNDLE_ID" "Check for Updates" 2>> "$OUT/osascript.log")"
    if [ -n "$r" ]; then printf '%s\n' "$r"; return 0; fi
    sleep 5
  done
}
log_has() { grep -qF "$2" "$1" 2>/dev/null; }
count_in() { grep -cF "$2" "$1" 2>/dev/null || true; }

log "Hickeyfield live update proof: install $FROM_TAG, expect $EXPECT ($MODE) on $(sw_vers -productVersion) $(uname -m)"
echo "from=$FROM_TAG expect=$EXPECT mode=$MODE macos=$(sw_vers -productVersion)" > "$OUT/run.txt"
curl -fsS "$FEED" -o "$OUT/feed-at-start.json"
FEED_V="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$OUT/feed-at-start.json" 2>/dev/null)"
check "the live feed announces $EXPECT" "$FEED version $FEED_V" test "$FEED_V" = "$EXPECT"

# ---- 1. Download, Gatekeeper, install ---------------------------------------------------------
mkdir -p "$WORK/dl"
URL="https://github.com/$REPO/releases/download/$FROM_TAG/$(basename "$DMG")"
log "download $URL"
curl -fsSL "$URL" -o "$DMG"
curl -fsSL "https://github.com/$REPO/releases/download/$FROM_TAG/SHA256SUMS.txt" -o "$WORK/dl/SHA256SUMS.txt"
check "the dmg is the release's (SHA256SUMS.txt)" "$(shasum -a 256 "$DMG" | cut -c1-16)…" \
  grep -q "^$(shasum -a 256 "$DMG" | cut -d' ' -f1)  $(basename "$DMG")\$" "$WORK/dl/SHA256SUMS.txt"
# The quarantine flag a browser sets on a download.
xattr -w com.apple.quarantine "0083;$(printf '%x' "$(date +%s)");Safari;$(uuidgen)" "$DMG"
check "quarantine flag on the downloaded dmg" "$(xattr -p com.apple.quarantine "$DMG")" xattr -p com.apple.quarantine "$DMG"
check "Gatekeeper accepts the quarantined dmg (Notarized Developer ID)" "spctl -t open" gatekeeper_ok open context:primary-signature "$DMG"

mount_dmg() { hdiutil attach -noautoopen -readonly -mountpoint "$MNT" "$DMG" >/dev/null; }
install_from_dmg() {
  pkill -f "$PROC_RE" 2>/dev/null; sleep 1
  rm -rf "$APP"
  mount_dmg || return 1
  # What a user does: drag the app from the dmg window to Applications, so Finder copies it.
  if limit 90 osascript -e "tell application \"Finder\" to duplicate (POSIX file \"$MNT/Hickeyfield.app\" as alias) to (POSIX file \"/Applications\" as alias) with replacing" > /dev/null 2>> "$OUT/osascript.log" && [ -d "$APP" ]; then
    INSTALLED_BY="Finder copy from the dmg"
  else
    ditto "$MNT/Hickeyfield.app" "$APP"
    INSTALLED_BY="ditto from the dmg (Finder did not copy it)"
  fi
  hdiutil detach "$MNT" -quiet || hdiutil detach "$MNT" -force -quiet
  [ -d "$APP" ]
}
# A copy made by a script (not a drag in a Finder window) runs translocated from a read-only
# folder while it is quarantined. After the Gatekeeper check the flag is cleared, which is what
# the user's "Open" in the first-launch question does; then the app runs from /Applications.
clear_quarantine() { xattr -dr com.apple.quarantine "$APP" 2>/dev/null; ! xattr -p com.apple.quarantine "$APP" >/dev/null 2>&1; }
check "Hickeyfield $FROM installed in /Applications" "" install_from_dmg
note "how it was installed" "$INSTALLED_BY"
check "the installed app is $FROM" "$(plist "$APP" CFBundleShortVersionString)" test "$(plist "$APP" CFBundleShortVersionString)" = "$FROM"
check "the installed copy carries the download's quarantine flag" "$(xattr -p com.apple.quarantine "$APP" 2>/dev/null)" xattr -p com.apple.quarantine "$APP"
check "Gatekeeper accepts the installed, quarantined app (Notarized Developer ID)" "spctl -t exec" gatekeeper_ok exec "" "$APP"
check "quarantine flag cleared after the Gatekeeper check (as the first-launch \"Open\" does)" "no App Translocation" clear_quarantine
FROM_CDHASH="$(cdhash "$APP")"
codesign -dvvv "$APP" > "$OUT/codesign-before.txt" 2>&1

# ---- 2. Designated requirement and TCC grants -------------------------------------------------
DR_BEFORE="$(dr "$APP")"
echo "$DR_BEFORE" > "$OUT/designated-requirement-before.txt"
log "designated requirement: $DR_BEFORE"
printf '%s' "$DR_BEFORE" | csreq -r- -b "$WORK/csreq.bin"
check "csreq blob made from the designated requirement" "$(wc -c < "$WORK/csreq.bin" | tr -d ' ') bytes" test -s "$WORK/csreq.bin"
python3 "$HERE/tcc.py" schema "$USER_DB" > "$OUT/tcc-schema-user.sql" 2>&1
sudo python3 "$HERE/tcc.py" schema "$SYS_DB" > "$OUT/tcc-schema-system.sql" 2>&1
note "TCC access table of this macOS version (tcc-schema-*.sql)" "$(tr -s ' \n' ' ' < "$OUT/tcc-schema-system.sql" | cut -c1-280)"
grant() { # db service auth_reason
  if [ "$1" = "$SYS_DB" ]; then sudo python3 "$HERE/tcc.py" grant "$1" "$2" "$BUNDLE_ID" "$WORK/csreq.bin" "$3"
  else python3 "$HERE/tcc.py" grant "$1" "$2" "$BUNDLE_ID" "$WORK/csreq.bin" "$3"; fi
}
db_of() { case "$1" in kTCCServiceCamera | kTCCServiceMicrophone) echo "$USER_DB" ;; *) echo "$SYS_DB" ;; esac; }
reason_of() { case "$1" in kTCCServiceCamera | kTCCServiceMicrophone) echo 2 ;; *) echo 3 ;; esac; }
db_label() { if [ "$(db_of "$1")" = "$SYS_DB" ]; then echo "system TCC.db"; else echo "user TCC.db"; fi; }
for s in "${SERVICES[@]}"; do
  check "TCC grant written: $s ($(db_label "$s"))" "auth_value 2, csreq of the designated requirement" grant "$(db_of "$s")" "$s" "$(reason_of "$s")"
done
tcc_rows() {
  python3 "$HERE/tcc.py" rows "$USER_DB" "$BUNDLE_ID"
  sudo python3 "$HERE/tcc.py" rows "$SYS_DB" "$BUNDLE_ID"
}
tcc_rows > "$OUT/tcc-rows-before.jsonl" 2>&1
# A row of the app for the service, allowed, with a csreq.
row_ok() { python3 -c 'import json,sys; sys.exit(0 if any(r["service"] == sys.argv[2] and r["auth_value"] == 2 and r.get("csreq") for r in map(json.loads, filter(str.strip, open(sys.argv[1])))) else 1)' "$2" "$1"; }
for s in "${SERVICES[@]}"; do
  check "TCC row present before the update: $s" "allowed, csreq set" row_ok "$s" "$OUT/tcc-rows-before.jsonl"
done
# The check tccd makes when the app uses a service: the app satisfies the stored csreq.
csreq_ok() { # service -> the installed app satisfies the csreq stored in that row
  local f="$WORK/stored-$1.bin"
  if [ "$(db_of "$1")" = "$SYS_DB" ]; then sudo python3 "$HERE/tcc.py" csreq "$SYS_DB" "$1" "$BUNDLE_ID" "$f" && sudo chown "$(id -u)" "$f"
  else python3 "$HERE/tcc.py" csreq "$USER_DB" "$1" "$BUNDLE_ID" "$f"; fi || return 1
  echo "$1: $(csreq -r "$f" -t 2>&1)" >> "$OUT/tcc-csreq-text.txt"
  codesign --verify -R "$f" "$APP" 2>> "$OUT/tcc-csreq-verify.txt"
}
for s in "${SERVICES[@]}"; do check "[before] $FROM satisfies the stored csreq of $s" "" csreq_ok "$s"; done

# ---- 3. Start the app the normal way ----------------------------------------------------------
mkdir -p "$HOME/Library/Application Support/publik/apps"
python3 "$HERE/fake-publik.py" "$PORT" unclaimed > "$OUT/fake-publik.log" 2>&1 &
FAKE_PID=$!
# The same environment for every launch through LaunchServices, also the one Tauri makes when it
# restarts into the update (open -n -a): no usage counts, the stand-in for sign-in, the updater's
# debug lines in the app log (src-tauri/src/lib.rs reads hickeyfield_LOG; the log goes to stdout).
ENV_VARS=(DO_NOT_TRACK=1 NO_COLOR=1 "PUBLIK_ORIGIN=http://127.0.0.1:$PORT" "hickeyfield_LOG=info,tauri_plugin_updater=debug")
for kv in "${ENV_VARS[@]}"; do launchctl setenv "${kv%%=*}" "${kv#*=}"; done
trap 'kill $FAKE_PID 2>/dev/null; pkill -f "$PROC_RE" 2>/dev/null; for kv in "${ENV_VARS[@]}"; do launchctl unsetenv "${kv%%=*}"; done; true' EXIT
wait_for 20 curl -fs "http://127.0.0.1:$PORT/api/v1/wallet" -o /dev/null
note "sign-in" "PUBLIK_ORIGIN is a local stand-in that answers \"unclaimed\": the app stays on Sign in with publik; it logs the version each app process sends with its install request"
CREDENTIAL="$HOME/Library/Application Support/publik/apps/hickeyfield.json"
# The next process that starts sends POST /installs with its compiled-in version: forget the stand-in key.
forget_key() { rm -f "$CREDENTIAL"; }
told_version() { grep -q "install request: app_version=$1 " "$OUT/fake-publik.log"; }
# The inode of the executable a process runs, and of the one on disk now.
running_inode() { lsof -a -p "$1" -d txt -Fin 2>/dev/null | awk '/^i/ { i = substr($0, 2) } /^n/ && /\/Contents\/MacOS\/hickeyfield-tauri$/ { print i; exit }'; }
disk_inode() { stat -f %i "$APP/Contents/MacOS/$EXE"; }
runs_disk_file() { local i; i="$(running_inode "$1")"; [ -n "$i" ] && [ "$i" = "$(disk_inode)" ]; }
runs_old_file() { local i; i="$(running_inode "$1")"; [ -n "$i" ] && [ "$i" != "$(disk_inode)" ]; }
# Quits Hickeyfield (SIGTERM; an Apple event to the app would need an Automation consent on the runner).
quit_app() { pkill -f "$PROC_RE"; wait_for 15 sh -c '! pgrep -f "$1" >/dev/null' _ "$PROC_RE" || pkill -9 -f "$PROC_RE"; sleep 1; }
start_app() { # log file (the app logs to stdout)
  local e=()
  for kv in "${ENV_VARS[@]}"; do e+=(--env "$kv"); done
  open "${e[@]}" --stdout "$1" --stderr "$1.stderr" "$APP"
}
# Starts the app; answers a first-launch question of Gatekeeper with "Open" as a user does.
# `open` waits while that question is on screen, so it runs in the background here.
launch() { # log file, label
  local pid="" t0="$SECONDS" open_pid
  : > "$1"
  start_app "$1" &
  open_pid=$!
  while [ $((SECONDS - t0)) -lt 90 ]; do
    pid="$(app_pid)"
    [ -n "$pid" ] && break
    if has_window CoreServicesUIAgent; then
      shot "$2-gatekeeper-question"
      echo "$2: Gatekeeper asked: $(texts_of CoreServicesUIAgent)" >> "$OUT/gatekeeper.txt"
      press CoreServicesUIAgent Open && echo "$2: pressed Open" >> "$OUT/gatekeeper.txt"
    fi
    sleep 1
  done
  if [ -z "$pid" ]; then
    note "$2: the app did not start after open" "removing the quarantine flag and opening it again"
    kill "$open_pid" 2>/dev/null
    xattr -dr com.apple.quarantine "$APP" 2>/dev/null
    start_app "$1" &
    wait_for 30 app_running
    pid="$(app_pid)"
  fi
  PID="$pid"
  [ -n "$PID" ]
}
running_from_applications() { ps -o command= -p "${1:-$PID}" | grep -q "^$APP/Contents/MacOS/$EXE"; }
gate_on_screen() { has_alert "Sign in with publik"; }

check "Hickeyfield starts with open" "" launch "$OUT/app-1.log" first
log "pid $PID: $(ps -o command= -p "$PID")"
if ! running_from_applications; then
  note "the app runs from $(ps -o command= -p "$PID")" "App Translocation: quit, clear the quarantine flag of the bundle and open it again"
  quit_app
  forget_key
  xattr -dr com.apple.quarantine "$APP" 2>/dev/null
  launch "$OUT/app-1.log" first-again
fi
if [ -f "$OUT/gatekeeper.txt" ]; then note "Gatekeeper first-launch question" "$(tr '\n' ' ' < "$OUT/gatekeeper.txt")"; else note "Gatekeeper first-launch question" "not shown (the flag was cleared after the Gatekeeper check)"; fi
check "it runs from /Applications (not translocated)" "$(ps -o command= -p "$PID")" running_from_applications
check "the app shows its \"Sign in with publik\" question (nobody signs in; the main window stays hidden)" "" wait_for 60 gate_on_screen
shot started

# "Check for Updates…" in the app menu, and the app's answer: "Hickeyfield <v> is the latest version."
# Arguments: version, label, app log ("" when the process logs nowhere: a restart by the app).
menu_says_latest() {
  local v="$1" label="$2" lg="${3:-}" before=0 clicked answer
  [ -n "$lg" ] && before="$(count_in "$lg" "updates: $v is the latest version")"
  clicked="$(click_check_for_updates)"
  check "[$label] \"Check for Updates…\" clicked in the app menu (System Events)" "$clicked" test -n "$clicked"
  check "[$label] the app answers \"Hickeyfield $v is the latest version.\"" "" wait_for 60 has_alert "Hickeyfield $v is the latest version"
  answer="$(alert_find "is the latest version")"
  shot "$label-menu-answer"
  note "[$label] the answer on screen" "$answer"
  if [ -n "$lg" ]; then
    check "[$label] the app log has the manual check" "" wait_for 10 sh -c '[ "$(grep -cF "$1" "$2")" -gt "$3" ]' _ "updates: $v is the latest version" "$lg" "$before"
  fi
  check "[$label] \"OK\" closes the answer" "" alert_press_ok
  check "[$label] the answer is gone" "" wait_for 10 no_alert "is the latest version"
}
alert_press_ok() { alert_press "is the latest version" OK >/dev/null; }

# ---- 4. The live check ------------------------------------------------------------------------
LOG="$OUT/app-1.log"
check "the updater asks the live feed" "$FEED" wait_for 60 log_has "$LOG" "checking for updates $FEED"
if [ "$MODE" = no-update ]; then
  check "the app reports \"$FROM is the latest version\"" "" wait_for 120 log_has "$LOG" "updates: $FROM is the latest version"
  sleep 20
  shot no-update
  check "no update was installed or offered" "" sh -c '! grep -q "updates: Hickeyfield .* is ready" "$1"' _ "$LOG"
  check "no update question on screen (no \"Hickeyfield update\" window)" "" no_alert "Hickeyfield update"
  check "the app keeps running (same process)" "pid $PID" kill -0 "$PID"
  check "the running app told its version: $FROM (install request)" "" told_version "$FROM"
  check "the running process executes the file on disk (inode)" "$(running_inode "$PID") = $(disk_inode)" runs_disk_file "$PID"
  check "the app on disk is still $FROM" "$(plist "$APP" CFBundleShortVersionString)" test "$(plist "$APP" CFBundleShortVersionString)" = "$FROM"
  check "the app on disk is the same code (CDHash)" "$(cdhash "$APP")" test "$(cdhash "$APP")" = "$FROM_CDHASH"
  menu_says_latest "$FROM" menu "$LOG"
  check "[menu] the app keeps running (same process)" "pid $PID" kill -0 "$PID"
  note "variants Restart and Later" "not applicable: the feed offers no newer version"
else
  # Variant A: "Restart".
  check "[Restart] the updater downloads, verifies and installs $EXPECT by itself" "" wait_for 300 log_has "$LOG" "updates: Hickeyfield $EXPECT is ready"
  check "[Restart] the app on disk is now $EXPECT" "$(plist "$APP" CFBundleShortVersionString)" test "$(plist "$APP" CFBundleShortVersionString)" = "$EXPECT"
  check "[Restart] the question \"Hickeyfield $EXPECT is installed. Restart Hickeyfield to use it.\" is on screen" "" wait_for 60 has_alert "Hickeyfield $EXPECT is installed"
  shot restart-question
  note "[Restart] the question" "$(alert_find "is installed")"
  OLD_PID="$PID"
  check "[Restart] until the answer, the old code keeps running (inode of the running file is not the new one)" "$(running_inode "$OLD_PID") / $(disk_inode)" runs_old_file "$OLD_PID"
  forget_key
  if alert_press "is installed" Restart > "$WORK/press.txt"; then
    note "[Restart] answered \"Restart\" in the question" "System Events click; Tauri relaunches the app with open -n -a"
    wait_for 30 sh -c '! kill -0 "$1" 2>/dev/null' _ "$OLD_PID"
    wait_for 30 sh -c 'pgrep -f "$1" | grep -qvx "$2"' _ "$PROC_RE" "$OLD_PID"
  else
    note "[Restart] the question could not be pressed on this runner" "quit and reopen instead"
    quit_app
    launch "$LOG.reopen" reopen-after-restart-question
  fi
  PID="$(pgrep -f "$PROC_RE" | grep -vx "$OLD_PID" | head -1)"
  check "[Restart] Hickeyfield runs again as a new process" "pid $OLD_PID -> ${PID:-none}" test -n "$PID"
  check "[Restart] the old process is gone" "" sh -c '! kill -0 "$1" 2>/dev/null' _ "$OLD_PID"
  check "[Restart] the new process is the installed app" "$(ps -o command= -p "${PID:-0}")" running_from_applications "${PID:-0}"
  check "[Restart] the new process executes the $EXPECT file on disk (inode)" "" runs_disk_file "${PID:-0}"
  check "[Restart] the running app told its version: $EXPECT (install request)" "" wait_for 60 told_version "$EXPECT"
  check "[Restart] the new process shows its \"Sign in with publik\" question" "" wait_for 60 gate_on_screen
  shot restarted
  sleep 15
  menu_says_latest "$EXPECT" Restart ""

  # Variant B: "Later", from a fresh install of the old version.
  log "variant Later: install $FROM again"
  quit_app
  check "[Later] Hickeyfield $FROM installed again" "" install_from_dmg
  check "[Later] quarantine flag cleared after the copy" "" clear_quarantine
  forget_key
  check "[Later] Hickeyfield starts with open" "" launch "$OUT/app-2.log" later
  LOG="$OUT/app-2.log"
  check "[Later] the updater installs $EXPECT by itself" "" wait_for 300 log_has "$LOG" "updates: Hickeyfield $EXPECT is ready"
  check "[Later] the question is on screen" "" wait_for 60 has_alert "Hickeyfield $EXPECT is installed"
  shot later-question
  LATER_PID="$PID"
  if alert_press "is installed" Later > "$WORK/press.txt"; then
    note "[Later] answered \"Later\"" "System Events click"
    sleep 15
    check "[Later] the app keeps running (same process)" "pid $LATER_PID" kill -0 "$LATER_PID"
    check "[Later] the question is gone" "" no_alert "is installed"
    check "[Later] the running process is still the old code (inode)" "$(running_inode "$LATER_PID") / $(disk_inode)" runs_old_file "$LATER_PID"
    # The menu offers the downloaded update again at once.
    clicked="$(click_check_for_updates)"
    # GitHub's macOS runner sometimes takes System Events access from osascript in the middle of a job.
    # Then the menu cannot be driven: that is a limit of the runner, not an answer of the app.
    if [ -z "$clicked" ] && tail -n 1 "$OUT/osascript.log" 2>/dev/null | grep -q "not allowed assistive access"; then
      note "[Later] \"Check for Updates…\" not driven" "the runner refused System Events access to osascript; the Restart variant drove the same menu"
    else
      check "[Later] \"Check for Updates…\" clicked in the app menu" "$clicked" test -n "$clicked"
      check "[Later] the menu offers $EXPECT again (the same question)" "" wait_for 60 has_alert "Hickeyfield $EXPECT is installed"
      shot later-menu-question
      alert_press "is installed" Later > "$WORK/press.txt"
      sleep 5
    fi
    check "[Later] after a second \"Later\" the app still runs (same process)" "pid $LATER_PID" kill -0 "$LATER_PID"
  else
    fail "[Later] the question could not be pressed on this runner" "the app is left running with the question open"
  fi
  check "[Later] the new version waits on disk ($EXPECT)" "$(plist "$APP" CFBundleShortVersionString)" test "$(plist "$APP" CFBundleShortVersionString)" = "$EXPECT"
  shot later
  log "variant Later: quit and open again"
  quit_app
  forget_key
  check "[Later] Hickeyfield opens again" "" launch "$OUT/app-3.log" reopen
  check "[Later] after the reopen the running app reports \"$EXPECT is the latest version\"" "" wait_for 120 log_has "$OUT/app-3.log" "updates: $EXPECT is the latest version"
  check "[Later] after the reopen it is the installed app, the $EXPECT file (inode)" "" runs_disk_file "${PID:-0}"
  check "[Later] after the reopen the running app told its version: $EXPECT (install request)" "" wait_for 60 told_version "$EXPECT"
fi

# ---- 5. After: signature, Gatekeeper, TCC -----------------------------------------------------
codesign -dvvv "$APP" > "$OUT/codesign-after.txt" 2>&1
DR_AFTER="$(dr "$APP")"
echo "$DR_AFTER" > "$OUT/designated-requirement-after.txt"
check "[after] the app on disk is $EXPECT" "$(plist "$APP" CFBundleShortVersionString)" test "$(plist "$APP" CFBundleShortVersionString)" = "$EXPECT"
check "[after] designated requirement unchanged" "$DR_AFTER" test "$DR_AFTER" = "$DR_BEFORE"
check "[after] signature intact (codesign --verify --strict --deep)" "" codesign --verify --strict --deep "$APP"
check "[after] Gatekeeper accepts the app (Notarized Developer ID)" "" gatekeeper_ok exec "" "$APP"
tcc_rows > "$OUT/tcc-rows-after.jsonl" 2>&1
for s in "${SERVICES[@]}"; do
  check "[after] TCC row still there: $s" "allowed, csreq set" row_ok "$s" "$OUT/tcc-rows-after.jsonl"
  check "[after] the installed app satisfies the stored csreq of $s" "codesign --verify -R <stored csreq>" csreq_ok "$s"
done
note "consent prompts" "a runner cannot show a TCC consent prompt; the stored csreq of every grant is satisfied by the installed app, which is the check tccd makes, so no new prompt is needed"
note "TCC services in use" "none: the app stays on its sign-in question with its main window hidden, so the camera, the microphone, screen capture and accessibility are never used by the app"
if [ "$MODE" = update ]; then
  # Only now, for the comparison: the release's own update bundle of $EXPECT.
  mkdir -p "$WORK/compare"
  curl -fsSL "https://github.com/$REPO/releases/download/v$EXPECT/Hickeyfield_${EXPECT}_aarch64.app.tar.gz" -o "$WORK/compare/new.app.tar.gz" &&
    tar -xzf "$WORK/compare/new.app.tar.gz" -C "$WORK/compare"
  check "[after] the installed app is the $EXPECT release's app, byte for byte (CDHash)" "$(cdhash "$APP")" \
    test -n "$(cdhash "$APP")" -a "$(cdhash "$APP")" = "$(cdhash "$WORK/compare/Hickeyfield.app")"
fi

# ---- What the app logged about the feed, and what this job downloaded -------------------------
for f in "$OUT"/app-*.log; do
  [ -f "$f" ] || continue
  echo "== $(basename "$f")"
  grep -E "checking for updates|update response|parsed release|updates:" "$f" | cut -c1-600
done > "$OUT/updater-lines.txt"
cat "$OUT/updater-lines.txt"
check "the app's request went to publikhq.com and the feed named the GitHub asset" "" \
  sh -c 'grep -qF "checking for updates $1" "$2" && grep -q "update response" "$2" && grep -q "https://github.com/$3/releases/download/" "$4"' _ "$FEED" "$OUT/updater-lines.txt" "$REPO" "$OUT/feed-at-start.json"
check "downloads by this job before the check: only the $FROM_TAG dmg and its checksums" "$(ls "$WORK/dl" | tr '\n' ' ')" \
  test "$(ls "$WORK/dl" | tr '\n' ' ')" = "$(basename "$DMG") SHA256SUMS.txt "

pkill -f "$PROC_RE" 2>/dev/null
shot end
echo
echo "$PASS passed, $FAILS failed"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  { echo "### macOS live update proof: $FROM_TAG -> $EXPECT ($MODE): $PASS passed, $FAILS failed"; echo; cat "$RESULTS"; } >> "$GITHUB_STEP_SUMMARY"
fi
[ "$FAILS" -eq 0 ]
