# Kaiwen Notify

Lightweight Windows desktop utility that plays a sound when your AI coding agents finish their tasks.

Works with **Claude Code**, **Codex** (CLI and Desktop) and **Antigravity**. Built with Tauri 2, Rust and vanilla TypeScript. English and Turkish UI. About 3.3 MB installer.

[![CI](https://github.com/ImtheKaiwen/kaiwen-notify/actions/workflows/ci.yml/badge.svg)](https://github.com/ImtheKaiwen/kaiwen-notify/actions/workflows/ci.yml) License: [MIT](LICENSE)

## Features

- Sound notification when an agent turn completes (original bundled chime, or your own WAV).
- One-click setup: **Install integrations** (TR: *Entegrasyonları Kur*) installs hooks per tool and shows real per-provider progress.
- Safe removal: **Remove integrations** (*Entegrasyonları Kaldır*) deletes only Kaiwen Notify's own hooks and backs up every file it edits. **Install missing / Repair** (*Eksikleri kur / Onar*) fixes a partial or outdated setup.
- **English / Türkçe** UI. Follows the Windows display language by default, can be switched in Settings instantly, and the choice is remembered.
- **Launch at startup** (off by default): starts quietly in the tray, no window, no splash.
- Short transparent splash animation on a normal launch (skipped for startup launches, tray reopen and the restart after an update).
- Signed auto-updates through GitHub Releases with a background check every 10 minutes (never downloads or installs without you).
- Minimal black-and-white UI, light/dark/system theme.
- Lives in the system tray. Closing the window destroys the WebView, so tray-only memory stays low.
- Single instance, per-user installer, no admin rights.

## Supported tools

| Tool | Mechanism | Config file touched |
|---|---|---|
| Claude Code | `Stop` hook | `~/.claude/settings.json` |
| Codex CLI / Desktop | independent `Stop` hook | `~/.codex/hooks.json` (its TOML config and `notify` setting are never touched) |
| Antigravity | named `Stop` hook group | `~/.gemini/config/hooks.json` |

Antigravity's Desktop App, IDE and CLI share the one global hooks file but are separate runtimes. The **Integrations** card shows per runtime whether the hook has *really run* (a hook file alone is not counted as success); restart Antigravity fully after installing, because the docs do not say whether it hot-reloads hooks.

Each provider is detected independently (**Not detected / Detected / Connected / Repair required / Permission required / Failed / Unsupported**). A tool that is not installed is skipped and never breaks the others. Existing hooks are preserved. Agents may need a restart to load new hooks. Codex asks you to trust a new hook (`/hooks` in the CLI, Settings > Hooks in Desktop).

## Install

Download `Kaiwen Notify_<version>_x64-setup.exe` from the [Releases](https://github.com/ImtheKaiwen/kaiwen-notify/releases) page and run it. Windows 10/11 x64. The installer is per-user (no administrator prompt) and installs the Microsoft WebView2 runtime if it is missing (internet needed for that step; Windows 11 already has it). Then open the app and press **Install integrations**.

## Everyday use

- **Change the sound:** Settings, *Notification sound*, **Change**, pick a `.wav` (PCM, under 10 MB). **Test notification** plays it; **Use default** restores the bundled chime. Your custom file stays on your computer.
- **Language / theme:** Settings. Both apply immediately.
- **Launch at startup:** Settings. The toggle shows the real Windows registration state. Uninstalling removes the entry.
- **Remove integrations:** press **Remove integrations** and confirm. Backups stay next to the edited files (`*.kaiwen-backup-*`). Do this before uninstalling the app, because removing the app from Windows Settings does not revert hook files.

## Updates

Official Tauri updater, signature verification always on.

- While the app is running (window open or tray only) it checks `https://github.com/ImtheKaiwen/kaiwen-notify/releases/latest/download/latest.json` about 10 seconds after start and then **every 10 minutes**, from a Rust thread that sleeps between checks (no WebView needed, no polling loop).
- Requests have a 20 s timeout; while you are offline the interval backs off (20 min, 40 min, up to 60 min) and errors are not repeated. Builds without an update source configured send no requests at all.
- A new version appears in the tray menu and in the **Updates** card with its release notes. It is **never downloaded or installed automatically**.
- **Download** shows real byte progress and verifies the signature; **Install and restart** is a separate, explicit step. The app then relaunches without replaying the splash.
- Draft releases and pre-releases are not served by GitHub's `releases/latest` URL.

Until a signed release is published (see [docs/RELEASING.md](docs/RELEASING.md)) the shipped installer reports "Update server not configured".

## How it works

Agent hook, then a tiny PowerShell bridge (`%LOCALAPPDATA%\KaiwenNotify\hooks\send-event.ps1`), then a one-way UDP datagram to `127.0.0.1:47451`, then the Rust listener plays the WAV through WinMM. Duplicate events from the same source within 650 ms are ignored. Antigravity events are skipped while `fullyIdle` is false or when the stop reason is an error or step-limit stop.

Diagnostics: create an empty file `%LOCALAPPDATA%\KaiwenNotify\diag.enabled` and each hook call appends metadata (no prompts or paths) to `diag.log` in the same folder. Delete the file to turn it off.

All paths come from the Windows environment (`USERPROFILE`, `LOCALAPPDATA`, Tauri's app directories); nothing is tied to a particular user name, drive or OneDrive.

## Performance

Short version (details, method and caveats in [docs/PERFORMANCE.md](docs/PERFORMANCE.md)):

| Mode | v0.5.0 release build (3 runs, one laptop) |
|---|---|
| Installer size | 3.3 MB |
| Tray only (window closed) | about 30 MB, 0 WebView2 processes, 0 CPU over 30 s |
| Started with Launch at startup | about 14 MB (single observation) |
| Window open | about 395 MB combined: app 30 MB plus 6 WebView2 processes (~365 MB) |
| First window (splash) after launch | about 0.5 to 0.7 s; the main window follows after the ~2.4 s splash |
| Idle CPU | 0 to 0.06 s over 30 s windows |

These are measurements from one laptop (Windows 11 build 26200, Core i5-12450H, 16 GB), taken with `Get-Process` on an installed release build, not guarantees. The window-open number includes Microsoft's WebView2 processes, which are shared-runtime processes that Windows reports as separate working sets.

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
python -m pytest tests -q   # i18n parity, splash/autostart/scheduler contracts, hook scripts in throw-away profiles
python scripts/secret-scan.py
```

Adding a language: create `src/locales/<code>.ts` exporting a full `Dictionary` (TypeScript and the parity test fail if a key is missing), register it in `src/i18n.ts`, add the tray strings in `src-tauri/src/i18n.rs`.

## Security and known limitations

See [docs/SECURITY.md](docs/SECURITY.md). In short:

- The UDP listener only binds `127.0.0.1` but has no sender authentication; a local process can trigger the sound. Nothing privileged is reachable through it.
- Hook commands run with `-ExecutionPolicy Bypass` for the app's own bundled script.
- Windows 10/11 x64 only. Notifications mean "agent turn ended", not "task succeeded".
- Antigravity real-task notification has not been confirmed end to end yet.
- Tested on one machine plus CI; other PCs are covered by isolated-profile tests only (see [docs/PERFORMANCE.md](docs/PERFORMANCE.md#compatibility-matrix)).

## License

Source code: [MIT](LICENSE). Provider logos and trademarks belong to their owners and are **not** MIT-licensed; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
