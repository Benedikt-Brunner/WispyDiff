import { click, exists, openViaPalette, pressShifted, scrollTop, selectedChips, settled, text, waitFor, MOD } from "./helpers";

const activeFile = () => browser.execute(() => document.querySelector(".file-list li.active")?.getAttribute("title") ?? null);
const fileNames = () => browser.execute(() => [...document.querySelectorAll(".file-list li")].map((li) => li.getAttribute("title")!));
const listHas = (path: string, cls: string) =>
  browser.execute((p: string, c: string) => document.querySelector(`.file-list li[title="${p}"]`)?.classList.contains(c) ?? false, path, cls);
const toTop = async () => {
  await browser.execute(() => {
    document.querySelector<HTMLElement>('[data-testid="diff-scroll"]')!.scrollTop = 0;
  });
  await waitFor(async () => (await scrollTop()) === 0);
};
const theme = () => browser.execute(() => document.documentElement.dataset.theme ?? null);

describe("keyboard", () => {
  let files: string[] = [];

  before(async () => {
    await openViaPalette("wispy/fixture#3");
    files = await fileNames();
  });

  it("scrolls with the arrow keys and moves between files with shift+arrows", async () => {
    await toTop();
    await browser.keys("ArrowDown");
    await browser.keys("ArrowDown");
    await waitFor(async () => (await scrollTop()) > 0 && (await settled()));
    const scrolled = await scrollTop();
    expect(scrolled).toBe(120); // two steps of three rows
    await browser.keys("ArrowUp");
    await waitFor(async () => (await scrollTop()) === 60 && (await settled()));

    await toTop();
    expect(await activeFile()).toBe(files[0]);
    await pressShifted("ArrowDown");
    await waitFor(async () => (await activeFile()) === files[1]);
    await pressShifted("ArrowUp");
    await waitFor(async () => (await activeFile()) === files[0]);
  });

  it("collapses a file with space without marking it viewed, moving on to the next file", async () => {
    await toTop();
    await browser.keys(" ");
    await waitFor(async () => (await listHas(files[0], "muted")) && (await activeFile()) === files[1]);
    expect(await listHas(files[0], "viewed")).toBe(false);
    expect(await text(".row-collapsed .code")).toContain("collapsed");

    // Expanding stays on the file.
    await click(`.file-list li[title="${files[0]}"]`);
    await waitFor(async () => (await activeFile()) === files[0]);
    await browser.keys(" ");
    await waitFor(async () => !(await listHas(files[0], "muted")));
    expect(await activeFile()).toBe(files[0]);
  });

  it("moves on to the next file when marking one viewed, so v v marks two in a row", async () => {
    await toTop();
    await browser.keys("v");
    await waitFor(async () => (await listHas(files[0], "viewed")) && (await activeFile()) === files[1]);
    await browser.keys("v");
    await waitFor(async () => (await listHas(files[1], "viewed")) && (await activeFile()) === files[2]);

    // Un-viewing stays put (and cleans up for other specs).
    for (const path of [files[1], files[0]]) {
      await click(`.file-list li[title="${path}"]`);
      await waitFor(async () => (await activeFile()) === path);
      await browser.keys("v");
      await waitFor(async () => !(await listHas(path, "viewed")));
      expect(await activeFile()).toBe(path);
    }
  });

  it("marks the last files viewed even though they can't scroll to the top", async () => {
    const [secondLast, last] = files.slice(-2);
    await click(`.file-list li[title="${secondLast}"]`);
    await waitFor(async () => (await activeFile()) === secondLast);
    await browser.keys("v");
    await waitFor(async () => (await listHas(secondLast, "viewed")) && (await activeFile()) === last);
    await browser.keys("v");
    await waitFor(() => listHas(last, "viewed"));
    expect(await activeFile()).toBe(last);
    // v again un-views that same file, not the collapsed one above it.
    await browser.keys("v");
    await waitFor(async () => !(await listHas(last, "viewed")));
    expect(await listHas(secondLast, "viewed")).toBe(true);

    await click(`.file-list li[title="${secondLast}"]`);
    await waitFor(async () => (await activeFile()) === secondLast);
    await browser.keys("v");
    await waitFor(async () => !(await listHas(secondLast, "viewed")));
  });

  it("closes the assistant with Escape even from its pickers", async () => {
    await browser.keys("a");
    await waitFor(() => exists('[data-testid="assistant-panel"]'));
    await browser.execute(() => document.querySelector<HTMLSelectElement>('[data-testid="assistant-panel"] .assistant-options select')!.focus());
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="assistant-panel"]')));
  });

  it("cycles through the stack's PRs with Tab and Shift+Tab", async () => {
    expect(await selectedChips()).toEqual([2]);
    await browser.keys("Tab");
    await waitFor(async () => JSON.stringify(await selectedChips()) === "[3]");
    await browser.keys("Tab");
    await waitFor(async () => JSON.stringify(await selectedChips()) === "[0]"); // wraps around
    await pressShifted("Tab");
    await waitFor(async () => JSON.stringify(await selectedChips()) === "[3]");
    await pressShifted("Tab");
    await waitFor(async () => JSON.stringify(await selectedChips()) === "[2]");
  });

  it("previews themes with ⌘T, reverts on Escape and keeps one on Enter", async () => {
    const before = await theme();
    await browser.keys([MOD, "t"]);
    await waitFor(() => exists('[data-testid="theme-picker"]'));
    await browser.keys("ArrowDown"); // Light
    if (before === "light") await browser.keys("ArrowDown"); // Dark: the system is light already
    await waitFor(async () => (await theme()) !== before);
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="theme-picker"]')));
    // WebKitGTK's prefers-color-scheme follows the window theme the preview set, so "system"
    // settles once the window is back to following the desktop.
    await waitFor(async () => (await theme()) === before, 5_000);

    await browser.keys([MOD, "t"]);
    await waitFor(() => exists('[data-testid="theme-picker"]'));
    await click('[data-theme-id="nord"]');
    await waitFor(async () => (await theme()) === "nord");
    expect(await browser.execute(() => localStorage.getItem("wispy.theme"))).toBe("nord");

    // Back to following the system for the other specs.
    await browser.keys([MOD, "t"]);
    await waitFor(() => exists('[data-testid="theme-picker"]'));
    await click('[data-theme-id="system"]');
    await waitFor(async () => (await theme()) === "light" || (await theme()) === "dark");
  });

  it("lists the keyboard shortcuts with ?", async () => {
    await browser.keys("?");
    await waitFor(() => exists('[data-testid="shortcuts"]'));
    expect(await text('[data-testid="shortcuts"]')).toContain("Next / previous PR");
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="shortcuts"]')));
  });
});
