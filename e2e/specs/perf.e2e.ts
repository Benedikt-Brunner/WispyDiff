import { click, exists, openViaPalette, waitFor } from "./helpers";

/** Binding budgets from SPEC.md. */
const BUDGET = {
  cachedOpenMs: 150,
  frameP95Ms: 20, // 60 fps with a little jitter tolerance
  longFrameShare: 0.02, // frames > 33 ms
  blankFrameShare: 0.05, // frames showing unloaded rows
};

const WORST_CASE = "wispy/fixture#1"; // ~500 files, ~50k changed lines, one 20k-line file

async function measureOpen(label: string) {
  await openViaPalette(label);
  let duration: number | null | undefined;
  await waitFor(async () => {
    // `execute` serializes undefined as null.
    duration = await browser.execute(() => window.__wispyPerf!.sinceLast("open:start", "open:first-file-visible"));
    return duration != null;
  });
  return duration!;
}

/** Scrolls the diff at a constant speed inside rAF and records frame intervals. */
async function scrollFrames(pxPerFrame: number, frames: number) {
  return browser.executeAsync(
    (pxPerFrame: number, frames: number, done: (r: { intervals: number[]; blank: number }) => void) => {
      const el = document.querySelector<HTMLElement>('[data-testid="diff-scroll"]')!;
      const intervals: number[] = [];
      let blank = 0;
      let last = performance.now();
      let n = 0;
      const step = (now: number) => {
        intervals.push(now - last);
        last = now;
        if (document.querySelector(".row-loading")) blank++;
        el.scrollTop += pxPerFrame;
        if (++n < frames) requestAnimationFrame(step);
        else done({ intervals: intervals.slice(1), blank });
      };
      requestAnimationFrame(step);
    },
    pxPerFrame,
    frames,
  );
}

const percentile = (values: number[], p: number) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))];
};

function report(name: string, intervals: number[], blank: number) {
  const p95 = percentile(intervals, 0.95);
  const long = intervals.filter((i) => i > 33).length / intervals.length;
  const blankShare = blank / (intervals.length + 1);
  console.log(`[perf] ${name}: p50 ${percentile(intervals, 0.5).toFixed(1)} ms, p95 ${p95.toFixed(1)} ms, long ${(long * 100).toFixed(1)}%, blank ${(blankShare * 100).toFixed(1)}%`);
  expect(p95).toBeLessThanOrEqual(BUDGET.frameP95Ms);
  expect(long).toBeLessThanOrEqual(BUDGET.longFrameShare);
  expect(blankShare).toBeLessThanOrEqual(BUDGET.blankFrameShare);
}

describe("performance budgets (synthetic worst case)", () => {
  before(async () => {
    // Cold open: fetch + diff + highlight + cache. Not budgeted (prefetch hides it), just reported.
    const cold = await measureOpen(WORST_CASE);
    console.log(`[perf] cold open (uncached) ${WORST_CASE}: ${cold.toFixed(0)} ms`);
  });

  it(`opens a cached worst-case PR with the first file visible in < ${BUDGET.cachedOpenMs} ms`, async () => {
    const samples: number[] = [];
    for (let i = 0; i < 5; i++) {
      await openViaPalette("wispy/fixture#2"); // switch away so the next open is a real reopen
      samples.push(await measureOpen(WORST_CASE));
    }
    const median = percentile(samples, 0.5);
    console.log(`[perf] cached open: ${samples.map((s) => s.toFixed(0)).join(", ")} ms (median ${median.toFixed(0)})`);
    expect(median).toBeLessThan(BUDGET.cachedOpenMs);
  });

  it("scrolls the whole 50k-line diff at 60 fps", async () => {
    const { intervals, blank } = await scrollFrames(60, 600);
    report("scroll whole diff", intervals, blank);
  });

  it("scrolls the 20k-line file at 60 fps", async () => {
    await click('.file-list li[title="src/Allocation/PickListAllocator.php"]');
    const { intervals, blank } = await scrollFrames(60, 600);
    report("scroll 20k-line file", intervals, blank);
  });

  it("jumps across the diff with n/p without blank frames lingering", async () => {
    const start = Date.now();
    for (let i = 0; i < 40; i++) await browser.keys("n");
    await waitFor(async () => !(await exists(".row-loading")), 1000);
    console.log(`[perf] 40 file jumps settled in ${Date.now() - start} ms`);
  });

  // Budgets whose features arrive in later milestones.
  it.skip("switches PR range in < 100 ms (milestone 2)");
  it.skip("toggles unified ↔ side-by-side in < 100 ms (milestone 3)");
  it.skip("finds usages in < 50 ms (milestone 7)");
  it.skip("streams first git grep hits in < 300 ms (milestone 7)");
});
