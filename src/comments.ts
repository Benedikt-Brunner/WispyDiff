/** Review comments: mirrors of wispy-core's draft / thread / review types. */
import type { FileSummary } from "./types";

export type Side = "LEFT" | "RIGHT";

/** A PR across repos, for per-PR preferences. */
export const prKey = (pr: { base_repo: string; number: number }) => `${pr.base_repo}#${pr.number}`;
export type Verdict = "COMMENT" | "APPROVE" | "REQUEST_CHANGES";

/** A PR-level position: `line` on `side` of `path` in stack PR `pr`'s own diff. */
export interface Anchor {
  pr: number;
  path: string;
  side: Side;
  line: number;
}

export interface Location {
  file: number;
  unified: number | null;
  split: number | null;
}

export type DraftKind = "line" | "file" | "summary" | "reply" | "resolve";

export interface Draft {
  id: string;
  repo: string;
  pr: number;
  kind: DraftKind;
  path: string | null;
  side: Side | null;
  line: number | null;
  startLine: number | null;
  commit: string;
  base: string;
  body: string;
  threadId: string | null;
  replyTo: number | null;
  asFile: boolean;
  /** Written by the assistant. */
  assistant: boolean;
  status: "draft" | "posting" | "posted" | "failed";
  error: string | null;
}

/** A pending draft with its anchor moved to the PR's current head (`line` null = outdated). */
export interface ShownDraft {
  draft: Draft;
  prIndex: number;
  path: string | null;
  line: number | null;
  startLine: number | null;
}

export interface NewDraft {
  prIndex: number;
  kind: DraftKind;
  path: string | null;
  side: Side | null;
  line: number | null;
  startLine: number | null;
  body: string;
  threadId: string | null;
  replyTo: number | null;
  assistant?: boolean;
}

export interface ThreadComment {
  id: string;
  database_id: number;
  author: string;
  body: string;
  /** GitHub's rendering of `body` (empty in older caches). */
  body_html: string;
  /** Hidden on GitHub (minimized). */
  minimized: boolean;
  created_at: string;
  url: string;
}

export interface ReviewThread {
  id: string;
  resolved: boolean;
  outdated: boolean;
  path: string;
  line: number | null;
  start_line: number | null;
  original_line: number | null;
  side: Side;
  file_level: boolean;
  comments: ThreadComment[];
}

export interface PrThreads {
  prIndex: number;
  threads: ReviewThread[];
}

export type Plan =
  | { kind: "line"; path: string; side: Side; line: number; startLine: number | null }
  | { kind: "file"; path: string; quoted: boolean }
  | { kind: "outdated" }
  | { kind: "reply" }
  | { kind: "resolve" }
  | { kind: "summary" };

export interface Planned {
  draft: Draft;
  plan: Plan;
}

export interface Outcome {
  posted: number;
  failed: [string, string][];
  unknown: number;
}

export const anchorKey = (a: Anchor) => `${a.pr}|${a.side}|${a.path}|${a.line}`;

/** The path a stack PR knew a file under (it may have been renamed later in the stack). */
export function pathInPr(file: FileSummary, pr: number, side: Side) {
  const renamed = file.pr_paths.find(([index]) => index === pr)?.[1];
  if (renamed) return renamed;
  return side === "LEFT" ? (file.old_path ?? file.path) : file.path;
}

export function fileIndexFor(files: FileSummary[], path: string) {
  return files.findIndex((f) => f.path === path || f.old_path === path || f.pr_paths.some(([, p]) => p === path));
}

export const lineLabel = (start: number | null, end: number | null) =>
  end === null ? "" : start !== null && start !== end ? `L${start}–${end}` : `L${end}`;
