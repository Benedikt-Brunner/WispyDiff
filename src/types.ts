/** Mirrors wispy-core's serialized types. */

/** [token class, text]; class 0 is plain. */
export type Seg = [number, string];

export const RowKind = { File: 0, Hunk: 1, Context: 2, Added: 3, Deleted: 4, Notice: 5, Filler: 6 } as const;

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
  /** The line's number in PR `a`'s own diff. */
  l: number | null;
}

/** One aligned side-by-side row: old line opposite new line (either may be a filler). */
export interface SplitRow {
  o: number | null;
  n: number | null;
  ok: number;
  nk: number;
  os: Seg[];
  ns: Seg[];
  oa: number | null;
  na: number | null;
  ol: number | null;
  nl: number | null;
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
  old_blob: string | null;
  new_blob: string | null;
  /** Offsets of hunk headers within the file's unified rows (header = 0). */
  hunks: number[];
  /** Side-by-side row count (header included); 0 = not available. */
  split_rows: number;
  split_blocks: number[];
  /** Why the file is collapsed by default, if it is. */
  noise: string | null;
  old_lines: number;
  new_lines: number;
  /** [stack index, path] where a PR knew the file under another path. */
  pr_paths: [number, string][];
  /** Identifies the file's change by content: viewed marks survive rebases that don't touch it. */
  content_key: string;
}

export interface DiffSummary {
  base_sha: string;
  head_sha: string;
  files: FileSummary[];
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
  ignoreWhitespace: boolean;
  summary: DiffSummary;
  /** Set for "changes since checkpoint" views. */
  since: {
    checkpointId: string;
    createdAt: number;
    conflicts: boolean;
    /** Each file's content key in the range's full diff, by path. */
    fullKeys: Record<string, string>;
  } | null;
}

/** "Reviewed these PRs at these heads" (local only). */
export interface Checkpoint {
  id: string;
  repo: string;
  createdAt: number;
  source: "manual" | "submit";
  entries: { pr: number; head: string; base: string }[];
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
