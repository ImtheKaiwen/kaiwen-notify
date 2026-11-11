//! Hook integration detection, install and uninstall with per-provider progress events.
//!
//! The PowerShell scripts own the file edits (backup + atomic replace). This module only
//! decides *what* to run, reports real per-provider progress, and re-reads the config files
//! afterwards so the UI never shows a success state the files do not back up.
//! Codex's TOML config and its `notify` setting are never read or written here.

use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Emitter};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    NotInstalled,
    Available,
    Connected,
    PermissionRequired,
    Failed,
    Unsupported,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
    Antigravity,
}

pub const ALL: [Provider; 3] = [Provider::Claude, Provider::Codex, Provider::Antigravity];

impl Provider {
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
        }
    }
    /// Directory created by the tool itself; its absence means the tool was never run.
    fn tool_dir(self, home: &Path) -> PathBuf {
        home.join(match self {
            Self::Claude => ".claude",
            Self::Codex => ".codex",
            Self::Antigravity => ".gemini",
        })
    }
    fn config_file(self, home: &Path) -> PathBuf {
        match self {
            Self::Claude => home.join(".claude").join("settings.json"),
            Self::Codex => home.join(".codex").join("hooks.json"),
            Self::Antigravity => home.join(".gemini").join("config").join("hooks.json"),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct ProviderReport {
    pub id: &'static str,
    pub state: ProviderState,
    pub detail: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct Progress {
    pub operation: &'static str,
    pub provider: &'static str,
    /// waiting | working | done | failed | skipped
    pub phase: &'static str,
    pub state: ProviderState,
    pub message: String,
}

pub fn expected_hook_path() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA bulunamadı")?;
    Ok(PathBuf::from(local)
        .join("KaiwenNotify")
        .join("hooks")
        .join("send-event.ps1"))
}

pub fn home_dir() -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        std::env::var_os("USERPROFILE").ok_or("USERPROFILE bulunamadı")?,
    ))
}

/// Antigravity gets a forward-slash path (robust to POSIX-style command splitting); the older
/// backslash form written by 0.4.0 is still recognised so existing installs keep working.
pub fn antigravity_commands(script: &Path) -> [String; 2] {
    let fwd = script.display().to_string().replace('\\', "/");
    [
        format!("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{fwd}\" -Source antigravity"),
        hook_command(script, "antigravity"),
    ]
}

pub fn hook_command(script: &Path, source: &str) -> String {
    format!(
        "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\" -Source {source}",
        script.display()
    )
}

pub fn has_own_hook(root: &serde_json::Value, path: &[&str], command: &str) -> bool {
    let Some(groups) = path
        .iter()
        .try_fold(root, |current, part| current.get(*part))
        .and_then(|v| v.as_array())
    else {
        return false;
    };
    groups.iter().any(|group| {
        // Claude/Codex groups contain a hooks array; Antigravity Stop groups are direct handlers.
        match group.get("hooks").and_then(|v| v.as_array()) {
            Some(handlers) => handlers
                .iter()
                .any(|hook| hook.get("command").and_then(|v| v.as_str()) == Some(command)),
            None => group.get("command").and_then(|v| v.as_str()) == Some(command),
        }
    })
}

fn read_json_optional(path: &Path) -> serde_json::Value {
    fs::read(path)
        .ok()
        .and_then(|content| serde_json::from_slice(&content).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn is_connected(provider: Provider, home: &Path, script: &Path) -> bool {
    let json = read_json_optional(&provider.config_file(home));
    let command = hook_command(script, provider.id());
    match provider {
        Provider::Antigravity => antigravity_commands(script)
            .iter()
            .any(|c| has_own_hook(&json, &["kaiwen-notify", "Stop"], c)),
        _ => has_own_hook(&json, &["hooks", "Stop"], &command),
    }
}

/// True when the Antigravity hook exists only in the older backslash form (0.4.0), so Install/Repair
/// should run once to migrate it.
fn needs_migration(provider: Provider) -> bool {
    if provider != Provider::Antigravity {
        return false;
    }
    let (Ok(home), Ok(script)) = (home_dir(), expected_hook_path()) else {
        return false;
    };
    let json = read_json_optional(&provider.config_file(&home));
    let [fwd, legacy] = antigravity_commands(&script);
    has_own_hook(&json, &["kaiwen-notify", "Stop"], &legacy)
        && !has_own_hook(&json, &["kaiwen-notify", "Stop"], &fwd)
}

pub fn detect_one(provider: Provider) -> Result<ProviderReport, String> {
    let report = |state, detail: Option<&str>| ProviderReport {
        id: provider.id(),
        state,
        detail: detail.map(str::to_owned),
    };
    if !cfg!(windows) {
        return Ok(report(
            ProviderState::Unsupported,
            Some("Yalnızca Windows destekleniyor"),
        ));
    }
    let home = home_dir()?;
    let script = expected_hook_path()?;
    if needs_migration(provider) {
        return Ok(report(
            ProviderState::Available,
            Some("Hook komutu güncellenmeli (Onar)"),
        ));
    }
    if is_connected(provider, &home, &script) {
        return Ok(report(ProviderState::Connected, None));
    }
    if !provider.tool_dir(&home).is_dir() {
        return Ok(report(
            ProviderState::NotInstalled,
            Some("Araç bu bilgisayarda bulunamadı"),
        ));
    }
    let config = provider.config_file(&home);
    if fs::metadata(&config).is_ok_and(|m| m.permissions().readonly()) {
        return Ok(report(
            ProviderState::PermissionRequired,
            Some("Yapılandırma dosyası salt okunur"),
        ));
    }
    Ok(report(ProviderState::Available, None))
}

pub fn detect_all() -> Result<Vec<ProviderReport>, String> {
    ALL.iter().map(|p| detect_one(*p)).collect()
}

/// Runs the bundled PowerShell script for one provider. Returns a short error on failure.
#[cfg(windows)]
fn run_script(scripts_dir: &Path, install: bool, provider: Provider) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let path = scripts_dir.join(if install {
        "install-hooks.ps1"
    } else {
        "uninstall-hooks.ps1"
    });
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&path)
        .args(["-Provider", provider.id()])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("PowerShell çalıştırılamadı: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let details = String::from_utf8_lossy(&output.stderr);
    Err(details
        .lines()
        .next()
        .unwrap_or("bilinmeyen hata")
        .chars()
        .take(200)
        .collect())
}

#[cfg(not(windows))]
fn run_script(_: &Path, _: bool, _: Provider) -> Result<(), String> {
    Err("Hook kurulumu yalnızca Windows üzerinde destekleniyor".into())
}

#[cfg(windows)]
fn write_scripts(dir: &Path) -> Result<(), String> {
    const INSTALL: &str = include_str!("../../scripts/install-hooks.ps1");
    const UNINSTALL: &str = include_str!("../../scripts/uninstall-hooks.ps1");
    const SEND: &str = include_str!("../../scripts/send-event.ps1");
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (name, body) in [
        ("send-event.ps1", SEND),
        ("install-hooks.ps1", INSTALL),
        ("uninstall-hooks.ps1", UNINSTALL),
    ] {
        fs::write(dir.join(name), body).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn write_scripts(_: &Path) -> Result<(), String> {
    Err("Hook kurulumu yalnızca Windows üzerinde destekleniyor".into())
}

/// RAII guard so only one install/uninstall runs at a time.
pub struct BusyGuard<'a>(&'a AtomicBool);
impl<'a> BusyGuard<'a> {
    pub fn acquire(flag: &'a AtomicBool) -> Result<Self, String> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self(flag))
            .map_err(|_| "Başka bir entegrasyon işlemi sürüyor".to_string())
    }
}
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn emit(
    app: &AppHandle,
    operation: &'static str,
    provider: Provider,
    phase: &'static str,
    state: ProviderState,
    message: &str,
) {
    let _ = app.emit(
        "integration-progress",
        Progress {
            operation,
            provider: provider.id(),
            phase,
            state,
            message: message.to_owned(),
        },
    );
}

pub fn run_operation(
    app: &AppHandle,
    scripts_dir: &Path,
    install: bool,
) -> Result<Vec<ProviderReport>, String> {
    let operation = if install { "install" } else { "uninstall" };
    write_scripts(scripts_dir)?;
    let before = detect_all()?;
    for report in &before {
        let provider = ALL.iter().copied().find(|p| p.id() == report.id).unwrap();
        emit(app, operation, provider, "waiting", report.state, "Sırada");
    }
    let mut failed: Vec<(&'static str, String)> = Vec::new();
    for provider in ALL {
        let current = detect_one(provider)?;
        let actionable = if install {
            matches!(
                current.state,
                ProviderState::Available | ProviderState::PermissionRequired
            ) || needs_migration(provider)
        } else {
            current.state == ProviderState::Connected || needs_migration(provider)
        };
        if !actionable {
            let note = match current.state {
                ProviderState::Connected => "Zaten bağlı",
                ProviderState::NotInstalled => "Atlandı: araç bulunamadı",
                _ => "Atlandı",
            };
            emit(app, operation, provider, "skipped", current.state, note);
            continue;
        }
        emit(
            app,
            operation,
            provider,
            "working",
            current.state,
            if install {
                "Hook ekleniyor…"
            } else {
                "Hook kaldırılıyor…"
            },
        );
        let outcome = run_script(scripts_dir, install, provider);
        // Always verify against the files, never trust the script exit code alone.
        let after = detect_one(provider)?;
        let verified = if install {
            after.state == ProviderState::Connected
        } else {
            after.state != ProviderState::Connected
        };
        match (outcome, verified) {
            (Ok(()), true) => emit(
                app,
                operation,
                provider,
                "done",
                after.state,
                if install {
                    "Bağlandı"
                } else {
                    "Kaldırıldı"
                },
            ),
            (outcome, _) => {
                let message = outcome
                    .err()
                    .unwrap_or_else(|| "Değişiklik doğrulanamadı".into());
                emit(
                    app,
                    operation,
                    provider,
                    "failed",
                    ProviderState::Failed,
                    &message,
                );
                failed.push((provider.id(), message));
            }
        }
    }
    let mut finals = detect_all()?;
    // Failed providers stay "failed" in the result so partial success is never shown as full success.
    for report in &mut finals {
        if let Some((_, message)) = failed.iter().find(|(id, _)| *id == report.id) {
            report.state = ProviderState::Failed;
            report.detail = Some(message.clone());
        }
    }
    Ok(finals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_claude_style_hook() {
        let cmd = hook_command(Path::new("C:/x/send-event.ps1"), "claude");
        let root = json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":cmd}]}]}});
        assert!(has_own_hook(&root, &["hooks", "Stop"], &cmd));
        assert!(!has_own_hook(&root, &["hooks", "Stop"], "other"));
    }

    #[test]
    fn detects_antigravity_direct_handler() {
        let cmd = hook_command(Path::new("C:/x/send-event.ps1"), "antigravity");
        let root = json!({"kaiwen-notify":{"Stop":[{"type":"command","command":cmd}]}});
        assert!(has_own_hook(&root, &["kaiwen-notify", "Stop"], &cmd));
    }

    #[test]
    fn foreign_hooks_are_not_ours() {
        let root = json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo hi"}]}]}});
        assert!(!has_own_hook(
            &root,
            &["hooks", "Stop"],
            "powershell.exe something"
        ));
        assert!(!has_own_hook(
            &serde_json::Value::Null,
            &["hooks", "Stop"],
            "x"
        ));
    }

    #[test]
    fn busy_guard_is_exclusive_and_releases() {
        let flag = AtomicBool::new(false);
        {
            let _g = BusyGuard::acquire(&flag).unwrap();
            assert!(BusyGuard::acquire(&flag).is_err());
        }
        assert!(BusyGuard::acquire(&flag).is_ok());
    }
}
