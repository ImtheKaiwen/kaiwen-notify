import './style.css';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { errorText, onLanguageChange, resolveLanguage, setLanguage, t, language } from './i18n';
import type { Key } from './locales/en';

type Theme = 'system' | 'light' | 'dark';
type ProductSeen = { last_seen: number; last_completed: number; last_note: string | null };
type SoundChoice = { file: string; name: string };
type ProviderSound = { enabled: boolean; volume: number; main: SoundChoice; subagent: SoundChoice };
type Settings = { antigravity: Record<string, ProductSeen>; providers: Record<'claude' | 'codex' | 'antigravity', ProviderSound>; subagent_notifications: boolean; subagent_seen: Record<string, number>; enabled: boolean; sound_path: string; theme: Theme; auto_update: boolean; last_update_check: number; language: string };
type NotificationEvent = { source: string; timestamp: number; category: 'main_complete' | 'subagent_complete' | 'unknown' };
type Status = { settings: Settings; missing_sounds: string[]; system_language: string; received: number; listener_ok: boolean; last_event: NotificationEvent | null };
type ProviderId = 'claude' | 'codex' | 'antigravity';
type ProviderState = 'not_installed' | 'available' | 'connected' | 'repair_required' | 'permission_required' | 'failed' | 'unsupported';
type ProviderReport = { id: ProviderId; state: ProviderState; detail: string | null; subagent: 'unsupported' | 'hook_missing' | 'ready' };
type Progress = { operation: 'install' | 'uninstall'; provider: ProviderId; phase: 'waiting' | 'working' | 'done' | 'failed' | 'skipped'; state: ProviderState; message: string };
type UpdateMeta = { version: string; notes: string | null; date: string | null };
type UpdateSnapshot = { configured: boolean; current_version: string; latest: UpdateMeta | null; downloaded: boolean; last_error: string | null; interval_minutes: number };
type DownloadProgress = { downloaded: number; total: number | null; phase: 'downloading' | 'verifying' | 'ready' };

// Static assets in /public/assets/providers. `mono` logos use currentColor, so they are drawn as a CSS
// mask to follow the light/dark text color; brand-colored logos are shown unchanged.
const PROVIDERS: { id: ProviderId; mark: string; logo: string; mono: boolean }[] = [
  { id: 'claude', mark: 'C', logo: '/assets/providers/claude-ai.svg', mono: false },
  { id: 'codex', mark: '>_', logo: '/assets/providers/openai-chatgpt.svg', mono: true },
  { id: 'antigravity', mark: 'A', logo: '/assets/providers/antigravity.png', mono: false },
];
// Backend detail codes that have a translation.
const DETAIL_CODES = ['unsupported_os', 'repair_required', 'not_detected', 'readonly_config', 'already_connected', 'skipped_not_detected', 'skipped', 'waiting'];

const el = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const installBtn = el<HTMLButtonElement>('install-integrations');
const repairBtn = el<HTMLButtonElement>('reinstall-integrations');
const uninstallBtn = el<HTMLButtonElement>('uninstall-integrations');
const updateBtn = el<HTMLButtonElement>('update-action');
const enabledBox = el<HTMLInputElement>('enabled');
const autoUpdateBox = el<HTMLInputElement>('auto-update');
const autostartBox = el<HTMLInputElement>('autostart');
const subagentBox = el<HTMLInputElement>('subagent-notifications');

let reports: ProviderReport[] = [];
let busy = false; // integration operation in flight
let toastTimer: ReturnType<typeof setTimeout> | undefined;
let status: Status | null = null;
let settings: Settings | null = null;
let update: UpdateSnapshot | null = null;
let updateMode: 'check' | 'download' | 'install' = 'check';
let updateBusy = false;
let updateError: string | null = null;
let languageApplied = false;
let showSetupHold = false; // keep the onboarding view visible while the final install states are shown
const live = new Map<ProviderId, Progress>();

function notify(message: string, isError = false) {
  const toast = el('toast');
  toast.textContent = message;
  toast.classList.toggle('error', isError);
  toast.classList.add('visible');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.remove('visible'), 3800);
}
const fail = (error: unknown) => notify(errorText(error), true);
function setLoading(button: HTMLButtonElement, on: boolean) {
  button.classList.toggle('loading', on);
  button.disabled = on;
}
function flash(button: HTMLButtonElement, ok: boolean) {
  button.classList.add(ok ? 'success' : 'error');
  setTimeout(() => button.classList.remove('success', 'error'), 1500);
}

/* ---------- theme & language ---------- */
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
    try { await invoke('set_theme', { theme }); } catch (e) { fail(e); }
  }));

function markLanguageChoice(preference: string) {
  document.querySelectorAll<HTMLElement>('[data-language-choice]').forEach((b) =>
    b.setAttribute('aria-checked', String(b.dataset.languageChoice === preference)));
}
document.querySelectorAll<HTMLElement>('[data-language-choice]').forEach((b) =>
  b.addEventListener('click', async () => {
    const preference = b.dataset.languageChoice!;
    markLanguageChoice(preference);
    if (settings) settings.language = preference;
    setLanguage(resolveLanguage(preference, status?.system_language)); // instant, no restart
    try { await invoke('set_language', { language: preference }); } catch (e) { fail(e); }
  }));

/* ---------- integrations ---------- */
const detailText = (code: string | null | undefined) =>
  code && DETAIL_CODES.includes(code) ? t(`detail.${code}` as Key) : (code ?? '');

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
  const name = Object.assign(document.createElement('span'), { className: 'strong', textContent: t(`provider.${id}` as Key) });
  const state = Object.assign(document.createElement('span'), { className: 'pstate' });
  const report = reports.find((r) => r.id === id);
  const progress = live.get(id);
  let cls = '';
  let text: string = report ? t(`state.${report.state}` as Key) : t('state.checking');
  let title = report?.detail ? detailText(report.detail) : '';
  if (progress && (busy || progress.phase === 'failed')) {
    title = progress.phase === 'failed' ? errorText(progress.message) : detailText(progress.message);
    if (progress.phase === 'waiting') text = t('phase.waiting');
    else if (progress.phase === 'working') {
      state.append(Object.assign(document.createElement('span'), { className: 'spin' }));
      text = progress.operation === 'install' ? t('phase.installing') : t('phase.uninstalling');
    } else if (progress.phase === 'failed') { text = t('phase.failed'); cls = 'failed'; }
    else if (progress.phase === 'done') { text = progress.operation === 'install' ? t('phase.connected') : t('phase.removed'); cls = progress.operation === 'install' ? 'connected' : ''; }
    else text = t(`state.${progress.state}` as Key);
  } else if (report?.state === 'connected') cls = 'connected';
  else if (report?.state === 'failed') cls = 'failed';
  state.append(document.createTextNode(text));
  if (cls) state.classList.add(cls);
  if (title) state.title = title;
  li.append(providerAvatar(meta), name, state);
  return li;
}

function renderProviders() {
  queueMicrotask(renderAntigravityDiag);
  for (const id of ['setup-providers', 'dash-providers']) {
    el(id).replaceChildren(...PROVIDERS.map((p) => providerRow(p.id)));
  }
  const connected = reports.filter((r) => r.state === 'connected' || r.state === 'repair_required').length;
  const actionable = reports.filter((r) => ['available', 'permission_required', 'repair_required'].includes(r.state)).length;
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
  feedback.textContent = unsupported ? t('onboarding.unsupported')
    : failed.length > 0 ? t('onboarding.failed', { n: failed.length, details: failed.map((f) => (f.detail ? errorText(f.detail) : '')).filter(Boolean).join('; ') || '—' })
    : reports.length > 0 && actionable === 0 && connected === 0 ? t('onboarding.noneDetected')
    : reports.length === 0 ? t('onboarding.checking')
    : t('onboarding.preserved');
  const allConnected = reports.length > 0 && reports.every((r) => r.state === 'connected');
  const msg = el('integration-message');
  msg.classList.toggle('problem', failed.length > 0);
  msg.textContent = failed.length > 0 ? t('integrations.partialFailure', { n: failed.length })
    : allConnected ? t('integrations.allConnected')
    : t('integrations.someMissing');
}

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
    if (install) notify(failed === 0 && connected > 0 ? t('toast.connected') : t('toast.partial', { ok: connected, failed }), failed > 0 || connected === 0);
    else notify(failed === 0 && connected === 0 ? t('toast.removed') : t('toast.removeIncomplete'), failed > 0 || connected > 0);
    // Show the real final states briefly, then let the connected count pick the view.
    showSetupHold = install && connected > 0 && el('dashboard-view').hidden;
    busy = false;
    renderProviders();
    if (showSetupHold) { await new Promise((r) => setTimeout(r, 900)); showSetupHold = false; renderProviders(); }
  } catch (error) {
    fail(error);
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
/** Per-runtime proof that Antigravity really ran our hook; a hook file alone is never shown as success. */
function renderAntigravityDiag() {
  const box = el('antigravity-diag');
  const state = reports.find((r) => r.id === 'antigravity')?.state;
  box.hidden = !(state === 'connected' || state === 'repair_required');
  if (box.hidden) return;
  const when = (ms: number) => new Date(ms).toLocaleString(t('time.locale'));
  const rows = (['app', 'ide', 'cli'] as const).map((key) => {
    const seen = settings?.antigravity?.[key];
    const row = document.createElement('div');
    row.className = 'diag-row';
    const who = Object.assign(document.createElement('span'), { className: 'who', textContent: t(`diag.${key}` as Key) });
    const what = document.createElement('span');
    what.className = 'what';
    if (!seen || seen.last_seen === 0) what.textContent = t('diag.none');
    else if (seen.last_completed > 0 && !seen.last_note) { what.textContent = t('diag.verified', { time: when(seen.last_completed) }); what.classList.add('ok'); }
    else {
      const why = seen.last_note ? t(`diag.note.${seen.last_note}` as Key) : '';
      what.textContent = t('diag.ranSilent', { time: when(seen.last_seen), why });
    }
    row.append(who, what);
    return row;
  });
  const title = Object.assign(document.createElement('h3'), { textContent: t('diag.title') });
  const hint = Object.assign(document.createElement('p'), { className: 'note', textContent: t('diag.hint') });
  box.replaceChildren(title, ...rows, hint);
}
/* ---------- per-provider sounds ---------- */
type El = HTMLElement;
function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  node.append(...kids);
  return node;
}
let draggingVolume = false;
const when = (ms: number) => new Date(ms).toLocaleString(t('time.locale'));

/** Whether the sub-agent sound of a provider can be used, and the one-line reason when it cannot. Based on what is
 *  really registered in the provider's config, never on a hard-coded list. */
function subagentAvailability(id: ProviderId): { usable: boolean; note: string; ok: boolean } {
  const name = t(`provider.${id}` as Key);
  const report = reports.find((r) => r.id === id);
  if (report?.subagent === 'unsupported') return { usable: false, ok: false, note: t('sound.subagent.unsupported', { provider: name }) };
  if (!settings?.subagent_notifications) return { usable: false, ok: false, note: t('sound.subagent.off') };
  if (!report || (report.state !== 'connected' && report.state !== 'repair_required')) return { usable: false, ok: false, note: t('sound.subagent.notConnected') };
  if (report.subagent !== 'ready') return { usable: false, ok: false, note: t('sound.subagent.hookMissing') };
  const seen = settings.subagent_seen?.[id];
  if (seen) return { usable: true, ok: true, note: t('sound.subagent.verified', { time: when(seen) }) };
  return { usable: true, ok: false, note: t(id === 'codex' ? 'sound.subagent.waitingCodex' : 'sound.subagent.waiting') };
}

async function previewSound(provider: ProviderId, kind: 'main' | 'subagent', button: HTMLButtonElement) {
  button.disabled = true;
  try {
    const fellBack = await invoke<boolean>('preview_sound', { provider, kind });
    if (fellBack) notify(t('sound.fallbackPlayed'), true);
  } catch (e) { fail(e); }
  finally { button.disabled = false; }
}
async function chooseSound(provider: ProviderId, kind: 'main' | 'subagent') {
  try {
    const path = await open({ multiple: false, directory: false, filters: [{ name: t('sound.filter'), extensions: ['wav', 'mp3', 'ogg'] }] });
    if (!path || Array.isArray(path)) return;
    await invoke('choose_sound', { provider, kind, path });
    await reload();
    notify(t('sound.saved'));
  } catch (e) { fail(e); }
}
async function resetSound(provider: ProviderId, kind: 'main' | 'subagent') {
  try { await invoke('reset_sound', { provider, kind }); await reload(); notify(t('sound.resetDone')); }
  catch (e) { fail(e); }
}

function soundRow(id: ProviderId, kind: 'main' | 'subagent', cfg: ProviderSound, avail: { usable: boolean; note: string; ok: boolean } | null): El {
  const choice = cfg[kind];
  const missing = status?.missing_sounds?.includes(`${id}.${kind}`) ?? false;
  const usable = avail ? avail.usable : true;
  const row = h('div', { class: `srow${usable ? '' : ' off'}` });
  const name = h('div', { class: 'name' }, choice.file ? choice.name : t(kind === 'main' ? 'sound.default.main' : 'sound.default.subagent'));
  const sub = h('div', { class: `sub${missing ? ' warn' : ''}` }, missing ? t('sound.missing') : (avail ? avail.note : (choice.file ? t('sound.customInfo') : t('sound.builtIn'))));
  const preview = h('button', { class: 'btn btn-sm', type: 'button', title: t('sound.preview') }, t('sound.preview'));
  const change = h('button', { class: 'btn btn-sm', type: 'button' }, t('sound.change'));
  const reset = h('button', { class: 'btn btn-sm btn-quiet', type: 'button' }, t('sound.reset'));
  for (const button of [preview, change, reset]) (button as HTMLButtonElement).disabled = !usable;
  (reset as HTMLButtonElement).disabled = !usable || !choice.file;
  preview.addEventListener('click', () => { void previewSound(id, kind, preview as HTMLButtonElement); });
  change.addEventListener('click', () => { void chooseSound(id, kind); });
  reset.addEventListener('click', () => { void resetSound(id, kind); });
  row.append(h('span', { class: 'kind' }, t(kind === 'main' ? 'sound.main' : 'sound.subagent')), h('div', { class: 'file' }, name, sub), h('div', { class: 'tools' }, preview, change, reset));
  return row;
}

function providerSoundCard(meta: (typeof PROVIDERS)[number]): El {
  const cfg = settings!.providers[meta.id];
  const card = h('section', { class: `card pcard${cfg.enabled ? '' : ' disabled'}` });
  const toggle = h('input', { type: 'checkbox', 'aria-label': t('sound.providerEnabled', { provider: t(`provider.${meta.id}` as Key) }) }) as HTMLInputElement;
  toggle.checked = cfg.enabled;
  toggle.addEventListener('change', async () => {
    try { await invoke('set_provider_enabled', { provider: meta.id, enabled: toggle.checked }); } catch (e) { fail(e); }
    await reload();
  });
  card.append(h('div', { class: 'pcard-head' }, providerAvatar(meta), h('span', { class: 'grow' }, t(`provider.${meta.id}` as Key)), h('span', { class: 'switch' }, toggle, h('span', { class: 'track' }))));

  const range = h('input', { type: 'range', min: '0', max: '100', step: '1', 'aria-label': t('sound.volume') }) as HTMLInputElement;
  range.value = String(cfg.volume);
  const value = h('span', { class: 'value' }, `${cfg.volume}%`);
  const paint = () => { range.style.setProperty('--pct', `${range.value}%`); value.textContent = `${range.value}%`; };
  paint();
  range.addEventListener('input', () => { draggingVolume = true; paint(); });
  range.addEventListener('change', async () => {
    draggingVolume = false;
    try { await invoke('set_provider_volume', { provider: meta.id, volume: Number(range.value) }); } catch (e) { fail(e); }
    await reload();
  });
  card.append(h('div', { class: 'vol' }, h('span', { class: 'label' }, t('sound.volume')), range, value));
  card.append(soundRow(meta.id, 'main', cfg, null));
  card.append(soundRow(meta.id, 'subagent', cfg, subagentAvailability(meta.id)));
  return card;
}

function renderProviderSounds() {
  if (!settings?.providers || draggingVolume) return;
  el('provider-sounds').replaceChildren(...PROVIDERS.map(providerSoundCard));
}

function renderStatus() {
  if (!status || !settings) return;
  renderProviderSounds();
  const active = status.listener_ok && settings.enabled;
  el('status-dot').className = `dot ${active ? 'active' : status.listener_ok ? '' : 'offline'}`;
  el('status-label').textContent = !status.listener_ok ? t('status.listenerFailed') : settings.enabled ? t('status.listening') : t('status.paused');
  el('event-count').textContent = t('status.count', { n: status.received });
  el('last-event').textContent = status.last_event
    ? t(status.last_event.category === 'subagent_complete' ? 'status.lastSub' : 'status.last', { source: status.last_event.source, time: new Date(status.last_event.timestamp).toLocaleTimeString(t('time.locale')) })
    : t('status.none');
  renderAntigravityDiag();
  el('auto-update-desc').textContent = t('settings.autoUpdateDesc', { n: update?.interval_minutes ?? 10 });
}
function applyStatus(next: Status) {
  status = next;
  settings = next.settings;
  enabledBox.checked = settings.enabled;
  autoUpdateBox.checked = settings.auto_update;
  subagentBox.checked = settings.subagent_notifications;
  applyTheme(settings.theme);
  markLanguageChoice(settings.language);
  const resolved = resolveLanguage(settings.language, next.system_language);
  if (!languageApplied || resolved !== language()) { languageApplied = true; setLanguage(resolved); }
  renderStatus();
  renderUpdate();
}
async function reload() { applyStatus(await invoke<Status>('get_status')); }
enabledBox.addEventListener('change', async () => {
  try { await invoke('set_enabled', { enabled: enabledBox.checked }); } catch (e) { fail(e); }
  await reload();
});
autoUpdateBox.addEventListener('change', async () => {
  try { await invoke('set_auto_update', { enabled: autoUpdateBox.checked }); } catch (e) { fail(e); }
  await reload();
});
autostartBox.addEventListener('change', async () => {
  const wanted = autostartBox.checked;
  try {
    // The backend returns the real OS registration state after the change.
    autostartBox.checked = await invoke<boolean>('set_autostart', { enabled: wanted });
    notify(t(autostartBox.checked ? 'toast.autostartOn' : 'toast.autostartOff'));
  } catch (e) { fail(e); autostartBox.checked = !wanted; }
});
subagentBox.addEventListener('change', async () => {
  const wanted = subagentBox.checked;
  subagentBox.disabled = true;
  try {
    // Turning it on registers the sub-agent hook for connected providers; off removes only our handler.
    reports = await invoke<ProviderReport[]>('set_subagent_notifications', { enabled: wanted });
    notify(t(wanted ? 'toast.subagentsOn' : 'toast.subagentsOff'));
  } catch (e) {
    fail(e);
    // Show what is really installed now, including after a failed rollback.
    try { reports = await invoke<ProviderReport[]>('get_integrations'); } catch { /* keep the last known report */ }
  } finally { subagentBox.disabled = false; await reload(); renderProviders(); }
});

/* ---------- updates ---------- */
const bytes = (n: number) => {
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
  notes.hidden = true;
  if (!update) { title.textContent = t('update.reading'); sub.textContent = ''; label.textContent = t('update.check'); updateBtn.disabled = true; return; }
  const last = settings?.last_update_check
    ? t('update.lastCheck', { time: new Date(settings.last_update_check).toLocaleString(t('time.locale')) })
    : t('update.neverChecked');
  if (!update.configured) {
    title.textContent = t('update.notConfigured');
    sub.textContent = t('update.notConfiguredSub', { v: update.current_version });
    label.textContent = t('update.check');
    updateBtn.disabled = true;
    return;
  }
  const subLine = t('update.sub', { v: update.current_version, last });
  if (updateError) { title.textContent = t('update.failed'); sub.textContent = updateError; }
  else if (update.latest) {
    title.textContent = update.downloaded ? t('update.downloaded', { v: update.latest.version }) : t('update.available', { v: update.latest.version });
    sub.textContent = subLine;
    if (update.latest.notes) { notes.hidden = false; notes.textContent = update.latest.notes; }
  } else {
    title.textContent = t('update.upToDate');
    sub.textContent = update.last_error ? `${subLine} · ${t('update.autoError', { error: errorText(update.last_error) })}` : subLine;
  }
  updateMode = update.latest ? (update.downloaded ? 'install' : 'download') : 'check';
  label.textContent = updateMode === 'install' ? t('update.install')
    : updateMode === 'download' ? (updateError ? t('update.retry') : t('update.download')) : t('update.check');
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
      if (!update.latest) notify(t('update.nowUpToDate'));
    } else if (updateMode === 'download') {
      setProgress(true, false, 0, t('update.starting'));
      update = await invoke<UpdateSnapshot>('download_update');
      setProgress(false);
    } else {
      setProgress(true, false, 0, t('update.launching'));
      await invoke('install_update'); // on success Windows closes the app and the installer relaunches it
    }
  } catch (error) {
    updateError = errorText(error);
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
onLanguageChange(() => { renderProviders(); renderStatus(); renderUpdate(); });
void listen<NotificationEvent>('notify-event', () => { void reload(); });
void listen<string>('hook-seen', () => { void reload(); });
void listen<Progress>('integration-progress', (e) => {
  live.set(e.payload.provider, e.payload);
  renderProviders();
});
void listen<DownloadProgress>('update-progress', (e) => {
  const { downloaded, total, phase } = e.payload;
  if (phase === 'verifying') setProgress(true, false, 0, t('update.verifying'));
  else if (total && total > 0) {
    setProgress(true, true, downloaded / total, t('update.progress', { pct: Math.floor((downloaded / total) * 100), done: bytes(downloaded), total: bytes(total) }));
  } else setProgress(true, false, 0, downloaded > 0 ? t('update.progressUnknown', { done: bytes(downloaded) }) : t('update.starting'));
});
// Background checks may find an update while this window is open; keep the state identical to the tray's.
void listen<UpdateSnapshot>('update-state', (e) => { update = e.payload; void reload(); });

(async () => {
  try {
    await reload();
    update = await invoke<UpdateSnapshot>('get_update_state');
    el('app-version').textContent = `v${update.current_version}`;
    autostartBox.checked = await invoke<boolean>('get_autostart').catch(() => false);
    await refreshIntegrations();
    renderStatus();
    renderUpdate();
  } catch (error) {
    notify(errorText(error), true);
    el('setup-view').hidden = false;
    el('setup-feedback').textContent = t('onboarding.statusError', { error: errorText(error) });
    el('setup-feedback').classList.add('problem');
  } finally {
    // Tell the backend the first render is done so it can reveal the window (no blank flash).
    void invoke('app_ready').catch(() => undefined);
  }
})();
