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

/** For line wrapping: `[offset, width]` of the code rows wider than `minWidth` columns, per (file, side by side). */
export const getRowWidths = (viewId: string, files: [number, boolean][], minWidth: number) =>
  invoke<[number, number][][]>("get_row_widths", { viewId, files, minWidth });

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

export type GrepEvent = { kind: "hits"; hits: GrepHit[] } | { kind: "done"; total: number; unsearched: number } | { kind: "failed"; message: string };

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

export type Provider = "claude" | "codex";

export interface AssistantSelection {
  path: string;
  prLabel: string;
  startLine: number;
  endLine: number;
  text: string;
}

export interface ThreadAnchor {
  path: string;
  prIndex: number;
  side: import("./comments").Side;
  startLine: number;
  endLine: number;
  headStart: number | null;
  headEnd: number | null;
}

export interface AssistantMessage {
  role: "user" | "assistant";
  text: string;
  at: number;
  error: boolean;
}

export interface AssistantThread {
  id: string;
  repo: string;
  prs: number[];
  provider: Provider;
  model: string | null;
  effort: string | null;
  session: string | null;
  selection: ThreadAnchor | null;
  messages: AssistantMessage[];
  createdAt: number;
}

export interface NewThread {
  provider: Provider;
  model: string | null;
  effort: string | null;
  selection: AssistantSelection | null;
  anchor: ThreadAnchor | null;
}

export type AssistantEvent =
  | { kind: "session"; id: string }
  | { kind: "delta"; text: string }
  | { kind: "text"; text: string }
  | { kind: "error"; message: string };

export function askAssistant(
  stackId: string,
  lo: number,
  hi: number,
  threadId: string | null,
  newThread: NewThread | null,
  question: string,
  onEvent: (e: AssistantEvent) => void,
) {
  const channel = new Channel<AssistantEvent>();
  channel.onmessage = onEvent;
  return invoke<AssistantThread>("ask_assistant", { stackId, lo, hi, threadId, newThread, question, onEvent: channel });
}

export const listAssistantThreads = (stackId: string) => invoke<AssistantThread[]>("list_assistant_threads", { stackId });

export const deleteAssistantThread = (id: string) => invoke<void>("delete_assistant_thread", { id });

/** Opens a web link in the default browser. */
export const openUrl = (url: string) => invoke<void>("open_url", { url });
