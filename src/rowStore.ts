import { getHeadLines, getRows, getSplitRows } from "./api";
import { RowKind, type Row, type Seg, type SplitRow } from "./types";

const PAGE_SIZE = 256;

type Loaded = { mode: "unified"; rows: Row[] } | { mode: "split"; rows: SplitRow[] };

/**
 * Fetches a file's rows (unified or side-by-side) in pages, on demand, by offset within the
 * file. Offset 0 is the header, which the frontend draws itself.
 */
export class RowStore {
  private pages = new Map<string, Loaded>();
  private pending = new Set<string>();
  /** Head lines shown from gaps, per file. */
  private lines = new Map<number, Map<number, Seg[]>>();
  /** The rows made for them, so each stays the same object (rows are memoized). */
  private contextRows = new Map<string, Row>();

  constructor(
    private readonly viewId: string,
    private readonly onPageLoaded: () => void,
  ) {}

  unified(file: number, offset: number): Row | undefined {
    const page = this.pages.get(key(file, "unified", Math.floor(offset / PAGE_SIZE)));
    return page?.mode === "unified" ? page.rows[offset % PAGE_SIZE] : undefined;
  }

  split(file: number, offset: number): SplitRow | undefined {
    const index = Math.floor(offset / PAGE_SIZE);
    const page = this.pages.get(key(file, "split", index));
    if (page?.mode !== "split") return undefined;
    // Page 0 starts at offset 1 (offset 0 is the header).
    return page.rows[offset - index * PAGE_SIZE - (index === 0 ? 1 : 0)];
  }

  /** Head line `n` of a file, if loaded (see `loadLines`). */
  line(file: number, n: number): Seg[] | undefined {
    return this.lines.get(file)?.get(n);
  }

  /** An unchanged line shown from a gap, as a context row (undefined until loaded). */
  contextRow(file: number, o: number, n: number): Row | undefined {
    const key = `${file}:${n}`;
    let row = this.contextRows.get(key);
    if (!row) {
      const s = this.line(file, n);
      if (!s) return undefined;
      row = { k: RowKind.Context, f: file, o, n, s, a: null, h: [], l: null, w: [] };
      this.contextRows.set(key, row);
    }
    return row;
  }

  /** Loads head lines `start..=end` of a file (resolves at once when they're there). */
  async loadLines(file: number, start: number, end: number) {
    const known = this.lines.get(file) ?? new Map<number, Seg[]>();
    this.lines.set(file, known);
    while (start <= end && known.has(start)) start++;
    while (end >= start && known.has(end)) end--;
    if (start > end) return;
    const lines = await getHeadLines(this.viewId, file, start, end);
    lines.forEach((segs, i) => known.set(start + i, segs));
  }

  /** Makes sure offsets [start, end) of a file are loaded or loading, plus one page of lookahead. */
  ensure(file: number, mode: "unified" | "split", start: number, end: number) {
    const first = Math.max(0, Math.floor(start / PAGE_SIZE));
    const last = Math.floor(Math.max(start, end - 1) / PAGE_SIZE) + 1;
    for (let page = first; page <= last; page++) this.load(file, mode, page);
  }

  private load(file: number, mode: "unified" | "split", page: number) {
    const k = key(file, mode, page);
    if (this.pages.has(k) || this.pending.has(k)) return;
    this.pending.add(k);
    const start = page * PAGE_SIZE;
    const request: Promise<Loaded> =
      mode === "unified"
        ? getRows(this.viewId, file, start, start + PAGE_SIZE).then((rows) => ({ mode, rows }))
        : getSplitRows(this.viewId, file, start, start + PAGE_SIZE).then((rows) => ({ mode, rows }));
    request
      .then((loaded) => {
        this.pages.set(k, loaded);
        this.onPageLoaded();
      })
      .catch((err) => console.error("failed to load rows", err))
      .finally(() => this.pending.delete(k));
  }
}

const key = (file: number, mode: string, page: number) => `${file}:${mode}:${page}`;
