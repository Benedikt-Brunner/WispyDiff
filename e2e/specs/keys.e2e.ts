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

  it("zooms with ⌘+ / ⌘- / ⌘0 and ⌘+wheel, remembering the level", async () => {
    // The whole webview scales, so the window gets narrower in CSS pixels as it zooms in.
    const width = () => browser.execute(() => window.innerWidth);
    const zoomPref = () => browser.execute(() => localStorage.getItem("wispy.zoom"));
    const base = await width();
    await browser.keys([MOD, "-"]);
    await waitFor(async () => (await width()) > base && (await zoomPref()) === "0.9");
    await browser.keys([MOD, "0"]);
    await waitFor(async () => (await width()) === base && (await zoomPref()) === "1");
    // "+" needs Shift on most layouts, so it's dispatched directly.
    await browser.execute((mac: boolean) => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "+", metaKey: mac, ctrlKey: !mac, bubbles: true, cancelable: true }));
    }, MOD === "Meta");
    await waitFor(async () => (await width()) < base && (await zoomPref()) === "1.1");

    const before = await scrollTop();
    const wheel = (deltaY: number) =>
      browser.execute(
        (dy: number, mac: boolean) => {
          const el = document.querySelector('[data-testid="diff-scroll"]')!;
          el.dispatchEvent(new WheelEvent("wheel", { deltaY: dy, metaKey: mac, ctrlKey: !mac, bubbles: true, cancelable: true }));
        },
        deltaY,
        MOD === "Meta",
      );
    await wheel(-100);
    await waitFor(async () => (await zoomPref()) === "1.25");
    await wheel(100);
    await wheel(100);
    await waitFor(async () => (await zoomPref()) === "1");
    expect(await scrollTop()).toBe(before); // zooming doesn't scroll the diff
    await waitFor(async () => (await width()) === base);
  });

  it("resizes the file list by dragging its edge, remembering the width", async () => {
    const listWidth = () => browser.execute(() => Math.round(document.querySelector(".file-list")!.getBoundingClientRect().width));
    const drag = (dx: number) =>
      browser.execute((d: number) => {
        const handle = document.querySelector<HTMLElement>(".file-list .resize-handle")!;
        const x = handle.getBoundingClientRect().left + 3;
        const fire = (target: EventTarget, type: string, clientX: number) =>
          target.dispatchEvent(new PointerEvent(type, { clientX, button: 0, bubbles: true, cancelable: true }));
        fire(handle, "pointerdown", x);
        fire(window, "pointermove", x + d / 2);
        fire(window, "pointermove", x + d);
        fire(window, "pointerup", x + d);
      }, dx);
    const start = await listWidth();
    await drag(100);
    await waitFor(async () => (await listWidth()) === start + 100);

    // Hidden and shown again (⌘B), it keeps its width.
    await browser.keys([MOD, "b"]);
    await waitFor(async () => !(await exists(".file-list")));
    await browser.keys([MOD, "b"]);
    await waitFor(async () => (await listWidth()) === start + 100);

    // Double-clicking the edge restores the default.
    await browser.execute(() =>
      document.querySelector(".file-list .resize-handle")!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })),
    );
    await waitFor(async () => (await listWidth()) === 280);
  });

  it("shows the file list as a tree with t, folding directories and opening the current file's", async () => {
    await toTop();
    const dirs = () => browser.execute(() => [...document.querySelectorAll(".file-list li[data-dir]")].map((li) => li.getAttribute("data-dir")!));
    const shown = (path: string) => browser.execute((p: string) => !!document.querySelector(`.file-list li[title="${p}"]`), path);
    await browser.keys("t");
    await waitFor(async () => (await dirs()).length > 0);
    expect(await browser.execute(() => document.querySelectorAll(".file-list li[title]").length)).toBe(files.length);
    expect(await activeFile()).toBe(files[0]);

    // Folding the next file's directory hides it; moving on to it (n) opens the directory again.
    const dir = (await dirs()).filter((d) => files[1].startsWith(`${d}/`)).sort((a, b) => b.length - a.length)[0];
    await click(`.file-list li[data-dir="${dir}"]`);
    await waitFor(async () => !(await shown(files[1])));
    await browser.keys("n");
    await waitFor(async () => (await activeFile()) === files[1]);

    // Viewing every file in a directory folds it.
    const inDir = (d: string) => files.filter((f) => f.startsWith(`${d}/`));
    const small = (await dirs()).sort((a, b) => inDir(a).length - inDir(b).length)[0];
    for (const path of inDir(small)) {
      await click(`.file-list li[title="${path}"]`);
      await waitFor(async () => (await activeFile()) === path);
      await browser.keys("v");
      // (Its row goes away once the last one folds the directory.)
      await waitFor(async () => (await listHas(path, "viewed")) || !(await shown(path)));
    }
    await waitFor(async () => !(await shown(inDir(small)[0])));
    // Opened again and un-viewed, for the other specs.
    await click(`.file-list li[data-dir="${small}"]`);
    for (const path of inDir(small)) {
      await click(`.file-list li[title="${path}"]`);
      await waitFor(async () => (await activeFile()) === path);
      await browser.keys("v");
      await waitFor(async () => !(await listHas(path, "viewed")));
    }

    // The footer toggle goes back to the flat list.
    await click('[data-testid="file-list-list"]');
    await waitFor(async () => (await dirs()).length === 0);
    expect(await fileNames()).toEqual(files);
  });

  it("lists the keyboard shortcuts with ?", async () => {
    await browser.keys("?");
    await waitFor(() => exists('[data-testid="shortcuts"]'));
    expect(await text('[data-testid="shortcuts"]')).toContain("Next / previous PR");
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="shortcuts"]')));
  });
});
