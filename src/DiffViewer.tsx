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
  existingFiles,
  updateDraft,
  usages as fetchUsages,
  askAssistant,
  deleteAssistantThread,
  listAssistantThreads,
  type AssistantThread,
  type CommentOutcome,
  type GrepHit,
  type Provider,
  type Usages,
} from "./api";
import { AssistantPanel, type AskContext, type PendingAnswer } from "./AssistantPanel";
import type { CodeRef } from "./Markdown";
import { CodePanel, type PanelState } from "./CodePanel";
import { FileView } from "./FileView";
import {
  prKey,
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
import { Composer, DraftCard, InsertBox, settledReason, ThreadCard, threadPreview } from "./Inserts";
import { Layout, ROW_HEIGHT, type Insert, type Mode, type Segment, type WrappedRows } from "./layout";
import { mark } from "./perf";
import { RowStore } from "./rowStore";
import { instantJumps } from "./themes";
import { prColor, RowKind, type DiffSummary, type FileSummary, type PullRequest, type Row, type Seg, type SplitRow } from "./types";
import { mod } from "./platform";
import { loadList, loadRecord, saveList, saveRecord } from "./prefs";
import { ResizeHandle, useSidebarWidth } from "./Resizable";
import { loadPref, savePref } from "./prefs";
import { allDirs, ancestors, buildTree, treeRows } from "./fileTree";

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
  /** Shows the file list (`f` opens its filter menu). */
  onShowFiles: () => void;
  keyboardEnabled: boolean;
  /** Global default for files without a per-file choice. */
  defaultMode: BaseMode;
  onDefaultModeChange: (mode: BaseMode) => void;
  /** Wrap long lines instead of scrolling sideways (global, remembered). */
  wrap: boolean;
  onWrapChange: (wrap: boolean) => void;
  /** PRs (`prKey`) whose review threads are all hidden, leaving icons in the gutter (remembered). */
  hiddenCommentPrs: Set<string>;
  onCommentsHiddenChange: (prKeys: string[], hide: boolean) => void;
  /** Files the user asked to collapse in this repo (ignore patterns). */
  isIgnored: (path: string) => boolean;
  /** Viewed marks, by file content key. */
  isViewed: (key: string) => boolean;
  onToggleViewed: (key: string, viewed: boolean) => void;
}

export function DiffViewer(props: Props) {
  const { viewId, summary, prs, showAttribution, multiPr, showFiles, keyboardEnabled, defaultMode, onDefaultModeChange, wrap, onWrapChange, isIgnored } = props;
  const { hiddenCommentPrs, onCommentsHiddenChange, onShowFiles } = props;
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
  /** The file list as a flat list or a directory tree (`t`, remembered). */
  const [listMode, setListMode] = useState(() => loadPref("fileList", LIST_MODES, "list"));
  const toggleListMode = useCallback(
    () =>
      setListMode((m) => {
        const next = m === "list" ? "tree" : "list";
        savePref("fileList", next);
        return next;
      }),
    [],
  );

  /** Which perf mark to set once the visible rows are loaded ("view" / "layout"). */
  const pendingMark = useRef<string | null>("view");
  /** Where the viewport's top is, as (file path, offset in file, pixels into that row). */
  const anchor = useRef<{ path: string; offset: number; mode: Mode; delta: number } | null>(null);

  const store = useMemo(() => new RowStore(viewId, () => setVersion((v) => v + 1)), [viewId]);

  // ---------- file filter (by extension; remembered per PR or range) ----------
  const filterKey = lo === hi ? prKey(prs[lo]) : `${prKey(prs[lo])}..#${prs[hi].number}`;
  const [filters, setFilters] = useState(() => loadRecord("fileFilters"));
  const filter = useMemo(() => toFileFilter(filters[filterKey]), [filters, filterKey]);
  /** Per file index: left out by the filter. */
  const filtered = useMemo(() => summary.files.map((f) => !filter.off && filter.exts.includes(extensionOf(f.path))), [summary, filter]);
  const [filterOpen, setFilterOpen] = useState(false);

  const modeOf = useCallback(
    (index: number): Mode => {
      const file = summary.files[index];
      if (filtered[index]) return "hidden";
      const hidden =
        collapsed.has(file.path) || ((file.noise !== null || isIgnored(file.path) || isViewed(file.content_key)) && !expanded.has(file.path));
      if (hidden) return "collapsed";
      const wanted = overrides.get(file.path) ?? defaultMode;
      return wanted === "split" && file.split_rows > 0 ? "split" : "unified";
    },
    [summary, overrides, expanded, collapsed, defaultMode, isIgnored, isViewed, filtered],
  );
  const baseLayout = useMemo(() => new Layout(summary, modeOf), [summary, modeOf]);

  // ---------- comments: what goes where ----------
  const [composer, setComposer] = useState<ComposerState | null>(null);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [heights, setHeights] = useState<Map<string, number>>(new Map());
  /** Where the comment anchors are in `viewId` (file indices differ between views, so stale ones are ignored). */
  const [located, setLocated] = useState<{ viewId: string; locations: Map<string, Location | null> } | null>(null);
  const hovered = useRef<Target | null>(null);
  const dragging = useRef<Target | null>(null);

  // ---------- code intelligence ----------
  const [panel, setPanel] = useState<PanelState | null>(null);
  const [usageResult, setUsageResult] = useState<Usages | null>(null);
  const [grepHits, setGrepHits] = useState<GrepHit[]>([]);
  const [grepStatus, setGrepStatus] = useState<string>("idle");
  /** Files the last search skipped because they couldn't be downloaded (offline). */
  const [grepUnsearched, setGrepUnsearched] = useState(0);
  const [fileView, setFileView] = useState<{ path: string; line: number; end: number; lines: Seg[][] | null; error: string | null } | null>(null);
  const [flash, setFlash] = useState<{ file: number; from: number; to: number } | null>(null);
  const pendingJump = useRef<{ file: number; line: number; end: number } | null>(null);

  // ---------- assistant ----------
  const [assistant, setAssistant] = useState<{ context: AskContext; activeId: string | null; note: string | null } | null>(null);
  const [assistantThreads, setAssistantThreads] = useState<AssistantThread[]>([]);
  const [pendingAnswer, setPendingAnswer] = useState<PendingAnswer | null>(null);
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

  // Threads hidden one by one (remembered), and the ones shown again from their gutter icon. The
  // rest follow `h` (hide all) or, by default, GitHub: resolved / outdated / minimized ones hidden.
  const [hiddenThreads, setHiddenThreads] = useState(() => new Set(loadList("hiddenThreads")));
  const [revealed, setRevealed] = useState<Set<string>>(new Set());
  const changeHidden = useCallback((change: (s: Set<string>) => void) => {
    setHiddenThreads((current) => {
      const next = new Set(current);
      change(next);
      saveList("hiddenThreads", [...next].slice(-500));
      return next;
    });
  }, []);
  // Hiding or showing a PR's threads (h) starts over: the ones shown from their icons follow it.
  const [hiddenPrsSeen, setHiddenPrsSeen] = useState(hiddenCommentPrs);
  if (hiddenPrsSeen !== hiddenCommentPrs) {
    setHiddenPrsSeen(hiddenCommentPrs);
    setRevealed(new Set());
  }
  const rangeKeys = useMemo(() => prs.slice(lo, hi + 1).map(prKey), [prs, lo, hi]);
  const rangeHidden = rangeKeys.every((k) => hiddenCommentPrs.has(k));
  const isHidden = useCallback(
    (thread: ReviewThread, prIndex: number) =>
      !revealed.has(thread.id) && (hiddenCommentPrs.has(prKey(prs[prIndex])) || hiddenThreads.has(thread.id) || settledReason(thread) !== null),
    [prs, hiddenCommentPrs, revealed, hiddenThreads],
  );
  const hideThread = useCallback(
    (id: string) => {
      setRevealed((s) => withOut(s, id));
      changeHidden((s) => s.add(id));
    },
    [changeHidden],
  );
  const showThreads = useCallback(
    (ids: string[]) => {
      setRevealed((s) => new Set([...s, ...ids]));
      changeHidden((s) => ids.forEach((id) => s.delete(id)));
    },
    [changeHidden],
  );

  const items = useMemo(
    () => commentItems({ drafts, threads, composer, lo, hi, prs, saveDraft, removeDraft, editDraft, closeComposer: () => setComposer(null), isHidden, hideThread }),
    [drafts, threads, composer, lo, hi, prs, saveDraft, removeDraft, editDraft, isHidden, hideThread],
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
        setLocated({ viewId, locations: new Map(anchorList.map((a, i) => [anchorKey(a), found[i]])) });
      })
      .catch((e) => console.error("locate failed", e));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewId, anchorsSignature]);

  const { placed, marks, tallies } = useMemo(() => {
    const locations = located?.viewId === viewId ? located.locations : new Map<string, Location | null>();
    const out: (Insert & { item: CommentItem })[] = [];
    /** Hidden threads by the row their icon goes on (and the side, side by side). */
    const marks = new Map<string, CommentMarkItem>();
    /** Threads and drafts per file, by tally (a collapsed file sums them up instead of showing them). */
    const tallies = new Map<number, Map<string, number>>();
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
      if (segment.mode === "hidden") continue;
      const after = segment.start + (offset ?? (segment.mode === "collapsed" ? 1 : 0));
      if (item.tally) {
        const counts = tallies.get(file) ?? new Map<string, number>();
        counts.set(item.tally, (counts.get(item.tally) ?? 0) + 1);
        tallies.set(file, counts);
        if (segment.mode === "collapsed") continue;
      }
      if (item.hidden) {
        const right = segment.mode === "split" && item.anchor?.side === "RIGHT";
        const key = `${after}:${right}`;
        const mark = marks.get(key) ?? { row: after, header: after === segment.start, right, settled: true, threads: [] };
        mark.threads.push(item.hidden);
        mark.settled &&= item.hidden.settled;
        marks.set(key, mark);
        continue;
      }
      out.push({ key: item.key, after, height: heights.get(item.key) ?? 72, item });
    }
    return { placed: out, marks: [...marks.values()], tallies };
  }, [items, located, viewId, baseLayout, summary, heights]);

  // ---------- line wrap ----------
  const gutterChars = Math.max(3, String(summary.max_line_number).length);
  /** Code columns per line when wrapping (see the .wrap rules in styles.css for the paddings). */
  const columns = useMemo(() => {
    if (!wrap || !charWidth || !viewportWidth) return null;
    const fit = (px: number) => Math.max(20, Math.floor(px / charWidth));
    // Room on the right for unified PR tags (see .pr-tag: 10px text, 6px padding, 8px from the edge), 8px clear of the code.
    const longest = Math.max(...prs.map((pr) => String(pr.number).length));
    const tagRoom = multiPr ? Math.ceil(((longest + 1) * charWidth * 10) / 12) + 12 + 8 : 0;
    return {
      unified: fit(viewportWidth - (gutterChars * 2 * charWidth + 44) - WRAP_PAD - tagRoom),
      split: fit(viewportWidth / 2 - 1 - (gutterChars * charWidth + 30) - WRAP_PAD),
      tagRoom,
    };
  }, [wrap, charWidth, viewportWidth, gutterChars, multiPr, prs]);
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
      if (mode === "collapsed" || mode === "hidden") return;
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
      const known = columns && (mode === "unified" || mode === "split") ? widths?.get(`${file}:${mode}`) : undefined;
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
  /** The layout now, for code that continues after an await. */
  const latest = useRef({ layout, summary });
  latest.current = { layout, summary };

  /**
   * Remembers the reader's place (the row at the top), which layout changes keep. A scroll made
   * here records it right away: a layout change can land before the scroll event (wrapped row
   * widths arriving for a file just switched to side by side) and must keep the new place.
   */
  const recordAnchor = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const { layout, summary } = latest.current;
    const row = layout.rowAt(el.scrollTop);
    const segment = layout.segmentAt(row);
    anchor.current = { path: summary.files[segment.file].path, offset: row - segment.start, mode: segment.mode, delta: el.scrollTop - layout.rowY(row) };
  }, []);

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

  /** Scrolls to head line `line` of view file `file`, switching it to side by side if only the
   * full file has that line; flashes the rows up to head line `end`. */
  const jumpToLine = useCallback(
    async (file: number, line: number, end = line) => {
      const [location, last] = await Promise.all([locateLine(viewId, file, line), end > line ? locateLine(viewId, file, end) : null]);
      if (!location) return;
      const { layout } = latest.current;
      const path = summary.files[file].path;
      const segment = layout.segments[file];
      const needsSplit = segment.mode === "collapsed" || (segment.mode === "unified" && location.unified === null);
      if (needsSplit) {
        pendingJump.current = { file, line, end };
        setExpanded((s) => new Set(s).add(path));
        setCollapsed((s) => withOut(s, path));
        if (location.unified === null) setOverrides((m) => new Map(m).set(path, "split"));
        return;
      }
      const offset = (segment.mode === "split" ? location.split : location.unified) ?? 0;
      const to = (last && (segment.mode === "split" ? last.split : last.unified)) ?? offset;
      const el = scrollRef.current;
      stopGlide();
      if (el) el.scrollTop = Math.max(0, layout.rowY(segment.start + offset) - el.clientHeight / 3);
      recordAnchor();
      setFlash({ file, from: offset, to: Math.max(offset, to) });
    },
    [viewId, summary, stopGlide, recordAnchor],
  );

  useEffect(() => {
    const jump = pendingJump.current;
    if (!jump) return;
    pendingJump.current = null;
    void jumpToLine(jump.file, jump.line, jump.end);
  }, [layout, jumpToLine]);

  useEffect(() => {
    if (!flash) return;
    const timer = window.setTimeout(() => setFlash(null), 1400);
    return () => window.clearTimeout(timer);
  }, [flash]);

  const jump = useCallback(
    (target: { path: string; line: number; end?: number; file?: number }) => {
      const file = target.file ?? summary.files.findIndex((f) => f.path === target.path);
      // Files left out by the filter open in the file view, like files outside the diff.
      if (file >= 0 && summary.files[file].new_blob && layout.segments[file].mode !== "hidden") {
        void jumpToLine(file, target.line, target.end);
        return;
      }
      setFileView({ path: target.path, line: target.line, end: target.end ?? target.line, lines: null, error: null });
      readFile(stackId, hi, target.path)
        .then((lines) => setFileView((v) => (v?.path === target.path ? { ...v, lines } : v)))
        .catch((e) => setFileView((v) => (v?.path === target.path ? { ...v, error: String(e) } : v)));
    },
    [summary, layout, jumpToLine, stackId, hi],
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
  /** After `v` or collapsing (e, Space): once `from` has collapsed, put `next`'s header at the top. */
  const pendingFile = useRef<{ from: number; next: number } | null>(null);
  /** After toggling a file (s, expanding, un-viewing): it stays the current file once the layout changes. */
  const keepFile = useRef<number | null>(null);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const place = anchor.current;
    const index = place ? summary.files.findIndex((f) => f.path === place.path) : -1;
    let top = 0;
    const moveOn = pendingFile.current;
    let pin: number | null = null;
    if (moveOn && previousView.current === viewId && ["collapsed", "hidden"].includes(layout.segments[moveOn.from]?.mode)) {
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
      selection: {
        path: summary.files[file].path,
        prLabel,
        startLine: start,
        endLine: end,
        text: lines.join("\n"),
        headStart: heads.length ? Math.min(...heads) : null,
        headEnd: heads.length ? Math.max(...heads) : null,
      },
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
      setPendingAnswer({ threadId, question, text: "", comments: [] });
      // Files hidden by the filter are named to the assistant but their diff is left out.
      const hidden = summary.files.filter((_, i) => filtered[i]).map((f) => f.path);
      const newThread = threadId ? null : { ...choice, selection: assistant.context.selection, anchor: assistant.context.anchor, hidden };
      askAssistant(stackId, lo, hi, threadId, newThread, question, (event) => {
        if (event.kind === "delta") setPendingAnswer((p) => p && { ...p, text: p.text + event.text });
        else if (event.kind === "text") setPendingAnswer((p) => p && { ...p, text: event.text });
        else if (event.kind === "comment") {
          // The assistant changed the drafts mid-answer: show them right away.
          setPendingAnswer((p) => p && { ...p, comments: [...p.comments, event.outcome] });
          if (!event.outcome.error) onDraftsChanged();
        }
      })
        .then((thread) => {
          setAssistantThreads((list) => [...list.filter((t) => t.id !== thread.id), thread]);
          const comments = thread.messages[thread.messages.length - 1]?.comments ?? [];
          const note = comments.length ? commentsNote(comments) : null;
          setAssistant((a) => a && { ...a, activeId: thread.id, note });
        })
        .catch((e) => setAssistant((a) => a && { ...a, note: String(e) }))
        .finally(() => setPendingAnswer(null));
    },
    [assistant, stackId, lo, hi, summary, filtered, onDraftsChanged],
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
    if (segment.mode === "unified" || segment.mode === "split") store.ensure(segment.file, segment.mode, row - segment.start, end - segment.start);
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
      (flash !== null && flash.file === segment.file && offset >= flash.from && offset <= flash.to);
    if (offset === 0) {
      rendered.push(
        <FileHeader
          key={key}
          y={y}
          file={file}
          mode={segment.mode}
          onComment={() => openFileComposer(segment.file)}
          hidden={marks.find((m) => m.header && m.row === row)}
          onShowHidden={showThreads}
        />,
      );
    } else if (segment.mode === "collapsed") {
      const why = file.noise ?? (isViewed(file.content_key) ? "viewed" : isIgnored(file.path) ? "ignored" : "collapsed");
      rendered.push(
        <CollapsedNotice
          key={key}
          y={y}
          file={file}
          why={why}
          tally={tallies.get(segment.file)}
          onExpand={() => {
            setExpanded((s) => new Set(s).add(file.path));
            setCollapsed((s) => withOut(s, file.path));
          }}
        />,
      );
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
  for (const mark of marks) {
    if (mark.header || mark.row < first || mark.row >= last) continue;
    rendered.push(<CommentMark key={`mark:${mark.row}:${mark.right}`} y={layout.rowY(mark.row)} mark={mark} onShow={showThreads} />);
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
    // First, as drawing the new rows can change the layout (threads measured), which keeps this place.
    recordAnchor();
    // Draw the rows for the new position right away; a deferred render shows up as blank frames.
    flushSync(() => setScrollTop(el.scrollTop));
    // Horizontal scrolling moves side-by-side code within its halves (no re-render needed).
    canvasRef.current?.style.setProperty("--split-x", `${el.scrollLeft}px`);
  }, [recordAnchor]);

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
      recordAnchor();
      if (pin !== undefined) setPinned({ file: pin, top: el.scrollTop });
    },
    [layout, stopGlide, recordAnchor],
  );

  /** Repo files outside the diff that answers name, and whether they exist at the range head.
   * Paths are checked in batches; until a path's answer is back it isn't a link. */
  const [repoFiles, setRepoFiles] = useState({ summary, exists: new Map<string, boolean>() });
  const asked = useRef({ summary, paths: new Set<string>(), queue: [] as string[] });
  const checkRepoFile = useCallback(
    (path: string) => {
      if (asked.current.summary !== summary) asked.current = { summary, paths: new Set(), queue: [] };
      const batch = asked.current;
      if (batch.paths.has(path)) return;
      batch.paths.add(path);
      batch.queue.push(path);
      if (batch.queue.length > 1) return;
      window.setTimeout(() => {
        const paths = batch.queue.splice(0);
        existingFiles(stackId, hi, paths)
          .then((found) =>
            setRepoFiles((r) => {
              const exists = new Map(r.summary === summary ? r.exists : []);
              for (const p of paths) exists.set(p, found.includes(p));
              return { summary, exists };
            }),
          )
          .catch(() => {});
      });
    },
    [summary, stackId, hi],
  );
  const repoFileExists = repoFiles.summary === summary ? repoFiles.exists : null;

  /** File references in assistant answers: files of the diff (a bare name if only one file has
   * it), or other files of the head, which open in the file view. */
  const openRef = useRef((_: CodeRef) => {});
  openRef.current = (ref) => {
    const file = summary.files.findIndex((f) => f.path === ref.path);
    if (ref.line === null && file >= 0 && layout.segments[file].mode !== "hidden") scrollToRow(layout.segments[file].start, 0, file);
    else jump({ path: ref.path, line: ref.line ?? 0, end: ref.end ?? undefined });
  };
  const codeRefs = useMemo(() => {
    const paths = new Set(summary.files.map((f) => f.path));
    const known = (path: string) => {
      if (paths.has(path)) return path;
      const matches = [...paths].filter((p) => p.endsWith(`/${path}`));
      return matches.length === 1 ? matches[0] : null;
    };
    return {
      resolve: (ref: CodeRef): CodeRef | null => {
        const path = known(ref.path) ?? known(ref.path.replace(/^[ab]\//, ""));
        if (path) return { ...ref, path };
        const exists = repoFileExists?.get(ref.path);
        if (exists === undefined) checkRepoFile(ref.path);
        return exists ? ref : null;
      },
      open: (ref: CodeRef) => openRef.current(ref),
    };
  }, [summary, repoFileExists, checkRepoFile]);

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

  const changeFilter = useCallback(
    (next: FileFilter) =>
      changeLayout(() =>
        setFilters((current) => {
          const all = { ...current };
          delete all[filterKey];
          if (next.exts.length) all[filterKey] = next;
          const kept = Object.fromEntries(Object.entries(all).slice(-300));
          saveRecord("fileFilters", kept);
          return kept;
        }),
      ),
    [changeLayout, filterKey],
  );
  const filteredCount = useMemo(() => filtered.filter(Boolean).length, [filtered]);

  useEffect(() => {
    if (!keyboardEnabled) return;
    const onKey = (e: KeyboardEvent) => {
      // Escape works from anywhere (e.g. the assistant's model picker); text fields handle their own.
      if (e.metaKey || e.ctrlKey || e.altKey || (isTyping(e.target) && e.key !== "Escape")) return;
      const el = scrollRef.current;
      if (!el) return;
      // Every file filtered out: nothing for the file keys to act on.
      if (!layout.totalRows && ["s", "e", " ", "c", "v", "u", "x"].includes(e.key)) return;
      // While gliding, j/k continue from where the glide is heading.
      const heading = glide.current?.target ?? el.scrollTop;
      const anchorRow = layout.rowAt(heading) + JUMP_MARGIN;
      // Files: the header of the file being read sits at the top after n/p.
      const here = fileAt(el.scrollTop);
      const topRow = here === layout.segments[pinned?.file ?? -1] ? here.start : layout.rowAt(heading);
      const starts = layout.segments.filter((s) => s.mode !== "hidden").map((s) => s.start);
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
        case "h":
          changeLayout(() => onCommentsHiddenChange(rangeKeys, !rangeHidden));
          break;
        case "t":
          toggleListMode();
          break;
        case "f":
          onShowFiles();
          setFilterOpen((o) => !o);
          break;
        case "F":
          if (filter.exts.length) changeFilter({ ...filter, off: !filter.off });
          break;
        case "x": {
          // Hides the current file's extension and moves on to the next file still shown.
          const ext = extensionOf(summary.files[here.file].path);
          const exts = [...new Set([...filter.exts, ext])];
          const next = summary.files.findIndex((f, i) => i > here.file && !exts.includes(extensionOf(f.path)));
          if (next >= 0) pendingFile.current = { from: here.file, next };
          changeFilter({ exts });
          break;
        }
        case "S":
          changeLayout(() => {
            setOverrides(new Map());
            onDefaultModeChange(defaultMode === "split" ? "unified" : "split");
          });
          break;
        case "e":
        case " ": {
          // Collapse/expand without touching the viewed mark. Collapsing moves on to the next
          // file that's still open, like `v`.
          const next =
            here.mode === "collapsed" ? -1 : layout.segments.findIndex((s) => s.file > here.file && s.mode !== "collapsed" && s.mode !== "hidden");
          if (next >= 0) pendingFile.current = { from: here.file, next };
          else keepFile.current = here.file;
          toggleCollapsed(here);
          break;
        }
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
            const next = viewed ? summary.files.findIndex((f, i) => i > here.file && !isViewed(f.content_key) && !filtered[i]) : -1;
            if (next >= 0) pendingFile.current = { from: here.file, next };
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
  }, [keyboardEnabled, layout, summary, fileAt, pinned, scrollToRow, glideTo, glideToRow, selection, toggleFileMode, toggleCollapsed, changeLayout, defaultMode, onDefaultModeChange, wrap, onWrapChange, rangeKeys, rangeHidden, onCommentsHiddenChange, openComposer, openFileComposer, isViewed, onToggleViewed, openUsages, panel, assistant, openAssistant, composer, toggleListMode, filtered, filter, changeFilter, onShowFiles]);

  return (
    <div className="diff-layout">
      {showFiles && (
        <FileList
          files={summary.files}
          isViewed={isViewed}
          segments={layout.segments}
          current={current.file}
          showPrs={multiPr}
          openThreads={(i) => tallies.get(i)?.get("open") ?? 0}
          mode={listMode}
          onToggleMode={toggleListMode}
          onSelect={(index) => scrollToRow(layout.segments[index].start, 0, index)}
          filtered={filtered}
          filteredCount={filteredCount}
          filter={filter}
          onFilterChange={changeFilter}
          filterOpen={filterOpen}
          onFilterOpenChange={setFilterOpen}
        />
      )}
      <div className="diff-main">
        {filteredCount > 0 && filteredCount === summary.files.length && (
          <div className="diff-all-filtered" data-testid="all-filtered">
            {filteredCount === 1 ? "The only file is" : `All ${filteredCount} files are`} hidden by the file filter.{" "}
            <button onClick={() => changeFilter(NO_FILTER)}>Show all files</button>
          </div>
        )}
        <div
          className="diff-scroll"
          ref={scrollRef}
          onScroll={onScroll}
          onWheel={stopGlide}
          data-testid="diff-scroll"
          onMouseMove={(e) => (lastPointer.current = { x: e.clientX, y: e.clientY })}
          onClick={(e) => {
            if (!mod(e)) return;
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
                "--wrap-tag-room": columns && `${columns.tagRoom}px`,
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
          onDelete={(id) => {
            void deleteAssistantThread(id);
            setAssistantThreads((list) => list.filter((t) => t.id !== id));
            setAssistant((a) => a && { ...a, activeId: null });
          }}
          onClose={() => setAssistant(null)}
          codeRefs={codeRefs}
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
/** Horizontal padding of a wrapped line's code. */
const WRAP_PAD = 8 + 8;

interface FileHeaderProps {
  y: number;
  file: FileSummary;
  mode: Mode;
  onComment: () => void;
  /** Hidden file-level (or outdated) threads of this file. */
  hidden?: CommentMarkItem;
  onShowHidden: (threadIds: string[]) => void;
}

const FileHeader = memo(function FileHeader({ y, file, mode, onComment, hidden, onShowHidden }: FileHeaderProps) {
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
      {hidden && (
        <button
          className={`file-hidden-comments${hidden.settled ? " settled" : ""}`}
          title={markTitle(hidden)}
          onClick={() => onShowHidden(hidden.threads.map((t) => t.id))}
        >
          <CommentIcon settled={hidden.settled} />
          {hidden.threads.length}
        </button>
      )}
    </div>
  );
});

/** Hidden threads that share a row (and side): an icon at the gutter's edge that shows them again. */
interface CommentMarkItem {
  row: number;
  /** On the file header (file-level threads, outdated ones that can't be placed). */
  header: boolean;
  /** On the right half, side by side. */
  right: boolean;
  /** Every thread here is one GitHub folds away (resolved, outdated, minimized). */
  settled: boolean;
  threads: HiddenThread[];
}

interface HiddenThread {
  id: string;
  preview: string;
  settled: boolean;
}

const markTitle = (mark: CommentMarkItem) => `${mark.threads.map((t) => t.preview).join("\n")}\n\nClick to show`;

function CommentMark({ y, mark, onShow }: { y: number; mark: CommentMarkItem; onShow: (threadIds: string[]) => void }) {
  return (
    <button
      className={`comment-mark${mark.right ? " right" : ""}${mark.settled ? " settled" : ""}`}
      style={{ transform: `translate(var(--split-x, 0px), ${y}px)` }}
      title={markTitle(mark)}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={() => onShow(mark.threads.map((t) => t.id))}
    >
      <CommentIcon settled={mark.settled} />
    </button>
  );
}

/** A speech bubble; with a check mark for settled threads. */
const CommentIcon = ({ settled }: { settled: boolean }) => (
  <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden>
    <path fill="currentColor" d="M2 2.5A1.5 1.5 0 0 1 3.5 1h9A1.5 1.5 0 0 1 14 2.5v7a1.5 1.5 0 0 1-1.5 1.5H7.4l-3 2.8A.75.75 0 0 1 3 13.25V11A1.5 1.5 0 0 1 2 9.5Z" />
    {settled && <path d="m5 6.2 2 2 4-4" fill="none" stroke="var(--bg)" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />}
  </svg>
);

interface CollapsedNoticeProps {
  y: number;
  file: FileSummary;
  why: string;
  /** The file's threads and drafts, by tally. */
  tally?: Map<string, number>;
  onExpand: () => void;
}

function CollapsedNotice({ y, file, why, tally, onExpand }: CollapsedNoticeProps) {
  const count = (key: string) => tally?.get(key) ?? 0;
  const settled = [...(tally ?? [])].filter(([key]) => key !== "open" && key !== "draft");
  const drafts = count("draft");
  return (
    <div className="row row-collapsed" style={at(y)} onClick={onExpand} data-file={file.path}>
      <span className="gutter" />
      <span className="code">
        {why} · +{file.additions} −{file.deletions}
        {count("open") > 0 && (
          <span className="collapsed-tally open">
            {" · "}
            <CommentIcon settled={false} /> {count("open")} open
          </span>
        )}
        {settled.map(([reason, n]) => (
          <span className="collapsed-tally" key={reason}>
            {" · "}
            <CommentIcon settled /> {n} {reason.toLowerCase()}
          </span>
        ))}
        {drafts > 0 && ` · ${drafts} draft${drafts === 1 ? "" : "s"}`} · click or press {why === "viewed" ? "v" : "e"} to expand
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

const LIST_MODES = ["list", "tree"] as const;
type ListMode = (typeof LIST_MODES)[number];

interface FileListProps {
  files: FileSummary[];
  isViewed: (key: string) => boolean;
  segments: Segment[];
  current: number;
  showPrs: boolean;
  /** Open (not settled) review threads, by file index. */
  openThreads: (file: number) => number;
  mode: ListMode;
  onToggleMode: () => void;
  onSelect: (index: number) => void;
  /** Per file index: left out by the filter (not listed). */
  filtered: boolean[];
  filteredCount: number;
  filter: FileFilter;
  onFilterChange: (filter: FileFilter) => void;
  filterOpen: boolean;
  onFilterOpenChange: (open: boolean) => void;
}

function FileList(props: FileListProps) {
  const { files, isViewed, segments, current, showPrs, openThreads, mode, onToggleMode, onSelect, filtered, filteredCount, filter, onFilterChange } = props;
  const { filterOpen, onFilterOpenChange } = props;
  const activeRef = useRef<HTMLLIElement>(null);
  const width = useSidebarWidth("files", 280);
  const tree = useMemo(() => buildTree(files.map((f, i) => (filtered[i] ? null : f.path))), [files, filtered]);
  const [closed, setClosed] = useState<Set<string>>(new Set());
  // A directory folds once every file in it is viewed.
  const done = useMemo(() => allDirs(tree).filter((d) => d.files.every((i) => isViewed(files[i].content_key))).map((d) => d.path), [tree, files, isViewed]);
  const wasDone = useRef<Set<string>>(new Set());
  useEffect(() => {
    const newly = done.filter((d) => !wasDone.current.has(d));
    wasDone.current = new Set(done);
    if (newly.length) setClosed((c) => new Set([...c, ...newly]));
  }, [done]);
  // The current file's directories open when it is reached (n/p, v, scrolling…).
  const currentPath = files[current]?.path;
  useEffect(() => {
    if (!currentPath) return;
    setClosed((c) => {
      const inside = ancestors(currentPath).filter((d) => c.has(d));
      return inside.length ? new Set([...c].filter((d) => !inside.includes(d))) : c;
    });
  }, [currentPath]);
  useEffect(() => activeRef.current?.scrollIntoView({ block: "nearest" }), [current, mode]);
  const toggleDir = (path: string) => setClosed((c) => (c.has(path) ? withOut(c, path) : new Set(c).add(path)));

  const fileRow = (i: number, depth?: number) => {
    const f = files[i];
    const viewed = isViewed(f.content_key);
    return (
      <li
        key={f.path + i}
        ref={i === current ? activeRef : undefined}
        className={[i === current && "active", segments[i]?.mode === "collapsed" && "muted", viewed && "viewed"].filter(Boolean).join(" ")}
        style={depth === undefined ? undefined : { paddingLeft: indent(depth) }}
        onClick={() => onSelect(i)}
        title={f.path}
      >
        <span className={`file-status status-${f.status}`}>{viewed ? "✓" : statusLetter(f.status)}</span>
        <span className="file-name">
          {depth === undefined && <span className="file-dir">{dirname(f.path)}</span>}
          {basename(f.path)}
        </span>
        {showPrs && (
          <span className="file-prs">
            {f.prs.map((p) => (
              <span key={p} className="file-pr-dot" style={{ background: prColor(p) }} />
            ))}
          </span>
        )}
        {openThreads(i) > 0 && (
          <span className="file-open-threads" title={`${openThreads(i)} open thread${openThreads(i) === 1 ? "" : "s"}`}>
            <CommentIcon settled={false} />
            {openThreads(i)}
          </span>
        )}
        <span className="file-counts">
          {f.additions > 0 && <span className="add">+{f.additions}</span>}
          {f.deletions > 0 && <span className="del">−{f.deletions}</span>}
        </span>
      </li>
    );
  };

  return (
    <nav className="file-list" style={{ width: width.width }}>
      <ResizeHandle edge="right" {...width} />
      <ul>
        {mode === "list"
          ? files.map((_, i) => !filtered[i] && fileRow(i))
          : treeRows(tree, closed).map((row) => {
              if ("file" in row) return fileRow(row.file, row.depth);
              const { dir } = row;
              const open = !closed.has(dir.path);
              const allViewed = dir.files.every((i) => isViewed(files[i].content_key));
              return (
                <li
                  key={`dir:${dir.path}`}
                  className={["file-tree-dir", allViewed && "viewed"].filter(Boolean).join(" ")}
                  style={{ paddingLeft: indent(row.depth) }}
                  onClick={() => toggleDir(dir.path)}
                  data-dir={dir.path}
                >
                  <span className="file-status">{open ? "▾" : "▸"}</span>
                  <span className="file-name">{dir.name}</span>
                  {allViewed ? <span className="file-tree-done">✓</span> : !open && <span className="file-tree-count">{dir.files.length}</span>}
                </li>
              );
            })}
      </ul>
      <div className="file-list-footer">
        <span className="file-list-modes" role="group" aria-label="File list layout">
          {LIST_MODES.map((m) => (
            <button
              key={m}
              className={m === mode ? "active" : undefined}
              onClick={() => m !== mode && onToggleMode()}
              title={`${m === "list" ? "Flat list" : "Directory tree"} (t)`}
              data-testid={`file-list-${m}`}
            >
              {m === "list" ? <ListIcon /> : <TreeIcon />}
            </button>
          ))}
        </span>
        <FileFilterMenu
          files={files}
          filter={filter}
          filteredCount={filteredCount}
          onChange={onFilterChange}
          open={filterOpen}
          onOpenChange={onFilterOpenChange}
        />
      </div>
    </nav>
  );
}

/** Files left out of the diff by extension (like GitHub's file filter). */
interface FileFilter {
  /** Hidden extensions ("" = files without one). */
  exts: string[];
  /** Switched off with `F`: everything shown, the extensions kept for switching back on. */
  off?: boolean;
}

const NO_FILTER: FileFilter = { exts: [] };

/** A remembered filter, checked (storage may hold anything). */
function toFileFilter(value: unknown): FileFilter {
  if (!value || typeof value !== "object") return NO_FILTER;
  const v = value as Partial<FileFilter>;
  return { exts: Array.isArray(v.exts) ? v.exts.filter((e): e is string => typeof e === "string") : [], off: v.off === true };
}

/** ".php" for "src/a.test.php"; "" without one (dotfiles like ".gitignore" included). */
const extensionOf = (path: string) => {
  const name = basename(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot) : "";
};

interface FileFilterMenuProps {
  files: FileSummary[];
  filter: FileFilter;
  filteredCount: number;
  onChange: (filter: FileFilter) => void;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function FileFilterMenu({ files, filter, filteredCount, onChange, open, onOpenChange }: FileFilterMenuProps) {
  const ref = useRef<HTMLSpanElement>(null);
  /** The highlighted extension (↑/↓ move it, Space/Enter toggle it). */
  const [highlight, setHighlight] = useState(0);
  const exts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const f of files) counts.set(extensionOf(f.path), (counts.get(extensionOf(f.path)) ?? 0) + 1);
    // Files without an extension last.
    return [...counts].sort(([a], [b]) => (a === "" ? 1 : b === "" ? -1 : a.localeCompare(b)));
  }, [files]);
  const active = filter.exts.length > 0 && !filter.off;
  // Ticking an extension (or "Show all") switches a filter that was off back on.
  const toggle = useCallback(
    (ext: string) => onChange({ exts: filter.exts.includes(ext) ? filter.exts.filter((e) => e !== ext) : [...filter.exts, ext] }),
    [filter, onChange],
  );

  useEffect(() => {
    if (open) setHighlight(0);
  }, [open]);

  // Closes on a click elsewhere, or Esc; ↑/↓ and Space/Enter work the list (all before the
  // diff's own key handling).
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onOpenChange(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "Escape") onOpenChange(false);
      else if (e.key === "ArrowDown") setHighlight((h) => Math.min(h + 1, exts.length - 1));
      else if (e.key === "ArrowUp") setHighlight((h) => Math.max(h - 1, 0));
      else if ((e.key === " " || e.key === "Enter") && exts[highlight]) toggle(exts[highlight][0]);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open, onOpenChange, exts, highlight, toggle]);

  const item = (index: number, label: string, count: number, shown: boolean, onClick: () => void, testId: string) => (
    <button
      key={testId}
      role="menuitemcheckbox"
      aria-checked={shown}
      className={index === highlight ? "highlighted" : undefined}
      onMouseEnter={() => setHighlight(index)}
      onClick={onClick}
      data-testid={testId}
    >
      <span className="file-filter-check">{shown ? "✓" : ""}</span>
      <span className="file-filter-label">{label}</span>
      <span className="file-filter-count">{count}</span>
    </button>
  );

  return (
    <span className="file-filter" ref={ref}>
      <button
        className={`file-filter-button${active ? " active" : ""}`}
        onClick={() => onOpenChange(!open)}
        title={filter.off ? "Filter files (f) — switched off, F switches it back on" : "Filter files (f)"}
        aria-expanded={open}
        data-testid="file-filter"
      >
        <FilterIcon />
        {filteredCount > 0 && <span>{filteredCount} hidden</span>}
        {filter.off && filter.exts.length > 0 && <span>filter off</span>}
      </button>
      {open && (
        <div className="file-filter-menu" role="menu" data-testid="file-filter-menu">
          <div className="file-filter-title">
            File extensions
            {filter.exts.length > 0 && (
              <button className="file-filter-reset" onClick={() => onChange(NO_FILTER)} data-testid="file-filter-reset">
                Show all
              </button>
            )}
          </div>
          {exts.map(([ext, count], i) => item(i, ext || "No extension", count, !filter.exts.includes(ext), () => toggle(ext), `file-filter-ext${ext}`))}
        </div>
      )}
    </span>
  );
}

const FilterIcon = () => (
  <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
    <path d="M2 3.5h10M4 7h6M6 10.5h2" />
  </svg>
);

const indent = (depth: number) => 12 + depth * 8;

const ListIcon = () => (
  <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
    <path d="M2 3.5h10M2 7h10M2 10.5h10" />
  </svg>
);

const TreeIcon = () => (
  <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
    <path d="M2 2.5h6M5 6h7M5 9.5h7M3 3v6.5h2M3 6h2" />
  </svg>
);

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

/** "Added 2 draft comments · updated 1 · 1 failed" */
function commentsNote(comments: CommentOutcome[]) {
  const count = (action: string) => comments.filter((c) => !c.error && (c.action ?? "add") === action).length;
  const parts = ([["Added", "add"], ["Updated", "edit"], ["Deleted", "delete"]] as const)
    .map(([verb, action]) => [verb, count(action)] as const)
    .filter(([, n]) => n > 0)
    .map(([verb, n], i) => (i === 0 ? `${verb} ${n} draft comment${n === 1 ? "" : "s"}` : `${verb.toLowerCase()} ${n}`));
  const failed = comments.filter((c) => c.error).length;
  if (failed) parts.push(`${failed} failed`);
  return parts.join(" · ");
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
  /** A hidden thread: drawn as a gutter icon instead. */
  hidden: HiddenThread | null;
  /** What a collapsed file counts this as: "open", a settled reason ("Resolved", …), or "draft"; null: always shown. */
  tally: string | null;
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
  isHidden: (thread: ReviewThread, prIndex: number) => boolean;
  hideThread: (threadId: string) => void;
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
        hidden: src.isHidden(thread, prIndex) ? { id: thread.id, preview: threadPreview(thread), settled: settledReason(thread) !== null } : null,
        tally: settledReason(thread) ?? "open",
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
            onHide={() => src.hideThread(thread.id)}
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
      hidden: null,
      tally: "draft",
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
      hidden: null,
      tally: null,
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