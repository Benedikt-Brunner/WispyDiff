import { invoke } from "@tauri-apps/api/core";
import type { OpenedPr, Row } from "./types";

export const openPr = (input: string) => invoke<OpenedPr>("open_pr", { input });

export const refreshPr = (input: string, knownHeadSha: string) =>
  invoke<OpenedPr | null>("refresh_pr", { input, knownHeadSha });

export const getRows = (viewId: string, start: number, end: number) =>
  invoke<Row[]>("get_rows", { viewId, start, end });
