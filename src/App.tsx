import { useCallback, useEffect, useState } from "react";
import { openPr, refreshPr } from "./api";
import { CommandPalette } from "./CommandPalette";
import { DiffViewer } from "./DiffViewer";
import { Mark, Wordmark } from "./Logo";
import { mark } from "./perf";
import { rememberPr } from "./recent";
import { prLabel, type OpenedPr } from "./types";
import "./styles.css";

export default function App() {
  const [opened, setOpened] = useState<OpenedPr | null>(null);
  const [newerVersion, setNewerVersion] = useState<OpenedPr | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showFiles, setShowFiles] = useState(true);

  const open = useCallback(async (input: string) => {
    mark("open:start");
    setBusy(input);
    setError(null);
    try {
      const result = await openPr(input);
      const label = prLabel(result.pr);
      rememberPr(label);
      setOpened(result);
      setNewerVersion(null);
      setPaletteOpen(false);
      if (result.fromCache) {
        // Stale-while-revalidate: show the cached diff now, offer the new one if the PR moved.
        refreshPr(label, result.summary.head_sha)
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
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey && e.key === "k") {
        e.preventDefault();
        setError(null);
        setPaletteOpen(true);
      } else if (e.metaKey && e.key === "b") {
        e.preventDefault();
        setShowFiles((s) => !s);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="app">
      <header className="titlebar" data-tauri-drag-region>
        {opened && <Mark className="titlebar-mark" />}
        {opened ? (
          <PrHeader opened={opened} />
        ) : (
          <span className="titlebar-hint" data-tauri-drag-region />
        )}
        {newerVersion && (
          <button className="update-banner" onClick={() => (setOpened(newerVersion), setNewerVersion(null))}>
            New commits pushed · load latest
          </button>
        )}
      </header>
      <main className="content">
        {opened ? (
          <DiffViewer opened={opened} showFiles={showFiles} keyboardEnabled={!paletteOpen} />
        ) : (
          <div className="empty">
            <Wordmark className="empty-wordmark" />
            <span className="empty-hint">⌘K to open a pull request</span>
          </div>
        )}
      </main>
      {paletteOpen && (
        <CommandPalette busy={busy} error={error} onOpen={open} onClose={opened ? () => setPaletteOpen(false) : null} />
      )}
    </div>
  );
}

function PrHeader({ opened }: { opened: OpenedPr }) {
  const { pullRequest: pr, summary } = opened;
  return (
    <div className="pr-header" data-tauri-drag-region>
      <span className="pr-title" data-tauri-drag-region>
        {pr.title}
      </span>
      <span className="pr-meta" data-tauri-drag-region>
        {prLabel(opened.pr)} · {pr.author} · {pr.base_ref} ← {pr.head_ref} · {summary.files.length} files ·{" "}
        <span className="add">+{summary.additions}</span> <span className="del">−{summary.deletions}</span>
      </span>
    </div>
  );
}
