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
}

export interface PrRef {
  owner: string;
  repo: string;
  number: number;
}

export interface OpenedPr {
  viewId: string;
  pr: PrRef;
  pullRequest: PullRequest;
  summary: DiffSummary;
  fromCache: boolean;
}

export const prLabel = (pr: PrRef) => `${pr.owner}/${pr.repo}#${pr.number}`;
