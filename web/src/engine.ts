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

  /** Requests posted per operation (a test hook: live solves are coalesced). */
  readonly posted: Record<string, number> = {};

  call(op: 'solve' | 'check' | 'parameters' | 'types' | 'version', netlist?: string): Promise<Reply> {
    const id = ++this.seq;
    this.posted[op] = (this.posted[op] ?? 0) + 1;
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

/**
 * Coalesced requests of one operation on a worker (spec Section 15,
 * "Responsiveness"): at most one call is in flight, and while it runs only
 * the newest request is kept; it is sent as soon as the call returns. A
 * slider drag therefore never queues work: the engine always computes the
 * newest state it can, and the previous result stays on screen meanwhile.
 */
export class Coalesced {
  private inflight: string | null = null;
  private next: string | null = null;
  /** Bumped by restart/cancel, so replies of abandoned calls are ignored. */
  private gen = 0;

  constructor(
    private readonly worker: EngineWorker,
    private readonly op: 'solve' | 'parameters',
    /** `superseded`: a newer request is already on its way. */
    private readonly deliver: (text: string, reply: Reply, superseded: boolean) => void,
  ) {}

  /** A call is in flight or queued. */
  get busy(): boolean {
    return this.inflight !== null || this.next !== null;
  }

  /** Solves `text` next; nothing is sent again for the text already in flight. */
  request(text: string): void {
    if (this.inflight !== null) {
      this.next = text === this.inflight ? null : text;
      return;
    }
    void this.send(text);
  }

  /** Abandons any call in flight (terminating the worker) and sends `text` now. */
  restart(text: string): void {
    this.cancel();
    void this.send(text);
  }

  /** Abandons the call in flight (terminating the worker) and the queued request. */
  cancel(): void {
    this.gen++;
    this.next = null;
    if (this.inflight !== null) this.worker.cancel();
    this.inflight = null;
  }

  private async send(text: string): Promise<void> {
    const gen = this.gen;
    this.inflight = text;
    let reply: Reply;
    try {
      reply = await this.worker.call(this.op, text);
    } catch (e) {
      if (e instanceof Cancelled) return;
      throw e;
    }
    if (gen !== this.gen) return;
    this.inflight = null;
    const next = this.next;
    this.next = null;
    // Start the next call before rendering this reply, so the worker never
    // idles while the main thread draws.
    if (next !== null) void this.send(next);
    this.deliver(text, reply, next !== null);
  }
}
