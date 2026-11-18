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
    /// Sub-agent notification capability: `unsupported` (no documented sub-agent event), `hook_missing`
    /// (supported, SubagentStop hook not registered) or `ready` (hook registered).
    pub subagent: &'static str,
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

/// Whether sub-agent notifications can work for this provider right now. Based on what is really registered in
/// the provider's config, never on a hard-coded assumption. Antigravity documents no sub-agent finished event.
fn subagent_state(provider: Provider) -> &'static str {
    if provider == Provider::Antigravity {
        return "unsupported";
    }
    let (Ok(home), Ok(script)) = (home_dir(), expected_hook_path()) else {
        return "hook_missing";
    };
    let json = read_json_optional(&provider.config_file(&home));
    if has_own_hook(
        &json,
        &["hooks", "SubagentStop"],
        &hook_command(&script, provider.id()),
    ) {
        "ready"
    } else {
        "hook_missing"
    }
}

pub fn detect_one(provider: Provider) -> Result<ProviderReport, String> {
    let subagent = subagent_state(provider);
    let report = |state, detail: Option<&str>| ProviderReport {
        id: provider.id(),
        state,
        detail: detail.map(str::to_owned),
        subagent,
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
fn run_script(
    scripts_dir: &Path,
    install: bool,
    provider: Provider,
    subagent: SubagentMode,
) -> Result<(), String> {
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
        .args(subagent.args(install))
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("E_POWERSHELL:{e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let details = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "E_SCRIPT:{}",
        details
            .lines()
            .next()
            .unwrap_or("unknown")
            .chars()
            .take(200)
            .collect::<String>()
    ))
}

#[cfg(not(windows))]
fn run_script(_: &Path, _: bool, _: Provider, _: SubagentMode) -> Result<(), String> {
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

/// What an install/repair run does with the optional SubagentStop hook.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SubagentMode {
    /// Leave it exactly as it is.
    Keep,
    /// Make sure it is registered (only meaningful for providers that document the event).
    On,
    /// Only manage the SubagentStop hook (used when toggling sub-agent notifications).
    OnlySet(bool),
}

impl SubagentMode {
    fn args(self, install: bool) -> Vec<&'static str> {
        if !install {
            return Vec::new(); // uninstall always removes our SubagentStop hook itself
        }
        match self {
            Self::Keep => Vec::new(),
            Self::On => vec!["-Subagent", "on"],
            Self::OnlySet(true) => vec!["-Subagent", "on", "-SubagentOnly"],
            Self::OnlySet(false) => vec!["-Subagent", "off", "-SubagentOnly"],
        }
    }
}

/// Raw copy of a provider's config file, used to restore it exactly if a multi-provider change fails halfway.
type Snapshot = Option<Vec<u8>>;

/// The side effects of toggling the SubagentStop hook, separated so partial failures can be tested.
trait HookOps {
    fn connected(&self, provider: Provider) -> bool;
    fn is_on(&self, provider: Provider) -> bool;
    fn snapshot(&self, provider: Provider) -> Result<Snapshot, String>;
    fn apply(&mut self, provider: Provider, on: bool) -> Result<(), String>;
    fn restore(&mut self, provider: Provider, snapshot: &Snapshot) -> Result<(), String>;
}

/// Error code when a change failed and everything was put back as it was.
pub const E_HOOKS_UNCHANGED: &str = "E_SUBAGENT_HOOKS";
/// Prefix of the error code when a change failed AND undoing it failed too: `E_SUBAGENT_ROLLBACK:claude,codex`
/// lists the providers whose real state is unknown.
pub const E_HOOKS_INCONSISTENT: &str = "E_SUBAGENT_ROLLBACK";

/// Applies the hook change provider by provider. If any step fails, the providers already changed (and the one
/// that failed, which may be half written) are restored from their snapshots. Rollback failures are reported
/// by name, never hidden. Providers already in the wanted state are not touched, so the call is repeatable.
fn sync_with(ops: &mut dyn HookOps, enabled: bool) -> Result<(), String> {
    let mut done: Vec<(Provider, Snapshot)> = Vec::new();
    for provider in ALL {
        if provider == Provider::Antigravity || !ops.connected(provider) {
            continue; // never touched / never install a Stop hook as a side effect
        }
        if ops.is_on(provider) == enabled {
            continue;
        }
        let step = ops.snapshot(provider).and_then(|snapshot| {
            // recorded before the change: the provider that fails may be half written and needs restoring too
            done.push((provider, snapshot));
            ops.apply(provider, enabled)
        });
        if let Err(error) = step {
            let mut stuck: Vec<&'static str> = Vec::new();
            for (changed, saved) in done.iter().rev() {
                if ops.restore(*changed, saved).is_err() {
                    stuck.push(changed.id());
                }
            }
            return Err(if stuck.is_empty() {
                format!("{E_HOOKS_UNCHANGED}:{error}")
            } else {
                stuck.reverse();
                format!("{E_HOOKS_INCONSISTENT}:{}", stuck.join(","))
            });
        }
    }
    Ok(())
}

struct RealHookOps<'a> {
    scripts_dir: &'a Path,
}

impl HookOps for RealHookOps<'_> {
    fn connected(&self, provider: Provider) -> bool {
        matches!(
            detect_one(provider).map(|r| r.state),
            Ok(ProviderState::Connected | ProviderState::RepairRequired)
        )
    }
    fn is_on(&self, provider: Provider) -> bool {
        subagent_state(provider) == "ready"
    }
    fn snapshot(&self, provider: Provider) -> Result<Snapshot, String> {
        let file = provider.config_file(&home_dir()?);
        match fs::read(&file) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn apply(&mut self, provider: Provider, on: bool) -> Result<(), String> {
        run_script(self.scripts_dir, true, provider, SubagentMode::OnlySet(on))
    }
    fn restore(&mut self, provider: Provider, snapshot: &Snapshot) -> Result<(), String> {
        let file = provider.config_file(&home_dir()?);
        match snapshot {
            None => match fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            },
            Some(bytes) => {
                let tmp = file.with_extension("kaiwen-restore.tmp");
                fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
                fs::rename(&tmp, &file).map_err(|e| {
                    let _ = fs::remove_file(&tmp);
                    e.to_string()
                })
            }
        }
    }
}

/// Registers or removes the SubagentStop hook for connected providers that support it. Antigravity is never
/// touched. Idempotent. All-or-nothing: on a partial failure the already changed providers are restored; see
/// `sync_with` for the error codes.
pub fn sync_subagent_hooks(scripts_dir: &Path, enabled: bool) -> Result<(), String> {
    write_scripts(scripts_dir)?;
    sync_with(&mut RealHookOps { scripts_dir }, enabled)
}

pub fn run_operation(
    app: &AppHandle,
    scripts_dir: &Path,
    install: bool,
    subagent_on: bool,
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
        let mode = if subagent_on && provider != Provider::Antigravity {
            SubagentMode::On
        } else {
            SubagentMode::Keep
        };
        let outcome = run_script(scripts_dir, install, provider, mode);
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

    #[derive(Default)]
    struct Fake {
        state: std::collections::HashMap<&'static str, (bool, bool, Vec<u8>)>, // connected, on, file
        fail_apply: Option<&'static str>,
        fail_restore: Vec<&'static str>,
        applied: Vec<(&'static str, bool)>,
    }

    impl Fake {
        fn with(claude_on: bool, codex_on: bool) -> Self {
            let mut fake = Self::default();
            fake.state
                .insert("claude", (true, claude_on, b"claude-0".to_vec()));
            fake.state
                .insert("codex", (true, codex_on, b"codex-0".to_vec()));
            fake.state
                .insert("antigravity", (true, false, b"anti-0".to_vec()));
            fake
        }
    }

    impl HookOps for Fake {
        fn connected(&self, p: Provider) -> bool {
            self.state[p.id()].0
        }
        fn is_on(&self, p: Provider) -> bool {
            self.state[p.id()].1
        }
        fn snapshot(&self, p: Provider) -> Result<Snapshot, String> {
            Ok(Some(self.state[p.id()].2.clone()))
        }
        fn apply(&mut self, p: Provider, on: bool) -> Result<(), String> {
            self.applied.push((p.id(), on));
            let entry = self.state.get_mut(p.id()).unwrap();
            entry.2.extend_from_slice(b"-half-written");
            if self.fail_apply == Some(p.id()) {
                return Err("E_SCRIPT:boom".into());
            }
            entry.1 = on;
            Ok(())
        }
        fn restore(&mut self, p: Provider, snapshot: &Snapshot) -> Result<(), String> {
            if self.fail_restore.contains(&p.id()) {
                return Err("denied".into());
            }
            let entry = self.state.get_mut(p.id()).unwrap();
            entry.2 = snapshot.clone().unwrap();
            Ok(())
        }
    }

    #[test]
    fn sync_changes_both_providers_and_never_antigravity() {
        let mut fake = Fake::with(false, false);
        sync_with(&mut fake, true).unwrap();
        assert_eq!(fake.applied, vec![("claude", true), ("codex", true)]);
        sync_with(&mut fake, true).unwrap(); // repeat is a no-op (idempotent)
        assert_eq!(fake.applied.len(), 2);
    }

    #[test]
    fn sync_skips_providers_that_are_not_connected() {
        let mut fake = Fake::with(false, false);
        fake.state.get_mut("codex").unwrap().0 = false;
        sync_with(&mut fake, true).unwrap();
        assert_eq!(fake.applied, vec![("claude", true)]);
    }

    #[test]
    fn failure_of_the_second_provider_restores_the_first_and_itself() {
        let mut fake = Fake::with(true, true); // turning off: claude succeeds, codex fails
        fake.fail_apply = Some("codex");
        let error = sync_with(&mut fake, false).unwrap_err();
        assert!(error.starts_with(E_HOOKS_UNCHANGED), "{error}");
        assert_eq!(fake.state["claude"].2, b"claude-0", "claude restored");
        assert_eq!(
            fake.state["codex"].2, b"codex-0",
            "half-written codex restored"
        );
    }

    #[test]
    fn failure_of_the_first_provider_changes_nothing_else() {
        let mut fake = Fake::with(false, false);
        fake.fail_apply = Some("claude");
        assert!(sync_with(&mut fake, true)
            .unwrap_err()
            .starts_with(E_HOOKS_UNCHANGED));
        assert_eq!(
            fake.applied,
            vec![("claude", true)],
            "codex was never attempted"
        );
        assert_eq!(fake.state["claude"].2, b"claude-0");
        assert_eq!(fake.state["codex"].2, b"codex-0");
    }

    #[test]
    fn failed_rollback_is_reported_by_provider() {
        let mut fake = Fake::with(true, true);
        fake.fail_apply = Some("codex");
        fake.fail_restore = vec!["claude"];
        assert_eq!(
            sync_with(&mut fake, false).unwrap_err(),
            "E_SUBAGENT_ROLLBACK:claude"
        );
        assert!(
            !fake.state["claude"].1,
            "claude really is off, and the error says so"
        );
        assert_eq!(
            fake.state["codex"].2, b"codex-0",
            "codex was still restored"
        );
    }

    #[test]
    fn total_rollback_failure_lists_every_provider() {
        let mut fake = Fake::with(true, true);
        fake.fail_apply = Some("codex");
        fake.fail_restore = vec!["claude", "codex"];
        assert_eq!(
            sync_with(&mut fake, false).unwrap_err(),
            "E_SUBAGENT_ROLLBACK:claude,codex"
        );
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

    /// End-to-end through the real PowerShell scripts in a throw-away Windows profile (Turkish letters and spaces
    /// in the path): sub-agent hooks follow the setting and capability reporting follows what is registered.
    #[cfg(windows)]
    #[test]
    fn subagent_hooks_follow_the_setting_in_an_isolated_profile() {
        use std::sync::Mutex;
        static ENV: Mutex<()> = Mutex::new(());
        let _lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir()
            .join(format!("kn-int-{}", std::process::id()))
            .join("Çağrı Ünal (test)");
        let (home, local) = (base.join("home dir"), base.join("local app"));
        let _ = fs::remove_dir_all(&base);
        for dir in [".claude", ".codex", ".gemini/config"] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        fs::create_dir_all(&local).unwrap();
        fs::write(home.join(".claude/settings.json"), r#"{"theme":"dark"}"#).unwrap();
        fs::write(
            home.join(".codex/config.toml"),
            "notify = [\"keep\", \"me\"]
",
        )
        .unwrap();
        let (old_home, old_local) = (
            std::env::var_os("USERPROFILE"),
            std::env::var_os("LOCALAPPDATA"),
        );
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("LOCALAPPDATA", &local);
        let scripts = base.join("scripts");
        write_scripts(&scripts).unwrap();

        // nothing connected yet: sync is a no-op and installs nothing as a side effect
        sync_subagent_hooks(&scripts, true).unwrap();
        assert_eq!(
            detect_one(Provider::Claude).unwrap().state,
            ProviderState::Available
        );
        assert!(!home.join(".codex/hooks.json").exists());

        run_script(&scripts, true, Provider::Claude, SubagentMode::Keep).unwrap();
        run_script(&scripts, true, Provider::Codex, SubagentMode::Keep).unwrap();
        let reports = detect_all().unwrap();
        assert!(reports
            .iter()
            .filter(|r| r.id != "antigravity")
            .all(|r| r.state == ProviderState::Connected));
        assert!(reports
            .iter()
            .filter(|r| r.id != "antigravity")
            .all(|r| r.subagent == "hook_missing"));

        sync_subagent_hooks(&scripts, true).unwrap();
        let reports = detect_all().unwrap();
        for r in &reports {
            let expected = if r.id == "antigravity" {
                "unsupported"
            } else {
                "ready"
            };
            assert_eq!(r.subagent, expected, "{}", r.id);
        }
        sync_subagent_hooks(&scripts, true).unwrap(); // idempotent
        assert_eq!(detect_one(Provider::Claude).unwrap().subagent, "ready");

        sync_subagent_hooks(&scripts, false).unwrap();
        assert_eq!(
            detect_one(Provider::Claude).unwrap().subagent,
            "hook_missing"
        );
        assert_eq!(
            detect_one(Provider::Claude).unwrap().state,
            ProviderState::Connected,
            "Stop hook stays"
        );
        assert_eq!(
            fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
            "notify = [\"keep\", \"me\"]
"
        );

        run_script(&scripts, true, Provider::Claude, SubagentMode::On).unwrap(); // Repair with the setting on
        assert_eq!(detect_one(Provider::Claude).unwrap().subagent, "ready");
        run_script(&scripts, false, Provider::Claude, SubagentMode::Keep).unwrap(); // Uninstall removes both
        assert_eq!(
            detect_one(Provider::Claude).unwrap().state,
            ProviderState::Available
        );
        assert_eq!(
            detect_one(Provider::Claude).unwrap().subagent,
            "hook_missing"
        );

        // ---- partial failure with the real scripts: Claude succeeds, Codex fails -------------------------
        run_script(&scripts, true, Provider::Claude, SubagentMode::Keep).unwrap();
        let claude_file = home.join(".claude/settings.json");
        let codex_file = home.join(".codex/hooks.json");
        let mut claude_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&claude_file).unwrap()).unwrap();
        claude_json["hooks"]["SubagentStop"] =
            json!([{"hooks": [{"type": "command", "command": "echo theirs"}]}]);
        fs::write(
            &claude_file,
            serde_json::to_vec_pretty(&claude_json).unwrap(),
        )
        .unwrap();
        let (claude_before, codex_before) = (
            fs::read(&claude_file).unwrap(),
            fs::read(&codex_file).unwrap(),
        );
        struct FailCodex<'a>(RealHookOps<'a>, bool);
        impl HookOps for FailCodex<'_> {
            fn connected(&self, p: Provider) -> bool {
                self.0.connected(p)
            }
            fn is_on(&self, p: Provider) -> bool {
                self.0.is_on(p)
            }
            fn snapshot(&self, p: Provider) -> Result<Snapshot, String> {
                self.0.snapshot(p)
            }
            fn apply(&mut self, p: Provider, on: bool) -> Result<(), String> {
                if p == Provider::Codex {
                    return Err("E_SCRIPT:codex failed".into());
                }
                self.0.apply(p, on)
            }
            fn restore(&mut self, p: Provider, s: &Snapshot) -> Result<(), String> {
                if self.1 && p == Provider::Claude {
                    return Err("locked".into());
                }
                self.0.restore(p, s)
            }
        }
        // rollback works: both files come back byte for byte, third-party hook included
        let error = sync_with(
            &mut FailCodex(
                RealHookOps {
                    scripts_dir: &scripts,
                },
                false,
            ),
            true,
        )
        .unwrap_err();
        assert!(error.starts_with("E_SUBAGENT_HOOKS:"), "{error}");
        assert_eq!(fs::read(&claude_file).unwrap(), claude_before);
        assert_eq!(fs::read(&codex_file).unwrap(), codex_before);
        assert_eq!(
            detect_one(Provider::Claude).unwrap().subagent,
            "hook_missing"
        );
        // rollback fails too: the real state is reported, not hidden
        let error = sync_with(
            &mut FailCodex(
                RealHookOps {
                    scripts_dir: &scripts,
                },
                true,
            ),
            true,
        )
        .unwrap_err();
        assert_eq!(error, "E_SUBAGENT_ROLLBACK:claude");
        assert_eq!(detect_one(Provider::Claude).unwrap().subagent, "ready");
        assert_eq!(
            detect_one(Provider::Codex).unwrap().subagent,
            "hook_missing"
        );
        assert!(fs::read_to_string(&claude_file)
            .unwrap()
            .contains("echo theirs"));
        // a later normal sync repairs it, and turning it off keeps the third-party hook
        sync_subagent_hooks(&scripts, true).unwrap();
        sync_subagent_hooks(&scripts, false).unwrap();
        assert!(fs::read_to_string(&claude_file)
            .unwrap()
            .contains("echo theirs"));
        assert_eq!(
            detect_one(Provider::Claude).unwrap().subagent,
            "hook_missing"
        );

        match old_home {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match old_local {
            Some(v) => std::env::set_var("LOCALAPPDATA", v),
            None => std::env::remove_var("LOCALAPPDATA"),
        }
        let _ = fs::remove_dir_all(base.parent().unwrap());
    }
}
