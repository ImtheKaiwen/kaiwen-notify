import './style.css';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';

type Theme = 'system' | 'light' | 'dark';
type Settings = { enabled: boolean; sound_path: string; theme: Theme; auto_update: boolean; last_update_check: number };
type NotificationEvent = { source: string; timestamp: number };
type Status = { settings: Settings; received: number; listener_ok: boolean; last_event: NotificationEvent | null };
type ProviderId = 'claude' | 'codex' | 'antigravity';
type ProviderState = 'not_installed' | 'available' | 'connected' | 'permission_required' | 'failed' | 'unsupported';
type ProviderReport = { id: ProviderId; state: ProviderState; detail: string | null };
type Progress = { operation: 'install' | 'uninstall'; provider: ProviderId; phase: 'waiting' | 'working' | 'done' | 'failed' | 'skipped'; state: ProviderState; message: string };
type UpdateMeta = { version: string; notes: string | null; date: string | null };
type UpdateSnapshot = { configured: boolean; current_version: string; latest: UpdateMeta | null; downloaded: boolean };
type DownloadProgress = { downloaded: number; total: number | null; phase: 'downloading' | 'verifying' | 'ready' };

// Static assets in /public/assets/providers. `mono` logos use currentColor, so they are drawn as a CSS
// mask to follow the light/dark text color; brand-colored logos are shown unchanged.
const PROVIDERS: { id: ProviderId; name: string; mark: string; logo: string; mono: boolean }[] = [
  { id: 'claude', name: 'Claude Code', mark: 'C', logo: '/assets/providers/claude-ai.svg', mono: false },
  { id: 'codex', name: 'Codex CLI / Desktop', mark: '>_', logo: '/assets/providers/openai-chatgpt.svg', mono: true },
  { id: 'antigravity', name: 'Antigravity', mark: 'A', logo: '/assets/providers/antigravity.png', mono: false },
];
const STATE_LABEL: Record<ProviderState, string> = {
  not_installed: 'Kurulu değil',
  available: 'Kullanılabilir',
  connected: 'Bağlı',
  permission_required: 'İzin gerekli',
  failed: 'Başarısız',
  unsupported: 'Desteklenmiyor',
};

const el = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const installBtn = el<HTMLButtonElement>('install-integrations');
const repairBtn = el<HTMLButtonElement>('reinstall-integrations');
const uninstallBtn = el<HTMLButtonElement>('uninstall-integrations');
const updateBtn = el<HTMLButtonElement>('update-action');
const enabledBox = el<HTMLInputElement>('enabled');
const autoUpdateBox = el<HTMLInputElement>('auto-update');

let reports: ProviderReport[] = [];
let busy = false; // integration operation in flight
let toastTimer: ReturnType<typeof setTimeout> | undefined;
let settings: Settings | null = null;
let update: UpdateSnapshot | null = null;
let updateMode: 'check' | 'download' | 'install' = 'check';
let updateBusy = false;
let updateError: string | null = null;
const live = new Map<ProviderId, Progress>();

function notify(message: string, isError = false) {
  const toast = el('toast');
  toast.textContent = message;
  toast.classList.toggle('error', isError);
  toast.classList.add('visible');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.remove('visible'), 3800);
}
function setLoading(button: HTMLButtonElement, on: boolean) {
  button.classList.toggle('loading', on);
  button.disabled = on;
}
async function flash(button: HTMLButtonElement, ok: boolean) {
  button.classList.add(ok ? 'success' : 'error');
  setTimeout(() => button.classList.remove('success', 'error'), 1500);
}

/* ---------- theme ---------- */
function applyTheme(theme: Theme) {
  if (theme === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  try { localStorage.setItem('kn-theme', theme); } catch { /* storage may be unavailable */ }
  document.querySelectorAll<HTMLElement>('[data-theme-choice]').forEach((b) =>
    b.setAttribute('aria-checked', String(b.dataset.themeChoice === theme)));
}
document.querySelectorAll<HTMLElement>('[data-theme-choice]').forEach((b) =>
  b.addEventListener('click', async () => {
    const theme = b.dataset.themeChoice as Theme;
    applyTheme(theme);
    try { await invoke('set_theme', { theme }); } catch (e) { notify(String(e), true); }
  }));

/* ---------- integrations ---------- */
function providerAvatar(meta: (typeof PROVIDERS)[number]): HTMLSpanElement {
  const avatar = Object.assign(document.createElement('span'), { className: 'avatar' });
  avatar.setAttribute('aria-hidden', 'true');
  const fallback = () => { avatar.replaceChildren(document.createTextNode(meta.mark)); avatar.classList.add('fallback'); };
  const probe = new Image();
  probe.alt = '';
  probe.addEventListener('error', fallback, { once: true });
  if (meta.mono) {
    const glyph = Object.assign(document.createElement('span'), { className: 'logo logo-mono' });
    glyph.style.setProperty('--logo', `url("${meta.logo}")`);
    avatar.append(glyph);
    probe.src = meta.logo; // only used to detect a missing file
  } else {
    probe.className = 'logo';
    probe.src = meta.logo;
    avatar.append(probe);
  }
  return avatar;
}
function providerRow(id: ProviderId): HTMLLIElement {
  const meta = PROVIDERS.find((p) => p.id === id)!;
  const li = document.createElement('li');
  const avatar = providerAvatar(meta);
  const name = Object.assign(document.createElement('span'), { className: 'strong', textContent: meta.name });
  const state = Object.assign(document.createElement('span'), { className: 'pstate' });
  const report = reports.find((r) => r.id === id);
  const progress = live.get(id);
  let cls = '';
  let text = report ? STATE_LABEL[report.state] : 'Kontrol ediliyor';
  let title = report?.detail ?? '';
  if (progress && (busy || progress.phase === 'failed')) {
    title = progress.message;
    if (progress.phase === 'waiting') text = 'Sırada';
    else if (progress.phase === 'working') {
      state.append(Object.assign(document.createElement('span'), { className: 'spin' }));
      text = progress.operation === 'install' ? 'Kuruluyor…' : 'Kaldırılıyor…';
    } else if (progress.phase === 'failed') { text = 'Başarısız'; cls = 'failed'; }
    else if (progress.phase === 'done') { text = progress.operation === 'install' ? 'Bağlandı' : 'Kaldırıldı'; cls = progress.operation === 'install' ? 'connected' : ''; }
    else text = progress.state === 'connected' ? 'Bağlı' : STATE_LABEL[progress.state];
  } else if (report?.state === 'connected') cls = 'connected';
  else if (report?.state === 'failed') cls = 'failed';
  state.append(document.createTextNode(text));
  state.classList.add(...(cls ? [cls] : []));
  if (title) state.title = title;
  li.append(avatar, name, state);
  return li;
}
function renderProviders() {
  for (const id of ['setup-providers', 'dash-providers']) {
    el(id).replaceChildren(...PROVIDERS.map((p) => providerRow(p.id)));
  }
  const connected = reports.filter((r) => r.state === 'connected').length;
  const actionable = reports.filter((r) => r.state === 'available' || r.state === 'permission_required').length;
  const failed = reports.filter((r) => r.state === 'failed');
  const unsupported = reports.length > 0 && reports.every((r) => r.state === 'unsupported');
  if (!busy) {
    el('setup-view').hidden = connected !== 0 && !showSetupHold;
    el('dashboard-view').hidden = connected === 0 || showSetupHold;
  }
  installBtn.disabled = busy || actionable === 0;
  repairBtn.disabled = busy || actionable === 0;
  uninstallBtn.disabled = busy || connected === 0;
  const feedback = el('setup-feedback');
  feedback.classList.toggle('problem', failed.length > 0 || unsupported);
  feedback.textContent = unsupported ? 'Bu platform desteklenmiyor; hook kurulumu yalnızca Windows üzerinde çalışır.'
    : failed.length > 0 ? `${failed.length} entegrasyon kurulamadı: ${failed.map((f) => f.detail ?? '').filter(Boolean).join('; ') || 'ayrıntı yok'}. Tekrar deneyebilirsin.`
    : reports.length > 0 && actionable === 0 && connected === 0 ? 'Desteklenen bir araç bulunamadı. Claude Code, Codex veya Antigravity\'yi bir kez çalıştırıp tekrar dene.'
    : 'Codex Computer Use dahil mevcut ayarlar korunur.';
  const msg = el('integration-message');
  msg.classList.toggle('problem', failed.length > 0);
  msg.textContent = failed.length > 0 ? `Bazı entegrasyonlar tamamlanamadı (${failed.length}). Onar ile yeniden dene.`
    : connected === PROVIDERS.length ? 'Tüm hook\'lar bağlı. Codex ilk kullanımda Settings > Hooks üzerinden güven onayı isteyebilir.'
    : 'Bazı entegrasyonlar bağlı değil. Eksikleri kur / Onar ile yeniden deneyebilirsin.';
}
let showSetupHold = false; // keep the onboarding view visible while the final install states are shown

async function refreshIntegrations() {
  reports = await invoke<ProviderReport[]>('get_integrations');
  renderProviders();
}
async function runOperation(install: boolean, trigger: HTMLButtonElement) {
  if (busy) return;
  busy = true;
  live.clear();
  setLoading(trigger, true);
  [installBtn, repairBtn, uninstallBtn].forEach((b) => { if (b !== trigger) b.disabled = true; });
  renderProviders();
  try {
    reports = await invoke<ProviderReport[]>(install ? 'install_integrations' : 'uninstall_integrations');
    const connected = reports.filter((r) => r.state === 'connected').length;
    const failed = reports.filter((r) => r.state === 'failed').length;
    flash(trigger, failed === 0);
    if (install) {
      notify(failed === 0 && connected > 0 ? 'Entegrasyonlar bağlandı' : `${connected} bağlandı, ${failed} başarısız`, failed > 0 || connected === 0);
    } else {
      notify(failed === 0 && connected === 0 ? 'Entegrasyonlar kaldırıldı' : 'Kaldırma tamamlanamadı; bazı hook\'lar duruyor', true);
    }
    // Show the real final states briefly, then let the connected count pick the view.
    showSetupHold = install && connected > 0 && el('dashboard-view').hidden;
    busy = false;
    renderProviders();
    if (showSetupHold) { await new Promise((r) => setTimeout(r, 900)); showSetupHold = false; renderProviders(); }
  } catch (error) {
    notify(String(error), true);
    flash(trigger, false);
    busy = false;
    try { await refreshIntegrations(); } catch { /* keep first error */ }
  } finally {
    busy = false;
    setLoading(trigger, false);
    renderProviders();
  }
}
installBtn.addEventListener('click', () => { void runOperation(true, installBtn); });
repairBtn.addEventListener('click', () => { void runOperation(true, repairBtn); });
uninstallBtn.addEventListener('click', () => {
  const dialog = el<HTMLDialogElement>('confirm-dialog');
  dialog.returnValue = 'cancel';
  dialog.addEventListener('close', () => { if (dialog.returnValue === 'confirm') void runOperation(false, uninstallBtn); }, { once: true });
  dialog.showModal();
});

/* ---------- status / sound / settings ---------- */
function presentSound(path: string) {
  el('sound-name').textContent = path ? (path.split(/[\\/]/).pop() ?? path) : 'Kaiwen Default';
  el('sound-info').textContent = path ? 'Özel WAV sesi' : 'Yerleşik kısa bildirim sesi';
}
function renderStatus(status: Status) {
  settings = status.settings;
  enabledBox.checked = settings.enabled;
  autoUpdateBox.checked = settings.auto_update;
  applyTheme(settings.theme);
  presentSound(settings.sound_path);
  const active = status.listener_ok && settings.enabled;
  el('status-dot').className = `dot ${active ? 'active' : status.listener_ok ? '' : 'offline'}`;
  el('status-label').textContent = !status.listener_ok ? 'Yerel dinleyici açılamadı' : settings.enabled ? 'Dinleniyor · Arka planda aktif' : 'Bildirimler duraklatıldı';
  el('event-count').textContent = `${status.received} bildirim`;
  el('last-event').textContent = status.last_event ? `Son: ${status.last_event.source} · ${new Date(status.last_event.timestamp).toLocaleTimeString()}` : 'Henüz görev bildirimi alınmadı.';
  renderUpdate();
}
async function reload() { renderStatus(await invoke<Status>('get_status')); }
enabledBox.addEventListener('change', async () => {
  try { await invoke('set_enabled', { enabled: enabledBox.checked }); } catch (e) { notify(String(e), true); }
  await reload();
});
autoUpdateBox.addEventListener('change', async () => {
  try { await invoke('set_auto_update', { enabled: autoUpdateBox.checked }); } catch (e) { notify(String(e), true); }
  await reload();
});
el('pick-sound').addEventListener('click', async () => {
  try {
    const path = await open({ multiple: false, directory: false, filters: [{ name: 'Wave Audio', extensions: ['wav'] }] });
    if (!path || Array.isArray(path)) return;
    await invoke('set_sound', { path });
    await reload();
    notify('Ses kaydedildi');
  } catch (e) { notify(String(e), true); }
});
el('reset-sound').addEventListener('click', async () => {
  try { await invoke('set_sound', { path: '' }); await reload(); notify('Varsayılan ses seçildi'); }
  catch (e) { notify(String(e), true); }
});
const testBtn = el<HTMLButtonElement>('test-sound');
testBtn.addEventListener('click', async () => {
  setLoading(testBtn, true);
  try { await invoke('test_sound'); flash(testBtn, true); }
  catch (e) { notify(String(e), true); flash(testBtn, false); }
  finally { setLoading(testBtn, false); }
});

/* ---------- updates ---------- */
const mb = (n: number) => {
  if (n < 1024) return `${n} B`;
  if (n < 1048576) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`;
  return `${(n / 1048576).toFixed(n < 10485760 ? 1 : 0)} MB`;
};
function setProgress(visible: boolean, determinate = false, ratio = 0, text = '') {
  const box = el('update-progress');
  box.hidden = !visible;
  box.classList.toggle('indeterminate', visible && !determinate);
  el('update-bar').style.width = determinate ? `${Math.min(100, ratio * 100).toFixed(1)}%` : '';
  const label = el('update-bytes');
  label.hidden = !visible;
  label.textContent = text;
}
function renderUpdate() {
  const title = el('update-title');
  const sub = el('update-sub');
  const notes = el<HTMLPreElement>('update-notes');
  const label = updateBtn.querySelector('.btn-label')!;
  const last = settings?.last_update_check ? `Son kontrol: ${new Date(settings.last_update_check).toLocaleString()}` : 'Henüz kontrol edilmedi';
  notes.hidden = true;
  if (!update) { title.textContent = 'Güncelleme durumu okunuyor…'; updateBtn.disabled = true; return; }
  const current = `Mevcut sürüm v${update.current_version}`;
  if (!update.configured) {
    title.textContent = 'Güncelleme sunucusu yapılandırılmamış';
    sub.textContent = `${current}. Bu derleme bir güncelleme kaynağına bağlı değil.`;
    label.textContent = 'Güncellemeleri Kontrol Et';
    updateBtn.disabled = true;
    return;
  }
  if (updateError) { title.textContent = 'Güncelleme başarısız'; sub.textContent = updateError; }
  else if (update.latest) {
    title.textContent = update.downloaded ? `v${update.latest.version} indirildi` : `Yeni sürüm: v${update.latest.version}`;
    sub.textContent = `${current} · ${last}`;
    if (update.latest.notes) { notes.hidden = false; notes.textContent = update.latest.notes; }
  } else { title.textContent = 'Uygulama güncel'; sub.textContent = `${current} · ${last}`; }
  updateMode = update.latest ? (update.downloaded ? 'install' : 'download') : 'check';
  label.textContent = updateMode === 'install' ? 'Kur ve Yeniden Başlat'
    : updateMode === 'download' ? (updateError ? 'Tekrar dene' : 'İndir') : 'Güncellemeleri Kontrol Et';
  updateBtn.classList.toggle('btn-primary', updateMode !== 'check');
  updateBtn.disabled = updateBusy;
}
async function updateAction() {
  if (updateBusy || !update) return;
  updateBusy = true;
  updateError = null;
  setLoading(updateBtn, true);
  try {
    if (updateMode === 'check') {
      update = await invoke<UpdateSnapshot>('check_update');
      await reload();
      if (!update.latest) notify('Uygulama güncel');
    } else if (updateMode === 'download') {
      setProgress(true, false, 0, 'İndirme başlıyor…');
      update = await invoke<UpdateSnapshot>('download_update');
      setProgress(false);
    } else {
      setProgress(true, false, 0, 'Yükleyici başlatılıyor…');
      await invoke('install_update'); // on success Windows closes the app and the installer relaunches it
    }
  } catch (error) {
    updateError = String(error);
    setProgress(false);
    notify(updateError, true);
    flash(updateBtn, false);
  } finally {
    updateBusy = false;
    setLoading(updateBtn, false);
    renderUpdate();
  }
}
updateBtn.addEventListener('click', () => { void updateAction(); });

/* ---------- events (registered once; Rust only emits when something happens) ---------- */
void listen<NotificationEvent>('notify-event', () => { void reload(); });
void listen<Progress>('integration-progress', (e) => {
  live.set(e.payload.provider, e.payload);
  // Make the real progress visible even when triggered from the dashboard.
  renderProviders();
});
void listen<DownloadProgress>('update-progress', (e) => {
  const { downloaded, total, phase } = e.payload;
  if (phase === 'verifying') setProgress(true, false, 0, 'İmza doğrulanıyor…');
  else if (total && total > 0) setProgress(true, true, downloaded / total, `%${Math.floor((downloaded / total) * 100)} · ${mb(downloaded)} / ${mb(total)}`);
  else setProgress(true, false, 0, downloaded > 0 ? `${mb(downloaded)} indirildi` : 'İndirme başlıyor…');
});
void listen<UpdateSnapshot>('update-state', (e) => { update = e.payload; void reload(); });

(async () => {
  try {
    document.getElementById('app-version')!.textContent = '';
    await Promise.all([reload(), refreshIntegrations()]);
    update = await invoke<UpdateSnapshot>('get_update_state');
    el('app-version').textContent = `v${update.current_version}`;
    renderUpdate();
  } catch (error) {
    notify(String(error), true);
    el('setup-view').hidden = false;
    el('setup-feedback').textContent = `Durum okunamadı: ${String(error)}`;
    el('setup-feedback').classList.add('problem');
  }
})();
