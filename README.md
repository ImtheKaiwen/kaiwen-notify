# Kaiwen Notify

Lightweight Windows desktop utility that plays a sound when your AI coding agents finish their tasks.

Works with **Claude Code**, **Codex** (CLI and Desktop) and **Antigravity**. Built with Tauri 2, Rust and vanilla TypeScript. About 3 MB installer, no background polling.

## Features

- Sound notification when an agent turn completes (bundled WAV or your own).
- One-click setup: **Entegrasyonları Kur** installs hooks for each tool and shows real per-provider progress.
- Safe removal: **Entegrasyonları Kaldır** removes only Kaiwen Notify's own hooks and backs up every file it edits.
- **Eksikleri kur / Onar** repairs a partial or outdated setup.
- Minimal black-and-white UI, light/dark/system theme.
- Lives in the system tray. Closing the window destroys the WebView, so tray-only memory stays low.
- Signed auto-updates through GitHub Releases (see [Updates](#updates)).
- Single instance, per-user installer, no admin rights.

## Supported tools

| Tool | Mechanism | Config file touched |
|---|---|---|
| Claude Code | `Stop` hook | `~/.claude/settings.json` |
| Codex CLI / Desktop | independent `Stop` hook | `~/.codex/hooks.json` (its TOML config and `notify` setting are never touched) |
| Antigravity | named `Stop` hook group | `~/.gemini/config/hooks.json` |

Existing hooks are preserved. Agents may need a restart to load new hooks. Codex asks you to trust a new hook (`/hooks` in the CLI, Settings > Hooks in Desktop).

## Install

Download `Kaiwen Notify_<version>_x64-setup.exe` from the [Releases](https://github.com/ImtheKaiwen/kaiwen-notify/releases) page and run it. Requires Windows 10/11 with the WebView2 runtime (included in Windows 11). Then open the app and press **Entegrasyonları Kur**.

## Change the notification sound

Settings, **Bildirim sesi**, **Değiştir**, then pick a `.wav` file (PCM, under 10 MB). **Bildirimi test et** plays it; **Varsayılana dön** restores the bundled sound.

## Remove the integrations

Press **Entegrasyonları Kaldır** and confirm. Only the hooks Kaiwen Notify added are removed; backups stay next to the edited files (`*.kaiwen-backup-*`). Do this before uninstalling the app, because removing the app from Windows Settings does not revert hook files.

## Updates

Uses the official Tauri updater with signature verification always on.

1. **Güncellemeleri Kontrol Et**, or an automatic check about 10 seconds after start (at most every 12 hours, can be switched off).
2. **İndir** downloads with real byte progress and verifies the signature.
3. **Kur ve Yeniden Başlat** is the only step that starts the installer; Windows then relaunches the app.

Release builds read `https://github.com/ImtheKaiwen/kaiwen-notify/releases/latest/download/latest.json`. Local and CI test builds without the signing key have the updater disabled. Release process: [docs/RELEASING.md](docs/RELEASING.md).

## How it works

Agent hook, then a tiny PowerShell bridge (`%LOCALAPPDATA%\KaiwenNotify\hooks\send-event.ps1`), then a one-way UDP datagram to `127.0.0.1:47451`, then the Rust listener plays the WAV through WinMM. Duplicate events from the same source within 650 ms are ignored. Antigravity events are skipped while `fullyIdle` is false or when the stop reason is an error or step-limit stop.

Diagnostics: create an empty file `%LOCALAPPDATA%\KaiwenNotify\diag.enabled` and each hook call appends metadata (no prompts or paths) to `diag.log` in the same folder. Delete the file to turn it off.

## Development

Prerequisites: Windows, Node.js 22+, Rust (MSVC) and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

```powershell
npm install
npm run desktop:dev        # run the app
npm run desktop:build      # NSIS installer in src-tauri\target\release\bundle\nsis\
```

Checks (all run in CI):

```powershell
npm run build                                                   # tsc + vite
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
python -m pytest tests -q   # contracts + hook scripts against a throw-away profile
```

## Security and known limitations

See [docs/SECURITY.md](docs/SECURITY.md). In short:

- The UDP listener only binds `127.0.0.1` but has no sender authentication; a local process can trigger the sound. Nothing privileged is reachable through it.
- Hook commands run with `-ExecutionPolicy Bypass` for the app's own bundled script.
- Windows only. Notifications mean "agent turn ended", not "task succeeded".
- Provider logos belong to their owners and are used only to identify the supported tools.
- Not yet verified end to end: Antigravity real-task notification (see CHANGELOG).

## License

No license has been chosen yet; all rights reserved until the repository owner adds one.
