import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Layout, type Mode, type Segment } from "./layout";
import { mark } from "./perf";
import { RowStore } from "./rowStore";
import { prColor, RowKind, type DiffSummary, type FileSummary, type PullRequest, type Row, type Seg, type SplitRow } from "./types";

const OVERSCAN = 20;
/** Rows kept above the target when jumping, so the jump target isn't glued to the top edge. */
const JUMP_MARGIN = 3;

export type BaseMode = "unified" | "split";

interface Props {
  viewId: string;
  summary: DiffSummary;
  /** The whole stack, bottom to top (row attributions index into it). */
  prs: PullRequest[];
  /** Color lines by PR (hidden for a lone PR, where every line has the same one). */
  showAttribution: boolean;
  /** More than one PR selected: tag runs of lines with their PR number. */
  multiPr: boolean;
  showFiles: boolean;
  keyboardEnabled: boolean;
  /** Global default for files without a per-file choice. */
  defaultMode: BaseMode;
  onDefaultModeChange: (mode: BaseMode) => void;
  /** Files the user asked to collapse in this repo (ignore patterns). */
  isIgnored: (path: string) => boolean;
}

export function DiffViewer(props: Props) {
  const { viewId, summary, prs, showAttribution, multiPr, showFiles, keyboardEnabled, defaultMode, onDefaultModeChange, isIgnored } = props;
  const scrollRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(800);
  const [, setVersion] = useState(0);

  // Per-file choices, by path so they survive range switches.
  const [overrides, setOverrides] = useState<Map<string, BaseMode>>(new Map());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  /** Which perf mark to set once the visible rows are loaded ("view" / "layout"). */
  const pendingMark = useRef<string | null>("view");
  /** Where the viewport's top is, as (file path, offset in file, pixels into that row). */
  const anchor = useRef<{ path: string; offset: number; mode: Mode; delta: number } | null>(null);

  const store = useMemo(() => new RowStore(viewId, () => setVersion((v) => v + 1)), [viewId]);

  const modeOf = useCallback(
    (index: number): Mode => {
      const file = summary.files[index];
      const hidden = collapsed.has(file.path) || ((file.noise !== null || isIgnored(file.path)) && !expanded.has(file.path));
      if (hidden) return "collapsed";
      const wanted = overrides.get(file.path) ?? defaultMode;
      return wanted === "split" && file.split_rows > 0 ? "split" : "unified";
    },
    [summary, overrides, expanded, collapsed, defaultMode, isIgnored],
  );
  const layout = useMemo(() => new Layout(summary, modeOf), [summary, modeOf]);

  // Keep the reader's place when the view or the layout changes.
  const previousView = useRef<string | null>(null);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const place = anchor.current;
    const index = place ? summary.files.findIndex((f) => f.path === place.path) : -1;
    let top = 0;
    if (index >= 0 && place) {
      const segment = layout.segments[index];
      const sameView = previousView.current === viewId;
      const keepOffset = sameView && segment.mode === place.mode;
      top = layout.rowY(segment.start + (keepOffset ? Math.min(place.offset, segment.rows - 1) : 0)) + (keepOffset ? place.delta : 0);
    }
    if (previousView.current !== viewId) pendingMark.current = "view";
    previousView.current = viewId;
    el.scrollTop = top;
    setScrollTop(el.scrollTop);
  }, [layout, viewId, summary]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () => {
      setViewportHeight(el.clientHeight);
      // Side-by-side rows always span exactly the viewport (see .row-split).
      canvasRef.current?.style.setProperty("--vw", `${el.clientWidth}px`);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    measure();
    return () => observer.disconnect();
  }, []);

  const first = Math.max(0, layout.rowAt(scrollTop) - OVERSCAN);
  const last = Math.min(layout.totalRows, layout.rowAt(scrollTop + viewportHeight) + 1 + OVERSCAN);

  // Load what's visible, per file segment.
  for (let row = first; row < last; ) {
    const segment = layout.segmentAt(row);
    const end = Math.min(last, segment.start + segment.rows);
    if (segment.mode !== "collapsed") store.ensure(segment.file, segment.mode, row - segment.start, end - segment.start);
    row = end;
  }

  const items: React.ReactNode[] = [];
  let visibleLoaded = true;
  for (let row = first; row < last; row++) {
    const segment = layout.segmentAt(row);
    const offset = row - segment.start;
    const file = summary.files[segment.file];
    const y = layout.rowY(row);
    const key = `${segment.file}:${segment.mode}:${offset}`;
    if (offset === 0) {
      items.push(<FileHeader key={key} y={y} file={file} mode={segment.mode} />);
    } else if (segment.mode === "collapsed") {
      items.push(<CollapsedNotice key={key} y={y} file={file} onExpand={() => setExpanded((s) => new Set(s).add(file.path))} />);
    } else if (segment.mode === "split") {
      const split = store.split(segment.file, offset);
      if (!split) visibleLoaded = false;
      items.push(<SplitRowView key={key} y={y} row={split} showAttribution={showAttribution} />);
    } else {
      const unified = store.unified(segment.file, offset);
      if (!unified) visibleLoaded = false;
      const tag = multiPr && unified !== undefined && startsRun(unified, store.unified(segment.file, offset - 1));
      items.push(<DiffRow key={key} y={y} row={unified} prs={prs} showAttribution={showAttribution} tag={tag} />);
    }
  }

  useEffect(() => {
    if (visibleLoaded && pendingMark.current) {
      const name = pendingMark.current;
      pendingMark.current = null;
      requestAnimationFrame(() => mark(`${name}:first-visible`));
    }
  });

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    // Horizontal scrolling moves side-by-side code within its halves (no re-render needed).
    canvasRef.current?.style.setProperty("--split-x", `${el.scrollLeft}px`);
    const row = layout.rowAt(el.scrollTop);
    const segment = layout.segmentAt(row);
    anchor.current = {
      path: summary.files[segment.file].path,
      offset: row - segment.start,
      mode: segment.mode,
      delta: el.scrollTop - layout.rowY(row),
    };
  }, [layout, summary]);

  const topRow = layout.rowAt(scrollTop);
  // The file whose rows are at the top (one row in, so a file header scrolled to the top counts).
  const current = layout.segmentAt(Math.min(layout.totalRows - 1, topRow + 1));

  const scrollToRow = useCallback(
    (row: number) => {
      const el = scrollRef.current;
      if (el) el.scrollTop = Math.max(0, layout.rowY(Math.max(0, row - JUMP_MARGIN)));
    },
    [layout],
  );

  // Record the anchor before any layout change triggered from here, then flag the perf mark.
  const changeLayout = useCallback(
    (apply: () => void) => {
      onScroll();
      pendingMark.current = "layout";
      mark("toggle:start");
      apply();
    },
    [onScroll],
  );

  const toggleFileMode = useCallback(
    (segment: Segment) => {
      const file = summary.files[segment.file];
      changeLayout(() => {
        if (segment.mode === "collapsed") {
          setExpanded((s) => new Set(s).add(file.path));
          setCollapsed((s) => withOut(s, file.path));
          return;
        }
        const next: BaseMode = segment.mode === "split" ? "unified" : "split";
        setOverrides((m) => new Map(m).set(file.path, next));
      });
    },
    [summary, changeLayout],
  );

  const toggleCollapsed = useCallback(
    (segment: Segment) => {
      const path = summary.files[segment.file].path;
      changeLayout(() => {
        if (segment.mode === "collapsed") {
          setExpanded((s) => new Set(s).add(path));
          setCollapsed((s) => withOut(s, path));
        } else {
          setCollapsed((s) => new Set(s).add(path));
          setExpanded((s) => withOut(s, path));
        }
      });
    },
    [summary, changeLayout],
  );

  useEffect(() => {
    if (!keyboardEnabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target)) return;
      const el = scrollRef.current;
      if (!el) return;
      const anchorRow = layout.rowAt(el.scrollTop) + JUMP_MARGIN;
      const here = layout.segmentAt(Math.min(layout.totalRows - 1, layout.rowAt(el.scrollTop) + 1));
      const starts = layout.segments.map((s) => s.start);
      const changes = layout.changeRows(summary);
      let target: number | undefined;
      switch (e.key) {
        case "j":
          target = changes.find((r) => r > anchorRow);
          break;
        case "k":
          target = findLast(changes, (r) => r < anchorRow);
          break;
        case "n":
          target = starts.find((r) => r > anchorRow);
          break;
        case "p":
          target = findLast(starts, (r) => r < anchorRow);
          break;
        case "s":
          toggleFileMode(here);
          break;
        case "S":
          changeLayout(() => {
            setOverrides(new Map());
            onDefaultModeChange(defaultMode === "split" ? "unified" : "split");
          });
          break;
        case "e":
          toggleCollapsed(here);
          break;
        default:
          return;
      }
      e.preventDefault();
      if (target !== undefined) scrollToRow(target);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [keyboardEnabled, layout, summary, scrollToRow, toggleFileMode, toggleCollapsed, changeLayout, defaultMode, onDefaultModeChange]);

  const gutterChars = Math.max(3, String(summary.max_line_number).length);

  return (
    <div className="diff-layout">
      {showFiles && (
        <FileList
          files={summary.files}
          segments={layout.segments}
          current={current.file}
          showPrs={multiPr}
          onSelect={(index) => scrollToRow(layout.segments[index].start + JUMP_MARGIN)}
        />
      )}
      <div className="diff-main">
        <div className="diff-scroll" ref={scrollRef} onScroll={onScroll} data-testid="diff-scroll">
          <div
            ref={canvasRef}
            className="diff-canvas"
            style={
              {
                height: layout.height,
                "--gutter-ch": gutterChars,
                minWidth: `calc(${summary.max_line_chars + 4}ch + ${gutterChars * 2}ch + 48px)`,
              } as React.CSSProperties
            }
          >
            {items}
          </div>
        </div>
      </div>
    </div>
  );
}

const at = (y: number): React.CSSProperties => ({ transform: `translateY(${y}px)` });

const FileHeader = memo(function FileHeader({ y, file, mode }: { y: number; file: FileSummary; mode: Mode }) {
  return (
    <div className="row row-file" style={at(y)} data-file={file.path} data-mode={mode}>
      <span className={`file-status status-${file.status}`}>{statusLetter(file.status)}</span>
      <span className="file-path">
        {file.old_path && <span className="file-old-path">{file.old_path} → </span>}
        {file.path}
      </span>
      <span className="file-counts">
        <span className="add">+{file.additions}</span> <span className="del">−{file.deletions}</span>
      </span>
      {file.noise && <span className="file-badge">{file.noise}</span>}
      <span className="file-mode">{mode === "split" ? "side by side" : mode === "collapsed" ? "collapsed" : ""}</span>
    </div>
  );
});

function CollapsedNotice({ y, file, onExpand }: { y: number; file: FileSummary; onExpand: () => void }) {
  const why = file.noise ?? "collapsed";
  return (
    <div className="row row-collapsed" style={at(y)} onClick={onExpand}>
      <span className="gutter" />
      <span className="code">
        {why} · +{file.additions} −{file.deletions} · click or press e to expand
      </span>
    </div>
  );
}

function Segments({ segs }: { segs: Seg[] }) {
  return (
    <>
      {segs.map(([cls, text], i) =>
        cls === 0 ? (
          text
        ) : (
          <span key={i} className={`t${cls}`}>
            {text}
          </span>
        ),
      )}
    </>
  );
}

interface RowProps {
  y: number;
  row?: Row;
  prs: PullRequest[];
  showAttribution: boolean;
  /** Show the PR tag: this row starts a run of lines from one PR. */
  tag: boolean;
}

const DiffRow = memo(function DiffRow({ y, row, prs, showAttribution, tag }: RowProps) {
  const style = at(y);
  if (!row) return <div className="row row-loading" style={style} />;
  switch (row.k) {
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
    case RowKind.File:
      return <div className="row" style={style} />;
    default: {
      const kind = row.k === RowKind.Added ? "add" : row.k === RowKind.Deleted ? "del" : "ctx";
      const attributed = showAttribution && row.a !== null;
      if (attributed) (style as Record<string, string>)["--pr"] = prColor(row.a!);
      return (
        <div className={`row row-${kind}${attributed ? " attributed" : ""}`} style={style} title={attributed ? lineHistory(row, prs) : undefined}>
          <span className="gutter">
            <span className="ln">{row.o ?? ""}</span>
            <span className="ln">{row.n ?? ""}</span>
            <span className="sign">{kind === "add" ? "+" : kind === "del" ? "−" : ""}</span>
          </span>
          <span className="code">
            <Segments segs={row.s} />
          </span>
          {tag && row.a !== null && <span className="pr-tag">#{prs[row.a].number}</span>}
        </div>
      );
    }
  }
});

const sideClass = (kind: number) =>
  kind === RowKind.Added ? "add" : kind === RowKind.Deleted ? "del" : kind === RowKind.Filler ? "filler" : "ctx";

const SplitRowView = memo(function SplitRowView({ y, row, showAttribution }: { y: number; row?: SplitRow; showAttribution: boolean }) {
  if (!row) return <div className="row row-loading" style={at(y)} />;
  const side = (no: number | null, kind: number, segs: Seg[], pr: number | null, left: boolean) => {
    const cls = sideClass(kind);
    const attributed = showAttribution && pr !== null;
    return (
      <span
        className={`half half-${cls}${left ? " half-left" : ""}${attributed ? " attributed" : ""}`}
        style={attributed ? ({ "--pr": prColor(pr!) } as React.CSSProperties) : undefined}
      >
        <span className="gutter half-gutter">
          <span className="ln">{no ?? ""}</span>
        </span>
        <span className="code">
          <Segments segs={segs} />
        </span>
      </span>
    );
  };
  return (
    <div className="row row-split" style={{ transform: `translate(var(--split-x, 0px), ${y}px)` }}>
      {side(row.o, row.ok, row.os, row.oa, true)}
      {side(row.n, row.nk, row.ns, row.na, false)}
    </div>
  );
});

interface FileListProps {
  files: FileSummary[];
  segments: Segment[];
  current: number;
  showPrs: boolean;
  onSelect: (index: number) => void;
}

function FileList({ files, segments, current, showPrs, onSelect }: FileListProps) {
  const activeRef = useRef<HTMLLIElement>(null);
  useEffect(() => activeRef.current?.scrollIntoView({ block: "nearest" }), [current]);
  return (
    <nav className="file-list">
      <ul>
        {files.map((f, i) => (
          <li
            key={f.path + i}
            ref={i === current ? activeRef : undefined}
            className={[i === current && "active", segments[i]?.mode === "collapsed" && "muted"].filter(Boolean).join(" ")}
            onClick={() => onSelect(i)}
            title={f.path}
          >
            <span className={`file-status status-${f.status}`}>{statusLetter(f.status)}</span>
            <span className="file-name">
              <span className="file-dir">{dirname(f.path)}</span>
              {basename(f.path)}
            </span>
            {showPrs && (
              <span className="file-prs">
                {f.prs.map((p) => (
                  <span key={p} className="file-pr-dot" style={{ background: prColor(p) }} />
                ))}
              </span>
            )}
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

/** A run of changed lines from one PR starts here (the previous row is another PR or not a change). */
function startsRun(row: Row, previous: Row | undefined) {
  if (row.a === null) return false;
  const changed = (r: Row) => r.k === RowKind.Added || r.k === RowKind.Deleted;
  return !previous || !changed(previous) || previous.a !== row.a;
}

function lineHistory(row: Row, prs: PullRequest[]) {
  const n = (i: number) => `#${prs[i]?.number}`;
  if (row.k === RowKind.Deleted) return `Deleted in ${n(row.a!)}`;
  if (row.h.length > 1) return `Added in ${n(row.h[0])} · modified in ${row.h.slice(1).map(n).join(", ")}`;
  return `Added in ${n(row.a!)}`;
}

function findLast(values: number[], pred: (v: number) => boolean) {
  for (let i = values.length - 1; i >= 0; i--) if (pred(values[i])) return values[i];
  return undefined;
}

function withOut<T>(set: Set<T>, value: T) {
  const next = new Set(set);
  next.delete(value);
  return next;
}

const statusLetter = (s?: string) => ({ added: "A", deleted: "D", renamed: "R", copied: "C" })[s ?? ""] ?? "M";
const dirname = (p: string) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/") + 1) : "");
const basename = (p: string) => p.slice(p.lastIndexOf("/") + 1);
const isTyping = (t: EventTarget | null) =>
  t instanceof HTMLElement && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName));
