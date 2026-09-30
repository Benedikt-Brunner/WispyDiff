import { click, clickChip, count, exists, isFocused, openViaPalette, scrollTop, selectedChips, text, typeInPalette, waitFor } from "./helpers";

describe("opening a pull request", () => {
  it("shows the palette focused (on launch, or with ⌘K once something is open)", async () => {
    // Spec files share one app process, so this spec may start after another left a PR open.
    if (!(await exists(".palette-input"))) await browser.keys(["Meta", "k"]);
    await waitFor(() => isFocused(".palette-input"));
  });

  it("rejects input that is not a PR reference", async () => {
    await typeInPalette("not a pr");
    await waitFor(async () => (await text(".palette-error"))?.includes("not a pull request reference") ?? false);
  });

  it("opens a PR by owner/repo#N and renders its diff", async () => {
    await openViaPalette("wispy/fixture#2");
    expect(await text(".pr-title")).toBe("Add bin capacity checks");
    expect(await text(".pr-meta")).toContain("stack/1 ← stack/2");
    expect(await count(".file-list li")).toBe(21);
    expect(await count(".row-add")).toBeGreaterThan(0);
    expect(await count(".row-del")).toBeGreaterThan(0);
    // Highlighted PHP tokens made it through.
    expect(await count(".row-add .t1, .row-add .t2")).toBeGreaterThan(0);
  });

  it("navigates hunks with j/k and files with n/p", async () => {
    const start = await scrollTop();
    await browser.keys("j");
    await browser.keys("j");
    const afterHunks = await scrollTop();
    expect(afterHunks).toBeGreaterThan(start);
    await browser.keys("n");
    expect(await scrollTop()).toBeGreaterThan(afterHunks);
    await browser.keys("p");
    await browser.keys("p");
    await browser.keys("k");
    expect(await scrollTop()).toBeLessThan(afterHunks);
  });

  it("reopens the palette with Cmd+K and closes it with Escape", async () => {
    await browser.keys(["Meta", "k"]);
    await waitFor(() => isFocused(".palette-input"));
    await browser.keys("Escape");
    await waitFor(async () => !(await exists(".palette")));
  });
});

describe("stacks", () => {
  it("discovers the whole stack from a middle PR and shows only that PR", async () => {
    await openViaPalette("wispy/fixture#2");
    expect(await count(".stack-chip")).toBe(4);
    expect(await selectedChips()).toEqual([1]);
    expect(await text(".stack-base")).toContain("main");
  });

  it("selects a contiguous range with shift-click and attributes lines to PRs", async () => {
    await clickChip(0);
    await waitFor(async () => (await selectedChips()).join() === "0");
    await clickChip(2, true);
    await waitFor(async () => (await text(".pr-title")) === "#1–#3 · 3 PRs");
    expect(await selectedChips()).toEqual([0, 1, 2]);
    expect(await text(".pr-meta")).toContain("main ← stack/3");
    expect(await count(".row.attributed")).toBeGreaterThan(0);
    expect(await count(".pr-tag")).toBeGreaterThan(0);
    expect(await count(".file-pr-dot")).toBeGreaterThan(0);
    const tooltip = await browser.execute(() => document.querySelector(".row.attributed")?.getAttribute("title"));
    expect(tooltip).toMatch(/^(Added|Deleted) in #\d/);
  });

  it("moves and extends the range with [ ] { }", async () => {
    await browser.keys("]");
    await waitFor(async () => (await selectedChips()).join() === "1,2,3");
    await browser.keys("{");
    await waitFor(async () => (await selectedChips()).join() === "0,1,2,3");
    await browser.keys("[");
    // Already at the bottom: nothing changes.
    expect(await selectedChips()).toEqual([0, 1, 2, 3]);
  });

  it("opens a different member of the same stack from the cache", async () => {
    await openViaPalette("wispy/fixture#4");
    expect(await selectedChips()).toEqual([3]);
    expect(await text(".pr-title")).toBe("Capacity docs and tests");
  });
});

describe("reading modes and noise", () => {
  const headerMode = (path: string) =>
    browser.execute((p: string) => document.querySelector(`.row-file[data-file="${p}"]`)?.getAttribute("data-mode") ?? null, path);

  it("collapses lockfiles and generated files by default", async () => {
    await openViaPalette("wispy/fixture#1");
    await click('.file-list li[title="composer.lock"]');
    await waitFor(async () => (await headerMode("composer.lock")) === "collapsed");
    const muted = await browser.execute(() => [...document.querySelectorAll(".file-list li.muted")].map((li) => li.getAttribute("title")));
    expect(muted).toEqual(["composer.lock", "src/Generated/ApiClient.php"]);
    expect(await text(".row-collapsed .code")).toContain("lockfile");
  });

  it("expands a collapsed file with e", async () => {
    await browser.keys("e");
    await waitFor(async () => (await headerMode("composer.lock")) === "unified");
  });

  it("toggles the current file to full-file side by side with s", async () => {
    await click('.file-list li[title="src/Module0/Service0.php"]');
    await browser.keys("s");
    await waitFor(async () => (await headerMode("src/Module0/Service0.php")) === "split");
    await waitFor(async () => (await count(".row-split .half-add")) > 0);
    expect(await count(".row-split .half-filler, .row-split .half-ctx")).toBeGreaterThan(0);
    await browser.keys("s");
    await waitFor(async () => (await headerMode("src/Module0/Service0.php")) === "unified");
  });

  it("switches every file with S", async () => {
    await browser.keys("S");
    await waitFor(async () => (await count('.row-file[data-mode="split"]')) > 0);
    expect(await count('.row-file[data-mode="unified"]')).toBe(0);
    await browser.keys("S");
    await waitFor(async () => (await count('.row-file[data-mode="split"]')) === 0);
  });

  it("hides whitespace-only changes with w", async () => {
    await openViaPalette("wispy/fixture#2");
    expect(await count(".file-list li")).toBe(21);
    await browser.keys("w");
    await waitFor(async () => (await count(".file-list li")) === 20);
    expect(await text(".pr-meta")).toContain("whitespace hidden");
    await browser.keys("w");
    await waitFor(async () => (await count(".file-list li")) === 21);
  });

  it("ignores files matching a pattern from the palette", async () => {
    await typeInPalette("> ignore src/Module0/**");
    await waitFor(async () => !(await exists(".palette")));
    await waitFor(async () => (await headerMode("src/Module0/Service0.php")) === "collapsed");
    expect(await count(".file-list li.muted")).toBeGreaterThanOrEqual(2);
    await typeInPalette("> stop ignoring src/Module0/**");
    await waitFor(async () => (await headerMode("src/Module0/Service0.php")) === "unified");
  });
});
