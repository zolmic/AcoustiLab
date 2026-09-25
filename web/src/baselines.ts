// Frozen baselines: snapshots of results drawn as muted overlays (thin
// lines, one dash pattern per baseline, see series.ts), named, removable,
// and one of them chosen as the reference for the readout's Δ and the
// difference plot. They live in memory for this page only.

import { svgKey } from './keys';
import { OVERLAY_DASHES, OVERLAY_WIDTH, type Baseline } from './series';
import type { SolveResult } from './types';

export interface BaselineElements {
  list: HTMLElement;
  /** Holds the Δ reference picker and the difference-plot switch. */
  options: HTMLElement;
  reference: HTMLSelectElement;
  difference: HTMLInputElement;
  freeze: HTMLButtonElement;
}

export class Baselines {
  items: Baseline[] = [];
  /** Id of the baseline the Δ readout and the difference plot refer to. */
  private refId: number | null = null;
  private seq = 0;

  constructor(
    private readonly els: BaselineElements,
    /** The set, a name or the reference changed: redraw what depends on them. */
    private readonly onChange: () => void,
  ) {
    els.reference.addEventListener('change', () => {
      this.refId = els.reference.value ? Number(els.reference.value) : null;
      this.syncOptions();
      this.onChange();
    });
    els.difference.addEventListener('change', () => this.onChange());
    this.render();
  }

  get reference(): Baseline | null {
    return this.items.find((b) => b.id === this.refId) ?? null;
  }

  /** The reference to draw a difference plot against, if that plot is on. */
  get difference(): Baseline | null {
    return this.els.difference.checked ? this.reference : null;
  }

  /** `name` made unique among the baselines. */
  uniqueName(name: string): string {
    let unique = name;
    for (let k = 2; this.items.some((b) => b.name === unique); k++) unique = `${name} (${k})`;
    return unique;
  }

  /** Freezes a result (solved from `text`) under `name`; the first baseline becomes the Δ reference. */
  add(r: SolveResult, text: string, name: string): void {
    const b: Baseline = {
      id: ++this.seq,
      name: this.uniqueName(name),
      text,
      freqs: r.frequencies_Hz,
      probes: r.probes.map((p) => ({
        id: p.id,
        quantity: p.quantity,
        unit: p.unit,
        domain: p.domain,
        magnitude: p.magnitude,
        phase_deg: p.phase_deg,
        spl_dB: p.spl_dB,
      })),
    };
    this.items.push(b);
    this.refId ??= b.id;
    this.render();
    this.onChange();
    this.els.list.querySelector<HTMLInputElement>(`[data-baseline="${b.id}"] input`)?.focus();
  }

  remove(id: number): void {
    const k = this.items.findIndex((b) => b.id === id);
    if (k < 0) return;
    this.items.splice(k, 1);
    if (this.refId === id) this.refId = this.items.length ? this.items[this.items.length - 1].id : null;
    this.render();
    this.onChange();
    // Keep keyboard focus in the list (or on Freeze once it is empty).
    const next = this.els.list.querySelectorAll<HTMLButtonElement>('.baseline-remove')[Math.min(k, this.items.length - 1)];
    (next ?? this.els.freeze).focus();
  }

  private render(): void {
    const { list } = this.els;
    list.replaceChildren();
    this.items.forEach((b, slot) => {
      const li = document.createElement('li');
      li.className = 'baseline-item';
      li.dataset.baseline = String(b.id);
      const key = svgKey('var(--ink-2)', OVERLAY_DASHES[slot % OVERLAY_DASHES.length], OVERLAY_WIDTH * 1.4, 32);
      const input = document.createElement('input');
      input.type = 'text';
      input.value = b.name;
      input.className = 'baseline-name';
      input.setAttribute('aria-label', `Name of baseline ${slot + 1}`);
      const rm = document.createElement('button');
      rm.type = 'button';
      rm.className = 'baseline-remove';
      rm.textContent = 'Remove';
      const label = () => rm.setAttribute('aria-label', `Remove baseline “${b.name}”`);
      label();
      let t = 0;
      input.addEventListener('input', () => {
        b.name = input.value.trim() || `baseline ${slot + 1}`;
        label();
        clearTimeout(t);
        t = window.setTimeout(() => {
          this.syncOptions();
          this.onChange();
        }, 250);
      });
      rm.addEventListener('click', () => this.remove(b.id));
      const tag = document.createElement('span');
      tag.className = 'hint baseline-tag';
      tag.textContent = 'frozen, thin patterned lines';
      li.append(key, input, tag, rm);
      list.append(li);
    });
    this.syncOptions();
  }

  private syncOptions(): void {
    const { options, reference, difference } = this.els;
    options.hidden = this.items.length === 0;
    reference.replaceChildren();
    const none = document.createElement('option');
    none.value = '';
    none.textContent = 'none';
    reference.append(none);
    for (const b of this.items) {
      const o = document.createElement('option');
      o.value = String(b.id);
      o.textContent = b.name;
      reference.append(o);
    }
    reference.value = this.refId === null ? '' : String(this.refId);
    if (this.refId === null) difference.checked = false;
    difference.disabled = this.refId === null;
  }
}
