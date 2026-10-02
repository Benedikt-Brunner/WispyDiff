import type { DiffSummary, FileSummary } from "./types";

export const ROW_HEIGHT = 20;

export type Mode = "unified" | "split" | "collapsed";

/** One file's rows in the chosen mode; `start` is its first global row. */
export interface Segment {
  file: number;
  mode: Mode;
  start: number;
  rows: number;
}

/** A variable-height block (comment box, thread, ...) drawn below global row `after`. */
export interface Insert {
  key: string;
  after: number;
  height: number;
}

export function segmentRows(file: FileSummary, mode: Mode) {
  if (mode === "collapsed") return 2; // header + "collapsed" notice
  if (mode === "split") return file.split_rows;
  return file.row_count;
}

/** `[offset, extra lines]` of a file's rows that wrap onto more than one line, by offset. */
export type WrappedRows = readonly (readonly [number, number])[];

/**
 * All files stacked in their modes, plus inserts. A row is ROW_HEIGHT tall, or a multiple when
 * it wraps; inserts add their height below their row. So positions are
 * `row * ROW_HEIGHT + (extra lines of wrapped rows above) + (heights of inserts above)`.
 */
export class Layout {
  readonly segments: Segment[];
  readonly totalRows: number;
  private readonly inserts: Insert[];
  /** cumulative[i] = total height of inserts[0..i). */
  private readonly cumulative: number[];
  /** Global rows that wrap, ascending, and wrappedCumulative[i] = extra height of wrapped[0..i). */
  private readonly wrapped: number[] = [];
  private readonly wrappedCumulative: number[] = [0];

  constructor(summary: DiffSummary, modeOf: (index: number) => Mode, inserts: Insert[] = [], wrappedOf?: (file: number, mode: Mode) => WrappedRows) {
    let start = 0;
    this.segments = summary.files.map((file, index) => {
      const mode = modeOf(index);
      const segment = { file: index, mode, start, rows: segmentRows(file, mode) };
      for (const [offset, extra] of wrappedOf?.(index, mode) ?? []) {
        if (offset >= segment.rows) break;
        this.wrapped.push(start + offset);
        this.wrappedCumulative.push(this.wrappedCumulative[this.wrappedCumulative.length - 1] + extra * ROW_HEIGHT);
      }
      start += segment.rows;
      return segment;
    });
    this.totalRows = start;
    this.inserts = [...inserts].sort((a, b) => a.after - b.after);
    this.cumulative = [0];
    for (const insert of this.inserts) this.cumulative.push(this.cumulative[this.cumulative.length - 1] + insert.height);
  }

  get height() {
    return this.totalRows * ROW_HEIGHT + this.wrappedCumulative[this.wrapped.length] + this.cumulative[this.cumulative.length - 1];
  }

  /** Top of global row `row`. */
  rowY(row: number) {
    return row * ROW_HEIGHT + this.wrappedCumulative[this.wrappedBefore(row)] + this.cumulative[this.insertsBefore(row)];
  }

  /** Height of global row `row` (taller when it wraps). */
  rowHeight(row: number) {
    const i = this.wrappedBefore(row);
    return this.wrapped[i] === row ? ROW_HEIGHT + this.wrappedCumulative[i + 1] - this.wrappedCumulative[i] : ROW_HEIGHT;
  }

  /** The row at vertical position `y` (an insert counts as part of the row above it). */
  rowAt(y: number) {
    let lo = 0;
    let hi = Math.max(0, this.totalRows - 1);
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.rowY(mid) <= y) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  /** Inserts below rows `first..=last`, with the top of each (stacked in order under their row). */
  insertsBetween(first: number, last: number): (Insert & { y: number })[] {
    const out: (Insert & { y: number })[] = [];
    this.inserts.forEach((insert, i) => {
      if (insert.after >= first && insert.after <= last) {
        const below = insert.after + 1;
        out.push({ ...insert, y: below * ROW_HEIGHT + this.wrappedCumulative[this.wrappedBefore(below)] + this.cumulative[i] });
      }
    });
    return out;
  }

  segmentAt(row: number): Segment {
    let lo = 0;
    let hi = this.segments.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.segments[mid].start <= row) lo = mid;
      else hi = mid - 1;
    }
    return this.segments[lo];
  }

  /** Global rows where changes start: hunks (unified), change blocks (split), or the file (collapsed). */
  changeRows(summary: DiffSummary): number[] {
    return this.segments.flatMap((s) => {
      const file = summary.files[s.file];
      if (s.mode === "collapsed") return [s.start];
      return (s.mode === "split" ? file.split_blocks : file.hunks).map((offset) => s.start + offset);
    });
  }

  private wrappedBefore(row: number) {
    let lo = 0;
    let hi = this.wrapped.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.wrapped[mid] < row) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  private insertsBefore(row: number) {
    let lo = 0;
    let hi = this.inserts.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.inserts[mid].after < row) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }
}
