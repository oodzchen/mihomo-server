// Cache the instance preference so login and desktop reloads use the same theme.
export type Theme = "system" | "light" | "dark";
const STORAGE_KEY = "mihomo-server-theme";

export function resolveTheme(value?: string | null): Theme {
  return value === "light" || value === "dark" ? value : "system";
}

export function savedTheme(): Theme {
  try { return resolveTheme(localStorage.getItem(STORAGE_KEY)); }
  catch { return "system"; }
}

export function applyTheme(theme: Theme) {
  document.documentElement.dataset.theme = theme;
  try { localStorage.setItem(STORAGE_KEY, theme); }
  catch { /* Theme still works when browser storage is unavailable. */ }
}

// Keep the preference (data-theme) separate from the resolved system appearance.
// WebKitGTK can leave prefers-color-scheme stale after a desktop theme change;
// the desktop host supplies a native snapshot/event when available.
declare global {
  interface Window {
    __MIHOMO_SYSTEM_THEME__?: { readonly current?: "light" | "dark" };
  }
}
const systemColors = window.matchMedia("(prefers-color-scheme: dark)");
function syncSystemTheme() {
  const native = window.__MIHOMO_SYSTEM_THEME__?.current;
  document.documentElement.dataset.systemTheme = native === "light" || native === "dark"
    ? native : systemColors.matches ? "dark" : "light";
}
systemColors.addEventListener("change", syncSystemTheme);
window.addEventListener("mihomo-system-theme-changed", syncSystemTheme);
// Recheck after backgrounding, BFCache restoration, or a suspended WebView.
window.addEventListener("focus", syncSystemTheme);
window.addEventListener("pageshow", syncSystemTheme);
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") syncSystemTheme();
});

// Apply before React mounts, including while restoring a desktop login token.
syncSystemTheme();
applyTheme(savedTheme());
