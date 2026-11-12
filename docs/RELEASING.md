# Releasing

One-time setup (already done for this repository; keep the key safe):

- Key pair: `~/.kaiwen-notify-signing/` on the maintainer's machine (`kaiwen-notify.key`, `kaiwen-notify.key.pub`, `password.txt`).
  **Back this folder up** (password manager or offline drive). If it is lost, installed apps can never verify
  future updates and everyone has to reinstall manually. Never commit it.
- GitHub Actions: secrets `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; variable `UPDATER_PUBKEY`
  (the public key, not secret). The app embeds that public key at build time and refuses unsigned or wrongly signed updates.

Cutting a release:

1. Bump the version in `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` and add a `## X.Y.Z` section to `CHANGELOG.md`.
   `python scripts/release_tools.py check-versions X.Y.Z` verifies all four.
2. Merge to `main` once CI is green, then push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. The **Release** workflow validates versions, builds the NSIS installer, signs the updater artifacts, creates a **draft**
   release with `latest.json`, verifies every asset (`verify-assets`), publishes it, and verifies the public endpoint
   `https://github.com/ImtheKaiwen/kaiwen-notify/releases/latest/download/latest.json` (`verify-endpoint`).
   Manual re-runs use *Run workflow* with `publish` checked.
4. Draft releases and pre-releases are not served by `releases/latest`, so apps only see versions that were published.

Notes: the installer is not Authenticode-signed (SmartScreen may warn); the updater signature is separate and always verified.
Installers older than the first updater-enabled release cannot self-update.
