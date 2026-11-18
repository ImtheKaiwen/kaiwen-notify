# Changelog

## 0.6.0

### Added
- **Per-provider sounds**: Claude Code, Codex and Antigravity each have their own notification settings: enable switch, volume, task sound and sub-agent sound, with preview, change and reset-to-default. Custom sounds can be WAV, MP3 or Ogg Vorbis (up to 10 MB / 30 s) and are copied into the app's private data folder, so moving or deleting the original never breaks anything. A bundled softer sub-agent chime is the default for sub-agent sounds.
- **Sub-agent notifications** (off by default, one global switch): Claude Code `SubagentStop` and Codex `SubagentStop` play the sub-agent sound. The hook is registered only while the switch is on (it costs a process per finished sub-agent), by Install/Repair or by the switch itself, and only our own handler is ever added or removed. Claude's internal helper agents (empty `agent_type`) and any `Stop` that carries an agent id are never announced. Antigravity documents no sub-agent finished event, so it only has the main sound and the UI says why.
- **Central event classification** (`events.rs`): provider-independent `main_complete` / `subagent_complete` / `unknown`; the UI and playback never read raw provider fields. Ambiguous events are `unknown` and silent.
- **Notification gate** (`gate.rs`): identity-based duplicate filtering, merging of simultaneous sub-agent completions, main-task priority and a 150 ms grace so a main completion replaces a just-queued sub-agent sound.
- Capability reporting per provider (`unsupported` / `hook_missing` / `ready`) and "verified" evidence once a real sub-agent event has arrived.
- `docs/NOTIFICATIONS.md`, new tests for sounds, classification, gate, hook install/uninstall and privacy.

### Changed
- The hook bridge now forwards a short whitelist of event metadata (hook name, session/turn/agent ids, agent type) instead of a bare "completed" ping. It still never sends prompts, replies or paths, and older app/bridge combinations keep working.
- Audio: decode on demand (Symphonia, MP3/Ogg/WAV only) and play through WinMM from memory with volume applied; no resident audio thread or cached samples.
- Existing users: the single shared sound becomes the task sound of every provider on first start; the old field stays in `settings.json` (downgrade safe).
- Antigravity hook command, Claude Code and Codex `Stop` commands, and Codex's `notify` setting are unchanged.

### Fixed
- Duplicate and overlapping notifications: the same event delivered twice is one sound, parallel sub-agents of one provider are merged into one sound, and a provider's main completion takes priority over its own sub-agent sounds. Priority, merging and long sounds are tracked per provider, so one provider never silences another.
- Turning sub-agent notifications on or off is all-or-nothing across providers: if one provider's config cannot be updated, the providers already changed are restored exactly (third-party hooks included); if the restore fails too, the error names the providers and the setting follows the hooks actually on disk.

### Verification status and limitations
- **Claude Code**: hooks, bridge, classification and playback are covered by automated tests and a real release-build end-to-end run with synthetic events. A real Claude Code sub-agent run through the installed hooks was **not verified**.
- **Codex**: `SubagentStop` is implemented from the Codex hooks documentation and tested synthetically. It was **not verified with a real Codex sub-agent run**; Codex asks you to trust the new hook first. Codex's `notify` setting is never touched.
- **Antigravity**: main-task sound only. It documents no sub-agent finished event, so the sub-agent row is disabled with an explanation. Desktop hook command unchanged from 0.5.1; IDE and CLI real tasks are **not verified**.
- **Performance** (single run, same laptop, vs 0.5.1): tray-only about +1.6 MB, window open about +1 MB, about 20 ms CPU per played sound, executable +1.5 MB, idle CPU still 0, no WebView2 in tray-only mode. Details in `docs/PERFORMANCE.md`.
- New dependency Symphonia (MPL-2.0, MP3/Ogg Vorbis/WAV decoding only) is listed in `THIRD_PARTY_NOTICES.md`.

## 0.5.1

### Fixed
- Fixed Antigravity Desktop hook execution on Windows. Antigravity passes the quotes around a `-File "path"` argument through to PowerShell, which then failed with "invalid characters in path" (confirmed in the Antigravity language server log). The hook is now a single `-EncodedCommand` that contains no quotes or spaces and resolves the bridge script from `%LOCALAPPDATA%` at run time, so it works for every Windows user name and profile path, including paths with spaces and non-ASCII characters.
- Improved detection and migration of previously installed notification hooks. The three earlier forms (v0.4 quoted backslashes, v0.5.0 quoted forward slashes, a hand-fixed unquoted command) are recognised: broken ones show **Repair required** and are upgraded in place by Install/Repair; a working unquoted command on a plain path is left alone; duplicates collapse to one handler; uninstall removes every known form and nothing else. Other hooks in the shared file are never touched and a backup is made before each edit.
- The hook bridge script is refreshed automatically after an app update, so bridge fixes reach existing installs without pressing Repair.
- Preserved Antigravity IDE, CLI, Claude Code and Codex compatibility (their commands are unchanged; Codex's `notify` setting is still never touched).

### Notes
- The opt-in diagnostic log (`diag.enabled`) now rolls over at 256 KB. It is still off by default.
- Verification status: the Antigravity **Desktop App** was verified on a real Windows task with the unquoted command (maintainer log: payload `product=app fullyIdle=True reason=NO_TOOL_CALL`). The final encoded command is covered by automated tests under several launchers and unusual profile paths; its real-Desktop confirmation is recorded in the release notes. **IDE and CLI real tasks are not verified.**

## 0.5.0

### Added
- **English / Turkish i18n**: dictionaries in `src/locales`, language follows the Windows display language, manual override in Settings, applied instantly and persisted. Backend errors are now stable codes translated in the UI; tray menu is localized too. Parity and placeholder tests.
- **Launch at startup** (official `tauri-plugin-autostart`, off by default, tray-only with no window or splash, state read from the OS, registry entry removed by the uninstaller).
- **Transparent animated splash** (Antigravity, Codex/OpenAI, Claude logos, then the app icon and wordmark; opacity/transform only; reduced-motion aware; hard 5 s cap). Skipped for startup launches, tray reopen, second launches and the restart after an update. The main window is created hidden and revealed once rendered.
- **Background update checks every 10 minutes** from a Rust thread (condvar sleep, 20 s timeout, exponential backoff while offline, no repeated error spam, no requests when the updater is unconfigured, one check at a time). New versions show in the tray menu/tooltip and the Updates card; nothing downloads or installs automatically.
- Provider state `repair_required`; Antigravity also detected through its per-user program folder.
- MIT `LICENSE`, `THIRD_PARTY_NOTICES.md`, `docs/PERFORMANCE.md`.
- Sandbox tests with Turkish-letter/space profile paths and single-provider machines.

### Antigravity
- The bridge now tells the Desktop App, IDE and CLI apart (they share `~/.gemini/config/hooks.json` but keep separate data folders) and also reports silent runs. The app shows per-runtime evidence in the Integrations card; a hook file alone is no longer presented as a working connection.
- Known gap: a real end-to-end notification from the Antigravity Desktop App is **not verified yet** (the CLI quota was exhausted during testing and the Desktop UI could not be driven). Antigravity probably has to be fully restarted after hooks change; the docs do not say whether it hot-reloads them.

### Changed
- Default notification sound replaced by an original synthesized chime (`scripts/make-default-sound.py`); the previous sound's origin was undocumented.
- The notification listener binds before any window is created, so the UI never shows a false "listener down".
- CI actions updated to current majors (Node 24 runtime).

## 0.4.0

### Antigravity (unreleased fixes in this version)
- Hook command now uses a forward-slash script path (robust to POSIX-style command splitting); older backslash commands are recognised, shown as needing repair, migrated by Install/Repair and removed by Uninstall.
- Bridge no longer requires the exact `model_stop` reason or a readable payload; it skips only `fullyIdle=false`, error/step-limit stops. It always answers `{"decision":"stop"}` so it never blocks the agent.
- Opt-in diagnostics (`diag.enabled`) log event metadata.
- Real-task verification is still pending: the Antigravity CLI hit its account quota during testing, and the IDE could not be driven here.

### UI
- Real provider logos (Claude, OpenAI for Codex, Antigravity) with fallback marks; KB/MB-aware download size.

### Added
- Auto-updater on the official Tauri 2 updater plugin: check, changelog, real byte-progress download, signature verification, explicit "Kur ve Yeniden Başlat". Disabled until a release build supplies pubkey + endpoint.
- Per-provider live progress for install/uninstall (`integration-progress` events), provider states, retry/repair, uninstall confirmation dialog, single-operation guard.
- Theme setting (system/light/dark), auto-update toggle.
- `release.yml` (draft GitHub Release with signed artifacts, blocked when signing secrets are missing) and `ci.yml` (typecheck, build, fmt, clippy, tests, Windows build).
- Sandbox regression tests for hook install/uninstall and Codex `notify` preservation.

### Changed
- UI redesigned: minimal monochrome, compact, light/dark.
- Hook scripts: per-provider `-Provider` switch, atomic writes (temp + replace), `USERPROFILE` instead of `$HOME`.
- Settings are saved atomically; new settings fields default safely for older files.
- Tauri capabilities reduced to event listen + dialog open.
- Installer is per-user (`currentUser`), Turkish + English.

### Fixed
- App icon had opaque near-white corners; icons regenerated with real alpha (PNG + multi-size ICO + separate tray icon).
- `npm run build` typecheck failed on `process` in `vite.config.ts`.
