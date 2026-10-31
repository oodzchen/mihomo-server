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

// Apply before React mounts, including while restoring a desktop login token.
applyTheme(savedTheme());
