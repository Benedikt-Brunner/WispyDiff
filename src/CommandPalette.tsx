import { useEffect, useMemo, useRef, useState } from "react";
import { recentPrs } from "./recent";

/** A palette action, shown when the input starts with ">". */
export interface Command {
  id: string;
  label: string;
  /** Commands with a keyword take the rest of the input as their argument (`> ignore src/**`). */
  keyword?: string;
  run: (argument: string) => void;
}

interface Props {
  busy: string | null;
  error: string | null;
  commands: Command[];
  onOpen: (input: string) => void;
  onClose: (() => void) | null;
}

export function CommandPalette({ busy, error, commands, onOpen, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const recents = useMemo(recentPrs, []);

  const commandMode = query.startsWith(">");
  const matches = useMemo(() => {
    if (commandMode) {
      const q = query.slice(1).trim().toLowerCase();
      return commands
        .filter((c) => (c.keyword && q.startsWith(c.keyword + " ")) || fuzzy(c.label.toLowerCase(), q))
        .map((c) => {
          const argument = c.keyword && q.startsWith(c.keyword + " ") ? query.slice(1).trim().slice(c.keyword.length + 1) : "";
          return { id: c.id, label: argument ? `${c.label} “${argument}”` : c.label, run: () => c.run(argument) };
        })
        .slice(0, 10);
    }
    const q = query.trim().toLowerCase();
    return (q ? recents.filter((r) => fuzzy(r.toLowerCase(), q)) : recents)
      .slice(0, 8)
      .map((label) => ({ id: label, label, run: () => onOpen(label) }));
  }, [query, commandMode, commands, recents, onOpen]);

  useEffect(() => inputRef.current?.focus(), []);
  useEffect(() => setSelected(0), [query]);

  const submit = () => {
    if (!commandMode && looksLikePr(query)) return onOpen(query.trim());
    const chosen = matches[selected];
    if (chosen) chosen.run();
    else if (!commandMode && query.trim()) onOpen(query.trim());
  };

  return (
    <div className="palette-backdrop" onMouseDown={() => onClose?.()}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="Paste a PR URL or owner/repo#123 · > for commands"
          value={query}
          disabled={busy !== null}
          spellCheck={false}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
            else if (e.key === "Escape") onClose?.();
            else if (e.key === "ArrowDown") setSelected((s) => Math.min(s + 1, matches.length - 1));
            else if (e.key === "ArrowUp") setSelected((s) => Math.max(s - 1, 0));
            else return;
            e.preventDefault();
          }}
        />
        {busy && <div className="palette-status">Loading {busy}…</div>}
        {error && !busy && <div className="palette-error">{error}</div>}
        {!busy && matches.length > 0 && (
          <ul className={`palette-list${commandMode ? " commands" : ""}`}>
            {matches.map((m, i) => (
              <li key={m.id} className={i === selected ? "selected" : undefined} onMouseEnter={() => setSelected(i)} onClick={m.run}>
                {m.label}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

const looksLikePr = (s: string) => /#\d+\s*$|\/pull\/\d+/.test(s);

function fuzzy(haystack: string, needle: string) {
  let i = 0;
  for (const ch of haystack) if (ch === needle[i]) i++;
  return i === needle.length;
}
