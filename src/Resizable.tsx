import { useCallback, useState } from "react";

const MIN_WIDTH = 180;
/** A sidebar never takes more than this share of the window (the diff keeps the rest). */
const MAX_SHARE = 0.6;

const clamp = (width: number) => Math.round(Math.max(MIN_WIDTH, Math.min(width, window.innerWidth * MAX_SHARE)));

function savedWidth(key: string): number | null {
  try {
    const value = Number(localStorage.getItem(`wispy.width.${key}`));
    return value > 0 ? value : null;
  } catch {
    return null;
  }
}

/** A sidebar's width, remembered per sidebar; `onResize` sets it, `onReset` restores the default. */
export function useSidebarWidth(key: string, fallback: number) {
  const [width, setWidth] = useState(() => savedWidth(key) ?? fallback);
  const save = useCallback(
    (value: number | null) => {
      setWidth(value ?? fallback);
      try {
        if (value === null) localStorage.removeItem(`wispy.width.${key}`);
        else localStorage.setItem(`wispy.width.${key}`, String(value));
      } catch {
        // Ignore: a remembered width is a convenience only.
      }
    },
    [key, fallback],
  );
  return { width, onResize: save, onReset: useCallback(() => save(null), [save]) };
}

interface ResizeHandleProps {
  /** The sidebar edge the handle sits on: "right" for a left sidebar, "left" for a right one. */
  edge: "left" | "right";
  onResize: (width: number) => void;
  onReset: () => void;
}

/**
 * A draggable sidebar edge (double-click resets the width). While dragging, the sidebar's width is
 * set on the element directly, so nothing re-renders until the drag ends.
 */
export function ResizeHandle({ edge, onResize, onReset }: ResizeHandleProps) {
  const start = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const panel = e.currentTarget.parentElement!;
    const startX = e.clientX;
    const startWidth = panel.getBoundingClientRect().width;
    let width = clamp(startWidth);
    const move = (ev: PointerEvent) => {
      const dx = ev.clientX - startX;
      width = clamp(startWidth + (edge === "right" ? dx : -dx));
      panel.style.width = `${width}px`;
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      document.body.classList.remove("resizing");
      onResize(width);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    document.body.classList.add("resizing");
  };
  return <div className={`resize-handle ${edge}`} onPointerDown={start} onDoubleClick={onReset} data-testid="resize-handle" />;
}
