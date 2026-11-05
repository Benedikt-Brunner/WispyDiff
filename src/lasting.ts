import { useCallback, useSyncExternalStore, type SetStateAction } from "react";

// State that outlives a component: the diff viewer remounts whenever a new version of the stack
// loads (or after a trip to the inbox), and work it started (an assistant answer) must land.
const values = new Map<string, unknown>();
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => void listeners.delete(listener);
};

export function setLasting<T>(key: string, next: SetStateAction<T>, initial: T) {
  const current = (values.has(key) ? values.get(key) : initial) as T;
  const value = typeof next === "function" ? (next as (prev: T) => T)(current) : next;
  if (Object.is(value, current)) return;
  values.set(key, value);
  listeners.forEach((l) => l());
}

/** Like `useState`, but kept under `key` across unmounts. `initial` must be a constant. */
export function useLasting<T>(key: string, initial: T): [T, (next: SetStateAction<T>) => void] {
  const value = useSyncExternalStore(subscribe, () => (values.has(key) ? values.get(key) : initial) as T);
  const set = useCallback((next: SetStateAction<T>) => setLasting(key, next, initial), [key, initial]);
  return [value, set];
}
