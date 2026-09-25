// Per-viewer conveniences in localStorage (the netlist being edited, the
// chosen example, open panel sections). Nothing leaves the browser, and the
// page works the same when storage is unavailable (private windows,
// blocked site data): every access is guarded.

export const store = {
  get(key: string): string | null {
    try {
      return localStorage.getItem(`acoustilab.${key}`);
    } catch {
      return null;
    }
  },
  set(key: string, value: string): void {
    try {
      localStorage.setItem(`acoustilab.${key}`, value);
    } catch {
      /* storage unavailable: nothing to remember */
    }
  },
  /** A JSON value, or `fallback` when absent or unreadable. */
  json<T>(key: string, fallback: T): T {
    const s = this.get(key);
    if (s === null) return fallback;
    try {
      return JSON.parse(s) as T;
    } catch {
      return fallback;
    }
  },
};
