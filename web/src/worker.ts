// Web Worker hosting the WebAssembly engine, so a solve never blocks the UI.
//
// Request:  { id, op: 'solve' | 'check' | 'parameters' | 'types' | 'version', netlist? }
// Reply:    { id, ok: true, value, ms }   value = parsed engine JSON
//           { id, ok: false, crash }      the engine trapped (Rust panic) or
//                                         failed to load; the client
//                                         (engine.ts) then replaces this
//                                         worker with a fresh one.

import init, { check, element_types, engine_version, parameters, solve, take_last_panic } from '@engine/acoustilab_wasm.js';
import wasmUrl from '@engine/acoustilab_wasm_bg.wasm?url';

type Op = 'solve' | 'check' | 'parameters' | 'types' | 'version';
interface Request {
  id: number;
  op: Op;
  netlist?: string;
}

// The DOM lib types `self` as a Window; in a dedicated worker it is the
// worker scope, which needs only these two members here.
const scope = self as unknown as {
  postMessage(message: unknown): void;
  onmessage: ((ev: MessageEvent<Request>) => void) | null;
};

let ready: Promise<unknown> | null = null;
const ensure = () => (ready ??= init({ module_or_path: wasmUrl }));

function run(req: Request): unknown {
  switch (req.op) {
    case 'solve':
      return JSON.parse(solve(req.netlist ?? ''));
    case 'check':
      return JSON.parse(check(req.netlist ?? ''));
    case 'parameters':
      return JSON.parse(parameters(req.netlist ?? '', ''));
    case 'types':
      return JSON.parse(element_types());
    case 'version':
      return engine_version();
  }
}

scope.onmessage = async (ev) => {
  const req = ev.data;
  try {
    await ensure();
  } catch (e) {
    ready = null;
    scope.postMessage({ id: req.id, ok: false, crash: `could not load the engine: ${String(e)}` });
    return;
  }
  const t0 = performance.now();
  try {
    const value = run(req);
    scope.postMessage({ id: req.id, ok: true, value, ms: performance.now() - t0 });
  } catch (e) {
    // A Rust panic traps as a RuntimeError ("unreachable"); its message was
    // recorded by the panic hook before the trap.
    let panic = '';
    try {
      panic = take_last_panic();
    } catch {
      /* the trapped instance may be unusable */
    }
    scope.postMessage({ id: req.id, ok: false, crash: panic || `engine failure: ${String(e)}` });
  }
};
