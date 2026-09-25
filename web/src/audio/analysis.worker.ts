// Worker for the level match: convolves the looping programme with each
// filter and measures the result (level.ts), off the main thread.
//
// Request:  { id, programme: { key, channels? }, filters: Float32Array[][], fs }
//           `channels` is sent once per programme key and kept here.
// Reply:    { id, programme: Measurement, filters: Measurement[], ms }
//           or { id, error }

import { convolveProgramme, measure, type Measurement } from './level';

interface Request {
  id: number;
  programme: { key: string; channels?: Float32Array[] };
  filters: Float32Array[][];
  fs: number;
}

const scope = self as unknown as {
  postMessage(message: unknown): void;
  onmessage: ((ev: MessageEvent<Request>) => void) | null;
};

let cached: { key: string; channels: Float32Array[]; plain: Measurement | null; fs: number } | null = null;

scope.onmessage = (ev) => {
  const req = ev.data;
  const t0 = performance.now();
  try {
    if (req.programme.channels) cached = { key: req.programme.key, channels: req.programme.channels, plain: null, fs: req.fs };
    if (!cached || cached.key !== req.programme.key) throw new Error('programme not loaded in the analysis worker');
    if (!cached.plain || cached.fs !== req.fs) {
      cached.plain = measure(cached.channels, req.fs);
      cached.fs = req.fs;
    }
    const filters = req.filters.map((taps) => measure(convolveProgramme(cached!.channels, taps), req.fs));
    scope.postMessage({ id: req.id, programme: cached.plain, filters, ms: performance.now() - t0 });
  } catch (e) {
    scope.postMessage({ id: req.id, error: String(e) });
  }
};
