import { Fragment, useEffect, useRef, useState } from "react";
import { applyTheme, saveTheme, savedTheme, THEMES } from "./themes";

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
            else if (!(e.metaKey && e.key === "t")) return;
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

const SHORTCUTS: [string, [string[], string][]][] = [
  [
    "Anywhere",
    [
      [["⌘K"], "Open a PR (URL or owner/repo#123) · type > for commands"],
      [["⌘I"], "Inbox"],
      [["⌘T"], "Theme"],
      [["?", "⌘/"], "This list"],
    ],
  ],
  [
    "Inbox",
    [
      [["j", "k"], "Move down / up"],
      [["↵"], "Open"],
      [["r"], "Refresh"],
    ],
  ],
  [
    "Moving around",
    [
      [["↓", "↑"], "Scroll"],
      [["⇧↓", "⇧↑"], "Next / previous file"],
      [["n", "p"], "Next / previous file"],
      [["j", "k"], "Next / previous change (hold to scroll through them)"],
      [["⌘B"], "Show / hide the file list"],
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
      [["Space", "e"], "Collapse / expand the file"],
      [["v"], "Mark viewed and go to the next file"],
    ],
  ],
  [
    "Review",
    [
      [["c"], "Comment on the line under the mouse (or the file)"],
      [["⌘↵"], "Save the comment"],
      [["⌘G"], "Suggest a change to the commented lines"],
      [["⌘-click", "u"], "Usages of the name under the mouse"],
      [["/"], "Search the repo"],
      [["a"], "Ask the assistant (about the selected lines, if any)"],
      [["Esc"], "Close the assistant / panel / comment box"],
    ],
  ],
];

/** `?` / ⌘/: every keyboard shortcut. */
export function ShortcutsHelp({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === "?" || (e.metaKey && e.key === "/")) {
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
