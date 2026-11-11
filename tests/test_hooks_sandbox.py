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


def test_antigravity_config_uses_forward_slash_path(profile):
    home, local = profile
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    cfg = load(home, '.gemini', 'config', 'hooks.json')
    group = cfg['kaiwen-notify']
    assert group['enabled'] is True and set(group) == {'enabled', 'Stop'}
    handler = group['Stop'][0]
    assert handler['type'] == 'command' and handler['timeout'] == 10
    assert '\\' not in handler['command'], 'no backslashes: robust to POSIX-style command splitting'
    assert handler['command'].endswith('-Source antigravity')
    assert cfg['someone-else'] == FOREIGN_ANTI['someone-else']


def test_antigravity_legacy_command_is_migrated_and_removed(profile):
    home, local = profile
    script = str(local / 'KaiwenNotify' / 'hooks' / 'send-event.ps1')
    legacy = ('powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "'
              + script + '" -Source antigravity')
    cfg_path = home / '.gemini' / 'config' / 'hooks.json'
    cfg = json.loads(cfg_path.read_text('utf8'))
    cfg['kaiwen-notify'] = {'enabled': True, 'Stop': [{'type': 'command', 'command': legacy, 'timeout': 10}]}
    cfg_path.write_text(json.dumps(cfg), 'utf8')
    run('install-hooks.ps1', home, local, '-Provider', 'antigravity')
    assert '\\' not in anti_cmd(home, local)
    run('uninstall-hooks.ps1', home, local, '-Provider', 'antigravity')
    after = json.loads(cfg_path.read_text('utf8'))
    assert 'kaiwen-notify' not in after and 'someone-else' in after
    # and a legacy entry alone is also removable
    cfg['kaiwen-notify'] = {'enabled': True, 'Stop': [{'type': 'command', 'command': legacy, 'timeout': 10}]}
    cfg_path.write_text(json.dumps(cfg), 'utf8')
    run('uninstall-hooks.ps1', home, local, '-Provider', 'antigravity')
    assert 'kaiwen-notify' not in json.loads(cfg_path.read_text('utf8'))


def invoke_bridge(source, payload, port_holder):
    """Runs the bridge with a stdin payload; returns (UDP datagrams received, stdout)."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind(('127.0.0.1', 47451))
    except OSError:
        pytest.skip('UDP port 47451 is in use (Kaiwen Notify running?)')
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
            input=payload, capture_output=True, text=True, encoding='utf8', errors='replace', timeout=60)
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
