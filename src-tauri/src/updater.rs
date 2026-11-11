//! Thin wrapper over the official Tauri updater plugin.
//! Signature verification happens inside `Update::download`; installation only happens
//! when the user explicitly calls `install_update`.

use serde::Serialize;
use std::{
    sync::{atomic::AtomicBool, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Automatic checks run at most this often.
pub const AUTO_CHECK_INTERVAL_MS: u64 = 12 * 60 * 60 * 1000;

#[derive(Default)]
pub struct UpdateState {
    pub pending: Mutex<Option<Update>>,
    pub bytes: Mutex<Option<Vec<u8>>>,
    pub busy: AtomicBool,
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
}

#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// downloading | verifying | ready
    pub phase: &'static str,
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
    }
}

fn friendly(error: &tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error as E;
    match error {
        E::Reqwest(_) | E::Network(_) => {
            "Güncelleme sunucusuna ulaşılamadı. İnternet bağlantını kontrol et.".into()
        }
        E::ReleaseNotFound => "Güncelleme sunucusunda yayın bulunamadı.".into(),
        E::SignatureUtf8(_) | E::Minisign(_) | E::Base64(_) => {
            "Güncelleme imzası doğrulanamadı; paket kurulmadı.".into()
        }
        other => format!(
            "Güncelleme hatası: {}",
            other.to_string().chars().take(160).collect::<String>()
        ),
    }
}

pub async fn check(app: &AppHandle) -> Result<UpdateSnapshot, String> {
    if !is_configured(app) {
        return Err("not_configured".into());
    }
    let updater = app.updater().map_err(|e| friendly(&e))?;
    let found = updater.check().await.map_err(|e| friendly(&e))?;
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
        .ok_or("İndirilecek güncelleme yok. Önce güncellemeleri kontrol et.")?;
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
        .ok_or("Kurulacak güncelleme yok")?;
    let bytes = state
        .bytes
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("Güncelleme henüz indirilmedi")?;
    update.install(bytes).map_err(|e| friendly(&e))
}
