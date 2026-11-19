#!/usr/bin/env bash
# The whole Hickeyfield release in one command, on the release Mac (docs/updates.md, "Cut a release"):
#
#   scripts/publik-release.sh <version>
#
# Before it: put <version> in src-tauri/tauri.conf.json, the workspace Cargo.toml, Cargo.lock
# (hickeyfield-core and hickeyfield-tauri) and ui/package.json, add a "## <version>" section to
# CHANGELOG.md, commit, push to main.
#
# Hickeyfield ships macOS arm64 only (no Windows release), so the release is: the dmg, the update
# bundle (.app.tar.gz + .sig), the feed latest.json + latest.json.sig, and SHA256SUMS.txt.
#
# What it does:
#  1. Checks: on main, clean tree, pushed; the version in every file; higher than the last
#     release; the tag is free; the Developer ID, the notary profile and the update key are there;
#     enough free disk.
#  2. Waits for ci.yml on that commit (the push run, else it starts one). Every job must pass. The
#     only exception: a step named in PUBLIK_RELEASE_KNOWN_RED_STEPS (default "Licence scan", red
#     on main because of old advisories) may fail. A run with any other failure is re-run once.
#  3. Downloads the unsigned app the macOS bundle job built (artifact Hickeyfield-macos-arm64-app).
#  4. scripts/sign-ci-macos.sh: signs the app (the FFmpeg sidecar first, hardened runtime,
#     src-tauri/Entitlements.plist) with the Developer ID, notarizes and staples it, packs and
#     signs the update bundle from the stapled app, builds the dmg, signs, notarizes, staples it.
#  5. Writes latest.json (absolute GitHub URLs, the CHANGELOG section as notes) and signs it with
#     the update key (latest.json.sig). Writes SHA256SUMS.txt.
#  6. Uploads everything to a DRAFT release.
#  7. Downloads the draft back and verifies it: the asset list, the checksums, the feed and every
#     signature with the app's public key (and that key is the last release's), the versions
#     (Info.plist of the app in the dmg and in the update bundle = the feed version), codesign,
#     spctl, stapler, the designated requirement and the publik app token against the last
#     release, the update bundle = the app of the dmg.
#  8. Publishes, then checks the tag and every URL of the feed.
#
# Run the same command again after a failure: it reuses the CI run of the commit, the files in
# the work folder and the draft, and goes on from there. For a published release it only runs
# the checks of step 7. It stops before publishing on any failed check (the draft stays), starts
# no app, and prints no secret: the update key goes from the keychain into the signer's
# environment only.
#
# Env: PUBLIK_RELEASE_WORK  work folder (default <repo>/.release/v<version>, git-ignored);
#                           deleted after a successful publish
#      APPLE_SIGNING_IDENTITY, NOTARY_PROFILE (default "publik"), TAURI_CLI
#      UPDATE_KEY_SERVICE   keychain service of the update key (default "publik-update-key",
#                           accounts mann-private and mann-password)
#      PUBLIK_RELEASE_KNOWN_RED_STEPS  comma-separated ci.yml step names that may fail
#                           (default "Licence scan"; empty: every step must pass)
#      PUBLIK_RELEASE_NO_PUBLISH=1  stop after the verify and leave the draft
set -euo pipefail

V="${1:?usage: scripts/publik-release.sh <version>}"
V="${V#v}"
TAG="v$V"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP=Hickeyfield
SLUG=hickeyfield
BUNDLE_ID=ai.hickeyfield.studio
BRANCH=main
IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Mann Bellani (R5R3ZS54LV)}"
TEAM=R5R3ZS54LV
PROFILE="${NOTARY_PROFILE:-publik}"
SERVICE="${UPDATE_KEY_SERVICE:-publik-update-key}"
WORK="${PUBLIK_RELEASE_WORK:-$ROOT/.release/$TAG}"
CI_WF=ci.yml
CI_MAC_JOB="Bundle (macos-latest)"
CI_ARTIFACT=Hickeyfield-macos-arm64-app
CI_ZIP=Hickeyfield-macos-arm64.app.zip
KNOWN_RED="${PUBLIK_RELEASE_KNOWN_RED_STEPS-Licence scan}"
DMG="hickeyfield-${V}-macos-arm64.dmg"
TGZ="${APP}_${V}_aarch64.app.tar.gz"
ASSETS=("$DMG" "$TGZ" "$TGZ.sig" latest.json latest.json.sig SHA256SUMS.txt)

say() { printf '\n==> %s\n' "$*"; }
ok() { printf '  ok    %s\n' "$*"; }
note() { printf '  note  %s\n' "$*"; }
die() { printf '\nSTOP: %s\n' "$*" >&2; exit 1; }
FAILS=0
bad() { printf '  FAIL  %s\n' "$*"; FAILS=$((FAILS + 1)); }
# expect <description> <command...>: ok when the command succeeds, FAIL otherwise.
expect() { local what="$1"; shift; if "$@"; then ok "$what"; else bad "$what"; fi; }
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
dr() { codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => //p' || true; }
stapled() { xcrun stapler validate "$1" >/dev/null 2>&1; }
# The publik app token compiled into a binary (pat_ + 48 characters; public by design, never
# printed here): one sha256 line per distinct token, so two binaries can be compared.
token_ids() { { LC_ALL=C grep -a -o -E 'pat_[A-Za-z0-9_-]{32,}' "$1" 2>/dev/null || true; } | sort -u | while IFS= read -r t; do printf '%s' "$t" | shasum -a 256 | cut -c1-16; done; }

[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "the version must be x.y.z, not $V"
for t in gh git node npm xcrun codesign hdiutil security shasum spctl ditto tar unzip file curl; do
  command -v "$t" >/dev/null || die "$t is not installed"
done
gh auth status >/dev/null 2>&1 || die "gh is not logged in"
mkdir -p "$WORK"
# gh for reads and downloads, with retries: the GitHub API limit (5000 calls an hour) is per user
# and other jobs of the same user share it; a 5xx or a dropped connection is retried too.
ghr() {
  local out err i
  for i in $(seq 1 45); do
    if out="$(gh "$@" 2> "$WORK/gh.err")"; then
      [ -z "$out" ] || printf '%s\n' "$out"
      return 0
    fi
    err="$(tr '\n' ' ' < "$WORK/gh.err")"
    case "$err" in
      *"rate limit"* | *"HTTP 5"* | *"timed out"* | *"timeout"* | *"connection"* | *"EOF"* | *"TLS"*) ;;
      *) printf '%s\n' "$err" >&2; return 1 ;;
    esac
    echo "    GitHub API: ${err:0:110}... waiting 90 s (try $i of 45)" >&2
    sleep 90
  done
  printf '%s\n' "$err" >&2
  return 1
}
REPO="$(ghr repo view --json nameWithOwner -q .nameWithOwner)"
BASE_URL="https://github.com/$REPO/releases/download/$TAG/"
STATE="$WORK/state"
touch "$STATE"
get_state() { sed -n "s/^$1=//p" "$STATE" | tail -1; }
set_state() { echo "$1=$2" >> "$STATE"; }

# ---------------------------------------------------------------------------------------------
say "$APP $V: checks"
RELEASE="$(ghr release view "$TAG" -R "$REPO" --json isDraft,targetCommitish 2>/dev/null || true)"
if [ -z "$RELEASE" ]; then RSTATE=none
elif [ "$(node -p 'JSON.parse(process.argv[1]).isDraft' "$RELEASE")" = true ]; then RSTATE=draft
else RSTATE=published; fi
ok "release $TAG: $RSTATE"
PREV_TAG="$(ghr release list -R "$REPO" --exclude-drafts --exclude-pre-releases -L 20 --json tagName -q '.[].tagName' | grep -vx "$TAG" | head -1)"
[ -n "$PREV_TAG" ] || die "no earlier published release"
ok "last published release: $PREV_TAG"
git fetch -q origin "$BRANCH" --tags

if [ "$RSTATE" = published ]; then
  SHA="$(git rev-parse "$TAG^{commit}")"
  note "$TAG is already published: verifying it only"
else
  [ "$(git rev-parse --abbrev-ref HEAD)" = "$BRANCH" ] || die "check out $BRANCH first"
  [ -z "$(git status --porcelain)" ] || die "the working tree is not clean"
  SHA="$(git rev-parse HEAD)"
  [ "$SHA" = "$(git rev-parse "origin/$BRANCH")" ] || die "HEAD is not origin/$BRANCH: push (or pull) first"
  ok "on $BRANCH at ${SHA:0:12}, clean, pushed"
  if git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then die "the tag $TAG exists but has no published release"; fi
  ok "tag $TAG is free"
  if [ "$RSTATE" = draft ]; then
    TARGET="$(node -p 'JSON.parse(process.argv[1]).targetCommitish' "$RELEASE")"
    [ "$TARGET" = "$SHA" ] || die "the draft $TAG is for commit $TARGET, not HEAD $SHA: delete the draft or check out that commit"
    ok "resuming the draft $TAG"
  fi
  # The version in every file a bump touches.
  CONF_V="$(node -p 'require("./src-tauri/tauri.conf.json").version')"
  UI_V="$(node -p 'require("./ui/package.json").version')"
  CARGO_V="$(awk '/^\[workspace.package\]/ { f = 1; next } /^\[/ { f = 0 } f && /^version = / { gsub(/version = |"/, ""); print; exit }' Cargo.toml)"
  LOCK_CORE="$(awk '/^name = "hickeyfield-core"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)"
  LOCK_TAURI="$(awk '/^name = "hickeyfield-tauri"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)"
  for pair in "src-tauri/tauri.conf.json:$CONF_V" "ui/package.json:$UI_V" "Cargo.toml:$CARGO_V" \
    "Cargo.lock (hickeyfield-core):$LOCK_CORE" "Cargo.lock (hickeyfield-tauri):$LOCK_TAURI"; do
    [ "${pair##*:}" = "$V" ] || die "${pair%:*} has version ${pair##*:}, not $V"
  done
  grep -Eq "^## $V( |$)" CHANGELOG.md || die "CHANGELOG.md has no \"## $V\" section"
  ok "version $V in tauri.conf.json, Cargo.toml, Cargo.lock (2 crates), ui/package.json and CHANGELOG.md"
  [ "$(printf '%s\n%s\n' "${PREV_TAG#v}" "$V" | sort -V | tail -1)" = "$V" ] && [ "${PREV_TAG#v}" != "$V" ] ||
    die "$V is not higher than the last release ${PREV_TAG#v}"
  ok "$V is higher than ${PREV_TAG#v}"
  security find-identity -v -p codesigning | grep -qF "\"$IDENTITY\"" || die "no signing identity \"$IDENTITY\" in the keychain"
  ok "signing identity: $IDENTITY"
  security find-generic-password -s "$SERVICE" -a mann-private >/dev/null 2>&1 || die "no update key: keychain service $SERVICE, account mann-private"
  ok "update key: keychain item $SERVICE (not read yet)"
  xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 || die "the notary profile \"$PROFILE\" does not work (xcrun notarytool store-credentials)"
  ok "notary profile: $PROFILE"
fi
# The work folder belongs to one commit: files from another commit's builds are never reused.
if [ "$RSTATE" != published ]; then
  if [ -n "$(get_state sha)" ] && [ "$(get_state sha)" != "$SHA" ]; then
    note "the work folder has the builds of $(get_state sha | cut -c1-12): starting it again for ${SHA:0:12}"
    rm -rf "$WORK/in" "$WORK/out" "$WORK/verify" "$WORK/changes.md" "$WORK/release-notes.md"
    : > "$STATE"
  fi
  [ "$(get_state sha)" = "$SHA" ] || set_state sha "$SHA"
fi
# The app is about 70 MB; the build, the downloads back and the last release need about 0.5 GB.
NEED_KB=1500000
[ "$RSTATE" = published ] && NEED_KB=500000
FREE_KB="$(df -k "$WORK" | awk 'NR == 2 { print $4 }')"
[ "$FREE_KB" -ge "$NEED_KB" ] || die "only $((FREE_KB / 1024)) MB free on the disk of $WORK; this needs $((NEED_KB / 1024)) MB"
ok "disk: $((FREE_KB / 1024)) MB free"

# The Tauri CLI signs the update files: the repo's, else the locked version in the work folder.
TAURI="${TAURI_CLI:-}"
if [ "$RSTATE" != published ]; then
  for c in "$ROOT/node_modules/.bin/tauri" "$ROOT/ui/node_modules/.bin/tauri" "$WORK/tauri-cli/node_modules/.bin/tauri"; do
    [ -z "$TAURI" ] && [ -x "$c" ] && TAURI="$c"
  done
  if [ -z "$TAURI" ]; then
    CLI_V="$(awk "/^      '@tauri-apps\/cli':\$/ { f = 1; next } f && /version:/ { print \$2; exit }" ui/pnpm-lock.yaml)"
    [ -n "$CLI_V" ] || die "no @tauri-apps/cli version in ui/pnpm-lock.yaml"
    npm install --silent --no-save --no-package-lock --no-audit --no-fund --prefix "$WORK/tauri-cli" "@tauri-apps/cli@$CLI_V" >/dev/null
    TAURI="$WORK/tauri-cli/node_modules/.bin/tauri"
  fi
  ok "Tauri CLI $("$TAURI" --version | awk '{ print $NF }')"
  export TAURI_CLI="$TAURI"
fi

# Runs a command with the update key in its environment. The key is never printed.
with_update_key() {
  if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then "$@"; return; fi
  local key pw
  key="$(security find-generic-password -s "$SERVICE" -a mann-private -w 2>/dev/null)" || die "cannot read the update key"
  pw="$(security find-generic-password -s "$SERVICE" -a mann-password -w 2>/dev/null || true)"
  TAURI_SIGNING_PRIVATE_KEY="$key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$pw" "$@"
}

# ---------------------------------------------------------------------------------------------
# At most one GitHub API call per 90 s while waiting: other jobs of the user share the limit.
POLL=90
# "status conclusion" of a run in one call.
run_state() { ghr run view "$1" -R "$REPO" --json status,conclusion -q '"\(.status) \(.conclusion)"'; }
# The newest run of ci.yml on $SHA (push or workflow_dispatch), or nothing.
last_ci_run() {
  ghr run list -R "$REPO" --workflow "$CI_WF" --commit "$SHA" -L 20 \
    --json databaseId,createdAt -q 'sort_by(.createdAt) | reverse | .[0].databaseId // empty'
}
# Starts ci.yml on $BRANCH (HEAD is $SHA, checked above) and prints the new run's id.
start_ci_run() {
  local before id
  before="$(ghr run list -R "$REPO" --workflow "$CI_WF" -L 50 --json databaseId -q '.[].databaseId' | sort)"
  gh workflow run "$CI_WF" -R "$REPO" --ref "$BRANCH" >/dev/null
  sleep 15
  for _ in $(seq 1 10); do
    id="$(ghr run list -R "$REPO" --workflow "$CI_WF" --commit "$SHA" -L 20 --json databaseId -q '.[].databaseId' |
      sort | comm -13 <(printf '%s\n' "$before") - | tail -1)"
    [ -n "$id" ] && { echo "$id"; return; }
    sleep "$POLL"
  done
  die "$CI_WF did not start"
}
# Waits for a run to complete.
wait_run() {
  local id="$1" t=0 st
  while :; do
    st="$(run_state "$id")"
    [ "${st%% *}" = completed ] && break
    [ $((t % (POLL * 4))) -eq 0 ] && echo "    run $id: ${st%% *} ($((t / 60)) min)"
    sleep "$POLL"; t=$((t + POLL))
    [ "$t" -lt 9000 ] || die "run $id did not finish in 150 minutes"
  done
}
# Every job passed, or failed only in a step named in $KNOWN_RED (with no step skipped after it);
# the macOS bundle job (the app that ships) must pass. Prints one line per job.
ci_good() {
  local jobs
  jobs="$(ghr run view "$1" -R "$REPO" --json jobs)" || return 1
  node -e '
    const [json, known, macJob] = process.argv.slice(1);
    const red = new Set(known.split(",").map((s) => s.trim()).filter(Boolean));
    const jobs = JSON.parse(json).jobs ?? [];
    let good = jobs.some((j) => j.name === macJob && j.conclusion === "success");
    if (!good) console.log(`  FAIL  CI job ${macJob}: missing or not successful`);
    for (const j of jobs) {
      if (j.conclusion === "success") { console.log(`  ok    CI job ${j.name}`); continue; }
      const steps = j.steps ?? [];
      const failed = steps.filter((s) => !["success", "skipped"].includes(s.conclusion));
      const first = steps.findIndex((s) => !["success", "skipped"].includes(s.conclusion));
      const skippedAfter = first < 0 ? [] : steps.slice(first + 1).filter((s) => s.conclusion === "skipped" && !/^Post /.test(s.name));
      if (j.conclusion === "failure" && failed.length && failed.every((s) => red.has(s.name)) && !skippedAfter.length) {
        console.log(`  note  CI job ${j.name}: only the known red step failed (${failed.map((s) => s.name).join(", ")}); every other step passed`);
      } else {
        good = false;
        console.log(`  FAIL  CI job ${j.name}: ${j.conclusion}${failed.length ? " (" + failed.map((s) => s.name + ": " + s.conclusion).join(", ") + ")" : ""}`);
      }
    }
    process.exit(good ? 0 : 1);
  ' "$jobs" "$KNOWN_RED" "$CI_MAC_JOB"
}

IN="$WORK/in"
OUT="$WORK/out"
mkdir -p "$IN" "$OUT"
have_all_assets() {
  local names
  names="$(ghr release view "$TAG" -R "$REPO" --json assets -q '.assets[] | select(.state == "uploaded") | .name' 2>/dev/null | sort)"
  [ "$names" = "$(printf '%s\n' "${ASSETS[@]}" | sort)" ]
}

if [ "$RSTATE" != published ]; then
  say "CI build of ${SHA:0:12} ($CI_WF)"
  CI_RUN="$(last_ci_run)"
  if [ -z "$CI_RUN" ]; then
    CI_RUN="$(start_ci_run)"
    echo "    started $CI_WF: https://github.com/$REPO/actions/runs/$CI_RUN"
  fi
  wait_run "$CI_RUN"
  if ! ci_good "$CI_RUN" > "$WORK/ci.txt"; then
    cat "$WORK/ci.txt"
    [ "$(get_state "rerun_$CI_RUN")" != "done" ] || die "$CI_WF failed again after a re-run: https://github.com/$REPO/actions/runs/$CI_RUN"
    echo "    $CI_WF run $CI_RUN: re-running the failed jobs once"
    set_state "rerun_$CI_RUN" "done"
    gh run rerun "$CI_RUN" -R "$REPO" --failed >/dev/null
    sleep 30
    wait_run "$CI_RUN"
    ci_good "$CI_RUN" > "$WORK/ci.txt" || { cat "$WORK/ci.txt"; die "$CI_WF failed: https://github.com/$REPO/actions/runs/$CI_RUN"; }
  fi
  cat "$WORK/ci.txt"
  ok "$CI_WF: https://github.com/$REPO/actions/runs/$CI_RUN"
  set_state ci_run "$CI_RUN"

  if [ "$RSTATE" = draft ] && have_all_assets && [ ! -f "$OUT/$DMG" ]; then
    note "the draft has every file and the work folder has none: verifying the draft"
  else
    # A dmg without a stapled ticket is from a run that stopped in the signing step (the dmg is
    # written before it is signed and notarized): make the files again.
    if [ -f "$OUT/$DMG" ] && [ -f "$OUT/$TGZ" ] && [ -f "$OUT/$TGZ.sig" ] && xcrun stapler validate -q "$OUT/$DMG" >/dev/null 2>&1; then
      note "signed, notarized and packed already (in $OUT)"
    else
      say "Download the CI build"
      [ -f "$IN/$CI_ZIP" ] || ghr run download "$CI_RUN" -R "$REPO" -n "$CI_ARTIFACT" -D "$IN"
      [ -f "$IN/$CI_ZIP" ] || die "the $CI_ARTIFACT artifact has no $CI_ZIP"
      unzip -p "$IN/$CI_ZIP" "$APP.app/Contents/Info.plist" > "$IN/Info.plist"
      [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$IN/Info.plist")" = "$V" ] || die "the CI app is not version $V"
      ok "$CI_ZIP (version $V, $(du -h "$IN/$CI_ZIP" | cut -f1))"

      say "Sign, notarize and staple the Mac app; update bundle; dmg"
      rm -f "$OUT/$DMG" "$OUT/$TGZ" "$OUT/$TGZ.sig" "$OUT/latest.json" "$OUT/latest.json.sig"
      with_update_key "$ROOT/scripts/sign-ci-macos.sh" "$IN/$CI_ZIP" "$OUT"
      [ -f "$OUT/$DMG" ] && [ -f "$OUT/$TGZ" ] && [ -f "$OUT/$TGZ.sig" ] || die "scripts/sign-ci-macos.sh did not make $DMG, $TGZ and $TGZ.sig"
      rm -rf "$IN"
      mkdir -p "$IN"
    fi

    say "Feed and checksums"
    awk -v v="$V" '$0 ~ "^## " v "( |$)" { f = 1; next } /^## / { f = 0 } f' CHANGELOG.md |
      sed -e '/./,$!d' > "$WORK/changes.md"
    [ -s "$WORK/changes.md" ] || die "the CHANGELOG.md section of $V is empty"
    if [ ! -f "$OUT/latest.json" ]; then
      node scripts/updater-latest-json.mjs --version "$V" --base-url "$BASE_URL" \
        --platform "darwin-aarch64=$OUT/$TGZ" --notes-file "$WORK/changes.md" --out "$OUT/latest.json"
      rm -f "$OUT/latest.json.sig"
    fi
    if [ ! -f "$OUT/latest.json.sig" ]; then
      with_update_key "$TAURI" signer sign --app-version "$V" "$OUT/latest.json" >/dev/null
    fi
    [ -s "$OUT/latest.json.sig" ] || die "latest.json was not signed"
    ok "latest.json and latest.json.sig"
    (cd "$OUT" && shasum -a 256 "$DMG" "$TGZ" "$TGZ.sig" latest.json latest.json.sig > SHA256SUMS.txt)
    ok "SHA256SUMS.txt"

    say "Draft release $TAG"
    {
      cat "$WORK/changes.md"
      printf '\n## Download\n\n'
      printf -- '- `%s`: macOS 13 or later, Apple silicon. Signed with Developer ID and notarized by Apple.\n' "$DMG"
      printf -- '- `SHA256SUMS.txt`: the sha256 of each file.\n'
      printf -- '- `%s`, its `.sig`, `latest.json` and `latest.json.sig`: the update files. The app downloads them. You do not need them.\n\n' "$TGZ"
      printf 'Hickeyfield 0.1.2 and later install this version themselves after publik approves the release. '
      printf 'The app checks the signature and the version of every update before it installs it. publik serves the feed at `https://publikhq.com/api/updates/%s/latest.json`.\n\n' "$SLUG"
      printf 'This release has no Windows build.\n\n'
      printf '## Built and tested\n\n'
      printf -- '- macOS app (unsigned, then signed and notarized on the release Mac with `scripts/publik-release.sh`): https://github.com/%s/actions/runs/%s\n' "$REPO" "$CI_RUN"
    } > "$WORK/release-notes.md"
    if [ "$RSTATE" = none ]; then
      (cd "$OUT" && gh release create "$TAG" -R "$REPO" --draft --target "$SHA" --title "$APP $V" \
        --notes-file "$WORK/release-notes.md" "${ASSETS[@]}" >/dev/null)
      RSTATE=draft
      ok "draft created with ${#ASSETS[@]} files"
    else
      DIGESTS="$(ghr release view "$TAG" -R "$REPO" --json assets -q '.assets[] | "\(.name) \(.digest)"')"
      for a in "${ASSETS[@]}"; do
        if printf '%s\n' "$DIGESTS" | grep -qxF "$a sha256:$(sha "$OUT/$a")"; then continue; fi
        gh release upload "$TAG" -R "$REPO" --clobber "$OUT/$a" >/dev/null
        ok "uploaded $a"
      done
      gh release edit "$TAG" -R "$REPO" --notes-file "$WORK/release-notes.md" >/dev/null
      ok "draft up to date"
    fi
  fi
fi

# ---------------------------------------------------------------------------------------------
say "Verify $TAG as published files (downloaded back)"
VER="$WORK/verify"
rm -rf "$VER"
mkdir -p "$VER/prev" "$VER/tgz" "$VER/mnt" "$VER/prevmnt"
ghr release download "$TAG" -R "$REPO" -D "$VER" --clobber >/dev/null
GOT="$(cd "$VER" && find . -maxdepth 1 -type f | sed 's|^\./||' | sort)"
if [ "$GOT" = "$(printf '%s\n' "${ASSETS[@]}" | sort)" ]; then ok "the release has exactly: ${ASSETS[*]}"
else bad "the release has exactly: ${ASSETS[*]}"; echo "        it has: $(echo "$GOT" | tr '\n' ' ')"; fi
expect "SHA256SUMS.txt matches every file it lists" sh -c "cd '$VER' && shasum -a 256 -c SHA256SUMS.txt >/dev/null"
expect "SHA256SUMS.txt lists the dmg and the update bundle" sh -c 'grep -q "  $2\$" "$1/SHA256SUMS.txt" && grep -q "  $3\$" "$1/SHA256SUMS.txt"' _ "$VER" "$DMG" "$TGZ"
git show "$SHA:src-tauri/tauri.conf.json" > "$VER/tauri.conf.json"
git show "$PREV_TAG:src-tauri/tauri.conf.json" > "$VER/tauri.conf.prev.json"
pubkey_of() { node -p 'require(process.argv[1]).plugins?.updater?.pubkey ?? ""' "$1"; }
if [ -n "$(pubkey_of "$VER/tauri.conf.prev.json")" ]; then
  expect "the update public key is the one $PREV_TAG ships (installed copies trust it)" \
    test "$(pubkey_of "$VER/tauri.conf.json")" = "$(pubkey_of "$VER/tauri.conf.prev.json")"
else
  note "$PREV_TAG has no updater: no public key to compare"
fi
if node scripts/updater-latest-json.mjs --check "$VER/latest.json" --config "$VER/tauri.conf.json" --version "$V" --base-url "$BASE_URL" \
  --platform "darwin-aarch64=$VER/$TGZ" 2>&1 | sed 's/^/        /'; then
  ok "latest.json: version $V, absolute URLs, every signature (and latest.json.sig) verifies with the app's public key"
else
  bad "latest.json and its signatures"
fi
expect "latest.json.sig is there" test -s "$VER/latest.json.sig"

# The last release's app, for the designated requirement and the app token: its update bundle, or
# else the app in its dmg.
PREV_APP=""
PREV_TGZ="${APP}_${PREV_TAG#v}_aarch64.app.tar.gz"
if ghr release download "$PREV_TAG" -R "$REPO" -p "$PREV_TGZ" -D "$VER/prev" --clobber >/dev/null 2>&1 && [ -f "$VER/prev/$PREV_TGZ" ]; then
  tar -xzf "$VER/prev/$PREV_TGZ" -C "$VER/prev" && PREV_APP="$VER/prev/$APP.app"
elif ghr release download "$PREV_TAG" -R "$REPO" -p "*.dmg" -D "$VER/prev" --clobber >/dev/null 2>&1; then
  hdiutil attach -nobrowse -noautoopen -readonly -mountpoint "$VER/prevmnt" "$(find "$VER/prev" -name '*.dmg' | head -1)" >/dev/null && PREV_APP="$VER/prevmnt/$APP.app"
fi
[ -d "$PREV_APP" ] && note "compared with the app of $PREV_TAG" || note "$PREV_TAG: no app found to compare with"

# The dmg and the app in it.
SPCTL_DMG="$(spctl -a -vv -t open --context context:primary-signature "$VER/$DMG" 2>&1 || true)"
expect "dmg: Gatekeeper accepts it (Notarized Developer ID)" sh -c 'printf "%s\n" "$1" | grep -q ": accepted" && printf "%s\n" "$1" | grep -q "source=Notarized Developer ID"' _ "$SPCTL_DMG"
expect "dmg: notarization ticket stapled" stapled "$VER/$DMG"
expect "dmg: signed by $IDENTITY" sh -c 'codesign -dvv "$1" 2>&1 | grep -qxF "Authority=$2"' _ "$VER/$DMG" "$IDENTITY"
hdiutil attach -nobrowse -noautoopen -readonly -mountpoint "$VER/mnt" "$VER/$DMG" >/dev/null || die "cannot mount $DMG ($FAILS check(s) failed before)"
trap 'hdiutil detach "$VER/mnt" -quiet 2>/dev/null; hdiutil detach "$VER/prevmnt" -quiet 2>/dev/null; true' EXIT
expect "dmg: Hickeyfield.app and an Applications link" sh -c 'test -d "$1/Hickeyfield.app" && test "$(readlink "$1/Applications")" = /Applications' _ "$VER/mnt"
COPYFILE_DISABLE=1 tar -xzf "$VER/$TGZ" -C "$VER/tgz" || bad "update bundle: tar cannot unpack $TGZ"
expect "update bundle: no AppleDouble (._) files" sh -c '! tar -tzf "$1" | grep -q "/\._"' _ "$VER/$TGZ"
expect "update bundle: one top folder, $APP.app" test "$(tar -tzf "$VER/$TGZ" | cut -d/ -f1 | sort -u)" = "$APP.app"
PREV_DR=""
PREV_TOKENS=""
if [ -d "$PREV_APP" ]; then
  PREV_DR="$(dr "$PREV_APP")"
  PREV_TOKENS="$(token_ids "$PREV_APP/Contents/MacOS/$(plist "$PREV_APP" CFBundleExecutable)")"
fi
FEED_V="$(node -p 'require(process.argv[1]).version' "$VER/latest.json" 2>/dev/null || true)"
for where in "dmg:$VER/mnt/$APP.app" "update bundle:$VER/tgz/$APP.app"; do
  w="${where%%:*}"
  a="${where#*:}"
  info="$(codesign -dvv "$a" 2>&1)"
  BIN="$a/Contents/MacOS/$(plist "$a" CFBundleExecutable)"
  expect "$w app: Info.plist version $(plist "$a" CFBundleShortVersionString) (build $(plist "$a" CFBundleVersion)) = feed version $FEED_V = $V" \
    test "$(plist "$a" CFBundleShortVersionString)" = "$V" -a "$(plist "$a" CFBundleVersion)" = "$V" -a "$FEED_V" = "$V"
  expect "$w app: bundle id $BUNDLE_ID" test "$(plist "$a" CFBundleIdentifier)" = "$BUNDLE_ID"
  expect "$w app: codesign --verify --strict --deep" codesign --verify --strict --deep "$a"
  expect "$w app: $IDENTITY, team $TEAM, hardened runtime" sh -c 'printf "%s\n" "$1" | grep -qxF "Authority=$2" && printf "%s\n" "$1" | grep -qxF "TeamIdentifier=$3" && printf "%s\n" "$1" | grep -q "flags=.*runtime"' _ "$info" "$IDENTITY" "$TEAM"
  expect "$w app: no entitlements (src-tauri/Entitlements.plist is empty)" sh -c '! codesign -d --entitlements - --xml "$1" 2>/dev/null | grep -q "com.apple.security"' _ "$a"
  expect "$w app: FFmpeg sidecar signed by $IDENTITY with hardened runtime" sh -c 'i="$(codesign -dvv "$1/Contents/MacOS/ffmpeg" 2>&1)"; printf "%s\n" "$i" | grep -qxF "Authority=$2" && printf "%s\n" "$i" | grep -q "flags=.*runtime"' _ "$a" "$IDENTITY"
  expect "$w app: notarization ticket stapled" stapled "$a"
  SPCTL_APP="$(spctl -a -vv -t exec "$a" 2>&1 || true)"
  expect "$w app: Gatekeeper accepts it (Notarized Developer ID)" sh -c 'printf "%s\n" "$1" | grep -q ": accepted" && printf "%s\n" "$1" | grep -q "source=Notarized Developer ID"' _ "$SPCTL_APP"
  expect "$w app: designated requirement as in $PREV_TAG (TCC grants carry over)" test -n "$PREV_DR" -a "$(dr "$a")" = "$PREV_DR"
  TOKENS="$(token_ids "$BIN")"
  expect "$w app: the publik app token is compiled in, the same as in $PREV_TAG (sha256 ${TOKENS:0:12}…)" \
    test -n "$TOKENS" -a "$(printf '%s\n' "$TOKENS" | wc -l | tr -d ' ')" = 1 -a "$TOKENS" = "$PREV_TOKENS"
done
CD_DMG="$(codesign -dvvv "$VER/mnt/$APP.app" 2>&1 | sed -n 's/^CDHash=//p' || true)"
CD_TGZ="$(codesign -dvvv "$VER/tgz/$APP.app" 2>&1 | sed -n 's/^CDHash=//p' || true)"
expect "the update bundle is the app of the dmg (CDHash $CD_DMG)" test -n "$CD_DMG" -a "$CD_DMG" = "$CD_TGZ"
echo "  designated requirement: $(dr "$VER/tgz/$APP.app")"
hdiutil detach "$VER/mnt" -quiet
hdiutil detach "$VER/prevmnt" -quiet 2>/dev/null || true
trap - EXIT
if [ "$RSTATE" = draft ]; then
  expect "the draft targets ${SHA:0:12}" test "$(ghr api "repos/$REPO/releases" -q ".[] | select(.tag_name == \"$TAG\") | .target_commitish")" = "$SHA"
fi
rm -rf "$VER/prev" "$VER/tgz"
if [ "$RSTATE" = draft ]; then [ "$FAILS" -eq 0 ] || die "$FAILS check(s) failed. $TAG stays a draft: fix the cause and run the command again."; fi
[ "$FAILS" -eq 0 ] || die "$FAILS check(s) failed on the published $TAG."
ok "every check passed"

# ---------------------------------------------------------------------------------------------
if [ "$RSTATE" = draft ]; then
  if [ "${PUBLIK_RELEASE_NO_PUBLISH:-0}" = 1 ]; then
    say "Not published (PUBLIK_RELEASE_NO_PUBLISH=1): https://github.com/$REPO/releases (draft $TAG)"
    exit 0
  fi
  say "Publish $TAG"
  gh release edit "$TAG" -R "$REPO" --draft=false --latest >/dev/null
  git fetch -q origin tag "$TAG"
  expect "tag $TAG points at ${SHA:0:12}" test "$(git rev-parse "$TAG^{commit}")" = "$SHA"
  expect "the release is public" test "$(ghr release view "$TAG" -R "$REPO" --json isDraft -q .isDraft)" = false
  for url in $(node -p 'Object.values(require(process.argv[1]).platforms).map((p) => p.url).join("\n")' "$VER/latest.json") \
    "${BASE_URL}latest.json" "${BASE_URL}latest.json.sig" "${BASE_URL}$DMG"; do
    code="$(curl -s -o /dev/null -L -r 0-0 -w '%{http_code}' "$url")"
    expect "$url answers ($code)" test "$code" = 200 -o "$code" = 206
  done
  [ "$FAILS" -eq 0 ] || die "$FAILS check(s) failed after publishing $TAG: look at https://github.com/$REPO/releases/tag/$TAG"
  rm -rf "$WORK"
  say "Published: https://github.com/$REPO/releases/tag/$TAG"
  echo "publik takes the release in at its next daily intake (10:00 UTC) and then serves its"
  echo "latest.json at https://publikhq.com/api/updates/$SLUG/latest.json."
else
  rm -rf "$VER"
  say "$TAG is published and passes every check: https://github.com/$REPO/releases/tag/$TAG"
fi
