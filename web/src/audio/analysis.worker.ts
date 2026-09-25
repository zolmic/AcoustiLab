// Worker for the level match: convolves the looping programme with each
// filter and measures the result (level.ts), off the main thread.
//
// Request:  { id, programme: { key, channels?, mono? }, filters: { key?, taps }[], fs }
//           `channels` is sent once per programme key and kept here; with
//           `mono` one channel stands for both (it is then convolved and
//           measured once per filter, not twice). A filter with a `key`
//           (e.g. the reference path's delay) is measured once per
//           programme and remembered.
// Reply:    { id, programme: Measurement, filters: Measurement[], ms }
//           or { id, error }

import { convolveProgramme, measure, type Measurement } from './level';

interface Request {
  id: number;
  programme: { key: string; channels?: Float32Array[]; mono?: boolean };
  filters: { key?: string; taps: Float32Array[] }[];
  fs: number;
}

const scope = self as unknown as {
  postMessage(message: unknown): void;
  onmessage: ((ev: MessageEvent<Request>) => void) | null;
};

interface Cached {
  key: string;
  channels: Float32Array[];
  fs: number;
  plain: Measurement | null;
  byFilter: Map<string, Measurement>;
}

let cached: Cached | null = null;

scope.onmessage = (ev) => {
  const req = ev.data;
  const t0 = performance.now();
  try {
    const p = req.programme;
    if (p.channels) {
      const channels = p.mono ? [p.channels[0], p.channels[0]] : p.channels;
      cached = { key: p.key, channels, fs: req.fs, plain: null, byFilter: new Map() };
    }
    const c = cached;
    if (!c || c.key !== p.key) throw new Error('programme not loaded in the analysis worker');
    if (c.fs !== req.fs) {
      c.fs = req.fs;
      c.plain = null;
      c.byFilter.clear();
    }
    c.plain ??= measure(c.channels, req.fs);
    const filters = req.filters.map(({ key, taps }) => {
      const hit = key ? c.byFilter.get(key) : undefined;
      if (hit) return hit;
      const m = measure(convolveProgramme(c.channels, taps), req.fs);
      if (key) c.byFilter.set(key, m);
      return m;
    });
    scope.postMessage({ id: req.id, programme: c.plain, filters, ms: performance.now() - t0 });
  } catch (e) {
    scope.postMessage({ id: req.id, error: String(e) });
  }
};
