/** Performance marks read by the e2e performance suite (see e2e/specs/perf.e2e.ts). */

export const mark = (name: string) => performance.mark(`wispy:${name}`);

/**
 * Time from the latest `start` mark to the first `end` mark after it, or undefined if that
 * end hasn't happened yet (so a stale end mark from an earlier action is never paired).
 */
export function sinceLast(start: string, end: string): number | undefined {
  const starts = performance.getEntriesByName(`wispy:${start}`, "mark");
  const begin = starts[starts.length - 1];
  if (!begin) return undefined;
  const finish = performance
    .getEntriesByName(`wispy:${end}`, "mark")
    .find((m) => m.startTime >= begin.startTime);
  return finish ? finish.startTime - begin.startTime : undefined;
}

declare global {
  interface Window {
    __wispyPerf?: { sinceLast: typeof sinceLast };
    __wispyErrors?: string[];
    /** Set by App: how many ranges of a stack are precomputed, and the open stack. */
    __wispyRanges?: { readyCount: (stackId: string) => number; current: () => string | null };
  }
}

window.__wispyPerf = { sinceLast };

// Keep recent frontend errors around for the e2e suite's failure reports.
const errors: string[] = [];
window.__wispyErrors = errors;
const originalError = console.error.bind(console);
console.error = (...args: unknown[]) => {
  errors.push(args.map((a) => (a instanceof Error ? a.message : typeof a === "string" ? a : JSON.stringify(a))).join(" "));
  if (errors.length > 50) errors.shift();
  originalError(...args);
};
window.addEventListener("unhandledrejection", (e) => errors.push(`unhandled: ${String(e.reason)}`));
