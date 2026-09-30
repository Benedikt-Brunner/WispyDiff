import { click, cmdClickWord, count, exists, openViaPalette, submitInput, text, waitFor } from "./helpers";

const sectionCount = (id: string) => browser.execute((s: string) => document.querySelectorAll(`[data-testid="${s}"] .panel-hit`).length, id);

describe("code intelligence", () => {
  it("shows usages of a symbol with ⌘-click, split into definitions, changed and unchanged code", async () => {
    await openViaPalette("wispy/fixture#1");
    // A method defined in the first file: find its name from a visible "public function" line.
    const name = await browser.execute(() => {
      const row = [...document.querySelectorAll(".row .code")].find((c) => /public function \w+\(/.test(c.textContent ?? ""));
      return row?.textContent?.match(/public function (\w+)\(/)?.[1] ?? null;
    });
    expect(name).not.toBeNull();
    expect(await cmdClickWord(".row .code span", name!)).toBe(true);
    await waitFor(() => exists('[data-testid="code-panel"]'));
    await waitFor(async () => (await text(".panel-title"))?.includes(name!) ?? false);
    expect(await sectionCount("usages-definitions")).toBeGreaterThan(0);

    expect(await cmdClickWord(".row-add .code span", "compute")).toBe(true);
    await waitFor(async () => (await text(".panel-title"))?.includes("compute") ?? false);
    expect(await sectionCount("usages-changed")).toBeGreaterThan(0);
    expect(await sectionCount("usages-unchanged")).toBeGreaterThan(0);
  });

  it("jumps to a use in unchanged code (switching to side by side when it's outside the hunks)", async () => {
    // The last unchanged hit: likely far from any hunk.
    const snippet = await browser.execute(() => {
      const hits = document.querySelectorAll<HTMLElement>('[data-testid="usages-unchanged"] .panel-hit');
      const hit = hits[hits.length - 1];
      hit.click();
      return hit.querySelector(".panel-hit-text")!.textContent!;
    });
    await waitFor(async () => (await count(".row.selected")) > 0);
    const flashed = await browser.execute(() => {
      const row = document.querySelector(".row.selected")!;
      return { text: row.textContent ?? "", split: row.classList.contains("row-split") };
    });
    expect(flashed.text).toContain(snippet.trim());
    if (flashed.split) expect(await count('.row-file[data-mode="split"]')).toBeGreaterThan(0);
  });

  it("searches the whole repo and opens files outside the diff read-only", async () => {
    await submitInput(".panel-input", "endblock");
    await waitFor(async () => (await count('[data-testid="grep-results"] .panel-hit')) > 0, 60_000);
    // A template the PR didn't touch.
    const outside = await browser.execute(() => {
      const inView = new Set([...document.querySelectorAll(".file-list li")].map((li) => li.getAttribute("title")));
      const hits = [...document.querySelectorAll<HTMLElement>('[data-testid="grep-results"] .panel-hit')];
      const hit = hits.find((h) => !inView.has(h.querySelector(".panel-hit-where")!.textContent!.replace(/:\d+$/, "")));
      hit?.click();
      return hit?.querySelector(".panel-hit-where")?.textContent ?? null;
    });
    expect(outside).not.toBeNull();
    await waitFor(() => exists('[data-testid="file-view"]'));
    await waitFor(async () => (await count('[data-testid="file-view"] .row')) > 0);
    expect(await text('[data-testid="file-view"] .sheet-title')).toBe(outside!.replace(/:\d+$/, ""));
    expect(await count('[data-testid="file-view"] .flash-static')).toBe(1);
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="file-view"]')));
    await click('[data-testid="code-panel"] .link');
    await waitFor(async () => !(await exists('[data-testid="code-panel"]')));
  });
});
