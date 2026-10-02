import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  getIgnorePatterns,
  getViewed,
  listCheckpoints,
  listDrafts,
  listThreads,
  markReviewed,
  openPr,
  refreshPr,
  selectRange,
  selectSince,
  setIgnorePatterns,
  setViewed,
} from "./api";
import type { PrThreads, ShownDraft } from "./comments";
import { SubmitSheet } from "./SubmitSheet";
import { CommandPalette, type Command } from "./CommandPalette";
import { DiffViewer, type BaseMode } from "./DiffViewer";
import { globMatcher } from "./glob";
import { Inbox, type InboxEntry } from "./Inbox";
import { Mark } from "./Logo";
import { ShortcutsHelp, ThemePicker } from "./Overlays";
import { mark } from "./perf";
import { loadPref, savePref } from "./prefs";
import { rememberPr } from "./recent";
import { rangeForKey, StackBar } from "./StackBar";
import { prLabel, type Checkpoint, type OpenedRange, type OpenedStack, type Range } from "./types";
import "./styles.css";

/** Which ranges the backend has precomputed, per stack (`"lo-hi"` keys). */
const readyRanges = new Map<string, Set<string>>();

export default function App() {
  const [stack, setStack] = useState<OpenedStack | null>(null);
  const [shown, setShown] = useState<OpenedRange | null>(null);
  const [newerVersion, setNewerVersion] = useState<OpenedStack | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [home, setHome] = useState(true);
  const [inbox, setInbox] = useState<InboxEntry[]>([]);
  const [online, setOnline] = useState<boolean | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showFiles, setShowFiles] = useState(true);
  const [defaultMode, setDefaultMode] = useState<BaseMode>(() => loadPref("defaultMode", ["unified", "split"] as const, "unified"));
  const [wrap, setWrap] = useState(() => loadPref("wrap", ["on", "off"] as const, "off") === "on");
  const [ignorePatterns, setIgnore] = useState<string[]>([]);
  const [drafts, setDrafts] = useState<ShownDraft[]>([]);
  const [threads, setThreads] = useState<PrThreads[]>([]);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [overlay, setOverlay] = useState<"theme" | "shortcuts" | null>(null);
  const [viewed, setViewedKeys] = useState<Set<string>>(new Set());
  const [checkpoints, setCheckpoints] = useState<Checkpoint[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);
  const say = useCallback((message: string) => {
    setToast(message);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 3500);
  }, []);
  const [, setReadyVersion] = useState(0);
  const rangeRequest = useRef(0);

  useEffect(() => {
    const unlisten = listen<{ stackId: string; lo: number; hi: number }>("range-ready", ({ payload }) => {
      const set = readyRanges.get(payload.stackId) ?? new Set<string>();
      set.add(`${payload.lo}-${payload.hi}`);
      readyRanges.set(payload.stackId, set);
      setReadyVersion((v) => v + 1);
    });
    return () => void unlisten.then((f) => f());
  }, []);

  // The open stack, for checking it against GitHub whenever the background refresh runs.
  const openStack = useRef<{ label: string; stackId: string } | null>(null);
  const checking = useRef(false);
  const checkForNewCommits = useCallback(() => {
    const open = openStack.current;
    if (!open || checking.current) return;
    checking.current = true;
    refreshPr(open.label, open.stackId)
      .then((fresh) => fresh && openStack.current?.stackId === open.stackId && setNewerVersion(fresh))
      .catch(() => undefined)
      .finally(() => (checking.current = false));
  }, []);

  useEffect(() => {
    invoke<InboxEntry[]>("get_inbox")
      .then(setInbox)
      .catch(() => undefined);
    const updates = listen<InboxEntry[]>("inbox-updated", ({ payload }) => setInbox(payload));
    // Each background refresh (launch, every 5 minutes, window focus) also checks the open PR.
    const status = listen<string>("inbox-status", ({ payload }) => {
      setOnline(payload === "online");
      if (payload === "online") checkForNewCommits();
    });
    return () => {
      void updates.then((f) => f());
      void status.then((f) => f());
    };
  }, [checkForNewCommits]);
  const refreshInbox = useCallback(() => void invoke("refresh_inbox"), []);

  useEffect(() => {
    window.__wispyRanges = {
      readyCount: (id) => readyRanges.get(id)?.size ?? 0,
      current: () => stack?.stackId ?? null,
    };
  }, [stack]);

  const show = useCallback((opened: OpenedStack) => {
    setStack(opened);
    setShown(opened);
    setNewerVersion(null);
  }, []);

  useEffect(() => {
    openStack.current = stack ? { label: prLabel(stack.prs[stack.focus]), stackId: stack.stackId } : null;
  }, [stack]);

  const stackId = stack?.stackId ?? null;
  const refreshDrafts = useCallback(() => {
    if (!stackId) return;
    listDrafts(stackId)
      .then(setDrafts)
      .catch((e) => console.error("drafts", e));
  }, [stackId]);
  useEffect(() => {
    if (!stackId) return;
    setDrafts([]);
    setThreads([]);
    refreshDrafts();
    // Cached threads first (offline-friendly), then GitHub's current state.
    listThreads(stackId, false)
      .then(setThreads)
      .catch(() => undefined);
    listThreads(stackId, true)
      .then(setThreads)
      .catch(() => undefined);
  }, [stackId, refreshDrafts]);

  const repo = stack?.prs[0].base_repo ?? null;
  useEffect(() => {
    if (!repo) return;
    getIgnorePatterns(repo)
      .then(setIgnore)
      .catch(() => setIgnore([]));
  }, [repo]);
  const isIgnored = useMemo(() => globMatcher(ignorePatterns), [ignorePatterns]);

  useEffect(() => {
    if (!repo) return;
    getViewed(repo)
      .then((keys) => setViewedKeys(new Set(keys)))
      .catch(() => setViewedKeys(new Set()));
  }, [repo]);
  const isViewed = useCallback((key: string) => viewed.has(key), [viewed]);
  const toggleViewed = useCallback(
    (key: string, value: boolean) => {
      if (!repo) return;
      setViewedKeys((current) => {
        const next = new Set(current);
        if (value) next.add(key);
        else next.delete(key);
        return next;
      });
      void setViewed(repo, key, value);
    },
    [repo],
  );

  const updateIgnore = useCallback(
    (patterns: string[]) => {
      if (!repo) return;
      setIgnore(patterns);
      void setIgnorePatterns(repo, patterns);
    },
    [repo],
  );

  const changeDefaultMode = useCallback((mode: BaseMode) => {
    setDefaultMode(mode);
    savePref("defaultMode", mode);
  }, []);
  const changeWrap = useCallback((on: boolean) => {
    setWrap(on);
    savePref("wrap", on ? "on" : "off");
  }, []);

  const open = useCallback(
    async (input: string) => {
      mark("open:start");
      setBusy(input);
      setError(null);
      try {
        const result = await openPr(input);
        const label = prLabel(result.prs[result.focus]);
        rememberPr(label);
        show(result);
        setPaletteOpen(false);
        setHome(false);
        if (result.fromCache) {
          // Stale-while-revalidate: show the cached stack now, offer the new one if anything moved.
          refreshPr(label, result.stackId)
            .then((fresh) => fresh && setNewerVersion(fresh))
            .catch(() => {
              /* offline: the cached version is all we have */
            });
        }
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(null);
      }
    },
    [show],
  );

  const chooseRange = useCallback(
    async (range: Range, ignoreWhitespace = shown?.ignoreWhitespace ?? false) => {
      if (!stack) return;
      mark("range:start");
      const request = ++rangeRequest.current;
      try {
        const result = await selectRange(stack.stackId, range.lo, range.hi, ignoreWhitespace);
        // A later selection wins even if its response arrives first.
        if (request === rangeRequest.current) setShown(result);
      } catch (e) {
        console.error("range switch failed", e);
      }
    },
    [stack, shown],
  );

  const toggleWhitespace = useCallback(() => {
    if (shown) void chooseRange(shown.range, !shown.ignoreWhitespace);
  }, [shown, chooseRange]);

  const markRangeReviewed = useCallback(async () => {
    if (!stack || !shown) return;
    const { lo, hi } = shown.range;
    await markReviewed(stack.stackId, lo, hi);
    const which = lo === hi ? `#${stack.prs[lo].number}` : `#${stack.prs[lo].number}–#${stack.prs[hi].number}`;
    say(`Marked ${which} as reviewed at ${stack.prs[hi].head_sha.slice(0, 7)} — press d later to see what changed`);
  }, [stack, shown, say]);

  const showSince = useCallback(
    async (checkpointId?: string) => {
      if (!stack || !shown) return;
      const { lo, hi } = shown.range;
      const available = await listCheckpoints(stack.stackId, lo, hi);
      setCheckpoints(available);
      const chosen = checkpointId ?? available[0]?.id;
      if (!chosen) {
        say("No checkpoint for this range yet — press M to mark it as reviewed");
        return;
      }
      mark("range:start");
      const request = ++rangeRequest.current;
      const result = await selectSince(stack.stackId, lo, hi, chosen);
      if (request === rangeRequest.current) setShown(result);
    },
    [stack, shown, say],
  );

  const toggleSince = useCallback(() => {
    if (!shown) return;
    if (shown.since) void chooseRange(shown.range, false);
    else void showSince().catch((e) => say(String(e)));
  }, [shown, chooseRange, showSince, say]);

  const commands = useMemo<Command[]>(() => {
    const list: Command[] = [];
    if (shown) {
      list.push({
        id: "whitespace",
        label: shown.ignoreWhitespace ? "Show whitespace changes" : "Hide whitespace changes",
        run: toggleWhitespace,
      });
    }
    list.push({ id: "inbox", label: "Go to inbox", run: () => setHome(true) });
    list.push({ id: "theme", label: "Change theme…", run: () => setOverlay("theme") });
    list.push({ id: "shortcuts", label: "Keyboard shortcuts", run: () => setOverlay("shortcuts") });
    if (stack) list.push({ id: "submit", label: "Submit review…", run: () => setSheetOpen(true) });
    if (shown) {
      list.push({ id: "reviewed", label: "Mark as reviewed (checkpoint)", run: () => void markRangeReviewed() });
      list.push({ id: "since", label: shown.since ? "Show the full diff" : "Show changes since last review", run: toggleSince });
    }
    list.push({
      id: "mode",
      label: defaultMode === "split" ? "Show all files unified" : "Show all files side by side",
      run: () => changeDefaultMode(defaultMode === "split" ? "unified" : "split"),
    });
    list.push({ id: "wrap", label: wrap ? "Don't wrap long lines" : "Wrap long lines", run: () => changeWrap(!wrap) });
    if (repo) {
      list.push({
        id: "ignore",
        keyword: "ignore",
        label: "Ignore files matching",
        run: (pattern) => pattern.trim() && updateIgnore([...new Set([...ignorePatterns, pattern.trim()])]),
      });
      for (const pattern of ignorePatterns) {
        list.push({ id: `unignore:${pattern}`, label: `Stop ignoring ${pattern}`, run: () => updateIgnore(ignorePatterns.filter((p) => p !== pattern)) });
      }
    }
    return list.map((c) => ({ ...c, run: (arg: string) => (c.run(arg), setPaletteOpen(false)) }));
  }, [shown, stack, repo, ignorePatterns, defaultMode, wrap, toggleWhitespace, changeDefaultMode, changeWrap, updateIgnore, markRangeReviewed, toggleSince]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey && e.key === "k") {
        e.preventDefault();
        setError(null);
        setOverlay(null);
        setPaletteOpen(true);
      } else if (e.metaKey && e.key === "t") {
        e.preventDefault();
        setPaletteOpen(false);
        setOverlay("theme");
      } else if ((e.metaKey && e.key === "/") || (e.key === "?" && !e.metaKey && !isTypingIn(e.target))) {
        e.preventDefault();
        setPaletteOpen(false);
        setOverlay("shortcuts");
      } else if (overlay) {
        return;
      } else if (e.metaKey && e.key === "i") {
        e.preventDefault();
        setHome(true);
      } else if (e.metaKey && e.key === "b") {
        e.preventDefault();
        setShowFiles((s) => !s);
      } else if (!paletteOpen && !sheetOpen && !home && stack && shown && !e.metaKey && !e.ctrlKey && !e.altKey && !isTypingIn(e.target)) {
        if (e.key === "w") {
          e.preventDefault();
          toggleWhitespace();
          return;
        }
        if (e.key === "M") {
          e.preventDefault();
          void markRangeReviewed();
          return;
        }
        if (e.key === "d") {
          e.preventDefault();
          toggleSince();
          return;
        }
        if (e.key === "Tab") {
          // Tab / Shift-Tab: the next / previous PR of the stack on its own, wrapping around.
          e.preventDefault();
          const size = stack.prs.length;
          const { lo, hi } = shown.range;
          if (size > 1) {
            const index = e.shiftKey ? (lo - 1 + size) % size : (hi + 1) % size;
            void chooseRange({ lo: index, hi: index });
          }
          return;
        }
        const next = rangeForKey(e.key, shown.range, stack.prs.length);
        if (next) {
          e.preventDefault();
          void chooseRange(next);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paletteOpen, sheetOpen, overlay, home, stack, shown, chooseRange, toggleWhitespace, markRangeReviewed, toggleSince]);

  const isStack = stack !== null && stack.prs.length > 1;
  const showingDiff = !home && stack !== null && shown !== null;
  const ready = (lo: number, hi: number) => (stack ? (readyRanges.get(stack.stackId)?.has(`${lo}-${hi}`) ?? false) : false);

  return (
    <div className="app">
      <header className="titlebar" data-tauri-drag-region>
        <button className="titlebar-home" title="Inbox (⌘I)" onClick={() => setHome(true)}>
          <Mark className="titlebar-mark" />
        </button>
        {showingDiff ? (
          <RangeHeader stack={stack} shown={shown} checkpoints={checkpoints} onPickCheckpoint={(id) => void showSince(id)} />
        ) : (
          <span className="titlebar-hint" data-tauri-drag-region />
        )}
        {showingDiff && (
          <button className="review-button" onClick={() => setSheetOpen(true)} data-testid="review-button">
            Submit review{drafts.length ? ` · ${drafts.length}` : ""}
          </button>
        )}
        {newerVersion && (
          <button className="update-banner" onClick={() => show(newerVersion)}>
            New commits pushed · load latest
          </button>
        )}
      </header>
      {showingDiff && isStack && (
        <StackBar key={stack.stackId} prs={stack.prs} needsRebase={stack.needsRebase} range={shown.range} ready={ready} onSelect={chooseRange} />
      )}
      <main className="content">
        {showingDiff ? (
          <DiffViewer
            key={stack.stackId}
            stackId={stack.stackId}
            lo={shown.range.lo}
            hi={shown.range.hi}
            drafts={drafts}
            threads={threads}
            onDraftsChanged={refreshDrafts}
            viewId={shown.viewId}
            summary={shown.summary}
            prs={stack.prs}
            showAttribution={isStack}
            multiPr={shown.range.hi > shown.range.lo}
            showFiles={showFiles}
            keyboardEnabled={!paletteOpen && !sheetOpen && !overlay && !home}
            defaultMode={defaultMode}
            onDefaultModeChange={changeDefaultMode}
            wrap={wrap}
            onWrapChange={changeWrap}
            isIgnored={isIgnored}
            isViewed={isViewed}
            onToggleViewed={toggleViewed}
          />
        ) : (
          <Inbox entries={inbox} online={online} keyboardEnabled={!paletteOpen && !overlay} onOpen={open} onRefresh={refreshInbox} />
        )}
      </main>
      {sheetOpen && stack && (
        <SubmitSheet
          stackId={stack.stackId}
          onClose={() => setSheetOpen(false)}
          onSubmitted={(fresh) => {
            refreshDrafts();
            listThreads(stack.stackId, false)
              .then(setThreads)
              .catch(() => undefined);
            if (fresh.stackId !== stack.stackId) setNewerVersion(fresh);
          }}
        />
      )}
      {toast && (
        <div className="toast" role="status">
          {toast}
        </div>
      )}
      {overlay === "theme" && <ThemePicker onClose={() => setOverlay(null)} />}
      {overlay === "shortcuts" && <ShortcutsHelp onClose={() => setOverlay(null)} />}
      {paletteOpen && (
        <CommandPalette busy={busy} error={error} commands={commands} onOpen={open} onClose={() => setPaletteOpen(false)} />
      )}
    </div>
  );
}

interface RangeHeaderProps {
  stack: OpenedStack;
  shown: OpenedRange;
  checkpoints: Checkpoint[];
  onPickCheckpoint: (id: string) => void;
}

function RangeHeader({ stack, shown, checkpoints, onPickCheckpoint }: RangeHeaderProps) {
  const { lo, hi } = shown.range;
  const [bottom, top] = [stack.prs[lo], stack.prs[hi]];
  const { summary } = shown;
  return (
    <div className="pr-header" data-tauri-drag-region>
      <span className="pr-title" data-tauri-drag-region>
        {lo === hi ? top.title : `#${bottom.number}–#${top.number} · ${hi - lo + 1} PRs`}
      </span>
      <span className="pr-meta" data-tauri-drag-region>
        {lo === hi ? `${prLabel(top)} · ${top.author} · ` : `${top.base_repo} · `}
        {bottom.base_ref} ← {top.head_ref} · {summary.files.length} files ·{" "}
        <span className="add">+{summary.additions}</span> <span className="del">−{summary.deletions}</span>
        {shown.ignoreWhitespace && <span className="pr-flag"> · whitespace hidden</span>}
        {shown.since && (
          <span className="pr-flag since">
            {" · changes since "}
            <select
              className="since-picker"
              value={shown.since.checkpointId}
              onChange={(e) => onPickCheckpoint(e.target.value)}
              data-testid="since-picker"
            >
              {checkpoints.map((c) => (
                <option key={c.id} value={c.id}>
                  {new Date(c.createdAt * 1000).toLocaleString()} ({c.source === "submit" ? "submitted" : "marked"})
                </option>
              ))}
            </select>
            {shown.since.conflicts && " · rebase conflicted, showing raw changes"}
          </span>
        )}
      </span>
    </div>
  );
}

const isTypingIn = (t: EventTarget | null) =>
  t instanceof HTMLElement && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName));
