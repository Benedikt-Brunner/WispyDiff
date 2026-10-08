import { useMemo } from "react";
import { Marked } from "marked";
import { openUrl, type CommentOutcome } from "./api";

/** A reference to code in an assistant answer, e.g. `src/app.ts:42-50`. */
export interface CodeRef {
  path: string;
  line: number | null;
  end: number | null;
}

/** Turns references into links: `resolve` says what a reference points at (null: not a link). */
export interface CodeRefs {
  resolve: (ref: CodeRef) => CodeRef | null;
  open: (ref: CodeRef) => void;
}

/** Rendered HTML (GitHub's `bodyHTML`, or Markdown rendered here); links open in the browser,
 * file references (with `refs`) in the app. */
export function Html({ html, className = "card-body", refs }: { html: string; className?: string; refs?: CodeRefs }) {
  const safe = useMemo(() => {
    const doc = sanitize(html);
    if (refs) linkRefs(doc, refs.resolve);
    return doc.body.innerHTML;
  }, [html, refs]);
  return (
    <div
      className={`${className} markdown`}
      dangerouslySetInnerHTML={{ __html: safe }}
      onClick={(e) => {
        // The app's window never navigates away.
        const link = (e.target as HTMLElement).closest("a");
        if (!link) return;
        e.preventDefault();
        const { refPath, refLine, refEnd } = link.dataset;
        if (refPath && refs) refs.open({ path: refPath, line: refLine ? Number(refLine) : null, end: refEnd ? Number(refEnd) : null });
        else if (/^https?:/.test(link.href)) void openUrl(link.href);
      }}
    />
  );
}

/** Markdown (GitHub-flavored, as the assistant CLIs write it). `breaks`: line breaks are kept,
 * as GitHub does in comments. */
export function Markdown({ text, className, refs, breaks = false }: { text: string; className?: string; refs?: CodeRefs; breaks?: boolean }) {
  const html = useMemo(() => markdown.parse(text, { async: false, breaks }), [text, breaks]);
  return <Html html={html} className={className} refs={refs} />;
}

const markdown: Marked = new Marked({
  gfm: true,
  breaks: false,
  renderer: {
    code({ text, lang }) {
      if (!/^suggestion\b/.test(lang ?? "")) return false;
      return `<div class="suggestion"><div class="suggestion-head">Suggested change</div><pre><code>${escapeHtml(text)}\n</code></pre></div>`;
    },
  },
});

function escapeHtml(text: string) {
  return text.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

const ACTIONS = { add: ["Comment", "✓ draft added", "not added"], edit: ["Edit", "✓ updated", "not updated"], delete: ["Delete", "✓ deleted", "not deleted"] };

/** What the assistant did to the review's drafts, one card each: a new comment's place (linked
 * to the diff), or the draft it changed, what became of it, and the text it wrote. */
export function CommentCards({ comments, refs }: { comments: CommentOutcome[]; refs?: CodeRefs }) {
  const html = useMemo(
    () =>
      comments
        .map((c) => {
          const [action, done, failed] = ACTIONS[c.action ?? "add"];
          const lines = c.lines ? `:${c.lines[0]}${c.lines[1] !== c.lines[0] ? `-${c.lines[1]}` : ""}` : "";
          const where = [c.target, c.action === "add" ? null : c.label].filter(Boolean).join(" · ");
          const head = [
            `<span class="assistant-comment-action">${action}</span>`,
            c.path ? `<code>${escapeHtml(c.path + lines)}</code>` : "",
            where ? `<span class="assistant-comment-ref">${escapeHtml(where)}</span>` : "",
            `<span class="assistant-comment-status">${escapeHtml(c.error ? `${failed}: ${c.error}` : done)}</span>`,
          ].join("");
          const body = c.body ? markdown.parse(c.body, { async: false, breaks: true }) : "";
          return `<div class="assistant-comment ${c.error ? "failed" : "done"}"><div class="assistant-comment-head">${head}</div>${body}</div>`;
        })
        .join(""),
    [comments],
  );
  return <Html html={html} className="card-body assistant-comments" refs={refs} />;
}

/** Makes sure nothing in the HTML can run here. */
function sanitize(html: string) {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("script, style, iframe, object, embed, form, input, link, meta, base").forEach((el) => el.remove());
  for (const el of doc.body.querySelectorAll("*")) {
    for (const attr of [...el.attributes]) {
      const value = attr.value.trim().toLowerCase();
      if (attr.name.startsWith("on") || ((attr.name === "href" || attr.name === "src") && /^(javascript|data|vbscript):/.test(value))) el.removeAttribute(attr.name);
    }
  }
  return doc;
}

/** Links inline code and Markdown links that name a file (`path`, `path:42`, `path:42-50`,
 * `path#L42-L50`); in a list of lines (`path:42, 50-52`) each one is its own link. */
function linkRefs(doc: Document, resolve: CodeRefs["resolve"]) {
  const mark = (link: HTMLElement, ref: CodeRef) => {
    link.classList.add("code-ref");
    link.dataset.refPath = ref.path;
    if (ref.line !== null) link.dataset.refLine = String(ref.line);
    if (ref.end !== null) link.dataset.refEnd = String(ref.end);
  };
  for (const code of doc.body.querySelectorAll("code")) {
    if (code.closest("pre, a")) continue;
    const list = LINE_LIST.exec((code.textContent ?? "").trim());
    if (list) {
      const [, file, lines] = list;
      const parts = lines.split(/(,\s*)/);
      const refs = parts.map((part, i) => (i % 2 ? null : parseRef(`${file}:${part}`)));
      const resolved = refs[0] && resolve(refs[0]);
      if (!resolved) continue;
      code.textContent = "";
      parts.forEach((part, i) => {
        const ref = refs[i];
        if (!ref) return code.append(part);
        const link = doc.createElement("a");
        mark(link, { ...ref, path: resolved.path });
        link.textContent = i === 0 ? `${file}:${part}` : part;
        code.append(link);
      });
      continue;
    }
    const ref = parseRef(code.textContent ?? "");
    const resolved = ref && resolve(ref);
    if (!resolved) continue;
    const link = doc.createElement("a");
    mark(link, resolved);
    code.replaceWith(link);
    link.append(code);
  }
  for (const link of doc.body.querySelectorAll<HTMLElement>("a[href]")) {
    const href = link.getAttribute("href") ?? "";
    if (link.dataset.refPath || /^(https?|mailto):/.test(href)) continue;
    const ref = parseRef(safeDecode(href));
    const resolved = ref && resolve(ref);
    if (resolved) mark(link, resolved);
  }
}

const LINE_LIST = /^(.+?):(\d+(?:[-–]\d+)?(?:,\s*\d+(?:[-–]\d+)?)+)$/;
const REF = /^([^\s:#`'"()<>]+?)(?::(\d+)(?:[-–]L?(\d+))?(?::\d+)?|#L(\d+)(?:C\d+)?(?:-L?(\d+))?)?$/;

function parseRef(text: string): CodeRef | null {
  let path = text
    .trim()
    .replace(/^file:\/\//, "")
    // Codex sometimes names files by their absolute path in the checkout.
    .replace(/^.*\/worktrees\/[^/]+\/[^/]+\/[0-9a-f]{7,40}\//, "")
    .replace(/^\.\//, "");
  const match = REF.exec(path);
  if (!match || match[1].startsWith("/") || match[1].includes("://")) return null;
  path = match[1];
  const line = match[2] ?? match[4];
  const end = match[3] ?? match[5];
  return { path, line: line ? Number(line) : null, end: line && end && Number(end) > Number(line) ? Number(end) : null };
}

function safeDecode(text: string) {
  try {
    return decodeURI(text);
  } catch {
    return text;
  }
}
