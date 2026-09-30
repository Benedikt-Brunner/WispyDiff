import { getRows, getSplitRows } from "./api";
import type { Row, SplitRow } from "./types";

const PAGE_SIZE = 256;

type Loaded = { mode: "unified"; rows: Row[] } | { mode: "split"; rows: SplitRow[] };

/**
 * Fetches a file's rows (unified or side-by-side) in pages, on demand, by offset within the
 * file. Offset 0 is the header, which the frontend draws itself.
 */
export class RowStore {
  private pages = new Map<string, Loaded>();
  private pending = new Set<string>();

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
