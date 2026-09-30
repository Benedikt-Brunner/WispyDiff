import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { mark } from "./perf";
import { RowStore } from "./rowStore";
import { RowKind, type FileSummary, type OpenedPr, type Row } from "./types";

export const ROW_HEIGHT = 20;
const OVERSCAN = 20;
/** Rows kept above the target when jumping, so the jump target isn't glued to the top edge. */
const JUMP_MARGIN = 3;

interface Props {
  opened: OpenedPr;
  showFiles: boolean;
  keyboardEnabled: boolean;
}

export function DiffViewer({ opened, showFiles, keyboardEnabled }: Props) {
  const { summary, viewId } = opened;
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(800);
  const [, setVersion] = useState(0);
  const firstPaint = useRef(false);

  const store = useMemo(
    () => new RowStore(viewId, summary.total_rows, () => setVersion((v) => v + 1)),
    [viewId, summary.total_rows],
  );

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = 0;
    setScrollTop(0);
    firstPaint.current = false;
    const observer = new ResizeObserver(() => setViewportHeight(el.clientHeight));
    observer.observe(el);
    setViewportHeight(el.clientHeight);
    return () => observer.disconnect();
  }, [viewId]);

  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const last = Math.min(summary.total_rows, Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + OVERSCAN);
  store.ensure(first, last);

  const rows: { index: number; row: Row | undefined }[] = [];
  for (let i = first; i < last; i++) rows.push({ index: i, row: store.get(i) });
  const visibleLoaded = rows.length > 0 && rows.every((r) => r.row !== undefined);

  useEffect(() => {
    if (visibleLoaded && !firstPaint.current) {
      firstPaint.current = true;
      requestAnimationFrame(() => mark("open:first-file-visible"));
    }
  }, [visibleLoaded]);

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (el) setScrollTop(el.scrollTop);
  }, []);

  const topRow = Math.floor(scrollTop / ROW_HEIGHT);
  const currentFile = fileAt(summary.files, topRow);

  const scrollToRow = useCallback((row: number) => {
    const el = scrollRef.current;
    if (el) el.scrollTop = Math.max(0, (row - JUMP_MARGIN) * ROW_HEIGHT);
  }, []);

  useEffect(() => {
    if (!keyboardEnabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target)) return;
      const el = scrollRef.current;
      if (!el) return;
      const anchor = Math.floor(el.scrollTop / ROW_HEIGHT) + JUMP_MARGIN;
      const target = navigationTarget(e.key, anchor, summary);
      if (target !== undefined) {
        e.preventDefault();
        scrollToRow(target);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [keyboardEnabled, summary, scrollToRow]);

  const gutterChars = Math.max(3, String(summary.max_line_number).length);

  return (
    <div className="diff-layout">
      {showFiles && <FileList files={summary.files} current={currentFile} onSelect={(f) => scrollToRow(f.first_row + JUMP_MARGIN)} />}
      <div className="diff-main">
        <div className="diff-scroll" ref={scrollRef} onScroll={onScroll} data-testid="diff-scroll">
          <div
            className="diff-canvas"
            style={
              {
                height: summary.total_rows * ROW_HEIGHT,
                "--gutter-ch": gutterChars,
                minWidth: `calc(${summary.max_line_chars + 4}ch + ${gutterChars * 2}ch + 48px)`,
              } as React.CSSProperties
            }
          >
            {rows.map(({ index, row }) => (
              <DiffRow key={index} index={index} row={row} file={row ? summary.files[row.f] : undefined} />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

const DiffRow = memo(function DiffRow({ index, row, file }: { index: number; row?: Row; file?: FileSummary }) {
  const style = { transform: `translateY(${index * ROW_HEIGHT}px)` };
  if (!row) return <div className="row row-loading" style={style} />;
  switch (row.k) {
    case RowKind.File:
      return (
        <div className="row row-file" style={style} data-file={file?.path}>
          <span className={`file-status status-${file?.status}`}>{statusLetter(file?.status)}</span>
          <span className="file-path">
            {file?.old_path && <span className="file-old-path">{file.old_path} → </span>}
            {file?.path}
          </span>
          <span className="file-counts">
            <span className="add">+{file?.additions}</span> <span className="del">−{file?.deletions}</span>
          </span>
        </div>
      );
    case RowKind.Hunk:
      return (
        <div className="row row-hunk" style={style}>
          <span className="gutter" />
          <span className="code">{row.s[0]?.[1]}</span>
        </div>
      );
    case RowKind.Notice:
      return (
        <div className="row row-notice" style={style}>
          <span className="gutter" />
          <span className="code">{row.s[0]?.[1]}</span>
        </div>
      );
    default: {
      const kind = row.k === RowKind.Added ? "add" : row.k === RowKind.Deleted ? "del" : "ctx";
      return (
        <div className={`row row-${kind}`} style={style}>
          <span className="gutter">
            <span className="ln">{row.o ?? ""}</span>
            <span className="ln">{row.n ?? ""}</span>
            <span className="sign">{kind === "add" ? "+" : kind === "del" ? "−" : ""}</span>
          </span>
          <span className="code">
            {row.s.map(([cls, text], i) =>
              cls === 0 ? text : (
                <span key={i} className={`t${cls}`}>
                  {text}
                </span>
              ),
            )}
          </span>
        </div>
      );
    }
  }
});

function FileList({ files, current, onSelect }: { files: FileSummary[]; current: number; onSelect: (f: FileSummary) => void }) {
  const activeRef = useRef<HTMLLIElement>(null);
  useEffect(() => activeRef.current?.scrollIntoView({ block: "nearest" }), [current]);
  return (
    <nav className="file-list">
      <ul>
        {files.map((f, i) => (
          <li
            key={f.path + i}
            ref={i === current ? activeRef : undefined}
            className={i === current ? "active" : undefined}
            onClick={() => onSelect(f)}
            title={f.path}
          >
            <span className={`file-status status-${f.status}`}>{statusLetter(f.status)}</span>
            <span className="file-name">
              <span className="file-dir">{dirname(f.path)}</span>
              {basename(f.path)}
            </span>
            <span className="file-counts">
              {f.additions > 0 && <span className="add">+{f.additions}</span>}
              {f.deletions > 0 && <span className="del">−{f.deletions}</span>}
            </span>
          </li>
        ))}
      </ul>
    </nav>
  );
}

/** `j`/`k` = next/previous hunk, `n`/`p` = next/previous file. */
export function navigationTarget(key: string, anchor: number, summary: OpenedPr["summary"]): number | undefined {
  const fileStarts = summary.files.map((f) => f.first_row);
  switch (key) {
    case "j":
      return summary.hunk_rows.find((r) => r > anchor);
    case "k":
      return findLast(summary.hunk_rows, (r) => r < anchor);
    case "n":
      return fileStarts.find((r) => r > anchor);
    case "p":
      return findLast(fileStarts, (r) => r < anchor);
    default:
      return undefined;
  }
}

function fileAt(files: FileSummary[], row: number) {
  let lo = 0;
  let hi = files.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (files[mid].first_row <= row) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

function findLast(values: number[], pred: (v: number) => boolean) {
  for (let i = values.length - 1; i >= 0; i--) if (pred(values[i])) return values[i];
  return undefined;
}

const statusLetter = (s?: string) => ({ added: "A", deleted: "D", renamed: "R", copied: "C" })[s ?? ""] ?? "M";
const dirname = (p: string) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/") + 1) : "");
const basename = (p: string) => p.slice(p.lastIndexOf("/") + 1);
const isTyping = (t: EventTarget | null) =>
  t instanceof HTMLElement && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName));
