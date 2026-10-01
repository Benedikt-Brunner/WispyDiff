import { click, count, exists, openViaPalette, originGit, originShow, originWrite, text, waitFor } from "./helpers";

const fileNames = () => browser.execute(() => [...document.querySelectorAll(".file-list li")].map((li) => li.getAttribute("title")));
const isViewedInList = (path: string) =>
  browser.execute((p: string) => document.querySelector(`.file-list li[title="${p}"]`)?.classList.contains("viewed") ?? false, path);

/** Opens #5 and, if GitHub has newer commits, loads them. */
async function openLatest(expectNewer: boolean) {
  await openViaPalette("wispy/fixture#5");
  if (expectNewer) {
    await waitFor(() => exists(".update-banner"), 60_000);
    await click(".update-banner");
    await waitFor(async () => !(await exists(".update-banner")));
  }
}

describe("review progress", () => {
  let path = "";

  it("marks a file viewed with v, which collapses and ticks it", async () => {
    await openLatest(false);
    [path] = (await fileNames()) as string[];
    await browser.keys("v");
    await waitFor(() => isViewedInList(path));
    expect(await text(".row-collapsed .code")).toContain("viewed");
  });

  it("records a local checkpoint with M", async () => {
    await browser.keys("M");
    await waitFor(async () => (await text(".toast"))?.includes("Marked #5 as reviewed") ?? false);
  });

  it("keeps the file viewed across a rebase that doesn't change it", async () => {
    // main moves on; the author rebases #5 onto it without touching their change.
    originGit("checkout", "--quiet", "main");
    originWrite("docs/CHANGELOG.md", "# Changelog\n\n- unrelated work on main\n");
    originGit("add", "-A");
    originGit("commit", "--quiet", "-m", "main moves on");
    originGit("checkout", "--quiet", "solo");
    originGit("rebase", "--quiet", "main");
    originGit("update-ref", "refs/pull/5/head", "solo");
    originGit("checkout", "--quiet", "main");

    await openLatest(true);
    expect(await isViewedInList(path)).toBe(true);
  });

  it("shows only the author's new changes since the checkpoint with d (not the rebase)", async () => {
    originGit("checkout", "--quiet", "solo");
    const current = originShow(`solo:${path}`);
    const lines = current.split("\n");
    const target = lines.findIndex((l) => l.includes("result = "));
    lines[target] += " // reviewed fix";
    originWrite(path, lines.join("\n"));
    originGit("commit", "--quiet", "-am", "address review");
    originGit("update-ref", "refs/pull/5/head", "solo");
    originGit("checkout", "--quiet", "main");

    await openLatest(true);
    expect(await isViewedInList(path)).toBe(false);
    await browser.keys("d");
    await waitFor(async () => (await text(".pr-meta"))?.includes("changes since") ?? false, 60_000);
    expect(await fileNames()).toEqual([path]);
    expect((await text(".file-list li .file-counts"))?.replace(/\s+/g, " ").trim()).toBe("+1 −1");
    await browser.keys("d");
    await waitFor(async () => !((await text(".pr-meta"))?.includes("changes since") ?? true));
    expect(await count(".file-list li")).toBe(1);
  });

  it("offers new commits for the PR on screen as soon as the background refresh sees them", async () => {
    expect(await exists(".update-banner")).toBe(false);
    originGit("checkout", "--quiet", "solo");
    originWrite("docs/NOTES.md", "pushed while the PR was open\n");
    originGit("add", "-A");
    originGit("commit", "--quiet", "-m", "more work");
    originGit("update-ref", "refs/pull/5/head", "solo");
    originGit("checkout", "--quiet", "main");

    // What the 5-minute timer or focusing the window does.
    await browser.execute(() => (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke("refresh_inbox"));
    await waitFor(() => exists(".update-banner"), 60_000);
    await click(".update-banner");
    await waitFor(async () => ((await fileNames()) as string[]).includes("docs/NOTES.md"), 60_000);
  });
});
