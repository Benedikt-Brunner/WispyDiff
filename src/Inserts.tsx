import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Html, Markdown } from "./Markdown";
import type { ReviewThread, ShownDraft, ThreadComment } from "./comments";
import { isSubmitKey, mod, keyLabel } from "./platform";

/** Measures its content and reports the height, so the layout can make room for it. */
export function InsertBox({ y, onHeight, children }: { y: number; onHeight: (h: number) => void; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const report = () => onHeight(Math.ceil(el.getBoundingClientRect().height));
    const observer = new ResizeObserver(report);
    observer.observe(el);
    report();
    return () => observer.disconnect();
  }, [onHeight]);
  return (
    <div className="insert" ref={ref} style={{ transform: `translate(var(--split-x, 0px), ${y}px)` }}>
      {children}
    </div>
  );
}

interface ComposerProps {
  title: string;
  /** `true`: GitHub won't take a line comment here, it'll go out as a file comment. */
  fileFallback: boolean | null;
  initial?: string;
  saveLabel?: string;
  /** The commented lines on the PR's head, offered as a GitHub suggestion to edit (null: not possible). */
  suggestion?: string | null;
  onSave: (text: string) => void;
  onCancel: () => void;
}

export function Composer({ title, fileFallback, initial = "", saveLabel = "Save draft", suggestion = null, onSave, onCancel }: ComposerProps) {
  const [text, setText] = useState(initial);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  const save = () => text.trim() && onSave(text.trim());
  // A file comment can't carry a suggestion.
  const canSuggest = suggestion !== null && fileFallback !== true;
  /** Inserts a ```suggestion block with the lines at the caret, and leaves the caret at their end. */
  const suggest = () => {
    const el = ref.current;
    if (!el || !canSuggest) return;
    const before = text.slice(0, el.selectionStart);
    const after = text.slice(el.selectionEnd);
    const block = `${before && !before.endsWith("\n") ? "\n" : ""}\`\`\`suggestion\n${suggestion}\n\`\`\``;
    setText(before + block + (after && !after.startsWith("\n") ? "\n" : "") + after);
    const caret = before.length + block.length - "\n```".length;
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(caret, caret);
    });
  };
  return (
    <div className="card composer">
      <div className="card-head">
        <span className="card-title">{title}</span>
        {fileFallback && (
          <span className="badge warn" title="GitHub only takes line comments inside the PR's diff">
            will post as file comment
          </span>
        )}
      </div>
      <textarea
        ref={ref}
        className="composer-input"
        value={text}
        placeholder={`Leave a comment · ↵ to save · ⇧↵ for a new line${canSuggest ? ` · ${keyLabel("G")} to suggest a change` : ""} · Esc to cancel`}
        rows={Math.min(20, Math.max(3, text.split("\n").length))}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (isSubmitKey(e)) {
            e.preventDefault();
            save();
          } else if (e.key.toLowerCase() === "g" && mod(e) && canSuggest) {
            e.preventDefault();
            e.stopPropagation();
            suggest();
          } else if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
          }
          // Plain keys stay in the editor; ⌘ shortcuts (⌘K, ⌘I, …) still reach the app.
          if (!mod(e)) e.stopPropagation();
        }}
      />
      <div className="card-actions">
        <button className="button primary" onClick={save} disabled={!text.trim()}>
          {saveLabel}
        </button>
        <button className="button" onClick={onCancel}>
          Cancel
        </button>
        {canSuggest && (
          <button className="button composer-suggest" onClick={suggest} title={`Suggest a change to these lines (${keyLabel("G")})`}>
            ± Suggest change
          </button>
        )}
      </div>
    </div>
  );
}

interface DraftCardProps {
  shown: ShownDraft;
  label: string;
  onEdit: (text: string) => void;
  onDelete: () => void;
}

export function DraftCard({ shown, label, onEdit, onDelete }: DraftCardProps) {
  const [editing, setEditing] = useState(false);
  const { draft } = shown;
  if (editing) {
    return (
      <Composer
        title={`Edit draft · ${label}`}
        fileFallback={null}
        initial={draft.body}
        saveLabel="Save"
        onSave={(text) => {
          onEdit(text);
          setEditing(false);
        }}
        onCancel={() => setEditing(false)}
      />
    );
  }
  const outdated = draft.kind === "line" && shown.line === null;
  return (
    <div className={`card draft${draft.status === "failed" ? " failed" : ""}`} data-draft={draft.id}>
      <div className="card-head">
        <span className="badge">Draft</span>
        {draft.assistant && <span className="badge">by the assistant</span>}
        <span className="card-title">{label}</span>
        {outdated && <span className="badge warn">outdated — lines changed since</span>}
        {draft.status === "posting" && <span className="badge">sending…</span>}
      </div>
      <Markdown text={draft.body} breaks />
      {draft.status === "failed" && <div className="card-error">GitHub rejected this: {draft.error}</div>}
      <div className="card-actions">
        <button className="button" onClick={() => setEditing(true)}>
          Edit
        </button>
        <button className="button" onClick={onDelete}>
          Delete
        </button>
      </div>
    </div>
  );
}

/** A GitHub comment as GitHub renders it (Markdown, HTML), or its text when there's no rendering cached. */
function CommentBody({ comment }: { comment: ThreadComment }) {
  if (!comment.body_html) return <div className="card-body">{comment.body.replace(/<!-- wispydiff:[^>]* -->/g, "").trim()}</div>;
  return <Html html={comment.body_html} />;
}

/**
 * Why GitHub would show the thread folded away (resolved, outdated, or its first comment
 * minimized), or null for an open thread. Such threads start hidden here too.
 */
export function settledReason(thread: ReviewThread) {
  if (thread.resolved) return "Resolved";
  if (thread.outdated) return "Outdated";
  if (thread.comments[0]?.minimized) return "Hidden on GitHub";
  return null;
}

/** A hidden thread's gutter icon; the tooltip previews the first comment. */
export function threadPreview(thread: ReviewThread) {
  const first = thread.comments[0];
  if (!first) return "Hidden comment";
  const text = first.body.replace(/<!--[\s\S]*?-->/g, "").replace(/\s+/g, " ").trim();
  const more = thread.comments.length > 1 ? ` (+${thread.comments.length - 1} more)` : "";
  const reason = settledReason(thread);
  return `${reason ? `${reason} · ` : ""}${first.author}: ${text.length > 120 ? `${text.slice(0, 120)}…` : text}${more}`;
}

/** One comment of a thread; a minimized one stays folded until clicked, like on GitHub. */
function ThreadCommentView({ comment }: { comment: ThreadComment }) {
  const [unfolded, setUnfolded] = useState(false);
  const folded = comment.minimized && !unfolded;
  return (
    <div className="comment">
      <div className="comment-meta">
        <span className="comment-author">{comment.author}</span> · {comment.created_at.slice(0, 10)}
        {folded && (
          <button className="link" onClick={() => setUnfolded(true)}>
            hidden on GitHub · show
          </button>
        )}
      </div>
      {!folded && <CommentBody comment={comment} />}
    </div>
  );
}

interface ThreadCardProps {
  thread: ReviewThread;
  label: string;
  replies: ShownDraft[];
  resolving: ShownDraft | undefined;
  onReply: (text: string) => void;
  onResolve: () => void;
  onDeleteDraft: (id: string) => void;
  /** Hide the thread, leaving an icon in the gutter. */
  onHide: () => void;
}

const HideButton = ({ onHide }: { onHide: () => void }) => (
  <button
    className="link card-hide"
    title="Hide this thread (a comment icon in the gutter brings it back)"
    onClick={(e) => {
      e.stopPropagation();
      onHide();
    }}
  >
    hide
  </button>
);

export function ThreadCard({ thread, label, replies, resolving, onReply, onResolve, onDeleteDraft, onHide }: ThreadCardProps) {
  const [replying, setReplying] = useState(false);
  return (
    <div className={`card thread${thread.resolved ? " resolved" : ""}`} data-thread={thread.id}>
      <div className="card-head">
        <span className="card-title">{label}</span>
        {thread.resolved && <span className="badge ok">resolved</span>}
        {thread.outdated && <span className="badge">outdated</span>}
        <HideButton onHide={onHide} />
      </div>
      {thread.comments.map((c) => (
        <ThreadCommentView key={c.id} comment={c} />
      ))}
      {replies.map((r) => (
        <div className="comment pending" key={r.draft.id}>
          <div className="comment-meta">
            <span className="badge">Draft reply</span>
            <button className="link" onClick={() => onDeleteDraft(r.draft.id)}>
              discard
            </button>
          </div>
          <Markdown text={r.draft.body} breaks />
        </div>
      ))}
      {replying ? (
        <Composer
          title="Reply"
          fileFallback={null}
          saveLabel="Save reply"
          onSave={(text) => {
            onReply(text);
            setReplying(false);
          }}
          onCancel={() => setReplying(false)}
        />
      ) : (
        <div className="card-actions">
          <button className="button" onClick={() => setReplying(true)}>
            Reply
          </button>
          {!thread.resolved &&
            (resolving ? (
              <button className="button" onClick={() => onDeleteDraft(resolving.draft.id)}>
                Resolves on submit · undo
              </button>
            ) : (
              <button className="button" onClick={onResolve}>
                Resolve
              </button>
            ))}
        </div>
      )}
    </div>
  );
}
