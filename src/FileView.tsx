import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ROW_HEIGHT } from "./layout";
import type { Seg } from "./types";

interface Props {
  path: string;
  line: number;
  /** Last line to highlight (`line` if just one). */
  end?: number;
  lines: Seg[][] | null;
  error: string | null;
  onClose: () => void;
}

/** A whole file from the head, read-only (for search hits outside the diff). */
export function FileView({ path, line, end = line, lines, error, onClose }: Props) {
  const scroll = useRef<HTMLDivElement>(null);
  const [top, setTop] = useState(0);
  const [height, setHeight] = useState(600);

  useLayoutEffect(() => {
    const el = scroll.current;
    if (!el || !lines) return;
    setHeight(el.clientHeight);
    el.scrollTop = Math.max(0, (line - 6) * ROW_HEIGHT);
    setTop(el.scrollTop);
  }, [lines, line]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const first = Math.max(0, Math.floor(top / ROW_HEIGHT) - 10);
  const last = Math.min(lines?.length ?? 0, Math.ceil((top + height) / ROW_HEIGHT) + 10);
  const rows = [];
  for (let i = first; i < last; i++) {
    rows.push(
      <div key={i} className={`row row-ctx${i + 1 >= line && i + 1 <= end ? " flash-static" : ""}`} style={{ transform: `translateY(${i * ROW_HEIGHT}px)` }}>
        <span className="gutter">
          <span className="ln">{i + 1}</span>
        </span>
        <span className="code">
          {lines![i].map(([cls, text], k) => (cls === 0 ? text : <span key={k} className={`t${cls}`}>{text}</span>))}
        </span>
      </div>,
    );
  }

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="file-view" onMouseDown={(e) => e.stopPropagation()} data-testid="file-view">
        <div className="sheet-head">
          <span className="sheet-title">{path}</span>
          <span className="file-view-note">read-only · not part of this diff</span>
          <button className="link" onClick={onClose}>
            close
          </button>
        </div>
        {error && <div className="sheet-error">{error}</div>}
        <div className="file-view-scroll" ref={scroll} onScroll={(e) => setTop(e.currentTarget.scrollTop)}>
          <div className="diff-canvas" style={{ height: (lines?.length ?? 0) * ROW_HEIGHT, "--gutter-ch": 3 } as React.CSSProperties}>
            {rows}
          </div>
        </div>
      </div>
    </div>
  );
}
