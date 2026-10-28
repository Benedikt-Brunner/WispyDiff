import { Fragment, useEffect, useRef, useState } from "react";
import { applyTheme, saveTheme, savedTheme, THEMES } from "./themes";
import { mod, keyLabel, clickLabel } from "./platform";

/** ⌘T: pick a theme. Moving through the list previews it; Enter keeps it, Escape goes back. */
export function ThemePicker({ onClose }: { onClose: () => void }) {
  const initial = useRef(savedTheme());
  const [selected, setSelected] = useState(() => THEMES.findIndex((t) => t.id === initial.current));
  const list = useRef<HTMLUListElement>(null);

  useEffect(() => applyTheme(THEMES[selected].id), [selected]);
  useEffect(() => list.current?.focus(), []);

  const cancel = () => {
    applyTheme(initial.current);
    onClose();
  };
  const choose = (index: number) => {
    saveTheme(THEMES[index].id);
    onClose();
  };

  return (
    <div className="palette-backdrop" onMouseDown={cancel}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="Theme">
        <div className="overlay-title">Theme</div>
        <ul
          ref={list}
          className="palette-list theme-list"
          tabIndex={0}
          data-testid="theme-picker"
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) setSelected((s) => (s + 1) % THEMES.length);
            else if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) setSelected((s) => (s - 1 + THEMES.length) % THEMES.length);
            else if (e.key === "Enter") choose(selected);
            else if (e.key === "Escape") cancel();
            else if (!(mod(e) && e.key === "t")) return;
            e.preventDefault();
            e.stopPropagation();
          }}
        >
          {THEMES.map((theme, i) => (
            <li
              key={theme.id}
              className={i === selected ? "selected" : undefined}
              data-theme-id={theme.id}
              onMouseEnter={() => setSelected(i)}
              onClick={() => choose(i)}
            >
              {theme.name}
              {theme.id === initial.current && <span className="theme-current">current</span>}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

/**
 * Marking a file viewed while newer commits are waiting and it changed in them: load the
 * newer version (Enter; the file stays unviewed) or mark it anyway (Esc; no more warnings for that version).
 */
export function NewerChangesWarning({ path, onLoad, onDismiss }: { path: string; onLoad: () => void; onDismiss: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => box.current?.focus(), []);
  return (
    <div className="palette-backdrop" onMouseDown={onDismiss}>
      <div
        ref={box}
        className="palette newer-warning"
        role="alertdialog"
        aria-label="Newer changes"
        tabIndex={-1}
        data-testid="newer-warning"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Enter") onLoad();
          else if (e.key === "Escape") onDismiss();
          e.preventDefault();
          e.stopPropagation();
        }}
      >
        <div className="overlay-title">Newer changes</div>
        <p className="newer-warning-text">
          <code>{path.split("/").pop()}</code> changed in commits pushed since this version was loaded.
        </p>
        <div className="newer-warning-actions">
          <button className="button" onClick={onDismiss}>
            Mark viewed anyway <kbd>Esc</kbd>
          </button>
          <button className="button primary" onClick={onLoad}>
            Load latest <kbd>↵</kbd>
          </button>
        </div>
      </div>
    </div>
  );
}

const SHORTCUTS: [string, [string[], string][]][] = [
  [
    "Anywhere",
    [
      [[keyLabel("K")], "Open a PR (URL or owner/repo#123) · type > for commands"],
      [[keyLabel("I")], "Inbox"],
      [[keyLabel("T")], "Theme"],
      [["?", keyLabel("/")], "This list"],
      [[keyLabel("+"), keyLabel("-"), keyLabel("0")], `Zoom in / out / reset (or ${keyLabel("scroll")})`],
    ],
  ],
  [
    "Inbox",
    [
      [["j", "k"], "Move down / up"],
      [["↵"], "Open (a collapsed stack: expand)"],
      [["Space", "e"], "Collapse / expand the stack"],
      [["r"], "Refresh"],
    ],
  ],
  [
    "Moving around",
    [
      [["↓", "↑"], "Scroll"],
      [["⇧↓", "⇧↑"], "Next / previous file"],
      [["n", "p"], "Next / previous file"],
      [["⇧⇧"], "Find a file by name"],
      [["j", "k"], "Next / previous change (hold to scroll through them)"],
      [[keyLabel("B")], "Show / hide the file list"],
      [["t"], "File list as a directory tree / flat list"],
      [["f"], "Filter files by extension (↑/↓, Space to toggle)"],
      [["x"], "Hide the current file's extension"],
      [["F"], "Switch the file filter off / on"],
    ],
  ],
  [
    "Stack",
    [
      [["⇥", "⇧⇥"], "Next / previous PR"],
      [["]", "["], "Move the selection up / down the stack"],
      [["}", "{"], "Extend the selection up / down"],
      [["w"], "Show / hide whitespace changes"],
      [["M"], "Mark as reviewed (local checkpoint)"],
      [["d"], "Changes since the last checkpoint"],
    ],
  ],
  [
    "Files",
    [
      [["s"], "Side by side for this file"],
      [["S"], "Side by side for all files"],
      [["z"], "Wrap long lines"],
      [["Space", "e"], "Collapse / expand the file"],
      [["v"], "Mark viewed and go to the next file"],
      [["V"], "Undo the last viewed mark and go back to that file"],
    ],
  ],
  [
    "Review",
    [
      [["c"], "Comment on the line under the mouse (or the file)"],
      [["h"], "Hide / show the selected PRs' comment threads"],
      [["↵"], "Save the comment (⇧↵: new line)"],
      [[keyLabel("G")], "Suggest a change to the commented lines"],
      [["Tab", "⇧Tab"], "Indent / outdent the selected lines of a suggestion"],
      [["↵"], `New line in a suggestion, indented like the last (${keyLabel("↵")} saves)`],
      [[clickLabel, "u"], "Usages of the name under the mouse"],
      [["/"], "Search the repo"],
      [["a"], "Ask the assistant (about the selected lines, if any)"],
      [["r"], "Submit review…"],
      [[keyLabel("1"), keyLabel("2"), keyLabel("3")], "In the submit sheet: comment / approve / request changes (your own PRs: comment only)"],
      [["⇥", "⇧⇥"], "In the submit sheet: next / previous PR's summary"],
      [["↵", keyLabel("↵")], "In the submit sheet: submit this PR / submit all"],
      [["Esc"], "Close the assistant / panel / comment box"],
    ],
  ],
];

/** `?` / ⌘/: every keyboard shortcut. */
export function ShortcutsHelp({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === "?" || (mod(e) && e.key === "/")) {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="sheet shortcuts" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="Keyboard shortcuts" data-testid="shortcuts">
        <div className="sheet-head">
          <span className="sheet-title">Keyboard shortcuts</span>
          <button className="link" onClick={onClose}>
            close
          </button>
        </div>
        <div className="shortcuts-body">
          {SHORTCUTS.map(([group, entries]) => (
            <section key={group} className="shortcuts-group">
              <h3>{group}</h3>
              <dl>
                {entries.map(([keys, what]) => (
                  <Fragment key={what + keys.join()}>
                    <dt>
                      {keys.map((k) => (
                        <kbd key={k}>{k}</kbd>
                      ))}
                    </dt>
                    <dd>{what}</dd>
                  </Fragment>
                ))}
              </dl>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
