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
mod audio;
mod events;
mod gate;
mod i18n;
mod integrations;
mod sound_config;
mod updater;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Wry,
};
use tauri_plugin_autostart::ManagerExt as _;

use events::{Category, Classified, Provider, RawEvent, Runtime};
use sound_config::SoundKind;

const PORT: u16 = 47451;

/// Listener port. `KAIWEN_NOTIFY_PORT` (4-5 digits) exists only so tests and side-by-side builds can run next to
/// a live instance; the hook bridge honours the same variable.
fn listener_port() -> u16 {
    std::env::var("KAIWEN_NOTIFY_PORT")
        .ok()
        .filter(|v| (4..=5).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|p| *p >= 1024)
        .unwrap_or(PORT)
}
const SKIP_SPLASH_MARKER: &str = "skip-splash";
const SPLASH_FAILSAFE: Duration = Duration::from_secs(6);
const DEFAULT_MAIN_WAV: &[u8] = include_bytes!("../resources/default.wav");
const DEFAULT_SUBAGENT_WAV: &[u8] = include_bytes!("../resources/subagent.wav");

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
    /// Independent sound settings per provider (volume, on/off, main and sub-agent sound).
    providers: sound_config::ProviderSounds,
    /// Global switch for sub-agent notifications (off by default; it also installs/removes the SubagentStop hooks).
    subagent_notifications: bool,
    /// Schema marker for the one-time migration from the single shared sound.
    sounds_version: u32,
    /// Unix millis of the last real sub-agent event per provider: proof the SubagentStop hook ran.
    subagent_seen: HashMap<String, u64>,
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
            providers: sound_config::ProviderSounds::default(),
            subagent_notifications: false,
            sounds_version: 0,
            subagent_seen: HashMap::new(),
        }
    }
}

#[derive(Clone, Serialize)]
struct NotificationEvent {
    source: String,
    timestamp: u64,
    category: Category,
}

#[derive(Serialize)]
struct Status {
    settings: Settings,
    /// Custom sounds whose private copy has disappeared (`<provider>.<kind>`); the default plays instead.
    missing_sounds: Vec<String>,
    system_language: &'static str,
    received: u64,
    listener_ok: bool,
    last_event: Option<NotificationEvent>,
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

struct AppState {
    settings: Mutex<Settings>,
    received: AtomicU64,
    listener_ok: AtomicBool,
    last_event: Mutex<Option<NotificationEvent>>,
    config_path: PathBuf,
    /// Private copies of user-chosen sounds (see sound_config).
    sounds_dir: PathBuf,
    gate: Mutex<gate::Gate>,
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
        let subagent_on = state
            .settings
            .lock()
            .is_ok_and(|settings| settings.subagent_notifications);
        integrations::run_operation(&app, &scripts_dir, install, subagent_on)
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
    write_settings_file(&state.config_path, settings)
}

fn write_settings_file(path: &Path, settings: &Settings) -> Result<(), String> {
    let serialized = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    // Atomic: temp file + rename, so a crash never leaves half-written settings.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serialized)
        .and_then(|_| fs::rename(&tmp, path))
        .map_err(|e| format!("E_SAVE_SETTINGS:{e}"))
}

fn default_wav(kind: SoundKind) -> &'static [u8] {
    match kind {
        SoundKind::Main => DEFAULT_MAIN_WAV,
        SoundKind::Subagent => DEFAULT_SUBAGENT_WAV,
    }
}

/// A sound ready to play: decoded on demand, volume applied, nothing kept afterwards.
struct Prepared {
    wav: Vec<u8>,
    length: Duration,
    /// The chosen custom file could not be used, so the bundled sound stands in.
    fell_back: bool,
}

fn prepare(state: &AppState, provider: Provider, kind: SoundKind) -> Result<Prepared, String> {
    let (choice, volume) = {
        let settings = state.settings.lock().map_err(|e| e.to_string())?;
        let config = settings.providers.get(provider);
        (config.choice(kind).clone(), config.volume)
    };
    let source = match sound_config::resolve(&state.sounds_dir, &choice) {
        Some(path) => audio::Source::File(path),
        None => audio::Source::Default,
    };
    let (pcm, fell_back) = audio::load(&source, default_wav(kind));
    let pcm = pcm?;
    Ok(Prepared {
        wav: audio::to_wav(&pcm, audio::gain_from_volume(volume)),
        length: Duration::from_secs_f32(pcm.seconds()),
        fell_back,
    })
}

/// Playback happens on a short-lived thread that ends with the sound: no resident audio thread or buffer.
fn spawn_playback(wav: Vec<u8>) {
    thread::spawn(move || {
        let _ = audio::play_wav_blocking(&wav);
    });
}

/// Tray "Test sound": the bundled main chime at full volume (independent of any provider setting).
fn play_default_preview() {
    if let Ok(pcm) = audio::decode_bytes(DEFAULT_MAIN_WAV.to_vec(), "wav") {
        spawn_playback(audio::to_wav(&pcm, 1.0));
    }
}

fn parse_provider(value: &str) -> Result<Provider, String> {
    Provider::from_key(value).ok_or_else(|| "E_INVALID_PROVIDER".to_string())
}

fn parse_kind(value: &str) -> Result<SoundKind, String> {
    SoundKind::parse(value).ok_or_else(|| "E_INVALID_SOUND_KIND".to_string())
}

#[tauri::command]
fn set_provider_enabled(
    state: tauri::State<'_, AppState>,
    provider: String,
    enabled: bool,
) -> Result<(), String> {
    let provider = parse_provider(&provider)?;
    update_settings(&state, |s| s.providers.get_mut(provider).enabled = enabled)
}

#[tauri::command]
fn set_provider_volume(
    state: tauri::State<'_, AppState>,
    provider: String,
    volume: u8,
) -> Result<(), String> {
    let provider = parse_provider(&provider)?;
    update_settings(&state, |s| {
        s.providers.get_mut(provider).volume = volume.min(100)
    })
}

/// Validates the picked file, copies it into the app's data folder and assigns it. Runs off the UI thread
/// because validation really decodes the file.
#[tauri::command]
async fn choose_sound(
    app: tauri::AppHandle,
    provider: String,
    kind: String,
    path: String,
) -> Result<(), String> {
    let provider = parse_provider(&provider)?;
    let kind = parse_kind(&kind)?;
    let dir = app.state::<AppState>().sounds_dir.clone();
    let choice =
        tauri::async_runtime::spawn_blocking(move || sound_config::import(Path::new(&path), &dir))
            .await
            .map_err(|e| e.to_string())??;
    let state = app.state::<AppState>();
    update_settings(&state, |s| {
        *s.providers.get_mut(provider).choice_mut(kind) = choice
    })?;
    cleanup_sounds(&state);
    Ok(())
}

#[tauri::command]
fn reset_sound(
    state: tauri::State<'_, AppState>,
    provider: String,
    kind: String,
) -> Result<(), String> {
    let provider = parse_provider(&provider)?;
    let kind = parse_kind(&kind)?;
    update_settings(&state, |s| {
        *s.providers.get_mut(provider).choice_mut(kind) = sound_config::SoundChoice::default()
    })?;
    cleanup_sounds(&state);
    Ok(())
}

fn cleanup_sounds(state: &AppState) {
    if let Ok(settings) = state.settings.lock() {
        sound_config::cleanup(&state.sounds_dir, &settings.providers);
    }
}

/// Plays the sound exactly as a real notification would (that provider's file and volume), ignoring the
/// enable switches. Returns true when the chosen file was unusable and the bundled sound was played instead.
#[tauri::command]
fn preview_sound(
    state: tauri::State<'_, AppState>,
    provider: String,
    kind: String,
) -> Result<bool, String> {
    let prepared = prepare(&state, parse_provider(&provider)?, parse_kind(&kind)?)?;
    let fell_back = prepared.fell_back;
    spawn_playback(prepared.wav);
    Ok(fell_back)
}

/// Turns sub-agent notifications on or off. Enabling also registers the SubagentStop hook for the connected
/// providers that document one (Claude Code, Codex); disabling removes only our own handler again.
#[tauri::command]
async fn set_subagent_notifications(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<Vec<integrations::ProviderReport>, String> {
    let dir = app.state::<AppState>().setup_scripts_dir.clone();
    let previous = {
        let state = app.state::<AppState>();
        let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
        let previous = settings.subagent_notifications;
        settings.subagent_notifications = enabled;
        if let Err(error) = save_settings(&state, &settings) {
            settings.subagent_notifications = previous;
            return Err(error);
        }
        previous
    };
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let _guard = integrations::BusyGuard::acquire(&state.integration_busy)?;
        integrations::sync_subagent_hooks(&dir, enabled)?;
        integrations::detect_all()
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    if outcome.is_err() {
        let state = app.state::<AppState>();
        let _ = update_settings(&state, |s| s.subagent_notifications = previous);
    }
    outcome
}

#[tauri::command]
fn get_status(state: tauri::State<'_, AppState>) -> Result<Status, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let mut missing_sounds = Vec::new();
    for provider in Provider::ALL {
        let config = settings.providers.get(provider);
        for (kind, choice) in [("main", &config.main), ("subagent", &config.subagent)] {
            if sound_config::resolve(&state.sounds_dir, choice).is_some_and(|path| !path.is_file())
            {
                missing_sounds.push(format!("{}.{kind}", provider.key()));
            }
        }
    }
    Ok(Status {
        settings,
        missing_sounds,
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

fn load_settings(path: &Path) -> Settings {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
        .unwrap_or_else(Settings::initial)
}

fn start_udp_listener(app: tauri::AppHandle) {
    // Bind synchronously so the UI never reads a "listener down" state during startup.
    let socket = match UdpSocket::bind(("127.0.0.1", listener_port())) {
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
        let mut buffer = [0u8; 2048];
        loop {
            let size = match socket.recv_from(&mut buffer) {
                Ok((size, _)) => size,
                Err(_) => continue,
            };
            // Anything that is not a well-formed event from one of our providers is dropped silently.
            let Ok(raw) = serde_json::from_slice::<RawEvent>(&buffer[..size]) else {
                continue;
            };
            if let Some(event) = events::classify(&raw) {
                handle_event(&app, event);
            }
        }
    });
}

/// The single path from "event arrived" to "sound plays": diagnostics, settings checks, duplicate/priority
/// gate, then a short-lived playback thread. Nothing here polls or keeps audio in memory.
fn handle_event(app: &tauri::AppHandle, event: Classified) {
    let state = app.state::<AppState>();
    if let (Provider::Antigravity, Some(runtime)) = (event.provider, event.runtime) {
        if event.silent_note.is_some() || event.category == Category::MainComplete {
            record_antigravity(app, runtime, event.silent_note.is_none(), event.silent_note);
        }
    }
    if event.category == Category::SubagentComplete {
        // Proof that the provider's SubagentStop hook really runs, whether or not the sound is enabled.
        let now = now_millis();
        let _ = update_settings(&state, |s| {
            s.subagent_seen
                .insert(event.provider.key().to_string(), now);
        });
        let _ = app.emit("hook-seen", event.provider.key());
    }
    let kind = match event.category {
        Category::MainComplete => SoundKind::Main,
        Category::SubagentComplete => SoundKind::Subagent,
        Category::Unknown => return,
    };
    let allowed = state.settings.lock().is_ok_and(|s| {
        s.enabled
            && s.providers.get(event.provider).enabled
            && (kind == SoundKind::Main || s.subagent_notifications)
    });
    if !allowed {
        return;
    }
    let now = Instant::now();
    let verdict = state
        .gate
        .lock()
        .map(|mut gate| gate.decide(&event, now))
        .unwrap_or(gate::Verdict::Skip("poisoned"));
    if verdict != gate::Verdict::Play {
        return;
    }
    let Ok(prepared) = prepare(&state, event.provider, kind) else {
        return;
    };
    match kind {
        SoundKind::Main => {
            if let Ok(mut gate) = state.gate.lock() {
                gate.main_started(now, prepared.length);
            }
            spawn_playback(prepared.wav);
            announce(app, &event);
        }
        SoundKind::Subagent => {
            let handle = app.clone();
            // Wait a moment so a main completion arriving right after can take over without a blip.
            thread::spawn(move || {
                thread::sleep(gate::SUBAGENT_DELAY);
                let state = handle.state::<AppState>();
                let wanted = state.gate.lock().is_ok_and(|mut gate| {
                    let wanted = gate.subagent_still_wanted(now);
                    if wanted {
                        gate.subagent_started(Instant::now(), prepared.length);
                    }
                    wanted
                });
                if wanted {
                    spawn_playback(prepared.wav);
                    announce(&handle, &event);
                }
            });
        }
    }
}

fn announce(app: &tauri::AppHandle, event: &Classified) {
    let state = app.state::<AppState>();
    state.received.fetch_add(1, Ordering::Relaxed);
    let detail = NotificationEvent {
        source: event.provider.label().to_string(),
        timestamp: now_millis(),
        category: event.category,
    };
    if let Ok(mut previous) = state.last_event.lock() {
        *previous = Some(detail.clone());
    }
    let _ = app.emit("notify-event", detail);
}

fn record_antigravity(
    app: &tauri::AppHandle,
    product: Runtime,
    completed: bool,
    note: Option<events::Note>,
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
            let sounds_dir = config_dir.join("sounds");
            let config_path = config_dir.join("settings.json");
            let mut settings = load_settings(&config_path);
            // One-time move from the single shared sound to per-provider sounds (keeps custom sounds).
            if sound_config::migrate(
                &settings.sound_path.clone(),
                &mut settings.sounds_version,
                &mut settings.providers,
                &sounds_dir,
            ) {
                let _ = write_settings_file(&config_path, &settings);
            }
            app.manage(AppState {
                settings: Mutex::new(settings),
                received: AtomicU64::new(0),
                listener_ok: AtomicBool::new(false),
                last_event: Mutex::new(None),
                config_path,
                sounds_dir,
                gate: Mutex::new(gate::Gate::default()),
                setup_scripts_dir: config_dir.join("hook-setup"),
                integration_busy: AtomicBool::new(false),
                startup: Mutex::new(Startup::default()),
            });
            app.manage(TrayState(Mutex::new(None)));

            // Bridge fixes shipped in an update reach already installed hooks without pressing Repair.
            integrations::refresh_bridge_script();
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
                        play_default_preview();
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
            set_provider_enabled,
            set_provider_volume,
            choose_sound,
            reset_sound,
            preview_sound,
            set_subagent_notifications,
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
