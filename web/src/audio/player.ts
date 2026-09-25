// The audition player on the main thread: the audio context, the worklet
// (or the ConvolverNode fallback), the looping programme, the level-match
// worker and the meters (spec Section 16).
//
// * The context is created on an explicit user action and requested at
//   48 kHz; if the device refuses, the default rate is taken and the page
//   redesigns the filters for it (`sampleRate`).
// * Worklet path: programme → AudioWorkletNode(processor.ts) → destination.
//   Meters read the output through analysers: the stereo audio (split),
//   the limiter's gain and the processor's state (outputs 1 and 2). The
//   worklet never posts messages; the page polls.
// * Fallback without AudioWorklet (insecure contexts, old engines): two
//   ConvolverNodes per slot, `normalize = false` set before the buffer is
//   assigned (the default normalisation rescales the filter and destroys
//   the level match), A/B by 5 ms gain ramps, and a DynamicsCompressorNode
//   near the ceiling standing in for the true-peak limiter, which it is
//   not (the diagnostics say so). WebKit and Gecko run late partitions of
//   long ConvolverNode filters on a background thread, Chromium does not
//   (erratum E35); either way the fallback stays exact in level.

import { StreamingLoudness } from './loudness';
import { TAPS_PER_PHASE, truePeak } from './oversample';
import type { Programme } from './noise';
import processorUrl from './processor.ts?worker&url';
import { PROCESSOR_NAME } from './processor-name';

export const REQUESTED_RATE = 48000;
export const RENDER_QUANTUM = 128;
/** Output volume at start-up, dB (re the level-matched programme). */
export const START_VOLUME_DB = -20;
export const VOLUME_RANGE_DB: [number, number] = [-60, 0];
export const CEILING_DBTP = -1;

export interface Diagnostics {
  sampleRate: number | null;
  requestedRate: number;
  rateRefused: boolean;
  baseLatency: number | null;
  outputLatency: number | null;
  renderQuantum: number;
  crossOriginIsolated: boolean;
  audioWorklet: boolean;
  path: 'worklet' | 'convolver' | null;
  wasmSimd: boolean;
  state: string;
}

export interface Meters {
  momentaryLufs: number;
  shortTermLufs: number;
  truePeakDbtp: number;
  /** Largest gain reduction of the limiter since the last poll, dB (≥ 0). */
  gainReductionDb: number;
  /** Playing slot (0 = A, 1 = B), whether a filter is loading or crossfading. */
  slot: number | null;
  loading: boolean;
}

/** Longest filter the worklet holds at `fs`: 16384 taps at 48 kHz, scaled with the rate. */
export function maxTaps(fs: number): number {
  return 16384 * 2 ** Math.max(0, Math.round(Math.log2(fs / 48000)));
}

/** WebAssembly SIMD support (a module with one v128 instruction validates). */
export function wasmSimd(): boolean {
  try {
    return WebAssembly.validate(
      new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253, 15, 253, 98, 11]),
    );
  } catch {
    return false;
  }
}

/**
 * A ConvolverNode holding `taps` exactly (no normalisation): the flag is
 * cleared before the buffer is assigned, as the Web Audio spec applies it
 * at assignment. The buffer must be at the context's rate.
 */
export function exactConvolver(ctx: BaseAudioContext, taps: Float32Array): ConvolverNode {
  const node = new ConvolverNode(ctx, { disableNormalization: true });
  node.normalize = false;
  const buf = new AudioBuffer({ length: taps.length, numberOfChannels: 1, sampleRate: ctx.sampleRate });
  buf.copyToChannel(taps as Float32Array<ArrayBuffer>, 0);
  node.buffer = buf;
  return node;
}

interface FallbackSlot {
  convL: ConvolverNode;
  convR: ConvolverNode;
  gain: GainNode;
}

export class AuditionPlayer {
  ctx: AudioContext | null = null;
  private node: AudioWorkletNode | null = null;
  private source: AudioBufferSourceNode | null = null;
  private programme: Programme | null = null;
  private master: GainNode | null = null;
  private splitter: ChannelSplitterNode | null = null;
  private readonly analysers: AnalyserNode[] = [];
  private readonly buffers: Float32Array<ArrayBuffer>[] = [];
  private loud: StreamingLoudness | null = null;
  private lastPoll = 0;
  private peakHold = { value: -Infinity, at: 0 };
  private rateRefused = false;
  private path: 'worklet' | 'convolver' | null = null;
  private volumeDb = START_VOLUME_DB;
  // Fallback graph.
  private fbSplit: ChannelSplitterNode | null = null;
  private fbMerge: ChannelMergerNode | null = null;
  private readonly fbSlots: (FallbackSlot | null)[] = [null, null];
  private fbSlot = 0;
  private fbLimiter: DynamicsCompressorNode | null = null;
  /** Force the ConvolverNode fallback (diagnostics switch). */
  forceFallback = false;

  get sampleRate(): number {
    return this.ctx?.sampleRate ?? REQUESTED_RATE;
  }

  get running(): boolean {
    return this.ctx?.state === 'running' && this.source !== null;
  }

  /** Creates the context (on a user action) and the processing graph. */
  async open(): Promise<void> {
    if (this.ctx) {
      if (this.ctx.state === 'suspended') await this.ctx.resume();
      return;
    }
    let ctx: AudioContext;
    try {
      ctx = new AudioContext({ sampleRate: REQUESTED_RATE, latencyHint: 'interactive' });
    } catch {
      ctx = new AudioContext({ latencyHint: 'interactive' });
    }
    this.rateRefused = ctx.sampleRate !== REQUESTED_RATE;
    this.ctx = ctx;
    this.master = new GainNode(ctx, { gain: 1 });
    this.splitter = new ChannelSplitterNode(ctx, { numberOfOutputs: 2 });
    this.master.connect(ctx.destination);
    this.master.connect(this.splitter);
    const analyser = () => new AnalyserNode(ctx, { fftSize: 16384 });
    this.analysers.length = 0;
    this.buffers.length = 0;
    for (let i = 0; i < 4; i++) {
      this.analysers.push(analyser());
      this.buffers.push(new Float32Array(16384));
    }
    this.splitter.connect(this.analysers[0], 0);
    this.splitter.connect(this.analysers[1], 1);
    this.loud = new StreamingLoudness(ctx.sampleRate);
    if (ctx.audioWorklet && !this.forceFallback) {
      await ctx.audioWorklet.addModule(processorUrl);
      const node = new AudioWorkletNode(ctx, PROCESSOR_NAME, {
        numberOfInputs: 1,
        numberOfOutputs: 3,
        outputChannelCount: [2, 1, 1],
        processorOptions: { maxTaps: maxTaps(ctx.sampleRate), ceilingDb: CEILING_DBTP },
      });
      node.connect(this.master, 0);
      node.connect(this.analysers[2], 1);
      node.connect(this.analysers[3], 2);
      this.node = node;
      this.path = 'worklet';
      this.setVolumeDb(this.volumeDb);
    } else {
      this.fbSplit = new ChannelSplitterNode(ctx, { numberOfOutputs: 2 });
      this.fbMerge = new ChannelMergerNode(ctx, { numberOfInputs: 2 });
      this.fbLimiter = new DynamicsCompressorNode(ctx, { threshold: CEILING_DBTP - 1, knee: 0, ratio: 20, attack: 0.001, release: 0.05 });
      this.fbMerge.connect(this.fbLimiter).connect(this.master);
      this.path = 'convolver';
      this.setVolumeDb(this.volumeDb);
    }
    if (ctx.state === 'suspended') await ctx.resume();
  }

  /** Sets the looping programme (restarts the source). */
  setProgramme(p: Programme): void {
    this.programme = p;
    if (this.source) this.startSource();
  }

  private startSource(): void {
    const ctx = this.ctx;
    const p = this.programme;
    if (!ctx || !p) return;
    this.source?.stop();
    this.source?.disconnect();
    const buf = new AudioBuffer({ length: p.channels[0].length, numberOfChannels: 2, sampleRate: ctx.sampleRate });
    buf.copyToChannel(p.channels[0] as Float32Array<ArrayBuffer>, 0);
    buf.copyToChannel(p.channels[1] as Float32Array<ArrayBuffer>, 1);
    const src = new AudioBufferSourceNode(ctx, { buffer: buf, loop: true });
    if (this.node) src.connect(this.node);
    else if (this.fbSplit) src.connect(this.fbSplit);
    src.start();
    this.source = src;
  }

  /** Starts playback (the context must be open). */
  play(): void {
    if (!this.ctx) return;
    this.startSource();
    void this.ctx.resume();
  }

  stop(): void {
    this.source?.stop();
    this.source?.disconnect();
    this.source = null;
    void this.ctx?.suspend();
    this.loud?.reset();
    this.peakHold = { value: -Infinity, at: 0 };
  }

  async close(): Promise<void> {
    this.stop();
    await this.ctx?.close();
    this.ctx = null;
    this.node = null;
    this.path = null;
  }

  /** Loads a filter (per channel taps) and its gain into slot A (0) or B (1). */
  loadFilter(slot: 0 | 1, left: Float32Array, right: Float32Array, gainDb: number): void {
    const gain = 10 ** (gainDb / 20);
    if (this.node) {
      this.node.port.postMessage({ type: 'load', slot, left, right, gain });
      return;
    }
    const ctx = this.ctx;
    if (!ctx || !this.fbSplit || !this.fbMerge) return;
    const s: FallbackSlot = {
      convL: exactConvolver(ctx, left),
      convR: exactConvolver(ctx, right),
      gain: new GainNode(ctx, { gain: 0 }),
    };
    const merge = new ChannelMergerNode(ctx, { numberOfInputs: 2 });
    this.fbSplit.connect(s.convL, 0);
    this.fbSplit.connect(s.convR, 1);
    s.convL.connect(merge, 0, 0);
    s.convR.connect(merge, 0, 1);
    merge.connect(s.gain).connect(this.fbMerge);
    const t = ctx.currentTime;
    const old = this.fbSlots[slot];
    const on = slot === this.fbSlot ? gain : 0;
    s.gain.gain.setValueAtTime(0, t);
    s.gain.gain.linearRampToValueAtTime(on, t + 0.005);
    (s.gain as GainNode & { target?: number }).target = gain;
    if (old) {
      old.gain.gain.setValueAtTime(old.gain.gain.value, t);
      old.gain.gain.linearRampToValueAtTime(0, t + 0.005);
      setTimeout(() => {
        old.convL.disconnect();
        old.convR.disconnect();
        old.gain.disconnect();
      }, 50);
    }
    this.fbSlots[slot] = s;
  }

  /** Plays slot A (0) or B (1), crossfading. */
  select(slot: 0 | 1): void {
    if (this.node) {
      this.node.port.postMessage({ type: 'select', slot });
      return;
    }
    const ctx = this.ctx;
    this.fbSlot = slot;
    if (!ctx) return;
    const t = ctx.currentTime;
    this.fbSlots.forEach((s, i) => {
      if (!s) return;
      const target = (s.gain as GainNode & { target?: number }).target ?? 1;
      s.gain.gain.setValueAtTime(s.gain.gain.value, t);
      s.gain.gain.linearRampToValueAtTime(i === slot ? target : 0, t + 0.005);
    });
  }

  setVolumeDb(db: number): void {
    this.volumeDb = db;
    const v = 10 ** (db / 20);
    const ctx = this.ctx;
    if (!ctx) return;
    if (this.node) {
      this.node.parameters.get('volume')?.setTargetAtTime(v, ctx.currentTime, 0.02);
    } else if (this.master) {
      this.master.gain.setTargetAtTime(v, ctx.currentTime, 0.02);
    }
  }

  setLimiter(ceilingDb: number, enabled: boolean): void {
    this.node?.port.postMessage({ type: 'limiter', ceilingDb, enabled });
  }

  /** Reads the analysers (call every ~100 ms while playing). */
  poll(): Meters | null {
    const ctx = this.ctx;
    if (!ctx || !this.loud || !this.running) return null;
    const now = ctx.currentTime;
    const size = this.buffers[0].length;
    const fresh = Math.min(size, Math.max(0, Math.round((now - this.lastPoll) * ctx.sampleRate)));
    this.lastPoll = now;
    for (let i = 0; i < 4; i++) this.analysers[i].getFloatTimeDomainData(this.buffers[i]);
    const from = size - fresh;
    const l = this.buffers[0].subarray(from);
    const r = this.buffers[1].subarray(from);
    this.loud.push([l, r], fresh);
    // The newest samples lack the neighbours the interpolation needs; they
    // are measured at the next poll (the ranges overlap).
    const edge = TAPS_PER_PHASE;
    const tp = Math.max(truePeak(this.buffers[0], from - edge, size - edge), truePeak(this.buffers[1], from - edge, size - edge));
    const tpDb = tp > 0 ? 20 * Math.log10(tp) : -Infinity;
    if (tpDb >= this.peakHold.value || now - this.peakHold.at > 3) this.peakHold = { value: tpDb, at: now };
    let minGain = 1;
    let status = -1;
    if (this.path === 'worklet') {
      const g = this.buffers[2].subarray(from);
      for (let i = 0; i < g.length; i++) minGain = Math.min(minGain, g[i]);
      status = Math.round(this.buffers[3][size - 1]);
    }
    return {
      momentaryLufs: this.loud.momentary(),
      shortTermLufs: this.loud.shortTerm(),
      truePeakDbtp: this.peakHold.value,
      gainReductionDb: minGain > 0 ? -20 * Math.log10(minGain) : Infinity,
      slot: status >= 0 ? status & 1 : this.fbSlot,
      loading: status >= 0 ? (status & 6) !== 0 : false,
    };
  }

  diagnostics(): Diagnostics {
    const ctx = this.ctx;
    return {
      sampleRate: ctx?.sampleRate ?? null,
      requestedRate: REQUESTED_RATE,
      rateRefused: this.rateRefused,
      baseLatency: ctx && typeof ctx.baseLatency === 'number' ? ctx.baseLatency : null,
      outputLatency: ctx && typeof ctx.outputLatency === 'number' ? ctx.outputLatency : null,
      renderQuantum: RENDER_QUANTUM,
      crossOriginIsolated: typeof crossOriginIsolated === 'boolean' ? crossOriginIsolated : false,
      audioWorklet: typeof AudioWorkletNode !== 'undefined',
      path: this.path,
      wasmSimd: wasmSimd(),
      state: ctx?.state ?? 'closed',
    };
  }
}
