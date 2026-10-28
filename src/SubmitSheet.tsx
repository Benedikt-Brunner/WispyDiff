import { useCallback, useEffect, useRef, useState } from "react";
import { deleteDraft, prepareSubmit, submitReview, updateDraft, type SubmitPlan } from "./api";
import { lineLabel, type Outcome, type Planned, type Verdict } from "./comments";
import type { OpenedStack } from "./types";
import { isSubmitKey, keyLabel, mod } from "./platform";
import { pasteImages } from "./images";

interface Props {
  stackId: string;
  /** The PRs on screen: the sheet starts in the first of them with drafts (else the lowest). */
  shown: { lo: number; hi: number };
  onClose: () => void;
  /**
   * Called after submitting, with the stack as it was on GitHub when planning. `done`: everything went out
   * and nothing is left to send, so the sheet should close.
   */
  onSubmitted: (fresh: OpenedStack, done: boolean) => void;
}

/** ⌘1 / ⌘2 / ⌘3 pick the verdict of the PR being written in. */
const VERDICT_KEYS: Record<string, Verdict> = { "1": "COMMENT", "2": "APPROVE", "3": "REQUEST_CHANGES" };

type Result = { state: "sending" } | { state: "done"; outcome: Outcome } | { state: "error"; message: string };

/** One click from the diff: review each PR of the stack with its drafts, verdict and summary. */
export function SubmitSheet({ stackId, shown, onClose, onSubmitted }: Props) {
  const [plan, setPlan] = useState<SubmitPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [verdicts, setVerdicts] = useState<Record<number, Verdict>>({});
  const [summaries, setSummaries] = useState<Record<number, string>>({});
  const [results, setResults] = useState<Record<number, Result>>({});
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
                      onKeyDown={(e) => {
                        if (isSubmitKey(e) && !mod(e)) {
                          e.preventDefault();
                          void submit([index]);
                        } else if (e.key === "Tab" && !mod(e) && !e.altKey) {
                          // The next / previous PR's summary, wrapping.
                          e.preventDefault();
                          const size = plan.stack.prs.length;
                          summaryOf((index + (e.shiftKey ? size - 1 : 1)) % size)?.focus();
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
              <span className="sheet-keys">
                {keyLabel("1")} comment · {keyLabel("2")} approve · {keyLabel("3")} request changes · ⇥ next PR · ↵ submit PR · {keyLabel("↵")} all
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
