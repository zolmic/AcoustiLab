// Web Worker hosting the WebAssembly engine, so a solve never blocks the UI.
//
// Request:  { id, op: 'solve' | 'check' | 'parameters' | 'types' | 'version', netlist? }
//           { id, op: 'call', fn, args }  any wasm export by name, with
//                                         string arguments (result views use
//                                         this; see views/types.ts)
// Reply:    { id, ok: true, value, ms }   value = parsed engine JSON
//           { id, ok: false, crash }      the engine trapped (Rust panic) or
//                                         failed to load; the client
//                                         (engine.ts) then replaces this
//                                         worker with a fresh one.

import init, * as engine from '@engine/acoustilab_wasm.js';
import { check, element_types, engine_version, parameters, solve, take_last_panic } from '@engine/acoustilab_wasm.js';
import wasmUrl from '@engine/acoustilab_wasm_bg.wasm?url';

type Op = 'solve' | 'check' | 'parameters' | 'types' | 'version' | 'call';
interface Request {
  id: number;
  op: Op;
  netlist?: string;
  /** For `call`: the export's name and its string arguments. */
  fn?: string;
  args?: string[];
}

/** Exports that `call` may not reach (lifecycle and test hooks). */
const NOT_CALLABLE = new Set(['default', 'initSync', 'take_last_panic']);

/** Calls a wasm export by name; JSON text results are parsed. */
function callExport(fn: string, args: string[]): unknown {
  const f = (engine as unknown as Record<string, unknown>)[fn];
  if (NOT_CALLABLE.has(fn) || typeof f !== 'function') {
    return { error: `the engine has no export '${fn}'`, kind: 'other' };
  }
  const out = (f as (...a: string[]) => unknown)(...args);
  if (typeof out !== 'string') return out;
  try {
    return JSON.parse(out);
  } catch {
    return out;
  }
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
    case 'call':
      return callExport(req.fn ?? '', req.args ?? []);
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
