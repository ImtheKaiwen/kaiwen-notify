# Notifications: events, sounds and sub-agents

How a hook turns into a sound, and why each decision was made.

## Pipeline

```
agent hook ──> send-event.ps1 ──UDP 127.0.0.1──> events.rs (classify) ──> gate.rs ──> audio.rs ──> PlaySound
                (whitelist only)                  provider-independent     duplicates,    decode on        WinMM
                                                  category                 merge, priority demand
```

- **Bridge** (`scripts/send-event.ps1`): reads the hook payload from stdin and forwards a short whitelist of metadata:
  hook name, session id, turn id, agent id, agent type (Claude Code / Codex) or conversation id, execution number and
  runtime (Antigravity). Each value is trimmed, stripped of control characters and capped at 100 characters. Prompts,
  assistant replies, file paths and transcripts are never put into the message (a test sends secrets and checks the
  datagram). Hooks always exit quietly; the agent is never blocked.
- **Classification** (`src-tauri/src/events.rs`): the only place that reads provider fields. Everything downstream sees
  a `Category`: `main_complete`, `subagent_complete` or `unknown`. Unknown events are silent.
- **Gate** (`src-tauri/src/gate.rs`): pure logic with injected time (unit tested), no timers or threads of its own.
- **Audio** (`src-tauri/src/audio.rs`): decode on demand, apply volume, hand an in-memory WAV to WinMM `PlaySound` on a
  short-lived thread. Nothing stays in memory between notifications.

Messages from older bridges (`{"source":"claude","kind":"completed"}`) are still accepted and count as main completions.

## What counts as what

| Provider | Main task | Sub-agent | Notes |
|---|---|---|---|
| Claude Code | `Stop` | `SubagentStop` **with** `agent_id` and `agent_type` | The docs say `Stop` fires for the main agent only. `SubagentStop` also fires for internal helpers (prompt suggestions, `/btw` side questions) with an **empty** `agent_type`; those are classified `unknown` and stay silent. A `Stop` that carries an `agent_id` is never played as a main completion. |
| Codex | `Stop` | `SubagentStop` with `agent_id` and `agent_type` | Documented in the Codex hooks reference (events `SubagentStart` / `SubagentStop`). The notify `agent-turn-complete` mechanism has no agent field, so it cannot tell sub-agents apart; Kaiwen Notify uses `hooks.json` and never touches `config.toml` `notify`. **Not yet verified with a real Codex sub-agent run.** |
| Antigravity | `Stop` (`fullyIdle`, not an error stop) | not supported | No documented per-sub-agent finished event. `fullyIdle` describes the whole agent (background commands finished), not a sub-agent. Only the main sound is offered. |

Sub-agent support in the UI is **derived from what is registered in the provider's config** (the `SubagentStop` hook with
our command), not from a hard-coded list: Antigravity is reported `unsupported`, Claude/Codex `hook_missing` until the
hook exists and `ready` afterwards. "Verified" appears only after a real sub-agent event has arrived.

## The SubagentStop hook is opt-in

Registering it costs a PowerShell process per finished sub-agent, so it is only added while the global
**Sub-agent notifications** switch is on (default off). The switch adds/removes only our own handler (matched by the
exact command) for providers that are already connected; third-party `SubagentStop` hooks are never touched.
Install/Repair keeps it when the switch is on, Uninstall always removes it. Codex asks you to trust the new hook
(`/hooks` or Settings > Hooks) before it runs.

## Duplicates, merging and priority

| Situation | Behaviour |
|---|---|
| Same event delivered twice (same provider, category, session, turn, agent) within 3 s | one sound |
| Event without any id, same provider and category within 650 ms | one sound (the pre-0.6 rule) |
| Next prompt/turn of the same session, even a second later | plays (different identity) |
| Several sub-agents finish together | one sub-agent sound; the rest are merged while it plays |
| Sub-agent finishes during or within 1.5 s after a main sound | not played (main has priority) |
| Main completion arrives within 150 ms after a sub-agent | the queued sub-agent sound is dropped, only the main plays |
| Different providers | independent |

Remembered identities are pruned after 60 s, so memory stays bounded.

## Sounds

- Per provider: enabled switch, volume 0-100 (perceptual curve), a **task sound** and a **sub-agent sound**; each can be
  previewed, changed or reset to the bundled default. There is also a master switch and the global sub-agent switch.
- Formats: WAV, MP3, Ogg Vorbis. Up to 10 MB and 30 s. Decoding uses Symphonia with only those three codecs compiled
  in; WinMM stays the output path, so there is no audio thread, no extra runtime and no resident decoder.
- A chosen file is validated by really decoding it, then copied into the app's data folder
  (`%APPDATA%\com.kaiwen.notify\sounds\<hash>.<ext>`). Moving or deleting the original changes nothing. The settings
  file stores only that file name; a tampered name that is not `<16 hex>.<wav|mp3|ogg>` falls back to the default.
  If the private copy itself disappears, the default plays and the UI says so.
- **Migration**: the single shared sound of earlier versions becomes the task sound of every provider on first start
  (custom sounds are kept). The old `sound_path` field is left in the file so downgrading still works.

## Security notes

- Playback/IPC commands take only a provider key and a sound kind, never a path to play. The one path accepted
  (`choose_sound`) comes from the system file dialog and goes through the extension allow-list, size limit and decoder
  before anything is stored.
- The PowerShell arguments added for the SubagentStop hook are fixed literals; nothing user-controlled reaches a
  command line. The scripts never evaluate strings.
- Settings are written atomically; hook files are backed up before every edit.
