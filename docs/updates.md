# Updates

Hickeyfield updates itself with `tauri-plugin-updater`. Every update package is signed with the
publik update key. The app checks each package against the public key in
`src-tauri/tauri.conf.json` before it installs anything. The code and the proof are copied from
GSpot (`Blueturboguy07/gspot`, `docs/updates.md`).

## How it works

- The app reads `https://publikhq.com/api/updates/hickeyfield/latest.json`. publik serves the
  `latest.json` asset of the scan-approved release tag, byte for byte. publik does not sign or
  change it, so the release `latest.json` must be complete and correct. The route answers 404
  until a scan-approved release carries a `latest.json` asset. A 404 is silent in the background
  check, and a manual check shows it as an error.
- The app checks 10 s after launch and then once a day. "Check for Updates…" in the app menu
  (after "About Hickeyfield") checks at once. `PUBLIK_NO_AUTO_UPDATE=1` turns off the automatic
  checks (the CI workflows set it).
- macOS: the app downloads the new `Hickeyfield.app`, verifies it, puts it in place of the old
  one, and asks "Restart" or "Later". If the user cannot write to the app's folder, the app does
  not replace it in the background. It asks "Install and Restart" or "Later", and macOS asks for
  an admin password only after "Install and Restart".
- The signature must also name the version that `latest.json` announces
  (`requireSignedVersion`). The app never installs an older version.
- A failed automatic check goes to the log only. A failed manual check shows the error.
- Releases ship macOS arm64 only, so the feed has `darwin-aarch64` only. The code also has the
  Windows path (passive installer), but no Windows release exists, so it is not proven here.
- Code: `src-tauri/src/updates.rs`. Proof: `.github/workflows/updater-proof.yml`.

## The key

| Where | What |
|---|---|
| Release Mac, login keychain, service `publik-update-key` | `mann-private` (private key), `mann-password` (its password), `mann-public` (public key) |
| GitHub secrets on this repo | `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (used by `updater-proof.yml` only) |
| `src-tauri/tauri.conf.json` | `plugins.updater.pubkey` (public, key ID `F5E0515C77912BD0`) |

This is the same key as GSpot. Keep a backup of the private key and its password. Without them,
installed copies can never update again.

## Cut a release

1. Set the new version in `src-tauri/tauri.conf.json`, the workspace `Cargo.toml` and
   `ui/package.json`. Push.
2. On the release Mac, build, sign, notarize and pack. The update key comes from the keychain:
   ```sh
   RELEASE_NOTES_FILE=notes.md scripts/build-macos.sh --target aarch64-apple-darwin
   ```
   After the app is stapled, the script writes `target/aarch64-apple-darwin/release/bundle/updater/`:
   `Hickeyfield_<v>_aarch64.app.tar.gz`, its `.sig`, and `latest.json`. The `.app.tar.gz` is the
   stapled Developer ID app, so an update keeps the TCC permissions. The feed script refuses a
   signature that does not verify or that names another version.
3. If you sign the CI app (`Hickeyfield-macos-arm64-app` artifact) by hand instead, make the same
   files from the stapled app:
   ```sh
   scripts/updater-pack-macos.sh Hickeyfield.app out
   node scripts/updater-latest-json.mjs --version <v> \
     --base-url https://github.com/Blueturboguy07/hickeyfield/releases/download/v<v>/ \
     --platform darwin-aarch64=out/Hickeyfield_<v>_aarch64.app.tar.gz \
     --notes-file notes.md --out out/latest.json
   ```
4. Make a draft release with the dmg, `SHA256SUMS.txt`, and the three update files. Keep the
   names of the update files: `latest.json` points at the `.app.tar.gz` by name.
   ```sh
   gh release create v<v> --draft --title "Hickeyfield <v>" --notes-file notes.md \
     <dmg> SHA256SUMS.txt \
     out/Hickeyfield_<v>_aarch64.app.tar.gz out/Hickeyfield_<v>_aarch64.app.tar.gz.sig out/latest.json
   ```
5. Run `scripts/verify-macos.sh --require-notarized`. Publish the release. Installed copies
   update when publik approves the release and serves its `latest.json`.

Copies of 0.1.1 and older have no updater. Their users must install the first release with the
updater by hand.

`tauri build` without the key fails when `createUpdaterArtifacts` is on. Every build in this repo
turns it off (`ci.yml`, `scripts/build-macos.sh`), because each of them is either unsigned or is
packed after stapling. For a local build, add
`--config '{"bundle":{"createUpdaterArtifacts":false}}'`.

## Roll back

The app installs only a version higher than its own. You cannot push an older version.

1. Stop the bad release at once: withdraw it in publik, or delete its `latest.json` asset.
   Copies that did not update yet stay on the old version.
2. Revert the bad change, give it a higher version (for example 0.1.4 after a bad 0.1.3), and
   cut a release. Copies on the bad version update to it.

Do not turn on `allowDowngrades`. It removes the only guard against an old, signed release
served as a new one.

## Rotate the key

The key is shared with GSpot. Rotate it for both apps together. Follow "Rotate the key" in
GSpot's `docs/updates.md`: put the new public key in `tauri.conf.json`, release that version
signed with the OLD key, and sign with the new key from the next release on.

## Test

Run `updater-proof.yml` (workflow_dispatch, or a push to `feat/updater` or `updater/**`). It
builds 0.0.1 and 0.0.2 on `macos-latest`, signs both with one throwaway certificate (CI has no
Developer ID), installs 0.0.1, and checks that 0.0.1 refuses three bad feeds and then updates
to 0.0.2 with the same bundle id, certificate and designated requirement. Then it starts 0.0.1
once more and checks that the restart question is on screen while the app keeps running.

The app has a test hook for this: `PUBLIK_UPDATE_SELF_TEST=version|check|install` runs the
updater with no window, no sign-in gate and no prompt, and prints one JSON line. `prompt` runs
the background path and shows the real question. The hook does not click the buttons.
