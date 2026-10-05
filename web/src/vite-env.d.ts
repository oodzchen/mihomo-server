/// <reference types="vite/client" />

interface Window {
  readonly __MIHOMO_DESKTOP_VERSION__?: string;
  /** Tauri IPC inside the desktop client; only its start-at-login commands are granted. */
  readonly __TAURI_INTERNALS__?: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> };
}
