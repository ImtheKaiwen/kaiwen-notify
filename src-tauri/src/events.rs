//! Central classification of hook events.
//!
//! The hook bridge forwards only a short whitelist of metadata (never prompts or message text). This module is
//! the single place that turns that raw metadata into a provider-independent category. Playback and the UI only
//! ever see `Category`; they never look at provider JSON fields.
//!
//! Rule of thumb: an event is only called a sub-agent completion when the provider says so explicitly
//! (`SubagentStop` with an agent id and type). Anything ambiguous is `Unknown` and stays silent.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Antigravity,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Claude, Provider::Codex, Provider::Antigravity];

    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Antigravity => "Antigravity",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    /// Whether this provider exposes a documented, per-sub-agent "finished" hook event we can use.
    /// Claude Code (`SubagentStop`) and Codex (`SubagentStop`) do; Antigravity documents none.
    pub fn has_subagent_event(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    MainComplete,
    SubagentComplete,
    Unknown,
}

/// Which Antigravity runtime sent the event (they share one hooks file).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    App,
    Ide,
    Cli,
}

impl Runtime {
    pub fn key(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Ide => "ide",
            Self::Cli => "cli",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Note {
    NotIdle,
    StoppedEarly,
}

impl Note {
    pub fn key(self) -> &'static str {
        match self {
            Self::NotIdle => "not_idle",
            Self::StoppedEarly => "stopped_early",
        }
    }
}

/// What the bridge sends over UDP. Every field except `source` is optional so older bridges keep working
/// (they only send `source` and `kind`). Unknown fields are ignored for forward compatibility.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct RawEvent {
    pub source: Option<Provider>,
    /// `completed` (play a sound) or `seen` (diagnostics only).
    pub kind: Option<String>,
    /// Provider hook name, e.g. `Stop` or `SubagentStop`.
    pub hook: Option<String>,
    pub session: Option<String>,
    pub turn: Option<String>,
    pub agent: Option<String>,
    pub agent_type: Option<String>,
    pub product: Option<Runtime>,
    pub note: Option<Note>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classified {
    pub provider: Provider,
    pub runtime: Option<Runtime>,
    pub category: Category,
    /// Diagnostics only: the provider reported the hook but no sound should follow.
    pub silent_note: Option<Note>,
    /// Identity used to drop duplicate deliveries of the same event; `None` when the event carries no ids.
    pub dedupe_key: Option<String>,
}

fn clean(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

pub fn classify(raw: &RawEvent) -> Option<Classified> {
    let provider = raw.source?;
    let hook = clean(&raw.hook);
    let agent = clean(&raw.agent);
    let agent_type = clean(&raw.agent_type);
    let session = clean(&raw.session);
    let turn = clean(&raw.turn);

    let category = match (raw.kind.as_deref(), hook) {
        // Diagnostics-only report (Antigravity "ran but stayed silent").
        (Some("seen"), _) => Category::Unknown,
        // A SubagentStop is a sub-agent completion only with an explicit id and type, and only for providers
        // that document the event. Empty `agent_type` is how Claude reports internal helpers (prompt
        // suggestions, side questions): never a real sub-agent.
        (_, Some("SubagentStop")) if provider.has_subagent_event() => {
            if agent.is_some() && agent_type.is_some() {
                Category::SubagentComplete
            } else {
                Category::Unknown
            }
        }
        // A Stop that carries an agent id came from inside a sub-agent; it must never play the main sound.
        (_, Some("Stop")) if agent.is_some() => Category::Unknown,
        (_, Some("Stop")) => Category::MainComplete,
        // Any other named hook is not a completion we understand.
        (_, Some(_)) => Category::Unknown,
        // Legacy bridges (<= 0.5.1) send only `kind: completed`, and only from Stop hooks.
        (Some("completed"), None) | (None, None) => Category::MainComplete,
        (Some(_), None) => Category::Unknown,
    };

    let dedupe_key = match (session, turn, agent) {
        (None, None, None) => None,
        _ => Some(format!(
            "{}|{:?}|{}|{}|{}",
            provider.key(),
            category,
            session.unwrap_or(""),
            turn.unwrap_or(""),
            agent.unwrap_or("")
        )),
    };

    Some(Classified {
        provider,
        runtime: raw.product,
        category,
        silent_note: raw.note,
        dedupe_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> RawEvent {
        serde_json::from_str(json).expect("valid event JSON")
    }

    fn category(json: &str) -> Category {
        classify(&parse(json)).expect("classified").category
    }

    #[test]
    fn legacy_messages_stay_main_completions() {
        assert_eq!(
            category(r#"{"source":"claude","kind":"completed"}"#),
            Category::MainComplete
        );
        assert_eq!(
            category(r#"{"source":"codex","kind":"completed"}"#),
            Category::MainComplete
        );
        assert_eq!(
            category(r#"{"source":"antigravity","kind":"completed","product":"app"}"#),
            Category::MainComplete
        );
    }

    #[test]
    fn claude_stop_is_main_and_subagentstop_is_subagent() {
        assert_eq!(
            category(r#"{"source":"claude","kind":"completed","hook":"Stop","session":"s1"}"#),
            Category::MainComplete
        );
        assert_eq!(
            category(
                r#"{"source":"claude","kind":"completed","hook":"SubagentStop","session":"s1","agent":"a1","agent_type":"Explore"}"#
            ),
            Category::SubagentComplete
        );
    }

    #[test]
    fn subagent_needs_explicit_id_and_type() {
        // Internal helpers report an empty agent_type: they are not sub-agents.
        for json in [
            r#"{"source":"claude","hook":"SubagentStop","session":"s","agent":"a1","agent_type":""}"#,
            r#"{"source":"claude","hook":"SubagentStop","session":"s","agent":"","agent_type":"Explore"}"#,
            r#"{"source":"claude","hook":"SubagentStop","session":"s"}"#,
            r#"{"source":"claude","hook":"SubagentStop","agent":"  ","agent_type":"  "}"#,
        ] {
            assert_eq!(category(json), Category::Unknown, "{json}");
        }
    }

    #[test]
    fn stop_from_inside_a_subagent_never_counts_as_main() {
        assert_eq!(
            category(
                r#"{"source":"codex","hook":"Stop","session":"s","agent":"a7","agent_type":"worker"}"#
            ),
            Category::Unknown
        );
    }

    #[test]
    fn codex_subagentstop_is_supported_like_claude() {
        assert_eq!(
            category(
                r#"{"source":"codex","hook":"SubagentStop","session":"s","turn":"t","agent":"a","agent_type":"explorer"}"#
            ),
            Category::SubagentComplete
        );
        assert_eq!(
            category(r#"{"source":"codex","hook":"Stop","session":"s","turn":"t"}"#),
            Category::MainComplete
        );
    }

    #[test]
    fn antigravity_never_produces_subagent_events() {
        // No documented per-sub-agent event: even a forged SubagentStop must not become a sub-agent sound.
        assert_eq!(
            category(
                r#"{"source":"antigravity","hook":"SubagentStop","agent":"a","agent_type":"x","product":"app"}"#
            ),
            Category::Unknown
        );
        assert!(!Provider::Antigravity.has_subagent_event());
    }

    #[test]
    fn seen_events_are_silent_diagnostics() {
        let event = classify(&parse(
            r#"{"source":"antigravity","kind":"seen","note":"not_idle","product":"ide"}"#,
        ))
        .unwrap();
        assert_eq!(event.category, Category::Unknown);
        assert_eq!(event.silent_note, Some(Note::NotIdle));
        assert_eq!(event.runtime, Some(Runtime::Ide));
    }

    #[test]
    fn unknown_hooks_and_kinds_stay_silent() {
        assert_eq!(
            category(r#"{"source":"claude","hook":"PreToolUse"}"#),
            Category::Unknown
        );
        assert_eq!(
            category(r#"{"source":"claude","kind":"mystery"}"#),
            Category::Unknown
        );
    }

    #[test]
    fn missing_fields_and_bad_input_are_tolerated() {
        assert!(classify(&parse(r#"{}"#)).is_none(), "no provider: dropped");
        assert!(
            serde_json::from_str::<RawEvent>(r#"{"source":"gemini"}"#).is_err(),
            "unknown provider rejected"
        );
        let extra = parse(r#"{"source":"claude","kind":"completed","future_field":{"a":1}}"#);
        assert_eq!(classify(&extra).unwrap().category, Category::MainComplete);
    }

    #[test]
    fn dedupe_keys_separate_real_tasks_and_agents() {
        let key = |json: &str| classify(&parse(json)).unwrap().dedupe_key;
        assert_eq!(
            key(r#"{"source":"claude","kind":"completed"}"#),
            None,
            "no ids: time window only"
        );
        let a = key(r#"{"source":"claude","hook":"Stop","session":"s1","turn":"p1"}"#);
        let b = key(r#"{"source":"claude","hook":"Stop","session":"s1","turn":"p2"}"#);
        let c = key(r#"{"source":"claude","hook":"Stop","session":"s2","turn":"p1"}"#);
        assert!(
            a.is_some() && a != b && a != c,
            "different turns/sessions are different events"
        );
        let s1 = key(
            r#"{"source":"claude","hook":"SubagentStop","session":"s","agent":"x","agent_type":"E"}"#,
        );
        let s2 = key(
            r#"{"source":"claude","hook":"SubagentStop","session":"s","agent":"y","agent_type":"E"}"#,
        );
        assert!(s1 != s2, "parallel sub-agents are not merged by identity");
        assert_eq!(
            a,
            key(r#"{"source":"claude","hook":"Stop","session":"s1","turn":"p1"}"#),
            "same event, same key"
        );
    }

    #[test]
    fn provider_keys_round_trip() {
        for p in Provider::ALL {
            assert_eq!(Provider::from_key(p.key()), Some(p));
        }
        assert_eq!(Provider::from_key("nope"), None);
    }
}
