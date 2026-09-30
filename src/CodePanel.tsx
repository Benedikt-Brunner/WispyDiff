import { useEffect, useRef, useState } from "react";
import type { GrepHit, Hit, Usages } from "./api";

export type PanelState = { kind: "usages"; name: string } | { kind: "search"; query: string };

interface Props {
  state: PanelState;
  usages: Usages | null;
  grepHits: GrepHit[];
  grepStatus: "idle" | "running" | "done" | string;
  onSearch: (query: string) => void;
  onJump: (target: { path: string; line: number; file?: number }) => void;
  onClose: () => void;
}

/** Usages of a symbol in the touched files, and whole-repo search (git grep). */
export function CodePanel({ state, usages, grepHits, grepStatus, onSearch, onJump, onClose }: Props) {
  const [query, setQuery] = useState(state.kind === "search" ? state.query : state.name);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    setQuery(state.kind === "search" ? state.query : state.name);
    if (state.kind === "search") input.current?.focus();
  }, [state]);

  const section = (title: string, hits: Hit[], testId: string) =>
    hits.length > 0 && (
      <div className="panel-section" data-testid={testId}>
        <div className="panel-section-title">
          {title} <span className="panel-count">{hits.length}</span>
        </div>
        {hits.map((hit, i) => (
          <button key={`${hit.path}:${hit.line}:${hit.col}:${i}`} className="panel-hit" onClick={() => onJump(hit)}>
            <span className="panel-hit-where">
              {hit.path.split("/").pop()}:{hit.line}
            </span>
            <span className="panel-hit-text">{hit.text}</span>
          </button>
        ))}
      </div>
    );

  return (
    <aside className="code-panel" data-testid="code-panel">
      <div className="panel-head">
        <input
          ref={input}
          className="panel-input"
          value={query}
          placeholder="Search the whole repo…"
          spellCheck={false}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && query.trim()) onSearch(query.trim());
            if (e.key === "Escape") onClose();
            // Plain keys stay in the input; ⌘ shortcuts (⌘K, ⌘I, …) still reach the app.
            if (!e.metaKey) e.stopPropagation();
          }}
        />
        <button className="link" onClick={onClose}>
          close
        </button>
      </div>
      <div className="panel-body">
        {state.kind === "usages" && usages && (
          <>
            <div className="panel-title">
              Usages of <code>{usages.name}</code> in this diff's files
            </div>
            {section("Definitions", usages.definitions, "usages-definitions")}
            {section("In changed lines", usages.changed, "usages-changed")}
            {section("In unchanged code", usages.unchanged, "usages-unchanged")}
            {usages.definitions.length + usages.changed.length + usages.unchanged.length === 0 && (
              <div className="panel-empty">No uses in the touched files.</div>
            )}
            <button className="button panel-search" onClick={() => onSearch(usages.name)}>
              Search the whole repo for “{usages.name}”
            </button>
          </>
        )}
        {(grepStatus !== "idle" || grepHits.length > 0) && (
          <div className="panel-section" data-testid="grep-results">
            <div className="panel-section-title">
              Repository <span className="panel-count">{grepHits.length}</span>
              {grepStatus === "running" && <span className="panel-count"> · searching…</span>}
            </div>
            {grepHits.map((hit, i) => (
              <button key={`${hit.path}:${hit.line}:${i}`} className="panel-hit" onClick={() => onJump(hit)}>
                <span className="panel-hit-where">
                  {hit.path}:{hit.line}
                </span>
                <span className="panel-hit-text">{hit.text}</span>
              </button>
            ))}
            {grepStatus !== "idle" && grepStatus !== "running" && grepStatus !== "done" && <div className="card-error">{grepStatus}</div>}
            {grepStatus === "done" && grepHits.length === 0 && <div className="panel-empty">No matches.</div>}
          </div>
        )}
      </div>
    </aside>
  );
}
