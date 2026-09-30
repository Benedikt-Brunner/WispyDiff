import { invoke } from "@tauri-apps/api/core";
import type { OpenedRange, OpenedStack, Row } from "./types";

export const openPr = (input: string) => invoke<OpenedStack>("open_pr", { input });

/** Resolves to the refreshed stack only if anything changed since `knownStackId`. */
export const refreshPr = (input: string, knownStackId: string) =>
  invoke<OpenedStack | null>("refresh_pr", { input, knownStackId });

export const selectRange = (stackId: string, lo: number, hi: number) =>
  invoke<OpenedRange>("select_range", { stackId, lo, hi });

export const getRows = (viewId: string, start: number, end: number) =>
  invoke<Row[]>("get_rows", { viewId, start, end });
