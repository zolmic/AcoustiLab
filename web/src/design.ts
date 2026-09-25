// Design panel: one control per declared parameter (docs/parameters.md),
// generated from the engine's `parameters()` description, in sections per
// `group` and in declaration order.
//
// The panel never holds the design: every committed value goes to the
// owner (`set`), which writes it into the netlist text; the text is then
// described again by the engine and the panel shows what came back. Values
// typed out of bounds are rejected here, inline, and never reach the text.

import { formatParam, paramUnit } from './format';
import { store } from './store';
import type { ParamDesc, ParamsDoc, Scalar, Tolerance } from './types';

export interface DesignCallbacks {
  /** Write these parameter values into the netlist. */
  set(values: [string, Scalar][]): void;
  /** The pointer or keyboard focus is on this parameter's row (null: none). */
  hover(name: string | null): void;
  /** Select the parameter's declaration in the netlist editor. */
  showInNetlist(name: string): void;
}

/** Positions of a slider that has no step, or too many steps. */
const SLIDER_N = 1000;

interface SliderScale {
  n: number;
  toValue(pos: number): number;
  toPos(v: number): number;
}

/** Slider mapping, or null when a bound is missing (entry only). */
function sliderScale(d: ParamDesc): SliderScale | null {
  const { min, max } = d;
  if (min === null || min === undefined || max === null || max === undefined || !(max > min)) return null;
  if (d.log && min > 0) {
    const r = Math.log(max / min);
    return {
      n: SLIDER_N,
      toValue: (p) => min * Math.exp((r * p) / SLIDER_N),
      toPos: (v) => Math.round((SLIDER_N * Math.log(Math.max(v, min) / min)) / r),
    };
  }
  const steps = d.step ? Math.round((max - min) / d.step) : 0;
  const n = steps > 0 && steps <= SLIDER_N ? steps : SLIDER_N;
  return {
    n,
    toValue: (p) => min + ((max - min) * p) / n,
    toPos: (v) => Math.round((n * (v - min)) / (max - min)),
  };
}

/** Digits after the decimal point of a step (0.005 -> 3, 1e-7 -> 7). */
function decimalsOf(x: number): number {
  const m = /(?:\.(\d+))?(?:e-(\d+))?$/.exec(String(x))!;
  return (m[1]?.length ?? 0) + Number(m[2] ?? 0);
}

const clampTo = (d: ParamDesc, x: number) =>
  Math.min(d.max ?? Infinity, Math.max(d.min ?? -Infinity, x));

/**
 * A value produced by a slider or a key press: a multiple of the step
 * (three significant figures first on log sliders), within the bounds.
 * Typed values are not quantized: the step is a hint, not a constraint.
 */
function quantize(d: ParamDesc, v: number): number {
  let x = v;
  if (d.log) x = Number(x.toPrecision(3));
  const step = d.kind === 'integer' ? (d.step ?? 1) : d.step;
  if (step && step > 0) {
    x = Number((Math.round(x / step) * step).toFixed(Math.min(15, decimalsOf(step))));
  } else if (d.min !== null && d.min !== undefined && d.max !== null && d.max !== undefined) {
    const q = 10 ** Math.floor(Math.log10((d.max - d.min) / SLIDER_N));
    x = Number((Math.round(x / q) * q).toPrecision(12));
  } else {
    x = Number(x.toPrecision(4));
  }
  return clampTo(d, x);
}

/** Moves a value by `k` key steps: the step (or 1 % of the range), or 1 % of a log range. */
function nudge(d: ParamDesc, sc: SliderScale | null, v: number, k: number): number {
  let x: number;
  if (sc && d.log) {
    x = quantize(d, sc.toValue(Math.min(sc.n, Math.max(0, sc.toPos(v) + (k * sc.n) / 100))));
  } else {
    const inc = d.kind === 'integer' ? (d.step ?? 1) : (d.step ?? (sc ? (d.max! - d.min!) / 100 : Math.abs(v) / 100 || 1));
    x = quantize(d, v + k * inc);
  }
  // Rounding must never swallow a key press.
  if (x === v && d.step) x = clampTo(d, Number((v + Math.sign(k) * d.step).toFixed(decimalsOf(d.step))));
  return x;
}

/** "±15 % (normal, 2σ), datasheet (…)" */
export function toleranceText(t: Tolerance, unit: string): string {
  const w = t.rel !== undefined ? `±${formatParam(t.rel * 100, 3)} %` : `±${formatParam(t.abs ?? 0, 3)}${unit ? ` ${unit}` : ''}`;
  const dist = t.dist === 'uniform' ? 'uniform' : t.dist === 'lognormal' ? 'log-normal, 2σ' : 'normal, 2σ';
  return `${w} (${dist})${t.source ? `, ${t.source}` : ''}`;
}

const same = (a: Scalar | null | undefined, b: Scalar | null | undefined) =>
  typeof a === 'number' && typeof b === 'number' ? Math.abs(a - b) <= 1e-12 * Math.max(Math.abs(a), Math.abs(b)) : a === b;

type Parsed = { ok: true; value: number } | { ok: false; message: string };

/** Parses a typed number, optionally followed by the parameter's unit. */
function parseEntry(text: string, d: ParamDesc): Parsed {
  const unit = paramUnit(d.unit);
  let t = text.trim();
  for (const u of [unit, d.unit ?? '']) if (u && t.endsWith(u)) t = t.slice(0, -u.length).trim();
  if (!t.includes('.') && /^[-+]?\d+,\d+$/.test(t)) t = t.replace(',', '.');
  const v = /^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/i.test(t) ? Number(t) : NaN;
  const u = unit ? ` ${unit}` : '';
  if (!Number.isFinite(v)) return { ok: false, message: `Enter a number${unit ? ` in ${unit}` : ''}.` };
  if (d.kind === 'integer' && !Number.isInteger(v)) return { ok: false, message: 'Enter a whole number.' };
  if (d.min !== null && d.min !== undefined && v < d.min) return { ok: false, message: `The minimum is ${formatParam(d.min, 6)}${u}.` };
  if (d.max !== null && d.max !== undefined && v > d.max) return { ok: false, message: `The maximum is ${formatParam(d.max, 6)}${u}.` };
  return { ok: true, value: v };
}

/** Text shown in a numeric entry: the value as the netlist holds it, without float noise. */
const entryText = (v: number) => String(Number(v.toPrecision(12)));

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

interface Row {
  desc: ParamDesc;
  el: HTMLElement;
  /** Shows a value; `force` also overwrites a control the user is editing. */
  show(v: Scalar | null, force: boolean): void;
  /** Marks the row as changed from the reference, with the value to reset to. */
  setChanged(ref: Scalar | undefined): void;
  focus(): void;
}

export class DesignPanel {
  private rows = new Map<string, Row>();
  private groupEls = new Map<string, HTMLDetailsElement>();
  private sig = '';
  private detailed = store.get('design.detailed') === '1';
  private open: Record<string, boolean> = store.json('design.groups', {});
  private doc: ParamsDoc | null = null;
  /** Row the user is editing (focus inside it). */
  private active: string | null = null;
  private pointer: string | null = null;
  /** Template values that "Reset" restores, by parameter name. */
  private reference = new Map<string, Scalar>();

  constructor(
    private readonly groupsEl: HTMLElement,
    private readonly messageEl: HTMLElement,
    private readonly detailToggle: HTMLButtonElement,
    private readonly resetAll: HTMLButtonElement,
    private readonly cb: DesignCallbacks,
  ) {
    detailToggle.addEventListener('click', () => this.setDetailed(!this.detailed));
    resetAll.addEventListener('click', () => {
      const changes = this.changed();
      if (changes.length) this.cb.set(changes);
    });
    this.setDetailed(this.detailed);
  }

  /** Parameters described by the last update. */
  get params(): ParamDesc[] {
    return this.doc?.parameters ?? [];
  }

  /** Current values shown, by name (derived included). */
  values(): Map<string, Scalar> {
    const m = new Map<string, Scalar>();
    for (const p of this.params) if (p.value !== null) m.set(p.name, p.value);
    return m;
  }

  setReference(ref: Map<string, Scalar>): void {
    this.reference = ref;
    this.refreshChanged();
  }

  /** Parameters whose value differs from the reference, with the reference value. */
  changed(): [string, Scalar][] {
    const out: [string, Scalar][] = [];
    for (const p of this.params) {
      const r = this.referenceFor(p);
      if (r !== undefined && !same(r, p.value)) out.push([p.name, r]);
    }
    return out;
  }

  /** The reference value if it is admissible for this parameter as declared now. */
  private referenceFor(p: ParamDesc): Scalar | undefined {
    if (p.kind === 'derived') return undefined;
    const r = this.reference.get(p.name);
    if (r === undefined) return undefined;
    if (p.kind === 'choice') return typeof r === 'string' && p.choices?.some((c) => c.value === r) ? r : undefined;
    if (p.kind === 'boolean') return typeof r === 'boolean' ? r : undefined;
    if (typeof r !== 'number') return undefined;
    if ((p.min !== null && p.min !== undefined && r < p.min) || (p.max !== null && p.max !== undefined && r > p.max)) return undefined;
    return r;
  }

  private setDetailed(on: boolean): void {
    this.detailed = on;
    store.set('design.detailed', on ? '1' : '0');
    this.detailToggle.setAttribute('aria-checked', String(on));
    this.groupsEl.classList.toggle('detailed', on);
    this.applyVisibility();
  }

  /** Shows a message instead of the controls (null: back to the controls). */
  showMessage(content: (string | Node)[] | null): void {
    this.messageEl.replaceChildren(...(content ?? []));
    this.messageEl.hidden = content === null;
    this.groupsEl.hidden = content !== null;
    this.resetAll.disabled = content !== null || this.changed().length === 0;
  }

  /**
   * Shows a `parameters()` description. Rebuilds the controls only when the
   * declarations changed; otherwise updates values in place, so a control
   * being dragged is never replaced. `fresh`: the description is of the
   * current text (a reply overtaken by further edits leaves the control
   * being edited alone).
   */
  update(doc: ParamsDoc, fresh: boolean): void {
    this.doc = doc;
    const sig = JSON.stringify(doc.parameters.map(({ value: _v, default: _d, ...rest }) => rest));
    if (sig !== this.sig) {
      this.sig = sig;
      this.build(doc);
    }
    for (const p of doc.parameters) this.rows.get(p.name)?.show(p.value, fresh || p.name !== this.active);
    this.refreshChanged();
  }

  /** Emphasises the rows of these parameters (from the sketch). */
  highlight(names: Set<string>): void {
    for (const [name, r] of this.rows) r.el.classList.toggle('pglow', names.has(name));
  }

  /** Opens the parameter's section (and the detailed view if needed) and focuses its control. */
  focusParam(name: string): void {
    const r = this.rows.get(name);
    if (!r) return;
    if (r.desc.advanced && !this.detailed) this.setDetailed(true);
    const g = r.el.closest('details');
    if (g && !g.open) g.open = true;
    r.el.scrollIntoView({ block: 'nearest' });
    r.focus();
  }

  private refreshChanged(): void {
    let total = 0;
    const perGroup = new Map<HTMLDetailsElement, number>();
    for (const p of this.params) {
      const r = this.rows.get(p.name);
      if (!r) continue;
      const ref = this.referenceFor(p);
      const changed = ref !== undefined && !same(ref, p.value);
      r.setChanged(changed ? ref : undefined);
      if (changed) {
        total++;
        const g = r.el.closest('details');
        if (g) perGroup.set(g, (perGroup.get(g) ?? 0) + 1);
      }
    }
    for (const g of this.groupEls.values()) {
      const n = perGroup.get(g) ?? 0;
      const meta = g.querySelector<HTMLElement>('.pgroup-changed')!;
      meta.textContent = n ? `${n} changed` : '';
    }
    this.resetAll.disabled = total === 0 || !this.messageEl.hidden;
    this.resetAll.textContent = total ? `Reset all (${total})` : 'Reset all';
  }

  private applyVisibility(): void {
    for (const r of this.rows.values()) r.el.hidden = r.desc.advanced && !this.detailed;
    for (const g of this.groupEls.values()) {
      const rows = [...g.querySelectorAll<HTMLElement>('.prow')];
      g.hidden = rows.every((r) => r.hidden);
      const shown = rows.filter((r) => !r.hidden).length;
      g.querySelector<HTMLElement>('.pgroup-count')!.textContent = `${shown} parameter${shown === 1 ? '' : 's'}`;
    }
  }

  private build(doc: ParamsDoc): void {
    // Keep focus on the same parameter's control across a rebuild.
    const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const refocus = focused && this.groupsEl.contains(focused) ? focused.closest<HTMLElement>('[data-param]')?.dataset.param : undefined;
    this.rows.clear();
    this.groupEls.clear();
    this.groupsEl.replaceChildren();
    for (const p of doc.parameters) {
      const gname = p.group ?? 'Parameters';
      let g = this.groupEls.get(gname);
      if (!g) {
        g = document.createElement('details');
        g.className = 'pgroup';
        g.dataset.group = gname;
        g.open = this.open[gname] ?? true;
        const s = el('summary');
        s.append(el('span', 'pgroup-name', gname), el('span', 'pgroup-count'), el('span', 'pgroup-changed'));
        g.append(s);
        g.addEventListener('toggle', () => {
          this.open[gname] = g!.open;
          store.set('design.groups', JSON.stringify(this.open));
        });
        this.groupEls.set(gname, g);
        this.groupsEl.append(g);
      }
      const row = this.row(p);
      this.rows.set(p.name, row);
      g.append(row.el);
    }
    this.applyVisibility();
    if (refocus) this.rows.get(refocus)?.focus();
  }

  // ----- rows --------------------------------------------------------------

  private row(d: ParamDesc): Row {
    const id = `p-${d.name}`;
    const unit = paramUnit(d.unit);
    const root = el('div', `prow prow-${d.kind}`);
    root.dataset.param = d.name;
    if (d.advanced) root.classList.add('advanced');

    // Hover and focus link the row to the sketch.
    const notify = () => this.cb.hover(this.pointer ?? this.active);
    root.addEventListener('pointerenter', () => {
      this.pointer = d.name;
      notify();
    });
    root.addEventListener('pointerleave', () => {
      this.pointer = null;
      notify();
    });
    root.addEventListener('focusin', () => {
      this.active = d.name;
      notify();
    });
    root.addEventListener('focusout', (ev) => {
      if (root.contains(ev.relatedTarget as Node | null)) return;
      if (this.active === d.name) this.active = null;
      notify();
    });

    const head = el('div', 'prow-head');
    const labelIsFor = d.kind === 'number' || d.kind === 'integer' || (d.kind === 'choice' && (d.choices?.length ?? 0) > 3);
    const label = el(labelIsFor ? 'label' : 'span', 'plabel', d.label);
    label.id = `${id}-label`;
    if (label instanceof HTMLLabelElement) label.htmlFor = id;
    head.append(label);

    // Help text and details (detailed view: name, tolerance, expression).
    const described: string[] = [];
    const tools = el('span', 'ptools');
    let help: HTMLElement | null = null;
    if (d.description) {
      help = el('p', 'phelp', d.description);
      help.id = `${id}-help`;
      help.hidden = true;
      described.push(help.id);
      const hb = el('button', 'picon phelp-btn', '?');
      hb.type = 'button';
      hb.setAttribute('aria-label', `About ${d.label}`);
      hb.setAttribute('aria-expanded', 'false');
      hb.setAttribute('aria-controls', help.id);
      hb.addEventListener('click', () => {
        help!.hidden = !help!.hidden;
        hb.setAttribute('aria-expanded', String(!help!.hidden));
      });
      tools.append(hb);
    }
    const reset = el('button', 'picon preset-btn', '↺');
    reset.type = 'button';
    reset.hidden = true;
    let resetTo: Scalar | undefined;
    reset.addEventListener('click', () => {
      if (resetTo !== undefined) this.cb.set([[d.name, resetTo]]);
    });
    tools.append(reset);
    head.append(tools);

    const detail = el('p', 'pdetail detail-only');
    detail.id = `${id}-detail`;
    const code = el('code', 'pname', d.name);
    detail.append(code);
    if (d.kind === 'derived' && d.expr) {
      detail.append(' = ', el('code', 'pexpr', d.expr));
    }
    if (d.tolerance) {
      detail.append(el('span', 'ptol', `Tolerance ${toleranceText(d.tolerance, unit)}`));
      described.push(detail.id);
    }
    const show = el('button', 'linkish', 'Show in netlist');
    show.type = 'button';
    show.setAttribute('aria-label', `Show ${d.label} in the netlist`);
    show.addEventListener('click', () => this.cb.showInNetlist(d.name));
    detail.append(show);

    const msg = el('p', 'pmsg');
    msg.id = `${id}-msg`;
    msg.setAttribute('aria-live', 'polite');

    const choiceLabel = (v: Scalar | null) => d.choices?.find((c) => c.value === v)?.label ?? String(v);
    const valueText = (v: Scalar | null) =>
      v === null ? 'n/a' : typeof v === 'number' ? `${formatParam(v, 6)}${unit ? ` ${unit}` : ''}` : d.kind === 'choice' ? choiceLabel(v) : v ? 'on' : 'off';

    let current: Scalar | null = d.value;
    const r: Row = {
      desc: d,
      el: root,
      show: () => {},
      setChanged: (ref) => {
        resetTo = ref;
        reset.hidden = ref === undefined;
        root.classList.toggle('changed', ref !== undefined);
        if (ref !== undefined) {
          reset.setAttribute('aria-label', `Reset ${d.label} to ${valueText(ref)}`);
          reset.title = `Reset to ${valueText(ref)} (template value)`;
        }
      },
      focus: () => {
        for (const s of ['input:checked', 'input.pnum', 'input', 'select', 'button[role="switch"]', 'output']) {
          const t = root.querySelector<HTMLElement>(s);
          if (t) {
            t.focus();
            return;
          }
        }
      },
    };

    if (d.kind === 'number' || d.kind === 'integer') {
      const sc = d.kind === 'number' ? sliderScale(d) : null;
      const entry = el('input', 'pnum');
      entry.id = id;
      entry.type = 'text';
      entry.inputMode = d.kind === 'integer' ? 'numeric' : 'decimal';
      entry.autocomplete = 'off';
      entry.spellcheck = false;
      entry.setAttribute('aria-describedby', [msg.id, ...described].join(' '));
      const unitEl = el('span', 'punit', unit);
      unitEl.setAttribute('aria-hidden', 'true');
      if (unit) entry.setAttribute('aria-label', `${d.label}, ${unit}`);

      let slider: HTMLInputElement | null = null;
      let dragging = false;
      /** The entry holds typed text not yet committed or normalised. */
      let dirty = false;
      let stepper: { sync(v: number): void } | null = null;
      const setSlider = (v: number) => {
        if (!slider || !sc) return;
        slider.value = String(sc.toPos(v));
        slider.setAttribute('aria-valuetext', valueText(v));
      };
      const setInvalid = (m: string | null) => {
        msg.textContent = m ?? '';
        root.classList.toggle('invalid', m !== null);
        if (m === null) entry.removeAttribute('aria-invalid');
        else entry.setAttribute('aria-invalid', 'true');
      };
      /** Sends a valid value; `normalise` also rewrites the entry text. */
      const commit = (v: number, normalise = true) => {
        setInvalid(null);
        if (normalise) {
          entry.value = entryText(v);
          dirty = false;
        }
        setSlider(v);
        stepper?.sync(v);
        if (!same(v, current)) {
          current = v;
          this.cb.set([[d.name, v]]);
        }
      };
      let typed = 0;
      /** `final`: Enter or leaving the field; otherwise a pause while typing. */
      const commitEntry = (final: boolean) => {
        clearTimeout(typed);
        const p = parseEntry(entry.value, d);
        if (!p.ok) {
          if (final || entry.value.trim() !== '') setInvalid(p.message);
          return;
        }
        commit(p.value, final);
      };
      const keyStep = (ev: KeyboardEvent): number | null => {
        const big = ev.key === 'PageUp' || ev.key === 'PageDown';
        const k = ev.key === 'ArrowUp' || ev.key === 'ArrowRight' || ev.key === 'PageUp' ? 1 : -1;
        const base = typeof current === 'number' ? current : Number(d.default ?? 0);
        return nudge(d, sc, base, big ? 10 * k : k);
      };
      entry.addEventListener('input', () => {
        dirty = true;
        clearTimeout(typed);
        typed = window.setTimeout(() => commitEntry(false), 700);
      });
      entry.addEventListener('change', () => commitEntry(true));
      entry.addEventListener('keydown', (ev) => {
        if (ev.key === 'Enter') {
          ev.preventDefault();
          commitEntry(true);
        } else if (ev.key === 'Escape') {
          clearTimeout(typed);
          setInvalid(null);
          dirty = false;
          entry.value = typeof current === 'number' ? entryText(current) : '';
        } else if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'].includes(ev.key)) {
          ev.preventDefault();
          commit(keyStep(ev)!);
        }
      });

      const entryBox = el('span', 'pentry');
      if (d.kind === 'integer') {
        const dec = el('button', 'pstep', '−');
        const inc = el('button', 'pstep', '+');
        for (const [b, k, word] of [
          [dec, -1, 'Decrease'],
          [inc, 1, 'Increase'],
        ] as const) {
          b.type = 'button';
          b.setAttribute('aria-label', `${word} ${d.label}`);
          b.addEventListener('click', () => {
            if (b.getAttribute('aria-disabled') === 'true') return;
            const base = typeof current === 'number' ? current : Number(d.default ?? 0);
            commit(nudge(d, null, base, k));
          });
        }
        stepper = {
          sync: (v: number) => {
            dec.setAttribute('aria-disabled', String(d.min !== null && d.min !== undefined && v <= d.min));
            inc.setAttribute('aria-disabled', String(d.max !== null && d.max !== undefined && v >= d.max));
          },
        };
        entryBox.classList.add('pstepper');
        entryBox.append(dec, entry, inc);
        if (unit) entryBox.append(unitEl);
      } else {
        entryBox.append(entry, unitEl);
      }
      head.append(entryBox);
      root.append(head);

      if (sc) {
        slider = el('input', 'pslider');
        slider.type = 'range';
        slider.min = '0';
        slider.max = String(sc.n);
        slider.step = '1';
        slider.setAttribute('aria-labelledby', label.id);
        if (described.length) slider.setAttribute('aria-describedby', described.join(' '));
        slider.addEventListener('pointerdown', () => (dragging = true));
        const up = () => (dragging = false);
        slider.addEventListener('pointerup', up);
        slider.addEventListener('pointercancel', up);
        slider.addEventListener('blur', up);
        slider.addEventListener('input', () => {
          const v = quantize(d, sc.toValue(Number(slider!.value)));
          slider!.setAttribute('aria-valuetext', valueText(v));
          setInvalid(null);
          entry.value = entryText(v);
          dirty = false;
          if (!same(v, current)) {
            current = v;
            this.cb.set([[d.name, v]]);
          }
        });
        slider.addEventListener('keydown', (ev) => {
          let v: number | null = null;
          if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown'].includes(ev.key)) v = keyStep(ev);
          else if (ev.key === 'Home') v = d.min!;
          else if (ev.key === 'End') v = d.max!;
          if (v === null) return;
          ev.preventDefault();
          commit(v);
        });
        root.append(slider);
      }
      r.show = (v, force) => {
        if (typeof v !== 'number' || !force) return;
        current = v;
        // Never overwrite what the user is typing.
        if (!(dirty && document.activeElement === entry)) {
          entry.value = entryText(v);
          dirty = false;
          setInvalid(null);
        }
        if (!dragging) setSlider(v);
        stepper?.sync(v);
      };
    } else if (d.kind === 'boolean') {
      const sw = el('button', 'pswitch');
      sw.type = 'button';
      sw.id = id;
      sw.setAttribute('role', 'switch');
      sw.setAttribute('aria-labelledby', label.id);
      if (described.length) sw.setAttribute('aria-describedby', described.join(' '));
      const state = el('span', 'pswitch-state');
      sw.append(el('span', 'pswitch-track'), state);
      sw.addEventListener('click', () => {
        current = !(current === true);
        r.show(current, true);
        this.cb.set([[d.name, current]]);
      });
      head.append(sw);
      root.append(head);
      r.show = (v) => {
        current = v === true;
        sw.setAttribute('aria-checked', String(current));
        state.textContent = current ? 'On' : 'Off';
      };
    } else if (d.kind === 'choice' && (d.choices?.length ?? 0) <= 3) {
      root.append(head);
      const seg = el('div', 'segmented');
      // Long option labels stack instead of wrapping into a ragged row.
      if ((d.choices ?? []).reduce((n, c) => n + (c.label ?? c.value).length, 0) > 42) seg.classList.add('stacked');
      seg.setAttribute('role', 'radiogroup');
      seg.setAttribute('aria-labelledby', label.id);
      if (described.length) seg.setAttribute('aria-describedby', described.join(' '));
      const radios: HTMLInputElement[] = [];
      for (const c of d.choices ?? []) {
        const l = el('label', 'seg');
        const input = el('input');
        input.type = 'radio';
        input.name = id;
        input.value = c.value;
        input.addEventListener('change', () => {
          if (!input.checked) return;
          current = c.value;
          this.cb.set([[d.name, c.value]]);
        });
        radios.push(input);
        l.append(input, el('span', undefined, c.label ?? c.value));
        seg.append(l);
      }
      root.append(seg);
      r.show = (v) => {
        current = v;
        for (const x of radios) x.checked = x.value === v;
      };
    } else if (d.kind === 'choice') {
      const sel = el('select', 'pselect');
      sel.id = id;
      if (described.length) sel.setAttribute('aria-describedby', described.join(' '));
      for (const c of d.choices ?? []) {
        const o = el('option', undefined, c.label ?? c.value);
        o.value = c.value;
        sel.append(o);
      }
      sel.addEventListener('change', () => {
        current = sel.value;
        this.cb.set([[d.name, sel.value]]);
      });
      head.append(sel);
      root.append(head);
      r.show = (v) => {
        current = v;
        sel.value = String(v);
      };
    } else {
      // Derived: read-only, with the expression as a tooltip (and as text in
      // the detailed view).
      const out = el('output', 'pout');
      out.id = id;
      out.setAttribute('aria-labelledby', label.id);
      if (d.expr) out.title = `= ${d.expr}`;
      out.tabIndex = -1;
      head.append(out);
      root.append(head);
      r.show = (v) => {
        current = v;
        out.textContent = typeof v === 'number' ? `${formatParam(v, 4)}${unit ? ` ${unit}` : ''}` : valueText(v);
      };
    }
    root.append(msg);
    if (help) root.append(help);
    root.append(detail);
    r.show(d.value, true);
    return r;
  }
}
