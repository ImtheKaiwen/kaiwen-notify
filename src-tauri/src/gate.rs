//! Decides whether an incoming event should make a sound: duplicate filtering, merging of simultaneous
//! sub-agent completions, and main-task priority. Pure logic with injected time so it is fully unit tested;
//! no timers, threads or polling live here.

use crate::events::{Category, Classified, Provider};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Same event identity (session/turn/agent) delivered twice within this window is one event.
pub const ID_WINDOW: Duration = Duration::from_secs(3);
/// Events without any id fall back to "same provider and category within this window" (the pre-0.6 behaviour).
pub const NO_ID_WINDOW: Duration = Duration::from_millis(650);
/// A sub-agent sound waits this long so a main completion arriving right after can replace it (no blip).
pub const SUBAGENT_DELAY: Duration = Duration::from_millis(150);
/// A sub-agent completion arriving this soon after (or during) a main sound is not played.
pub const MAIN_GUARD: Duration = Duration::from_millis(1500);
/// Remembered identities older than this are forgotten, so the map cannot grow without bound.
const PRUNE_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Play,
    /// Not played; the reason is a stable code used by tests and diagnostics.
    Skip(&'static str),
}

#[derive(Default)]
pub struct Gate {
    by_id: HashMap<String, Instant>,
    by_kind: HashMap<(Provider, Category), Instant>,
    last_main: Option<Instant>,
    main_until: Option<Instant>,
    sub_until: Option<Instant>,
}

impl Gate {
    /// First stage: duplicates and unknown events. Mutates dedupe memory only when the event is accepted.
    pub fn decide(&mut self, event: &Classified, now: Instant) -> Verdict {
        if event.category == Category::Unknown {
            return Verdict::Skip("unknown");
        }
        self.by_id
            .retain(|_, seen| now.duration_since(*seen) < PRUNE_AFTER);

        if let Some(key) = &event.dedupe_key {
            if self
                .by_id
                .get(key)
                .is_some_and(|seen| now.duration_since(*seen) < ID_WINDOW)
            {
                return Verdict::Skip("duplicate");
            }
        } else if self
            .by_kind
            .get(&(event.provider, event.category))
            .is_some_and(|seen| now.duration_since(*seen) < NO_ID_WINDOW)
        {
            return Verdict::Skip("duplicate");
        }

        match event.category {
            Category::SubagentComplete => {
                if self.main_until.is_some_and(|until| until > now)
                    || self
                        .last_main
                        .is_some_and(|at| now.duration_since(at) < MAIN_GUARD)
                {
                    self.remember(event, now); // still the same event if it is redelivered
                    return Verdict::Skip("main_priority");
                }
                if self.sub_until.is_some_and(|until| until > now) {
                    self.remember(event, now);
                    return Verdict::Skip("merged");
                }
            }
            Category::MainComplete | Category::Unknown => {}
        }
        self.remember(event, now);
        if event.category == Category::SubagentComplete {
            // Reserve the slot immediately: sub-agents finishing during the queue delay merge into this one.
            self.sub_until = Some(now + SUBAGENT_DELAY + Duration::from_millis(550));
        }
        Verdict::Play
    }

    fn remember(&mut self, event: &Classified, now: Instant) {
        if let Some(key) = &event.dedupe_key {
            self.by_id.insert(key.clone(), now);
        }
        self.by_kind.insert((event.provider, event.category), now);
    }

    /// Call when a main sound starts: it takes priority over any sub-agent sound.
    pub fn main_started(&mut self, now: Instant, sound: Duration) {
        self.last_main = Some(now);
        self.main_until = Some(now + sound + Duration::from_millis(300));
        self.sub_until = None; // a sub-agent sound still playing is interrupted by the main one
    }

    /// Call when a sub-agent sound starts: further sub-agent completions during it are merged into it.
    pub fn subagent_started(&mut self, now: Instant, sound: Duration) {
        self.sub_until =
            Some(now + sound.max(Duration::from_millis(400)) + Duration::from_millis(150));
    }

    /// After `SUBAGENT_DELAY`, is a queued sub-agent sound still wanted? Not if a main sound started meanwhile.
    pub fn subagent_still_wanted(&self, queued_at: Instant) -> bool {
        !self.last_main.is_some_and(|at| at >= queued_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(provider: Provider, category: Category, key: Option<&str>) -> Classified {
        Classified {
            provider,
            runtime: None,
            category,
            silent_note: None,
            dedupe_key: key.map(str::to_owned),
        }
    }

    const SOUND: Duration = Duration::from_millis(700);

    #[test]
    fn unknown_events_never_play() {
        let mut gate = Gate::default();
        assert_eq!(
            gate.decide(
                &event(Provider::Claude, Category::Unknown, None),
                Instant::now()
            ),
            Verdict::Skip("unknown")
        );
    }

    #[test]
    fn same_identity_twice_is_one_notification_but_new_turns_are_not_lost() {
        let mut gate = Gate::default();
        let t = Instant::now();
        let first = event(
            Provider::Claude,
            Category::MainComplete,
            Some("claude|s|p1"),
        );
        assert_eq!(gate.decide(&first, t), Verdict::Play);
        assert_eq!(
            gate.decide(&first, t + Duration::from_millis(900)),
            Verdict::Skip("duplicate")
        );
        // the next prompt of the same session, even a second later, is a legitimate new notification
        let next = event(
            Provider::Claude,
            Category::MainComplete,
            Some("claude|s|p2"),
        );
        assert_eq!(
            gate.decide(&next, t + Duration::from_millis(1000)),
            Verdict::Play
        );
        // and the original identity can complete again after the id window
        assert_eq!(
            gate.decide(&first, t + ID_WINDOW + Duration::from_millis(1)),
            Verdict::Play
        );
    }

    #[test]
    fn events_without_ids_use_the_short_window_per_provider() {
        let mut gate = Gate::default();
        let t = Instant::now();
        let claude = event(Provider::Claude, Category::MainComplete, None);
        let codex = event(Provider::Codex, Category::MainComplete, None);
        assert_eq!(gate.decide(&claude, t), Verdict::Play);
        assert_eq!(
            gate.decide(&claude, t + Duration::from_millis(300)),
            Verdict::Skip("duplicate")
        );
        assert_eq!(
            gate.decide(&codex, t + Duration::from_millis(300)),
            Verdict::Play,
            "other provider is independent"
        );
        assert_eq!(
            gate.decide(&claude, t + NO_ID_WINDOW + Duration::from_millis(1)),
            Verdict::Play
        );
    }

    #[test]
    fn parallel_subagents_merge_into_one_sound() {
        let mut gate = Gate::default();
        let t = Instant::now();
        let sub = |id: &str| {
            event(
                Provider::Claude,
                Category::SubagentComplete,
                Some(&format!("claude|s||{id}")),
            )
        };
        assert_eq!(gate.decide(&sub("a"), t), Verdict::Play);
        gate.subagent_started(t, SOUND);
        for (i, id) in ["b", "c", "d", "e"].iter().enumerate() {
            let at = t + Duration::from_millis(40 * (i as u64 + 1));
            assert_eq!(gate.decide(&sub(id), at), Verdict::Skip("merged"), "{id}");
        }
        // once the merged sound is over, a later sub-agent is announced again
        assert_eq!(
            gate.decide(&sub("late"), t + Duration::from_secs(3)),
            Verdict::Play
        );
    }

    #[test]
    fn main_completion_has_priority_over_subagents() {
        let mut gate = Gate::default();
        let t = Instant::now();
        let main = event(
            Provider::Claude,
            Category::MainComplete,
            Some("claude|s|p1"),
        );
        assert_eq!(gate.decide(&main, t), Verdict::Play);
        gate.main_started(t, SOUND);
        let sub = event(
            Provider::Claude,
            Category::SubagentComplete,
            Some("claude|s||a"),
        );
        assert_eq!(
            gate.decide(&sub, t + Duration::from_millis(100)),
            Verdict::Skip("main_priority")
        );
        // still suppressed shortly after the sound ended (guard window), but not forever
        assert_eq!(
            gate.decide(
                &event(
                    Provider::Claude,
                    Category::SubagentComplete,
                    Some("claude|s||b")
                ),
                t + Duration::from_millis(1400)
            ),
            Verdict::Skip("main_priority")
        );
        assert_eq!(
            gate.decide(
                &event(
                    Provider::Claude,
                    Category::SubagentComplete,
                    Some("claude|s||c")
                ),
                t + MAIN_GUARD + SOUND + Duration::from_secs(1)
            ),
            Verdict::Play
        );
    }

    #[test]
    fn main_replaces_a_queued_subagent_sound() {
        let mut gate = Gate::default();
        let t = Instant::now();
        let sub = event(
            Provider::Claude,
            Category::SubagentComplete,
            Some("claude|s||a"),
        );
        assert_eq!(gate.decide(&sub, t), Verdict::Play);
        assert!(gate.subagent_still_wanted(t), "nothing else happened");
        // the main completion arrives inside the queue delay
        let main = event(
            Provider::Claude,
            Category::MainComplete,
            Some("claude|s|p1"),
        );
        assert_eq!(
            gate.decide(&main, t + Duration::from_millis(60)),
            Verdict::Play
        );
        gate.main_started(t + Duration::from_millis(60), SOUND);
        assert!(
            !gate.subagent_still_wanted(t),
            "the queued sub-agent sound must be dropped"
        );
    }

    #[test]
    fn main_completions_of_different_providers_both_play() {
        let mut gate = Gate::default();
        let t = Instant::now();
        assert_eq!(
            gate.decide(
                &event(Provider::Claude, Category::MainComplete, Some("claude|a|1")),
                t
            ),
            Verdict::Play
        );
        gate.main_started(t, SOUND);
        assert_eq!(
            gate.decide(
                &event(Provider::Codex, Category::MainComplete, Some("codex|b|1")),
                t + Duration::from_millis(50)
            ),
            Verdict::Play
        );
    }

    #[test]
    fn dedupe_memory_is_bounded() {
        let mut gate = Gate::default();
        let t = Instant::now();
        for i in 0..500 {
            let key = format!("claude|s|{i}");
            let _ = gate.decide(
                &event(Provider::Claude, Category::MainComplete, Some(&key)),
                t + Duration::from_secs(i),
            );
        }
        assert!(
            gate.by_id.len() <= 61,
            "old identities are pruned, got {}",
            gate.by_id.len()
        );
    }
}
