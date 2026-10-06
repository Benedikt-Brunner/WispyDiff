import { useEffect, useRef, useState } from "react";
import { signInAssistant, type AssistantSelection, type AssistantThread, type Provider, type ThreadAnchor } from "./api";
import { loadPref, savePref } from "./prefs";
import { mod, keyLabel } from "./platform";
import { ResizeHandle, useSidebarWidth } from "./Resizable";

const MODELS: Record<Provider, string[]> = {
  claude: ["", "opus", "sonnet", "haiku", "fable"],
  codex: [""],
};
const PROVIDER_NAMES: Record<Provider, string> = { claude: "Claude Code", codex: "Codex" };
const EFFORTS: Record<Provider, string[]> = {
  claude: ["", "low", "medium", "high", "xhigh", "max"],
  codex: ["", "minimal", "low", "medium", "high"],
};

export interface AskContext {
  /** Set when asking about selected lines. */
  selection: AssistantSelection | null;
  anchor: ThreadAnchor | null;
  /** Shown when there's no selection, e.g. "#2–#3". */
  rangeLabel: string;
}

interface Props {
  threads: AssistantThread[];
  activeId: string | null;
  context: AskContext;
  /** The answer being streamed, if any. */
  pending: { threadId: string | null; question: string; text: string } | null;
  /** A short status line (e.g. "Saved as a draft", or an error). */
  note: string | null;
  onSelect: (id: string | null) => void;
  onAsk: (question: string, choice: { provider: Provider; model: string | null; effort: string | null }) => void;
  onDraft: (thread: AssistantThread, text: string) => void;
  onDelete: (id: string) => void;
  onClose: () => void;
}

/** Side-panel conversations with Claude Code or Codex about the range or a selection. */
export function AssistantPanel({ threads, activeId, context, pending, note, onSelect, onAsk, onDraft, onDelete, onClose }: Props) {
  const width = useSidebarWidth("assistant", 420);
  const [provider, setProvider] = useState<Provider>(() => loadPref("assistant.provider", ["claude", "codex"] as const, "claude"));
  const [model, setModel] = useState(() => localValue(`assistant.model.${provider}`));
  const [effort, setEffort] = useState(() => localValue(`assistant.effort.${provider}`));
  const [question, setQuestion] = useState("");
  const [signIn, setSignIn] = useState<{ state: "waiting" | "done" | "failed"; url?: string; message?: string } | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const bottom = useRef<HTMLDivElement>(null);
  const active = threads.find((t) => t.id === activeId) ?? null;

  useEffect(() => input.current?.focus(), [activeId, context]);
  useEffect(() => setSignIn(null), [activeId]);
  useEffect(() => bottom.current?.scrollIntoView({ block: "end" }), [active?.messages.length, pending?.text]);

  const changeProvider = (next: Provider) => {
    setProvider(next);
    savePref("assistant.provider", next);
    setModel(localValue(`assistant.model.${next}`));
    setEffort(localValue(`assistant.effort.${next}`));
  };
  const remember = (key: "model" | "effort", value: string) => {
    savePref(`assistant.${key}.${provider}`, value);
    (key === "model" ? setModel : setEffort)(value);
  };
  const send = () => {
    if (!question.trim() || pending) return;
    onAsk(question.trim(), { provider, model: model || null, effort: effort || null });
    setQuestion("");
  };

  const startSignIn = (thread: AssistantThread) => {
    setSignIn({ state: "waiting" });
    signInAssistant(thread.provider, (url) => setSignIn({ state: "waiting", url }))
      .then(() => setSignIn({ state: "done" }))
      .catch((e) => setSignIn({ state: "failed", message: String(e) }));
  };
  const askAgain = (thread: AssistantThread) => {
    const last = [...thread.messages].reverse().find((m) => m.role === "user");
    if (!last || pending) return;
    setSignIn(null);
    onAsk(last.text, { provider: thread.provider, model: thread.model, effort: thread.effort });
  };

  return (
    <aside className="code-panel assistant-panel" style={{ width: width.width }} data-testid="assistant-panel">
      <ResizeHandle edge="left" {...width} />
      <div className="panel-head">
        <select className="verdict" value={activeId ?? ""} onChange={(e) => onSelect(e.target.value || null)} data-testid="assistant-threads">
          <option value="">New conversation</option>
          {threads.map((t) => (
            <option key={t.id} value={t.id}>
              {t.messages[0]?.text.slice(0, 40) ?? "…"}
            </option>
          ))}
        </select>
        {active && (
          <button className="link" onClick={() => onDelete(active.id)}>
            delete
          </button>
        )}
        <button className="link" onClick={onClose}>
          close
        </button>
      </div>
      <div className="panel-body assistant-body">
        {(active?.messages ?? []).map((m, i) => (
          <div key={i} className={`assistant-message ${m.role}${m.error ? " error" : ""}`}>
            <div className="card-body">{m.text}</div>
            {m.role === "assistant" && !m.error && active && (
              <button className="link" onClick={() => onDraft(active, m.text)}>
                turn into draft comment
              </button>
            )}
            {m.signIn && active && i === active.messages.length - 1 && !pending && (
              <div className="assistant-sign-in" data-testid="assistant-sign-in">
                {signIn === null && (
                  <button className="button primary" onClick={() => startSignIn(active)}>
                    Sign in to {PROVIDER_NAMES[active.provider]} again
                  </button>
                )}
                {signIn?.state === "waiting" && (
                  <span>
                    {signIn.url ? "Finish signing in in your browser…" : "Starting sign-in…"}
                    {signIn.url && <span className="assistant-sign-in-url">{signIn.url}</span>}
                  </span>
                )}
                {signIn?.state === "done" && (
                  <>
                    <span>Signed in.</span>
                    <button className="button primary" onClick={() => askAgain(active)}>
                      Ask again
                    </button>
                  </>
                )}
                {signIn?.state === "failed" && (
                  <>
                    <span>{signIn.message}</span>
                    <button className="link" onClick={() => startSignIn(active)}>
                      try again
                    </button>
                  </>
                )}
              </div>
            )}
          </div>
        ))}
        {pending && pending.threadId === (active?.id ?? null) && (
          <>
            <div className="assistant-message user">
              <div className="card-body">{pending.question}</div>
            </div>
            <div className="assistant-message assistant streaming" data-testid="assistant-streaming">
              <div className="card-body">{pending.text || "Thinking…"}</div>
            </div>
          </>
        )}
        {!active && !pending && (
          <div className="panel-empty">
            {context.selection
              ? `Ask about ${context.selection.path.split("/").pop()} L${context.selection.startLine}–${context.selection.endLine} (${context.selection.prLabel}).`
              : `Ask about ${context.rangeLabel}. Select code first to ask about specific lines.`}
          </div>
        )}
        <div ref={bottom} />
      </div>
      {note && (
        <div className="assistant-note" data-testid="assistant-note">
          {note}
        </div>
      )}
      <div className="assistant-compose">
        {!active && (
          <div className="assistant-context" data-testid="assistant-context">
            {context.selection
              ? `${context.selection.path.split("/").pop()} L${context.selection.startLine}–${context.selection.endLine} · ${context.selection.prLabel}`
              : context.rangeLabel}
          </div>
        )}
        <textarea
          ref={input}
          className="composer-input"
          rows={3}
          value={question}
          placeholder={`${active ? "Follow up…" : "Ask…"} (${keyLabel("↵")})`}
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && mod(e)) {
              e.preventDefault();
              send();
            } else if (e.key === "Escape") onClose();
            if (!mod(e)) e.stopPropagation();
          }}
          data-testid="assistant-input"
        />
        <div className="assistant-options">
          <select className="verdict" value={provider} onChange={(e) => changeProvider(e.target.value as Provider)} disabled={!!active}>
            {(["claude", "codex"] as const).map((p) => (
              <option key={p} value={p}>
                {PROVIDER_NAMES[p]}
              </option>
            ))}
          </select>
          {provider === "claude" ? (
            <select className="verdict" value={model} onChange={(e) => remember("model", e.target.value)} disabled={!!active}>
              {MODELS.claude.map((m) => (
                <option key={m} value={m}>
                  {m || "default model"}
                </option>
              ))}
            </select>
          ) : (
            <input
              className="panel-input assistant-model"
              value={model}
              placeholder="default model"
              onChange={(e) => remember("model", e.target.value)}
              disabled={!!active}
            />
          )}
          <select className="verdict" value={effort} onChange={(e) => remember("effort", e.target.value)} disabled={!!active}>
            {EFFORTS[provider].map((level) => (
              <option key={level} value={level}>
                {level || "default effort"}
              </option>
            ))}
          </select>
          <button className="button primary" onClick={send} disabled={!question.trim() || !!pending} data-testid="assistant-send">
            {pending ? "…" : "Ask"}
          </button>
        </div>
      </div>
    </aside>
  );
}

function localValue(key: string) {
  try {
    return localStorage.getItem(`wispy.${key}`) ?? "";
  } catch {
    return "";
  }
}
