import { getCurrentWindow } from "@tauri-apps/api/window";
import { loadPref, savePref } from "./prefs";
import "./themes.css";

export const THEMES = [
  { id: "system", name: "System (light / dark)" },
  { id: "light", name: "Light" },
  { id: "dark", name: "Dark" },
  { id: "solarized-light", name: "Solarized Light" },
  { id: "solarized-dark", name: "Solarized Dark" },
  { id: "nord", name: "Nord" },
  { id: "dracula", name: "Dracula" },
  { id: "sepia", name: "Sepia" },
] as const;

export type ThemeId = (typeof THEMES)[number]["id"];

const DARK: readonly ThemeId[] = ["dark", "solarized-dark", "nord", "dracula"];
const systemDark = window.matchMedia("(prefers-color-scheme: dark)");
let current: ThemeId = "system";

export const savedTheme = (): ThemeId => loadPref("theme", THEMES.map((t) => t.id), "system");

/** Shows `id` (without remembering it; `saveTheme` does that). "system" follows macOS. */
export function applyTheme(id: ThemeId) {
  current = id;
  const resolved = id === "system" ? (systemDark.matches ? "dark" : "light") : id;
  document.documentElement.dataset.theme = resolved;
  // Match the native window (traffic lights, scrollbars) to the theme.
  const scheme = id === "system" ? null : DARK.includes(resolved) ? "dark" : "light";
  getCurrentWindow()
    .setTheme(scheme)
    .catch(() => undefined);
}

export function saveTheme(id: ThemeId) {
  savePref("theme", id);
  applyTheme(id);
}

systemDark.addEventListener("change", () => current === "system" && applyTheme("system"));
