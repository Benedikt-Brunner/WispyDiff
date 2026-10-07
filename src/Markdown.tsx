import { useMemo } from "react";
import { marked } from "marked";
import { openUrl } from "./api";

/** Rendered HTML (GitHub's `bodyHTML`, or Markdown rendered here); links open in the browser. */
export function Html({ html, className = "card-body" }: { html: string; className?: string }) {
  const safe = useMemo(() => sanitize(html), [html]);
  return (
    <div
      className={`${className} markdown`}
      dangerouslySetInnerHTML={{ __html: safe }}
      onClick={(e) => {
        // The app's window never navigates away.
        const link = (e.target as HTMLElement).closest("a");
        if (!link) return;
        e.preventDefault();
        if (/^https?:/.test(link.href)) void openUrl(link.href);
      }}
    />
  );
}

/** Markdown (GitHub-flavored, as the assistant CLIs write it). */
export function Markdown({ text, className }: { text: string; className?: string }) {
  const html = useMemo(() => marked.parse(text, { gfm: true, breaks: false, async: false }), [text]);
  return <Html html={html} className={className} />;
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
  return doc.body.innerHTML;
}
