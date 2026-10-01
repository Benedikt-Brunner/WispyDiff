import { click, clickButton, clickChip, count, exists, gutterDrag, openViaPalette, selectedChips, setTextarea, text, waitFor } from "./helpers";

const draftCount = async () => {
  const label = (await text('[data-testid="review-button"]')) ?? "";
  return Number(label.match(/· (\d+)/)?.[1] ?? 0);
};

describe("comments", () => {
  it("shows a teammate's open thread inline", async () => {
    await openViaPalette("wispy/fixture#2");
    await waitFor(async () => (await text(".card.thread")) !== null);
    expect(await text(".card.thread")).toContain("Is this threshold right?");
    expect(await text(".card.thread")).toContain("teammate");
  });

  it("drafts a comment on a changed line from the gutter", async () => {
    await gutterDrag(".row-add", 0);
    await waitFor(() => exists(".composer-input"));
    expect(await text(".composer .card-title")).toMatch(/Comment on #2 · .+ L\d+$/);
    await setTextarea(".composer-input", "Why this threshold?");
    await clickButton(".composer", "Save draft");
    await waitFor(async () => (await text(".card.draft")) !== null);
    expect(await text(".card.draft")).toContain("Why this threshold?");
    expect(await draftCount()).toBe(1);
  });

  it("drafts a range comment by dragging across lines", async () => {
    await gutterDrag(".row-ctx", 0, 2);
    await waitFor(() => exists(".composer-input"));
    expect(await text(".composer .card-title")).toMatch(/L\d+–\d+$/);
    await setTextarea(".composer-input", "These three lines");
    await clickButton(".composer", "Save draft");
    await waitFor(async () => (await draftCount()) === 2);
  });

  it("warns when a line is outside the PR's diff and will go out as a file comment", async () => {
    await click('.file-list li[title="src/Module0/Service0.php"]');
    await browser.keys("s");
    await waitFor(async () => (await count(".row-split")) > 20);
    // A context line a screen away from the file's first hunk.
    await gutterDrag(".row-split", 12, 12, ".half:not(.half-left) .gutter");
    await waitFor(() => exists(".composer-input"));
    await waitFor(async () => (await text(".composer .badge.warn")) === "will post as file comment");
    await setTextarea(".composer-input", "Unrelated but worth changing");
    await clickButton(".composer", "Save draft");
    await waitFor(async () => (await draftCount()) === 3);
    await browser.keys("s");
  });

  it("queues a reply and a resolve on the teammate's thread", async () => {
    await clickButton(".card.thread", "Reply");
    await waitFor(() => exists(".card.thread .composer-input"));
    await setTextarea(".card.thread .composer-input", "Yes — 304 is the bin limit.");
    await clickButton(".card.thread", "Save reply");
    await waitFor(async () => (await text(".card.thread .comment.pending")) !== null);
    await clickButton(".card.thread", "Resolve");
    await waitFor(async () => (await text(".card.thread"))?.includes("Resolves on submit") ?? false);
    expect(await draftCount()).toBe(5);
  });

  it("keeps drafts across reopening the stack", async () => {
    await openViaPalette("wispy/fixture#3");
    await openViaPalette("wispy/fixture#2");
    await waitFor(async () => (await draftCount()) === 5);
  });

  it("routes a comment in a multi-PR range to the PR that introduced the line", async () => {
    await clickChip(0);
    await clickChip(1, true);
    await waitFor(async () => (await text(".pr-title")) === "#1–#2 · 2 PRs");
    const fromPr1 = await browser.execute(() =>
      [...document.querySelectorAll(".row-add")].findIndex((r) => r.getAttribute("title") === "Added in #1"),
    );
    expect(fromPr1).toBeGreaterThanOrEqual(0);
    await gutterDrag(".row-add", fromPr1);
    await waitFor(() => exists(".composer-input"));
    expect(await text(".composer .card-title")).toContain("Comment on #1");
    await browser.keys("Escape");
    await waitFor(async () => !(await exists(".composer-input")));
    await clickChip(1);
    await waitFor(async () => (await text(".pr-title")) === "Add bin capacity checks");
  });

  it("submits one review with a verdict and posts everything", async () => {
    await click('[data-testid="review-button"]');
    await waitFor(async () => (await count('.sheet-pr[data-pr="2"] .sheet-items li')) === 5);
    const kinds = await browser.execute(() => [...document.querySelectorAll('.sheet-pr[data-pr="2"] .sheet-item-kind')].map((k) => k.textContent));
    expect(kinds).toContain("reply");
    expect(kinds).toContain("resolve");
    expect(kinds.some((k) => k?.includes("file comment (outside the diff)"))).toBe(true);

    await browser.execute(() => {
      const select = document.querySelector<HTMLSelectElement>('.sheet-pr[data-pr="2"] select')!;
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(select, "APPROVE");
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await click('[data-testid="submit-all"]');
    await waitFor(async () => (await text('.sheet-pr[data-pr="2"] .sheet-done')) === "Posted ✓", 60_000);
    await browser.keys("Escape");
    await waitFor(async () => (await draftCount()) === 0);
  });

  it("shows the posted comments as threads and the resolved thread collapsed", async () => {
    await waitFor(async () => (await text(".card.thread.collapsed"))?.includes("Resolved") ?? false);
    const bodies = await browser.execute(() => [...document.querySelectorAll(".card.thread .card-body")].map((b) => b.textContent));
    expect(bodies).toContain("Why this threshold?");
    expect(bodies).toContain("These three lines");
    expect(await count(".card.draft")).toBe(0);
  });

  it("submits a single PR of the stack and keeps the other PRs' drafts", async () => {
    const draftOn = async (chip: number, body: string) => {
      await clickChip(chip);
      await waitFor(async () => JSON.stringify(await selectedChips()) === `[${chip}]`);
      await gutterDrag(".row-add", 0);
      await waitFor(() => exists(".composer-input"));
      await setTextarea(".composer-input", body);
      await clickButton(".composer", "Save draft");
      await waitFor(async () => (await text(".card.draft"))?.includes(body) ?? false);
    };
    await draftOn(2, "Only this one goes out");
    await draftOn(1, "This one stays a draft");
    await waitFor(async () => (await draftCount()) === 2);

    await click('[data-testid="review-button"]');
    await waitFor(async () => (await count('.sheet-pr[data-pr="3"] .sheet-items li')) === 1);
    expect(await count('.sheet-pr[data-pr="2"] .sheet-items li')).toBe(1);
    await click('[data-testid="submit-pr-3"]');
    await waitFor(async () => (await text('.sheet-pr[data-pr="3"] .sheet-done')) === "Posted ✓", 60_000);
    await waitFor(async () => (await count('.sheet-pr[data-pr="3"] .sheet-items li')) === 0);
    expect(await count('.sheet-pr[data-pr="2"] .sheet-items li')).toBe(1);
    expect(await text('.sheet-pr[data-pr="2"] .sheet-done')).toBeNull();
    await browser.keys("Escape");
    await waitFor(async () => (await draftCount()) === 1);

    await clickButton(".card.draft", "Delete");
    await waitFor(async () => (await draftCount()) === 0);
  });
});
