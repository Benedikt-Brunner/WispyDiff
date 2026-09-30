const KEY = "wispy.recentPrs";
const MAX = 30;

export function recentPrs(): string[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export function rememberPr(label: string) {
  try {
    localStorage.setItem(KEY, JSON.stringify([label, ...recentPrs().filter((x) => x !== label)].slice(0, MAX)));
  } catch {
    // Storage unavailable: recents are a convenience only.
  }
}
