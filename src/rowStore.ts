import { getRows } from "./api";
import type { Row } from "./types";

const PAGE_SIZE = 512;

/** Fetches rows from the backend in pages, on demand, and keeps them for the view's lifetime. */
export class RowStore {
  private pages = new Map<number, Row[]>();
  private pending = new Set<number>();

  constructor(
    private readonly viewId: string,
    private readonly totalRows: number,
    private readonly onPageLoaded: () => void,
  ) {}

  get(index: number): Row | undefined {
    return this.pages.get(Math.floor(index / PAGE_SIZE))?.[index % PAGE_SIZE];
  }

  /** Makes sure rows [start, end) are loaded or loading, plus one page of lookahead each way. */
  ensure(start: number, end: number) {
    const first = Math.max(0, Math.floor(start / PAGE_SIZE) - 1);
    const last = Math.min(Math.floor((this.totalRows - 1) / PAGE_SIZE), Math.floor(end / PAGE_SIZE) + 1);
    for (let page = first; page <= last; page++) this.load(page);
  }

  private load(page: number) {
    if (this.pages.has(page) || this.pending.has(page)) return;
    this.pending.add(page);
    const start = page * PAGE_SIZE;
    getRows(this.viewId, start, start + PAGE_SIZE)
      .then((rows) => {
        this.pages.set(page, rows);
        this.onPageLoaded();
      })
      .catch((err) => console.error("failed to load rows", err))
      .finally(() => this.pending.delete(page));
  }
}
