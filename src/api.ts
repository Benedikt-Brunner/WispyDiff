import { Channel, invoke } from "@tauri-apps/api/core";
import type { Anchor, Draft, Location, NewDraft, Outcome, Planned, PrThreads, ShownDraft, Side, Verdict } from "./comments";
import type { Checkpoint, OpenedRange, OpenedStack, Row, SplitRow } from "./types";

export const openPr = (input: string) => invoke<OpenedStack>("open_pr", { input });

/** Resolves to the refreshed stack only if anything changed since `knownStackId`. */
export const refreshPr = (input: string, knownStackId: string) =>
  invoke<OpenedStack | null>("refresh_pr", { input, knownStackId });

export const selectRange = (stackId: string, lo: number, hi: number, ignoreWhitespace: boolean) =>
  invoke<OpenedRange>("select_range", { stackId, lo, hi, ignoreWhitespace });

/** Unified rows of one file, by offset within the file (0 = header). */
export const getRows = (viewId: string, file: number, start: number, end: number) =>
  invoke<Row[]>("get_rows", { viewId, file, start, end });

/** Side-by-side rows of one file for offsets [max(start, 1), end). */
export const getSplitRows = (viewId: string, file: number, start: number, end: number) =>
  invoke<SplitRow[]>("get_split_rows", { viewId, file, start, end });

export const getIgnorePatterns = (repo: string) => invoke<string[]>("get_ignore_patterns", { repo });

export const setIgnorePatterns = (repo: string, patterns: string[]) =>
  invoke<void>("set_ignore_patterns", { repo, patterns });

export const listDrafts = (stackId: string) => invoke<ShownDraft[]>("list_drafts", { stackId });

export const createDraft = (stackId: string, draft: NewDraft) => invoke<Draft>("create_draft", { stackId, draft });

export const updateDraft = (id: string, change: { body?: string; asFile?: boolean }) =>
  invoke<Draft | null>("update_draft", { id, body: change.body ?? null, asFile: change.asFile ?? null });

export const deleteDraft = (id: string) => invoke<void>("delete_draft", { id });

export const acceptsLineComment = (stackId: string, prIndex: number, path: string, side: Side, start: number, end: number) =>
  invoke<boolean>("accepts_line_comment", { stackId, prIndex, path, side, start, end });

export const listThreads = (stackId: string, refresh: boolean) => invoke<PrThreads[]>("list_threads", { stackId, refresh });

export const locateAnchors = (viewId: string, anchors: Anchor[]) =>
  invoke<(Location | null)[]>("locate_anchors", { viewId, anchors });

export interface SubmitPlan {
  stack: OpenedStack;
  prs: { prIndex: number; planned: Planned[] }[];
}

export const prepareSubmit = (stackId: string) => invoke<SubmitPlan>("prepare_submit", { stackId });

export const submitReview = (stackId: string, prIndex: number, verdict: Verdict, summary: string | null) =>
  invoke<Outcome>("submit_review", { stackId, prIndex, verdict, summary });

export const selectSince = (stackId: string, lo: number, hi: number, checkpointId: string) =>
  invoke<OpenedRange>("select_since", { stackId, lo, hi, checkpointId });

export const listCheckpoints = (stackId: string, lo: number, hi: number) => invoke<Checkpoint[]>("list_checkpoints", { stackId, lo, hi });

export const markReviewed = (stackId: string, lo: number, hi: number) => invoke<Checkpoint>("mark_reviewed", { stackId, lo, hi });

export const getViewed = (repo: string) => invoke<string[]>("get_viewed", { repo });

export const setViewed = (repo: string, key: string, viewed: boolean) => invoke<void>("set_viewed", { repo, key, viewed });

export interface Hit {
  file: number;
  path: string;
  line: number;
  col: number;
  text: string;
}

export interface Usages {
  name: string;
  definitions: Hit[];
  changed: Hit[];
  unchanged: Hit[];
}

export interface GrepHit {
  path: string;
  line: number;
  text: string;
}

export type GrepEvent = { kind: "hits"; hits: GrepHit[] } | { kind: "done"; total: number } | { kind: "failed"; message: string };

export const usages = (viewId: string, name: string) => invoke<Usages>("usages", { viewId, name });

/** Streams whole-repo search results to `onEvent`; resolves when the search ends. */
export function grep(stackId: string, prIndex: number, query: string, wholeWord: boolean, onEvent: (e: GrepEvent) => void) {
  const channel = new Channel<GrepEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("grep", { stackId, prIndex, query, wholeWord, onEvent: channel });
}

export const readFile = (stackId: string, prIndex: number, path: string) =>
  invoke<import("./types").Seg[][]>("read_file", { stackId, prIndex, path });

export const locateLine = (viewId: string, file: number, line: number) =>
  invoke<import("./comments").Location | null>("locate_line", { viewId, file, line });
