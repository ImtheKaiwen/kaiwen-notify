# Releasing

The updater needs a signing key pair. **A human must create and store it**; the repo never contains the private key.

1. Generate once: `npm run tauri signer generate -- -w kaiwen-notify.key` (choose a password). Keep the file safe, never commit it.
2. GitHub repo, Settings, Secrets and variables, Actions:
   - secret `TAURI_SIGNING_PRIVATE_KEY` = contents of the private key file
   - secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = its password
   - variable `UPDATER_PUBKEY` = contents of `kaiwen-notify.key.pub`
3. Bump the version in `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` (tests enforce they match) and update `CHANGELOG.md`.
4. Push tag `vX.Y.Z` (or run the **Release (draft)** workflow). It fails early if the secret/variable is missing, builds the signed NSIS installer, and uploads installer, `.sig` and `latest.json` to a **draft** release.
5. Review the draft and publish it manually. Running apps read `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`.

Losing the private key means installed apps can no longer verify updates; they must be reinstalled manually. Keep a backup.
The endpoint is derived from the repository name, so a private repository cannot serve updates to anonymous clients.
