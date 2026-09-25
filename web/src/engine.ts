// Promise-based client of the engine worker. `cancel()` terminates the
// worker (the only way to stop a running wasm call) and starts a fresh one.
// After an engine crash (a Rust panic traps the wasm instance) the worker is
// replaced too: wasm-bindgen's init() would otherwise hand back the trapped
// instance.

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
  private worker: Worker;
  private seq = 0;
  private pending = new Map<number, Pending>();

  constructor() {
    this.worker = this.spawn();
  }

  private spawn(): Worker {
    const w = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', name: 'acoustilab-engine' });
    w.onmessage = (ev: MessageEvent<{ id: number } & Reply>) => {
      const p = this.pending.get(ev.data.id);
      if (!p) return;
      this.pending.delete(ev.data.id);
      const { id: _id, ...reply } = ev.data;
      p.resolve(reply as Reply);
      if (!reply.ok) this.restart('the engine was restarted after a crash');
    };
    w.onerror = (ev) => {
      ev.preventDefault();
      this.restart(ev.message || 'worker error');
    };
    return w;
  }

  /** Replaces the worker; calls still pending on the old one fail with `reason`. */
  private restart(reason: string): void {
    this.worker.terminate();
    for (const p of this.pending.values()) p.resolve({ ok: false, crash: reason });
    this.pending.clear();
    this.worker = this.spawn();
  }

  get busy(): boolean {
    return this.pending.size > 0;
  }

  call(op: 'solve' | 'check' | 'types' | 'version', netlist?: string): Promise<Reply> {
    const id = ++this.seq;
    return new Promise<Reply>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, op, netlist });
    });
  }

  cancel(): void {
    this.worker.terminate();
    for (const p of this.pending.values()) p.reject(new Cancelled());
    this.pending.clear();
    this.worker = this.spawn();
  }
}
