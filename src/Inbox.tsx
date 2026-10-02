import { useCallback, useEffect, useMemo, useState } from "react";
import { Wordmark } from "./Logo";
import { keyLabel } from "./platform";
import { loadList, saveList } from "./prefs";

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

/** Where you stand with one PR. */
export interface PrProgress {
  /** Your latest submitted GitHub review: APPROVED, CHANGES_REQUESTED, COMMENTED or DISMISSED. */
  review: string | null;
  changedSinceReview: boolean;
  /** Marked as reviewed locally (M). */
  marked: boolean;
  changedSinceMarked: boolean;
}

export interface InboxEntry {
  group: { repo: string; prs: InboxPr[]; updatedAt: string };
  /** Prefetched: opens instantly and offline. */
  ready: boolean;
  drafts: number;
  /** Same order as `group.prs`. */
  progress: PrProgress[];
}

const REVIEW_LABELS: Record<string, string> = {
  APPROVED: "you approved",
  CHANGES_REQUESTED: "you requested changes",
  COMMENTED: "you commented",
  DISMISSED: "your review was dismissed",
};

const groupKey = (entry: InboxEntry) => `${entry.group.repo}#${entry.group.prs[0].number}`;
const prLabel = (pr: InboxPr) => `${pr.repo}#${pr.number}`;

/** Reviewed or marked at the PR's current head. */
const upToDate = (p: PrProgress) => (p.review !== null && !p.changedSinceReview) || (p.marked && !p.changedSinceMarked);
const changedSince = (p: PrProgress) => (p.review !== null || p.marked) && !upToDate(p);

function ProgressBadges({ progress }: { progress: PrProgress | undefined }) {
  if (!progress) return null;
  const { review, changedSinceReview, marked, changedSinceMarked } = progress;
  return (
    <>
      {review && (
        <span
          className={`badge inbox-review${changedSinceReview ? " warn" : " ok"}`}
          title={changedSinceReview ? "New commits were pushed since your review" : "Your latest review on GitHub is on the current head"}
        >
          {REVIEW_LABELS[review] ?? review.toLowerCase()}
          {changedSinceReview && " · new commits"}
        </span>
      )}
      {marked && (
        <span
          className={`badge inbox-marked${changedSinceMarked ? " warn" : " ok"}`}
          title={changedSinceMarked ? "Changed since you marked it reviewed (d shows what's new)" : "Marked reviewed at the current head"}
        >
          {changedSinceMarked ? "marked reviewed · changed since" : "marked reviewed"}
        </span>
      )}
    </>
  );
}

interface Props {
  entries: InboxEntry[];
  online: boolean | null;
  keyboardEnabled: boolean;
  onOpen: (label: string) => void;
  onRefresh: () => void;
}

/** Home: review requests and your own PRs, grouped into stacks. j/k to move, Enter to open, Space/e to collapse a stack. */
export function Inbox({ entries, online, keyboardEnabled, onOpen, onRefresh }: Props) {
  const [collapsed, setCollapsed] = useState(() => new Set(loadList("inboxCollapsed")));
  // What j/k move over: each PR, or a collapsed stack as a whole ("group:<key>").
  const items = useMemo(
    () =>
      entries.flatMap((e) => {
        const key = groupKey(e);
        return collapsed.has(key) && e.group.prs.length > 1 ? [`group:${key}`] : e.group.prs.map(prLabel);
      }),
    [entries, collapsed],
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selected = Math.max(0, selectedId === null ? 0 : items.indexOf(selectedId));

  const toggle = useCallback(
    (entry: InboxEntry) => {
      const key = groupKey(entry);
      const next = new Set(collapsed);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      setCollapsed(next);
      saveList("inboxCollapsed", [...next]);
      setSelectedId(next.has(key) ? `group:${key}` : prLabel(entry.group.prs[0]));
    },
    [collapsed],
  );

  useEffect(() => {
    if (!keyboardEnabled) return;
    const entryOf = (id: string) => entries.find((e) => (id.startsWith("group:") ? `group:${groupKey(e)}` === id : e.group.prs.some((p) => prLabel(p) === id)));
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const current = items[selected];
      if (e.key === "j" || e.key === "ArrowDown") setSelectedId(items[Math.min(selected + 1, items.length - 1)]);
      else if (e.key === "k" || e.key === "ArrowUp") setSelectedId(items[Math.max(selected - 1, 0)]);
      else if (e.key === "Enter" && current?.startsWith("group:")) toggle(entryOf(current)!);
      else if (e.key === "Enter" && current) onOpen(current);
      else if ((e.key === " " || e.key === "e") && current) {
        const entry = entryOf(current);
        if (entry && entry.group.prs.length > 1) toggle(entry);
      } else if (e.key === "r") onRefresh();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [keyboardEnabled, entries, items, selected, toggle, onOpen, onRefresh]);

  if (entries.length === 0) {
    return (
      <div className="empty">
        <Wordmark className="empty-wordmark" />
        <span className="empty-hint">
          {online === false ? "Offline — no cached inbox yet" : `Nothing waiting for you · ${keyLabel("K")} to open any pull request`}
        </span>
      </div>
    );
  }

  return (
    <div className="inbox" data-testid="inbox">
      <div className="inbox-head">
        <span className="inbox-title">Inbox</span>
        <span className="inbox-status">{online === false ? "offline · showing cached" : `${entries.length} group${entries.length === 1 ? "" : "s"}`}</span>
        <button className="link" onClick={onRefresh}>
          refresh
        </button>
      </div>
      {entries.map((entry) => {
        const key = groupKey(entry);
        const stack = entry.group.prs.length > 1;
        const isCollapsed = stack && collapsed.has(key);
        const headSelected = isCollapsed && items[selected] === `group:${key}`;
        const current = entry.progress?.filter(upToDate).length ?? 0;
        const changed = entry.progress?.filter(changedSince).length ?? 0;
        return (
          <section className={`inbox-group${isCollapsed ? " collapsed" : ""}`} key={key} data-group={entry.group.prs.map((p) => p.number).join(",")} data-collapsed={isCollapsed || undefined}>
            <div
              className={`inbox-group-head${headSelected ? " selected" : ""}`}
              onClick={isCollapsed ? () => toggle(entry) : undefined}
              onMouseEnter={isCollapsed ? () => setSelectedId(`group:${key}`) : undefined}
            >
              {stack && (
                <button
                  className="inbox-toggle"
                  title={isCollapsed ? "Expand (Space)" : "Collapse (Space)"}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggle(entry);
                  }}
                >
                  {isCollapsed ? "▸" : "▾"}
                </button>
              )}
              <span className="inbox-repo">{entry.group.repo}</span>
              {stack && <span className="badge">stack of {entry.group.prs.length}</span>}
              {isCollapsed && current > 0 && (
                <span className="badge ok inbox-summary">
                  {current}/{entry.group.prs.length} reviewed
                </span>
              )}
              {isCollapsed && changed > 0 && <span className="badge warn inbox-summary">{changed} changed since</span>}
              {entry.drafts > 0 && <span className="badge warn">{entry.drafts} draft{entry.drafts === 1 ? "" : "s"}</span>}
              <span className={`inbox-ready${entry.ready ? " ready" : ""}`} title={entry.ready ? "Prefetched: opens instantly, offline too" : "Prefetching…"}>
                {entry.ready ? "offline ready" : "prefetching…"}
              </span>
            </div>
            {!isCollapsed && (
              <ul>
                {entry.group.prs.map((pr, i) => {
                  const label = prLabel(pr);
                  return (
                    <li key={pr.number} className={items[selected] === label ? "selected" : undefined} onClick={() => onOpen(label)} onMouseEnter={() => setSelectedId(label)} data-pr={pr.number}>
                      <span className="stack-number">#{pr.number}</span>
                      <span className="inbox-pr-title">{pr.title}</span>
                      {pr.draft && <span className="badge">draft</span>}
                      <ProgressBadges progress={entry.progress?.[i]} />
                      <span className="inbox-meta">
                        {pr.requested ? "review requested" : "yours"} · {pr.author} · {pr.baseRef} ← {pr.headRef}
                      </span>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        );
      })}
    </div>
  );
}
