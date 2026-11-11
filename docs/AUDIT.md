# Repository audit (start of 0.4.0 milestone)

| Area | Finding | Action |
|---|---|---|
| Stack | Tauri 2, Rust 2021, Vite 7, TypeScript 5, vanilla UI; deps: tauri, dialog, single-instance, serde | Kept; added `tauri-plugin-updater` only |
| Hook install | PowerShell scripts embedded in the EXE; all-or-nothing, no progress; non-atomic writes | Per-provider runs, progress events, atomic writes, post-verification |
| Uninstall | Worked but no confirmation, no progress | Confirmation dialog, progress, partial failure surfaced |
| Codex | Uses `~/.codex/hooks.json`; Codex's TOML config never touched by the app | Kept; sandbox regression tests added |
| Tray | Icon was the opaque-corner window icon | Dedicated tray icon with light outline |
| Idle cost | One blocking UDP thread on 127.0.0.1:47451, no timers | Kept; auto-update check is a single delayed short-lived thread |
| Capabilities | `core:default` + `dialog:default` (broad) | Reduced to listen/unlisten/dialog open |
| Build | `npm run build` failed typecheck (`process`) | Fixed |
| CI | Single workflow, no lint/tests | New CI + draft release workflow |
| Updater | None; no git remote, so no endpoint exists | Endpoint derived in CI from `github.repository`; nothing invented |
| Risk | UDP port has no sender auth (side effect: a sound) | Documented, unchanged |
| Risk | Hook scripts run with `-ExecutionPolicy Bypass` for the user's own bundled script | Documented, unchanged |
