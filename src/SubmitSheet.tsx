import { useCallback, useEffect, useRef, useState } from "react";
import { deleteDraft, mergeInfo, mergePr, prepareSubmit, submitReview, updateDraft, type MergeInfo, type MergeMethod, type SubmitPlan } from "./api";
import { lineLabel, type Outcome, type Planned, type Verdict } from "./comments";
import type { OpenedStack } from "./types";
import { isMac, isSubmitKey, keyLabel, mod } from "./platform";
import { pasteImages } from "./images";

interface Props {
  stackId: string;
  /** The PRs on screen: the sheet starts in the first of them with drafts (else the lowest). */
  shown: { lo: number; hi: number };
  onClose: () => void;
  /**
   * Called after submitting, with the stack as it was on GitHub when planning. `done`: everything went out
   * and nothing is left to send, so the sheet should close with `message` as a toast.
   */
  onSubmitted: (fresh: OpenedStack, done: boolean, message: string) => void;
}

/** ⌘1 / ⌘2 / ⌘3 pick the verdict of the PR being written in. */
const VERDICT_KEYS: Record<string, Verdict> = { "1": "COMMENT", "2": "APPROVE", "3": "REQUEST_CHANGES" };

type Result = { state: "sending" } | { state: "done"; outcome: Outcome } | { state: "error"; message: string };

/** Merging a PR along with its review: what GitHub says about it, and the commit message being edited. */
type Merge = { info: MergeInfo | null; error?: string; method: MergeMethod; title: string; body: string };
type MergeResult = { state: "sending" } | { state: "done"; outcome: "merged" | "autoMergeEnabled" } | { state: "error"; message: string };

const METHOD_LABELS: Record<MergeMethod, string> = { SQUASH: "Squash and merge", MERGE: "Merge commit", REBASE: "Rebase and merge" };
const MERGE_KEY = isMac ? "⇧M" : "Shift+M";

/** Why a PR can't be merged at all (a review can't change it), if so. */
function mergeBlocked(info: MergeInfo): string | null {
  if (!info.open) return "This PR isn't open.";
  if (info.draft) return "Draft PRs can't be merged.";
  if (!info.methods.length) return "The repository allows no merge method.";
  return null;
}

/** What submitting will do: merge now when green, otherwise enable auto-merge, and why. */
function mergeStatus(info: MergeInfo): string {
  if (info.mergeState === "CLEAN" || info.mergeState === "HAS_HOOKS") return "Checks green — merges now";
  const why =
    info.mergeState === "DIRTY"
      ? "has conflicts"
      : info.mergeState === "BEHIND"
        ? "branch is behind its base"
        : info.checks === "PENDING" || info.checks === "EXPECTED"
          ? "checks running"
          : info.checks === "FAILURE" || info.checks === "ERROR" || info.mergeState === "UNSTABLE"
            ? "checks failing"
            : info.mergeState === "BLOCKED"
              ? "waiting for required reviews or checks"
              : info.mergeState.toLowerCase() || "state unknown";
  if (info.autoMergeEnabled) return `Auto-merge already enabled (${why}); submitting enables it again with this message`;
  if (!info.autoMergeAllowed) return `Can't merge yet (${why}), and the repository doesn't allow auto-merge`;
  return `Enables auto-merge — ${why}`;
}

/** One click from the diff: review each PR of the stack with its drafts, verdict and summary. */
export function SubmitSheet({ stackId, shown, onClose, onSubmitted }: Props) {
  const [plan, setPlan] = useState<SubmitPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [verdicts, setVerdicts] = useState<Record<number, Verdict>>({});
  const [summaries, setSummaries] = useState<Record<number, string>>({});
  const [results, setResults] = useState<Record<number, Result>>({});
  const [merges, setMerges] = useState<Record<number, Merge>>({});
  const [mergeResults, setMergeResults] = useState<Record<number, MergeResult>>({});
  const [submitting, setSubmitting] = useState(false);
  const sheet = useRef<HTMLDivElement>(null);

  const load = useCallback(() => {
    setError(null);
    prepareSubmit(stackId)
      .then(setPlan)
      .catch((e) => setError(String(e)));
  }, [stackId]);
  useEffect(load, [load]);

  const plannedFor = (index: number): Planned[] => plan?.prs.find((p) => p.prIndex === index)?.planned ?? [];
  /** GitHub lets the author of a PR only comment on it. */
  const own = (index: number) => plan?.stack.prs[index]?.author === plan?.viewer;
  const blocked = (index: number) => plannedFor(index).some((p) => p.plan.kind === "outdated");
  const hasReviewWork = (index: number) =>
    plannedFor(index).length > 0 || (verdicts[index] ?? "COMMENT") !== "COMMENT" || (summaries[index] ?? "").trim() !== "";
  /** Merging was asked for and can go ahead (a commit title is needed unless rebasing). */
  const wantsMerge = (index: number) => {
    const merge = merges[index];
    return !!merge?.info && !mergeBlocked(merge.info) && (merge.method === "REBASE" || merge.title.trim() !== "");
  };
  const hasWork = (index: number) => hasReviewWork(index) || wantsMerge(index);

  /** Turns merging a PR on (asking GitHub how it would merge) or off. */
  const toggleMerge = (index: number) => {
    if (merges[index]) {
      setMerges(({ [index]: _, ...rest }) => rest);
      return;
    }
    setMerges((m) => ({ ...m, [index]: { info: null, method: "SQUASH", title: "", body: "" } }));
    mergeInfo(stackId, index)
      .then((info) =>
        setMerges((m) => (m[index] ? { ...m, [index]: { info, method: info.methods[0] ?? "SQUASH", title: info.title, body: info.body } } : m)),
      )
      .catch((e) => setMerges((m) => (m[index] ? { ...m, [index]: { ...m[index], error: String(e) } } : m)));
  };
  const editMerge = (index: number, change: Partial<Merge>) => setMerges((m) => (m[index] ? { ...m, [index]: { ...m[index], ...change } } : m));

  /** Submits the review of each PR in `indices` that has something to send. */
  const submit = async (indices: number[]) => {
    if (!plan) return;
    setSubmitting(true);
    const fresh = plan.stack;
    // Work this submit leaves behind: other PRs' drafts, verdicts or summaries, or PRs held back by outdated comments.
    let done = !plan.stack.prs.some((_, index) => hasWork(index) && (!indices.includes(index) || blocked(index)));
    const said: string[] = [];
    for (const index of indices) {
      if (!hasWork(index) || blocked(index)) continue;
      const number = fresh.prs[index].number;
      if (hasReviewWork(index)) {
        setResults((r) => ({ ...r, [index]: { state: "sending" } }));
        try {
          const outcome = await submitReview(fresh.stackId, index, verdicts[index] ?? "COMMENT", summaries[index]?.trim() || null);
          setResults((r) => ({ ...r, [index]: { state: "done", outcome } }));
          // Sent: the verdict and summary don't go out again with a later submit.
          setVerdicts(({ [index]: _, ...rest }) => rest);
          setSummaries(({ [index]: _, ...rest }) => rest);
          if (!said.includes("Review submitted")) said.unshift("Review submitted");
          if (outcome.failed.length || outcome.unknown) {
            // Not merged on top of a review that didn't fully go out.
            done = false;
            continue;
          }
        } catch (e) {
          setResults((r) => ({ ...r, [index]: { state: "error", message: String(e) } }));
          done = false;
          continue;
        }
      }
      const merge = merges[index];
      if (!merge || !wantsMerge(index)) continue;
      setMergeResults((r) => ({ ...r, [index]: { state: "sending" } }));
      try {
        const outcome = await mergePr(fresh.stackId, index, { method: merge.method, title: merge.title, body: merge.body });
        setMergeResults((r) => ({ ...r, [index]: { state: "done", outcome } }));
        setMerges(({ [index]: _, ...rest }) => rest);
        said.push(outcome === "merged" ? `#${number} merged` : `Auto-merge enabled for #${number}`);
      } catch (e) {
        setMergeResults((r) => ({ ...r, [index]: { state: "error", message: String(e) } }));
        done = false;
      }
    }
    setSubmitting(false);
    onSubmitted(fresh, done, said.join(" · ") || "Review submitted");
    if (done) return;
    // What's left to send for the other PRs (posted drafts drop out).
    prepareSubmit(stackId)
      .then(setPlan)
      .catch(() => undefined);
  };
  const submitAll = () => submit(plan ? plan.stack.prs.map((_, i) => i) : []);

  /** Enter submits the PR; Tab / ⇧Tab move through the summaries and merge fields of all PRs, wrapping. */
  const fieldKeys = (index: number) => (e: React.KeyboardEvent<HTMLTextAreaElement | HTMLInputElement>) => {
    if (isSubmitKey(e) && !mod(e)) {
      e.preventDefault();
      void submit([index]);
    } else if (e.key === "Tab" && !mod(e) && !e.altKey) {
      e.preventDefault();
      const fields = [...(sheet.current?.querySelectorAll<HTMLElement>(".sheet-summary, .sheet-merge-field") ?? [])];
      const at = fields.indexOf(e.currentTarget);
      fields[(at + (e.shiftKey ? fields.length - 1 : 1)) % fields.length]?.focus();
    }
  };

  const summaryOf = (index: number) => sheet.current?.querySelector<HTMLTextAreaElement>(`.sheet-pr[data-index="${index}"] .sheet-summary`);
  // Keyboard first: start in the summary of the PR on screen (the first with drafts), unless already in the sheet.
  useEffect(() => {
    if (!plan || submitting || sheet.current?.contains(document.activeElement)) return;
    const onScreen = plan.stack.prs.map((_, i) => i).filter((i) => i >= shown.lo && i <= shown.hi);
    summaryOf(onScreen.find((i) => plannedFor(i).length > 0) ?? shown.lo)?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [plan, submitting]);

  // Re-registered every render, so it always sees the current verdicts, summaries and plan.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (!submitting) onClose();
      } else if (mod(e) && e.key === "Enter") {
        e.preventDefault();
        if (plan && !submitting) void submitAll();
      } else if (mod(e) && e.shiftKey && e.key.toLowerCase() === "m") {
        e.preventDefault();
        const section = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>(".sheet-pr");
        if (plan && !submitting) toggleMerge(Number(section?.dataset.index ?? shown.lo));
      } else if (mod(e) && VERDICT_KEYS[e.key]) {
        e.preventDefault();
        const section = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>(".sheet-pr");
        const index = Number(section?.dataset.index ?? plan?.prs[0]?.prIndex ?? 0);
        const verdict = VERDICT_KEYS[e.key];
        if (plan && !submitting && (verdict === "COMMENT" || !own(index))) setVerdicts((v) => ({ ...v, [index]: verdict }));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const resolveOutdated = async (id: string, choice: "file" | "discard") => {
    if (choice === "file") await updateDraft(id, { asFile: true });
    else await deleteDraft(id);
    load();
  };

  return (
    <div className="palette-backdrop" onMouseDown={() => !submitting && onClose()}>
      <div className="sheet" ref={sheet} onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="Submit review">
        <div className="sheet-head">
          <span className="sheet-title">Submit review</span>
          <button className="link" onClick={onClose} disabled={submitting}>
            close
          </button>
        </div>
        {error && (
          <div className="sheet-error">
            Can't reach GitHub to prepare the review: {error}. Your drafts are safe locally.
            <button className="button" onClick={load}>
              Retry
            </button>
          </div>
        )}
        {!plan && !error && <div className="sheet-status">Checking the stack on GitHub…</div>}
        {plan && (
          <>
            <div className="sheet-body">
              {plan.stack.prs.map((pr, index) => {
                const planned = plannedFor(index);
                const result = results[index];
                return (
                  <section className="sheet-pr" key={pr.number} data-pr={pr.number} data-index={index}>
                    <div className="sheet-pr-head">
                      <span className="stack-number">#{pr.number}</span>
                      <span className="sheet-pr-title">{pr.title}</span>
                      <span className="sheet-count">
                        {planned.length} item{planned.length === 1 ? "" : "s"}
                      </span>
                      <select
                        className="verdict"
                        value={verdicts[index] ?? "COMMENT"}
                        onChange={(e) => setVerdicts((v) => ({ ...v, [index]: e.target.value as Verdict }))}
                        disabled={submitting || own(index)}
                        title={own(index) ? "Your own PR: GitHub only lets you comment" : `${keyLabel("1")} Comment · ${keyLabel("2")} Approve · ${keyLabel("3")} Request changes`}
                      >
                        <option value="COMMENT">Comment</option>
                        {!own(index) && <option value="APPROVE">Approve</option>}
                        {!own(index) && <option value="REQUEST_CHANGES">Request changes</option>}
                      </select>
                      <label className="sheet-merge-toggle" title={`Merge #${pr.number} after submitting (${keyLabel(MERGE_KEY)})`}>
                        <input type="checkbox" checked={!!merges[index]} onChange={() => toggleMerge(index)} disabled={submitting} data-testid={`merge-toggle-${pr.number}`} />
                        Merge
                      </label>
                      <button
                        className="button"
                        onClick={() => void submit([index])}
                        disabled={submitting || !hasWork(index) || blocked(index)}
                        title={`Submit only #${pr.number} (↵ in its summary)`}
                        data-testid={`submit-pr-${pr.number}`}
                      >
                        Submit #{pr.number}
                      </button>
                    </div>
                    <textarea
                      className="sheet-summary"
                      placeholder="Summary (optional)"
                      rows={2}
                      value={summaries[index] ?? ""}
                      onChange={(e) => setSummaries((s) => ({ ...s, [index]: e.target.value }))}
                      onPaste={(e) =>
                        pasteImages(e, (message) => message && setResults((r) => ({ ...r, [index]: { state: "error", message: `Couldn't paste the image: ${message}` } })))
                      }
                      onKeyDown={fieldKeys(index)}
                      disabled={submitting}
                    />
                    {merges[index] && (
                      <MergeSection
                        number={pr.number}
                        merge={merges[index]}
                        disabled={submitting}
                        onChange={(change) => editMerge(index, change)}
                        onKeyDown={fieldKeys(index)}
                      />
                    )}
                    <ul className="sheet-items">
                      {planned.map(({ draft, plan: p }) => (
                        <li key={draft.id} className={p.kind === "outdated" ? "outdated" : undefined}>
                          <span className="sheet-item-kind">{planLabel(p, draft)}</span>
                          <span className="sheet-item-body">{draft.kind === "resolve" ? "resolve thread" : draft.body}</span>
                          {p.kind === "outdated" && (
                            <span className="sheet-item-actions">
                              <button className="button" onClick={() => void resolveOutdated(draft.id, "file")}>
                                Post as file comment
                              </button>
                              <button className="button" onClick={() => void resolveOutdated(draft.id, "discard")}>
                                Discard
                              </button>
                            </span>
                          )}
                          {draft.status === "failed" && <span className="card-error">{draft.error}</span>}
                        </li>
                      ))}
                    </ul>
                    {blocked(index) && <div className="sheet-note">Decide on the outdated comments before this PR can be submitted.</div>}
                    {result?.state === "sending" && <div className="sheet-note">Sending…</div>}
                    {result?.state === "error" && <div className="card-error">{result.message}</div>}
                    {mergeResults[index]?.state === "sending" && <div className="sheet-note">Merging…</div>}
                    {mergeResults[index]?.state === "error" && <div className="card-error">Not merged: {(mergeResults[index] as { message: string }).message}</div>}
                    {mergeResults[index]?.state === "done" && (
                      <div className="sheet-done" data-testid={`merged-${pr.number}`}>
                        {(mergeResults[index] as { outcome: string }).outcome === "merged" ? "Merged ✓" : "Auto-merge enabled ✓ — merges once it's green"}
                      </div>
                    )}
                    {result?.state === "done" && (
                      <div className={result.outcome.failed.length ? "card-error" : "sheet-done"}>
                        {result.outcome.failed.length
                          ? `${result.outcome.failed.length} rejected: ${result.outcome.failed.map((f) => f[1]).join("; ")}`
                          : result.outcome.unknown
                            ? `Sent; ${result.outcome.unknown} not confirmed yet — checked before the next submit`
                            : "Posted ✓"}
                      </div>
                    )}
                  </section>
                );
              })}
            </div>
            <div className="sheet-foot">
              <span className="sheet-keys">
                {keyLabel("1")} comment · {keyLabel("2")} approve · {keyLabel("3")} request changes · {keyLabel(MERGE_KEY)} merge · ⇥ next field · ↵ submit PR · {keyLabel("↵")} all
              </span>
              <button className="button primary" onClick={() => void submitAll()} disabled={submitting} title={`Submit all (${keyLabel("↵")})`} data-testid="submit-all">
                Submit all
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function planLabel(plan: Planned["plan"], draft: Planned["draft"]) {
  switch (plan.kind) {
    case "line":
      return `${plan.path.split("/").pop()} ${lineLabel(plan.startLine, plan.line)}`;
    case "file":
      return plan.quoted ? `${plan.path.split("/").pop()} · file comment (outside the diff)` : `${plan.path.split("/").pop()} · file`;
    case "outdated":
      return `⚠ outdated · ${draft.path?.split("/").pop()} ${lineLabel(draft.startLine, draft.line)}`;
    case "reply":
      return "reply";
    case "resolve":
      return "resolve";
    case "summary":
      return "summary";
  }
}

/** Where and how the PR merges, with its commit message to edit. */
function MergeSection({
  number,
  merge,
  disabled,
  onChange,
  onKeyDown,
}: {
  number: number;
  merge: Merge;
  disabled: boolean;
  onChange: (change: Partial<Merge>) => void;
  onKeyDown: (e: React.KeyboardEvent<HTMLTextAreaElement | HTMLInputElement>) => void;
}) {
  const { info } = merge;
  if (merge.error) return <div className="card-error sheet-merge">Can't check how #{number} merges: {merge.error}</div>;
  if (!info) return <div className="sheet-note sheet-merge">Checking how #{number} merges…</div>;
  const blocked = mergeBlocked(info);
  const elsewhere = info.baseRef !== info.defaultBranch;
  return (
    <div className="sheet-merge" data-testid={`merge-${number}`}>
      <div className="sheet-merge-head">
        <span>Merge into</span>
        <span className={`sheet-merge-base${elsewhere ? " elsewhere" : ""}`} data-testid={`merge-base-${number}`}>
          {info.baseRef}
        </span>
        {elsewhere && <span className="badge warn">not {info.defaultBranch}</span>}
        {!blocked && (
          <select className="verdict" value={merge.method} onChange={(e) => onChange({ method: e.target.value as MergeMethod })} disabled={disabled}>
            {info.methods.map((m) => (
              <option key={m} value={m}>
                {METHOD_LABELS[m]}
              </option>
            ))}
          </select>
        )}
        <span className="sheet-merge-status">{blocked ?? mergeStatus(info)}</span>
      </div>
      {!blocked && merge.method !== "REBASE" && (
        <>
          <input
            className="sheet-merge-field sheet-merge-title"
            value={merge.title}
            placeholder="Commit title"
            onChange={(e) => onChange({ title: e.target.value })}
            onKeyDown={onKeyDown}
            disabled={disabled}
          />
          <textarea
            className="sheet-merge-field sheet-merge-body"
            value={merge.body}
            placeholder="Commit message (optional)"
            rows={2}
            onChange={(e) => onChange({ body: e.target.value })}
            onKeyDown={onKeyDown}
            disabled={disabled}
          />
        </>
      )}
    </div>
  );
}
