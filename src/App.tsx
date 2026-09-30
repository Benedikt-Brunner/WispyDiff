import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getIgnorePatterns, openPr, refreshPr, selectRange, setIgnorePatterns } from "./api";
import { CommandPalette, type Command } from "./CommandPalette";
import { DiffViewer, type BaseMode } from "./DiffViewer";
import { globMatcher } from "./glob";
import { Mark, Wordmark } from "./Logo";
import { mark } from "./perf";
import { loadPref, savePref } from "./prefs";
import { rememberPr } from "./recent";
import { rangeForKey, StackBar } from "./StackBar";
import { prLabel, type OpenedRange, type OpenedStack, type Range } from "./types";
import "./styles.css";

/** Which ranges the backend has precomputed, per stack (`"lo-hi"` keys). */
const readyRanges = new Map<string, Set<string>>();

export default function App() {
  const [stack, setStack] = useState<OpenedStack | null>(null);
  const [shown, setShown] = useState<OpenedRange | null>(null);
  const [newerVersion, setNewerVersion] = useState<OpenedStack | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showFiles, setShowFiles] = useState(true);
  const [defaultMode, setDefaultMode] = useState<BaseMode>(() => loadPref("defaultMode", ["unified", "split"] as const, "unified"));
  const [ignorePatterns, setIgnore] = useState<string[]>([]);
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

  const repo = stack?.prs[0].base_repo ?? null;
  useEffect(() => {
    if (!repo) return;
    getIgnorePatterns(repo)
      .then(setIgnore)
      .catch(() => setIgnore([]));
  }, [repo]);
  const isIgnored = useMemo(() => globMatcher(ignorePatterns), [ignorePatterns]);

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

  const commands = useMemo<Command[]>(() => {
    const list: Command[] = [];
    if (shown) {
      list.push({
        id: "whitespace",
        label: shown.ignoreWhitespace ? "Show whitespace changes" : "Hide whitespace changes",
        run: toggleWhitespace,
      });
    }
    list.push({
      id: "mode",
      label: defaultMode === "split" ? "Show all files unified" : "Show all files side by side",
      run: () => changeDefaultMode(defaultMode === "split" ? "unified" : "split"),
    });
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
  }, [shown, repo, ignorePatterns, defaultMode, toggleWhitespace, changeDefaultMode, updateIgnore]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey && e.key === "k") {
        e.preventDefault();
        setError(null);
        setPaletteOpen(true);
      } else if (e.metaKey && e.key === "b") {
        e.preventDefault();
        setShowFiles((s) => !s);
      } else if (!paletteOpen && stack && shown && !e.metaKey && !e.ctrlKey && !e.altKey) {
        if (e.key === "w") {
          e.preventDefault();
          toggleWhitespace();
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
  }, [paletteOpen, stack, shown, chooseRange, toggleWhitespace]);

  const isStack = stack !== null && stack.prs.length > 1;
  const ready = (lo: number, hi: number) => (stack ? (readyRanges.get(stack.stackId)?.has(`${lo}-${hi}`) ?? false) : false);

  return (
    <div className="app">
      <header className="titlebar" data-tauri-drag-region>
        {stack && <Mark className="titlebar-mark" />}
        {stack && shown ? <RangeHeader stack={stack} shown={shown} /> : <span className="titlebar-hint" data-tauri-drag-region />}
        {newerVersion && (
          <button className="update-banner" onClick={() => show(newerVersion)}>
            New commits pushed · load latest
          </button>
        )}
      </header>
      {isStack && shown && (
        <StackBar key={stack.stackId} prs={stack.prs} needsRebase={stack.needsRebase} range={shown.range} ready={ready} onSelect={chooseRange} />
      )}
      <main className="content">
        {stack && shown ? (
          <DiffViewer
            key={stack.stackId}
            viewId={shown.viewId}
            summary={shown.summary}
            prs={stack.prs}
            showAttribution={isStack}
            multiPr={shown.range.hi > shown.range.lo}
            showFiles={showFiles}
            keyboardEnabled={!paletteOpen}
            defaultMode={defaultMode}
            onDefaultModeChange={changeDefaultMode}
            isIgnored={isIgnored}
          />
        ) : (
          <div className="empty">
            <Wordmark className="empty-wordmark" />
            <span className="empty-hint">⌘K to open a pull request</span>
          </div>
        )}
      </main>
      {paletteOpen && (
        <CommandPalette busy={busy} error={error} commands={commands} onOpen={open} onClose={stack ? () => setPaletteOpen(false) : null} />
      )}
    </div>
  );
}

function RangeHeader({ stack, shown }: { stack: OpenedStack; shown: OpenedRange }) {
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
      </span>
    </div>
  );
}
