# Performance and compatibility report

These numbers describe one build on one computer. They are observations, not guarantees. Other machines, Windows
builds, WebView2 versions and drivers will differ.

## Method

| Item | Value |
|---|---|
| App version | 0.5.0 (release build, `npm run desktop:build`, NSIS installer installed per-user, not a debug or dev build) |
| Date | 12 Nov 2026 |
| OS | Windows 11 Home Single Language, build 26200 |
| Hardware | 12th Gen Intel Core i5-12450H, 15.7 GB RAM, NVIDIA RTX 4050 Laptop GPU, SSD |
| WebView2 runtime | 157.0.4341.46 |
| Tool | PowerShell: `Get-Process` (`WorkingSet64`, cumulative `CPU` seconds, thread count), `Get-CimInstance Win32_Process` to find WebView2 child processes, `Stopwatch` for start time |
| Repetitions | 3 full runs (start, wait 12 s, measure a 30 s window with the window open, close the window, wait 8 s, measure a 30 s tray-only window) |
| Debug port | **Not** attached for these runs (an earlier 0.4.0 note used a debug port; that is not comparable) |

"Working set" is memory resident in RAM for a process. WebView2 runs as several processes that share some memory
with each other and with other WebView2 apps, so summing their working sets overstates the unique memory they use.
It is still what Task Manager shows per process, so it is reported as is.

## Results (v0.5.0)

| Metric | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| First window visible after launch (the splash; the main window follows ~2.4 s later) | 576 ms | 656 ms | 511 ms |
| Window open: app process | 30.4 MB | 30.3 MB | 30.5 MB |
| Window open: WebView2 processes (count) | 6 | 6 | 6 |
| Window open: WebView2 processes (summed working set) | 367.8 MB | 364.0 MB | 365.6 MB |
| Window open: combined | 398.2 MB | 394.3 MB | 396.1 MB |
| Window open: CPU over 30 s | 0.016 s | 0.062 s | 0 s |
| Tray only (window closed): app process | 30.4 MB | 30.4 MB | 30.4 MB |
| Tray only: WebView2 processes | 0 | 0 | 0 |
| Tray only: CPU over 30 s | 0 s | 0 s | 0 s |
| Tray only: threads | 22 | 22 | 22 |

Other single observations:

- Started with `--autostart` (the Launch-at-startup path): about 14 MB working set, no window, no WebView2 process,
  UDP listener bound.
- Splash to main window (screen capture, one run): the splash animation took about 2.3 s once visible and the main
  window appeared right after it.
- Installer: 3.31 MB (`Kaiwen Notify_0.5.0_x64-setup.exe`).
- Background update check: one request every 10 minutes (observed at 02:48:19 and 02:58:19 against a local test
  server). Not measured separately for CPU or network volume; the request is a single small JSON GET with a 20 s
  timeout and the thread sleeps between checks.

The earlier v0.4.0 figures (about 156 ms to the window, about 30 MB tray only, about 391 MB window open, 3.24 MB
installer) were taken the same way on the same machine and are in the same range. The v0.5.0 first-window time is
higher because the splash and the hidden main window now start together and compete for WebView2 start-up.

## What this means

- With the window closed the app is a ~30 MB tray process that does nothing between hook events.
- With the window open, most memory is Microsoft's WebView2, not the app's own code.
- A tray-only start (startup launch) is cheaper still because the first window never existed.

## Compatibility matrix

"Verified" means I observed it. "Isolated-profile test" means automated tests with a throw-away profile on the
development machine or CI, not a different computer. Nothing here was run on a second physical PC or a VM.

| Scenario | Status |
|---|---|
| Windows 11 x64, this machine, real user profile | Verified (install, run, update, uninstall) |
| Windows 11 x64, clean user profile | Not run. Hook scripts: isolated-profile test only |
| Windows 10 x64 | Not run. Tauri 2 and WebView2 support it; unverified here |
| Non-administrator user | Verified indirectly: installer is `currentUser` mode and installed without elevation prompts here |
| No OneDrive | Not run. Code uses only `USERPROFILE`, `LOCALAPPDATA` and Tauri directories; a source scan test blocks hard-coded user paths |
| Turkish letters and spaces in the profile path | Isolated-profile test (install, uninstall and executing the stored hook commands) |
| Claude Code / Codex / Antigravity not installed | Isolated-profile test (provider reported "Not detected", skipped, others unaffected) |
| Only one provider installed | Isolated-profile test |
| WebView2 runtime missing | Not run. Installer is configured with `downloadBootstrapper` (needs internet at install time) |
| Update install | Verified: 0.5.0 to 0.5.1 with a test signing key against a local server, driven from the app UI |
| Uninstall | Verified: files removed; the Launch-at-startup registry entry is removed |
| Reboot with Launch at startup | Not run. Verified instead by launching with the exact registered command line (`--autostart`) |
| High DPI | Partly: this machine runs at 125% scaling and the splash and main window rendered correctly. Other scales not run |
| Offline update checks | Backoff and no-request-when-unconfigured are unit tested. A real offline period was not run |
| Antigravity real task notification | **Not verified** (CLI quota exhausted; IDE not driven) |
