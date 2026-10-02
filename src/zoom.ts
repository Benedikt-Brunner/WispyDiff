import { getCurrentWebview } from "@tauri-apps/api/webview";
import { mod } from "./platform";
import { loadPref, savePref } from "./prefs";

/** Zoom steps, like a browser's. The whole webview scales, so the layout's CSS pixels stay exact. */
const LEVELS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2] as const;

let level = Number(loadPref("zoom", LEVELS.map(String), "1"));

function apply() {
  getCurrentWebview()
    .setZoom(level)
    .catch((e) => console.error("zoom", e));
}

/** Restores the remembered zoom (call once at startup). */
export function restoreZoom() {
  if (level !== 1) apply();
}

/** Zooms one step in (`1`) or out (`-1`), or back to 100% (`0`); returns the new level. */
function zoom(step: -1 | 0 | 1): number {
  const index = LEVELS.indexOf(level as (typeof LEVELS)[number]);
  level = step === 0 ? 1 : LEVELS[Math.max(0, Math.min(LEVELS.length - 1, index + step))];
  savePref("zoom", String(level));
  apply();
  return level;
}

/** The zoom step for a ⌘+ / ⌘- / ⌘0 key press, or null. */
function stepForKey(e: KeyboardEvent): -1 | 0 | 1 | null {
  if (!mod(e) || e.altKey) return null;
  if (e.key === "+" || e.key === "=") return 1;
  if (e.key === "-" || e.key === "_") return -1;
  if (e.key === "0") return 0;
  return null;
}

/** Wheel distance (px) per zoom step, so a trackpad doesn't race through the levels. */
const WHEEL_STEP = 50;

/** ⌘+ / ⌘- / ⌘0 and ⌘+wheel zoom anywhere in the app; `onZoom` gets each new level. */
export function installZoomShortcuts(onZoom: (level: number) => void) {
  let wheelDelta = 0;
  const onKey = (e: KeyboardEvent) => {
    const step = stepForKey(e);
    if (step === null) return;
    e.preventDefault();
    onZoom(zoom(step));
  };
  const onWheel = (e: WheelEvent) => {
    if (!mod(e)) return;
    e.preventDefault();
    wheelDelta += e.deltaMode === WheelEvent.DOM_DELTA_PIXEL ? e.deltaY : e.deltaY * WHEEL_STEP;
    if (Math.abs(wheelDelta) < WHEEL_STEP) return;
    const step = wheelDelta < 0 ? 1 : -1;
    wheelDelta = 0;
    onZoom(zoom(step));
  };
  window.addEventListener("keydown", onKey);
  window.addEventListener("wheel", onWheel, { passive: false });
  return () => {
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("wheel", onWheel);
  };
}
