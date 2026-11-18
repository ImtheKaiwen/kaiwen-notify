# Performance and compatibility report

These numbers describe one build on one computer. They are observations, not guarantees. Other machines, Windows
builds, WebView2 versions and drivers will differ.

## Per-provider sounds and sub-agent events: old vs new (v0.6.0)

Single run on the machine below, 18 Nov 2026, Windows 11 build 26200, WebView2 157, release builds, measured with
`Get-Process` (working set, cumulative CPU time) and `Get-CimInstance` for WebView2 child processes. Script:
`perf2.ps1` flow (tray-only 30 s, window open 30 s, close window, burst of notifications, tray-only again).

| State | OLD v0.5.1 (installed, attached to a long-running instance) | NEW (this branch, release build, `--autostart` start) |
|---|---|---|
| Tray only, idle 30 s: app process | 25.7 MB, CPU 0.016 s | 14.3 MB (fresh start), CPU 0 s |
| Tray only after opening and closing the window | 25.8 MB, CPU 0 s | 27.4 MB, CPU 0 s |
| WebView2 processes in tray-only mode | 0 | 0 |
| Window open, idle 30 s: app + 6 WebView2 processes | 26.0 + 344.5 = 370.5 MB, CPU 0 s | 27.1 + 344.4 = 371.5 MB, CPU 0 s |
| While notifications play (peak app working set) | 25.8 MB; CPU 0 s over 4 s (3 sounds) | 30.0 MB; CPU 0.125 s over 8 s (6 sounds, about 20 ms CPU each) |
| Tray only, 5 s after the burst | 25.8 MB | 29.8 MB |
| Executable size | 13.97 MB | 15.46 MB (+1.5 MB: Symphonia MP3/Ogg/WAV decoders) |

Reading it honestly:

- The OLD numbers come from the instance that was already running for a while, the NEW "fresh start" number from a cold
  `--autostart` launch, so those two cells are not comparable (a cold tray-only start was about 14 MB in v0.5.0 too).
  The like-for-like cells are the ones after a window open/close cycle: **about +1.6 MB** tray-only (27.4 vs 25.8 MB)
  and **about +1 MB** with the window open. The 5-second-after-burst value (29.8 MB) shows allocator retention of
  roughly 2 to 3 MB after playing; it did not keep growing across the 6 sounds.
- During a notification the NEW build decodes the sound, applies the volume and plays it from memory on a short-lived
  thread, so it spends about 20 ms of CPU per sound and holds the decoded samples only while it plays (peak +2.6 MB
  over idle for a 0.7 s mono 24 kHz chime; a long custom file would use more, bounded by the 30 s / 10 MB limits). The
  old build handed a file path to WinMM and measured no CPU at all.
- Idle CPU stays at 0 in every idle window; no timers or polling were added (the only new sleep is the 150 ms
  sub-agent grace delay, per sub-agent event).
- No WebView2 process exists in tray-only mode, before or after this change.
- Not measured: a long soak test (hours) for leaks. The gate's memory is bounded by pruning (unit test), and the app
  did not grow across the burst above, but that is not a soak test.

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
