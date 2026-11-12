"""Zero-dependency contract validation for the source archive (not a Windows build)."""
import json
import re
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_assets():
    for path in ROOT.rglob('*.json'):
        if 'node_modules' not in path.parts:
            json.loads(path.read_text(encoding='utf8'))
    with wave.open(str(ROOT / 'src-tauri/resources/default.wav')) as sound:
        assert sound.getnchannels() == 1
        assert sound.getframerate() == 24000
        assert sound.getnframes() > 1000
    for name in ('32x32.png', '128x128.png', '128x128@2x.png', 'icon.ico'):
        assert (ROOT / 'src-tauri/icons' / name).is_file()


def test_ipc_contract():
    frontend = (ROOT / 'src/main.ts').read_text()
    rust = (ROOT / 'src-tauri/src/lib.rs').read_text()
    commands = set(re.findall(r"invoke(?:<[^>]+>)?\('([a-z_]+)'", frontend))
    registered = re.search(r'generate_handler!\[([^]]+)\]', rust).group(1)
    for command in commands:
        assert re.search(r'\b' + re.escape(command) + r'\b', registered), command


def test_sources():
    bridge = (ROOT / 'scripts/send-event.ps1').read_text()
    installer = (ROOT / 'scripts/install-hooks.ps1').read_text()
    for provider in ('claude', 'codex', 'antigravity'):
        assert provider in bridge
        assert f'-Source {provider}' in installer or provider == 'codex'
    assert 'fullyIdle' in bridge
    assert 'stop_hook_active' in bridge
    assert '127.0.0.1' in bridge
    assert '127.0.0.1' in (ROOT / 'src-tauri/src/lib.rs').read_text()
    assert 'Backup-Existing' in installer


def test_rollback_contract():
    undo = (ROOT / 'scripts/uninstall-hooks.ps1').read_text()
    assert 'param([switch]$DryRun' in undo
    assert 'Backup-Existing' in undo
    assert "$handler.command -ceq $claudeExpected" in undo
    assert "Get-AntigravityHandlerKind" in undo
    assert '$handler.command -ceq $codexExpected' in undo
    assert 'Commit-Json $codexHooksPath' in undo
    # Older v0.2 installs can be cleaned up without erasing any custom/native notify.
    assert "[regex]::Escape($expectedLine)" in undo
    assert "Remove('kaiwen-notify')" in undo
    assert "Write-Utf8 $codexPath $updated" in undo


def test_onboarding_contract():
    html = (ROOT / 'index.html').read_text()
    frontend = (ROOT / 'src/main.ts').read_text()
    rust = (ROOT / 'src-tauri/src/lib.rs').read_text()
    integ = (ROOT / 'src-tauri/src/integrations.rs').read_text()
    for element_id in ('setup-view', 'dashboard-view', 'install-integrations',
                       'uninstall-integrations', 'reinstall-integrations', 'confirm-dialog',
                       'update-action', 'update-progress'):
        assert html.count(f'id="{element_id}"') == 1, element_id
    for command in ('get_integrations', 'install_integrations', 'uninstall_integrations'):
        assert f"'{command}'" in frontend, command
        assert f'fn {command}(' in rust, command
    assert "listen<Progress>('integration-progress'" in frontend
    assert 'spawn_blocking' in rust
    assert 'creation_flags(CREATE_NO_WINDOW)' in integ
    for filename in ('install-hooks.ps1', 'uninstall-hooks.ps1', 'send-event.ps1'):
        assert f'include_str!("../../scripts/{filename}")' in integ
    # The Codex Desktop computer-use `notify` setting must never be read or written by the app.
    assert r"'.codex\hooks.json'" in (ROOT / 'scripts/install-hooks.ps1').read_text()
    assert 'config.toml' not in rust and 'config.toml' not in integ
    assert 'native notify korunuyor' in (ROOT / 'scripts/install-hooks.ps1').read_text()


def test_updater_contract():
    frontend = (ROOT / 'src/main.ts').read_text()
    rust = (ROOT / 'src-tauri/src/lib.rs').read_text()
    upd = (ROOT / 'src-tauri/src/updater.rs').read_text()
    conf = json.loads((ROOT / 'src-tauri/tauri.conf.json').read_text(encoding='utf8'))
    # No endpoint/pubkey is invented in the committed config; releases inject them.
    assert 'updater' not in conf.get('plugins', {})
    assert 'createUpdaterArtifacts' not in conf['bundle']
    assert 'download(' in upd and 'install(' in upd
    assert "listen<DownloadProgress>('update-progress'" in frontend
    assert 'dangerous' not in upd.lower() and 'dangerous' not in rust.lower()
    # Install only on explicit user action: no install call outside install_update.
    assert rust.count('updater::install(') == 1
    assert 'download_and_install' not in upd + rust
    caps = json.loads((ROOT / 'src-tauri/capabilities/default.json').read_text())
    assert 'core:default' not in caps['permissions']


def test_versions_match():
    pkg = json.loads((ROOT / 'package.json').read_text())['version']
    conf = json.loads((ROOT / 'src-tauri/tauri.conf.json').read_text(encoding='utf8'))['version']
    cargo = re.search(r'^version = "([^"]+)"', (ROOT / 'src-tauri/Cargo.toml').read_text(), re.M).group(1)
    assert pkg == conf == cargo


def test_icons_transparent():
    from PIL import Image
    for name in ('32x32.png', '64x64.png', '128x128.png', '128x128@2x.png', 'icon.png', 'tray.png'):
        im = Image.open(ROOT / 'src-tauri/icons' / name).convert('RGBA')
        assert im.getpixel((0, 0))[3] == 0, name
    ico = Image.open(ROOT / 'src-tauri/icons/icon.ico')
    assert {(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)} <= set(ico.info['sizes'])


def test_readable_docs():
    readme = (ROOT / 'README.md').read_text(encoding='utf8')
    for term in ('Entegrasyonları Kur', 'Entegrasyonları Kaldır', 'Eksikleri kur / Onar', 'latest.json'):
        assert term in readme


if __name__ == '__main__':
    test_assets()
    test_ipc_contract()
    test_sources()
    test_rollback_contract()
    test_onboarding_contract()
    test_updater_contract()
    test_versions_match()
    test_icons_transparent()
    test_readable_docs()
    print('9 offline contract checks passed.')
