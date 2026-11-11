# Changelog

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
