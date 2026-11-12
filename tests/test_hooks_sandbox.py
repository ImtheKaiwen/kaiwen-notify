"""Runs the real install/uninstall PowerShell scripts against a throw-away profile.

Windows only. Verifies: foreign hooks survive, Codex config.toml (computer-use `notify`) is
byte-identical, operations are idempotent, -Provider scopes the change, backups are made.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / 'scripts'
pytestmark = pytest.mark.skipif(sys.platform != 'win32', reason='PowerShell hook scripts are Windows-only')

TOML = 'model = "gpt-5"\nnotify = ["C:/x/codex-computer-use.exe", "turn-ended"]\n[mcp_servers.foo]\ncommand = "foo"\n'
FOREIGN_CLAUDE = {'theme': 'dark', 'hooks': {'Stop': [{'hooks': [{'type': 'command', 'command': 'echo mine'}]}]}}
FOREIGN_CODEX = {'hooks': {'Stop': [{'hooks': [{'type': 'command', 'command': 'echo codex-mine'}]}]}}
FOREIGN_ANTI = {'someone-else': {'enabled': True, 'Stop': [{'type': 'command', 'command': 'echo anti'}]}}


PROFILE_NAMES = ['plain', 'Çağrı Ünal şğı (test)']  # second: Turkish letters, spaces, parentheses


@pytest.fixture(params=PROFILE_NAMES, ids=['ascii', 'turkish-with-spaces'])
def profile(request):
    base = Path(tempfile.mkdtemp(prefix='kn-test-')) / request.param
    home, local = base / 'home dir', base / 'local app'
    (home / '.claude').mkdir(parents=True)
    (home / '.codex').mkdir()
    (home / '.gemini' / 'config').mkdir(parents=True)
    local.mkdir()
    (home / '.claude' / 'settings.json').write_text(json.dumps(FOREIGN_CLAUDE), 'utf8')
    (home / '.codex' / 'hooks.json').write_text(json.dumps(FOREIGN_CODEX), 'utf8')
    (home / '.gemini' / 'config' / 'hooks.json').write_text(json.dumps(FOREIGN_ANTI), 'utf8')
    (home / '.codex' / 'config.toml').write_bytes(TOML.encode('utf8'))
    yield home, local
    shutil.rmtree(base.parent, ignore_errors=True)


def run(script, home, local, *args):
    env = dict(os.environ, USERPROFILE=str(home), LOCALAPPDATA=str(local))
    result = subprocess.run(
        ['powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File',
         str(SCRIPTS / script), *args],
        env=env, capture_output=True, text=True, encoding='utf8', errors='replace', timeout=120)
    assert result.returncode == 0, result.stderr
    return result


def load(home, *parts):
    return json.loads((home.joinpath(*parts)).read_text('utf8'))


def count_ours(text):
    return text.count('KaiwenNotify')


def test_install_preserves_everything_and_is_idempotent(profile):
    home, local = profile
    run('install-hooks.ps1', home, local)
    claude = (home / '.claude' / 'settings.json').read_text('utf8')
    assert 'echo mine' in claude and count_ours(claude) == 1
    assert load(home, '.claude', 'settings.json')['theme'] == 'dark'
    assert 'echo codex-mine' in (home / '.codex' / 'hooks.json').read_text('utf8')
    assert 'kaiwen-notify' in load(home, '.gemini', 'config', 'hooks.json')
    assert 'someone-else' in load(home, '.gemini', 'config', 'hooks.json')
    assert (home / '.codex' / 'config.toml').read_bytes() == TOML.encode('utf8')
    before = {p: p.read_bytes() for p in home.rglob('*.json')}
    run('install-hooks.ps1', home, local)  # second run changes nothing
    assert before == {p: p.read_bytes() for p in home.rglob('*.json')}
    assert list(home.rglob('*.kaiwen-backup-*')), 'backups must be created before edits'
    assert not list(home.rglob('*.kaiwen-tmp-*')), 'no temp files left behind'


def test_uninstall_removes_only_ours_and_keeps_notify(profile):
    home, local = profile
    originals = {p: json.loads(p.read_text('utf8')) for p in home.rglob('*.json')}
    run('install-hooks.ps1', home, local)
    run('uninstall-hooks.ps1', home, local)
    for path, original in originals.items():
        assert json.loads(path.read_text('utf8')) == original, path
    assert (home / '.codex' / 'config.toml').read_bytes() == TOML.encode('utf8')
    run('uninstall-hooks.ps1', home, local)  # idempotent
    for path, original in originals.items():
        assert json.loads(path.read_text('utf8')) == original, path


def test_provider_switch_scopes_changes(profile):
    home, local = profile
    run('install-hooks.ps1', home, local, '-Provider', 'claude')
    assert count_ours((home / '.claude' / 'settings.json').read_text('utf8')) == 1
    assert count_ours((home / '.codex' / 'hooks.json').read_text('utf8')) == 0
    assert 'kaiwen-notify' not in load(home, '.gemini', 'config', 'hooks.json')
    run('install-hooks.ps1', home, local)
    run('uninstall-hooks.ps1', home, local, '-Provider', 'codex')
    assert count_ours((home / '.codex' / 'hooks.json').read_text('utf8')) == 0
    assert count_ours((home / '.claude' / 'settings.json').read_text('utf8')) == 1


def test_corrupt_config_is_not_overwritten(profile):
    home, local = profile
    bad = home / '.claude' / 'settings.json'
    bad.write_text('{ not json', 'utf8')
    result = subprocess.run(
        ['powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File',
         str(SCRIPTS / 'install-hooks.ps1'), '-Provider', 'claude'],
        env=dict(os.environ, USERPROFILE=str(home), LOCALAPPDATA=str(local)),
        capture_output=True, text=True, encoding='utf8', errors='replace', timeout=120)
    assert result.returncode != 0
    assert bad.read_text('utf8') == '{ not json'


# ---- Antigravity: config (A) and hook invocation (B) ------------------------------------------
import socket
import threading

BRIDGE = SCRIPTS / 'send-event.ps1'


def anti_cmd(home, local):
    cfg = load(home, '.gemini', 'config', 'hooks.json')
    return cfg['kaiwen-notify']['Stop'][0]['command']


def invoke_bridge(source, payload, port_holder):
    """Runs the bridge with a stdin payload; returns (UDP datagrams received, stdout).
    Uses a free private port (KAIWEN_NOTIFY_PORT) so it works beside a running Kaiwen Notify."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
    sock.settimeout(1.5)
    got = []

    def listen():
        try:
            while True:
                got.append(sock.recvfrom(1024)[0])
        except OSError:
            pass
    t = threading.Thread(target=listen, daemon=True)
    t.start()
    try:
        result = subprocess.run(
            ['powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File',
             str(BRIDGE), '-Source', source],
            input=payload, capture_output=True, text=True, encoding='utf8', errors='replace', timeout=60,
            env=dict(os.environ, KAIWEN_NOTIFY_PORT=str(port)))
        t.join(timeout=3)
    finally:
        sock.close()
    return got, result


STOP_BASE = {'conversationId': '123e4567-e89b-12d3-a456-426614174000', 'executionNum': 1,
             'workspacePaths': ['C:/proj'], 'modelName': 'gemini', 'lastUserInput': 'hi'}


@pytest.mark.parametrize('extra,expect_event', [
    ({'terminationReason': 'model_stop', 'fullyIdle': True}, True),
    ({'terminationReason': 'some_new_success_reason', 'fullyIdle': True}, True),
    ({'terminationReason': 'model_stop', 'fullyIdle': False}, False),
    ({'terminationReason': 'error', 'error': 'boom', 'fullyIdle': True}, False),
    ({'terminationReason': 'max_steps_exceeded', 'fullyIdle': True}, False),
])
def test_antigravity_stop_payloads(extra, expect_event):
    got, result = invoke_bridge('antigravity', json.dumps({**STOP_BASE, **extra}), None)
    assert result.returncode == 0
    assert json.loads(result.stdout.strip()) == {'decision': 'stop'}, 'must never block the agent'
    assert (got == [b'{"source":"antigravity","kind":"completed"}']) == expect_event, got


def test_antigravity_garbage_or_empty_stdin_still_returns_valid_json():
    for payload in ('not json at all', ''):
        got, result = invoke_bridge('antigravity', payload, None)
        assert result.returncode == 0
        assert json.loads(result.stdout.strip()) == {'decision': 'stop'}


def test_installed_hook_commands_run_from_unusual_paths(profile):
    """The exact command stored in each config must execute when the path has spaces/Turkish letters."""
    home, local = profile
    run('install-hooks.ps1', home, local)
    cfg_cmds = {
        'claude': load(home, '.claude', 'settings.json')['hooks']['Stop'][-1]['hooks'][0]['command'],
        'codex': load(home, '.codex', 'hooks.json')['hooks']['Stop'][-1]['hooks'][0]['command'],
        'antigravity': anti_cmd(home, local),
    }
    for source, cmd in cfg_cmds.items():
        payload = json.dumps({'terminationReason': 'model_stop', 'fullyIdle': True})
        env = dict(os.environ, USERPROFILE=str(home), LOCALAPPDATA=str(local))
        result = subprocess.run(cmd, shell=True, input=payload, capture_output=True, text=True, encoding='utf8', errors='replace',
                                timeout=60, env=env)
        assert result.returncode == 0, (source, result.stderr)
        expected = {'claude': '', 'codex': '{}', 'antigravity': '{"decision":"stop"}'}[source]
        assert result.stdout.strip() == expected, (source, result.stdout)


def test_only_detected_provider_missing_tools_do_not_break_install(profile):
    """A machine with just one tool installed: scripts for the others must not fail or create junk."""
    home, local = profile
    shutil.rmtree(home / '.codex')
    shutil.rmtree(home / '.gemini')
    run('install-hooks.ps1', home, local, '-Provider', 'claude')
    assert count_ours((home / '.claude' / 'settings.json').read_text('utf8')) == 1
    assert not (home / '.codex').exists() and not (home / '.gemini').exists()
    run('uninstall-hooks.ps1', home, local)  # nothing to remove elsewhere: still succeeds
    assert count_ours((home / '.claude' / 'settings.json').read_text('utf8')) == 0


# ---- Antigravity App / IDE / CLI are told apart by their data folder ---------------------------
@pytest.mark.parametrize('folder,product', [
    ('antigravity', 'app'), ('antigravity-ide', 'ide'), ('antigravity-cli', 'cli')])
def test_antigravity_product_detected_from_transcript_path(folder, product):
    payload = {**STOP_BASE, 'terminationReason': 'model_stop', 'fullyIdle': True,
               'transcriptPath': f'C:/Users/Someone Else/.gemini/{folder}/brain/abc/.system_generated/logs/transcript.jsonl'}
    got, result = invoke_bridge('antigravity', json.dumps(payload), None)
    assert json.loads(result.stdout.strip()) == {'decision': 'stop'}
    assert got == [('{"source":"antigravity","kind":"completed","product":"%s"}' % product).encode()], got
    assert b'Someone' not in b''.join(got), 'no paths or user names leave the hook'


def test_antigravity_not_idle_reports_a_silent_seen_event():
    payload = {**STOP_BASE, 'terminationReason': 'model_stop', 'fullyIdle': False,
               'transcriptPath': 'C:/Users/x/.gemini/antigravity/brain/a/.system_generated/logs/transcript.jsonl'}
    got, result = invoke_bridge('antigravity', json.dumps(payload), None)
    assert got == [b'{"source":"antigravity","kind":"seen","note":"not_idle","product":"app"}'], got
    assert json.loads(result.stdout.strip()) == {'decision': 'stop'}


# ---- Antigravity hook command contract (v0.5.1): one portable -EncodedCommand ----------------------
import base64
import re

ANTI_SCRIPT_TEXT = ("$ProgressPreference = 'SilentlyContinue'; $d = $env:LOCALAPPDATA; if (-not $d) { $d = "
                    "[Environment]::GetFolderPath('LocalApplicationData') }; & (Join-Path $d "
                    "'KaiwenNotify\\hooks\\send-event.ps1') -Source antigravity")
ANTI_COMMAND = ('powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -OutputFormat Text '
                '-EncodedCommand ' + base64.b64encode(ANTI_SCRIPT_TEXT.encode('utf-16-le')).decode())
PS_PREFIX = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass '
CFG = ('.gemini', 'config', 'hooks.json')


def legacy_forms(local):
    script = str(local / 'KaiwenNotify' / 'hooks' / 'send-event.ps1')
    fwd = script.replace('\\', '/')
    return {
        'v0.4 quoted backslashes': PS_PREFIX + f'-File "{script}" -Source antigravity',
        'v0.5.0 quoted forward slashes': PS_PREFIX + f'-File "{fwd}" -Source antigravity',
        'hand-fixed unquoted': PS_PREFIX + f'-File {fwd} -Source antigravity',
    }


def write_cfg(home, groups):
    path = home.joinpath(*CFG)
    cfg = json.loads(path.read_text('utf8'))
    cfg.update(groups)
    path.write_text(json.dumps(cfg), 'utf8')
    return path


def our_handlers(home):
    return load(home, *CFG).get('kaiwen-notify', {}).get('Stop', [])


def test_sources_agree_on_the_encoded_command():
    """Python, the PowerShell scripts and the Rust constant must describe exactly one command."""
    for script in ('install-hooks.ps1', 'uninstall-hooks.ps1'):
        text = (SCRIPTS / script).read_text('utf8')
        here = re.search(r"\$AntigravityScriptText = @'\r?\n(.*?)\r?\n'@", text, re.S).group(1)
        assert here == ANTI_SCRIPT_TEXT, script
    rust = (ROOT / 'src-tauri/src/integrations.rs').read_text('utf8')
    expected = base64.b64encode(ANTI_SCRIPT_TEXT.encode('utf-16-le')).decode()
    assert f'const EXPECTED_B64: &str = "{expected}";' in rust
    assert not re.search(r'[\w]+@[\w.]+|C:[\\/]Users', ANTI_COMMAND)
    assert '"' not in ANTI_COMMAND and ' ' not in ANTI_COMMAND.split('-EncodedCommand ')[1]


def test_fresh_install_writes_the_portable_command(profile):
    home, local = profile
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    cfg = load(home, *CFG)
    group = cfg['kaiwen-notify']
    assert group['enabled'] is True and set(group) == {'enabled', 'Stop'}
    assert group['Stop'] == [{'type': 'command', 'command': ANTI_COMMAND, 'timeout': 10}]
    assert cfg['someone-else'] == FOREIGN_ANTI['someone-else']
    # no user name, drive letter or profile path leaks into the command
    assert str(local) not in ANTI_COMMAND and str(home) not in ANTI_COMMAND


def test_install_is_idempotent_for_antigravity(profile):
    home, local = profile
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    first = home.joinpath(*CFG).read_bytes()
    backups = len(list(home.joinpath('.gemini', 'config').glob('hooks.json.kaiwen-backup-*')))
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    assert home.joinpath(*CFG).read_bytes() == first
    assert len(list(home.joinpath('.gemini', 'config').glob('hooks.json.kaiwen-backup-*'))) == backups


@pytest.mark.parametrize('form', ['v0.4 quoted backslashes', 'v0.5.0 quoted forward slashes'])
def test_broken_legacy_forms_are_upgraded_in_place(profile, form):
    home, local = profile
    legacy = legacy_forms(local)[form]
    write_cfg(home, {'kaiwen-notify': {'enabled': True, 'Stop': [
        {'type': 'command', 'command': 'echo theirs', 'timeout': 5},
        {'type': 'command', 'command': legacy, 'timeout': 10}]}})
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    handlers = our_handlers(home)
    assert [h['command'] for h in handlers] == ['echo theirs', ANTI_COMMAND]
    assert list(home.joinpath('.gemini', 'config').glob('hooks.json.kaiwen-backup-*')), 'backup before edit'
    assert load(home, *CFG)['someone-else'] == FOREIGN_ANTI['someone-else']


def test_working_hand_fixed_command_is_left_untouched(profile):
    home, local = profile
    plain = legacy_forms(local)['hand-fixed unquoted']
    if re.search(r'[^\x21-\x7E ]|\s', plain.split('-File ')[1].split(' -Source')[0]):
        pytest.skip('sandbox path has spaces/non-ASCII, so the unquoted form is not a working one here')
    write_cfg(home, {'kaiwen-notify': {'enabled': True, 'Stop': [{'type': 'command', 'command': plain, 'timeout': 10}]}})
    before = home.joinpath(*CFG).read_bytes()
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    assert home.joinpath(*CFG).read_bytes() == before, 'a working user fix must not be reverted'


def test_unquoted_form_with_unusual_path_counts_as_broken(profile):
    home, local = profile
    legacy = PS_PREFIX + '-File C:/Users/John Smith/AppData/Local/KaiwenNotify/hooks/send-event.ps1 -Source antigravity'
    # such a command splits at the space, so it must be treated as legacy and replaced
    write_cfg(home, {'kaiwen-notify': {'enabled': True, 'Stop': [{'type': 'command', 'command': legacy, 'timeout': 10}]}})
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    assert [h['command'] for h in our_handlers(home)] == [ANTI_COMMAND]


def test_duplicates_collapse_to_one_working_handler(profile):
    home, local = profile
    forms = legacy_forms(local)
    write_cfg(home, {'kaiwen-notify': {'enabled': True, 'Stop': [
        {'type': 'command', 'command': forms['v0.4 quoted backslashes'], 'timeout': 10},
        {'type': 'command', 'command': ANTI_COMMAND, 'timeout': 10},
        {'type': 'command', 'command': 'echo keep me', 'timeout': 3}]}})
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    commands = [h['command'] for h in our_handlers(home)]
    assert commands.count(ANTI_COMMAND) == 1 and 'echo keep me' in commands and len(commands) == 2


@pytest.mark.parametrize('form', ['v0.4 quoted backslashes', 'v0.5.0 quoted forward slashes', 'hand-fixed unquoted', 'current'])
def test_uninstall_removes_every_known_form_and_only_ours(profile, form):
    home, local = profile
    command = ANTI_COMMAND if form == 'current' else legacy_forms(local)[form]
    probe = {'kaiwen-temp-probe': {'enabled': True, 'PreInvocation': [{'type': 'command', 'command': 'echo probe'}]}}
    write_cfg(home, {'kaiwen-notify': {'enabled': True, 'Stop': [{'type': 'command', 'command': command, 'timeout': 10}]}, **probe})
    run('uninstall-hooks.ps1', home, local, '-Provider', 'antigravity')
    after = load(home, *CFG)
    assert 'kaiwen-notify' not in after
    assert after['someone-else'] == FOREIGN_ANTI['someone-else'] and after['kaiwen-temp-probe'] == probe['kaiwen-temp-probe']
    run('uninstall-hooks.ps1', home, local, '-Provider', 'antigravity')  # idempotent
    assert load(home, *CFG) == after


def test_claude_and_codex_commands_are_unchanged_by_the_antigravity_work(profile):
    home, local = profile
    run('install-hooks.ps1', home, local)
    script = str(local / 'KaiwenNotify' / 'hooks' / 'send-event.ps1')
    claude = load(home, '.claude', 'settings.json')['hooks']['Stop'][-1]['hooks'][0]['command']
    codex = load(home, '.codex', 'hooks.json')['hooks']['Stop'][-1]['hooks'][0]['command']
    assert claude == PS_PREFIX + f'-File "{script}" -Source claude'
    assert codex == PS_PREFIX + f'-File "{script}" -Source codex'
    assert (home / '.codex' / 'config.toml').read_bytes() == TOML.encode('utf8')


def _run_layers(command, payload, env):
    return {
        'shell=True (cmd /c)': subprocess.run(command, shell=True, input=payload, capture_output=True, text=True, encoding='utf8', errors='replace', timeout=60, env=env),
        'cmd.exe /c <command>': subprocess.run(['cmd.exe', '/c', command], input=payload, capture_output=True, text=True, encoding='utf8', errors='replace', timeout=60, env=env),
        'powershell -Command <command>': subprocess.run(['powershell.exe', '-NoProfile', '-Command', command], input=payload, capture_output=True, text=True, encoding='utf8', errors='replace', timeout=60, env=env),
    }


def test_stored_command_runs_from_spaced_unicode_profile_under_every_launcher(profile):
    """The exact command from hooks.json, fed a Stop payload on stdin, must work however it is launched."""
    home, local = profile
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    command = our_handlers(home)[0]['command']
    (local / 'KaiwenNotify' / 'diag.enabled').write_text('')
    payload = json.dumps({'terminationReason': 'NO_TOOL_CALL', 'fullyIdle': True, 'executionNum': 0,
                          'transcriptPath': 'C:/Users/x/.gemini/antigravity/brain/a/.system_generated/logs/transcript.jsonl'})
    env = dict(os.environ, USERPROFILE=str(home), LOCALAPPDATA=str(local))
    for name, result in _run_layers(command, payload, env).items():
        assert result.returncode == 0, (name, result.stderr)
        assert json.loads(result.stdout.strip()) == {'decision': 'stop'}, (name, result.stdout)
        assert result.stderr.strip() == '', (name, result.stderr)
    log = (local / 'KaiwenNotify' / 'diag.log').read_text('utf8')
    assert log.count('antigravity payload product=app fullyIdle=True reason=NO_TOOL_CALL') == 3, log


def test_bad_or_missing_script_never_breaks_the_agent(profile):
    """If the bridge file is missing, the hook still exits quietly instead of failing the agent turn."""
    home, local = profile
    command = ANTI_COMMAND
    env = dict(os.environ, USERPROFILE=str(home), LOCALAPPDATA=str(local))
    result = subprocess.run(command, shell=True, input='{}', capture_output=True, text=True, encoding='utf8',
                            errors='replace', timeout=60, env=env)
    assert result.returncode in (0, 1)  # reported as a failed hook, never a hung or crashing agent
    assert 'decision' not in result.stdout


def test_diag_log_is_bounded_and_opt_in(profile):
    home, local = profile
    hooks = local / 'KaiwenNotify' / 'hooks'
    hooks.mkdir(parents=True, exist_ok=True)
    shutil.copy(SCRIPTS / 'send-event.ps1', hooks / 'send-event.ps1')
    env = dict(os.environ, USERPROFILE=str(home), LOCALAPPDATA=str(local))
    payload = json.dumps({'terminationReason': 'NO_TOOL_CALL', 'fullyIdle': True})
    run_bridge = lambda: subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', str(hooks / 'send-event.ps1'), '-Source', 'antigravity'], input=payload, capture_output=True, text=True, timeout=60, env=env)
    run_bridge()
    assert not (local / 'KaiwenNotify' / 'diag.log').exists(), 'no log unless diag.enabled exists'
    (local / 'KaiwenNotify' / 'diag.enabled').write_text('')
    (local / 'KaiwenNotify' / 'diag.log').write_text('x' * 300000)
    run_bridge()
    assert (local / 'KaiwenNotify' / 'diag.log.1').exists() and (local / 'KaiwenNotify' / 'diag.log').stat().st_size < 5000


# ---- bridge behaviour for every provider and unusual Antigravity payloads -------------------------
def test_claude_and_codex_events_still_flow():
    got, result = invoke_bridge('claude', json.dumps({'stop_hook_active': False}), None)
    assert got == [b'{"source":"claude","kind":"completed"}'] and result.stdout.strip() == ''
    got, result = invoke_bridge('claude', json.dumps({'stop_hook_active': True}), None)
    assert got == [], 'Claude loop guard: stop_hook_active must stay silent'
    got, result = invoke_bridge('codex', json.dumps({'turn_id': 'x'}), None)
    assert got == [b'{"source":"codex","kind":"completed"}'] and result.stdout.strip() == '{}'


@pytest.mark.parametrize('payload,expected', [
    ({'terminationReason': 'NO_TOOL_CALL', 'fullyIdle': True, 'executionNum': 0}, b'completed'),
    ({'terminationReason': 'NO_TOOL_CALL', 'fullyIdle': False}, b'seen'),
    ({'terminationReason': 'cancelled', 'fullyIdle': True}, b'seen'),
    ({'terminationReason': 'user_cancelled', 'fullyIdle': True}, b'seen'),
    ({'terminationReason': 'max_steps_exceeded', 'fullyIdle': True}, b'seen'),
    ({'terminationReason': 'error', 'error': 'x', 'fullyIdle': True}, b'seen'),
    ({'fullyIdle': True}, b'completed'),                                   # missing reason
    ({}, b'completed'),                                                    # missing everything
    ({'terminationReason': 'BRAND_NEW_REASON', 'fullyIdle': True, 'surprise': {'a': 1}}, b'completed'),
])
def test_antigravity_unusual_payloads(payload, expected):
    got, result = invoke_bridge('antigravity', json.dumps(payload), None)
    assert len(got) == 1 and b'"kind":"' + expected + b'"' in got[0], got
    assert json.loads(result.stdout.strip()) == {'decision': 'stop'}
