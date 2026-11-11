import { en, type Dictionary, type Key } from './locales/en';
import { tr } from './locales/tr';

/** Add a language by creating `src/locales/<code>.ts` (a full `Dictionary`) and registering it here. */
export const LOCALES: Record<string, Dictionary> = { en, tr };
export const SUPPORTED = Object.keys(LOCALES);
export type Preference = 'system' | string;

let current = 'en';
const listeners = new Set<() => void>();

/** Maps any BCP-47-ish tag (`tr-TR`, `en_US`) to a supported code, otherwise English. */
export function matchLanguage(tag: string | undefined | null): string {
  const primary = (tag ?? '').toLowerCase().split(/[-_]/)[0];
  return SUPPORTED.includes(primary) ? primary : 'en';
}

/** Resolves the stored preference; `system` uses the language reported by Windows (via the backend). */
export function resolveLanguage(preference: Preference, systemLanguage?: string): string {
  if (preference !== 'system' && SUPPORTED.includes(preference)) return preference;
  return matchLanguage(systemLanguage ?? navigator.language);
}

export function language(): string { return current; }
export function onLanguageChange(fn: () => void) { listeners.add(fn); }

export function setLanguage(code: string) {
  current = SUPPORTED.includes(code) ? code : 'en';
  document.documentElement.lang = current;
  applyStatic();
  listeners.forEach((fn) => fn());
}

/** Looks up a key with `{name}` placeholders; falls back to English, then to the key itself. */
export function translate(code: string, key: string, params: Record<string, string | number> = {}): string {
  const dict = LOCALES[code] as Record<string, string> | undefined;
  const text = dict?.[key] ?? (en as Record<string, string>)[key] ?? key;
  return text.replace(/\{(\w+)\}/g, (m, name: string) => (name in params ? String(params[name]) : m));
}

export function t(key: Key, params: Record<string, string | number> = {}): string {
  return translate(current, key, params);
}

/** Translates a backend error. Codes look like `E_CODE` or `E_CODE:detail`; anything else is shown raw. */
export function errorText(error: unknown): string {
  const raw = String(error instanceof Error ? error.message : error);
  const match = /^(E_[A-Z_]+)(?::(.*))?$/s.exec(raw);
  if (!match) return translate(current, 'error.generic', { detail: raw });
  const key = `error.${match[1]}`;
  if (!(key in en)) return translate(current, 'error.generic', { detail: raw });
  return translate(current, key, { detail: match[2] ?? '' });
}

/** Applies `data-i18n`, `data-i18n-title` and `data-i18n-aria` attributes in the static HTML. */
export function applyStatic() {
  document.querySelectorAll<HTMLElement>('[data-i18n]').forEach((node) => {
    node.textContent = translate(current, node.dataset.i18n!, JSON.parse(node.dataset.i18nParams ?? '{}'));
  });
  document.querySelectorAll<HTMLElement>('[data-i18n-title]').forEach((node) => {
    node.title = translate(current, node.dataset.i18nTitle!);
  });
  document.querySelectorAll<HTMLElement>('[data-i18n-aria]').forEach((node) => {
    node.setAttribute('aria-label', translate(current, node.dataset.i18nAria!));
  });
}
