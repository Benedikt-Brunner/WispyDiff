import { click, clickButton, count, exists, goHome, gutterDrag, openViaPalette, originGit, originShow, originWrite, setTextarea, text, waitFor } from "./helpers";

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

  it("marking a file viewed in the changes since the checkpoint marks it in the full diff too", async () => {
    await browser.keys("d");
    await waitFor(async () => (await text(".pr-meta"))?.includes("changes since") ?? false, 60_000);
    await browser.keys("v");
    await waitFor(() => isViewedInList(path));
    await browser.keys("d");
    await waitFor(async () => !((await text(".pr-meta"))?.includes("changes since") ?? true));
    expect(await isViewedInList(path)).toBe(true);
    // Unmark again for the specs below.
    await browser.keys("v");
    await waitFor(async () => !(await isViewedInList(path)));
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

  it("shows in the inbox that #5 changed since it was marked reviewed, until it's marked again", async () => {
    const marked = 'li[data-pr="5"] .inbox-marked';
    await goHome();
    await waitFor(async () => (await text(marked)) === "marked reviewed · changed since", 60_000);
    await openViaPalette("wispy/fixture#5");
    await browser.keys("M");
    await waitFor(async () => (await text(".toast"))?.includes("Marked #5 as reviewed") ?? false);
    await goHome();
    await waitFor(async () => (await text(marked)) === "marked reviewed");
  });

  it("switches to the changes since the checkpoint with a comment on a file that isn't in them", async () => {
    await openViaPalette("wispy/fixture#5");
    // #5 is now docs/NOTES.md and `path`; comment on the second file.
    expect(await fileNames()).toEqual(["docs/NOTES.md", path]);
    await gutterDrag('.row-add[data-file="1"]', 0);
    await waitFor(() => exists(".composer-input"));
    await setTextarea(".composer-input", "Still needed?");
    await clickButton(".composer", "Save draft");
    await waitFor(async () => (await text(".card.draft")) !== null);

    originGit("checkout", "--quiet", "solo");
    originWrite("docs/NOTES.md", "pushed while the PR was open, then reworded\n");
    originGit("commit", "--quiet", "-am", "reword notes");
    originGit("update-ref", "refs/pull/5/head", "solo");
    originGit("checkout", "--quiet", "main");
    await openLatest(true);

    await browser.keys("d");
    await waitFor(async () => (await text(".pr-meta"))?.includes("changes since") ?? false, 60_000);
    expect(await fileNames()).toEqual(["docs/NOTES.md"]);
    await browser.keys("d");
    await waitFor(async () => !((await text(".pr-meta"))?.includes("changes since") ?? true));
    await waitFor(async () => (await text(".card.draft"))?.includes("Still needed?") ?? false);
  });
});
