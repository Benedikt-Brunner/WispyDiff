import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { acceptsLineComment, createDraft, deleteDraft, locateAnchors, updateDraft } from "./api";
import {
  anchorKey,
  fileIndexFor,
  lineLabel,
  pathInPr,
  type Anchor,
  type Location,
  type NewDraft,
  type PrThreads,
  type ReviewThread,
  type ShownDraft,
} from "./comments";
import { Composer, DraftCard, InsertBox, ThreadCard } from "./Inserts";
import { Layout, type Insert, type Mode, type Segment } from "./layout";
import { mark } from "./perf";
import { RowStore } from "./rowStore";
import { prColor, RowKind, type DiffSummary, type FileSummary, type PullRequest, type Row, type Seg, type SplitRow } from "./types";

const OVERSCAN = 20;
/** Rows kept above the target when jumping, so the jump target isn't glued to the top edge. */
const JUMP_MARGIN = 3;

export type BaseMode = "unified" | "split";

interface Props {
  stackId: string;
  /** Stack indices of the selected range. */
  lo: number;
  hi: number;
  drafts: ShownDraft[];
  threads: PrThreads[];
  /** Call after creating, editing or deleting drafts. */
  onDraftsChanged: () => void;
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
  /** Viewed marks, by file content key. */
  isViewed: (key: string) => boolean;
  onToggleViewed: (key: string, viewed: boolean) => void;
}

export function DiffViewer(props: Props) {
  const { viewId, summary, prs, showAttribution, multiPr, showFiles, keyboardEnabled, defaultMode, onDefaultModeChange, isIgnored } = props;
  const { stackId, lo, hi, drafts, threads, onDraftsChanged, isViewed, onToggleViewed } = props;
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
      const hidden =
        collapsed.has(file.path) || ((file.noise !== null || isIgnored(file.path) || isViewed(file.content_key)) && !expanded.has(file.path));
      if (hidden) return "collapsed";
      const wanted = overrides.get(file.path) ?? defaultMode;
      return wanted === "split" && file.split_rows > 0 ? "split" : "unified";
    },
    [summary, overrides, expanded, collapsed, defaultMode, isIgnored, isViewed],
  );
  const baseLayout = useMemo(() => new Layout(summary, modeOf), [summary, modeOf]);

  // ---------- comments: what goes where ----------
  const [composer, setComposer] = useState<ComposerState | null>(null);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [heights, setHeights] = useState<Map<string, number>>(new Map());
  const [locations, setLocations] = useState<Map<string, Location | null>>(new Map());
  const hovered = useRef<Target | null>(null);
  const dragging = useRef<Target | null>(null);

  const saveDraft = useCallback(
    async (draft: NewDraft) => {
      await createDraft(stackId, draft);
      onDraftsChanged();
    },
    [stackId, onDraftsChanged],
  );
  const removeDraft = useCallback(
    async (id: string) => {
      await deleteDraft(id);
      onDraftsChanged();
    },
    [onDraftsChanged],
  );
  const editDraft = useCallback(
    async (id: string, body: string) => {
      await updateDraft(id, { body });
      onDraftsChanged();
    },
    [onDraftsChanged],
  );

  const items = useMemo(
    () => commentItems({ drafts, threads, composer, lo, hi, prs, saveDraft, removeDraft, editDraft, closeComposer: () => setComposer(null) }),
    [drafts, threads, composer, lo, hi, prs, saveDraft, removeDraft, editDraft],
  );
  const anchorList = useMemo(() => {
    const unique = new Map<string, Anchor>();
    for (const item of items) if (item.anchor) unique.set(anchorKey(item.anchor), item.anchor);
    return [...unique.values()];
  }, [items]);
  const anchorsSignature = anchorList.map(anchorKey).join(",");
  useEffect(() => {
    if (anchorList.length === 0) return;
    let cancelled = false;
    locateAnchors(viewId, anchorList)
      .then((found) => {
        if (cancelled) return;
        setLocations(new Map(anchorList.map((a, i) => [anchorKey(a), found[i]])));
      })
      .catch((e) => console.error("locate failed", e));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewId, anchorsSignature]);

  const placed = useMemo(() => {
    const out: (Insert & { item: CommentItem })[] = [];
    for (const item of items) {
      let file: number;
      let offset: number | null = null;
      if (item.anchor) {
        const location = locations.get(anchorKey(item.anchor));
        if (!location) continue;
        file = location.file;
        const segment = baseLayout.segments[file];
        offset = segment.mode === "split" ? location.split : segment.mode === "unified" ? location.unified : 1;
      } else {
        file = fileIndexFor(summary.files, item.path);
        if (file < 0) continue;
      }
      const segment = baseLayout.segments[file];
      const after = segment.start + (offset ?? (segment.mode === "collapsed" ? 1 : 0));
      out.push({ key: item.key, after, height: heights.get(item.key) ?? 72, item });
    }
    return out;
  }, [items, locations, baseLayout, summary, heights]);

  const layout = useMemo(() => new Layout(summary, modeOf, placed), [summary, modeOf, placed]);
  const placedItems = useMemo(() => new Map(placed.map((p) => [p.key, p.item])), [placed]);
  const reportHeight = useRef(new Map<string, (h: number) => void>());
  const heightReporter = useCallback((key: string) => {
    let report = reportHeight.current.get(key);
    if (!report) {
      report = (h: number) =>
        setHeights((current) => (Math.abs((current.get(key) ?? -1) - h) < 1 ? current : new Map(current).set(key, h)));
      reportHeight.current.set(key, report);
    }
    return report;
  }, []);

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

  /** The comment target for a row side (null for fillers and headers). */
  const targetAt = useCallback(
    (file: number, mode: "unified" | "split", offset: number, side: "old" | "new" | null): Target | null => {
      const summaryFile = summary.files[file];
      let anchor: Anchor | null = null;
      if (mode === "unified") {
        const row = store.unified(file, offset);
        if (row) anchor = unifiedAnchor(row, summaryFile, hi);
      } else {
        const row = store.split(file, offset);
        if (row) anchor = splitAnchor(row, summaryFile, hi, side ?? "new");
      }
      return anchor ? { file, mode, offset, anchor } : null;
    },
    [summary, store, hi],
  );

  const openComposer = useCallback(
    (start: Target, end: Target) => {
      const same = start.anchor.pr === end.anchor.pr && start.anchor.side === end.anchor.side && start.anchor.path === end.anchor.path;
      const from = same ? start : end;
      const lines = [from.anchor.line, end.anchor.line].sort((a, b) => a - b);
      const state: ComposerState = {
        key: `composer:${anchorKey(end.anchor)}`,
        kind: "line",
        prIndex: end.anchor.pr,
        path: end.anchor.path,
        side: end.anchor.side,
        line: lines[1],
        startLine: lines[0] !== lines[1] ? lines[0] : null,
        fallback: null,
      };
      setComposer(state);
      setSelection(null);
      acceptsLineComment(stackId, state.prIndex, state.path, state.side!, lines[0], lines[1])
        .then((accepted) => setComposer((c) => (c?.key === state.key ? { ...c, fallback: !accepted } : c)))
        .catch(() => undefined);
    },
    [stackId],
  );

  const openFileComposer = useCallback(
    (file: number) => {
      const summaryFile = summary.files[file];
      const touched = summaryFile.prs.filter((p) => p >= lo && p <= hi);
      const pr = touched.length ? Math.max(...touched) : hi;
      setComposer({ key: `composer:file:${summaryFile.path}`, kind: "file", prIndex: pr, path: pathInPr(summaryFile, pr, "RIGHT"), side: null, line: null, startLine: null, fallback: false });
    },
    [summary, lo, hi],
  );

  /** Gutter interactions: click = comment on the line, drag = comment on a range. */
  const onGutter = useCallback(
    (event: "down" | "enter" | "up" | "hover", file: number, mode: "unified" | "split", offset: number, side: "old" | "new" | null) => {
      const target = targetAt(file, mode, offset, side);
      if (event === "hover") {
        hovered.current = target;
        return;
      }
      if (event === "down") {
        dragging.current = target;
        if (target) setSelection({ file, mode, from: offset, to: offset });
        return;
      }
      const start = dragging.current;
      if (!start || !target || start.file !== file || start.mode !== mode) return;
      if (event === "enter") {
        setSelection({ file, mode, from: Math.min(start.offset, offset), to: Math.max(start.offset, offset) });
      } else {
        dragging.current = null;
        const [first, last] = start.offset <= offset ? [start, target] : [target, start];
        openComposer(first, last);
      }
    },
    [targetAt, openComposer],
  );

  // Load what's visible, per file segment.
  for (let row = first; row < last; ) {
    const segment = layout.segmentAt(row);
    const end = Math.min(last, segment.start + segment.rows);
    if (segment.mode !== "collapsed") store.ensure(segment.file, segment.mode, row - segment.start, end - segment.start);
    row = end;
  }

  const rendered: React.ReactNode[] = [];
  let visibleLoaded = true;
  for (let row = first; row < last; row++) {
    const segment = layout.segmentAt(row);
    const offset = row - segment.start;
    const file = summary.files[segment.file];
    const y = layout.rowY(row);
    const key = `${segment.file}:${segment.mode}:${offset}`;
    const selected =
      selection !== null && selection.file === segment.file && selection.mode === segment.mode && offset >= selection.from && offset <= selection.to;
    if (offset === 0) {
      rendered.push(<FileHeader key={key} y={y} file={file} mode={segment.mode} onComment={() => openFileComposer(segment.file)} />);
    } else if (segment.mode === "collapsed") {
      const why = file.noise ?? (isViewed(file.content_key) ? "viewed" : isIgnored(file.path) ? "ignored" : "collapsed");
      rendered.push(<CollapsedNotice key={key} y={y} file={file} why={why} onExpand={() => setExpanded((s) => new Set(s).add(file.path))} />);
    } else if (segment.mode === "split") {
      const split = store.split(segment.file, offset);
      if (!split) visibleLoaded = false;
      rendered.push(
        <SplitRowView key={key} y={y} row={split} showAttribution={showAttribution} selected={selected} file={segment.file} offset={offset} onGutter={onGutter} />,
      );
    } else {
      const unified = store.unified(segment.file, offset);
      if (!unified) visibleLoaded = false;
      const tag = multiPr && unified !== undefined && startsRun(unified, store.unified(segment.file, offset - 1));
      rendered.push(
        <DiffRow
          key={key}
          y={y}
          row={unified}
          prs={prs}
          showAttribution={showAttribution}
          tag={tag}
          selected={selected}
          file={segment.file}
          offset={offset}
          onGutter={onGutter}
        />,
      );
    }
  }
  for (const insert of layout.insertsBetween(first - 1, last)) {
    rendered.push(
      <InsertBox key={`insert:${insert.key}`} y={insert.y} onHeight={heightReporter(insert.key)}>
        {placedItems.get(insert.key)?.render()}
      </InsertBox>,
    );
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
        case "c":
          if (hovered.current) openComposer(hovered.current, hovered.current);
          else openFileComposer(here.file);
          break;
        case "v": {
          const file = summary.files[here.file];
          const viewed = !isViewed(file.content_key);
          changeLayout(() => {
            onToggleViewed(file.content_key, viewed);
            setExpanded((s) => withOut(s, file.path));
            if (!viewed) setCollapsed((s) => withOut(s, file.path));
          });
          break;
        }
        default:
          return;
      }
      e.preventDefault();
      if (target !== undefined) scrollToRow(target);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [keyboardEnabled, layout, summary, scrollToRow, toggleFileMode, toggleCollapsed, changeLayout, defaultMode, onDefaultModeChange, openComposer, openFileComposer, isViewed, onToggleViewed]);

  const gutterChars = Math.max(3, String(summary.max_line_number).length);

  return (
    <div className="diff-layout">
      {showFiles && (
        <FileList
          files={summary.files}
          isViewed={isViewed}
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
            {rendered}
          </div>
        </div>
      </div>
    </div>
  );
}

const at = (y: number): React.CSSProperties => ({ transform: `translateY(${y}px)` });

const FileHeader = memo(function FileHeader({ y, file, mode, onComment }: { y: number; file: FileSummary; mode: Mode; onComment: () => void }) {
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
      <button className="file-comment" title="Comment on this file" onClick={onComment}>
        Comment
      </button>
    </div>
  );
});

function CollapsedNotice({ y, file, why, onExpand }: { y: number; file: FileSummary; why: string; onExpand: () => void }) {
  return (
    <div className="row row-collapsed" style={at(y)} onClick={onExpand}>
      <span className="gutter" />
      <span className="code">
        {why} · +{file.additions} −{file.deletions} · click or press {why === "viewed" ? "v" : "e"} to expand
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

type GutterHandler = (event: "down" | "enter" | "up" | "hover", file: number, mode: "unified" | "split", offset: number, side: "old" | "new" | null) => void;

interface RowProps {
  y: number;
  row?: Row;
  prs: PullRequest[];
  showAttribution: boolean;
  /** Show the PR tag: this row starts a run of lines from one PR. */
  tag: boolean;
  selected: boolean;
  file: number;
  offset: number;
  onGutter: GutterHandler;
}

const gutterEvents = (onGutter: GutterHandler, file: number, mode: "unified" | "split", offset: number, side: "old" | "new" | null) => ({
  onMouseDown: (e: React.MouseEvent) => {
    e.preventDefault();
    onGutter("down", file, mode, offset, side);
  },
  onMouseEnter: () => onGutter("enter", file, mode, offset, side),
  onMouseUp: () => onGutter("up", file, mode, offset, side),
});

const DiffRow = memo(function DiffRow({ y, row, prs, showAttribution, tag, selected, file, offset, onGutter }: RowProps) {
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
        <div
          className={`row row-${kind}${attributed ? " attributed" : ""}${selected ? " selected" : ""}`}
          style={style}
          title={attributed ? lineHistory(row, prs) : undefined}
          onMouseEnter={() => onGutter("hover", file, "unified", offset, null)}
        >
          <span className="gutter commentable" {...gutterEvents(onGutter, file, "unified", offset, null)}>
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

interface SplitProps {
  y: number;
  row?: SplitRow;
  showAttribution: boolean;
  selected: boolean;
  file: number;
  offset: number;
  onGutter: GutterHandler;
}

const SplitRowView = memo(function SplitRowView({ y, row, showAttribution, selected, file, offset, onGutter }: SplitProps) {
  if (!row) return <div className="row row-loading" style={at(y)} />;
  const side = (no: number | null, kind: number, segs: Seg[], pr: number | null, left: boolean) => {
    const cls = sideClass(kind);
    const attributed = showAttribution && pr !== null;
    return (
      <span
        className={`half half-${cls}${left ? " half-left" : ""}${attributed ? " attributed" : ""}`}
        style={attributed ? ({ "--pr": prColor(pr!) } as React.CSSProperties) : undefined}
      >
        <span
          className={`gutter half-gutter${cls === "filler" ? "" : " commentable"}`}
          {...(cls === "filler" ? {} : gutterEvents(onGutter, file, "split", offset, left ? "old" : "new"))}
          onMouseEnter={() => cls !== "filler" && onGutter("enter", file, "split", offset, left ? "old" : "new")}
        >
          <span className="ln">{no ?? ""}</span>
        </span>
        <span className="code">
          <Segments segs={segs} />
        </span>
      </span>
    );
  };
  return (
    <div
      className={`row row-split${selected ? " selected" : ""}`}
      style={{ transform: `translate(var(--split-x, 0px), ${y}px)` }}
      onMouseEnter={() => onGutter("hover", file, "split", offset, "new")}
    >
      {side(row.o, row.ok, row.os, row.oa, true)}
      {side(row.n, row.nk, row.ns, row.na, false)}
    </div>
  );
});

interface FileListProps {
  files: FileSummary[];
  isViewed: (key: string) => boolean;
  segments: Segment[];
  current: number;
  showPrs: boolean;
  onSelect: (index: number) => void;
}

function FileList({ files, isViewed, segments, current, showPrs, onSelect }: FileListProps) {
  const activeRef = useRef<HTMLLIElement>(null);
  useEffect(() => activeRef.current?.scrollIntoView({ block: "nearest" }), [current]);
  return (
    <nav className="file-list">
      <ul>
        {files.map((f, i) => (
          <li
            key={f.path + i}
            ref={i === current ? activeRef : undefined}
            className={[i === current && "active", segments[i]?.mode === "collapsed" && "muted", isViewed(f.content_key) && "viewed"]
              .filter(Boolean)
              .join(" ")}
            onClick={() => onSelect(i)}
            title={f.path}
          >
            <span className={`file-status status-${f.status}`}>{isViewed(f.content_key) ? "✓" : statusLetter(f.status)}</span>
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

// ---------- comment helpers ----------

interface Target {
  file: number;
  mode: "unified" | "split";
  offset: number;
  anchor: Anchor;
}

interface Selection {
  file: number;
  mode: Mode;
  from: number;
  to: number;
}

interface ComposerState {
  key: string;
  kind: "line" | "file";
  prIndex: number;
  path: string;
  side: Anchor["side"] | null;
  line: number | null;
  startLine: number | null;
  /** Whether it'll go out as a file comment (null while checking). */
  fallback: boolean | null;
}

/** Where a comment on this unified row goes: the PR that last touched it, at its line there. */
function unifiedAnchor(row: Row, file: FileSummary, top: number): Anchor | null {
  if (row.k === RowKind.Added && row.a !== null && row.l !== null) return { pr: row.a, side: "RIGHT", line: row.l, path: pathInPr(file, row.a, "RIGHT") };
  if (row.k === RowKind.Deleted && row.a !== null && row.l !== null) return { pr: row.a, side: "LEFT", line: row.l, path: pathInPr(file, row.a, "LEFT") };
  if (row.k === RowKind.Context && row.n !== null) return { pr: top, side: "RIGHT", line: row.n, path: file.path };
  return null;
}

function splitAnchor(row: SplitRow, file: FileSummary, top: number, side: "old" | "new"): Anchor | null {
  if (side === "old") {
    if (row.ok === RowKind.Deleted && row.oa !== null && row.ol !== null) return { pr: row.oa, side: "LEFT", line: row.ol, path: pathInPr(file, row.oa, "LEFT") };
    if (row.ok === RowKind.Context && row.n !== null) return { pr: top, side: "RIGHT", line: row.n, path: file.path };
    return null;
  }
  if (row.nk === RowKind.Added && row.na !== null && row.nl !== null) return { pr: row.na, side: "RIGHT", line: row.nl, path: pathInPr(file, row.na, "RIGHT") };
  if (row.nk === RowKind.Context && row.n !== null) return { pr: top, side: "RIGHT", line: row.n, path: file.path };
  return null;
}

interface CommentItem {
  key: string;
  /** Line items are placed at their anchor; file items under their file's header. */
  anchor: Anchor | null;
  path: string;
  render: () => React.ReactNode;
}

interface ItemSources {
  drafts: ShownDraft[];
  threads: PrThreads[];
  composer: ComposerState | null;
  lo: number;
  hi: number;
  prs: PullRequest[];
  saveDraft: (draft: NewDraft) => Promise<void>;
  removeDraft: (id: string) => Promise<void>;
  editDraft: (id: string, body: string) => Promise<void>;
  closeComposer: () => void;
}

/** Threads, drafts and the open composer of the PRs in the range, as placeable items. */
function commentItems(src: ItemSources): CommentItem[] {
  const { drafts, threads, composer, lo, hi, prs } = src;
  const inRange = (i: number) => i >= lo && i <= hi;
  const items: CommentItem[] = [];
  const byThread = new Map<string, ShownDraft[]>();
  for (const d of drafts) if (d.draft.threadId) byThread.set(d.draft.threadId, [...(byThread.get(d.draft.threadId) ?? []), d]);

  for (const { prIndex, threads: list } of threads) {
    if (!inRange(prIndex)) continue;
    for (const thread of list) {
      const pending = byThread.get(thread.id) ?? [];
      const anchor: Anchor | null =
        thread.file_level || thread.line === null ? null : { pr: prIndex, path: thread.path, side: thread.side, line: thread.line };
      items.push({
        key: `thread:${thread.id}`,
        anchor,
        path: thread.path,
        render: () => (
          <ThreadCard
            thread={thread}
            label={threadLabel(thread, prs[prIndex].number)}
            replies={pending.filter((d) => d.draft.kind === "reply")}
            resolving={pending.find((d) => d.draft.kind === "resolve")}
            onReply={(body) =>
              void src.saveDraft({ prIndex, kind: "reply", path: thread.path, side: null, line: null, startLine: null, body, threadId: thread.id, replyTo: thread.comments[0]?.database_id ?? null })
            }
            onResolve={() =>
              void src.saveDraft({ prIndex, kind: "resolve", path: thread.path, side: null, line: null, startLine: null, body: "", threadId: thread.id, replyTo: null })
            }
            onDeleteDraft={(id) => void src.removeDraft(id)}
          />
        ),
      });
    }
  }

  for (const shown of drafts) {
    const { draft, prIndex } = shown;
    if (!inRange(prIndex) || (draft.kind !== "line" && draft.kind !== "file")) continue;
    const path = shown.path ?? draft.path ?? "";
    const anchor: Anchor | null =
      draft.kind === "line" && shown.line !== null && draft.side ? { pr: prIndex, path, side: draft.side, line: shown.line } : null;
    const where = draft.kind === "file" ? "file" : lineLabel(shown.startLine, shown.line) || lineLabel(draft.startLine, draft.line);
    items.push({
      key: `draft:${draft.id}`,
      anchor,
      path,
      render: () => (
        <DraftCard
          shown={shown}
          label={`#${prs[prIndex].number} · ${where}`}
          onEdit={(body) => void src.editDraft(draft.id, body)}
          onDelete={() => void src.removeDraft(draft.id)}
        />
      ),
    });
  }

  if (composer) {
    const anchor: Anchor | null =
      composer.kind === "line" && composer.line !== null && composer.side
        ? { pr: composer.prIndex, path: composer.path, side: composer.side, line: composer.line }
        : null;
    const where = composer.kind === "file" ? composer.path : `${composer.path} ${lineLabel(composer.startLine, composer.line)}`;
    items.push({
      key: composer.key,
      anchor,
      path: composer.path,
      render: () => (
        <Composer
          title={`Comment on #${prs[composer.prIndex].number} · ${where}`}
          fileFallback={composer.fallback}
          onSave={(body) => {
            src.closeComposer();
            void src.saveDraft({
              prIndex: composer.prIndex,
              kind: composer.kind,
              path: composer.path,
              side: composer.side,
              line: composer.line,
              startLine: composer.startLine,
              body,
              threadId: null,
              replyTo: null,
            });
          }}
          onCancel={src.closeComposer}
        />
      ),
    });
  }
  return items;
}

function threadLabel(thread: ReviewThread, prNumber: number) {
  const where = thread.file_level ? "file" : thread.line !== null ? lineLabel(thread.start_line, thread.line) : `was ${lineLabel(null, thread.original_line)}`;
  return `#${prNumber} · ${thread.path.split("/").pop()} ${where}`;
}