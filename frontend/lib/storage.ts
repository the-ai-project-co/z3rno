import type { Settings } from "./api";

const KEY = "z3rno-frontend-settings";

// Wrapped in try/catch: a private browsing window or blocked site data can
// make localStorage throw on access, not just return null.
export function loadSettings(): Settings {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) return JSON.parse(raw) as Settings;
  } catch {
    // fall through to defaults
  }
  return { serverUrl: "http://localhost:8080", token: "" };
}

export function saveSettings(settings: Settings): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(settings));
  } catch {
    // per-viewer convenience only — losing it just means re-typing settings
  }
}
