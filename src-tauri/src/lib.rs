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
mod i18n;
mod integrations;
mod updater;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Wry,
};
use tauri_plugin_autostart::ManagerExt as _;

const PORT: u16 = 47451;
const SKIP_SPLASH_MARKER: &str = "skip-splash";
const SPLASH_FAILSAFE: Duration = Duration::from_secs(6);
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
    /// Unix millis of the last update check attempt that reached the server (0 = never).
    last_update_check: u64,
    /// system | en | tr
    #[serde(default = "system_theme")]
    language: String,
    /// Per Antigravity runtime (`app`, `ide`, `cli`): proof the hook really ran.
    antigravity: HashMap<String, ProductSeen>,
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
            language: system_theme(),
            antigravity: HashMap::new(),
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
    system_language: &'static str,
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
    /// Antigravity only: which runtime called the hook (App/IDE/CLI share one hooks.json).
    #[serde(default)]
    product: Option<Product>,
    /// Antigravity `seen` events: why no sound was played.
    #[serde(default)]
    note: Option<Note>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Product {
    App,
    Ide,
    Cli,
}
impl Product {
    fn key(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Ide => "ide",
            Self::Cli => "cli",
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Note {
    NotIdle,
    StoppedEarly,
}
impl Note {
    fn key(self) -> &'static str {
        match self {
            Self::NotIdle => "not_idle",
            Self::StoppedEarly => "stopped_early",
        }
    }
}

/// Evidence that a real Antigravity runtime executed our hook (file presence alone proves nothing).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct ProductSeen {
    /// Unix millis of the last hook invocation (any outcome).
    last_seen: u64,
    /// Unix millis of the last invocation that produced a notification.
    last_completed: u64,
    /// Why the last invocation stayed silent, if it did.
    last_note: Option<String>,
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
    Seen,
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
    startup: Mutex<Startup>,
}

/// Coordinates the splash and main window so the main window is only shown once it has rendered.
#[derive(Default)]
struct Startup {
    splash_active: bool,
    main_ready: bool,
}

struct TrayHandles {
    status: MenuItem<Wry>,
    show: MenuItem<Wry>,
    test: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    tray: tauri::tray::TrayIcon<Wry>,
}
struct TrayState(Mutex<Option<TrayHandles>>);

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
    refresh_tray(&app);
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
    // After an update the installer relaunches the app: skip the splash once.
    let marker = app
        .state::<AppState>()
        .config_path
        .with_file_name(SKIP_SPLASH_MARKER);
    let _ = fs::write(&marker, b"1");
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = updater::install(&handle);
        if result.is_err() {
            let _ = fs::remove_file(&marker);
        }
        result
    })
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
        return Err("E_INVALID_THEME".into());
    }
    update_settings(&state, |s| s.theme = theme)
}

#[tauri::command]
fn set_auto_update(state: tauri::State<'_, AppState>, enabled: bool) -> Result<(), String> {
    update_settings(&state, |s| s.auto_update = enabled)
}

#[tauri::command]
fn set_language(app: tauri::AppHandle, language: String) -> Result<(), String> {
    if !matches!(language.as_str(), "system" | "en" | "tr") {
        return Err("E_INVALID_LANGUAGE".into());
    }
    update_settings(&app.state::<AppState>(), |s| s.language = language)?;
    refresh_tray(&app);
    Ok(())
}

#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    // Always read the real OS registration, never a cached flag.
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("E_AUTOSTART:{e}"))
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let launcher = app.autolaunch();
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| format!("E_AUTOSTART:{e}"))?;
    launcher
        .is_enabled()
        .map_err(|e| format!("E_AUTOSTART:{e}"))
}

#[tauri::command]
fn app_ready(app: tauri::AppHandle) {
    let state = app.state::<AppState>();
    let reveal = match state.startup.lock() {
        Ok(mut startup) => {
            startup.main_ready = true;
            !startup.splash_active
        }
        Err(_) => true,
    };
    if reveal {
        show_main(&app);
    }
}

#[tauri::command]
fn splash_finished(app: tauri::AppHandle) {
    finish_splash(&app, false);
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
        .map_err(|e| format!("E_SAVE_SETTINGS:{e}"))
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
        return Err("E_SOUND_MISSING".into());
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
        Err("E_SOUND_PLAY".into())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn play_wav(_path: &Path) -> Result<(), String> {
    Err("E_UNSUPPORTED_OS".into())
}

fn play_configured(state: &AppState) -> Result<(), String> {
    play_wav(&current_sound(state)?)
}

#[tauri::command]
fn get_status(state: tauri::State<'_, AppState>) -> Result<Status, String> {
    Ok(Status {
        settings: state.settings.lock().map_err(|e| e.to_string())?.clone(),
        system_language: i18n::system_language(),
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
            return Err("E_SOUND_INVALID".into());
        }
        let size = fs::metadata(p).map_err(|e| e.to_string())?.len();
        if size > 10 * 1024 * 1024 {
            return Err("E_SOUND_TOO_LARGE".into());
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
    // Bind synchronously so the UI never reads a "listener down" state during startup.
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
    thread::spawn(move || {
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
            if let (Source::Antigravity, Some(product)) = (&event.source, event.product) {
                record_antigravity(
                    &app,
                    product,
                    matches!(event.kind, Kind::Completed),
                    event.note,
                );
            }
            if matches!(event.kind, Kind::Seen) {
                continue; // diagnostics only, never a sound
            }
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

fn record_antigravity(
    app: &tauri::AppHandle,
    product: Product,
    completed: bool,
    note: Option<Note>,
) {
    let state = app.state::<AppState>();
    let now = now_millis();
    let _ = update_settings(&state, |s| {
        let entry = s.antigravity.entry(product.key().to_string()).or_default();
        entry.last_seen = now;
        if completed {
            entry.last_completed = now;
            entry.last_note = None;
        } else {
            entry.last_note = note.map(|n| n.key().to_string());
        }
    });
    let _ = app.emit("hook-seen", product.key());
}

fn start_update_scheduler(app: tauri::AppHandle) {
    let reader = app.clone();
    updater::spawn_scheduler(
        app,
        move || {
            reader
                .state::<AppState>()
                .settings
                .lock()
                .is_ok_and(|s| s.auto_update)
        },
        |app| {
            record_update_check(app);
            refresh_tray(app);
        },
    );
}

fn current_language(app: &tauri::AppHandle) -> &'static str {
    let preference = app
        .state::<AppState>()
        .settings
        .lock()
        .map(|s| s.language.clone())
        .unwrap_or_default();
    i18n::resolve(&preference)
}

/// Re-applies tray texts (language) and update availability. Safe to call from any thread.
fn refresh_tray(app: &tauri::AppHandle) {
    let lang = current_language(app);
    let text = i18n::tray_text(lang);
    let snapshot = updater::snapshot(app);
    let state = app.state::<TrayState>();
    let Ok(guard) = state.0.lock() else { return };
    let Some(handles) = guard.as_ref() else {
        return;
    };
    let _ = handles.show.set_text(text.open);
    let _ = handles.test.set_text(text.test);
    let _ = handles.quit.set_text(text.quit);
    match snapshot.latest {
        Some(latest) => {
            let _ = handles
                .status
                .set_text(format!("{}: v{}", text.update_available, latest.version));
            let _ = handles.status.set_enabled(true);
            let _ = handles.tray.set_tooltip(Some(format!(
                "Kaiwen Notify — {} v{}",
                text.update_available, latest.version
            )));
        }
        None => {
            let _ = handles.status.set_text(format!(
                "{} {}",
                text.version_prefix, snapshot.current_version
            ));
            let _ = handles.status.set_enabled(false);
            let _ = handles.tray.set_tooltip(Some("Kaiwen Notify"));
        }
    }
}

/// Builds the main window hidden; it is revealed by `app_ready` (or the failsafe) once rendered.
fn build_main(app: &tauri::AppHandle) {
    if let Ok(mut startup) = app.state::<AppState>().startup.lock() {
        startup.main_ready = false;
    }
    let Some(config) = app.config().app.windows.first() else {
        return;
    };
    match WebviewWindowBuilder::from_config(app, config).and_then(|b| b.visible(false).build()) {
        Ok(_) => {}
        Err(error) => {
            eprintln!("Cannot create window: {error}");
            return;
        }
    }
    // Failsafe: never leave the user without a window if the page fails to report ready.
    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(SPLASH_FAILSAFE);
        finish_splash(&handle, true);
        // Only reveal if still hidden (never un-minimize a window the user minimized).
        let h2 = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if let Some(w) = h2.get_webview_window("main") {
                if matches!(w.is_visible(), Ok(false)) {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        });
    });
}

fn show_main(app: &tauri::AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    });
}

/// Transparent, borderless, non-taskbar splash window; the page animates and then calls `splash_finished`.
fn open_splash(app: &tauri::AppHandle) -> bool {
    let built = WebviewWindowBuilder::new(app, "splash", WebviewUrl::App("splash.html".into()))
        .title("Kaiwen Notify")
        .inner_size(560.0, 220.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(false)
        .focused(false)
        .center()
        .build();
    match built {
        Ok(_) => true,
        Err(error) => {
            eprintln!("Cannot create splash: {error}");
            false
        }
    }
}

/// Closes the splash and reveals the main window when it is ready (or when forced by the failsafe).
fn finish_splash(app: &tauri::AppHandle, force: bool) {
    let reveal = match app.state::<AppState>().startup.lock() {
        Ok(mut startup) => {
            startup.splash_active = false;
            startup.main_ready || force
        }
        Err(_) => true,
    };
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(splash) = handle.get_webview_window("splash") {
            let _ = splash.destroy();
        }
    });
    if reveal {
        show_main(app);
    }
}

/// Used by the tray and by a second launch: no splash, just show (or recreate) the main window.
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
        build_main(&handle);
    });
}

fn fresh_marker(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(10 * 60))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            open_window(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--autostart"])
                .app_name("Kaiwen Notify")
                .build(),
        )
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
                startup: Mutex::new(Startup::default()),
            });
            app.manage(TrayState(Mutex::new(None)));

            start_udp_listener(app.handle().clone());
            let lang = i18n::resolve(
                &app.state::<AppState>()
                    .settings
                    .lock()
                    .map(|s| s.language.clone())
                    .unwrap_or_default(),
            );
            let text = i18n::tray_text(lang);
            let status = MenuItem::with_id(app, "update", "", false, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", text.open, true, None::<&str>)?;
            let test = MenuItem::with_id(app, "test", text.test, true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", text.quit, true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&status, &show, &test, &quit])?;
            let tray = TrayIconBuilder::new()
                .tooltip("Kaiwen Notify")
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/tray.png"
                ))?)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" | "update" => open_window(app),
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
            if let Ok(mut slot) = app.state::<TrayState>().0.lock() {
                *slot = Some(TrayHandles {
                    status,
                    show,
                    test,
                    quit,
                    tray,
                });
            }
            refresh_tray(app.handle());

            // Startup mode: --autostart means tray only (no window, no splash). After an update the
            // installer relaunches us once: show the window directly. Otherwise splash, then window.
            let autostart = std::env::args().any(|a| a == "--autostart");
            let marker = app
                .state::<AppState>()
                .config_path
                .with_file_name(SKIP_SPLASH_MARKER);
            let after_update = fresh_marker(&marker);
            let _ = fs::remove_file(&marker);
            if autostart {
                // tray only
            } else if after_update {
                build_main(app.handle());
            } else {
                if let Ok(mut startup) = app.state::<AppState>().startup.lock() {
                    startup.splash_active = true;
                }
                build_main(app.handle());
                if !open_splash(app.handle()) {
                    finish_splash(app.handle(), true);
                }
            }
            start_update_scheduler(app.handle().clone());
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
            set_auto_update,
            set_language,
            get_autostart,
            set_autostart,
            app_ready,
            splash_finished
        ])
        .build(tauri::generate_context!())
        .expect("Kaiwen Notify could not start")
        .run(|_app, event| {
            // Closing the window destroys the WebView (frees its RAM); only the tray keeps the app alive.
            // Explicit quits (tray menu, updater) pass an exit code and are allowed through.
            if let tauri::RunEvent::Exit = event {
                updater::request_stop(_app);
            }
            if let tauri::RunEvent::ExitRequested {
                api, code: None, ..
            } = event
            {
                api.prevent_exit();
            }
        });
}
