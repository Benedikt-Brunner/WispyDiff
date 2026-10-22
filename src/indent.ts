/** Tab / Shift+Tab and Enter inside a comment's ```suggestion block. */

export interface Edit {
  /** Replace text[from, to) with `insert`, then select [start, end). */
  from: number;
  to: number;
  insert: string;
  start: number;
  end: number;
}

/**
 * Indents (or outdents) by `unit` the lines of a ```suggestion block that the selection
 * [start, end) touches. Null when the selection isn't inside one block's lines, or nothing changes.
 */
/** Whether lines [first, last] all sit inside one ```suggestion block, between its fences. */
function inBlock(lines: string[], first: number, last: number): boolean {
  let open: number | null = null;
  for (let i = 0; i < lines.length; i++) {
    if (open === null && /^\s*```suggestion\s*$/.test(lines[i])) open = i;
    else if (open !== null && /^\s*```\s*$/.test(lines[i])) {
      if (open < first && last < i) return true;
      open = null;
    }
  }
  return false;
}

/**
 * Enter inside a ```suggestion block: replaces the selection [start, end) with a line break and
 * the indentation of the caret's line (up to the caret). Null when the selection isn't inside one block's lines.
 */
export function newline(text: string, start: number, end: number): Edit | null {
  const lines = text.split("\n");
  const first = text.slice(0, start).split("\n").length - 1;
  const last = text.slice(0, end).split("\n").length - 1;
  if (!inBlock(lines, first, last)) return null;
  const lineStart = text.lastIndexOf("\n", start - 1) + 1;
  const indentation = /^[ \t]*/.exec(text.slice(lineStart, start))![0];
  const insert = "\n" + indentation;
  return { from: start, to: end, insert, start: start + insert.length, end: start + insert.length };
}

export function reindent(text: string, start: number, end: number, unit: string, outdent: boolean): Edit | null {
  const lines = text.split("\n");
  const lineStarts: number[] = [];
  let at = 0;
  for (const line of lines) {
    lineStarts.push(at);
    at += line.length + 1;
  }
  const lineOf = (pos: number) => {
    let i = lineStarts.length - 1;
    while (lineStarts[i] > pos) i--;
    return i;
  };
  const first = lineOf(start);
  // A selection ending at the start of a line doesn't take that line along.
  const last = end > start && end === lineStarts[lineOf(end)] ? lineOf(end) - 1 : lineOf(end);

  if (!inBlock(lines, first, last)) return null;

  const outdentWidth = unit === "\t" ? 4 : unit.length;
  const deltas = new Map<number, number>();
  const changed: string[] = [];
  for (let i = first; i <= last; i++) {
    const line = lines[i];
    let next = line;
    if (outdent) next = line.startsWith("\t") ? line.slice(1) : line.slice(Math.min(outdentWidth, line.length - line.trimStart().length));
    // A multi-line selection leaves its empty lines empty.
    else if (line !== "" || first === last) next = unit + line;
    deltas.set(i, next.length - line.length);
    changed.push(next);
  }
  const from = lineStarts[first];
  const to = lineStarts[last] + lines[last].length;
  const insert = changed.join("\n");
  if (insert === text.slice(from, to)) return null;

  /** Where `pos` ends up: it moves with its line's text, and stays at the line start if it was there in a selection. */
  const shifted = (pos: number) => {
    const line = Math.min(Math.max(lineOf(pos), first), last);
    const column = pos - lineStarts[line];
    const before = [...deltas].filter(([i]) => i < line).reduce((sum, [, d]) => sum + d, 0);
    const delta = deltas.get(line) ?? 0;
    const moved = column === 0 && end > start ? 0 : Math.max(0, column + delta);
    return lineStarts[line] + before + moved;
  };
  return { from, to, insert, start: shifted(start), end: end > start && last < lineOf(end) ? end + insert.length - (to - from) : shifted(end) };
}
