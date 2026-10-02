import { flushSync } from "react-dom";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  acceptsLineComment,
  createDraft,
  deleteDraft,
  getRowWidths,
  grep,
  locateAnchors,
  locateLine,
  readFile,
  updateDraft,
  usages as fetchUsages,
  askAssistant,
  deleteAssistantThread,
  listAssistantThreads,
  type AssistantThread,
  type GrepHit,
  type Provider,
  type Usages,
} from "./api";
import { AssistantPanel, type AskContext } from "./AssistantPanel";
import { CodePanel, type PanelState } from "./CodePanel";
import { FileView } from "./FileView";
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
import { Layout, ROW_HEIGHT, type Insert, type Mode, type Segment, type WrappedRows } from "./layout";
import { mark } from "./perf";
import { RowStore } from "./rowStore";
import { instantJumps } from "./themes";
import { prColor, RowKind, type DiffSummary, type FileSummary, type PullRequest, type Row, type Seg, type SplitRow } from "./types";

/** Rows drawn beyond the viewport, at least this many and at least a screen on each side. */
const OVERSCAN = 20;
/** Rows are fetched this many screens beyond the viewport, so files arrive before they're drawn. */
const PREFETCH_SCREENS = 2;
/** Arrow keys scroll by this many pixels (held: continuously). */
const ARROW_STEP = 3 * ROW_HEIGHT;
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
  /** Wrap long lines instead of scrolling sideways (global, remembered). */
  wrap: boolean;
  onWrapChange: (wrap: boolean) => void;
  /** Files the user asked to collapse in this repo (ignore patterns). */
  isIgnored: (path: string) => boolean;
  /** Viewed marks, by file content key. */
  isViewed: (key: string) => boolean;
  onToggleViewed: (key: string, viewed: boolean) => void;
}

export function DiffViewer(props: Props) {
  const { viewId, summary, prs, showAttribution, multiPr, showFiles, keyboardEnabled, defaultMode, onDefaultModeChange, wrap, onWrapChange, isIgnored } = props;
  const { stackId, lo, hi, drafts, threads, onDraftsChanged, isViewed, onToggleViewed } = props;
  const scrollRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(800);
  const [viewportWidth, setViewportWidth] = useState(0);
  /** Width of one monospace column in the diff, in pixels. */
  const [charWidth, setCharWidth] = useState(0);
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

  // ---------- code intelligence ----------
  const [panel, setPanel] = useState<PanelState | null>(null);
  const [usageResult, setUsageResult] = useState<Usages | null>(null);
  const [grepHits, setGrepHits] = useState<GrepHit[]>([]);
  const [grepStatus, setGrepStatus] = useState<string>("idle");
  /** Files the last search skipped because they couldn't be downloaded (offline). */
  const [grepUnsearched, setGrepUnsearched] = useState(0);
  const [fileView, setFileView] = useState<{ path: string; line: number; lines: Seg[][] | null; error: string | null } | null>(null);
  const [flash, setFlash] = useState<{ file: number; offset: number } | null>(null);
  const pendingJump = useRef<{ file: number; line: number } | null>(null);

  // ---------- assistant ----------
  const [assistant, setAssistant] = useState<{ context: AskContext; activeId: string | null; note: string | null } | null>(null);
  const [assistantThreads, setAssistantThreads] = useState<AssistantThread[]>([]);
  const [pendingAnswer, setPendingAnswer] = useState<{ threadId: string | null; question: string; text: string } | null>(null);
  useEffect(() => {
    listAssistantThreads(stackId)
      .then(setAssistantThreads)
      .catch(() => undefined);
  }, [stackId]);
  const lastPointer = useRef<{ x: number; y: number } | null>(null);
  const grepRun = useRef(0);

  const openUsages = useCallback(
    (name: string) => {
      const clean = name.replace(/^\$/, "");
      if (!clean) return;
      mark("usages:start");
      setAssistant(null);
      setPanel({ kind: "usages", name: clean });
      setGrepHits([]);
      setGrepStatus("idle");
      fetchUsages(viewId, clean)
        .then((result) => {
          setUsageResult(result);
          requestAnimationFrame(() => mark("usages:visible"));
        })
        .catch((e) => console.error("usages failed", e));
    },
    [viewId],
  );

  const search = useCallback(
    (query: string) => {
      const run = ++grepRun.current;
      mark("grep:start");
      setPanel((p) => (p?.kind === "usages" ? p : { kind: "search", query }));
      setGrepHits([]);
      setGrepStatus("running");
      setGrepUnsearched(0);
      let first = true;
      grep(stackId, hi, query, /^[A-Za-z0-9_$]+$/.test(query), (event) => {
        if (run !== grepRun.current) return;
        if (event.kind === "hits") {
          setGrepHits((current) => [...current, ...event.hits]);
          if (first) {
            first = false;
            requestAnimationFrame(() => mark("grep:first-hit"));
          }
        } else if (event.kind === "done") {
          setGrepUnsearched(event.unsearched);
          setGrepStatus("done");
        } else setGrepStatus(event.message);
      }).catch((e) => setGrepStatus(String(e)));
    },
    [stackId, hi],
  );

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

  // ---------- line wrap ----------
  const gutterChars = Math.max(3, String(summary.max_line_number).length);
  /** Code columns per line when wrapping (see the .wrap rules in styles.css for the paddings). */
  const columns = useMemo(() => {
    if (!wrap || !charWidth || !viewportWidth) return null;
    const fit = (px: number) => Math.max(20, Math.floor(px / charWidth));
    return {
      unified: fit(viewportWidth - (gutterChars * 2 * charWidth + 44) - WRAP_PAD_UNIFIED),
      split: fit(viewportWidth / 2 - 1 - (gutterChars * charWidth + 30) - WRAP_PAD_SPLIT),
    };
  }, [wrap, charWidth, viewportWidth, gutterChars]);
  /** Widths of the rows wider than `min` columns, per `file:mode`, fetched as files need them. */
  const [rowWidths, setRowWidths] = useState<{ viewId: string; files: Map<string, { min: number; rows: [number, number][] }> }>({ viewId, files: new Map() });
  const widths = rowWidths.viewId === viewId ? rowWidths.files : null;
  const requested = useRef(new Map<string, number>());
  useEffect(() => {
    if (!columns) return;
    if (!widths) {
      requested.current = new Map();
      setRowWidths({ viewId, files: new Map() });
      return;
    }
    const wanted: [number, boolean][] = [];
    let min = Infinity;
    summary.files.forEach((_, file) => {
      const mode = modeOf(file);
      if (mode === "collapsed") return;
      const key = `${file}:${mode}`;
      const cols = columns[mode];
      if ((widths.get(key)?.min ?? Infinity) <= cols || (requested.current.get(key) ?? Infinity) <= cols) return;
      wanted.push([file, mode === "split"]);
      // A bit below what fits now, so narrowing the window a little needs no new request.
      min = Math.min(min, Math.floor(cols * 0.8));
    });
    if (!wanted.length) return;
    for (const [file, split] of wanted) requested.current.set(`${file}:${split ? "split" : "unified"}`, min);
    getRowWidths(viewId, wanted, min)
      .then((result) =>
        setRowWidths((current) => {
          if (current.viewId !== viewId) return current;
          const files = new Map(current.files);
          wanted.forEach(([file, split], i) => files.set(`${file}:${split ? "split" : "unified"}`, { min, rows: result[i] }));
          return { viewId, files };
        }),
      )
      .catch((e) => console.error("row widths failed", e));
  }, [columns, widths, summary, modeOf, viewId]);
  const wrappedOf = useCallback(
    (file: number, mode: Mode): WrappedRows => {
      const known = columns && mode !== "collapsed" ? widths?.get(`${file}:${mode}`) : undefined;
      if (!columns || !known) return NO_ROWS;
      const cols = columns[mode as BaseMode];
      const out: [number, number][] = [];
      for (const [offset, width] of known.rows) if (width > cols) out.push([offset, Math.ceil(width / cols) - 1]);
      return out;
    },
    [columns, widths],
  );

  const layout = useMemo(() => new Layout(summary, modeOf, placed, wrappedOf), [summary, modeOf, placed, wrappedOf]);
  const placedItems = useMemo(() => new Map(placed.map((p) => [p.key, p.item])), [placed]);

  /** Scrolls to head line `line` of view file `file`, switching it to side by side if only the
   * full file has that line; flashes the row. */
  /**
   * The file picked by navigation (n/p, the file list, v) and the scroll position it was shown at.
   * While the view stays there it is the current file, even if it couldn't scroll to the top
   * (the last files of the diff) and a collapsed file sits above it.
   */
  const [pinned, setPinned] = useState<{ file: number; top: number } | null>(null);

  /** The running j/k glide (see `glideToRow`); anything else that moves the view stops it. */
  const glide = useRef<{ target: number; held: boolean; frame: number } | null>(null);
  const stopGlide = useCallback(() => {
    if (glide.current) cancelAnimationFrame(glide.current.frame);
    glide.current = null;
  }, []);
  useEffect(() => stopGlide, [stopGlide]);

  const jumpToLine = useCallback(
    async (file: number, line: number) => {
      const location = await locateLine(viewId, file, line);
      if (!location) return;
      const path = summary.files[file].path;
      const segment = layout.segments[file];
      const needsSplit = segment.mode === "collapsed" || (segment.mode === "unified" && location.unified === null);
      if (needsSplit) {
        pendingJump.current = { file, line };
        setExpanded((s) => new Set(s).add(path));
        setCollapsed((s) => withOut(s, path));
        if (location.unified === null) setOverrides((m) => new Map(m).set(path, "split"));
        return;
      }
      const offset = (segment.mode === "split" ? location.split : location.unified) ?? 0;
      const el = scrollRef.current;
      stopGlide();
      if (el) el.scrollTop = Math.max(0, layout.rowY(segment.start + offset) - el.clientHeight / 3);
      setFlash({ file, offset });
    },
    [viewId, summary, layout, stopGlide],
  );

  useEffect(() => {
    const jump = pendingJump.current;
    if (!jump) return;
    pendingJump.current = null;
    void jumpToLine(jump.file, jump.line);
  }, [layout, jumpToLine]);

  useEffect(() => {
    if (!flash) return;
    const timer = window.setTimeout(() => setFlash(null), 1400);
    return () => window.clearTimeout(timer);
  }, [flash]);

  const jump = useCallback(
    (target: { path: string; line: number; file?: number }) => {
      const file = target.file ?? summary.files.findIndex((f) => f.path === target.path);
      if (file >= 0 && summary.files[file].new_blob) {
        void jumpToLine(file, target.line);
        return;
      }
      setFileView({ path: target.path, line: target.line, lines: null, error: null });
      readFile(stackId, hi, target.path)
        .then((lines) => setFileView((v) => (v?.path === target.path ? { ...v, lines } : v)))
        .catch((e) => setFileView((v) => (v?.path === target.path ? { ...v, error: String(e) } : v)));
    },
    [summary, jumpToLine, stackId, hi],
  );
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
  /** After `v`: once the viewed file has collapsed, put `next`'s header at the top. */
  const pendingFile = useRef<{ viewed: number; next: number } | null>(null);
  /** After toggling a file (s, e, Space, v): it stays the current file once the layout changes. */
  const keepFile = useRef<number | null>(null);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const place = anchor.current;
    const index = place ? summary.files.findIndex((f) => f.path === place.path) : -1;
    let top = 0;
    const moveOn = pendingFile.current;
    let pin: number | null = null;
    if (moveOn && previousView.current === viewId && layout.segments[moveOn.viewed]?.mode === "collapsed") {
      pendingFile.current = null;
      top = layout.rowY(layout.segments[moveOn.next].start);
      pin = moveOn.next;
    } else if (index >= 0 && place) {
      const segment = layout.segments[index];
      const sameView = previousView.current === viewId;
      const keepOffset = sameView && segment.mode === place.mode;
      top = layout.rowY(segment.start + (keepOffset ? Math.min(place.offset, segment.rows - 1) : 0)) + (keepOffset ? place.delta : 0);
    }
    if (keepFile.current !== null && pin === null) pin = keepFile.current;
    keepFile.current = null;
    if (previousView.current !== viewId) {
      pendingMark.current = "view";
      pendingFile.current = null;
      pin = null;
      stopGlide();
    }
    previousView.current = viewId;
    el.scrollTop = top;
    setScrollTop(el.scrollTop);
    // Near the end of the diff a file can't reach the top; pinning keeps it current anyway.
    setPinned(pin === null ? null : { file: pin, top: el.scrollTop });
  }, [layout, viewId, summary]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () => {
      setViewportHeight(el.clientHeight);
      setViewportWidth(el.clientWidth);
      const probe = canvasRef.current?.querySelector<HTMLElement>(".char-probe");
      if (probe) setCharWidth(probe.getBoundingClientRect().width / probe.textContent!.length);
      // Side-by-side rows always span exactly the viewport (see .row-split).
      canvasRef.current?.style.setProperty("--vw", `${el.clientWidth}px`);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    measure();
    return () => observer.disconnect();
  }, []);

  const overscan = Math.max(OVERSCAN, Math.ceil(viewportHeight / ROW_HEIGHT));
  const first = Math.max(0, layout.rowAt(scrollTop) - overscan);
  const last = Math.min(layout.totalRows, layout.rowAt(scrollTop + viewportHeight) + 1 + overscan);

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

  /** The new-side text of the rows from `start` to `end` (null unless that's exactly `count` loaded lines). */
  const headText = useCallback(
    (start: Target, end: Target, count: number): string | null => {
      const lines: string[] = [];
      for (let offset = start.offset; offset <= end.offset; offset++) {
        let kind: number, segs: Seg[];
        if (start.mode === "unified") {
          const row = store.unified(start.file, offset);
          if (!row) return null;
          [kind, segs] = [row.k, row.s];
        } else {
          const row = store.split(start.file, offset);
          if (!row) return null;
          [kind, segs] = [row.nk, row.ns];
        }
        if (kind === RowKind.Added || kind === RowKind.Context) lines.push(segs.map((s) => s[1]).join(""));
      }
      return lines.length === count ? lines.join("\n") : null;
    },
    [store],
  );

  const openComposer = useCallback(
    (start: Target, end: Target) => {
      const same = start.anchor.pr === end.anchor.pr && start.anchor.side === end.anchor.side && start.anchor.path === end.anchor.path;
      const from = same ? start : end;
      const lines = [from.anchor.line, end.anchor.line].sort((a, b) => a - b);
      const suggestion = end.anchor.side === "RIGHT" ? headText(from, end, lines[1] - lines[0] + 1) : null;
      const state: ComposerState = {
        key: `composer:${anchorKey(end.anchor)}`,
        kind: "line",
        prIndex: end.anchor.pr,
        path: end.anchor.path,
        side: end.anchor.side,
        line: lines[1],
        startLine: lines[0] !== lines[1] ? lines[0] : null,
        fallback: null,
        suggestion,
      };
      setComposer(state);
      setSelection(null);
      acceptsLineComment(stackId, state.prIndex, state.path, state.side!, lines[0], lines[1])
        .then((accepted) => setComposer((c) => (c?.key === state.key ? { ...c, fallback: !accepted } : c)))
        .catch(() => undefined);
    },
    [stackId, headText],
  );

  const openFileComposer = useCallback(
    (file: number) => {
      const summaryFile = summary.files[file];
      const touched = summaryFile.prs.filter((p) => p >= lo && p <= hi);
      const pr = touched.length ? Math.max(...touched) : hi;
      setComposer({ key: `composer:file:${summaryFile.path}`, kind: "file", prIndex: pr, path: pathInPr(summaryFile, pr, "RIGHT"), side: null, line: null, startLine: null, fallback: false, suggestion: null });
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

  const rangeLabel = lo === hi ? `#${prs[lo].number}` : `#${prs[lo].number}–#${prs[hi].number}`;

  /** The selected code (a native text selection across rows) as assistant context. */
  const selectionContext = useCallback((): AskContext => {
    const none: AskContext = { selection: null, anchor: null, rangeLabel };
    const selected = window.getSelection();
    if (!selected || selected.isCollapsed) return none;
    const rowOf = (node: Node | null) => (node instanceof Element ? node : node?.parentElement)?.closest<HTMLElement>(".row[data-file]") ?? null;
    const [a, b] = [rowOf(selected.anchorNode), rowOf(selected.focusNode)];
    if (!a || !b || a.dataset.file !== b.dataset.file || a.dataset.mode !== b.dataset.mode) return none;
    const file = Number(a.dataset.file);
    const mode = a.dataset.mode as "unified" | "split";
    const [from, to] = [Number(a.dataset.offset), Number(b.dataset.offset)].sort((x, y) => x - y);
    const first = targetAt(file, mode, from, "new") ?? targetAt(file, mode, from, "old");
    const last = targetAt(file, mode, to, "new") ?? targetAt(file, mode, to, "old");
    if (!first || !last) return none;
    const lines: string[] = [];
    const heads: number[] = [];
    for (let offset = from; offset <= to; offset++) {
      if (mode === "unified") {
        const row = store.unified(file, offset);
        if (!row || row.k === RowKind.Hunk || row.k === RowKind.File) continue;
        const sign = row.k === RowKind.Added ? "+" : row.k === RowKind.Deleted ? "-" : " ";
        lines.push(sign + row.s.map((seg) => seg[1]).join(""));
        if (row.n !== null) heads.push(row.n);
      } else {
        const row = store.split(file, offset);
        if (!row) continue;
        lines.push((row.nk === RowKind.Filler ? row.os : row.ns).map((seg) => seg[1]).join(""));
        if (row.n !== null) heads.push(row.n);
      }
    }
    const same = first.anchor.pr === last.anchor.pr && first.anchor.side === last.anchor.side && first.anchor.path === last.anchor.path;
    const start = same ? Math.min(first.anchor.line, last.anchor.line) : last.anchor.line;
    const end = same ? Math.max(first.anchor.line, last.anchor.line) : last.anchor.line;
    const prLabel = `#${prs[last.anchor.pr].number}`;
    return {
      rangeLabel,
      selection: { path: summary.files[file].path, prLabel, startLine: start, endLine: end, text: lines.join("\n") },
      anchor: {
        path: last.anchor.path,
        prIndex: last.anchor.pr,
        side: last.anchor.side,
        startLine: start,
        endLine: end,
        headStart: heads.length ? Math.min(...heads) : null,
        headEnd: heads.length ? Math.max(...heads) : null,
      },
    };
  }, [rangeLabel, targetAt, store, summary, prs]);

  const openAssistant = useCallback(
    (threadId: string | null = null) => {
      setPanel(null);
      const context = threadId ? { selection: null, anchor: null, rangeLabel } : selectionContext();
      setAssistant({ context, activeId: threadId, note: null });
    },
    [selectionContext, rangeLabel],
  );

  const ask = useCallback(
    (question: string, choice: { provider: Provider; model: string | null; effort: string | null }) => {
      if (!assistant) return;
      const threadId = assistant.activeId;
      setPendingAnswer({ threadId, question, text: "" });
      const newThread = threadId ? null : { ...choice, selection: assistant.context.selection, anchor: assistant.context.anchor };
      askAssistant(stackId, lo, hi, threadId, newThread, question, (event) => {
        if (event.kind === "delta") setPendingAnswer((p) => p && { ...p, text: p.text + event.text });
        else if (event.kind === "text") setPendingAnswer((p) => p && { ...p, text: event.text });
      })
        .then((thread) => {
          setAssistantThreads((list) => [...list.filter((t) => t.id !== thread.id), thread]);
          setAssistant((a) => a && { ...a, activeId: thread.id });
        })
        .catch((e) => setAssistant((a) => a && { ...a, note: String(e) }))
        .finally(() => setPendingAnswer(null));
    },
    [assistant, stackId, lo, hi],
  );

  const answerToDraft = useCallback(
    async (thread: AssistantThread, text: string) => {
      const anchor = thread.selection;
      await createDraft(
        stackId,
        anchor
          ? { prIndex: anchor.prIndex, kind: "line", path: anchor.path, side: anchor.side, line: anchor.endLine, startLine: anchor.startLine, body: text, threadId: null, replyTo: null }
          : { prIndex: hi, kind: "summary", path: null, side: null, line: null, startLine: null, body: text, threadId: null, replyTo: null },
      );
      onDraftsChanged();
      setAssistant((a) => a && { ...a, note: anchor ? "Saved as a draft comment on those lines" : `Saved as the review summary draft for #${prs[hi].number}` });
    },
    [stackId, hi, prs, onDraftsChanged],
  );

  /** Head lines of the files in view that assistant threads were asked about (gutter markers). */
  const markers = useMemo(() => {
    const byPath = new Map<string, { from: number; to: number; id: string }[]>();
    for (const thread of assistantThreads) {
      const a = thread.selection;
      if (!a || a.headStart === null || a.headEnd === null) continue;
      const path = summary.files.find((f) => f.path === a.path || f.pr_paths.some(([, p]) => p === a.path))?.path;
      if (path) byPath.set(path, [...(byPath.get(path) ?? []), { from: a.headStart, to: a.headEnd, id: thread.id }]);
    }
    return byPath;
  }, [assistantThreads, summary]);
  const markerAt = (path: string, line: number | null) =>
    line === null ? undefined : markers.get(path)?.find((m) => line >= m.from && line <= m.to)?.id;

  // Load what's drawn and a couple of screens beyond, per file segment.
  const loadFirst = Math.max(0, layout.rowAt(scrollTop - PREFETCH_SCREENS * viewportHeight));
  const loadLast = Math.min(layout.totalRows, layout.rowAt(scrollTop + (PREFETCH_SCREENS + 1) * viewportHeight) + 1);
  for (let row = loadFirst; row < loadLast; ) {
    const segment = layout.segmentAt(row);
    const end = Math.min(loadLast, segment.start + segment.rows);
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
    const height = columns ? layout.rowHeight(row) : ROW_HEIGHT;
    const key = `${segment.file}:${segment.mode}:${offset}`;
    const selected =
      (selection !== null && selection.file === segment.file && selection.mode === segment.mode && offset >= selection.from && offset <= selection.to) ||
      (flash !== null && flash.file === segment.file && flash.offset === offset);
    if (offset === 0) {
      rendered.push(<FileHeader key={key} y={y} file={file} mode={segment.mode} onComment={() => openFileComposer(segment.file)} />);
    } else if (segment.mode === "collapsed") {
      const why = file.noise ?? (isViewed(file.content_key) ? "viewed" : isIgnored(file.path) ? "ignored" : "collapsed");
      rendered.push(<CollapsedNotice key={key} y={y} file={file} why={why} onExpand={() => setExpanded((s) => new Set(s).add(file.path))} />);
    } else if (segment.mode === "split") {
      const split = store.split(segment.file, offset);
      if (!split) visibleLoaded = false;
      rendered.push(
        <SplitRowView
          key={key}
          y={y}
          height={height}
          row={split}
          showAttribution={showAttribution}
          selected={selected}
          file={segment.file}
          offset={offset}
          onGutter={onGutter}
          marker={split ? markerAt(file.path, split.n) : undefined}
          onMarker={openAssistant}
        />,
      );
    } else {
      const unified = store.unified(segment.file, offset);
      if (!unified) visibleLoaded = false;
      const tag = multiPr && unified !== undefined && startsRun(unified, store.unified(segment.file, offset - 1));
      rendered.push(
        <DiffRow
          key={key}
          y={y}
          height={height}
          row={unified}
          prs={prs}
          showAttribution={showAttribution}
          tag={tag}
          selected={selected}
          file={segment.file}
          offset={offset}
          onGutter={onGutter}
          marker={unified ? markerAt(file.path, unified.n) : undefined}
          onMarker={openAssistant}
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
    // Draw the rows for the new position right away; a deferred render shows up as blank frames.
    flushSync(() => setScrollTop(el.scrollTop));
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

  /** The file keys act on: the pinned one while the view hasn't moved, else the one at the top. */
  const fileAt = useCallback(
    (top: number) =>
      pinned && Math.abs(pinned.top - top) < 1 && layout.segments[pinned.file]
        ? layout.segments[pinned.file]
        : // One row in, so a file header scrolled to the top counts.
          layout.segmentAt(Math.min(layout.totalRows - 1, layout.rowAt(top) + 1)),
    [pinned, layout],
  );
  const current = fileAt(scrollTop);

  /** Puts `row` near the top, with `margin` rows above it; `pin` makes that file the current one. */
  const scrollToRow = useCallback(
    (row: number, margin = JUMP_MARGIN, pin?: number) => {
      stopGlide();
      const el = scrollRef.current;
      if (!el) return;
      el.scrollTop = Math.max(0, layout.rowY(Math.max(0, row - margin)));
      if (pin !== undefined) setPinned({ file: pin, top: el.scrollTop });
    },
    [layout, stopGlide],
  );

  /**
   * j/k: ease to a change instead of teleporting, so a held key reads as one continuous scroll
   * that settles on a change when released. A single hop further than a screen jumps directly.
   */
  const glideTo = useCallback(
    (to: number, held: boolean) => {
      const el = scrollRef.current;
      if (!el) return;
      const top = Math.min(Math.max(0, to), el.scrollHeight - el.clientHeight);
      if (glide.current) {
        glide.current.target = top;
        glide.current.held = held;
        return;
      }
      if (!held && (instantJumps() || Math.abs(top - el.scrollTop) > el.clientHeight)) {
        el.scrollTop = top;
        flushSync(() => setScrollTop(el.scrollTop));
        return;
      }
      const state = { target: top, held, frame: 0 };
      glide.current = state;
      const step = () => {
        const before = el.scrollTop;
        const distance = state.target - before;
        // Held: at most ~6 screens a second, so the passing code stays readable.
        const cap = el.clientHeight / (state.held ? 10 : 3);
        const move = Math.sign(distance) * Math.min(Math.abs(distance), cap, Math.max(Math.abs(distance) * 0.35, 6));
        el.scrollTop = before + move;
        // Render the rows for the new position before this frame paints, not a frame later.
        flushSync(() => setScrollTop(el.scrollTop));
        // Done when there, or when the browser won't scroll any further.
        if (Math.abs(distance) < 1 || el.scrollTop === before) glide.current = null;
        else state.frame = requestAnimationFrame(step);
      };
      state.frame = requestAnimationFrame(step);
    },
    [],
  );
  const glideToRow = useCallback(
    (row: number, held: boolean) => glideTo(layout.rowY(Math.max(0, row - JUMP_MARGIN)), held),
    [layout, glideTo],
  );

  // Record the anchor before any layout change triggered from here, then flag the perf mark.
  const changeLayout = useCallback(
    (apply: () => void) => {
      stopGlide();
      onScroll();
      pendingMark.current = "layout";
      mark("toggle:start");
      apply();
    },
    [onScroll, stopGlide],
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
      // Escape works from anywhere (e.g. the assistant's model picker); text fields handle their own.
      if (e.metaKey || e.ctrlKey || e.altKey || (isTyping(e.target) && e.key !== "Escape")) return;
      const el = scrollRef.current;
      if (!el) return;
      // While gliding, j/k continue from where the glide is heading.
      const heading = glide.current?.target ?? el.scrollTop;
      const anchorRow = layout.rowAt(heading) + JUMP_MARGIN;
      // Files: the header of the file being read sits at the top after n/p.
      const here = fileAt(el.scrollTop);
      const topRow = here === layout.segments[pinned?.file ?? -1] ? here.start : layout.rowAt(heading);
      const starts = layout.segments.map((s) => s.start);
      const changes = layout.changeRows(summary);
      let target: number | undefined;
      let fileTarget: number | undefined;
      switch (e.key) {
        case "j":
        case "k": {
          e.preventDefault();
          // A held key doesn't run more than a screen ahead of what's shown.
          if (e.repeat && Math.abs(heading - el.scrollTop) > el.clientHeight) return;
          const change = e.key === "j" ? changes.find((r) => r > anchorRow) : findLast(changes, (r) => r < anchorRow);
          if (change !== undefined) glideToRow(change, e.repeat);
          return;
        }
        case "ArrowDown":
        case "ArrowUp": {
          e.preventDefault();
          const down = e.key === "ArrowDown";
          if (e.shiftKey) {
            // Shift+arrows: next/previous file, like n/p.
            fileTarget = down ? starts.find((r) => r > topRow) : findLast(starts, (r) => r < topRow);
            break;
          }
          glideTo(heading + (down ? ARROW_STEP : -ARROW_STEP), true);
          return;
        }
        case "n":
          fileTarget = starts.find((r) => r > topRow);
          break;
        case "p":
          fileTarget = findLast(starts, (r) => r < topRow);
          break;
        case "s":
          keepFile.current = here.file;
          toggleFileMode(here);
          break;
        case "z":
          changeLayout(() => onWrapChange(!wrap));
          break;
        case "S":
          changeLayout(() => {
            setOverrides(new Map());
            onDefaultModeChange(defaultMode === "split" ? "unified" : "split");
          });
          break;
        case "e":
        case " ":
          // Collapse/expand without touching the viewed mark.
          keepFile.current = here.file;
          toggleCollapsed(here);
          break;
        case "c":
          if (hovered.current) openComposer(hovered.current, hovered.current);
          else openFileComposer(here.file);
          break;
        case "u": {
          const point = lastPointer.current;
          const word = point && wordAt(point.x, point.y);
          if (word) openUsages(word);
          break;
        }
        case "/":
          setAssistant(null);
          setPanel({ kind: "search", query: "" });
          break;
        case "a":
          openAssistant();
          break;
        case "Escape":
          // The assistant first, then the other panels, then an open comment box. (A focused
          // comment box closes itself.)
          if (assistant) setAssistant(null);
          else if (panel) setPanel(null);
          else if (composer) setComposer(null);
          else if (selection) setSelection(null);
          else return;
          break;
        case "v": {
          const file = summary.files[here.file];
          const viewed = !isViewed(file.content_key);
          changeLayout(() => {
            onToggleViewed(file.content_key, viewed);
            setExpanded((s) => withOut(s, file.path));
            if (!viewed) setCollapsed((s) => withOut(s, file.path));
            // Marking viewed moves on to the next file still to review, so v, v, v… works.
            const next = viewed ? summary.files.findIndex((f, i) => i > here.file && !isViewed(f.content_key)) : -1;
            if (next >= 0) pendingFile.current = { viewed: here.file, next };
            else keepFile.current = here.file;
          });
          break;
        }
        default:
          return;
      }
      e.preventDefault();
      if (target !== undefined) scrollToRow(target);
      if (fileTarget !== undefined) scrollToRow(fileTarget, 0, layout.segmentAt(fileTarget).file);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [keyboardEnabled, layout, summary, fileAt, pinned, scrollToRow, glideTo, glideToRow, selection, toggleFileMode, toggleCollapsed, changeLayout, defaultMode, onDefaultModeChange, wrap, onWrapChange, openComposer, openFileComposer, isViewed, onToggleViewed, openUsages, panel, assistant, openAssistant, composer]);

  return (
    <div className="diff-layout">
      {showFiles && (
        <FileList
          files={summary.files}
          isViewed={isViewed}
          segments={layout.segments}
          current={current.file}
          showPrs={multiPr}
          onSelect={(index) => scrollToRow(layout.segments[index].start, 0, index)}
        />
      )}
      <div className="diff-main">
        <div
          className="diff-scroll"
          ref={scrollRef}
          onScroll={onScroll}
          onWheel={stopGlide}
          data-testid="diff-scroll"
          onMouseMove={(e) => (lastPointer.current = { x: e.clientX, y: e.clientY })}
          onClick={(e) => {
            if (!e.metaKey) return;
            const word = wordAt(e.clientX, e.clientY);
            if (word) {
              e.preventDefault();
              openUsages(word);
            }
          }}
        >
          <div
            ref={canvasRef}
            className={`diff-canvas${columns ? " wrap" : ""}`}
            style={
              {
                height: layout.height,
                "--gutter-ch": gutterChars,
                // Wrapped: no sideways scrolling, and each line's code is exactly this many columns.
                "--wrap-unified": columns?.unified,
                "--wrap-split": columns?.split,
                minWidth: columns ? undefined : `calc(${summary.max_line_chars + 4}ch + ${gutterChars * 2}ch + 48px)`,
              } as React.CSSProperties
            }
          >
            <span className="char-probe" aria-hidden>
              {"0".repeat(100)}
            </span>
            {rendered}
          </div>
        </div>
      </div>
      {panel && (
        <CodePanel
          state={panel}
          usages={panel.kind === "usages" ? usageResult : null}
          grepHits={grepHits}
          grepStatus={grepStatus}
          grepUnsearched={grepUnsearched}
          onSearch={search}
          onJump={jump}
          onClose={() => setPanel(null)}
        />
      )}
      {assistant && (
        <AssistantPanel
          threads={assistantThreads}
          activeId={assistant.activeId}
          context={assistant.context}
          pending={pendingAnswer}
          note={assistant.note}
          onSelect={(id) => setAssistant((a) => a && { ...a, activeId: id, note: null })}
          onAsk={ask}
          onDraft={(thread, text) => void answerToDraft(thread, text)}
          onDelete={(id) => {
            void deleteAssistantThread(id);
            setAssistantThreads((list) => list.filter((t) => t.id !== id));
            setAssistant((a) => a && { ...a, activeId: null });
          }}
          onClose={() => setAssistant(null)}
        />
      )}
      {fileView && <FileView {...fileView} onClose={() => setFileView(null)} />}
    </div>
  );
}

/** The identifier under a screen position (for ⌘-click and `u`). */
function wordAt(x: number, y: number): string | null {
  const range = document.caretRangeFromPoint?.(x, y);
  const node = range?.startContainer;
  if (!range || !node || node.nodeType !== Node.TEXT_NODE || !node.parentElement?.closest(".code")) return null;
  const text = node.textContent ?? "";
  const isWord = (c: string | undefined) => c !== undefined && /[A-Za-z0-9_$]/.test(c);
  let start = range.startOffset;
  let end = start;
  while (start > 0 && isWord(text[start - 1])) start--;
  while (end < text.length && isWord(text[end])) end++;
  const word = text.slice(start, end).replace(/^\$/, "");
  return /^[A-Za-z_]/.test(word) ? word : null;
}

const at = (y: number, height = ROW_HEIGHT): React.CSSProperties =>
  height === ROW_HEIGHT ? { transform: `translateY(${y}px)` } : { transform: `translateY(${y}px)`, height };
const NO_ROWS: WrappedRows = [];
/** Horizontal padding of a wrapped line's code (unified leaves room for the PR tag). */
const WRAP_PAD_UNIFIED = 8 + 48;
const WRAP_PAD_SPLIT = 8 + 24;

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
  height: number;
  row?: Row;
  prs: PullRequest[];
  showAttribution: boolean;
  /** Show the PR tag: this row starts a run of lines from one PR. */
  tag: boolean;
  selected: boolean;
  file: number;
  offset: number;
  onGutter: GutterHandler;
  /** An assistant thread was asked about this line. */
  marker?: string;
  onMarker: (threadId: string) => void;
}

const gutterEvents = (onGutter: GutterHandler, file: number, mode: "unified" | "split", offset: number, side: "old" | "new" | null) => ({
  onMouseDown: (e: React.MouseEvent) => {
    e.preventDefault();
    onGutter("down", file, mode, offset, side);
  },
  onMouseEnter: () => onGutter("enter", file, mode, offset, side),
  onMouseUp: () => onGutter("up", file, mode, offset, side),
});

const DiffRow = memo(function DiffRow({ y, height, row, prs, showAttribution, tag, selected, file, offset, onGutter, marker, onMarker }: RowProps) {
  const style = at(y, height);
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
          data-file={file}
          data-mode="unified"
          data-offset={offset}
        >
          <span className="gutter commentable" {...gutterEvents(onGutter, file, "unified", offset, null)}>
            {marker && <AssistantMark id={marker} onOpen={onMarker} />}
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
  height: number;
  row?: SplitRow;
  showAttribution: boolean;
  selected: boolean;
  file: number;
  offset: number;
  onGutter: GutterHandler;
  marker?: string;
  onMarker: (threadId: string) => void;
}

const SplitRowView = memo(function SplitRowView({ y, height, row, showAttribution, selected, file, offset, onGutter, marker, onMarker }: SplitProps) {
  if (!row) return <div className="row row-loading" style={at(y, height)} />;
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
          {!left && marker && <AssistantMark id={marker} onOpen={onMarker} />}
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
      style={{ transform: `translate(var(--split-x, 0px), ${y}px)`, height: height === ROW_HEIGHT ? undefined : height }}
      onMouseEnter={() => onGutter("hover", file, "split", offset, "new")}
      data-file={file}
      data-mode="split"
      data-offset={offset}
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

function AssistantMark({ id, onOpen }: { id: string; onOpen: (id: string) => void }) {
  return (
    <span
      className="assistant-mark"
      title="Assistant conversation about these lines"
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.stopPropagation();
        onOpen(id);
      }}
    />
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
  /** The commented lines' text, for a suggested change (new side only). */
  suggestion: string | null;
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
          suggestion={composer.suggestion}
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