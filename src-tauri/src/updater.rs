//! Thin wrapper over the official Tauri updater plugin, plus the background check scheduler.
//! Signature verification happens inside `Update::download`; installation only happens
//! when the user explicitly calls `install_update`. Finding an update never downloads or installs it.

use serde::Serialize;
use std::{
    sync::{atomic::AtomicBool, Condvar, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Background checks run this often while the app is alive.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// First background check shortly after startup.
pub const INITIAL_DELAY: Duration = Duration::from_secs(10);
/// Longest wait between checks while the network keeps failing.
pub const MAX_BACKOFF: Duration = Duration::from_secs(60 * 60);
/// Network timeout for one check.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Default)]
pub struct UpdateState {
    pub pending: Mutex<Option<Update>>,
    pub bytes: Mutex<Option<Vec<u8>>>,
    pub busy: AtomicBool,
    pub last_error: Mutex<Option<String>>,
    /// Shutdown signal for the scheduler thread.
    pub stop: (Mutex<bool>, Condvar),
}

#[derive(Clone, Serialize)]
pub struct UpdateMeta {
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct UpdateSnapshot {
    pub configured: bool,
    pub current_version: String,
    pub latest: Option<UpdateMeta>,
    pub downloaded: bool,
    pub last_error: Option<String>,
    pub interval_minutes: u64,
}

#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// downloading | verifying | ready
    pub phase: &'static str,
}

/// Delay before the next background check: the normal interval, doubling per consecutive
/// failure up to `MAX_BACKOFF`, so being offline never causes request spam.
pub fn next_delay(consecutive_failures: u32) -> Duration {
    if consecutive_failures == 0 {
        return CHECK_INTERVAL;
    }
    let factor = 1u32
        .checked_shl(consecutive_failures.min(10))
        .unwrap_or(u32::MAX);
    CHECK_INTERVAL.saturating_mul(factor).min(MAX_BACKOFF)
}

/// A background tick performs a request only when the updater is configured and enabled.
pub fn should_check(configured: bool, auto_enabled: bool) -> bool {
    configured && auto_enabled
}

/// The updater is only registered when a release build supplied `plugins.updater` (pubkey + endpoints).
pub fn is_configured(app: &AppHandle) -> bool {
    app.config().plugins.0.contains_key("updater")
}

pub fn snapshot(app: &AppHandle) -> UpdateSnapshot {
    let state = app.state::<UpdateState>();
    let latest = state.pending.lock().ok().and_then(|p| {
        p.as_ref().map(|u| UpdateMeta {
            version: u.version.clone(),
            notes: u.body.clone(),
            date: u.date.map(|d| d.to_string()),
        })
    });
    UpdateSnapshot {
        configured: is_configured(app),
        current_version: app.package_info().version.to_string(),
        latest,
        downloaded: state.bytes.lock().is_ok_and(|b| b.is_some()),
        last_error: state.last_error.lock().ok().and_then(|e| e.clone()),
        interval_minutes: CHECK_INTERVAL.as_secs() / 60,
    }
}

fn friendly(error: &tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error as E;
    match error {
        E::Reqwest(_) | E::Network(_) => "E_UPDATE_NETWORK".into(),
        E::ReleaseNotFound => "E_UPDATE_NOT_FOUND".into(),
        E::SignatureUtf8(_) | E::Minisign(_) | E::Base64(_) => "E_UPDATE_SIGNATURE".into(),
        other => format!(
            "E_UPDATE_GENERIC:{}",
            other.to_string().chars().take(160).collect::<String>()
        ),
    }
}

fn set_error(app: &AppHandle, error: Option<String>) {
    let state = app.state::<UpdateState>();
    let mut slot = state.last_error.lock();
    if let Ok(slot) = slot.as_mut() {
        **slot = error;
    }
}

pub async fn check(app: &AppHandle) -> Result<UpdateSnapshot, String> {
    if !is_configured(app) {
        return Err("not_configured".into());
    }
    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| friendly(&e))?;
    let found = match updater.check().await {
        Ok(found) => found,
        Err(error) => {
            let message = friendly(&error);
            set_error(app, Some(message.clone()));
            return Err(message);
        }
    };
    set_error(app, None);
    let state = app.state::<UpdateState>();
    // A newer check result invalidates any previously downloaded package.
    let same = found.as_ref().map(|u| u.version.clone())
        == state
            .pending
            .lock()
            .ok()
            .and_then(|p| p.as_ref().map(|u| u.version.clone()));
    if !same {
        if let Ok(mut bytes) = state.bytes.lock() {
            *bytes = None;
        }
    }
    if let Ok(mut pending) = state.pending.lock() {
        *pending = found;
    }
    Ok(snapshot(app))
}

pub async fn download(app: &AppHandle) -> Result<UpdateSnapshot, String> {
    let update = app
        .state::<UpdateState>()
        .pending
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("E_NO_PENDING")?;
    let emit = |downloaded: u64, total: Option<u64>, phase: &'static str| {
        let _ = app.emit(
            "update-progress",
            DownloadProgress {
                downloaded,
                total,
                phase,
            },
        );
    };
    emit(0, None, "downloading");
    let mut downloaded: u64 = 0;
    let mut last = Instant::now() - Duration::from_secs(1);
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                // Real byte counts, throttled to ~10 events/s to keep the WebView idle.
                if last.elapsed() >= Duration::from_millis(100) {
                    last = Instant::now();
                    emit(downloaded, total, "downloading");
                }
            },
            || emit(0, None, "verifying"),
        )
        .await
        .map_err(|e| friendly(&e))?;
    emit(bytes.len() as u64, Some(bytes.len() as u64), "ready");
    if let Ok(mut slot) = app.state::<UpdateState>().bytes.lock() {
        *slot = Some(bytes);
    }
    Ok(snapshot(app))
}

/// Hands the verified package to the Windows installer. On Windows the updater exits the app
/// and the NSIS installer (passive mode) relaunches it after a successful update.
pub fn install(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<UpdateState>();
    let update = state
        .pending
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("E_NO_PENDING")?;
    let bytes = state
        .bytes
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("E_NOT_DOWNLOADED")?;
    update.install(bytes).map_err(|e| friendly(&e))
}

/// Waits up to `duration`; returns true when the app asked the scheduler to stop.
#[allow(clippy::let_and_return)] // the binding releases the lock guard before `state` drops
fn wait_or_stop(app: &AppHandle, duration: Duration) -> bool {
    let state = app.state::<UpdateState>();
    let (lock, cvar) = &state.stop;
    let Ok(guard) = lock.lock() else { return true };
    let outcome = cvar.wait_timeout_while(guard, duration, |stopped| !*stopped);
    let stopped = match outcome {
        Ok((stopped, _)) => *stopped,
        Err(_) => true,
    };
    stopped
}

pub fn request_stop(app: &AppHandle) {
    let state = app.state::<UpdateState>();
    if let Ok(mut stopped) = state.stop.0.lock() {
        *stopped = true;
    }
    state.stop.1.notify_all();
}

/// Starts the background scheduler: one thread that sleeps on a condvar (no polling, no WebView).
/// `on_result` runs after every state-changing attempt (used for tray updates).
pub fn spawn_scheduler(
    app: AppHandle,
    auto_enabled: impl Fn() -> bool + Send + 'static,
    on_result: impl Fn(&AppHandle) + Send + 'static,
) {
    if !is_configured(&app) {
        return; // nothing to check against: never touch the network
    }
    std::thread::spawn(move || {
        if wait_or_stop(&app, INITIAL_DELAY) {
            return;
        }
        let mut failures: u32 = 0;
        loop {
            if should_check(true, auto_enabled()) {
                let state = app.state::<UpdateState>();
                // A manual check or download in flight owns the slot; skip this tick.
                let guard = crate::integrations::BusyGuard::acquire(&state.busy);
                if guard.is_ok() {
                    let previous_error = state.last_error.lock().ok().and_then(|e| e.clone());
                    match tauri::async_runtime::block_on(check(&app)) {
                        Ok(snapshot) => {
                            failures = 0;
                            on_result(&app);
                            let _ = app.emit("update-state", snapshot);
                        }
                        Err(error) => {
                            failures = failures.saturating_add(1);
                            // Only tell the UI when the error actually changed (no spam while offline).
                            if previous_error.as_deref() != Some(error.as_str()) {
                                on_result(&app);
                                let _ = app.emit("update-state", snapshot(&app));
                            }
                        }
                    }
                }
            }
            if wait_or_stop(&app, next_delay(failures)) {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_ten_minutes() {
        assert_eq!(next_delay(0), Duration::from_secs(600));
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(next_delay(1), Duration::from_secs(1200));
        assert_eq!(next_delay(2), Duration::from_secs(2400));
        assert_eq!(next_delay(3), MAX_BACKOFF);
        assert_eq!(next_delay(50), MAX_BACKOFF);
    }

    #[test]
    fn no_request_unless_configured_and_enabled() {
        assert!(should_check(true, true));
        assert!(!should_check(false, true));
        assert!(!should_check(true, false));
        assert!(!should_check(false, false));
    }
}
