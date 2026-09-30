import { readFileSync } from "node:fs";
import { clickButton, count, exists, openViaPalette, setTextarea, text, waitFor } from "./helpers";

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

  it("asks Codex about the whole range, and shows failures in the thread", async () => {
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="assistant-panel"]')));
    await browser.execute(() => window.getSelection()?.removeAllRanges());
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

    await ask("fail");
    await waitFor(async () => (await text(".assistant-message.error"))?.includes("model not available (fake)") ?? false, 60_000);
    expect(await count('[data-testid="assistant-threads"] option')).toBe(3);
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="assistant-panel"]')));
  });
});
