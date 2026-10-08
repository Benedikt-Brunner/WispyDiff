import { useCallback, useEffect, useState } from "react";
import { deleteDraft, prepareSubmit, submitReview, updateDraft, type SubmitPlan } from "./api";
import { lineLabel, type Outcome, type Planned, type Verdict } from "./comments";
import type { OpenedStack } from "./types";
import { isSubmitKey } from "./platform";

interface Props {
  stackId: string;
  onClose: () => void;
  /**
   * Called after submitting, with the stack as it was on GitHub when planning. `done`: everything went out
   * and nothing is left to send, so the sheet should close.
   */
  onSubmitted: (fresh: OpenedStack, done: boolean) => void;
}

type Result = { state: "sending" } | { state: "done"; outcome: Outcome } | { state: "error"; message: string };

/** One click from the diff: review each PR of the stack with its drafts, verdict and summary. */
export function SubmitSheet({ stackId, onClose, onSubmitted }: Props) {
  const [plan, setPlan] = useState<SubmitPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [verdicts, setVerdicts] = useState<Record<number, Verdict>>({});
  const [summaries, setSummaries] = useState<Record<number, string>>({});
  const [results, setResults] = useState<Record<number, Result>>({});
  const [submitting, setSubmitting] = useState(false);

  const load = useCallback(() => {
    setError(null);
    prepareSubmit(stackId)
      .then(setPlan)
      .catch((e) => setError(String(e)));
  }, [stackId]);
  useEffect(load, [load]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !submitting && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, submitting]);

  const plannedFor = (index: number): Planned[] => plan?.prs.find((p) => p.prIndex === index)?.planned ?? [];
  const blocked = (index: number) => plannedFor(index).some((p) => p.plan.kind === "outdated");
  const hasWork = (index: number) =>
    plannedFor(index).length > 0 || (verdicts[index] ?? "COMMENT") !== "COMMENT" || (summaries[index] ?? "").trim() !== "";

  /** Submits the review of each PR in `indices` that has something to send. */
  const submit = async (indices: number[]) => {
    if (!plan) return;
    setSubmitting(true);
    const fresh = plan.stack;
    // Work this submit leaves behind: other PRs' drafts, verdicts or summaries, or PRs held back by outdated comments.
    let done = !plan.stack.prs.some((_, index) => hasWork(index) && (!indices.includes(index) || blocked(index)));
    for (const index of indices) {
      if (!hasWork(index) || blocked(index)) continue;
      setResults((r) => ({ ...r, [index]: { state: "sending" } }));
      try {
        const outcome = await submitReview(fresh.stackId, index, verdicts[index] ?? "COMMENT", summaries[index]?.trim() || null);
        setResults((r) => ({ ...r, [index]: { state: "done", outcome } }));
        if (outcome.failed.length || outcome.unknown) done = false;
        // Sent: the verdict and summary don't go out again with a later submit.
        setVerdicts(({ [index]: _, ...rest }) => rest);
        setSummaries(({ [index]: _, ...rest }) => rest);
      } catch (e) {
        setResults((r) => ({ ...r, [index]: { state: "error", message: String(e) } }));
        done = false;
      }
    }
    setSubmitting(false);
    onSubmitted(fresh, done);
    if (done) return;
    // What's left to send for the other PRs (posted drafts drop out).
    prepareSubmit(stackId)
      .then(setPlan)
      .catch(() => undefined);
  };
  const submitAll = () => submit(plan ? plan.stack.prs.map((_, i) => i) : []);

  const resolveOutdated = async (id: string, choice: "file" | "discard") => {
    if (choice === "file") await updateDraft(id, { asFile: true });
    else await deleteDraft(id);
    load();
  };

  return (
    <div className="palette-backdrop" onMouseDown={() => !submitting && onClose()}>
      <div className="sheet" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="Submit review">
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
                  <section className="sheet-pr" key={pr.number} data-pr={pr.number}>
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
                        disabled={submitting}
                      >
                        <option value="COMMENT">Comment</option>
                        <option value="APPROVE">Approve</option>
                        <option value="REQUEST_CHANGES">Request changes</option>
                      </select>
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
                      onKeyDown={(e) => {
                        if (isSubmitKey(e)) {
                          e.preventDefault();
                          void submit([index]);
                        }
                      }}
                      disabled={submitting}
                    />
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
              <button className="button primary" onClick={() => void submitAll()} disabled={submitting} data-testid="submit-all">
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
