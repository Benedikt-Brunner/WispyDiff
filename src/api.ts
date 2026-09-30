import { invoke } from "@tauri-apps/api/core";
import type { OpenedRange, OpenedStack, Row, SplitRow } from "./types";

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
