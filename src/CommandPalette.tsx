import { useEffect, useMemo, useRef, useState } from "react";
import { recentPrs } from "./recent";

interface Props {
  busy: string | null;
  error: string | null;
  onOpen: (input: string) => void;
  onClose: (() => void) | null;
}

export function CommandPalette({ busy, error, onOpen, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const recents = useMemo(recentPrs, []);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (q ? recents.filter((r) => fuzzy(r.toLowerCase(), q)) : recents).slice(0, 8);
  }, [query, recents]);

  useEffect(() => inputRef.current?.focus(), []);
  useEffect(() => setSelected(0), [query]);

  const submit = () => {
    const chosen = matches[selected] && !looksLikePr(query) ? matches[selected] : query.trim();
    if (chosen) onOpen(chosen);
  };

  return (
    <div className="palette-backdrop" onMouseDown={() => onClose?.()}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="Paste a PR URL or owner/repo#123"
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
          <ul className="palette-list">
            {matches.map((label, i) => (
              <li
                key={label}
                className={i === selected ? "selected" : undefined}
                onMouseEnter={() => setSelected(i)}
                onClick={() => onOpen(label)}
              >
                {label}
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
