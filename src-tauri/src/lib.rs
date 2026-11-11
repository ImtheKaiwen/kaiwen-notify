use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    net::UdpSocket,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
mod integrations;
mod updater;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};

const PORT: u16 = 47451;
const DEFAULT_WAV: &[u8] = include_bytes!("../resources/default.wav");

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    #[serde(default = "yes")]
    enabled: bool,
    sound_path: String,
    /// system | light | dark
    #[serde(default = "system_theme")]
    theme: String,
    #[serde(default = "yes")]
    auto_update: bool,
    /// Unix millis of the last successful update check (0 = never).
    last_update_check: u64,
}
fn yes() -> bool {
    true
}
fn system_theme() -> String {
    "system".into()
}

impl Settings {
    fn initial() -> Self {
        Self {
            enabled: true,
            sound_path: String::new(),
            theme: system_theme(),
            auto_update: true,
            last_update_check: 0,
        }
    }
}

#[derive(Clone, Serialize)]
struct NotificationEvent {
    source: String,
    timestamp: u64,
}

#[derive(Serialize)]
struct Status {
    settings: Settings,
    received: u64,
    listener_ok: bool,
    last_event: Option<NotificationEvent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct HookEvent {
    source: Source,
    kind: Kind,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Source {
    Claude,
    Codex,
    Antigravity,
}
impl Source {
    fn label(&self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Antigravity => "Antigravity",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Completed,
}

struct AppState {
    settings: Mutex<Settings>,
    received: AtomicU64,
    listener_ok: AtomicBool,
    last_event: Mutex<Option<NotificationEvent>>,
    config_path: PathBuf,
    default_sound: PathBuf,
    setup_scripts_dir: PathBuf,
    integration_busy: AtomicBool,
}

#[tauri::command]
fn get_integrations() -> Result<Vec<integrations::ProviderReport>, String> {
    integrations::detect_all()
}

#[tauri::command]
async fn install_integrations(
    app: tauri::AppHandle,
) -> Result<Vec<integrations::ProviderReport>, String> {
    run_integration_op(app, true).await
}

#[tauri::command]
async fn uninstall_integrations(
    app: tauri::AppHandle,
) -> Result<Vec<integrations::ProviderReport>, String> {
    run_integration_op(app, false).await
}

async fn run_integration_op(
    app: tauri::AppHandle,
    install: bool,
) -> Result<Vec<integrations::ProviderReport>, String> {
    let scripts_dir = app.state::<AppState>().setup_scripts_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _guard = integrations::BusyGuard::acquire(&state.integration_busy)?;
        integrations::run_operation(&app, &scripts_dir, install)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_update_state(app: tauri::AppHandle) -> updater::UpdateSnapshot {
    updater::snapshot(&app)
}

#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<updater::UpdateSnapshot, String> {
    let state = app.state::<updater::UpdateState>();
    let _guard = integrations::BusyGuard::acquire(&state.busy)?;
    let result = updater::check(&app).await;
    if result.is_ok() {
        record_update_check(&app);
    }
    result
}

#[tauri::command]
async fn download_update(app: tauri::AppHandle) -> Result<updater::UpdateSnapshot, String> {
    let state = app.state::<updater::UpdateState>();
    let _guard = integrations::BusyGuard::acquire(&state.busy)?;
    updater::download(&app).await
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<updater::UpdateState>();
    let _guard = integrations::BusyGuard::acquire(&state.busy)?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || updater::install(&handle))
        .await
        .map_err(|e| e.to_string())?
}

fn record_update_check(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let _ = update_settings(&state, |s| s.last_update_check = now_millis());
}

#[tauri::command]
fn set_theme(state: tauri::State<'_, AppState>, theme: String) -> Result<(), String> {
    if !matches!(theme.as_str(), "system" | "light" | "dark") {
        return Err("Geçersiz tema".into());
    }
    update_settings(&state, |s| s.theme = theme)
}

#[tauri::command]
fn set_auto_update(state: tauri::State<'_, AppState>, enabled: bool) -> Result<(), String> {
    update_settings(&state, |s| s.auto_update = enabled)
}

fn update_settings(state: &AppState, change: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
    let previous = settings.clone();
    change(&mut settings);
    if let Err(error) = save_settings(state, &settings) {
        *settings = previous;
        return Err(error);
    }
    Ok(())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn save_settings(state: &AppState, settings: &Settings) -> Result<(), String> {
    let serialized = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    // Atomic: temp file + rename, so a crash never leaves half-written settings.
    let tmp = state.config_path.with_extension("json.tmp");
    fs::write(&tmp, serialized)
        .and_then(|_| fs::rename(&tmp, &state.config_path))
        .map_err(|e| format!("Ayarlar kaydedilemedi: {e}"))
}

fn current_sound(state: &AppState) -> Result<PathBuf, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?;
    Ok(if settings.sound_path.is_empty() {
        state.default_sound.clone()
    } else {
        PathBuf::from(&settings.sound_path)
    })
}

#[cfg(windows)]
fn play_wav(path: &Path) -> Result<(), String> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u16, hmod: *mut c_void, flags: u32) -> i32;
    }
    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_FILENAME: u32 = 0x0002_0000;
    if !path.is_file() {
        return Err("Ses dosyası bulunamadı".into());
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // WinMM plays WAV asynchronously; no audio thread, media decoder, or child process.
    let result = unsafe {
        PlaySoundW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            SND_ASYNC | SND_NODEFAULT | SND_FILENAME,
        )
    };
    if result == 0 {
        Err("Windows WAV dosyasını oynatamadı".into())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn play_wav(_path: &Path) -> Result<(), String> {
    Err("Bu MVP, Windows WAV çıkışı için geliştirildi".into())
}

fn play_configured(state: &AppState) -> Result<(), String> {
    play_wav(&current_sound(state)?)
}

#[tauri::command]
fn get_status(state: tauri::State<'_, AppState>) -> Result<Status, String> {
    Ok(Status {
        settings: state.settings.lock().map_err(|e| e.to_string())?.clone(),
        received: state.received.load(Ordering::Relaxed),
        listener_ok: state.listener_ok.load(Ordering::Relaxed),
        last_event: state.last_event.lock().map_err(|e| e.to_string())?.clone(),
    })
}

#[tauri::command]
fn set_enabled(state: tauri::State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
    let previous = settings.clone();
    settings.enabled = enabled;
    if let Err(error) = save_settings(&state, &settings) {
        *settings = previous;
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
fn set_sound(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    if !path.is_empty() {
        let p = Path::new(&path);
        if !p.is_file()
            || !p
                .extension()
                .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("wav"))
        {
            return Err("Geçerli bir .wav dosyası seçmelisin".into());
        }
        let size = fs::metadata(p).map_err(|e| e.to_string())?.len();
        if size > 10 * 1024 * 1024 {
            return Err("Ses dosyası 10 MB'den küçük olmalı".into());
        }
    }
    let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
    let previous = settings.clone();
    settings.sound_path = path;
    if let Err(error) = save_settings(&state, &settings) {
        *settings = previous;
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
fn test_sound(state: tauri::State<'_, AppState>) -> Result<(), String> {
    play_configured(&state)
}

fn load_settings(path: &Path) -> Settings {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
        .unwrap_or_else(Settings::initial)
}

fn start_udp_listener(app: tauri::AppHandle) {
    thread::spawn(move || {
        let socket = match UdpSocket::bind(("127.0.0.1", PORT)) {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!("Cannot bind notification listener: {error}");
                return;
            }
        };
        app.state::<AppState>()
            .listener_ok
            .store(true, Ordering::Release);
        let mut recent: HashMap<&'static str, Instant> = HashMap::new();
        let mut buffer = [0u8; 1024];
        loop {
            let size = match socket.recv_from(&mut buffer) {
                Ok((size, _)) => size,
                Err(_) => continue,
            };
            let Ok(event) = serde_json::from_slice::<HookEvent>(&buffer[..size]) else {
                continue;
            };
            let source = event.source.label();
            let now = Instant::now();
            if recent
                .get(source)
                .is_some_and(|t| now.duration_since(*t) < Duration::from_millis(650))
            {
                continue;
            }
            recent.insert(source, now);
            let state = app.state::<AppState>();
            if !state.settings.lock().is_ok_and(|settings| settings.enabled) {
                continue;
            }
            let _ = play_configured(&state);
            state.received.fetch_add(1, Ordering::Relaxed);
            let detail = NotificationEvent {
                source: source.to_string(),
                timestamp: now_millis(),
            };
            if let Ok(mut previous) = state.last_event.lock() {
                *previous = Some(detail.clone());
            }
            let _ = app.emit("notify-event", detail);
        }
    });
}

/// One delayed, throttled check after startup on a short-lived thread; no timers or polling afterwards.
fn schedule_auto_check(app: tauri::AppHandle) {
    if !updater::is_configured(&app) {
        return;
    }
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(10));
        let due = {
            let state = app.state::<AppState>();
            let settings = state.settings.lock().ok();
            settings.is_some_and(|s| {
                s.auto_update
                    && now_millis().saturating_sub(s.last_update_check)
                        >= updater::AUTO_CHECK_INTERVAL_MS
            })
        };
        if !due {
            return;
        }
        let update_state = app.state::<updater::UpdateState>();
        let Ok(_guard) = integrations::BusyGuard::acquire(&update_state.busy) else {
            return;
        };
        if let Ok(snapshot) = tauri::async_runtime::block_on(updater::check(&app)) {
            record_update_check(&app);
            let _ = app.emit("update-state", snapshot);
        }
    });
}

fn open_window(app: &tauri::AppHandle) {
    // Window creation must happen on the main thread (this is also called from the single-instance thread).
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window("main") {
            if window.show().is_ok() {
                let _ = window.unminimize();
                let _ = window.set_focus();
                return;
            }
        }
        // The WebView is destroyed while the window is closed (tray-only), so recreate it on demand.
        if let Some(config) = handle.config().app.windows.first() {
            match tauri::WebviewWindowBuilder::from_config(&handle, config).and_then(|b| b.build())
            {
                Ok(_) => {}
                Err(error) => eprintln!("Cannot reopen window: {error}"),
            }
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            open_window(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(updater::UpdateState::default());
            // Register the updater only when a release build supplied its pubkey + endpoints.
            if updater::is_configured(app.handle()) {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
            }
            let config_dir = app.path().app_config_dir()?;
            fs::create_dir_all(&config_dir)?;
            let default_sound = config_dir.join("default.wav");
            if !default_sound.exists() {
                fs::write(&default_sound, DEFAULT_WAV)?;
            }
            let config_path = config_dir.join("settings.json");
            let settings = load_settings(&config_path);
            app.manage(AppState {
                settings: Mutex::new(settings),
                received: AtomicU64::new(0),
                listener_ok: AtomicBool::new(false),
                last_event: Mutex::new(None),
                config_path,
                default_sound,
                setup_scripts_dir: config_dir.join("hook-setup"),
                integration_busy: AtomicBool::new(false),
            });

            let show = MenuItem::with_id(app, "show", "Ayarları aç", true, None::<&str>)?;
            let test = MenuItem::with_id(app, "test", "Sesi test et", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Tamamen çık", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &test, &quit])?;
            let tray = TrayIconBuilder::new()
                .tooltip("Kaiwen Notify")
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/tray.png"
                ))?)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => open_window(app),
                    "test" => {
                        let _ = play_configured(&app.state::<AppState>());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::DoubleClick { .. } = event {
                        open_window(tray.app_handle());
                    }
                })
                .build(app)?;
            // The app owns the icon for its entire lifetime via Tauri's tray manager.
            let _ = tray;
            start_udp_listener(app.handle().clone());
            schedule_auto_check(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            set_enabled,
            set_sound,
            test_sound,
            get_integrations,
            install_integrations,
            uninstall_integrations,
            get_update_state,
            check_update,
            download_update,
            install_update,
            set_theme,
            set_auto_update
        ])
        .build(tauri::generate_context!())
        .expect("Kaiwen Notify could not start")
        .run(|_app, event| {
            // Closing the window destroys the WebView (frees its RAM); only the tray keeps the app alive.
            // Explicit quits (tray menu, updater) pass an exit code and are allowed through.
            if let tauri::RunEvent::ExitRequested {
                api, code: None, ..
            } = event
            {
                api.prevent_exit();
            }
        });
}
