# Changelog

Each release has a `## <version>` section. `scripts/publik-release.sh` puts the section in the
release notes and in the update feed (`latest.json`), so the app shows it with the update.
Older releases: https://github.com/Blueturboguy07/hickeyfield/releases

## 0.1.4

After you sign in with publik, the app opens at once. The "Choose how to pay for AI" screen is gone.

- Your provider keys work as before. Hickeyfield makes images and videos with them.
- Settings still has "Use my own keys".

## 0.1.3

No change in the app. Hickeyfield 0.1.3 is the same app as 0.1.2.

- This is the first version that reaches you through the self-updater: Hickeyfield 0.1.2 finds it, checks its signature, installs it, and asks "Restart" or "Later".
- It is the first release made with one command (`scripts/publik-release.sh`).

## 0.1.2

Hickeyfield now updates itself.

- The app checks for a new version 10 seconds after it opens, and then once a day.
- "Check for Updates…" in the app menu checks at once.
- The app installs only an update that is signed with the Hickeyfield update key. It never installs an older version.
- When an update is ready, the app asks "Restart" or "Later".
- Tauri 2.11 → 2.12.
