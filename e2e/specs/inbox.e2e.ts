import { MOD, click, clickButton, count, exists, goHome, gutterDrag, selectedChips, setOffline, setTextarea, text, waitFor } from "./helpers";

const openFromInbox = async (pr: number) => {
  await browser.execute((n: number) => document.querySelector<HTMLElement>(`[data-testid="inbox"] li[data-pr="${n}"]`)!.click(), pr);
  await waitFor(async () => !(await exists('[data-testid="inbox"]')) && (await exists(".row-file")), 60_000);
};

describe("inbox", () => {
  it("lists review requests and my own PRs grouped into stacks", async () => {
    await goHome();
    await waitFor(async () => (await count(".inbox-group")) === 2, 60_000);
    const groups = await browser.execute(() => [...document.querySelectorAll(".inbox-group")].map((g) => g.getAttribute("data-group")));
    expect(groups.sort()).toEqual(["1,2,3,4", "5"]);
    expect(await text('li[data-pr="1"] .inbox-meta')).toContain("yours");
    expect(await text('li[data-pr="3"] .inbox-meta')).toContain("review requested");
  });

  it("prefetches every group so it opens offline", async () => {
    await waitFor(async () => (await count(".inbox-ready.ready")) === 2, 120_000);
  });

  it("shows the reviews I submitted", async () => {
    // The comments spec approved #2 at its current head.
    await waitFor(async () => (await text('li[data-pr="2"] .inbox-review')) === "you approved", 60_000);
    expect(await exists('li[data-pr="2"] .inbox-review.ok')).toBe(true);
    expect(await exists('li[data-pr="1"] .inbox-review')).toBe(false);
  });

  it("keeps every PR title visible when the row is crowded", async () => {
    // Narrowest window, zoomed in: the badges and branch names don't all fit next to the titles.
    const size = await browser.getWindowSize();
    const zoomIn = (times: number) =>
      browser.execute(
        (n: number, mac: boolean) => {
          for (let i = 0; i < n; i++) window.dispatchEvent(new KeyboardEvent("keydown", { key: "+", metaKey: mac, ctrlKey: !mac, bubbles: true, cancelable: true }));
        },
        times,
        MOD === "Meta",
      );
    await browser.setWindowSize(720, size.height);
    try {
      await zoomIn(5);
      await waitFor(async () => (await browser.execute(() => localStorage.getItem("wispy.zoom"))) === "2");
      await waitFor(async () => (await browser.execute(() => window.innerWidth)) < 400);
      const squeezed = await browser.execute(() =>
        [...document.querySelectorAll<HTMLElement>(".inbox-pr-title")].filter((t) => t.offsetWidth < Math.min(t.scrollWidth, 60)).map((t) => t.textContent),
      );
      expect(squeezed).toEqual([]);
    } finally {
      await browser.keys([MOD, "0"]);
      await browser.setWindowSize(size.width, size.height);
    }
  });

  it("collapses and expands a stack with its toggle, Space/e and Enter", async () => {
    const stack = '.inbox-group[data-group="1,2,3,4"]';
    const collapsed = () => exists(`${stack}[data-collapsed]`);
    await click(`${stack} .inbox-toggle`);
    await waitFor(collapsed);
    expect(await count(`${stack} li`)).toBe(0);
    expect(await text(`${stack} .inbox-summary`)).toMatch(/^\d\/4 reviewed$/);
    expect(await exists(`${stack} .inbox-group-head.selected`)).toBe(true);

    await browser.keys(" ");
    await waitFor(async () => !(await collapsed()));
    expect(await count(`${stack} li`)).toBe(4);
    await browser.keys("e");
    await waitFor(collapsed);
    await browser.keys("Enter");
    await waitFor(async () => !(await collapsed()));
    expect(await browser.execute(() => document.querySelector(".inbox-group li.selected")?.getAttribute("data-pr"))).toBe("1");
  });

  it("opens a PR from the inbox with its stack", async () => {
    await openFromInbox(3);
    expect(await text(".pr-title")).toBe("Show capacity in admin");
    expect(await selectedChips()).toEqual([2]);
  });

  it("moves through the inbox with j/k and opens with Enter", async () => {
    await goHome();
    await browser.keys("k");
    await browser.keys("k");
    await browser.keys("k");
    await browser.keys("k");
    await browser.keys("k");
    await browser.keys("j");
    const selected = await browser.execute(() => document.querySelector(".inbox-group li.selected")?.getAttribute("data-pr"));
    await browser.keys("Enter");
    await waitFor(async () => !(await exists('[data-testid="inbox"]')) && (await exists(".row-file")));
    expect(selected).not.toBeNull();
  });

  it("opens prefetched PRs and takes drafts while offline", async () => {
    await setOffline(true);
    try {
      await goHome();
      await openFromInbox(5);
      expect(await text(".pr-title")).toBe("Fix carrier label typo");
      await gutterDrag(".row-add", 0);
      await waitFor(() => exists(".composer-input"));
      await setTextarea(".composer-input", "Written on a plane");
      await clickButton(".composer", "Save draft");
      await waitFor(async () => (await text(".card.draft")) !== null);
      expect(await text(".card.draft")).toContain("Written on a plane");

      // Submitting needs GitHub: the sheet says so and nothing is lost.
      await clickButton(".titlebar", "Submit review");
      await waitFor(async () => (await text(".sheet-error")) !== null, 60_000);
      expect(await text(".sheet-error")).toContain("Your drafts are safe locally");
      await browser.keys("Escape");
      expect(await count(".card.draft")).toBe(1);
      await clickButton(".card.draft", "Delete");
      await waitFor(async () => (await count(".card.draft")) === 0);
    } finally {
      await setOffline(false);
    }
  });
});
