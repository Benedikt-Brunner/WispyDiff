import { useState } from "react";
import { prColor, type PullRequest, type Range } from "./types";

interface Props {
  prs: PullRequest[];
  needsRebase: boolean[];
  range: Range;
  ready: (lo: number, hi: number) => boolean;
  onSelect: (range: Range) => void;
}

/** The stack, bottom to top. Click selects one PR; shift-click extends to a contiguous range. */
export function StackBar({ prs, needsRebase, range, ready, onSelect }: Props) {
  const [anchor, setAnchor] = useState(range.lo);

  const choose = (index: number, extend: boolean) => {
    if (extend) {
      onSelect({ lo: Math.min(anchor, index), hi: Math.max(anchor, index) });
    } else {
      setAnchor(index);
      onSelect({ lo: index, hi: index });
    }
  };

  return (
    <nav className="stack-bar" aria-label="Stack">
      <span className="stack-base">{prs[0].base_ref}</span>
      {prs.map((pr, i) => {
        const selected = i >= range.lo && i <= range.hi;
        const classes = ["stack-chip", selected && "selected", selected && i === range.lo && "first", selected && i === range.hi && "last"];
        return (
          <button
            key={pr.number}
            className={classes.filter(Boolean).join(" ")}
            style={{ "--pr": prColor(i) } as React.CSSProperties}
            data-index={i}
            title={`${pr.title}\n${pr.base_ref} ← ${pr.head_ref}${needsRebase[i] ? "\n⚠ Not based on the latest head of the PR below" : ""}\nShift-click to select a range`}
            onClick={(e) => choose(i, e.shiftKey)}
          >
            <span className="stack-dot" />
            <span className="stack-number">#{pr.number}</span>
            <span className="stack-title">{pr.title}</span>
            {needsRebase[i] && <span className="stack-warn">⚠</span>}
            {!ready(i, i) && <span className="stack-pending" aria-label="preparing" />}
          </button>
        );
      })}
    </nav>
  );
}

/** `[`/`]` move the selection down/up the stack; `{`/`}` extend it. */
export function rangeForKey(key: string, range: Range, size: number): Range | undefined {
  const { lo, hi } = range;
  switch (key) {
    case "[":
      return lo > 0 ? { lo: lo - 1, hi: hi - 1 } : undefined;
    case "]":
      return hi < size - 1 ? { lo: lo + 1, hi: hi + 1 } : undefined;
    case "{":
      return lo > 0 ? { lo: lo - 1, hi } : undefined;
    case "}":
      return hi < size - 1 ? { lo, hi: hi + 1 } : undefined;
    default:
      return undefined;
  }
}
