import { click, clickButton, clickChip, count, exists, gutterDrag, MOD, openViaPalette, pressShifted, selectedChips, setTextarea, text, waitFor } from "./helpers";

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

  it("renders the thread's comments as GitHub formats them", async () => {
    expect(await text(".card.thread .card-body.markdown strong")).toBe("threshold");
    expect(await text(".card.thread .card-body")).not.toContain("**");
  });

  it("hides a thread to a gutter icon and shows it again", async () => {
    await clickButton(".card.thread", "hide");
    await waitFor(async () => !(await exists(".card.thread")) && (await exists(".comment-mark")));
    await click(".comment-mark");
    await waitFor(async () => (await exists(".card.thread")) && !(await exists(".comment-mark")));
  });

  it("hides and shows the PR's threads with h, remembered per PR", async () => {
    await browser.keys("h");
    await waitFor(async () => !(await exists(".card.thread")) && (await exists(".comment-mark")));
    expect(await browser.execute(() => localStorage.getItem("wispy.hiddenCommentPrs"))).toBe('["wispy/fixture#2"]');
    await browser.keys("h");
    await waitFor(async () => (await exists(".card.thread")) && !(await exists(".comment-mark")));
  });

  it("sums up a collapsed file's threads on its notice and counts open ones in the file list", async () => {
    const file = '.file-list li[title="src/Module0/Service0.php"]';
    const notice = '.row-collapsed[data-file="src/Module0/Service0.php"]';
    expect(await text(`${file} .file-open-threads`)).toBe("1");
    await click(file);
    await browser.keys("e");
    await waitFor(async () => (await text(notice))?.includes("1 open") ?? false);
    expect(await exists(".card.thread")).toBe(false);
    expect(await text(`${file} .file-open-threads`)).toBe("1");
    await click(notice);
    await waitFor(() => exists(".card.thread"));
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

  it("offers the commented lines as a suggested change", async () => {
    // Read the line before dragging: opening the composer scrolls it into view, which can shift
    // which off-screen rows are rendered and so what `.row-add` index 1 is (it did on macOS CI).
    const line = await browser.execute(() => document.querySelectorAll(".row-add")[1].querySelector(".code")!.textContent);
    await gutterDrag(".row-add", 1);
    await waitFor(() => exists(".composer-input"));
    await setTextarea(".composer-input", "Simpler:");
    expect(await clickButton(".composer", "Suggest change")).toBe(true);
    const value = await browser.execute(() => document.querySelector<HTMLTextAreaElement>(".composer-input")!.value);
    expect(value).toBe(`Simpler:\n\`\`\`suggestion\n${line}\n\`\`\``);
    await browser.keys("Escape");
    await waitFor(async () => !(await exists(".composer-input")));
  });

  it("keeps the comment being written when it leaves the rendered rows", async () => {
    const value = () => browser.execute(() => document.querySelector<HTMLTextAreaElement>(".composer-input")?.value ?? null);
    const scrollTo = (top: number) => browser.execute((t: number) => void (document.querySelector(".diff-scroll")!.scrollTop = t), top);
    const toggleWrap = async (to: "on" | "off") => {
      await browser.execute(() => (document.activeElement as HTMLElement | null)?.blur());
      await browser.keys("z");
      await waitFor(() => browser.execute((v: string) => localStorage.getItem("wispy.wrap") === v, to));
    };
    // A wide file list leaves room for few columns, so the fixture's lines wrap and toggling the list reflows them.
    await browser.execute(() => {
      const handle = document.querySelector<HTMLElement>(".file-list .resize-handle")!;
      const x = handle.getBoundingClientRect().left + 3;
      const fire = (target: EventTarget, type: string, clientX: number) =>
        target.dispatchEvent(new PointerEvent(type, { clientX, button: 0, bubbles: true, cancelable: true }));
      fire(handle, "pointerdown", x);
      fire(window, "pointermove", x + 5000);
      fire(window, "pointerup", x + 5000);
    });
    await waitFor(() => browser.execute(() => document.querySelector(".file-list")!.getBoundingClientRect().width > innerWidth / 2));
    await toggleWrap("on");
    try {
      await browser.execute(() => {
        const view = document.querySelector(".diff-scroll")!;
        view.scrollTop = view.scrollHeight / 2;
      });
      await waitFor(() =>
        browser.execute(() => {
          const view = document.querySelector(".diff-scroll")!.getBoundingClientRect();
          return [...document.querySelectorAll(".row-add")].some((r) => r.getBoundingClientRect().top > view.top + view.height / 2);
        }),
      );
      const index = await browser.execute(() => {
        const view = document.querySelector(".diff-scroll")!.getBoundingClientRect();
        return [...document.querySelectorAll(".row-add")].findIndex((r) => r.getBoundingClientRect().top > view.top + view.height / 2);
      });
      await gutterDrag(".row-add", index);
      await waitFor(() => exists(".composer-input"));
      await setTextarea(".composer-input", "Half-written thought");
      for (let i = 0; i < 2; i++) {
        await browser.execute(() => document.querySelector<HTMLTextAreaElement>(".composer-input")!.focus());
        await browser.keys([MOD, "b"]);
        await waitFor(async () => (await exists(".file-list")) === (i === 1));
        expect(await value()).toBe("Half-written thought");
      }
      const top = await browser.execute(() => document.querySelector(".diff-scroll")!.scrollTop);
      await scrollTo(0);
      await waitFor(async () => (await browser.execute(() => document.querySelector(".diff-scroll")!.scrollTop)) === 0);
      await scrollTo(top);
      await waitFor(() => exists(".composer-input"));
      expect(await value()).toBe("Half-written thought");
    } finally {
      if (await exists(".composer-input")) {
        await clickButton(".composer", "Cancel");
        await waitFor(async () => !(await exists(".composer-input")));
      }
      if (!(await exists(".file-list"))) await browser.keys([MOD, "b"]);
      await browser.execute(() =>
        document.querySelector(".file-list .resize-handle")!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })),
      );
      await waitFor(() => browser.execute(() => document.querySelector(".file-list")!.getBoundingClientRect().width === 280));
      await toggleWrap("off");
      await scrollTo(0);
    }
  });

  it("drafts a range comment by dragging across lines, saving it with Enter", async () => {
    await gutterDrag(".row-ctx", 0, 2);
    await waitFor(() => exists(".composer-input"));
    expect(await text(".composer .card-title")).toMatch(/L\d+–\d+$/);
    await setTextarea(".composer-input", "These three lines");
    await pressShifted("Enter");
    expect(await exists(".composer-input")).toBe(true);
    await browser.keys("Enter");
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
    // Everything went out: the sheet closes by itself.
    await waitFor(async () => !(await exists(".sheet")), 60_000);
    expect(await text(".toast")).toBe("Review submitted");
    await waitFor(async () => (await draftCount()) === 0);
  });

  it("shows the posted comments as threads and the resolved thread as a gutter icon", async () => {
    await waitFor(() => exists(".comment-mark.settled"));
    expect(await browser.execute(() => document.querySelector(".comment-mark.settled")!.getAttribute("title"))).toMatch(/^Resolved · teammate: /);
    await click(".comment-mark.settled");
    await waitFor(async () => (await text(".card.thread .badge.ok")) === "resolved");
    await clickButton(".card.thread:has(.badge.ok)", "hide");
    await waitFor(async () => !(await exists(".card.thread .badge.ok")) && (await exists(".comment-mark.settled")));
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
