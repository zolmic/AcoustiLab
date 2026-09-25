// AudioWorklet processor of the audition path (spec Section 16):
// programme → partitioned convolution (A or B filter, crossfaded on every
// change) → volume → true-peak limiter → output.
//
// Outputs: 0, the stereo audio; 1, the limiter's gain per sample (for the
// gain-reduction meter); 2, the processor's state per sample: the playing
// slot (0 = A, 1 = B) plus 2 while a filter is loading, 4 while a
// crossfade runs and 8 while the input is muted or fading in (read by the
// page through analysers, and by the tests).
//
// Messages (all handled outside process(), which allocates nothing and
// posts nothing):
//   { type: 'load', slot: 0 | 1, left: Float32Array, right: Float32Array, gain }
//       loads a filter and its linear gain into slot A or B; the transform
//       is spread over the following render quanta (4 partitions per
//       channel each), then the slot switches, crossfading if it plays;
//   { type: 'select', slot }   crossfades to slot A or B;
//   { type: 'limiter', ceilingDb, enabled };
//   { type: 'reset' }          clears the input history and the limiter;
//   { type: 'mute' }           fades the input out (over the crossfade
//                              length) and keeps it at zero;
//   { type: 'unmute' }         fades it back in once every filter posted
//                              before it is in place (nothing queued,
//                              loading or crossfading), so a new programme
//                              never plays through gains matched to the
//                              previous one.
// A load whose taps are not all finite, or whose gain is not a finite
// number from 0 to MAX_LOAD_GAIN (+40 dB), is refused (ignored). Input
// samples that are not finite are replaced by zeros.
// Parameter: `volume` (linear, k-rate, at most 1), ramped linearly across
// each quantum.

import { BLOCK, PartitionedConvolver } from './convolver';
import { TruePeakLimiter } from './limiter';
import { PROCESSOR_NAME } from './processor-name';

declare const sampleRate: number;
declare function registerProcessor(name: string, ctor: unknown): void;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
  constructor(options?: unknown);
}

/** Partitions transformed per channel per render quantum while loading. */
export const LOAD_PARTITIONS_PER_QUANTUM = 4;
/** Largest filter gain a load may carry (+40 dB, the page's level-match limit). */
export const MAX_LOAD_GAIN = 100;

function allFinite(x: ArrayLike<number>): boolean {
  for (let i = 0; i < x.length; i++) if (!Number.isFinite(x[i])) return false;
  return true;
}

interface Options {
  processorOptions?: { maxTaps?: number; ceilingDb?: number; fadeBlocks?: number };
}

interface Load {
  slot: number;
  left: Float32Array;
  right: Float32Array;
  gain: number;
}

class AuditionProcessor extends AudioWorkletProcessor {
  static get parameterDescriptors() {
    return [{ name: 'volume', defaultValue: 0.1, minValue: 0, maxValue: 1, automationRate: 'k-rate' }];
  }

  private readonly conv: PartitionedConvolver;
  private readonly limiter: TruePeakLimiter;
  private readonly bufL = new Float32Array(BLOCK);
  private readonly bufR = new Float32Array(BLOCK);
  private readonly outs: Float32Array[];
  /** The input after the mute gate. */
  private readonly inL = new Float32Array(BLOCK);
  private readonly inR = new Float32Array(BLOCK);
  private readonly ins: Float32Array[];
  /** Input gain (0 while muted) and where it is heading; an unmute waiting for the loads. */
  private inGain = 1;
  private inTarget = 1;
  private unmutePending = false;
  /** Bank of each slot; the third bank is free for loading. */
  private readonly slotBank = [0, 1];
  private freeBank = 2;
  private activeSlot = 0;
  private wantSlot = 0;
  /** Filter waiting to be loaded, per slot (a newer one replaces it). */
  private readonly queued: (Load | null)[] = [null, null];
  /** Slot of the load in progress, or of one loaded and waiting to switch. */
  private loadingSlot = -1;
  private loaded = false;
  private volume = 0.1;
  private readonly started = [false, false];
  private readonly pair: Float32Array[] = [new Float32Array(0), new Float32Array(0)];

  constructor(options: Options) {
    super(options);
    const o = options.processorOptions ?? {};
    this.conv = new PartitionedConvolver(o.maxTaps ?? 16384, 2, 3, (o.fadeBlocks ?? 2) * BLOCK);
    this.limiter = new TruePeakLimiter(sampleRate, { ceilingDb: o.ceilingDb ?? -1 });
    this.outs = [this.bufL, this.bufR];
    this.ins = [this.inL, this.inR];
    this.port.onmessage = (ev: MessageEvent) => this.onMessage(ev.data);
  }

  private onMessage(m: { type: string; slot?: number; left?: Float32Array; right?: Float32Array; gain?: number; ceilingDb?: number; enabled?: boolean }): void {
    switch (m.type) {
      case 'load': {
        const slot = m.slot === 1 ? 1 : 0;
        const left = m.left;
        const right = m.right ?? m.left;
        const gain = m.gain ?? 1;
        if (!(left instanceof Float32Array) || !(right instanceof Float32Array) || !allFinite(left) || !allFinite(right)) break;
        if (!(Number.isFinite(gain) && gain >= 0 && gain <= MAX_LOAD_GAIN)) break;
        this.queued[slot] = { slot, left, right, gain };
        break;
      }
      case 'mute':
        this.inTarget = 0;
        this.unmutePending = false;
        break;
      case 'unmute':
        this.unmutePending = true;
        break;
      case 'select':
        this.wantSlot = m.slot === 1 ? 1 : 0;
        break;
      case 'limiter':
        if (typeof m.ceilingDb === 'number') this.limiter.setCeiling(m.ceilingDb);
        if (typeof m.enabled === 'boolean') this.limiter.enabled = m.enabled;
        break;
      case 'reset':
        this.conv.reset();
        this.limiter.reset();
        break;
    }
  }

  /** Starts, advances and completes filter loads; switches slots. */
  private manage(): void {
    const conv = this.conv;
    if (!conv.loading && !this.loaded && !conv.fading) {
      // The playing slot first.
      const q = this.queued[this.activeSlot] ?? this.queued[1 - this.activeSlot];
      if (q) {
        this.pair[0] = q.left;
        this.pair[1] = q.right;
        if (conv.beginLoad(this.freeBank, this.pair, q.gain)) {
          this.loadingSlot = q.slot;
          this.queued[q.slot] = null;
        }
      }
    }
    if (conv.loading && conv.loadStep(LOAD_PARTITIONS_PER_QUANTUM)) this.loaded = true;
    if (this.loaded && !conv.fading) {
      const slot = this.loadingSlot;
      const old = this.slotBank[slot];
      this.slotBank[slot] = this.freeBank;
      this.freeBank = old;
      if (slot === this.activeSlot) conv.crossfadeTo(this.slotBank[slot], !this.started[slot]);
      this.started[slot] = true;
      this.loaded = false;
      this.loadingSlot = -1;
    }
    if (this.wantSlot !== this.activeSlot && !conv.fading && !this.loaded) {
      conv.crossfadeTo(this.slotBank[this.wantSlot]);
      this.activeSlot = this.wantSlot;
    }
    if (this.unmutePending && !this.busy() && !conv.fading) {
      this.inTarget = 1;
      this.unmutePending = false;
    }
  }

  private busy(): boolean {
    return this.conv.loading || this.loaded || this.queued[0] !== null || this.queued[1] !== null;
  }

  /** The input through the mute gate (ramped over the crossfade length), non-finite samples as zeros. */
  private gate(input: Float32Array[] | null): void {
    const step = 1 / this.conv.fadeLength;
    let g = this.inGain;
    const target = this.inTarget;
    const xl = input ? input[0] : null;
    const xr = input ? input[Math.min(1, input.length - 1)] : null;
    for (let i = 0; i < BLOCK; i++) {
      g = g < target ? Math.min(target, g + step) : Math.max(target, g - step);
      const l = xl ? xl[i] : 0;
      const r = xr ? xr[i] : 0;
      this.inL[i] = Number.isFinite(l) ? g * l : 0;
      this.inR[i] = Number.isFinite(r) ? g * r : 0;
    }
    this.inGain = g;
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][], parameters: Record<string, Float32Array>): boolean {
    this.manage();
    this.gate(inputs[0] && inputs[0].length ? inputs[0] : null);
    this.conv.process(this.ins, this.outs);
    const v0 = this.volume;
    const v1 = parameters.volume[0];
    this.volume = v1;
    for (let i = 0; i < BLOCK; i++) {
      const g = v0 + ((v1 - v0) * (i + 1)) / BLOCK;
      this.bufL[i] *= g;
      this.bufR[i] *= g;
    }
    const out = outputs[0];
    const gain = outputs[1]?.[0];
    const outL = out[0] ?? this.bufL;
    const outR = out[1] ?? this.bufR;
    this.limiter.process(this.bufL, this.bufR, outL, outR, BLOCK, gain);
    const status = outputs[2]?.[0];
    if (status) {
      const code = this.activeSlot + (this.busy() ? 2 : 0) + (this.conv.fading ? 4 : 0) + (this.inGain < 1 ? 8 : 0);
      status.fill(code);
    }
    return true;
  }
}

registerProcessor(PROCESSOR_NAME, AuditionProcessor);
