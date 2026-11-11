// Offline compile-only declarations. The real Tauri types are used in normal npm builds.
declare module '@tauri-apps/api/core' {
  export function invoke<T = unknown>(name: string, args?: Record<string, unknown>): Promise<T>;
}
declare module '@tauri-apps/api/event' {
  export function listen<T>(name: string, callback: (event: { payload: T }) => void | Promise<void>): Promise<() => void>;
}
declare module '@tauri-apps/plugin-dialog' {
  export function open(options: unknown): Promise<string | string[] | null>;
}
