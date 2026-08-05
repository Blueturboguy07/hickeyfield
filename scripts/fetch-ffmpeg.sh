#!/usr/bin/env bash
#
# Fetches the static FFmpeg sidecars Halation bundles, one per target triple,
# into src-tauri/binaries/ with the names Tauri's `externalBin` resolver wants.
#
# Why a script and not a committed binary: a 50-100 MB per-platform blob in git
# history is both unpleasant and unauditable. The provenance of what we ship has
# to be reproducible from a URL, a checksum and a recorded set of configure
# flags — which is exactly what this file is. .gitignore already excludes
# src-tauri/binaries/*.
#
# ## The licence gate is the point of this script
#
# Default FFmpeg is LGPL-2.1+. `--enable-gpl` (needed for libx264) makes it
# GPL-2.0+, which is compatible with Halation's AGPL-3.0-or-later: FFmpeg's "or
# later" reaches GPL-3, and AGPL-3 §13 permits the combination explicitly.
#
# `--enable-nonfree` makes a build **legally unredistributable**. Such a binary
# is indistinguishable from a legal one in normal use — it encodes, it decodes,
# it passes every functional test. The only way to notice is to ask it, so this
# script asks, and refuses to install a build that answers wrong. The same
# assertion runs against the installed sidecar in CI.
#
# Obligations that come with shipping GPL FFmpeg, and where they are met:
#   * record the exact configure flags     -> --print-notice, pasted into NOTICE
#   * ship FFmpeg's licence text           -> LICENSES/GPL-2.0.txt
#   * a written offer for the source       -> NOTICE
#
# ## Two further gates that are not about licensing
#
#   * libx264 must be present, or there is no software encoder to fall back to.
#   * libfreetype must be present, or `drawtext` does not exist and every
#     burned-in caption silently fails at render time. This is not theoretical:
#     Homebrew's own ffmpeg 8.1.2 is built without it, so a developer testing
#     against PATH will not reproduce what a user sees.
#
# Usage:
#   scripts/fetch-ffmpeg.sh                    # the host's own target
#   scripts/fetch-ffmpeg.sh --all              # every shipping target
#   scripts/fetch-ffmpeg.sh --target x86_64-pc-windows-msvc
#   scripts/fetch-ffmpeg.sh --check            # verify what is already there
#   scripts/fetch-ffmpeg.sh --print-notice     # emit the NOTICE stanza
#   scripts/fetch-ffmpeg.sh --update-checksums # print new pins after a re-pin
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
DEST="$REPO_ROOT/src-tauri/binaries"
CACHE="${HALATION_FFMPEG_CACHE:-${TMPDIR:-/tmp}/halation-ffmpeg-cache}"

# --------------------------------------------------------------------------
# Sources. One row per target triple:
#   url | archive sha256 | member inside the archive | binary sha256
#
# Both hashes are checked. The archive hash catches a corrupted or substituted
# download; the *binary* hash is the one that actually matters, because it
# survives an upstream re-zip and is what the vendors publish.
#
# macOS arm64 — osxexperts.net, FFmpeg 8.1, genuinely static (only system
#   frameworks in `otool -L`). Homebrew is not an option: its builds are
#   dynamic, and dylib resolution fails once the binary is inside an .app
#   bundle (Tauri discussion #9029).
#
# Windows x64 — BtbN/FFmpeg-Builds, the `-gpl` variant. Do NOT switch to a
#   "full" build from another vendor without reading its configure line: several
#   popular Windows builds include libfdk-aac, which requires --enable-nonfree.
#   The `-shared` variants are also wrong for us — same dylib problem as above.
#
# NOTE ON PINNING: BtbN publishes to a rolling `latest` tag, so the Windows
# archive hash changes whenever they rebuild. A mismatch is therefore expected
# from time to time and is not automatically an attack — but it must be a
# deliberate human re-pin (review the new configure line, then
# --update-checksums), never an automatic "just take whatever is there".
# --------------------------------------------------------------------------
TARGETS="aarch64-apple-darwin x86_64-pc-windows-msvc"

source_for() {
  case "$1" in
    aarch64-apple-darwin)
      URL="https://www.osxexperts.net/ffmpeg81arm.zip"
      ARCHIVE_SHA="ebb82529562b71170807bbc6b0e7eb4f0b13af8cbb0e085bb9e8f6fe709598ad"
      MEMBER="ffmpeg"
      BINARY_SHA="9a08d61f9328e8164ba560ee7a79958e357307fcfeea6fe626b7d66cdc287028"
      SUFFIX=""
      ;;
    x86_64-pc-windows-msvc)
      # A **dated** autobuild tag, not `latest`.
      #
      # BtbN rebuilds `latest` in place, so pinning there guarantees the
      # checksum breaks every few days — and a gate that cries wolf gets
      # bypassed, which is worse than no gate. The dated tags are immutable.
      URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-04-21-26/ffmpeg-n8.1.2-34-g9b6c8969e0-win64-gpl-8.1.zip"
      ARCHIVE_SHA="b7f08f5b4975e6ceecb9785584e559cfed0968fae701db92abd968e7a2ae0402"
      MEMBER="ffmpeg-n8.1.2-34-g9b6c8969e0-win64-gpl-8.1/bin/ffmpeg.exe"
      BINARY_SHA="1c399e61a900e08bf22e7308b99c5a5510d3d47261b5a97029d1b0de1ba770c3"
      SUFFIX=".exe"
      ;;
    *)
      echo "no source recorded for target: $1" >&2
      echo "add one to source_for() rather than fetching by hand." >&2
      exit 2
      ;;
  esac
}

host_target() {
  local arch os
  arch="$(uname -m)"
  [ "$arch" = "arm64" ] && arch="aarch64"
  os="$(uname -s)"
  case "$os" in
    Darwin) echo "$arch-apple-darwin" ;;
    MINGW*|MSYS*|CYGWIN*) echo "$arch-pc-windows-msvc" ;;
    Linux) echo "$arch-unknown-linux-gnu" ;;
    *) echo "unsupported host: $os" >&2; exit 2 ;;
  esac
}

sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    sha256sum "$1" | cut -d' ' -f1
  fi
}

# The `configuration:` line, however we can get at it.
#
# A Windows .exe cannot be executed on a macOS or Linux CI runner, so fall back
# to reading the string out of the binary. FFmpeg stores its configure line as a
# static string, so `strings` finds it without running anything — which is also
# the safer way to interrogate a binary you have just downloaded.
configure_line() {
  local bin="$1"
  if [ -x "$bin" ] && "$bin" -hide_banner -version >/dev/null 2>&1; then
    "$bin" -hide_banner -version | sed -n 's/^ *configuration: *//p'
  else
    strings -a "$bin" 2>/dev/null | grep -m1 -- '--enable-gpl' || true
  fi
}

# Refuse anything we are not allowed to ship, or that would break at render time.
assert_usable() {
  local bin="$1" target="$2" cfg
  cfg="$(configure_line "$bin")"
  if [ -z "$cfg" ]; then
    echo "  FAIL  could not read the configure line out of $bin" >&2
    exit 1
  fi

  case "$cfg" in
    *--enable-nonfree*)
      echo "  FAIL  $target is a --enable-nonfree build." >&2
      echo "        Such a binary is legally unredistributable and must not be" >&2
      echo "        bundled, published, or committed. Pick a different source." >&2
      exit 1 ;;
  esac
  case "$cfg" in
    *--enable-gpl*) ;;
    *) echo "  FAIL  $target is not a GPL build; libx264 will be absent." >&2; exit 1 ;;
  esac
  case "$cfg" in
    *--enable-libx264*) ;;
    *) echo "  FAIL  $target has no libx264 — nothing to fall back to when" >&2
       echo "        hardware encoding is unavailable." >&2; exit 1 ;;
  esac
  # Captions are burned in with drawtext, which does not exist without freetype.
  case "$cfg" in
    *--enable-libfreetype*) ;;
    *) echo "  FAIL  $target has no libfreetype — drawtext is missing and every" >&2
       echo "        burned-in caption would fail at render time." >&2; exit 1 ;;
  esac

  # Only meaningful when the binary can actually run here; `ffmpeg -L` is what
  # halation_core::ffmpeg::licence_check asserts at runtime and in CI.
  if [ -x "$bin" ] && "$bin" -hide_banner -L >/dev/null 2>&1; then
    local lic
    lic="$("$bin" -hide_banner -L)"
    case "$lic" in
      *nonfree*|*"not legally redistributable"*)
        echo "  FAIL  $target reports non-free parts via -L" >&2; exit 1 ;;
    esac
    case "$lic" in
      *"Lesser General Public License"*)
        echo "  FAIL  $target reports LGPL via -L, expected GPL" >&2; exit 1 ;;
      *"General Public License"*) ;;
      *) echo "  FAIL  $target reports an unrecognised licence via -L" >&2; exit 1 ;;
    esac
  fi

  echo "  ok    GPL, not nonfree, libx264 + libfreetype present"
}

fetch_one() {
  local target="$1"
  source_for "$target"
  local out="$DEST/ffmpeg-$target$SUFFIX"
  local archive="$CACHE/$(basename "$URL")"

  echo "$target"
  mkdir -p "$CACHE" "$DEST"

  if [ -f "$archive" ] && [ "$(sha256_of "$archive")" = "$ARCHIVE_SHA" ]; then
    echo "  cached $archive"
  else
    echo "  get   $URL"
    curl --fail --location --progress-bar --output "$archive.part" "$URL"
    mv "$archive.part" "$archive"
  fi

  local got
  got="$(sha256_of "$archive")"
  if [ "$got" != "$ARCHIVE_SHA" ]; then
    echo "  WARN  archive sha256 mismatch for $target" >&2
    echo "          expected $ARCHIVE_SHA" >&2
    echo "          got      $got" >&2
    echo "        BtbN rebuilds its rolling 'latest' tag, so this is expected" >&2
    echo "        occasionally — but re-pin deliberately: read the new configure" >&2
    echo "        line, then run --update-checksums. Never auto-accept." >&2
    if [ "${HALATION_ALLOW_REPIN:-0}" != "1" ]; then
      echo "        Set HALATION_ALLOW_REPIN=1 to proceed once you have reviewed it." >&2
      exit 1
    fi
  fi

  rm -rf "$CACHE/x-$target"
  mkdir -p "$CACHE/x-$target"
  unzip -q -o "$archive" "$MEMBER" -d "$CACHE/x-$target"
  cp "$CACHE/x-$target/$MEMBER" "$out"
  chmod +x "$out"

  got="$(sha256_of "$out")"
  if [ -n "$BINARY_SHA" ] && [ "$got" != "$BINARY_SHA" ]; then
    echo "  WARN  binary sha256 mismatch: expected $BINARY_SHA, got $got" >&2
    [ "${HALATION_ALLOW_REPIN:-0}" = "1" ] || exit 1
  fi
  echo "  sha   $got  (as published)"

  # macOS kills a quarantined, unsigned binary on Apple Silicon before it can
  # print anything, and the error the caller sees is "killed: 9" with no
  # explanation. `tauri build` re-signs the sidecar with the Developer ID
  # identity later; the ad-hoc signature is only to make it runnable in dev.
  #
  # Signing REWRITES the file, so the on-disk hash stops matching the published
  # one from here on. That is why the pin is checked above, before signing, and
  # why the provenance record quotes the published hash rather than re-hashing
  # whatever is sitting in this directory.
  if [ "$(uname -s)" = "Darwin" ] && [ "$SUFFIX" = "" ]; then
    xattr -dr com.apple.quarantine "$out" 2>/dev/null || true
    if [ "$(uname -m)" = "arm64" ]; then
      if codesign --force --sign - "$out" >/dev/null 2>&1; then
        echo "  sign  ad-hoc (on-disk hash now differs from the published one)"
      else
        echo "  WARN  ad-hoc signing failed; the binary may be killed on launch" >&2
      fi
    fi
  fi

  assert_usable "$out" "$target"
  echo "  ->    $out"
}

check_one() {
  local target="$1"
  source_for "$target"
  local out="$DEST/ffmpeg-$target$SUFFIX"
  echo "$target"
  if [ ! -f "$out" ]; then
    echo "  FAIL  missing $out — run scripts/fetch-ffmpeg.sh" >&2
    exit 1
  fi
  local got
  got="$(sha256_of "$out")"
  echo "  sha   $got"
  # An exact match is only expected where the binary was not re-signed after
  # download; see the note in fetch_one. The licence and feature gates below
  # are what actually has to hold, and they read the binary itself.
  if [ -n "$BINARY_SHA" ] && [ "$got" != "$BINARY_SHA" ]; then
    echo "  note  differs from the published $BINARY_SHA (expected after ad-hoc signing)"
  fi
  assert_usable "$out" "$target"
}

print_notice() {
  cat <<'HDR'
FFmpeg — GNU General Public License
  (see LICENSES/GPL-2.0.txt and LICENSES/GPL-3.0.txt)

  The two builds are not under the same version of the GPL, which the entries
  below state per target: --enable-gpl alone gives GPL-2.0-or-later, while
  --enable-gpl --enable-version3 gives GPL-3.0-or-later. Both are compatible
  with Halation's AGPL-3.0-or-later, and both licence texts must ship.

  Halation bundles an unmodified static FFmpeg binary per platform, fetched by
  scripts/fetch-ffmpeg.sh. It is dynamically invoked as a separate process and
  is not linked into Halation.

  Written offer: the complete corresponding source for the FFmpeg builds below,
  including the configure invocation, is available for three years from the
  date of distribution. Write to the address in README.md, or fetch it from
  https://github.com/FFmpeg/FFmpeg at the tagged release named in each entry.

  None of these builds is configured with --enable-nonfree.

HDR
  for target in $TARGETS; do
    source_for "$target"
    local out="$DEST/ffmpeg-$target$SUFFIX" cfg=""
    [ -f "$out" ] && cfg="$(configure_line "$out")"
    echo "  $target"
    echo "    source:    $URL"
    # The published hash, not a re-hash of the local file: on Apple Silicon the
    # local copy has been ad-hoc signed and no longer matches what the vendor
    # published, and it is the vendor's bytes the provenance record is about.
    if [ -n "$BINARY_SHA" ]; then
      echo "    sha256:    $BINARY_SHA  (as published)"
    else
      echo "    sha256:    $(if [ -f "$out" ]; then sha256_of "$out"; else echo '(not fetched)'; fi)"
    fi
    if [ -n "$cfg" ]; then
      case "$cfg" in
        *--enable-version3*) echo "    licence:   GPL-3.0-or-later (--enable-version3)" ;;
        *)                   echo "    licence:   GPL-2.0-or-later" ;;
      esac
      echo "    configure:"
      # One flag per line: a 600-character single line in NOTICE is not a
      # record anybody can actually check.
      for flag in $cfg; do echo "      $flag"; done
    else
      echo "    configure: (not fetched — run scripts/fetch-ffmpeg.sh --all)"
    fi
    echo
  done
}

update_checksums() {
  echo "Paste these into source_for() after reviewing the configure lines:"
  echo
  for target in $TARGETS; do
    source_for "$target"
    local out="$DEST/ffmpeg-$target$SUFFIX"
    local archive="$CACHE/$(basename "$URL")"
    echo "  $target"
    [ -f "$archive" ] && echo "    ARCHIVE_SHA=\"$(sha256_of "$archive")\""
    [ -f "$out" ] && echo "    BINARY_SHA=\"$(sha256_of "$out")\""
  done
}

MODE="fetch"
WANTED=""
while [ $# -gt 0 ]; do
  case "$1" in
    --all) WANTED="$TARGETS"; shift ;;
    --target) WANTED="$2"; shift 2 ;;
    --check) MODE="check"; shift ;;
    --print-notice) MODE="notice"; shift ;;
    --update-checksums) MODE="update"; shift ;;
    -h|--help) sed -n '2,45p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$WANTED" ] || WANTED="$(host_target)"

case "$MODE" in
  notice) print_notice ;;
  update) update_checksums ;;
  check)  for t in $WANTED; do check_one "$t"; done ;;
  fetch)
    for t in $WANTED; do fetch_one "$t"; done
    echo
    echo "Done. Run --print-notice and paste the result into NOTICE before release."
    ;;
esac
