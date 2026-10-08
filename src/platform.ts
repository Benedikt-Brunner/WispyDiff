/** Shortcuts use ⌘ on macOS and Ctrl elsewhere (Super is the desktop's on Linux). */
export const isMac = /Mac/.test(navigator.platform);

/** Whether the platform's shortcut modifier (⌘ / Ctrl) is held. */
export const mod = (e: { metaKey: boolean; ctrlKey: boolean }) => (isMac ? e.metaKey : e.ctrlKey);

/** A shortcut label: `keyLabel("K")` is "⌘K" on macOS, "Ctrl+K" elsewhere. */
export const keyLabel = (key: string) => (isMac ? `⌘${key}` : `Ctrl+${key}`);

/** "⌘-click" / "Ctrl-click". */
export const clickLabel = isMac ? "⌘-click" : "Ctrl-click";

/** Enter submits a text box; Shift+Enter (and Enter while an IME is composing) inserts a new line. */
export const isSubmitKey = (e: { key: string; shiftKey: boolean; nativeEvent: KeyboardEvent }) =>
  e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing;
