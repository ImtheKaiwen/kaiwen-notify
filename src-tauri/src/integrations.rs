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
    RepairRequired,
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
    /// True when the tool looks installed: its own config directory exists, or (Antigravity, whose
    /// config dir only appears after the first run) a per-user program folder exists.
    fn is_present(self, home: &Path) -> bool {
        if self.tool_dir(home).is_dir() {
            return true;
        }
        if self == Self::Antigravity {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                let programs = PathBuf::from(local).join("Programs");
                return ["Antigravity", "Antigravity IDE"]
                    .iter()
                    .any(|name| programs.join(name).is_dir());
            }
        }
        false
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
    let local = std::env::var_os("LOCALAPPDATA").ok_or("E_NO_LOCALAPPDATA")?;
    Ok(PathBuf::from(local)
        .join("KaiwenNotify")
        .join("hooks")
        .join("send-event.ps1"))
}

pub fn home_dir() -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        std::env::var_os("USERPROFILE").ok_or("E_NO_USERPROFILE")?,
    ))
}

/// The one Antigravity hook command (keep identical to `$AntigravityScriptText` in the PowerShell scripts).
///
/// Antigravity forwards the quotes of a quoted `-File "path"` argument to PowerShell, which then fails with
/// "invalid characters in path". An `-EncodedCommand` contains no spaces or quotes, and the script path is
/// resolved at run time from `%LOCALAPPDATA%`, so this exact string works for every Windows user and path.
const ANTIGRAVITY_SCRIPT_TEXT: &str = "$ProgressPreference = 'SilentlyContinue'; $d = $env:LOCALAPPDATA; if (-not $d) { $d = [Environment]::GetFolderPath('LocalApplicationData') }; & (Join-Path $d 'KaiwenNotify\\hooks\\send-event.ps1') -Source antigravity";
const POWERSHELL_PREFIX: &str =
    "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass ";

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn antigravity_command() -> String {
    let utf16le: Vec<u8> = ANTIGRAVITY_SCRIPT_TEXT
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    format!(
        "{POWERSHELL_PREFIX}-OutputFormat Text -EncodedCommand {}",
        base64(&utf16le)
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HandlerKind {
    /// The current contract.
    Current,
    /// Older unquoted `-File` form; it works because the path has no spaces or non-ASCII characters.
    Compatible,
    /// Older quoted forms that Antigravity breaks.
    Legacy,
}

/// Classifies one Antigravity handler command; `None` means it is not Kaiwen Notify's.
pub fn classify_antigravity(command: &str) -> Option<HandlerKind> {
    if command == antigravity_command() {
        return Some(HandlerKind::Current);
    }
    let rest = command.strip_prefix(POWERSHELL_PREFIX)?;
    let middle = rest
        .strip_prefix("-File ")?
        .strip_suffix(" -Source antigravity")?;
    let (path, quoted) = match middle.strip_prefix('"').and_then(|m| m.strip_suffix('"')) {
        Some(inner) => (inner, true),
        None => (middle, false),
    };
    if path.contains('"') {
        return None;
    }
    let normalized = path.replace('\\', "/");
    if !normalized.ends_with("KaiwenNotify/hooks/send-event.ps1") {
        return None;
    }
    let plain = !quoted && path.bytes().all(|b| (0x21..=0x7E).contains(&b));
    Some(if plain {
        HandlerKind::Compatible
    } else {
        HandlerKind::Legacy
    })
}

/// Kinds of our handlers found in `kaiwen-notify.Stop` of the shared Antigravity hooks file.
pub fn antigravity_kinds(root: &serde_json::Value) -> Vec<HandlerKind> {
    root.get("kaiwen-notify")
        .and_then(|g| g.get("Stop"))
        .and_then(|v| v.as_array())
        .map(|handlers| {
            handlers
                .iter()
                .filter(|h| h.get("type").and_then(|t| t.as_str()) == Some("command"))
                .filter_map(|h| h.get("command").and_then(|c| c.as_str()))
                .filter_map(classify_antigravity)
                .collect()
        })
        .unwrap_or_default()
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
    match provider {
        Provider::Antigravity => antigravity_kinds(&json)
            .iter()
            .any(|k| matches!(k, HandlerKind::Current | HandlerKind::Compatible)),
        _ => has_own_hook(
            &json,
            &["hooks", "Stop"],
            &hook_command(script, provider.id()),
        ),
    }
}

/// True when Antigravity only has broken legacy handlers of ours, so Install/Repair should upgrade them.
fn needs_migration(provider: Provider) -> bool {
    if provider != Provider::Antigravity {
        return false;
    }
    let Ok(home) = home_dir() else { return false };
    let kinds = antigravity_kinds(&read_json_optional(&provider.config_file(&home)));
    !kinds.is_empty()
        && !kinds
            .iter()
            .any(|k| matches!(k, HandlerKind::Current | HandlerKind::Compatible))
}

pub fn detect_one(provider: Provider) -> Result<ProviderReport, String> {
    let report = |state, detail: Option<&str>| ProviderReport {
        id: provider.id(),
        state,
        detail: detail.map(str::to_owned),
    };
    if !cfg!(windows) {
        return Ok(report(ProviderState::Unsupported, Some("unsupported_os")));
    }
    let home = home_dir()?;
    let script = expected_hook_path()?;
    if needs_migration(provider) {
        return Ok(report(
            ProviderState::RepairRequired,
            Some("repair_required"),
        ));
    }
    if is_connected(provider, &home, &script) {
        return Ok(report(ProviderState::Connected, None));
    }
    if !provider.is_present(&home) {
        return Ok(report(ProviderState::NotInstalled, Some("not_detected")));
    }
    let config = provider.config_file(&home);
    if fs::metadata(&config).is_ok_and(|m| m.permissions().readonly()) {
        return Ok(report(
            ProviderState::PermissionRequired,
            Some("readonly_config"),
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
        .map_err(|e| format!("E_POWERSHELL:{e}"))?;
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
    Err("E_UNSUPPORTED_OS".into())
}

#[cfg(windows)]
const SEND_EVENT_SCRIPT: &str = include_str!("../../scripts/send-event.ps1");

/// Keeps an already installed bridge script current after an app update, so bridge fixes reach existing
/// hooks without the user pressing Repair. It only touches our own file and only when it differs.
#[cfg(windows)]
pub fn refresh_bridge_script() {
    let Ok(path) = expected_hook_path() else {
        return;
    };
    if !path.is_file() || fs::read_to_string(&path).is_ok_and(|c| c == SEND_EVENT_SCRIPT) {
        return;
    }
    let tmp = path.with_extension("ps1.tmp");
    if fs::write(&tmp, SEND_EVENT_SCRIPT).is_ok() && fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(not(windows))]
pub fn refresh_bridge_script() {}

#[cfg(windows)]
fn write_scripts(dir: &Path) -> Result<(), String> {
    const INSTALL: &str = include_str!("../../scripts/install-hooks.ps1");
    const UNINSTALL: &str = include_str!("../../scripts/uninstall-hooks.ps1");
    const SEND: &str = SEND_EVENT_SCRIPT;
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
    Err("E_UNSUPPORTED_OS".into())
}

/// RAII guard so only one install/uninstall runs at a time.
pub struct BusyGuard<'a>(&'a AtomicBool);
impl<'a> BusyGuard<'a> {
    pub fn acquire(flag: &'a AtomicBool) -> Result<Self, String> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self(flag))
            .map_err(|_| "E_BUSY".to_string())
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
        emit(app, operation, provider, "waiting", report.state, "waiting");
    }
    let mut failed: Vec<(&'static str, String)> = Vec::new();
    for provider in ALL {
        let current = detect_one(provider)?;
        let actionable = if install {
            matches!(
                current.state,
                ProviderState::Available
                    | ProviderState::PermissionRequired
                    | ProviderState::RepairRequired
            )
        } else {
            matches!(
                current.state,
                ProviderState::Connected | ProviderState::RepairRequired
            )
        };
        if !actionable {
            let note = match current.state {
                ProviderState::Connected => "already_connected",
                ProviderState::NotInstalled => "skipped_not_detected",
                _ => "skipped",
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
                "installing"
            } else {
                "uninstalling"
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
                if install { "connected" } else { "removed" },
            ),
            (outcome, _) => {
                let message = outcome.err().unwrap_or_else(|| "E_VERIFY".into());
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

    // Independently computed with Python: base64(utf-16-le(script text)).
    const EXPECTED_B64: &str = "JABQAHIAbwBnAHIAZQBzAHMAUAByAGUAZgBlAHIAZQBuAGMAZQAgAD0AIAAnAFMAaQBsAGUAbgB0AGwAeQBDAG8AbgB0AGkAbgB1AGUAJwA7ACAAJABkACAAPQAgACQAZQBuAHYAOgBMAE8AQwBBAEwAQQBQAFAARABBAFQAQQA7ACAAaQBmACAAKAAtAG4AbwB0ACAAJABkACkAIAB7ACAAJABkACAAPQAgAFsARQBuAHYAaQByAG8AbgBtAGUAbgB0AF0AOgA6AEcAZQB0AEYAbwBsAGQAZQByAFAAYQB0AGgAKAAnAEwAbwBjAGEAbABBAHAAcABsAGkAYwBhAHQAaQBvAG4ARABhAHQAYQAnACkAIAB9ADsAIAAmACAAKABKAG8AaQBuAC0AUABhAHQAaAAgACQAZAAgACcASwBhAGkAdwBlAG4ATgBvAHQAaQBmAHkAXABoAG8AbwBrAHMAXABzAGUAbgBkAC0AZQB2AGUAbgB0AC4AcABzADEAJwApACAALQBTAG8AdQByAGMAZQAgAGEAbgB0AGkAZwByAGEAdgBpAHQAeQA=";

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn antigravity_command_is_the_documented_encoded_form() {
        let cmd = antigravity_command();
        assert_eq!(
            cmd,
            format!("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -OutputFormat Text -EncodedCommand {EXPECTED_B64}")
        );
        assert!(
            !cmd.contains('"') && !cmd.contains('\\'),
            "no quotes or backslashes to mangle"
        );
        assert!(
            !cmd.to_lowercase().contains("users"),
            "no user-specific path"
        );
        assert!(cmd.len() < 700);
    }

    #[test]
    fn classifies_every_known_antigravity_form() {
        let current = antigravity_command();
        assert_eq!(classify_antigravity(&current), Some(HandlerKind::Current));
        let p = "C:/Users/me/AppData/Local/KaiwenNotify/hooks/send-event.ps1";
        let base = "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File";
        // v0.5.0 quoted forward slashes and v0.4.0 quoted backslashes: both broken under Antigravity
        assert_eq!(
            classify_antigravity(&format!("{base} \"{p}\" -Source antigravity")),
            Some(HandlerKind::Legacy)
        );
        assert_eq!(
            classify_antigravity(&format!(
                "{base} \"{}\" -Source antigravity",
                p.replace('/', "\\")
            )),
            Some(HandlerKind::Legacy)
        );
        // hand-fixed unquoted form works only while the path is plain ASCII without spaces
        assert_eq!(
            classify_antigravity(&format!("{base} {p} -Source antigravity")),
            Some(HandlerKind::Compatible)
        );
        let spaced = "C:/Users/John Smith/AppData/Local/KaiwenNotify/hooks/send-event.ps1";
        assert_eq!(
            classify_antigravity(&format!("{base} {spaced} -Source antigravity")),
            Some(HandlerKind::Legacy)
        );
        let turkish = "C:/Users/Çağrı/AppData/Local/KaiwenNotify/hooks/send-event.ps1";
        assert_eq!(
            classify_antigravity(&format!("{base} {turkish} -Source antigravity")),
            Some(HandlerKind::Legacy)
        );
    }

    #[test]
    fn foreign_commands_are_never_classified_as_ours() {
        for cmd in [
            "echo hi",
            "powershell.exe -NoProfile -File C:/other/script.ps1 -Source antigravity",
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:/x/send-event.ps1 -Source antigravity",
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:/x/KaiwenNotify/hooks/send-event.ps1 -Source claude",
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand AAAA",
        ] {
            assert_eq!(classify_antigravity(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn antigravity_detection_reads_the_shared_group_only() {
        let current = antigravity_command();
        let root = json!({
            "kaiwen-notify": {"enabled": true, "Stop": [
                {"type":"command","command":"echo someone else"},
                {"type":"command","command": current}]},
            "kaiwen-temp-probe": {"PreInvocation": [{"type":"command","command": current}]}
        });
        assert_eq!(antigravity_kinds(&root), vec![HandlerKind::Current]);
        assert!(antigravity_kinds(&json!({"other": {}})).is_empty());
        assert!(antigravity_kinds(&serde_json::Value::Null).is_empty());
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
