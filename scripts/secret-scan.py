"""Fails if tracked files contain obvious secrets. Run from the repo root (CI and locally)."""
import re
import subprocess
import sys

PATTERNS = {
    'private key block': r'-----BEGIN [A-Z ]*PRIVATE KEY-----',
    'minisign/tauri secret key': r'untrusted comment: [^\n]*secret key',
    'GitHub token': r'\b(gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})\b',
    'AWS access key': r'\bAKIA[0-9A-Z]{16}\b',
    'Anthropic/OpenAI style key': r'\bsk-(ant-)?[A-Za-z0-9_-]{32,}\b',
}
SKIP_SUFFIX = ('.png', '.ico', '.wav', '.exe', '.lock', 'package-lock.json')
files = subprocess.run(['git', 'ls-files'], capture_output=True, text=True, check=True).stdout.split('\n')
bad = []
for name in filter(None, files):
    if name.endswith(SKIP_SUFFIX) or name == 'scripts/secret-scan.py':
        continue
    try:
        text = open(name, encoding='utf8', errors='ignore').read()
    except OSError:
        continue
    for label, pattern in PATTERNS.items():
        if re.search(pattern, text):
            bad.append(f'{name}: {label}')
if bad:
    print('Possible secrets found:\n' + '\n'.join(bad))
    sys.exit(1)
print(f'secret scan ok ({len(files)} tracked files)')
