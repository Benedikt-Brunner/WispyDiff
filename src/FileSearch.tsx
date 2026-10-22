import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { FileSummary } from "./types";

interface Props {
  files: FileSummary[];
  /** Per file index: left out by the file filter (listed after the others, opens in the file view). */
  filtered: boolean[];
  isViewed: (key: string) => boolean;
  onPick: (index: number) => void;
  onClose: () => void;
}

const MAX_RESULTS = 50;

/** ⇧⇧: the diff's files by fuzzy name, Spotlight style. */
export function FileSearch({ files, filtered, isViewed, onPick, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  const results = useMemo(() => {
    const found: { index: number; score: number; positions: number[] }[] = [];
    files.forEach((f, index) => {
      const match = matchPath(f.path, query);
      if (match) found.push({ index, ...match });
    });
    // Files the filter hides come last; otherwise best match first, ties in diff order.
    found.sort((a, b) => Number(filtered[a.index]) - Number(filtered[b.index]) || b.score - a.score || a.index - b.index);
    return found.slice(0, MAX_RESULTS);
  }, [files, filtered, query]);

  useEffect(() => inputRef.current?.focus(), []);
  useEffect(() => setSelected(0), [query]);
  useEffect(() => listRef.current?.querySelector(".selected")?.scrollIntoView({ block: "nearest" }), [selected]);

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="palette file-search" onMouseDown={(e) => e.stopPropagation()} data-testid="file-search">
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="Find a file in this diff"
          value={query}
          spellCheck={false}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              if (results[selected]) onPick(results[selected].index);
            } else if (e.key === "Escape") onClose();
            else if (e.key === "ArrowDown") setSelected((s) => Math.min(s + 1, results.length - 1));
            else if (e.key === "ArrowUp") setSelected((s) => Math.max(s - 1, 0));
            else return;
            e.preventDefault();
          }}
        />
        {results.length === 0 ? (
          <div className="palette-status">No file matches.</div>
        ) : (
          <ul className="palette-list" ref={listRef}>
            {results.map((r, i) => {
              const file = files[r.index];
              const nameStart = file.path.lastIndexOf("/") + 1;
              return (
                <li
                  key={file.path + r.index}
                  className={[i === selected && "selected", filtered[r.index] && "muted"].filter(Boolean).join(" ")}
                  onMouseEnter={() => setSelected(i)}
                  onClick={() => onPick(r.index)}
                  title={file.path}
                >
                  <span className="file-search-viewed">{isViewed(file.content_key) ? "✓" : ""}</span>
                  <span className="file-search-name">{highlight(file.path, r.positions, nameStart, file.path.length)}</span>
                  <span className="file-search-dir">{highlight(file.path, r.positions, 0, nameStart)}</span>
                  {filtered[r.index] && <span className="file-search-tag">hidden by filter</span>}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}

/** `path[from..to)` with the matched characters marked. */
function highlight(path: string, positions: number[], from: number, to: number) {
  const parts: ReactNode[] = [];
  let run = from;
  for (const p of positions) {
    if (p < from || p >= to) continue;
    if (p > run) parts.push(path.slice(run, p));
    parts.push(<mark key={p}>{path[p]}</mark>);
    run = p + 1;
  }
  if (run < to) parts.push(path.slice(run, to));
  return parts;
}

const BOUNDARY = 8;
const RUN = 5;
const IN_NAME = 2;
const ALL_IN_NAME = 10;

/**
 * Whether `query`'s characters appear in `path` in order (case-insensitive, spaces ignored), and
 * how well: word starts (after / _ - . or a camelCase hump), runs of consecutive characters and
 * the file name count most. The best placement wins (positions: the matched indices).
 */
export function matchPath(path: string, query: string): { score: number; positions: number[] } | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return { score: 0, positions: [] };
  const lower = path.toLowerCase();
  const n = path.length;
  const nameStart = path.lastIndexOf("/") + 1;
  const charScore = (j: number) => {
    const before = path[j - 1];
    const boundary = j === 0 || "/_-. ".includes(before) || (before === before.toLowerCase() && path[j] !== lower[j]);
    return 1 + (boundary ? BOUNDARY : 0) + (j >= nameStart ? IN_NAME : 0);
  };
  // scores[i][j]: the best score with query character i matched at path index j; from: the previous match.
  const scores: Float64Array[] = [];
  const from: Int32Array[] = [];
  for (let i = 0; i < q.length; i++) {
    const row = new Float64Array(n).fill(-Infinity);
    const back = new Int32Array(n).fill(-1);
    const prev = i > 0 ? scores[i - 1] : null;
    let best = prev ? -Infinity : 0;
    let bestAt = -1;
    for (let j = 0; j < n; j++) {
      if (prev && j > 0 && prev[j - 1] > best) {
        best = prev[j - 1];
        bestAt = j - 1;
      }
      if (lower[j] !== q[i]) continue;
      let score = best;
      let at = bestAt;
      if (prev && j > 0 && prev[j - 1] + RUN >= score) {
        score = prev[j - 1] + RUN;
        at = j - 1;
      }
      if (score === -Infinity) continue;
      row[j] = score + charScore(j);
      back[j] = at;
    }
    scores.push(row);
    from.push(back);
  }
  const last = scores[q.length - 1];
  let end = -1;
  for (let j = 0; j < n; j++) if (last[j] > -Infinity && (end < 0 || last[j] > last[end])) end = j;
  if (end < 0) return null;
  const positions: number[] = [];
  for (let i = q.length - 1, j = end; i >= 0; j = from[i][j], i--) positions.unshift(j);
  // Shorter paths win ties.
  const score = last[end] + (positions[0] >= nameStart ? ALL_IN_NAME : 0) - n / 100;
  return { score, positions };
}
