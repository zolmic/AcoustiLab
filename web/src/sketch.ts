// Parametric cross-section (spec Section 15, "Parametric cross-section"):
// an SVG section through the cup axis, drawn to scale from the resolved
// parameter values of the netlist. The netlist's `ui.sketch` block binds
// the sketch's slots to parameter names (docs/web.md, "The ui block"); no
// dimension is invented here, and a slot left unbound is simply not drawn.
//
// Two things are deliberately not to scale, and both say so on screen: the
// leak gap (tenths of a millimetre; drawn a few pixels high with its true
// value labelled) and the positions of the vents along the section.

import { formatParam } from './format';
import type { ParamDesc, Scalar, SketchSpec } from './types';

export type Part = 'front' | 'driver' | 'pad' | 'leak' | 'rear' | 'shell' | 'vents' | 'grille' | 'ear';

/** Slots of the `over_ear` sketch and the parts each one shapes. */
export const OVER_EAR_SLOTS: Record<string, Part[]> = {
  cup_radius_mm: ['front', 'rear', 'pad', 'shell'],
  front_depth_mm: ['front', 'pad'],
  front_volume_cm3: ['front'],
  driver_diameter_mm: ['driver'],
  pad_width_mm: ['pad'],
  leak_gap_mm: ['leak'],
  open_back: ['rear', 'shell', 'vents', 'grille'],
  rear_depth_mm: ['rear', 'shell'],
  rear_volume_cm3: ['rear'],
  vent_count: ['vents'],
  vent_diameter_mm: ['vents'],
  vent_length_mm: ['vents', 'shell'],
  vent_mesh_rayl: ['vents'],
  grille_rayl: ['grille'],
};

const PART_NAMES: Record<Part, string> = {
  front: 'front cavity',
  driver: 'driver',
  pad: 'pad',
  leak: 'leak gap',
  rear: 'rear cavity',
  shell: 'cup shell',
  vents: 'rear vents',
  grille: 'grille',
  ear: 'ear load',
};

/** Geometry in millimetres (and the labels) read from the bound parameters. */
interface Geo {
  R: number;
  D: number;
  Vf?: number;
  d?: number;
  W?: number;
  gap?: number;
  open: boolean;
  Dr?: number;
  Vr?: number;
  n?: number;
  dv?: number;
  t?: number;
  mesh?: number;
  grille?: number;
  ear?: string;
}

export interface SketchCallbacks {
  hoverPart(part: Part | null): void;
  clickPart(part: Part): void;
}

const SVG = 'http://www.w3.org/2000/svg';

function node(tag: string, attrs: Record<string, string | number>, text?: string): SVGElement {
  const e = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  if (text !== undefined) e.textContent = text;
  return e;
}

const mm = (v: number) => `${formatParam(v, 3)} mm`;

/** Parameter names an expression refers to (identifiers that are parameters). */
function refs(expr: string, names: Set<string>): string[] {
  return (expr.match(/[A-Za-z_][A-Za-z0-9_]*/g) ?? []).filter((w) => names.has(w));
}

export class Sketch {
  private spec: SketchSpec | null = null;
  private earParam: string | undefined;
  /** Parameter -> every parameter it depends on through derived expressions, itself included. */
  private deps = new Map<string, Set<string>>();
  private values = new Map<string, Scalar>();
  private choiceLabels = new Map<string, Map<string, string>>();
  private reference: Geo | null = null;
  private glow = new Set<Part>();
  private readonly svg: SVGSVGElement;
  private width = 0;

  constructor(
    private readonly figure: HTMLElement,
    private readonly desc: HTMLElement,
    private readonly note: HTMLElement,
    private readonly cb: SketchCallbacks,
  ) {
    this.svg = figure.querySelector('svg')!;
    new ResizeObserver(() => {
      const w = Math.round(this.svg.getBoundingClientRect().width);
      if (w && w !== this.width) {
        this.width = w;
        this.render();
      }
    }).observe(this.svg);
    this.svg.addEventListener('pointerover', (ev) => {
      const p = (ev.target as Element).closest<SVGElement>('[data-part]')?.dataset.part as Part | undefined;
      this.cb.hoverPart(p ?? null);
    });
    this.svg.addEventListener('pointerleave', () => this.cb.hoverPart(null));
    this.svg.addEventListener('click', (ev) => {
      const p = (ev.target as Element).closest<SVGElement>('[data-part]')?.dataset.part as Part | undefined;
      if (p) this.cb.clickPart(p);
    });
  }

  /**
   * Binds the sketch to a netlist's parameters. Returns false (and hides
   * the figure) when the netlist has no sketch or one of an unknown kind.
   */
  configure(spec: SketchSpec | undefined, earParam: string | undefined, params: ParamDesc[]): boolean {
    this.spec = null;
    this.earParam = earParam;
    this.note.hidden = true;
    const names = new Set(params.map((p) => p.name));
    if (spec && spec.kind !== 'over_ear') {
      this.note.textContent = `This netlist asks for a "${spec.kind}" sketch, which this interface does not draw.`;
      this.note.hidden = false;
    } else if (spec && typeof spec.bind === 'object' && spec.bind) {
      const missing = ['cup_radius_mm', 'front_depth_mm'].filter((s) => !names.has(spec.bind[s]));
      const unknown = Object.keys(spec.bind).filter((s) => !(s in OVER_EAR_SLOTS));
      if (missing.length || unknown.length) {
        this.note.textContent =
          'The sketch binding is incomplete: ' +
          [missing.length ? `needs ${missing.join(', ')}` : '', unknown.length ? `unknown slots ${unknown.join(', ')}` : '']
            .filter(Boolean)
            .join('; ') +
          ' (docs/web.md).';
        this.note.hidden = false;
      } else {
        this.spec = spec;
      }
    }
    // Dependencies through derived parameters, transitively.
    const direct = new Map<string, string[]>();
    for (const p of params) direct.set(p.name, p.kind === 'derived' && p.expr ? refs(p.expr, names) : []);
    this.deps.clear();
    for (const p of params) {
      const seen = new Set<string>([p.name]);
      const stack = [...(direct.get(p.name) ?? [])];
      while (stack.length) {
        const q = stack.pop()!;
        if (seen.has(q)) continue;
        seen.add(q);
        stack.push(...(direct.get(q) ?? []));
      }
      this.deps.set(p.name, seen);
    }
    this.choiceLabels.clear();
    for (const p of params) {
      if (p.choices) this.choiceLabels.set(p.name, new Map(p.choices.map((c) => [c.value, c.label ?? c.value])));
    }
    this.figure.hidden = this.spec === null;
    return this.spec !== null;
  }

  /** Parts driven by parameter `name` (directly or through derived parameters). */
  partsOf(name: string): Set<Part> {
    const out = new Set<Part>();
    if (!this.spec) return out;
    const drives = (target: string) => this.deps.get(target)?.has(name) ?? target === name;
    for (const [slot, param] of Object.entries(this.spec.bind)) {
      if (drives(param)) for (const p of OVER_EAR_SLOTS[slot] ?? []) out.add(p);
    }
    for (const [part, list] of Object.entries(this.spec.parts ?? {})) {
      if (list.some(drives)) out.add(part as Part);
    }
    if (this.earParam && drives(this.earParam)) out.add('ear');
    return out;
  }

  /** Parameters that drive a part. */
  paramsOf(part: Part): Set<string> {
    const out = new Set<string>();
    for (const name of this.deps.keys()) if (this.partsOf(name).has(part)) out.add(name);
    return out;
  }

  /** Glows the parts driven by `name` (null: none). */
  highlightParam(name: string | null): void {
    this.highlightParts(name ? this.partsOf(name) : new Set());
  }

  highlightParts(parts: Set<Part>): void {
    this.glow = parts;
    this.svg.classList.toggle('has-glow', parts.size > 0);
    for (const g of this.svg.querySelectorAll<SVGElement>('[data-part]')) {
      g.classList.toggle('glow', parts.has(g.dataset.part as Part));
    }
  }

  /** Values of the template as loaded: the drawing scale covers them, so it stays put while you edit. */
  setReference(values: Map<string, Scalar> | null): void {
    this.reference = values ? this.geometry(values) : null;
  }

  update(values: Map<string, Scalar>): void {
    this.values = values;
    this.render();
  }

  private geometry(values: Map<string, Scalar>): Geo | null {
    if (!this.spec) return null;
    const b = this.spec.bind;
    const num = (slot: string) => {
      const v = b[slot] ? values.get(b[slot]) : undefined;
      return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
    };
    const R = num('cup_radius_mm');
    const D = num('front_depth_mm');
    if (R === undefined || D === undefined || R <= 0 || D <= 0) return null;
    const openV = b.open_back ? values.get(b.open_back) : undefined;
    let ear: string | undefined;
    if (this.earParam) {
      const v = values.get(this.earParam);
      if (v !== undefined) ear = this.choiceLabels.get(this.earParam)?.get(String(v)) ?? String(v);
    }
    return {
      R,
      D,
      Vf: num('front_volume_cm3'),
      d: num('driver_diameter_mm'),
      W: num('pad_width_mm'),
      gap: num('leak_gap_mm'),
      open: openV === true,
      Dr: num('rear_depth_mm'),
      Vr: num('rear_volume_cm3'),
      n: num('vent_count'),
      dv: num('vent_diameter_mm'),
      t: num('vent_length_mm'),
      mesh: num('vent_mesh_rayl'),
      grille: num('grille_rayl'),
      ear,
    };
  }

  /** Half-width and height of the drawn structure, in mm. */
  private static extents(g: Geo): [number, number] {
    const t = g.t ?? 0;
    const x = Math.max(g.R + Math.max(g.W ?? 0, t), (g.d ?? 0) / 2);
    const y = g.D + (g.open ? 0 : (g.Dr ?? 0) + t);
    return [x, y];
  }

  /** Text alternative: every dimension the drawing shows. */
  private describe(g: Geo): string {
    const s: string[] = [
      `Cross-section through the cup axis, to scale. Front cavity: radius ${mm(g.R)}, depth ${mm(g.D)}` +
        (g.Vf !== undefined ? `, volume ${formatParam(g.Vf, 3)} cm³.` : '.'),
    ];
    if (g.d !== undefined) s.push(`Driver: diaphragm diameter ${mm(g.d)}.`);
    if (g.W !== undefined || g.gap !== undefined) {
      s.push(
        `Pad: ${g.W !== undefined ? `${mm(g.W)} wide` : 'width not bound'}` +
          (g.gap === undefined ? '.' : g.gap > 0 ? `, leak gap ${mm(g.gap)} (drawn enlarged).` : ', sealed (no leak gap).'),
      );
    }
    if (g.open) {
      s.push(`Rear: open back${g.grille !== undefined ? ` behind a ${formatParam(g.grille, 3)} rayl grille` : ''}.`);
    } else if (g.Dr !== undefined) {
      let r = `Rear: closed cavity ${mm(g.Dr)} deep${g.Vr !== undefined ? ` (${formatParam(g.Vr, 3)} cm³)` : ''}`;
      if (g.n !== undefined) {
        r +=
          g.n > 0
            ? ` with ${g.n} vent${g.n === 1 ? '' : 's'}${g.dv !== undefined ? ` of ${mm(g.dv)} diameter` : ''}` +
              `${g.t !== undefined ? ` through a ${mm(g.t)} wall` : ''}` +
              (g.mesh !== undefined ? (g.mesh > 0 ? `, each under a ${formatParam(g.mesh, 3)} rayl mesh` : ', open holes') : '') +
              ' (vent positions schematic)'
            : ', no vents';
      }
      s.push(`${r}.`);
    }
    if (g.ear) s.push(`Ear load: ${g.ear}.`);
    return s.join(' ');
  }

  private render(): void {
    const svg = this.svg;
    svg.replaceChildren();
    const g = this.geometry(this.values);
    this.desc.textContent = g ? this.describe(g) : '';
    if (!g || !this.width) return;

    // The height depends on the width only, so the controls below never
    // move while a value is dragged.
    const Wp = this.width;
    const Hp = Math.round(Math.min(250, Math.max(200, Wp * 0.5)));
    svg.setAttribute('viewBox', `0 0 ${Wp} ${Hp}`);
    svg.setAttribute('height', String(Hp));
    // Margins: dimension labels at the left, room for the vent or grille
    // label above and the ear-load and leak labels below.
    const mL = 78;
    const mR = 14;
    const mT = 40;
    const mB = 46;
    const [xc, yc] = Sketch.extents(g);
    const [xr, yr] = this.reference ? Sketch.extents(this.reference) : [0, 0];
    const X = Math.max(xc, xr);
    const Y = Math.max(yc, yr, 1);
    const topReserve = g.open ? 36 : 0;
    const s = Math.min((Wp - mL - mR) / (2 * X), (Hp - mT - mB - topReserve) / Y);
    const ox = mL + (Wp - mL - mR) / 2;
    const oy = Hp - mB;
    const px = (x: number) => ox + x * s;
    const py = (y: number) => oy - y * s;

    const defs = node('defs', {});
    const hatch = node('pattern', { id: 'sk-hatch', width: 6, height: 6, patternUnits: 'userSpaceOnUse', patternTransform: 'rotate(45)' });
    hatch.append(node('line', { x1: 0, y1: 0, x2: 0, y2: 6, class: 'sk-hatch-line' }));
    const meshP = node('pattern', { id: 'sk-meshp', width: 3, height: 3, patternUnits: 'userSpaceOnUse', patternTransform: 'rotate(45)' });
    meshP.append(node('line', { x1: 0, y1: 0, x2: 0, y2: 3, class: 'sk-mesh-line' }));
    defs.append(hatch, meshP);
    // Shapes below, dimensions and labels above; both layers carry the part
    // they belong to, so highlighting covers a part's labels too.
    const shapes = node('g', {});
    const labels = node('g', {});
    svg.append(defs, shapes, labels);
    const part = (p: Part) => {
      const e = node('g', { 'data-part': p, class: 'sk-part' });
      e.append(node('title', {}, PART_NAMES[p]));
      shapes.append(e);
      return e;
    };
    const lab = (p: Part | null) => {
      const e = node('g', p ? { 'data-part': p, class: 'sk-part' } : {});
      labels.append(e);
      return e;
    };
    const text = (x: number, y: number, t: string, anchor = 'middle') =>
      node('text', { x, y, 'text-anchor': anchor, class: 'sk-label' }, t);
    /** Dimension line with end ticks. */
    const dim = (x1: number, y1: number, x2: number, y2: number) => {
      const e = node('g', { class: 'sk-dim' });
      e.append(node('line', { x1, y1, x2, y2 }));
      const vertical = Math.abs(x2 - x1) < Math.abs(y2 - y1);
      for (const [x, y] of [
        [x1, y1],
        [x2, y2],
      ]) {
        e.append(vertical ? node('line', { x1: x - 4, y1: y, x2: x + 4, y2: y }) : node('line', { x1: x, y1: y - 4, x2: x, y2: y + 4 }));
      }
      return e;
    };

    const t = g.t ?? 0;
    const W = g.W ?? 0;
    const dia = g.d ?? 0;
    const outer = g.R + Math.max(W, t);
    const leftDimX = px(-Math.max(outer, dia / 2)) - 12;
    const closed = !g.open && g.Dr !== undefined;
    const Dr = closed ? g.Dr! : 0;
    const tw = Math.max(t * s, 1.5);
    const top = py(g.D + Dr);

    // Ear load: the head or fixture surface the pad rests on.
    const x0 = px(-X) - 28;
    const x1 = px(X) + 28;
    part('ear').append(
      node('rect', { x: x0, y: oy, width: x1 - x0, height: 9, class: 'sk-head' }),
      node('line', { x1: x0, y1: oy, x2: x1, y2: oy, class: 'sk-surface' }),
      node('circle', { cx: ox, cy: oy, r: 3.5, class: 'sk-entrance' }),
    );
    lab('ear').append(text(ox, oy + 23, g.ear ? `Ear load: ${g.ear}` : 'Head or fixture surface'));

    // Air: front cavity, and the rear cavity when the back is closed.
    part('front').append(node('rect', { x: px(-g.R), y: py(g.D), width: 2 * g.R * s, height: g.D * s, class: 'sk-air' }));
    if (closed) part('rear').append(node('rect', { x: px(-g.R), y: top, width: 2 * g.R * s, height: Dr * s, class: 'sk-air' }));

    // Pad, with the leak gap under it enlarged to a few pixels.
    const gapPx = g.gap === undefined || g.gap <= 0 ? 0 : Math.min(10, 3 + 25 * g.gap);
    const gapMm = gapPx / s;
    if (g.W !== undefined) {
      const pad = part('pad');
      for (const xa of [px(-g.R - W), px(g.R)]) {
        pad.append(node('rect', { x: xa, y: py(g.D), width: W * s, height: (g.D - gapMm) * s, rx: 2, class: 'sk-pad' }));
      }
      const yp = py(gapMm + (g.D - gapMm) * 0.32);
      if (W * s >= 44) {
        lab('pad').append(dim(px(g.R), yp, px(g.R + W), yp), text((px(g.R) + px(g.R + W)) / 2, yp - 5, mm(W)));
      } else {
        lab('pad').append(text(px(g.R + W) + 4, yp + 4, mm(W), 'start'));
      }
    }
    if (g.gap !== undefined) {
      const leak = part('leak');
      if (gapPx > 0) {
        for (const xa of [px(-g.R - W), px(g.R)]) {
          leak.append(node('rect', { x: xa, y: oy - gapPx, width: Math.max(W * s, 6), height: gapPx, class: 'sk-leak' }));
        }
      }
      const xa = px(g.R + W / 2);
      const ly = oy + 39;
      lab('leak').append(
        node('polyline', { points: `${xa},${oy - gapPx / 2} ${xa + 10},${ly - 12} ${Wp - 6},${ly - 12}`, class: 'sk-leader' }),
        text(Wp - 6, ly, gapPx > 0 ? `leak gap ${mm(g.gap)} (drawn enlarged)` : 'pad sealed: no leak gap', 'end'),
      );
    }

    // Cup shell (side walls and a top plate as thick as the vents are
    // long) and the baffle the driver sits in.
    let holes: [number, number][] = [];
    const n = Math.max(0, Math.round(g.n ?? 0));
    if (closed) {
      const shell = part('shell');
      for (const xa of [px(-g.R) - tw, px(g.R)]) {
        shell.append(node('rect', { x: xa, y: top - tw, width: tw, height: Dr * s + tw, class: 'sk-solid' }));
      }
      const dv = g.dv ?? 0;
      for (let k = 0; k < n; k++) {
        const xk = -g.R + ((k + 1) * 2 * g.R) / (n + 1);
        holes.push([px(xk - dv / 2), px(xk + dv / 2)]);
      }
      let xa = px(-g.R);
      for (const [a, b] of holes) {
        if (a > xa) shell.append(node('rect', { x: xa, y: top - tw, width: a - xa, height: tw, class: 'sk-solid' }));
        xa = Math.max(xa, b);
      }
      if (px(g.R) > xa) shell.append(node('rect', { x: xa, y: top - tw, width: px(g.R) - xa, height: tw, class: 'sk-solid' }));
    } else {
      holes = [];
    }
    const baffle = closed ? part('shell') : shapes;
    baffle.append(
      node('line', { x1: px(-outer), y1: py(g.D), x2: px(-dia / 2), y2: py(g.D), class: 'sk-baffle' }),
      node('line', { x1: px(dia / 2), y1: py(g.D), x2: px(outer), y2: py(g.D), class: 'sk-baffle' }),
    );

    // Driver: the diaphragm spans its effective diameter; the dome is a symbol.
    if (g.d !== undefined) {
      const dome = Math.min(0.1 * g.d, 0.35 * g.D);
      part('driver').append(
        node('path', { d: `M ${px(-g.d / 2)} ${py(g.D)} Q ${ox} ${py(g.D - 2 * dome)} ${px(g.d / 2)} ${py(g.D)}`, class: 'sk-diaphragm' }),
      );
      const yd = py(g.D) - 9;
      if (closed ? Dr * s >= 30 : true) {
        lab('driver').append(dim(px(-g.d / 2), yd, px(g.d / 2), yd), text(ox, yd - 5, `driver Ø ${mm(g.d)}`));
      } else {
        lab('driver').append(text(px(g.d / 2) + 4, py(g.D) + 14, `Ø ${mm(g.d)}`, 'start'));
      }
    }

    // Rear: vents through the top plate, or the open grille.
    if (closed) {
      const vents = part('vents');
      for (const [a, b] of holes) {
        vents.append(node('rect', { x: a, y: top - tw, width: Math.max(b - a, 1), height: tw, class: 'sk-hole' }));
        if ((g.mesh ?? 0) > 0) {
          vents.append(node('rect', { x: a - 1, y: top - tw - 4, width: Math.max(b - a, 1) + 2, height: 4, class: 'sk-mesh' }));
        }
      }
      let vl = n === 0 ? 'no vents' : `${n} vent${n === 1 ? '' : 's'}`;
      if (n > 0 && g.dv !== undefined) vl += ` Ø ${mm(g.dv)}`;
      if (n > 0 && g.t !== undefined) vl += `, ${mm(g.t)} long`;
      if (n > 0 && g.mesh !== undefined) vl += g.mesh > 0 ? `, mesh ${formatParam(g.mesh, 3)} rayl` : ', open holes';
      lab('vents').append(text(ox, top - tw - 9, vl));
      const rl = lab('rear');
      if (g.Vr !== undefined && Dr * s >= 34) rl.append(text(px(-g.R) + 5, top + 14, `${formatParam(g.Vr, 3)} cm³`, 'start'));
      rl.append(dim(leftDimX, py(g.D), leftDimX, top), text(leftDimX - 6, py(g.D + Dr / 2) + 4, mm(Dr), 'end'));
    } else if (g.open) {
      const half = Math.max(dia / 2, g.R * 0.6);
      const yg = py(g.D) - 30;
      part('grille').append(node('line', { x1: ox - half * s, y1: yg, x2: ox + half * s, y2: yg, class: 'sk-grille' }));
      lab('grille').append(
        text(ox, yg - 10, `open back: grille${g.grille !== undefined ? ` ${formatParam(g.grille, 3)} rayl` : ''}, to the room`),
      );
    }

    // Front cavity dimensions: radius in the right half, volume in the left.
    const ym = py(g.D / 2);
    const fl = lab('front');
    fl.append(dim(ox, ym, px(g.R), ym), text((ox + px(g.R)) / 2, ym - 5, `r ${mm(g.R)}`));
    if (g.Vf !== undefined) fl.append(text((px(-g.R) + ox) / 2, ym + 4, `${formatParam(g.Vf, 3)} cm³`));
    fl.append(dim(leftDimX, py(0), leftDimX, py(g.D)), text(leftDimX - 6, ym + 4, mm(g.D), 'end'));

    // Axis and scale bar.
    shapes.append(node('line', { x1: ox, y1: oy + 6, x2: ox, y2: py(yc) - 6 - topReserve, class: 'sk-axis' }));
    const L = [1, 2, 5, 10, 20, 50].find((l) => l * s >= 36) ?? 50;
    const sb = lab(null);
    sb.append(dim(8, 14, 8 + L * s, 14), text(8 + L * s + 6, 18, `${L} mm`, 'start'));
    this.highlightParts(this.glow);
  }
}
