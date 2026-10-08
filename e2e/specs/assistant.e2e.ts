import { readFileSync } from "node:fs";
import { click, clickButton, count, exists, openViaPalette, setTextarea, text, waitFor } from "./helpers";

/** Selects the code of the `from`-th to `to`-th unified change rows, like dragging the mouse. */
const selectRows = (from: number, to: number) =>
  browser.execute(
    (a: number, b: number) => {
      const rows = [...document.querySelectorAll<HTMLElement>(".row-add, .row-del")].sort(
        (x, y) => x.getBoundingClientRect().top - y.getBoundingClientRect().top,
      );
      const range = document.createRange();
      range.setStart(rows[a].querySelector(".code")!, 0);
      range.setEnd(rows[b].querySelector(".code")!, 1);
      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);
      return rows[b].querySelector(".ln:last-child")?.textContent ?? "";
    },
    from,
    to,
  );

const ask = async (question: string) => {
  await setTextarea('[data-testid="assistant-input"]', question);
  await browser.execute(() => document.querySelector<HTMLElement>('[data-testid="assistant-send"]')!.click());
};

/** Picks the provider (the choice is remembered across runs, so tests set it explicitly). */
const chooseProvider = (provider: "claude" | "codex") =>
  browser.execute((p: string) => {
    const select = document.querySelector<HTMLSelectElement>('[data-testid="assistant-panel"] .assistant-options select')!;
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(select, p);
    select.dispatchEvent(new Event("change", { bubbles: true }));
  }, provider);

const lastAnswer = () =>
  browser.execute(() => {
    const answers = document.querySelectorAll(".assistant-message.assistant:not(.streaming) .card-body");
    return answers[answers.length - 1]?.textContent ?? null;
  });

const fakeCalls = () =>
  readFileSync(process.env.WISPY_FAKE_LOG!, "utf8")
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l) as { provider: string; args: string[]; cwd: string; prompt: string });

describe("assistant", () => {
  it("asks about selected lines with the selection as context", async () => {
    await openViaPalette("wispy/fixture#2");
    await selectRows(0, 1);
    await browser.keys("a");
    await waitFor(() => exists('[data-testid="assistant-panel"]'));
    expect(await text('[data-testid="assistant-context"]')).toMatch(/Service0\.php L\d+–\d+ · #2/);
    await chooseProvider("claude");
    await ask("Why does this threshold change?");
    await waitFor(async () => (await lastAnswer())?.includes("About “Why does this threshold change?”") ?? false, 60_000);
    expect(await lastAnswer()).toContain("in the checkout");
    expect(await text(".assistant-message.assistant:not(.streaming) .markdown strong")).toBe("checkout");

    const call = fakeCalls().at(-1)!;
    expect(call.provider).toBe("claude");
    expect(call.args.join(" ")).toContain("--allowed-tools Read,Grep,Glob --permission-mode dontAsk");
    expect(call.prompt).toContain("The question is about these lines of src/Module0/Service0.php (in #2");
    expect(call.cwd).toContain("/worktrees/wispy/fixture/");
  });

  it("follows up in the same session", async () => {
    await ask("And what breaks if it's lower?");
    await waitFor(async () => (await lastAnswer())?.startsWith("(follow-up)") ?? false, 60_000);
    const call = fakeCalls().at(-1)!;
    expect(call.args).toContain("--resume");
    expect(call.prompt).toBe("And what breaks if it's lower?");
  });

  it("marks the asked-about lines in the gutter", async () => {
    await waitFor(async () => (await count(".assistant-mark")) > 0);
  });

  it("turns an answer into a draft comment on those lines", async () => {
    await clickButton('[data-testid="assistant-panel"]', "turn into draft comment");
    await waitFor(async () => (await text('[data-testid="assistant-note"]'))?.includes("Saved as a draft comment") ?? false);
    await waitFor(async () => (await text(".card.draft"))?.includes("About “") ?? false);
    await clickButton(".card.draft", "Delete");
    await waitFor(async () => (await count(".card.draft")) === 0);
  });

  it("offers to sign in again when the CLI's sign-in expired, then asks again", async () => {
    await clickButton('[data-testid="assistant-panel"]', "delete");
    await ask("expired");
    await waitFor(async () => (await text(".assistant-message.error"))?.includes("OAuth session expired") ?? false, 60_000);
    await clickButton('[data-testid="assistant-sign-in"]', "Sign in to Claude Code again");
    await waitFor(async () => (await text('[data-testid="assistant-sign-in"]'))?.includes("Signed in.") ?? false, 30_000);
    const calls = readFileSync(process.env.WISPY_FAKE_LOG!, "utf8").trim().split("\n").map((l) => JSON.parse(l));
    expect(calls.some((c) => c.args?.join(" ") === "auth login" && c.browser === "true")).toBe(true);
    expect(calls.at(-1).opened).toBe("https://claude.example.test/oauth/authorize?state=fake");

    // Asking again sends the question with the review context (the failed turn left no session).
    await clickButton('[data-testid="assistant-sign-in"]', "Ask again");
    await waitFor(async () => fakeCalls().at(-1)!.prompt?.includes("Question: expired") ?? false, 60_000);
    expect(fakeCalls().at(-1)!.prompt).toContain("Under review: wispy/fixture #2");
    expect(fakeCalls().at(-1)!.args).not.toContain("--resume");
    await waitFor(async () => (await count(".assistant-message.error")) === 2, 60_000);
  });

  it("asks Codex about the whole range, and shows failures in the thread", async () => {
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="assistant-panel"]')));
    await browser.execute(() => window.getSelection()?.removeAllRanges());
    // Files hidden by the file filter are named in the prompt, without their changes.
    const files = await browser.execute(() => [...document.querySelectorAll(".file-list li")].map((li) => li.getAttribute("title")!));
    const ext = files.find((f) => !f.endsWith(".php"))!.replace(/^.*(?=\.)/, "");
    const hidden = files.filter((f) => f.endsWith(ext));
    await click('[data-testid="file-filter"]');
    await click(`[data-testid="file-filter-ext${ext}"]`);
    await browser.keys("Escape");
    await waitFor(async () => (await text('[data-testid="file-filter"]'))?.trim() === `${hidden.length} hidden`);
    await browser.keys("a");
    await waitFor(() => exists('[data-testid="assistant-panel"]'));
    expect(await text('[data-testid="assistant-context"]')).toBe("#2");
    await chooseProvider("codex");
    await ask("Summarize this PR");
    await waitFor(async () => (await lastAnswer())?.includes("About “Summarize this PR”") ?? false, 60_000);
    const call = fakeCalls().at(-1)!;
    expect(call.provider).toBe("codex");
    expect(call.args).toContain('sandbox_mode="read-only"');
    expect(call.prompt).toContain("Under review: wispy/fixture #2");
    for (const path of hidden) {
      expect(call.prompt).toContain(`\n- ${path}\n`);
      expect(call.prompt).not.toContain(`diff --git a/${path}`);
    }
    expect(call.prompt).toContain("diff --git a/src/Module0/Service0.php");
    await click('[data-testid="file-filter"]');
    await click('[data-testid="file-filter-reset"]');
    await browser.keys("Escape");

    await ask("fail");
    await waitFor(async () => (await text(".assistant-message.error"))?.includes("model not available (fake)") ?? false, 60_000);
    expect(await count('[data-testid="assistant-threads"] option')).toBe(3);
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="assistant-panel"]')));
  });

  it("links file references in answers to the diff, or the file view outside it", async () => {
    const files = await browser.execute(() => [...document.querySelectorAll(".file-list li")].map((li) => li.getAttribute("title")!));
    const ext = files.find((f) => !f.endsWith(".php"))!.replace(/^.*(?=\.)/, "");
    await click('[data-testid="file-filter"]');
    await click(`[data-testid="file-filter-ext${ext}"]`);
    await browser.keys("Escape");
    await browser.keys("a");
    await waitFor(() => exists('[data-testid="assistant-panel"]'));
    // Codex also names files by their absolute path in the checkout.
    await chooseProvider("codex");
    await ask("refs");
    const refs = () =>
      browser.execute(() =>
        [...document.querySelectorAll<HTMLElement>('[data-testid="assistant-panel"] .assistant-message:not(.streaming) a.code-ref')].map((a) => ({ ...a.dataset })),
      );
    // Files outside the diff become links once the app has checked that they exist.
    await waitFor(async () => (await refs()).length >= 8, 60_000);
    const [range, bare, absolute, outside, hidden, ...listed] = await refs();
    const unchanged = listed.pop()!;
    expect(unchanged.refPath).toBe(range.refPath);
    // A list of lines (`name:12, 13-14`) links each of them.
    const line = Number(range.refLine);
    expect(listed).toEqual([
      { refPath: range.refPath, refLine: String(line) },
      { refPath: range.refPath, refLine: String(line + 1), refEnd: String(line + 2) },
    ]);
    expect(range.refPath).toMatch(/^src\//);
    expect(Number(range.refEnd)).toBe(Number(range.refLine) + 1);
    expect(bare).toEqual({ refPath: range.refPath });
    expect(absolute).toEqual({ refPath: range.refPath, refLine: range.refLine });
    expect(files).not.toContain(outside.refPath);
    expect(hidden.refPath!.endsWith(ext)).toBe(true);
    const plain = await browser.execute(() =>
      [...document.querySelectorAll('[data-testid="assistant-panel"] .markdown code')].filter((c) => !c.closest("a") && !c.querySelector("a")).map((c) => c.textContent),
    );
    expect(plain).toEqual(["this.state", `${range.refPath!.replace(/\/[^/]+$/, "")}/*.php`, "order/germany-gross-order-import/order-create"]);

    const open = (i: number) => browser.execute((n: number) => document.querySelectorAll<HTMLElement>('[data-testid="assistant-panel"] a.code-ref')[n].click(), i);
    // A line range flashes in the diff (a row's head line number is its last).
    await open(0);
    await waitFor(() =>
      browser.execute(
        (line: string) => {
          const rows = [...document.querySelectorAll(".row.selected")];
          return rows.length >= 2 && rows.some((r) => [...r.querySelectorAll(".ln")].at(-1)?.textContent === line);
        },
        range.refLine!,
      ),
    );
    expect(await exists('[data-testid="assistant-panel"]')).toBe(true);
    // A line outside the changes: its file switches to side by side and the view goes there, also
    // with wrapped lines, whose heights in that mode arrive right after the jump.
    const wrap = async () => {
      await browser.execute(() => (document.activeElement as HTMLElement | null)?.blur());
      await browser.keys("z");
    };
    await wrap();
    await waitFor(() => browser.execute(() => localStorage.getItem("wispy.wrap") === "on"));
    await browser.execute(() => {
      const view = document.querySelector(".diff-scroll")!;
      view.scrollTop = view.scrollHeight;
    });
    await open(7);
    await waitFor(() =>
      browser.execute((line: string) => {
        const view = document.querySelector(".diff-scroll")!.getBoundingClientRect();
        return [...document.querySelectorAll(".row.selected")].some((r) => {
          const box = r.getBoundingClientRect();
          return box.top >= view.top && box.bottom <= view.bottom && [...r.querySelectorAll(".ln")].some((ln) => ln.textContent === line);
        });
      }, unchanged.refLine!),
    );
    // Back to unified (spec files share the app), and no wrapping.
    const header = `.row-file[data-file="${range.refPath}"]`;
    expect(await browser.execute((h: string) => document.querySelector(h)?.getAttribute("data-mode"), header)).toBe("split");
    await browser.keys("s");
    await waitFor(() => browser.execute((h: string) => document.querySelector(h)?.getAttribute("data-mode") === "unified", header));
    await wrap();
    await waitFor(() => browser.execute(() => localStorage.getItem("wispy.wrap") === "off"));
    // Files outside the diff, and files the filter hides, open in the file view.
    for (const [i, ref] of [[3, outside], [4, hidden]] as const) {
      await open(i);
      await waitFor(async () => (await text('[data-testid="file-view"] .sheet-title')) === ref.refPath);
      await waitFor(() => exists('[data-testid="file-view"] .flash-static'));
      await browser.keys("Escape");
      await waitFor(async () => !(await exists('[data-testid="file-view"]')));
    }

    await click('[data-testid="file-filter"]');
    await click('[data-testid="file-filter-reset"]');
    await browser.keys("Escape");
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="assistant-panel"]')));
  });
});
