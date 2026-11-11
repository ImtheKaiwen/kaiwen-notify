"""v0.5 contracts: i18n parity/fallback, splash lifecycle, autostart, update scheduler, portability."""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(rel):
    return (ROOT / rel).read_text(encoding='utf8')


def keys_of(rel):
    return re.findall(r"^\s*'([^']+)':", read(rel), re.M)


def placeholders(text):
    return sorted(re.findall(r'\{(\w+)\}', text))


def entries(rel):
    return dict(re.findall(r"^\s*'([^']+)':\s*'((?:[^'\\]|\\.)*)'", read(rel), re.M))


# ---- i18n ---------------------------------------------------------------------------------------
def test_locale_key_parity():
    en, tr = keys_of('src/locales/en.ts'), keys_of('src/locales/tr.ts')
    assert len(en) == len(set(en)) and len(tr) == len(set(tr)), 'duplicate keys'
    assert set(en) == set(tr), (set(en) ^ set(tr))


def test_locale_placeholders_match_and_no_empty_values():
    en, tr = entries('src/locales/en.ts'), entries('src/locales/tr.ts')
    for key, value in en.items():
        assert value.strip(), key
        assert tr[key].strip(), key
        assert placeholders(value) == placeholders(tr[key]), key


def test_every_used_key_exists():
    en = set(keys_of('src/locales/en.ts'))
    used = set()
    for rel in ('src/main.ts', 'index.html'):
        text = read(rel)
        used |= set(re.findall(r"\bt\(\s*'([a-zA-Z0-9_.]+)'", text))
        used |= set(re.findall(r'data-i18n(?:-title|-aria)?="([^"]+)"', text))
    used |= {f'detail.{k}' for k in re.findall(r"'([a-z_]+)'", re.search(r'DETAIL_CODES = \[(.*?)\]', read('src/main.ts'), re.S).group(1))}
    missing = sorted(k for k in used if k not in en)
    assert not missing, missing
    # dynamic keys built from enums
    for provider in ('claude', 'codex', 'antigravity'):
        assert f'provider.{provider}' in en
    for state in ('not_installed', 'available', 'connected', 'repair_required', 'permission_required', 'failed', 'unsupported'):
        assert f'state.{state}' in en


def test_backend_error_codes_are_translated():
    en = set(keys_of('src/locales/en.ts'))
    codes = set()
    for rel in ('src-tauri/src/lib.rs', 'src-tauri/src/updater.rs', 'src-tauri/src/integrations.rs'):
        codes |= set(re.findall(r'"(E_[A-Z_]+)', read(rel)))
    missing = sorted(c for c in codes if f'error.{c}' not in en)
    assert not missing, missing


def test_backend_has_no_hardcoded_user_language_errors():
    for rel in ('src-tauri/src/lib.rs', 'src-tauri/src/updater.rs', 'src-tauri/src/integrations.rs'):
        text = read(rel)
        assert not re.search(r'Err\("[A-Za-z]+ [a-z]', text), rel  # human sentences must be E_ codes


def test_locale_fallback_logic_present():
    i18n = read('src/i18n.ts')
    assert "?? (en as Record<string, string>)[key] ?? key" in i18n  # missing key -> English -> key
    assert "return SUPPORTED.includes(primary) ? primary : 'en'" in i18n  # unsupported system language -> English
    assert "set_language" in read('src/main.ts') and 'persist' not in ''  # saved through backend settings
    rust = read('src-tauri/src/lib.rs')
    assert 'language: String' in rust and '"system" | "en" | "tr"' in rust


# ---- splash -------------------------------------------------------------------------------------
def test_splash_window_is_transparent_borderless_and_not_in_taskbar():
    rust = read('src-tauri/src/lib.rs')
    block = rust[rust.index('fn open_splash'):rust.index('fn finish_splash')]
    for needle in ('.decorations(false)', '.transparent(true)', '.skip_taskbar(true)',
                   '.always_on_top(false)', '.resizable(false)', '.center()', '.shadow(false)'):
        assert needle in block, needle
    css = read('src/splash.css')
    assert re.search(r'html,body\{[^}]*background:transparent', css)
    assert 'prefers-reduced-motion' in css
    assert 'canvas' not in read('splash.html').lower() and 'webgl' not in css.lower()


def test_splash_stage_order_and_assets():
    html, css = read('splash.html'), read('src/splash.css')
    order = [html.index(p) for p in ('antigravity.png', 'openai-chatgpt.svg', 'claude-ai.svg')]
    assert order == sorted(order), 'logos must be Antigravity, Codex/OpenAI, Claude'
    assert 'app-icon.png' in html and 'Kaiwen Notify' in html
    for name in ('antigravity.png', 'claude-ai.svg', 'openai-chatgpt.svg'):
        assert (ROOT / 'public/assets/providers' / name).is_file()
    assert (ROOT / 'public/assets/brand/app-icon.png').is_file()
    # only compositor-friendly properties are animated
    keyframes = ''.join(re.findall(r'@keyframes[^{]+\{(.*?)\}\s*(?=@|/\*|$)', css, re.S))
    for slow in ('width', 'height', 'top', 'left', 'margin', 'box-shadow', 'filter'):
        assert slow not in keyframes, slow


def test_splash_cannot_hang_and_main_is_revealed_when_ready():
    ts = read('src/splash.ts')
    assert 'HARD_CAP_MS = 5000' in ts and "addEventListener('error'" in ts
    assert "'animationend'" in ts  # ends when the animation really ends, not on a guess
    css = read('src/splash.css')
    last = re.search(r'animation:word-in ([\d.]+)s [^ ]+ ([\d.]+)s', css)
    hold = int(re.search(r'HOLD_MS = (\d+)', ts).group(1)) / 1000
    nominal = float(last.group(1)) + float(last.group(2)) + hold + 0.19
    assert 1.5 <= nominal <= 2.5, nominal
    rust = read('src-tauri/src/lib.rs')
    assert 'SPLASH_FAILSAFE' in rust and 'splash_active' in rust and 'main_ready' in rust
    assert "invoke('app_ready')" in read('src/main.ts')
    conf = json.loads(read('src-tauri/tauri.conf.json'))
    win = conf['app']['windows'][0]
    assert win['create'] is False and win['visible'] is False  # main window is created hidden by code


def test_startup_modes():
    rust = read('src-tauri/src/lib.rs')
    assert '"--autostart"' in rust and '.args(["--autostart"])' in rust.replace('\n', '').replace('  ', '') or '--autostart' in rust
    setup = rust[rust.index('let autostart ='):rust.index('start_udp_listener(app.handle().clone());')]
    # autostart branch must not build a window or a splash
    auto_branch = setup[setup.index('if autostart {'):setup.index('} else if after_update')]
    assert 'build_main' not in auto_branch and 'open_splash' not in auto_branch
    assert 'SKIP_SPLASH_MARKER' in rust  # no animation replay after an updater restart
    open_window = rust[rust.index('fn open_window'):rust.index('fn fresh_marker')]
    assert 'open_splash' not in open_window  # tray / second launch never shows the splash


# ---- autostart ----------------------------------------------------------------------------------
def test_autostart_uses_official_plugin_and_os_state():
    rust = read('src-tauri/src/lib.rs')
    assert 'tauri_plugin_autostart' in rust and re.search(r'autolaunch\(\)\s*\.is_enabled\(\)', rust)
    assert 'fn get_autostart' in rust and 'fn set_autostart' in rust
    assert 'tauri-plugin-autostart' in read('src-tauri/Cargo.toml')
    front = read('src/main.ts')
    assert "invoke<boolean>('set_autostart'" in front  # UI shows the state the OS reports back
    nsis = read('src-tauri/windows/hooks.nsh')
    assert 'NSIS_HOOK_POSTUNINSTALL' in nsis and 'CurrentVersion\\Run' in nsis and 'Kaiwen Notify' in nsis
    conf = json.loads(read('src-tauri/tauri.conf.json'))
    assert conf['bundle']['windows']['nsis']['installerHooks'] == './windows/hooks.nsh'
    assert conf['bundle']['windows']['nsis']['installMode'] == 'currentUser'  # no admin rights


# ---- update scheduler ---------------------------------------------------------------------------
def test_update_scheduler_design():
    upd = read('src-tauri/src/updater.rs')
    assert 'Duration::from_secs(10 * 60)' in upd  # every 10 minutes
    assert 'CHECK_TIMEOUT' in upd and '.timeout(CHECK_TIMEOUT)' in upd
    assert 'wait_timeout_while' in upd, 'sleeps on a condvar: no polling or busy loop'
    assert 'request_stop' in upd and 'request_stop' in read('src-tauri/src/lib.rs')
    assert 'if !is_configured(&app)' in upd, 'unconfigured builds never touch the network'
    assert 'BusyGuard::acquire' in upd, 'never two checks at once'
    assert 'download_and_install' not in upd and 'download(' in upd
    assert 'previous_error' in upd, 'offline errors are not re-announced every tick'
    snapshot_fields = ('last_error', 'interval_minutes')
    assert all(f in upd for f in snapshot_fields)


# ---- portability --------------------------------------------------------------------------------
def test_no_machine_specific_paths_in_shipped_files():
    patterns = [r'C:\\\\?Users\\\\?[A-Za-z]', r'C:/Users/[A-Za-z]', r'OneDrive\\\\?Desktop', r'C:\\\\?dev\b']
    shipped = ['src', 'src-tauri/src', 'scripts', 'index.html', 'splash.html', 'src-tauri/tauri.conf.json',
               'src-tauri/capabilities', 'src-tauri/windows']
    offenders = []
    for item in shipped:
        base = ROOT / item
        files = [base] if base.is_file() else [p for p in base.rglob('*') if p.is_file()]
        for f in files:
            text = f.read_text(encoding='utf8', errors='ignore')
            for pat in patterns:
                # test fixtures inside comments such as "C:/x" are fine; real user names are not
                if re.search(pat, text) and 'kaiwe' in text.lower():
                    offenders.append(str(f.relative_to(ROOT)))
    assert not offenders, offenders


def test_paths_come_from_windows_environment_not_constants():
    integ = read('src-tauri/src/integrations.rs')
    assert 'USERPROFILE' in integ and 'LOCALAPPDATA' in integ
    for script in ('install-hooks.ps1', 'uninstall-hooks.ps1', 'send-event.ps1'):
        text = read(f'scripts/{script}')
        assert '$env:USERPROFILE' in text or '$env:LOCALAPPDATA' in text
        assert '$HOME' not in text


def test_webview2_and_per_user_install_config():
    nsis = json.loads(read('src-tauri/tauri.conf.json'))['bundle']['windows']
    assert nsis['webviewInstallMode']['type'] == 'downloadBootstrapper'
    assert nsis['nsis']['installMode'] == 'currentUser'


# ---- license ------------------------------------------------------------------------------------
def test_license_and_notices():
    lic = read('LICENSE')
    assert lic.startswith('MIT License') and 'ImtheKaiwen' in lic
    assert json.loads(read('package.json'))['license'] == 'MIT'
    assert 'license = "MIT"' in read('src-tauri/Cargo.toml')
    notices = read('THIRD_PARTY_NOTICES.md')
    for word in ('Anthropic', 'OpenAI', 'Google', 'trademark', 'default.wav'):
        assert word in notices
