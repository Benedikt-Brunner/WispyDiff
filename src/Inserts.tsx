import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ReviewThread, ShownDraft } from "./comments";

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
        placeholder={`Leave a comment · ⌘↵ to save${canSuggest ? " · ⌘G to suggest a change" : ""} · Esc to cancel`}
        rows={Math.min(20, Math.max(3, text.split("\n").length))}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && e.metaKey) {
            e.preventDefault();
            save();
          } else if (e.key.toLowerCase() === "g" && e.metaKey && canSuggest) {
            e.preventDefault();
            e.stopPropagation();
            suggest();
          } else if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
          }
          // Plain keys stay in the editor; ⌘ shortcuts (⌘K, ⌘I, …) still reach the app.
          if (!e.metaKey) e.stopPropagation();
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
          <button className="button composer-suggest" onClick={suggest} title="Suggest a change to these lines (⌘G)">
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
        <span className="card-title">{label}</span>
        {outdated && <span className="badge warn">outdated — lines changed since</span>}
        {draft.status === "posting" && <span className="badge">sending…</span>}
      </div>
      <div className="card-body">{draft.body}</div>
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

interface ThreadCardProps {
  thread: ReviewThread;
  label: string;
  replies: ShownDraft[];
  resolving: ShownDraft | undefined;
  onReply: (text: string) => void;
  onResolve: () => void;
  onDeleteDraft: (id: string) => void;
}

export function ThreadCard({ thread, label, replies, resolving, onReply, onResolve, onDeleteDraft }: ThreadCardProps) {
  const [open, setOpen] = useState(!thread.resolved);
  const [replying, setReplying] = useState(false);
  // Collapse when the thread becomes resolved (e.g. after submitting a queued resolve).
  useEffect(() => {
    if (thread.resolved) setOpen(false);
  }, [thread.resolved]);
  if (!open) {
    return (
      <div className="card thread collapsed" onClick={() => setOpen(true)}>
        <span className="badge">{thread.resolved ? "Resolved" : "Thread"}</span>
        <span className="card-title">
          {label} · {thread.comments.length} comment{thread.comments.length === 1 ? "" : "s"}
        </span>
      </div>
    );
  }
  return (
    <div className={`card thread${thread.resolved ? " resolved" : ""}`} data-thread={thread.id}>
      <div className="card-head">
        <span className="card-title">{label}</span>
        {thread.outdated && <span className="badge">outdated</span>}
        {thread.resolved && (
          <button className="link" onClick={() => setOpen(false)}>
            collapse
          </button>
        )}
      </div>
      {thread.comments.map((c) => (
        <div className="comment" key={c.id}>
          <div className="comment-meta">
            <span className="comment-author">{c.author}</span> · {c.created_at.slice(0, 10)}
          </div>
          <div className="card-body">{c.body.replace(/<!-- wispydiff:[^>]* -->/g, "").trim()}</div>
        </div>
      ))}
      {replies.map((r) => (
        <div className="comment pending" key={r.draft.id}>
          <div className="comment-meta">
            <span className="badge">Draft reply</span>
            <button className="link" onClick={() => onDeleteDraft(r.draft.id)}>
              discard
            </button>
          </div>
          <div className="card-body">{r.draft.body}</div>
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
