"""v0.6 contracts: per-provider sounds, sub-agent events, central classification, lightness and privacy."""
import json
import re
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(rel):
    return (ROOT / rel).read_text(encoding='utf8')


# ---- settings model and migration ---------------------------------------------------------------
def test_subagent_notifications_default_off_and_legacy_field_kept():
    lib = read('src-tauri/src/lib.rs')
    assert 'subagent_notifications: false' in lib, 'off by default'
    assert 'sound_path: String' in lib, 'legacy field stays in the file (downgrade safe)'
    assert 'sound_config::migrate(' in lib and 'SOUNDS_VERSION' in read('src-tauri/src/sound_config.rs')


def test_three_providers_have_independent_settings():
    cfg = read('src-tauri/src/sound_config.rs')
    for field in ('pub claude: ProviderSound', 'pub codex: ProviderSound', 'pub antigravity: ProviderSound',
                  'pub enabled: bool', 'pub volume: u8', 'pub main: SoundChoice', 'pub subagent: SoundChoice'):
        assert field in cfg, field


def test_custom_sounds_are_private_copies_with_validated_names():
    cfg = read('src-tauri/src/sound_config.rs')
    assert 'pub fn import(' in cfg and 'audio::decode_bytes' in cfg, 'validated by really decoding'
    assert 'fn is_store_name' in cfg and 'fn resolve' in cfg
    lib = read('src-tauri/src/lib.rs')
    assert 'sounds_dir' in lib and 'sound_config::import' in lib
    # IPC never accepts a path for playback: only provider + kind
    assert re.search(r'fn preview_sound\(\s*state: tauri::State<\'_, AppState>,\s*provider: String,\s*kind: String', lib)


# ---- central classification ---------------------------------------------------------------------
def test_classification_lives_in_one_module():
    events = read('src-tauri/src/events.rs')
    for name in ('MainComplete', 'SubagentComplete', 'Unknown', 'pub fn classify'):
        assert name in events, name
    lib = read('src-tauri/src/lib.rs')
    # lib.rs/UI/playback never read raw provider fields such as hook names or agent ids
    for raw in ('"SubagentStop"', 'agent_type', 'hook_event_name', 'terminationReason'):
        assert raw not in lib, raw
    assert 'events::classify' in lib
    front = read('src/main.ts')
    for raw in ('SubagentStop', 'hook_event_name', 'agent_id'):
        assert raw not in front, raw


def test_only_documented_providers_get_subagent_events():
    events = read('src-tauri/src/events.rs')
    assert 'matches!(self, Self::Claude | Self::Codex)' in events
    assert 'Antigravity' in events and 'documents none' in events


# ---- lightness: no resident audio, no polling ---------------------------------------------------
def test_audio_is_on_demand_and_dependency_light():
    cargo = read('src-tauri/Cargo.toml')
    sym = re.search(r'^symphonia = (.+)$', cargo, re.M).group(1)
    assert 'default-features = false' in sym and 'mp3' in sym and 'vorbis' in sym
    for heavy in ('rodio', 'cpal', 'kira', 'rust-ffmpeg', 'gstreamer'):
        assert heavy not in cargo, heavy
    audio = read('src-tauri/src/audio.rs')
    assert 'PlaySoundW' in audio and 'SND_MEMORY' in audio and 'SND_SYNC' in audio
    assert 'SND_ASYNC' not in audio, 'async + memory buffers would need to outlive the call'
    lib = read('src-tauri/src/lib.rs')
    assert 'fn spawn_playback' in lib and 'thread::spawn' in lib.split('fn spawn_playback')[1][:300]
    assert not re.search(r'static [A-Z_]+: .*(Vec<u8>|Pcm)', lib + audio), 'decoded sounds are never cached'


def test_no_new_polling_or_timers():
    lib = read('src-tauri/src/lib.rs')
    gate = read('src-tauri/src/gate.rs')
    assert 'interval' not in gate.lower().replace('interval_minutes', '')
    # the only sleeps are the 150 ms sub-agent grace delay and the existing splash failsafe
    assert lib.count('thread::sleep') <= 3
    assert 'fn handle_event' in lib and 'socket.recv_from' in lib, 'event driven: blocks on the socket'


def test_tray_mode_does_not_gain_a_webview():
    lib = read('src-tauri/src/lib.rs')
    auto = lib[lib.index('let autostart ='):lib.index('start_update_scheduler(app.handle().clone());')]
    assert 'build_main' not in auto.split('} else if after_update')[0]
    conf = json.loads(read('src-tauri/tauri.conf.json'))
    assert conf['app']['windows'][0]['create'] is False


# ---- gate behaviour is wired in -----------------------------------------------------------------
def test_gate_rules_are_wired():
    gate = read('src-tauri/src/gate.rs')
    for rule in ('ID_WINDOW', 'NO_ID_WINDOW', 'SUBAGENT_DELAY', 'MAIN_GUARD', 'main_priority', 'merged', 'duplicate'):
        assert rule in gate, rule
    lib = read('src-tauri/src/lib.rs')
    for call in ('gate.decide', 'main_started', 'subagent_started', 'subagent_still_wanted'):
        assert call in lib, call


# ---- UI: unsupported features are disabled, not pretended ---------------------------------------
def test_ui_disables_unsupported_subagent_controls():
    front = read('src/main.ts')
    assert "report?.subagent === 'unsupported'" in front
    assert "(button as HTMLButtonElement).disabled = !usable" in front
    assert 'sound.subagent.unsupported' in front and 'sound.subagent.off' in front and 'sound.subagent.hookMissing' in front
    # the capability comes from the backend report, not from a hard-coded provider list in the UI
    assert "id === 'antigravity'" not in front.split('function subagentAvailability')[1].split('async function previewSound')[0]
    rust = read('src-tauri/src/integrations.rs')
    assert 'return "unsupported"' in rust and 'hook_missing' in rust and '"ready"' in rust


def test_ui_has_per_provider_controls():
    front = read('src/main.ts')
    for command in ('set_provider_enabled', 'set_provider_volume', 'choose_sound', 'reset_sound', 'preview_sound',
                    'set_subagent_notifications'):
        assert f"invoke('{command}'" in front or f"invoke<boolean>('{command}'" in front or f"invoke<ProviderReport[]>('{command}'" in front, command
    assert "extensions: ['wav', 'mp3', 'ogg']" in front
    assert "type: 'range'" in front


def test_new_ui_strings_exist_in_both_languages():
    en = read('src/locales/en.ts')
    tr = read('src/locales/tr.ts')
    keys = ['sound.title', 'sound.volume', 'sound.main', 'sound.subagent', 'sound.preview', 'sound.missing',
            'sound.fallbackPlayed', 'sound.providerEnabled', 'sound.subagent.unsupported', 'sound.subagent.off',
            'sound.subagent.notConnected', 'sound.subagent.hookMissing', 'sound.subagent.waiting',
            'sound.subagent.waitingCodex', 'sound.subagent.verified', 'settings.subagents', 'settings.subagentsDesc',
            'status.lastSub', 'toast.subagentsOn', 'toast.subagentsOff', 'error.E_SOUND_DECODE',
            'error.E_SOUND_TOO_LONG', 'error.E_INVALID_PROVIDER', 'error.E_INVALID_SOUND_KIND']
    for key in keys:
        assert f"'{key}':" in en and f"'{key}':" in tr, key


def test_all_backend_error_codes_have_translations():
    en = read('src/locales/en.ts')
    codes = set()
    for rel in ('lib.rs', 'audio.rs', 'sound_config.rs', 'integrations.rs', 'updater.rs'):
        codes |= set(re.findall(r'"(E_[A-Z_]+)', read(f'src-tauri/src/{rel}')))
    missing = sorted(c for c in codes if f"'error.{c}'" not in en)
    assert not missing, missing


# ---- hook scripts: privacy and injection --------------------------------------------------------
def test_bridge_forwards_only_a_whitelist():
    bridge = read('scripts/send-event.ps1')
    for private in ('last_assistant_message', 'lastUserInput', 'cwd', 'transcript_path', 'agent_transcript_path',
                    'workspacePaths', 'input-messages'):
        assert private not in bridge.replace('transcriptPath', ''), private
    assert "foreach ($key in @('note', 'product', 'hook', 'session', 'turn', 'agent', 'agent_type'))" in bridge
    assert 'Substring(0, 100)' in bridge, 'values are capped'


def test_scripts_never_evaluate_strings():
    for name in ('install-hooks.ps1', 'uninstall-hooks.ps1', 'send-event.ps1'):
        text = read(f'scripts/{name}')
        for banned in ('Invoke-Expression', 'iex ', 'Start-Process', 'cmd /c', '[scriptblock]::Create'):
            assert banned not in text, (name, banned)


def test_powershell_arguments_are_fixed_literals():
    rust = read('src-tauri/src/integrations.rs')
    block = rust[rust.index('impl SubagentMode'):rust.index('/// Raw copy of a provider')]
    assert 'vec!["-Subagent", "on"' in block and 'vec!["-Subagent", "off"' in block
    for user_data in ('format!', 'provider_str', 'path'):
        assert user_data not in block, user_data


# ---- assets -------------------------------------------------------------------------------------
def test_default_sounds_are_valid_original_assets():
    for name, low, high in (('default.wav', 0.5, 1.0), ('subagent.wav', 0.2, 0.6)):
        with wave.open(str(ROOT / 'src-tauri/resources' / name)) as sound:
            assert sound.getnchannels() == 1 and sound.getframerate() == 24000
            assert low < sound.getnframes() / sound.getframerate() < high, name
    assert 'subagent.wav' in read('scripts/make-default-sound.py')
    assert 'subagent.wav' in read('THIRD_PARTY_NOTICES.md')
    for fixture in ('tone.wav', 'tone.mp3', 'tone.ogg'):
        assert (ROOT / 'src-tauri/test-fixtures' / fixture).stat().st_size < 30000
    assert 'test-fixtures' in read('THIRD_PARTY_NOTICES.md')


def test_bundle_does_not_ship_test_fixtures_or_user_sounds():
    conf = json.loads(read('src-tauri/tauri.conf.json'))
    resources = json.dumps(conf['bundle'].get('resources', ''))
    assert 'test-fixtures' not in resources


def test_new_turkish_strings_are_really_translated():
    def entries(rel):
        return dict(re.findall(r"^\s*'([^']+)':\s*'((?:[^'\\]|\\.)*)'", read(rel), re.M))
    en, tr = entries('src/locales/en.ts'), entries('src/locales/tr.ts')
    keep_same = {'sound.subagent.unsupported'}  # contains only the provider placeholder plus words that differ anyway
    checked = [k for k in en if k.startswith(('sound.', 'settings.subagents', 'toast.subagents', 'status.lastSub',
                                              'error.E_SOUND_DECODE', 'error.E_SOUND_TOO_LONG', 'error.E_INVALID_'))]
    assert len(checked) >= 25
    same = [k for k in checked if en[k] == tr[k] and k not in keep_same]
    assert not same, same
    # placeholders survive translation
    for key in checked:
        assert sorted(re.findall(r'\{\w+\}', en[key])) == sorted(re.findall(r'\{\w+\}', tr[key])), key


# ---- review fixes: provider-scoped gate, atomic hook sync ---------------------------------------
def test_gate_priority_state_is_per_provider():
    gate = read('src-tauri/src/gate.rs').split('#[cfg(test)]')[0]
    assert 'lanes: HashMap<Provider, Lane>' in gate
    for shared in ('    last_main:', '    main_until:', '    sub_until:'):
        # the fields live only inside Lane, never directly on Gate
        assert gate.split('pub struct Gate')[1].split('}')[0].count(shared) == 0, shared
    lib = read('src-tauri/src/lib.rs')
    for call in ('main_started(event.provider', 'subagent_started(event.provider', 'subagent_still_wanted(event.provider'):
        assert call in lib, call


def test_subagent_hook_sync_rolls_back_and_reports_truthfully():
    rust = read('src-tauri/src/integrations.rs').split('#[cfg(test)]')[0]
    assert 'fn sync_with' in rust and 'ops.restore(' in rust and 'E_SUBAGENT_ROLLBACK' in rust
    lib = read('src-tauri/src/lib.rs')
    assert 'E_HOOKS_INCONSISTENT' in lib, 'setting follows the real hooks when rollback failed'
