import { useEffect, useMemo, useState } from "react";
import { Wordmark } from "./Logo";

export interface InboxPr {
  repo: string;
  number: number;
  title: string;
  url: string;
  draft: boolean;
  author: string;
  baseRef: string;
  headRef: string;
  headSha: string;
  updatedAt: string;
  requested: boolean;
  authored: boolean;
}

export interface InboxEntry {
  group: { repo: string; prs: InboxPr[]; updatedAt: string };
  /** Prefetched: opens instantly and offline. */
  ready: boolean;
  drafts: number;
}

interface Props {
  entries: InboxEntry[];
  online: boolean | null;
  keyboardEnabled: boolean;
  onOpen: (label: string) => void;
  onRefresh: () => void;
}

/** Home: review requests and your own PRs, grouped into stacks. j/k to move, Enter to open. */
export function Inbox({ entries, online, keyboardEnabled, onOpen, onRefresh }: Props) {
  const flat = useMemo(() => entries.flatMap((e) => e.group.prs.map((pr) => `${pr.repo}#${pr.number}`)), [entries]);
  const [selected, setSelected] = useState(0);

  useEffect(() => {
    if (!keyboardEnabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "j" || e.key === "ArrowDown") setSelected((s) => Math.min(s + 1, flat.length - 1));
      else if (e.key === "k" || e.key === "ArrowUp") setSelected((s) => Math.max(s - 1, 0));
      else if (e.key === "Enter" && flat[selected]) onOpen(flat[selected]);
      else if (e.key === "r") onRefresh();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [keyboardEnabled, flat, selected, onOpen, onRefresh]);

  if (entries.length === 0) {
    return (
      <div className="empty">
        <Wordmark className="empty-wordmark" />
        <span className="empty-hint">
          {online === false ? "Offline — no cached inbox yet" : "Nothing waiting for you · ⌘K to open any pull request"}
        </span>
      </div>
    );
  }

  let index = -1;
  return (
    <div className="inbox" data-testid="inbox">
      <div className="inbox-head">
        <span className="inbox-title">Inbox</span>
        <span className="inbox-status">{online === false ? "offline · showing cached" : `${entries.length} group${entries.length === 1 ? "" : "s"}`}</span>
        <button className="link" onClick={onRefresh}>
          refresh
        </button>
      </div>
      {entries.map((entry) => (
        <section className="inbox-group" key={`${entry.group.repo}#${entry.group.prs[0].number}`} data-group={entry.group.prs.map((p) => p.number).join(",")}>
          <div className="inbox-group-head">
            <span className="inbox-repo">{entry.group.repo}</span>
            {entry.group.prs.length > 1 && <span className="badge">stack of {entry.group.prs.length}</span>}
            {entry.drafts > 0 && <span className="badge warn">{entry.drafts} draft{entry.drafts === 1 ? "" : "s"}</span>}
            <span className={`inbox-ready${entry.ready ? " ready" : ""}`} title={entry.ready ? "Prefetched: opens instantly, offline too" : "Prefetching…"}>
              {entry.ready ? "offline ready" : "prefetching…"}
            </span>
          </div>
          <ul>
            {entry.group.prs.map((pr) => {
              index++;
              const label = `${pr.repo}#${pr.number}`;
              const mine = index === selected;
              return (
                <li key={pr.number} className={mine ? "selected" : undefined} onClick={() => onOpen(label)} onMouseEnter={() => setSelected(flat.indexOf(label))} data-pr={pr.number}>
                  <span className="stack-number">#{pr.number}</span>
                  <span className="inbox-pr-title">{pr.title}</span>
                  {pr.draft && <span className="badge">draft</span>}
                  <span className="inbox-meta">
                    {pr.requested ? "review requested" : "yours"} · {pr.author} · {pr.baseRef} ← {pr.headRef}
                  </span>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}
