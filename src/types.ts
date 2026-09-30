/** Mirrors wispy-core's serialized types. */

/** [token class, text]; class 0 is plain. */
export type Seg = [number, string];

export const RowKind = { File: 0, Hunk: 1, Context: 2, Added: 3, Deleted: 4, Notice: 5 } as const;

export interface Row {
  k: number;
  f: number;
  o: number | null;
  n: number | null;
  s: Seg[];
  /** Stack index of the PR that last touched this line (added/deleted rows). */
  a: number | null;
  /** Stack indices of every PR that shaped this line, when more than one did. */
  h: number[];
}

export type FileStatus = "added" | "deleted" | "modified" | "renamed" | "copied";

export interface FileSummary {
  path: string;
  old_path: string | null;
  status: FileStatus;
  additions: number;
  deletions: number;
  binary: boolean;
  language: string | null;
  first_row: number;
  row_count: number;
  /** Stack indices of the PRs that touched this file within the range. */
  prs: number[];
}

export interface DiffSummary {
  base_sha: string;
  head_sha: string;
  files: FileSummary[];
  hunk_rows: number[];
  total_rows: number;
  max_line_chars: number;
  max_line_number: number;
  additions: number;
  deletions: number;
}

export interface PullRequest {
  number: number;
  title: string;
  state: string;
  draft: boolean;
  html_url: string;
  author: string;
  base_ref: string;
  base_sha: string;
  head_ref: string;
  head_sha: string;
  clone_url: string;
  base_repo: string;
  head_repo: string | null;
  default_branch: string;
}

/** Inclusive range of stack indices. */
export interface Range {
  lo: number;
  hi: number;
}

export interface OpenedRange {
  viewId: string;
  range: Range;
  summary: DiffSummary;
}

export interface OpenedStack extends OpenedRange {
  stackId: string;
  /** Bottom to top. */
  prs: PullRequest[];
  needsRebase: boolean[];
  focus: number;
  fromCache: boolean;
}

export const prLabel = (pr: PullRequest) => `${pr.base_repo}#${pr.number}`;

/** Per-PR colors cycle through six hues (see `--pr0`…`--pr5` in styles.css). */
export const prColor = (index: number) => `var(--pr${index % 6})`;
