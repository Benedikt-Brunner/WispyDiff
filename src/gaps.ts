import type { FileSummary, Gap, Seg } from "./types";

/** Lines a click on a gap shows. */
export const GAP_STEP = 20;

/** Per gap of a file (by index): how many of its lines are shown [below the hunk above, above the hunk below]. */
export type Reveal = readonly (readonly [number, number])[];

type Piece =
  /** Unified rows `base..base + n` of the file. */
  | { kind: "base"; at: number; n: number; base: number }
  /** Shown lines of gap `gap`. */
  | { kind: "lines"; at: number; n: number; gap: number; newStart: number; oldStart: number }
  /** The row standing for what's still hidden of gap `gap`: its hunk header (`base`), or a row of its own after the last hunk. */
  | { kind: "gap"; at: number; n: 1; gap: number; base: number | null };

export type DisplayRow =
  | { kind: "base"; base: number }
  | { kind: "line"; o: number; n: number }
  | { kind: "gap"; gap: number; base: number | null };

/** How many lines of `gap` are still hidden. */
export const hiddenLines = (gap: Gap, shown: readonly [number, number] | undefined) => Math.max(0, gap.len - (shown ? shown[0] + shown[1] : 0));

/**
 * A file's unified rows with some of its gaps (unchanged lines left out between hunks) shown:
 * maps the rows as drawn ("display" offsets, header = 0) to the file's own unified rows and the
 * shown head lines. A gap row stays for whatever is still hidden of each gap.
 */
export class UnifiedMap {
  readonly rows: number;
  private readonly pieces: Piece[] = [];

  constructor(
    readonly file: FileSummary,
    readonly reveal: Reveal | undefined,
  ) {
    let base = 0;
    let at = 0;
    const push = (piece: Piece) => {
      if (piece.n > 0) this.pieces.push(piece);
      at += piece.n;
    };
    file.gaps.forEach((gap, index) => {
      const trailing = gap.offset >= file.row_count;
      push({ kind: "base", at, n: Math.min(gap.offset, file.row_count) - base, base });
      const [down, up] = reveal?.[index] ?? [0, 0];
      if (down + up >= gap.len) {
        push({ kind: "lines", at, n: gap.len, gap: index, newStart: gap.new_start, oldStart: gap.old_start });
      } else {
        push({ kind: "lines", at, n: down, gap: index, newStart: gap.new_start, oldStart: gap.old_start });
        push({ kind: "gap", at, n: 1, gap: index, base: trailing ? null : gap.offset });
        const skip = gap.len - up;
        push({ kind: "lines", at, n: up, gap: index, newStart: gap.new_start + skip, oldStart: gap.old_start + skip });
      }
      base = trailing ? file.row_count : gap.offset + 1;
    });
    push({ kind: "base", at, n: file.row_count - base, base });
    this.rows = at;
  }

  /** What display row `offset` is. */
  at(offset: number): DisplayRow {
    const piece = this.pieceAt(offset);
    if (!piece) return { kind: "base", base: offset };
    const i = offset - piece.at;
    if (piece.kind === "base") return { kind: "base", base: piece.base + i };
    if (piece.kind === "lines") return { kind: "line", o: piece.oldStart + i, n: piece.newStart + i };
    return { kind: "gap", gap: piece.gap, base: piece.base };
  }

  /** Where unified row `base` is drawn (a hunk header shown away: where its hunk starts). */
  fromBase(base: number): number {
    for (const piece of this.pieces) {
      if (piece.kind === "base" && base < piece.base + piece.n) return piece.at + Math.max(0, base - piece.base);
      if (piece.kind === "gap" && piece.base === base) return piece.at;
    }
    return this.rows;
  }

  /** Where the row standing for gap `index` is drawn, if the gap isn't all shown. */
  gapRow(index: number): number | null {
    for (const piece of this.pieces) if (piece.kind === "gap" && piece.gap === index) return piece.at;
    return null;
  }

  /** Where shown head line `line` of a gap is drawn, if it is. */
  fromNewLine(line: number): number | null {
    for (const piece of this.pieces) if (piece.kind === "lines" && line >= piece.newStart && line < piece.newStart + piece.n) return piece.at + line - piece.newStart;
    return null;
  }

  /** The unified rows and head lines that display rows `start..end` need loaded. */
  needs(start: number, end: number): { base: [number, number] | null; lines: [number, number] | null } {
    let base: [number, number] | null = null;
    let lines: [number, number] | null = null;
    for (const piece of this.pieces) {
      const from = Math.max(start, piece.at);
      const to = Math.min(end, piece.at + piece.n);
      if (from >= to) continue;
      if (piece.kind === "lines") {
        const range: [number, number] = [piece.newStart + from - piece.at, piece.newStart + to - piece.at];
        lines = lines ? [Math.min(lines[0], range[0]), Math.max(lines[1], range[1])] : range;
      } else {
        const b = piece.kind === "base" ? piece.base + from - piece.at : piece.base;
        if (b === null) continue;
        const range: [number, number] = [b, piece.kind === "base" ? piece.base + to - piece.at : b + 1];
        base = base ? [Math.min(base[0], range[0]), Math.max(base[1], range[1])] : range;
      }
    }
    return { base, lines };
  }

  /** `[display offset, width]` of every shown gap line, given the loaded head lines. */
  lineWidths(line: (n: number) => Seg[] | undefined): [number, number][] {
    const out: [number, number][] = [];
    for (const piece of this.pieces) {
      if (piece.kind !== "lines") continue;
      for (let i = 0; i < piece.n; i++) {
        const segs = line(piece.newStart + i);
        if (segs) out.push([piece.at + i, textWidth(segs)]);
      }
    }
    return out;
  }

  private pieceAt(offset: number): Piece | undefined {
    let lo = 0;
    let hi = this.pieces.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.pieces[mid].at <= offset) lo = mid;
      else hi = mid - 1;
    }
    return this.pieces[lo];
  }
}

/** Shows `count` more lines of gap `index` (`down`: below the hunk above it), or all with `count = Infinity`. */
export function revealMore(file: FileSummary, reveal: Reveal | undefined, index: number, down: boolean, count: number): Reveal {
  const next = file.gaps.map((_, i) => reveal?.[i] ?? ([0, 0] as const));
  const gap = file.gaps[index];
  const [d, u] = next[index];
  const add = Math.min(count, hiddenLines(gap, next[index]));
  next[index] = down ? [d + add, u] : [d, u + add];
  return next;
}

/** Every gap shown. */
export const revealAll = (file: FileSummary): Reveal => file.gaps.map((g) => [g.len, 0] as const);

export const allShown = (file: FileSummary, reveal: Reveal | undefined) => file.gaps.every((g, i) => hiddenLines(g, reveal?.[i]) === 0);

const TAB_WIDTH = 8;

/** Monospace columns, as wispy-core's `wrap::text_width` counts them. */
export function textWidth(segs: Seg[]): number {
  let width = 0;
  for (const [, text] of segs) {
    for (const ch of text) {
      const code = ch.codePointAt(0)!;
      width += ch === "\t" ? TAB_WIDTH - (width % TAB_WIDTH) : isWide(code) ? 2 : 1;
    }
  }
  return width;
}

const WIDE: [number, number][] = [
  [0x1100, 0x115f], [0x2e80, 0x303e], [0x3041, 0x33ff], [0x3400, 0x4dbf], [0x4e00, 0x9fff], [0xa000, 0xa4cf], [0xac00, 0xd7a3],
  [0xf900, 0xfaff], [0xfe30, 0xfe4f], [0xff00, 0xff60], [0xffe0, 0xffe6], [0x1f300, 0x1f64f], [0x1f900, 0x1f9ff], [0x20000, 0x3fffd],
];
const isWide = (code: number) => WIDE.some(([lo, hi]) => code >= lo && code <= hi);
