import { click, clickChip, cmdClickWord, count, exists, goHome, openViaPalette, submitInput, waitFor, waitForRanges } from "./helpers";

/** Binding budgets from SPEC.md. */
const BUDGET = {
  cachedOpenMs: 150,
  rangeSwitchMs: 100,
  toggleMs: 100,
  usagesMs: 50,
  grepFirstHitMs: 300,
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
    duration = await browser.execute(() => window.__wispyPerf!.sinceLast("open:start", "view:first-visible"));
    return duration != null;
  });
  return duration!;
}

/** Waits for the first `end` mark after the latest `start` mark and returns the time between them. */
async function waitForMark(start: string, end: string) {
  let duration: number | null | undefined;
  await waitFor(async () => {
    duration = await browser.execute((s: string, e: string) => window.__wispyPerf!.sinceLast(s, e), start, end);
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

/**
 * Holds `key` like the OS key repeat does (a keydown every 33 ms), sampling every frame: a frame
 * is blank when part of the viewport shows no rows or rows still loading. `maxStep` is the largest
 * move between two frames, in screens (a held key should scroll, not teleport).
 */
async function holdKey(key: string, presses: number) {
  return browser.executeAsync(
    (key: string, presses: number, done: (r: { intervals: number[]; blank: number; moved: number; maxStep: number }) => void) => {
      const el = document.querySelector<HTMLElement>('[data-testid="diff-scroll"]')!;
      const startTop = el.scrollTop;
      const intervals: number[] = [];
      let blank = 0;
      let last = performance.now();
      let lastTop = el.scrollTop;
      let maxStep = 0;
      let sent = 0;
      const timer = setInterval(() => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key, repeat: sent > 0, bubbles: true }));
        if (++sent >= presses) clearInterval(timer);
      }, 33);
      const isBlank = () => {
        const box = el.getBoundingClientRect();
        for (let i = 1; i < 8; i++) {
          const y = box.top + (box.height * i) / 8;
          const hit = document.elementFromPoint(box.left + box.width / 2, y)?.closest(".row, .insert");
          if (!hit || hit.classList.contains("row-loading")) return true;
        }
        return false;
      };
      const step = (now: number) => {
        intervals.push(now - last);
        last = now;
        if (isBlank()) blank++;
        maxStep = Math.max(maxStep, Math.abs(el.scrollTop - lastTop) / el.clientHeight);
        lastTop = el.scrollTop;
        if (sent < presses || intervals.length < 30) requestAnimationFrame(step);
        else done({ intervals: intervals.slice(1), blank, moved: el.scrollTop - startTop, maxStep });
      };
      requestAnimationFrame(step);
    },
    key,
    presses,
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

  it(`opens a prefetched stack from the inbox with the first file visible in < ${BUDGET.cachedOpenMs} ms`, async () => {
    const samples: number[] = [];
    for (let i = 0; i < 5; i++) {
      await goHome();
      await waitFor(async () => (await count(".inbox-ready.ready")) === 2, 120_000);
      await browser.execute(() => document.querySelector<HTMLElement>('[data-testid="inbox"] li[data-pr="1"]')!.click());
      let duration: number | null | undefined;
      await waitFor(async () => {
        duration = await browser.execute(() => window.__wispyPerf!.sinceLast("open:start", "view:first-visible"));
        return duration != null;
      });
      samples.push(duration!);
    }
    const median = percentile(samples, 0.5);
    console.log(`[perf] open from inbox: ${samples.map((s) => s.toFixed(0)).join(", ")} ms (median ${median.toFixed(0)})`);
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

  it("holds j/k to move through hunks without blank frames", async () => {
    await openViaPalette(WORST_CASE);
    await browser.execute(() => {
      document.querySelector<HTMLElement>('[data-testid="diff-scroll"]')!.scrollTop = 0;
    });
    await waitFor(async () => !(await exists(".row-loading")), 2000);
    const down = await holdKey("j", 60);
    report("hold j", down.intervals, down.blank);
    const up = await holdKey("k", 60);
    report("hold k", up.intervals, up.blank);
    console.log(`[perf] hold j/k: moved ${down.moved} / ${up.moved} px, largest step ${down.maxStep.toFixed(2)} / ${up.maxStep.toFixed(2)} screens`);
    expect(down.moved).toBeGreaterThan(0);
    expect(up.moved).toBeLessThan(0);
    // Never a jump: a tap moves at most a third of a screen per frame, a held key a tenth.
    expect(Math.max(down.maxStep, up.maxStep)).toBeLessThanOrEqual(0.34);
  });

  it(`switches the PR range in < ${BUDGET.rangeSwitchMs} ms once the stack is precomputed`, async () => {
    await openViaPalette(WORST_CASE);
    await waitForRanges(10); // 4 PRs → 10 contiguous ranges
    // Plain click = single PR, shift-click = extend from it. Every step changes the range.
    const clicks: [number, boolean][] = [[3, false], [0, true], [1, false], [2, true], [0, false], [1, true], [2, false], [3, true], [0, false]];
    const samples: number[] = [];
    for (const [index, extend] of clicks) {
      await clickChip(index, extend);
      let duration: number | null | undefined;
      await waitFor(async () => {
        duration = await browser.execute(() => window.__wispyPerf!.sinceLast("range:start", "view:first-visible"));
        return duration != null;
      });
      samples.push(duration!);
    }
    const median = percentile(samples, 0.5);
    console.log(`[perf] range switch: ${samples.map((s) => s.toFixed(0)).join(", ")} ms (median ${median.toFixed(0)})`);
    expect(median).toBeLessThan(BUDGET.rangeSwitchMs);
  });

  it(`toggles the 20k-line file to side by side in < ${BUDGET.toggleMs} ms`, async () => {
    await openViaPalette(WORST_CASE);
    const samples: number[] = [];
    for (let i = 0; i < 4; i++) {
      await click('.file-list li[title="src/Allocation/PickListAllocator.php"]');
      await browser.keys("s");
      samples.push(await waitForMark("toggle:start", "layout:first-visible"));
    }
    const median = percentile(samples, 0.5);
    console.log(`[perf] toggle 20k-line file: ${samples.map((s) => s.toFixed(0)).join(", ")} ms (median ${median.toFixed(0)})`);
    expect(median).toBeLessThan(BUDGET.toggleMs);
  });

  it("scrolls the 20k-line file side by side at 60 fps", async () => {
    await click('.file-list li[title="src/Allocation/PickListAllocator.php"]');
    await browser.keys("s");
    await waitForMark("toggle:start", "layout:first-visible");
    const { intervals, blank } = await scrollFrames(60, 600);
    report("scroll 20k-line file side by side", intervals, blank);
  });

  it(`switches every file of the worst case between modes in < ${BUDGET.toggleMs} ms`, async () => {
    const samples: number[] = [];
    for (let i = 0; i < 4; i++) {
      await browser.keys("S");
      samples.push(await waitForMark("toggle:start", "layout:first-visible"));
    }
    const median = percentile(samples, 0.5);
    console.log(`[perf] toggle all files: ${samples.map((s) => s.toFixed(0)).join(", ")} ms (median ${median.toFixed(0)})`);
    expect(median).toBeLessThan(BUDGET.toggleMs);
  });

  it(`finds usages of a symbol in < ${BUDGET.usagesMs} ms`, async () => {
    await openViaPalette(WORST_CASE);
    const samples: number[] = [];
    for (const word of ["compute", "result", "value", "max", "compute"]) {
      await cmdClickWord(".row .code span", word);
      samples.push(await waitForMark("usages:start", "usages:visible"));
    }
    const median = percentile(samples, 0.5);
    console.log(`[perf] usages: ${samples.map((s) => s.toFixed(0)).join(", ")} ms (median ${median.toFixed(0)})`);
    expect(median).toBeLessThan(BUDGET.usagesMs);
  });

  it(`streams the first git grep hits in < ${BUDGET.grepFirstHitMs} ms`, async () => {
    // The first search of a head downloads its blobs; the budget is for searching.
    await submitInput(".panel-input", "endblock");
    await waitFor(async () => (await count('[data-testid="grep-results"] .panel-hit')) > 0, 60_000);
    const samples: number[] = [];
    for (const query of ["computed", "threshold", "helper", "endblock", "strlen"]) {
      await submitInput(".panel-input", query);
      samples.push(await waitForMark("grep:start", "grep:first-hit"));
    }
    const median = percentile(samples, 0.5);
    console.log(`[perf] grep first hit: ${samples.map((s) => s.toFixed(0)).join(", ")} ms (median ${median.toFixed(0)})`);
    expect(median).toBeLessThan(BUDGET.grepFirstHitMs);
  });
});
