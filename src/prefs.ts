/** Per-viewer conveniences; the app works without them (storage may be unavailable). */

export function loadPref<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const value = localStorage.getItem(`wispy.${key}`);
    return allowed.includes(value as T) ? (value as T) : fallback;
  } catch {
    return fallback;
  }
}

export function savePref(key: string, value: string) {
  try {
    localStorage.setItem(`wispy.${key}`, value);
  } catch {
    // Ignore: preferences are a convenience only.
  }
}

export function loadList(key: string): string[] {
  try {
    const value = JSON.parse(localStorage.getItem(`wispy.${key}`) ?? "[]");
    return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
  } catch {
    return [];
  }
}

export function saveList(key: string, values: string[]) {
  savePref(key, JSON.stringify(values));
}
