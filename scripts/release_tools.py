"""Release helpers used by .github/workflows/release.yml (also runnable locally).

  python scripts/release_tools.py check-versions <version>   all version fields agree and CHANGELOG has the section
  python scripts/release_tools.py notes <version>            GitHub release body (English)
  python scripts/release_tools.py verify-assets <tag> [repo] draft/published release has installer, .sig, latest.json and they agree
  python scripts/release_tools.py verify-endpoint <repo>     the public updater endpoint serves a valid latest.json
"""
import json
import re
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO_DEFAULT = 'ImtheKaiwen/kaiwen-notify'


def read(rel):
    return (ROOT / rel).read_text(encoding='utf8')


def versions():
    return {
        'package.json': json.loads(read('package.json'))['version'],
        'tauri.conf.json': json.loads(read('src-tauri/tauri.conf.json'))['version'],
        'Cargo.toml': re.search(r'^version = "([^"]+)"', read('src-tauri/Cargo.toml'), re.M).group(1),
    }


def changelog_section(version):
    text = read('CHANGELOG.md')
    match = re.search(rf'^## {re.escape(version)}\s*\n(.*?)(?=^## |\Z)', text, re.M | re.S)
    return match.group(1).strip() if match else None


def check_versions(version):
    found = versions()
    bad = {k: v for k, v in found.items() if v != version}
    if bad:
        sys.exit(f'version mismatch (expected {version}): {bad}')
    if not changelog_section(version):
        sys.exit(f'CHANGELOG.md has no "## {version}" section')
    print(f'versions ok: {version} in {", ".join(found)} and CHANGELOG.md')


def notes(version):
    section = changelog_section(version) or ''
    print(f'''## Kaiwen Notify {version}

Lightweight Windows utility that plays a sound when Claude Code, Codex and Antigravity finish a task.

### Install

1. Download the **`*_x64-setup.exe`** installer below and run it (Windows 10/11 x64, per-user install, no administrator rights).
   Windows SmartScreen may warn because the installer is not Authenticode-signed; choose "More info", then "Run anyway".
2. Open Kaiwen Notify and press **Install integrations**. Existing hooks and your Codex `notify` setting are preserved.
3. Pick your own notification sound under Settings if you like.

Updates are checked every 10 minutes while the app runs and are never installed without your confirmation; they are
verified against a signing key built into the app. Versions earlier than this one have no updater and need a manual install.

### What's new

{section}

### Assets

- `*_x64-setup.exe`: NSIS installer
- `.sig`: updater signature for the installer
- `latest.json`: update manifest read by the app

Source code is MIT licensed. Provider logos belong to their owners (see THIRD_PARTY_NOTICES.md).''')


def run(*args):
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout


def validate_manifest(manifest, version, sig_text=None, repo=REPO_DEFAULT, expected_size=None):
    assert manifest.get('version') == version, ('manifest version', manifest.get('version'))
    assert manifest.get('pub_date'), 'missing pub_date'
    platform = manifest['platforms']['windows-x86_64']
    assert platform['url'].startswith(f'https://github.com/{repo}/releases/download/v{version}/'), platform['url']
    assert platform['url'].endswith('_x64-setup.exe') or platform['url'].endswith('x64-setup.exe'), platform['url']
    signature = platform['signature']
    assert signature.strip(), 'empty signature'
    if sig_text is not None:
        assert signature.strip() == sig_text.strip(), 'latest.json signature differs from the .sig asset'
    print('manifest ok:', platform['url'])
    return platform


def verify_assets(tag, repo=REPO_DEFAULT):
    version = tag.lstrip('v')
    info = json.loads(run('gh', 'release', 'view', tag, '--repo', repo, '--json', 'assets,isDraft,tagName'))
    names = {a['name']: a for a in info['assets']}
    setup = [n for n in names if n.endswith('_x64-setup.exe')]
    sigs = [n for n in names if n.endswith('_x64-setup.exe.sig')]
    assert len(setup) == 1, ('installer', list(names))
    assert len(sigs) == 1, ('signature', list(names))
    assert 'latest.json' in names, ('latest.json', list(names))
    assert names[setup[0]]['size'] > 1_000_000, 'installer suspiciously small'
    with tempfile.TemporaryDirectory() as tmp:
        run('gh', 'release', 'download', tag, '--repo', repo, '--dir', tmp, '--pattern', 'latest.json', '--pattern', '*.sig')
        manifest = json.loads((Path(tmp) / 'latest.json').read_text(encoding='utf8'))
        sig_text = (Path(tmp) / sigs[0]).read_text(encoding='utf8')
    platform = validate_manifest(manifest, version, sig_text, repo)
    print(f'assets ok: {setup[0]} ({names[setup[0]]["size"]} bytes), {sigs[0]}, latest.json; draft={info["isDraft"]}')


def verify_endpoint(repo=REPO_DEFAULT):
    url = f'https://github.com/{repo}/releases/latest/download/latest.json'
    with urllib.request.urlopen(url, timeout=30) as response:
        assert response.status == 200, response.status
        manifest = json.loads(response.read().decode('utf8'))
    version = manifest['version']
    platform = validate_manifest(manifest, version, repo=repo)
    request = urllib.request.Request(platform['url'], method='HEAD')
    with urllib.request.urlopen(request, timeout=30) as response:
        size = int(response.headers.get('Content-Length', '0'))
        assert response.status == 200 and size > 1_000_000, (response.status, size)
    print(f'endpoint ok: {url} -> v{version}, installer {size} bytes')


if __name__ == '__main__':
    command, *args = sys.argv[1:] or ['']
    commands = {'check-versions': check_versions, 'notes': notes, 'verify-assets': verify_assets,
                'verify-endpoint': verify_endpoint}
    if command not in commands:
        sys.exit(__doc__)
    commands[command](*args)
