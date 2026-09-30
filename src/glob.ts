/**
 * gitignore-style globs: `*` and `?` stay within a path segment, `**` crosses segments, and a
 * pattern without `/` matches a file name anywhere.
 */
export function globMatcher(patterns: string[]): (path: string) => boolean {
  const compiled = patterns.filter((p) => p.trim()).map(compile);
  return (path) => compiled.some((re) => re.test(path));
}

function compile(pattern: string): RegExp {
  const trimmed = pattern.trim().replace(/^\//, "");
  const anywhere = !trimmed.includes("/");
  let source = "";
  for (let i = 0; i < trimmed.length; i++) {
    const c = trimmed[i];
    if (c === "*" && trimmed[i + 1] === "*") {
      const slash = trimmed[i + 2] === "/";
      source += slash ? "(?:.*/)?" : ".*";
      i += slash ? 2 : 1;
    } else if (c === "*") source += "[^/]*";
    else if (c === "?") source += "[^/]";
    else source += c.replace(/[.+^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(anywhere ? `(?:^|/)${source}$` : `^${source}(?:/.*)?$`);
}
