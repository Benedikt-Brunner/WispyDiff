/**
 * The embedded macOS WebDriver is fast for `execute`/`keys` but very slow (or hangs) for element
 * lookups (`$`, `$$`, `waitFor*`). All helpers therefore query the DOM through `execute`.
 */

export const count = (selector: string) =>
  browser.execute((s: string) => document.querySelectorAll(s).length, selector);

export const exists = async (selector: string) => (await count(selector)) > 0;

export const text = (selector: string) =>
  browser.execute((s: string) => document.querySelector<HTMLElement>(s)?.innerText ?? null, selector);

export const isFocused = (selector: string) =>
  browser.execute((s: string) => document.activeElement === document.querySelector(s), selector);

export const click = (selector: string) =>
  browser.execute((s: string) => document.querySelector<HTMLElement>(s)!.click(), selector);

export const scrollTop = () =>
  browser.execute(() => document.querySelector<HTMLElement>('[data-testid="diff-scroll"]')!.scrollTop);

export const waitFor = (condition: () => Promise<boolean>, timeout = 30_000) =>
  browser.waitUntil(condition, { timeout, interval: 25 });

/** Types into the palette like a user (opening it with ⌘K if needed) and presses Enter. */
export async function typeInPalette(value: string) {
  if (!(await exists(".palette-input"))) await browser.keys(["Meta", "k"]);
  await waitFor(() =>
    browser.execute(() => {
      const input = document.querySelector<HTMLInputElement>(".palette-input");
      if (!input || input.disabled) return false;
      input.focus();
      input.select();
      return true;
    }),
  );
  await browser.keys("Backspace");
  await browser.keys(value.split(""));
  await browser.keys("Enter");
}

/** Opens a PR through the command palette and waits for its diff. */
export async function openViaPalette(label: string) {
  await typeInPalette(label);
  await waitFor(async () => !(await exists(".palette")) && (await exists(".row-file")), 60_000);
}

/** Clicks a stack chip; `extend` is a shift-click (extends the selected range). */
export const clickChip = (index: number, extend = false) =>
  browser.execute(
    (i: number, shiftKey: boolean) => {
      const chip = document.querySelector<HTMLElement>(`.stack-chip[data-index="${i}"]`)!;
      chip.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey }));
    },
    index,
    extend,
  );

/** Selected chip indices, e.g. [1, 2]. */
export const selectedChips = () =>
  browser.execute(() =>
    [...document.querySelectorAll<HTMLElement>(".stack-chip.selected")].map((c) => Number(c.dataset.index)),
  );

/** Waits until the backend has precomputed `count` ranges of the open stack. */
export const waitForRanges = (count: number, timeout = 60_000) =>
  waitFor(
    () =>
      browser.execute((n: number) => {
        const ranges = window.__wispyRanges!;
        const id = ranges.current();
        return id !== null && ranges.readyCount(id) >= n;
      }, count),
    timeout,
  );
