import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isMac, mod } from "./platform";
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

/** The key `mod` checks: Meta on macOS, Control elsewhere. */
const MOD_KEY = isMac ? "Meta" : "Control";

/**
 * ⌘+ / ⌘- / ⌘0 and ⌘+wheel zoom anywhere in the app; `onZoom` gets each new level.
 *
 * The wheel listener that can cancel scrolling is only attached while ⌘ is held: as long as one is
 * attached, WebKit hands every wheel event to the page's main thread and waits for it before
 * scrolling, so scrolling stutters whenever that thread is busy drawing rows. A passive listener
 * catches ⌘ already being held when the window got focus: it zooms on that first wheel step (which
 * may also scroll a little, as it can't be cancelled) and attaches the cancelling one for the rest.
 */
export function installZoomShortcuts(onZoom: (level: number) => void) {
  let wheelDelta = 0;
  let zooming = false;
  const zoomBy = (e: WheelEvent) => {
    wheelDelta += e.deltaMode === WheelEvent.DOM_DELTA_PIXEL ? e.deltaY : e.deltaY * WHEEL_STEP;
    if (Math.abs(wheelDelta) < WHEEL_STEP) return;
    const step = wheelDelta < 0 ? 1 : -1;
    wheelDelta = 0;
    onZoom(zoom(step));
  };
  const onWheel = (e: WheelEvent) => {
    if (!mod(e)) return;
    e.preventDefault();
    zoomBy(e);
  };
  const startZooming = () => {
    if (zooming) return;
    zooming = true;
    wheelDelta = 0;
    window.addEventListener("wheel", onWheel, { passive: false });
  };
  const stopZooming = () => {
    if (!zooming) return;
    zooming = false;
    window.removeEventListener("wheel", onWheel);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === MOD_KEY) startZooming();
    const step = stepForKey(e);
    if (step === null) return;
    e.preventDefault();
    onZoom(zoom(step));
  };
  const onKeyUp = (e: KeyboardEvent) => {
    if (e.key === MOD_KEY || !mod(e)) stopZooming();
  };
  const onPassiveWheel = (e: WheelEvent) => {
    if (!mod(e)) return stopZooming();
    if (zooming) return; // `onWheel` has it.
    // A listener added now doesn't see this event (DOM dispatch), so this one zooms for it.
    startZooming();
    zoomBy(e);
  };
  window.addEventListener("keydown", onKey);
  window.addEventListener("keyup", onKeyUp);
  window.addEventListener("blur", stopZooming);
  window.addEventListener("wheel", onPassiveWheel, { passive: true });
  return () => {
    stopZooming();
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("keyup", onKeyUp);
    window.removeEventListener("blur", stopZooming);
    window.removeEventListener("wheel", onPassiveWheel);
  };
}
