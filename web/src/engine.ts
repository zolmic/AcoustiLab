// Promise-based client of the engine worker. `cancel()` terminates the
// worker (the only way to stop a running wasm call). After an engine crash
// (a Rust panic traps the wasm instance) the worker is discarded too:
// wasm-bindgen's init() would otherwise hand back the trapped instance.
//
// A worker is spawned lazily, by the first call after the previous one was
// discarded. Respawning eagerly on `error` would loop without end when the
// worker script itself cannot load (every new worker fails the same way).

export type Reply = { ok: true; value: unknown; ms: number } | { ok: false; crash: string };

export class Cancelled extends Error {
  constructor() {
    super('cancelled');
  }
}

interface Pending {
  resolve(r: Reply): void;
  reject(e: unknown): void;
}

export class EngineWorker {
  private worker: Worker | null = null;
  private seq = 0;
  private pending = new Map<number, Pending>();

  private spawn(): Worker {
    const w = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', name: 'acoustilab-engine' });
    w.onmessage = (ev: MessageEvent<{ id: number } & Reply>) => {
      if (this.worker !== w) return;
      const p = this.pending.get(ev.data.id);
      if (!p) return;
      this.pending.delete(ev.data.id);
      const { id: _id, ...reply } = ev.data;
      p.resolve(reply as Reply);
      if (!reply.ok) this.discard('the engine was restarted after a crash');
    };
    w.onerror = (ev) => {
      ev.preventDefault();
      if (this.worker !== w) return;
      this.discard(ev.message || 'the engine worker failed to start or crashed');
    };
    return w;
  }

  /** Drops the worker; calls still pending on it fail with `reason`. */
  private discard(reason: string): void {
    this.worker?.terminate();
    this.worker = null;
    const calls = [...this.pending.values()];
    this.pending.clear();
    for (const p of calls) p.resolve({ ok: false, crash: reason });
  }

  get busy(): boolean {
    return this.pending.size > 0;
  }

  call(op: 'solve' | 'check' | 'types' | 'version', netlist?: string): Promise<Reply> {
    const id = ++this.seq;
    this.worker ??= this.spawn();
    const w = this.worker;
    return new Promise<Reply>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      w.postMessage({ id, op, netlist });
    });
  }

  cancel(): void {
    this.worker?.terminate();
    this.worker = null;
    const calls = [...this.pending.values()];
    this.pending.clear();
    for (const p of calls) p.reject(new Cancelled());
  }
}
