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

/**
 * Enters `value` in the palette (opening it with ⌘K if needed) and presses Enter. The value is
 * set through React's input path: the embedded driver drops spaces from typed text.
 */
export async function typeInPalette(value: string) {
  if (!(await exists(".palette-input"))) await browser.keys(["Meta", "k"]);
  await waitFor(() =>
    browser.execute((v: string) => {
      const input = document.querySelector<HTMLInputElement>(".palette-input");
      if (!input || input.disabled) return false;
      input.focus();
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, v);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    }, value),
  );
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

/** Sets a textarea's value through React's input path. */
export const setTextarea = (selector: string, value: string) =>
  browser.execute(
    (s: string, v: string) => {
      const el = document.querySelector<HTMLTextAreaElement>(s)!;
      el.focus();
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(el, v);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    },
    selector,
    value,
  );

/** Presses the mouse on the gutter of the `index`-th element matching `rowSelector` (React sees
 * mouseover as enter), optionally dragging to another row before releasing. */
export const gutterDrag = (rowSelector: string, fromIndex: number, toIndex = fromIndex, gutter = ".gutter") =>
  browser.execute(
    (s: string, from: number, to: number, g: string) => {
      const rows = [...document.querySelectorAll<HTMLElement>(s)];
      const at = (i: number) => rows[i].querySelector<HTMLElement>(g)!;
      const fire = (el: HTMLElement, type: string) =>
        el.dispatchEvent(new MouseEvent(type, { bubbles: true, relatedTarget: document.body }));
      fire(at(from), "mouseover");
      fire(at(from), "mousedown");
      if (to !== from) fire(at(to), "mouseover");
      fire(at(to), "mouseup");
    },
    rowSelector,
    fromIndex,
    toIndex,
    gutter,
  );

/** Clicks the first button inside `scope` whose text contains `label`. */
export const clickButton = (scope: string, label: string) =>
  browser.execute(
    (s: string, l: string) => {
      const button = [...document.querySelectorAll<HTMLButtonElement>(`${s} button`)].find((b) => b.textContent?.includes(l));
      button?.click();
      return button !== undefined;
    },
    scope,
    label,
  );
