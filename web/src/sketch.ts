// Parametric cross-section (spec Section 15, "Parametric cross-section"):
// an SVG section through the device, drawn to scale from the resolved
// parameter values of the netlist. The netlist's `ui.sketch` block names
// the drawing (`kind`) and binds its slots to parameter names (docs/web.md,
// "The ui block"); no dimension is invented here, and a slot left unbound
// is simply not drawn.
//
// Kinds: `over_ear` and `on_ear` (a section through the cup axis, the head
// at the bottom) and `in_ear` (a section along the earphone's axis, the ear
// on the right). What each draws without its dimension is listed in its
// caption, and the text alternative says so too.

import { formatParam } from './format';
import type { ParamDesc, Scalar, SketchSpec } from './types';

export type Part =
  | 'front'
  | 'driver'
  | 'pad'
  | 'leak'
  | 'rear'
  | 'shell'
  | 'vents'
  | 'grille'
  | 'ear'
  | 'damping'
  | 'pinna'
  | 'nozzle'
  | 'tip';

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
  damping_rayl: ['damping'],
};

/** Slots of the `on_ear` sketch: the over-ear cup on a compressed pinna. */
export const ON_EAR_SLOTS: Record<string, Part[]> = {
  pad_inner_radius_mm: ['front', 'pad'],
  front_depth_mm: ['front', 'pad'],
  front_volume_cm3: ['front'],
  concha_volume_cm3: ['pinna', 'front'],
  driver_diameter_mm: ['driver'],
  pad_width_mm: ['pad'],
  fit: ['leak'],
  leak_gap_mm: ['leak'],
  open_back: ['rear', 'shell', 'vents', 'grille'],
  cup_radius_mm: ['rear', 'shell'],
  rear_depth_mm: ['rear', 'shell'],
  rear_volume_cm3: ['rear'],
  vent_count: ['vents'],
  vent_diameter_mm: ['vents'],
  vent_length_mm: ['vents', 'shell'],
  vent_mesh_rayl: ['vents'],
  grille_rayl: ['grille'],
  damping_rayl: ['damping'],
};

/** Slots of the `in_ear` sketch, along the earphone's axis. */
export const IN_EAR_SLOTS: Record<string, Part[]> = {
  driver_diameter_mm: ['driver', 'front', 'rear', 'shell'],
  front_depth_mm: ['front'],
  front_volume_cm3: ['front'],
  nozzle_diameter_mm: ['nozzle', 'tip'],
  nozzle_length_mm: ['nozzle'],
  nozzle_mesh_rayl: ['nozzle'],
  canal_diameter_mm: ['ear', 'tip'],
  back_depth_mm: ['damping'],
  damping_rayl: ['damping'],
  rear_depth_mm: ['rear', 'shell'],
  rear_volume_cm3: ['rear'],
  vent_count: ['vents'],
  vent_diameter_mm: ['vents'],
  vent_length_mm: ['vents', 'shell'],
  vent_mesh_rayl: ['vents'],
  fit: ['leak'],
  leak_diameter_mm: ['leak'],
  leak_length_mm: ['leak'],
};

const PART_NAMES: Record<Part, string> = {
  front: 'front cavity',
  driver: 'driver',
  pad: 'pad',
  leak: 'leak',
  rear: 'rear cavity',
  shell: 'shell',
  vents: 'vents',
  grille: 'grille',
  ear: 'ear load',
  damping: 'damping cloth',
  pinna: 'pinna and concha',
  nozzle: 'nozzle',
  tip: 'ear tip',
};

interface KindDef {
  slots: Record<string, Part[]>;
  required: string[];
  /** Parts drawn without their dimensions: the caption's note. */
  caption: string;
}

const KINDS: Record<string, KindDef> = {
  over_ear: {
    slots: OVER_EAR_SLOTS,
    required: ['cup_radius_mm', 'front_depth_mm'],
    caption:
      '(leak gap enlarged; vent positions, dome shape, side walls, damping cloth and grille position schematic; point at a control to see what it shapes)',
  },
  on_ear: {
    slots: ON_EAR_SLOTS,
    required: ['pad_inner_radius_mm', 'front_depth_mm'],
    caption:
      '(leak gap enlarged and its position schematic; pinna, concha, vent positions, dome shape, side walls, damping cloth and grille position schematic; point at a control to see what it shapes)',
  },
  in_ear: {
    slots: IN_EAR_SLOTS,
    required: ['driver_diameter_mm', 'front_depth_mm', 'nozzle_diameter_mm', 'nozzle_length_mm'],
    caption:
      '(shell and nozzle walls, ear tip, canal length, vent positions, dome shape and damping cloth thickness schematic; point at a control to see what it shapes)',
  },
};

/** Names of the sketch kinds this interface draws. */
export const SKETCH_KINDS = Object.keys(KINDS);

/** Geometry of the cup kinds, in millimetres (and the labels). */
interface Cup {
  kind: 'over_ear' | 'on_ear';
  /** Radius of the front cavity: the cup's inner radius, or the pad's opening on the pinna. */
  R: number;
  D: number;
  Vf?: number;
  concha?: number;
  d?: number;
  W?: number;
  gap?: number;
  /** Label of the fit state (on-ear leak). */
  fit?: string;
  open: boolean;
  /** Radius of the rear cavity (the front radius when not bound separately). */
  Rr: number;
  Dr?: number;
  Vr?: number;
  n?: number;
  dv?: number;
  t?: number;
  mesh?: number;
  grille?: number;
  damping?: number;
  ear?: string;
}

/** Geometry of the in-ear kind, in millimetres. */
interface Iem {
  kind: 'in_ear';
  d: number;
  Df: number;
  Vf?: number;
  dn: number;
  Ln: number;
  nmesh?: number;
  dc?: number;
  Db?: number;
  damping?: number;
  Dr?: number;
  Vr?: number;
  n?: number;
  dv?: number;
  t?: number;
  vmesh?: number;
  fit?: string;
  /** Leak tube diameter (0: sealed). */
  dl?: number;
  Ll?: number;
  ear?: string;
}

type Geo = Cup | Iem;

/** Compressed pinna under an on-ear pad: drawn this thick, not to scale (mm). */
const PINNA_MM = 3;
/** Schematic sizes of the in-ear drawing (mm): shell wall, nozzle wall, ear tip length, canal shown. */
const IEM_WALL = 1;
const IEM_NOZZLE_WALL = 0.6;
const IEM_TIP = 5;
const IEM_CANAL = 9;

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
const cm3 = (v: number) => `${formatParam(v, 3)} cm³`;
const rayl = (v: number) => `${formatParam(v, 3)} rayl`;

/** Parameter names an expression refers to (identifiers that are parameters). */
function refs(expr: string, names: Set<string>): string[] {
  return (expr.match(/[A-Za-z_][A-Za-z0-9_]*/g) ?? []).filter((w) => names.has(w));
}

/** Drawing helpers shared by the kinds: layers, parts, labels and dimension lines. */
class Canvas {
  readonly shapes: SVGElement;
  readonly labels: SVGElement;

  constructor(
    readonly svg: SVGSVGElement,
    readonly Wp: number,
    readonly Hp: number,
  ) {
    const defs = node('defs', {});
    const hatch = node('pattern', { id: 'sk-hatch', width: 6, height: 6, patternUnits: 'userSpaceOnUse', patternTransform: 'rotate(45)' });
    hatch.append(node('line', { x1: 0, y1: 0, x2: 0, y2: 6, class: 'sk-hatch-line' }));
    const meshP = node('pattern', { id: 'sk-meshp', width: 3, height: 3, patternUnits: 'userSpaceOnUse', patternTransform: 'rotate(45)' });
    meshP.append(node('line', { x1: 0, y1: 0, x2: 0, y2: 3, class: 'sk-mesh-line' }));
    defs.append(hatch, meshP);
    // Shapes below, dimensions and labels above; both layers carry the part
    // they belong to, so highlighting covers a part's labels too.
    this.shapes = node('g', {});
    this.labels = node('g', {});
    svg.append(defs, this.shapes, this.labels);
  }

  part(p: Part): SVGElement {
    const e = node('g', { 'data-part': p, class: 'sk-part' });
    e.append(node('title', {}, PART_NAMES[p]));
    this.shapes.append(e);
    return e;
  }

  lab(p: Part | null): SVGElement {
    const e = node('g', p ? { 'data-part': p, class: 'sk-part' } : {});
    this.labels.append(e);
    return e;
  }

  readonly text = (x: number, y: number, t: string, anchor = 'middle'): SVGElement =>
    node('text', { x, y, 'text-anchor': anchor, class: 'sk-label' }, t);

  /** Dimension line with end ticks. */
  readonly dim = (x1: number, y1: number, x2: number, y2: number): SVGElement => {
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

  /**
   * Keeps an appended label inside the drawing: a label that would run past
   * an edge is moved to end 4 px inside it. The width is taken 10 % wide,
   * for the bold weight of a highlighted part.
   */
  keepInside(el: SVGElement): void {
    const box = (el as SVGGraphicsElement).getBBox();
    if (!box.width) return;
    const x = Number(el.getAttribute('x'));
    const anchor = el.getAttribute('text-anchor');
    const w = box.width * 1.1;
    const left = anchor === 'end' ? box.x + box.width - w : anchor === 'middle' ? box.x - (w - box.width) / 2 : box.x;
    // Too long for the drawing: its start stays visible.
    if (w > this.Wp - 8 || left < 4) el.setAttribute('x', String(x + 4 - left));
    else if (left + w > this.Wp - 4) el.setAttribute('x', String(x - (left + w - (this.Wp - 4))));
  }

  /** Scale bar at (x, y): the first round length at least 36 px long. */
  scaleBar(x: number, y: number, s: number): void {
    const L = [0.5, 1, 2, 5, 10, 20, 50].find((l) => l * s >= 36) ?? 50;
    this.lab(null).append(this.dim(x, y, x + L * s, y), this.text(x + L * s + 6, y + 4, `${L} mm`, 'start'));
  }
}

export class Sketch {
  private spec: SketchSpec | null = null;
  private kind: KindDef | null = null;
  private earParam: string | undefined;
  /** Parameter -> every parameter it depends on through derived expressions, itself included. */
  private deps = new Map<string, Set<string>>();
  private values = new Map<string, Scalar>();
  private choiceLabels = new Map<string, Map<string, string>>();
  private reference: Geo | null = null;
  private glow = new Set<Part>();
  private readonly svg: SVGSVGElement;
  private readonly hint: HTMLElement | null;
  private readonly defaultHint: string;
  private width = 0;

  constructor(
    private readonly figure: HTMLElement,
    private readonly desc: HTMLElement,
    private readonly note: HTMLElement,
    private readonly cb: SketchCallbacks,
  ) {
    this.svg = figure.querySelector('svg')!;
    this.hint = figure.querySelector<HTMLElement>('figcaption .hint');
    this.defaultHint = this.hint?.textContent ?? '';
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
    this.kind = null;
    this.earParam = earParam;
    this.note.hidden = true;
    const names = new Set(params.map((p) => p.name));
    const kind = spec ? KINDS[spec.kind] : undefined;
    if (spec && !kind) {
      this.note.textContent = `This netlist asks for a "${spec.kind}" sketch, which this interface does not draw.`;
      this.note.hidden = false;
    } else if (spec && kind && typeof spec.bind === 'object' && spec.bind) {
      const missing = kind.required.filter((s) => !names.has(spec.bind[s]));
      const unknown = Object.keys(spec.bind).filter((s) => !(s in kind.slots));
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
        this.kind = kind;
      }
    }
    if (this.hint) this.hint.textContent = this.kind?.caption ?? this.defaultHint;
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
    if (!this.spec || !this.kind) return out;
    const drives = (target: string) => this.deps.get(target)?.has(name) ?? target === name;
    for (const [slot, param] of Object.entries(this.spec.bind)) {
      if (drives(param)) for (const p of this.kind.slots[slot] ?? []) out.add(p);
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
    const label = (param: string | undefined) => {
      if (!param) return undefined;
      const v = values.get(param);
      return v === undefined ? undefined : (this.choiceLabels.get(param)?.get(String(v)) ?? String(v));
    };
    const ear = label(this.earParam);
    const openV = b.open_back ? values.get(b.open_back) : undefined;
    switch (this.spec.kind) {
      case 'in_ear': {
        const d = num('driver_diameter_mm');
        const Df = num('front_depth_mm');
        const dn = num('nozzle_diameter_mm');
        const Ln = num('nozzle_length_mm');
        if (d === undefined || Df === undefined || dn === undefined || Ln === undefined) return null;
        if (d <= 0 || Df <= 0 || dn <= 0 || Ln <= 0) return null;
        return {
          kind: 'in_ear',
          d,
          Df,
          Vf: num('front_volume_cm3'),
          dn,
          Ln,
          nmesh: num('nozzle_mesh_rayl'),
          dc: num('canal_diameter_mm'),
          Db: num('back_depth_mm'),
          damping: num('damping_rayl'),
          Dr: num('rear_depth_mm'),
          Vr: num('rear_volume_cm3'),
          n: num('vent_count'),
          dv: num('vent_diameter_mm'),
          t: num('vent_length_mm'),
          vmesh: num('vent_mesh_rayl'),
          fit: label(b.fit),
          dl: num('leak_diameter_mm'),
          Ll: num('leak_length_mm'),
          ear,
        };
      }
      case 'on_ear':
      case 'over_ear': {
        const onEar = this.spec.kind === 'on_ear';
        const R = num(onEar ? 'pad_inner_radius_mm' : 'cup_radius_mm');
        const D = num('front_depth_mm');
        if (R === undefined || D === undefined || R <= 0 || D <= 0) return null;
        const Rc = onEar ? num('cup_radius_mm') : undefined;
        return {
          kind: onEar ? 'on_ear' : 'over_ear',
          R,
          D,
          Vf: num('front_volume_cm3'),
          concha: onEar ? num('concha_volume_cm3') : undefined,
          d: num('driver_diameter_mm'),
          W: num('pad_width_mm'),
          gap: num('leak_gap_mm'),
          fit: onEar ? label(b.fit) : undefined,
          open: openV === true,
          Rr: Rc !== undefined && Rc > 0 ? Rc : R,
          Dr: num('rear_depth_mm'),
          Vr: num('rear_volume_cm3'),
          n: num('vent_count'),
          dv: num('vent_diameter_mm'),
          t: num('vent_length_mm'),
          mesh: num('vent_mesh_rayl'),
          grille: num('grille_rayl'),
          damping: num('damping_rayl'),
          ear,
        };
      }
      default:
        return null;
    }
  }

  private render(): void {
    const svg = this.svg;
    svg.replaceChildren();
    const g = this.geometry(this.values);
    // Not laid out yet: the scale, and so whether the gap is enlarged, is unknown.
    this.desc.textContent = g ? describe(g, true) : '';
    if (!g || !this.width) return;

    // The height depends on the width only, so the controls below never
    // move while a value is dragged.
    const Wp = this.width;
    const Hp = Math.round(Math.min(250, Math.max(200, Wp * 0.5)));
    svg.setAttribute('viewBox', `0 0 ${Wp} ${Hp}`);
    svg.setAttribute('height', String(Hp));
    const cv = new Canvas(svg, Wp, Hp);
    const ref = this.reference?.kind === g.kind ? this.reference : null;
    this.desc.textContent = g.kind === 'in_ear' ? drawInEar(cv, g, ref as Iem | null) : drawCup(cv, g, ref as Cup | null);
    this.highlightParts(this.glow);
  }
}

// ----- text alternatives ------------------------------------------------------

function describe(g: Geo, gapEnlarged: boolean): string {
  return g.kind === 'in_ear' ? describeInEar(g) : describeCup(g, gapEnlarged);
}

function ventText(n: number | undefined, dv: number | undefined, t: number | undefined, mesh: number | undefined, noun: string): string {
  if (n === undefined) return '';
  if (n <= 0) return `, no ${noun}s`;
  return (
    ` with ${n} ${noun}${n === 1 ? '' : 's'}${dv !== undefined ? ` of ${mm(dv)} diameter` : ''}` +
    `${t !== undefined ? ` through a ${mm(t)} wall` : ''}` +
    (mesh !== undefined ? (mesh > 0 ? `, each under a ${rayl(mesh)} mesh` : ', open holes') : '') +
    ` (${noun} positions schematic)`
  );
}

/** Every dimension the cup drawing shows (`gapEnlarged`: the leak gap is drawn larger than to scale). */
function describeCup(g: Cup, gapEnlarged: boolean): string {
  const onEar = g.kind === 'on_ear';
  const s: string[] = [
    onEar
      ? `Cross-section through the cup axis, to scale, the pad resting on the pinna. Front chamber: radius ${mm(g.R)}, depth ${mm(g.D)} to the pinna` +
        (g.Vf !== undefined ? `, volume ${formatParam(g.Vf, 3)} cm³` : '') +
        (g.concha !== undefined ? ` including a ${cm3(g.concha)} concha` : '') +
        '. Pinna and concha drawn schematically.'
      : `Cross-section through the cup axis, to scale. Front cavity: radius ${mm(g.R)}, depth ${mm(g.D)}` +
        (g.Vf !== undefined ? `, volume ${formatParam(g.Vf, 3)} cm³.` : '.'),
  ];
  if (g.d !== undefined) s.push(`Driver: diaphragm diameter ${mm(g.d)}.`);
  if (g.damping !== undefined) {
    s.push(g.damping > 0 ? `Damping cloth of ${rayl(g.damping)} behind the diaphragm (thickness schematic).` : 'No damping cloth.');
  }
  if (g.W !== undefined || g.gap !== undefined || g.fit !== undefined) {
    let p = `Pad: ${g.W !== undefined ? `${mm(g.W)} wide` : 'width not bound'}`;
    if (onEar) {
      const leak = g.fit === undefined ? '' : `, fit: ${g.fit}`;
      const gap =
        g.gap === undefined
          ? ''
          : g.gap > 0
            ? `, leak slit ${mm(g.gap)} high${gapEnlarged ? ' (drawn enlarged)' : ''}, position schematic`
            : '';
      p += `${leak}${gap}.`;
    } else {
      p +=
        g.gap === undefined
          ? '.'
          : g.gap > 0
            ? `, leak gap ${mm(g.gap)}${gapEnlarged ? ' (drawn enlarged)' : ''}.`
            : ', sealed (no leak gap).';
    }
    s.push(p);
  }
  if (g.open) {
    s.push(`Rear: open back${g.grille !== undefined ? ` behind a ${rayl(g.grille)} grille` : ''}.`);
  } else if (g.Dr !== undefined) {
    const where = onEar ? `, radius ${mm(g.Rr)}` : '';
    s.push(
      `Rear: closed cavity ${mm(g.Dr)} deep${where}${g.Vr !== undefined ? ` (${formatParam(g.Vr, 3)} cm³)` : ''}` +
        `${ventText(g.n, g.dv, g.t, g.mesh, 'vent')}.`,
    );
  }
  if (g.ear) s.push(`Ear load: ${g.ear}.`);
  return s.join(' ');
}

function describeInEar(g: Iem): string {
  const s: string[] = [
    `Cross-section along the earphone's axis, to scale. Driver: diaphragm diameter ${mm(g.d)}.`,
    `Front volume${g.Vf !== undefined ? ` ${cm3(g.Vf)}` : ''}, ${mm(g.Df)} deep over the diaphragm.`,
    `Nozzle: bore ${mm(g.dn)}, ${mm(g.Ln)} long` +
      (g.nmesh !== undefined ? (g.nmesh > 0 ? `, a ${rayl(g.nmesh)} mesh at its outlet.` : ', no mesh.') : '.'),
  ];
  if (g.damping !== undefined) {
    s.push(
      g.damping > 0
        ? `Damping of ${rayl(g.damping)}${g.Db !== undefined ? ` ${mm(g.Db)} behind the diaphragm` : ' behind the diaphragm'} (thickness schematic).`
        : 'No damping behind the diaphragm.',
    );
  }
  if (g.Dr !== undefined) {
    s.push(`Rear volume${g.Vr !== undefined ? ` ${cm3(g.Vr)}` : ''}, ${mm(g.Dr)} deep${ventText(g.n, g.dv, g.t, g.vmesh, 'vent')}.`);
  }
  if (g.fit !== undefined || g.dl !== undefined) {
    const fit = g.fit !== undefined ? `Ear tip: ${g.fit}` : 'Ear tip';
    const sealed = g.dl !== undefined && g.dl <= 0;
    s.push(
      sealed || g.dl === undefined
        ? `${fit}${sealed ? ', no leak' : ''}; tip shape schematic.`
        : `${fit}, a leak tube of ${mm(g.dl)} diameter${g.Ll !== undefined ? `, ${mm(g.Ll)} long,` : ''} through the tip (position schematic); tip shape schematic.`,
    );
  }
  if (g.dc !== undefined) s.push(`Canal bore ${mm(g.dc)} (length schematic).`);
  if (g.ear) s.push(`Ear load: ${g.ear}.`);
  return s.join(' ');
}

// ----- cup kinds (over_ear, on_ear) ----------------------------------------------

/** Half-width and height of the drawn cup, in mm. */
function cupExtents(g: Cup): [number, number] {
  const t = g.t ?? 0;
  const x = Math.max(g.R + Math.max(g.W ?? 0, t), (g.d ?? 0) / 2, g.open ? 0 : g.Rr + t);
  const y = (g.kind === 'on_ear' ? PINNA_MM : 0) + g.D + (g.open ? 0 : (g.Dr ?? 0) + t);
  return [x, y];
}

/** Draws a cup kind and returns its text alternative. */
function drawCup(cv: Canvas, g: Cup, ref: Cup | null): string {
  const { Wp, Hp } = cv;
  const onEar = g.kind === 'on_ear';
  // Margins: dimension labels at the left, room for the vent or grille
  // label above and the ear-load and leak labels below.
  const mL = 78;
  const mR = 14;
  const mT = 40;
  const mB = 46;
  const [xc, yc] = cupExtents(g);
  const [xr, yr] = ref ? cupExtents(ref) : [0, 0];
  const X = Math.max(xc, xr);
  const Y = Math.max(yc, yr, 1);
  const topReserve = g.open ? 36 : 0;
  const s = Math.min((Wp - mL - mR) / (2 * X), (Hp - mT - mB - topReserve) / Y);
  const ox = mL + (Wp - mL - mR) / 2;
  const oy = Hp - mB;
  // The pad and the front cavity stand on the compressed pinna (on-ear).
  const P = onEar ? PINNA_MM : 0;
  const px = (x: number) => ox + x * s;
  const py = (y: number) => oy - (y + P) * s;
  const { text, dim } = cv;

  const t = g.t ?? 0;
  const W = g.W ?? 0;
  const dia = g.d ?? 0;
  const outer = Math.max(g.R + Math.max(W, t), onEar && !g.open ? g.Rr : 0);
  const leftDimX = px(-Math.max(outer, dia / 2, onEar && !g.open ? g.Rr + t : 0)) - 12;
  const closed = !g.open && g.Dr !== undefined;
  const Dr = closed ? g.Dr! : 0;
  const Rr = g.Rr;
  const tw = Math.max(t * s, 1.5);
  const top = py(g.D + Dr);

  // Ear load: the head or fixture surface the pad rests on.
  const x0 = px(-X) - 28;
  const x1 = px(X) + 28;
  cv.part('ear').append(
    node('rect', { x: x0, y: oy, width: x1 - x0, height: 9, class: 'sk-head' }),
    node('line', { x1: x0, y1: oy, x2: x1, y2: oy, class: 'sk-surface' }),
    node('circle', { cx: ox, cy: oy, r: 3.5, class: 'sk-entrance' }),
  );
  const earLabel = text(ox, oy + 23, g.ear ? `Ear load: ${g.ear}` : 'Head or fixture surface');
  cv.lab('ear').append(earLabel);
  cv.keepInside(earLabel);

  // On-ear: the pinna, compressed under the pad, with the concha opening
  // under the front chamber (both schematic).
  if (onEar) {
    const pin = cv.part('pinna');
    const half = Math.max(g.R + W, 1) + 4;
    const concha = Math.min(g.R * 0.6, 12);
    pin.append(node('rect', { x: px(-concha), y: oy - P * s, width: 2 * concha * s, height: P * s, class: 'sk-air' }));
    for (const [a, b] of [
      [-half, -concha],
      [concha, half],
    ]) {
      pin.append(node('rect', { x: px(a), y: oy - P * s, width: (b - a) * s, height: P * s, rx: 3, class: 'sk-pinna' }));
    }
    cv.lab('pinna').append(text(px(-half) - 4, oy - (P * s) / 2 + 4, 'pinna', 'end'));
  }

  // Air: front cavity, and the rear cavity when the back is closed.
  cv.part('front').append(node('rect', { x: px(-g.R), y: py(g.D), width: 2 * g.R * s, height: g.D * s, class: 'sk-air' }));
  if (closed) cv.part('rear').append(node('rect', { x: px(-Rr), y: top, width: 2 * Rr * s, height: Dr * s, class: 'sk-air' }));

  // Pad, with the leak gap under it enlarged to a few pixels.
  // Tenths of a millimetre are sub-pixel at this scale: the gap is drawn
  // 3 to 10 px high, and says so only when that is more than its true size.
  const trueGapPx = g.gap === undefined || g.gap <= 0 ? 0 : g.gap * s;
  const gapPx = trueGapPx > 0 ? Math.max(trueGapPx, Math.min(10, 3 + 25 * g.gap!)) : 0;
  const gapEnlarged = gapPx > trueGapPx + 0.5;
  const gapMm = gapPx / s;
  // The on-ear leak is one slit: drawn under the right-hand pad only.
  const gapUnder = (side: number) => (onEar ? (side > 0 ? gapMm : 0) : gapMm);
  if (g.W !== undefined) {
    const pad = cv.part('pad');
    for (const [side, xa] of [
      [-1, px(-g.R - W)],
      [1, px(g.R)],
    ]) {
      const gm = gapUnder(side);
      pad.append(node('rect', { x: xa, y: py(g.D), width: W * s, height: (g.D - gm) * s, rx: 2, class: 'sk-pad' }));
    }
    const yp = py(gapMm + (g.D - gapMm) * 0.32);
    if (W * s >= 44) {
      cv.lab('pad').append(dim(px(g.R), yp, px(g.R + W), yp), text((px(g.R) + px(g.R + W)) / 2, yp - 5, mm(W)));
    } else {
      cv.lab('pad').append(text(px(g.R + W) + 4, yp + 4, mm(W), 'start'));
    }
  }
  if (g.gap !== undefined) {
    const leak = cv.part('leak');
    if (gapPx > 0) {
      for (const [side, xa] of [
        [-1, px(-g.R - W)],
        [1, px(g.R)],
      ]) {
        if (gapUnder(side) > 0) leak.append(node('rect', { x: xa, y: py(0) - gapPx, width: Math.max(W * s, 6), height: gapPx, class: 'sk-leak' }));
      }
    }
    const xa = px(g.R + W / 2);
    const ly = oy + 39;
    const what = onEar ? 'leak slit' : 'leak gap';
    cv.lab('leak').append(
      node('polyline', { points: `${xa},${py(0) - gapPx / 2} ${xa + 10},${ly - 12} ${Wp - 6},${ly - 12}`, class: 'sk-leader' }),
      text(
        Wp - 6,
        ly,
        gapPx > 0 ? `${what} ${mm(g.gap)}${gapEnlarged ? ' (drawn enlarged)' : ''}` : onEar ? 'sealed on the pinna' : 'pad sealed: no leak gap',
        'end',
      ),
    );
  }

  // Cup shell (side walls and a top plate as thick as the vents are
  // long) and the baffle the driver sits in.
  let holes: [number, number][] = [];
  const n = Math.max(0, Math.round(g.n ?? 0));
  if (closed) {
    const shell = cv.part('shell');
    for (const xa of [px(-Rr) - tw, px(Rr)]) {
      shell.append(node('rect', { x: xa, y: top - tw, width: tw, height: Dr * s + tw, class: 'sk-solid' }));
    }
    const dv = g.dv ?? 0;
    for (let k = 0; k < n; k++) {
      const xk = -Rr + ((k + 1) * 2 * Rr) / (n + 1);
      holes.push([px(xk - dv / 2), px(xk + dv / 2)]);
    }
    let xa = px(-Rr);
    for (const [a, b] of holes) {
      if (a > xa) shell.append(node('rect', { x: xa, y: top - tw, width: a - xa, height: tw, class: 'sk-solid' }));
      xa = Math.max(xa, b);
    }
    if (px(Rr) > xa) shell.append(node('rect', { x: xa, y: top - tw, width: px(Rr) - xa, height: tw, class: 'sk-solid' }));
  } else {
    holes = [];
  }
  const baffle = closed ? cv.part('shell') : cv.shapes;
  baffle.append(
    node('line', { x1: px(-outer), y1: py(g.D), x2: px(-dia / 2), y2: py(g.D), class: 'sk-baffle' }),
    node('line', { x1: px(dia / 2), y1: py(g.D), x2: px(outer), y2: py(g.D), class: 'sk-baffle' }),
  );

  // Damping cloth behind the diaphragm: a band of schematic thickness.
  if ((g.damping ?? 0) > 0 && dia > 0) {
    cv.part('damping').append(node('rect', { x: px(-dia / 2), y: py(g.D) - 4, width: dia * s, height: 3, class: 'sk-damping' }));
  }

  // Driver: the diaphragm spans its effective diameter; the dome is a symbol.
  if (g.d !== undefined) {
    const dome = Math.min(0.1 * g.d, (onEar ? 0.15 : 0.35) * g.D);
    cv.part('driver').append(
      node('path', { d: `M ${px(-g.d / 2)} ${py(g.D)} Q ${ox} ${py(g.D - 2 * dome)} ${px(g.d / 2)} ${py(g.D)}`, class: 'sk-diaphragm' }),
    );
    const yd = py(g.D) - 9;
    if (closed ? onEar || Dr * s >= 30 : true) {
      cv.lab('driver').append(dim(px(-g.d / 2), yd, px(g.d / 2), yd), text(ox, yd - 5, `driver Ø ${mm(g.d)}`));
    } else {
      cv.lab('driver').append(text(px(g.d / 2) + 4, py(g.D) + 14, `Ø ${mm(g.d)}`, 'start'));
    }
  }

  // Rear: vents through the top plate, or the open grille.
  if (closed) {
    const vents = cv.part('vents');
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
    cv.lab('vents').append(text(ox, top - tw - 9, vl));
    const rl = cv.lab('rear');
    if (g.Vr !== undefined && Dr * s >= 34) rl.append(text(px(-Rr) + 5, top + 14, `${formatParam(g.Vr, 3)} cm³`, 'start'));
    rl.append(dim(leftDimX, py(g.D), leftDimX, top), text(leftDimX - 6, py(g.D + Dr / 2) + 4, mm(Dr), 'end'));
  } else if (g.open) {
    const half = Math.max(dia / 2, g.R * 0.6);
    const yg = py(g.D) - 30;
    cv.part('grille').append(node('line', { x1: ox - half * s, y1: yg, x2: ox + half * s, y2: yg, class: 'sk-grille' }));
    cv.lab('grille').append(
      text(ox, yg - 10, `open back: grille${g.grille !== undefined ? ` ${formatParam(g.grille, 3)} rayl` : ''}, to the room`),
    );
  }

  // Front cavity dimensions: radius in the right half, volume in the left.
  const ym = py(g.D * (onEar ? 0.3 : 0.5));
  const fl = cv.lab('front');
  fl.append(dim(ox, ym, px(g.R), ym), text((ox + px(g.R)) / 2, ym - 5, `r ${mm(g.R)}`));
  if (g.Vf !== undefined) fl.append(text((px(-g.R) + ox) / 2, ym + 4, `${formatParam(g.Vf, 3)} cm³`));
  fl.append(dim(leftDimX, py(0), leftDimX, py(g.D)), text(leftDimX - 6, ym + 4, mm(g.D), 'end'));

  // Axis and scale bar.
  cv.shapes.append(node('line', { x1: ox, y1: oy + 6, x2: ox, y2: oy - yc * s - 6 - topReserve, class: 'sk-axis' }));
  cv.scaleBar(8, 14, s);
  return describeCup(g, gapEnlarged);
}

// ----- in_ear ----------------------------------------------------------------------

/**
 * Positions along the axis (mm from the outer face of the back wall), the
 * drawn half-height, and the route of the leak tube: through the lower half
 * of the tip ring, out along the ear's surface and on under the earphone,
 * so that its drawn length is its length.
 */
function iemLayout(g: Iem) {
  const t = g.t ?? IEM_WALL;
  const Dr = g.Dr ?? 0;
  const Db = g.Db ?? 0;
  const xRear = t;
  const xDamp = xRear + Dr;
  const xDia = xDamp + Db;
  const xNoz = xDia + g.Df;
  const xOut = xNoz + g.Ln;
  const xEnd = xOut + IEM_CANAL;
  const dc = g.dc ?? g.d * 0.75;
  const half = Math.max(g.d / 2 + IEM_WALL, dc / 2 + IEM_WALL);
  const tipLen = Math.min(IEM_TIP, 0.7 * g.Ln);
  const xEntrance = xOut - tipLen;
  const nozOuter = g.dn / 2 + IEM_NOZZLE_WALL;
  const leak: [number, number][] = [];
  let low = half;
  if (g.dl !== undefined && g.dl > 0) {
    const yl = -(dc / 2 + nozOuter) / 2;
    const yRun = -(half + 1);
    const route: [number, number][] = [
      [xOut, yl],
      [xEntrance, yl],
      [xEntrance, yRun],
      [xEntrance - 1e3, yRun],
    ];
    let left = g.Ll ?? tipLen;
    leak.push(route[0]);
    for (let k = 1; k < route.length && left > 0; k++) {
      const [x0, y0] = route[k - 1];
      const [x1, y1] = route[k];
      const seg = Math.hypot(x1 - x0, y1 - y0);
      const f = Math.min(1, left / seg);
      leak.push([x0 + (x1 - x0) * f, y0 + (y1 - y0) * f]);
      left -= seg * f;
    }
    low = Math.max(half, -Math.min(...leak.map((q) => q[1])) + g.dl / 2);
  }
  const xMin = Math.min(0, ...leak.map((q) => q[0]));
  return { t, Dr, Db, xRear, xDamp, xDia, xNoz, xOut, xEnd, dc, half, low, tipLen, xEntrance, nozOuter, leak, xMin };
}

function drawInEar(cv: Canvas, g: Iem, ref: Iem | null): string {
  const { Wp, Hp, text, dim } = cv;
  const L = iemLayout(g);
  const R = ref ? iemLayout(ref) : null;
  // Label rows: the scale bar and three rows of labels above the drawing,
  // three rows of labels and the ear load below it.
  const row = 13;
  const mT = 8 + 4 * row;
  const mB = 8 + 4 * row;
  const mL = 84;
  const mR = 10;
  const xMin = Math.min(L.xMin, R?.xMin ?? 0);
  const spanX = Math.max(L.xEnd, R?.xEnd ?? 0) - xMin;
  const spanY = 2 * Math.max(L.low, R?.low ?? 0);
  const s = Math.min((Wp - mL - mR) / spanX, (Hp - mT - mB) / spanY);
  const x0 = mL - xMin * s;
  const yAxis = mT + (Hp - mT - mB) / 2;
  const px = (x: number) => x0 + x * s;
  const py = (y: number) => yAxis - y * s;
  const rd = g.d / 2;
  const above = (k: number) => 4 + (k + 1) * row;
  const below = (k: number) => Hp - mB + 4 + (k + 1) * row;
  // Labels start at their part and run to the right, on rows above and
  // below the drawing. The leftmost part takes the row farthest from the
  // drawing, so no leader crosses another label.
  interface Call {
    part: Part;
    x: number;
    y: number;
    t: string;
  }
  const up: Call[] = [];
  const down: Call[] = [];
  const place = (list: Call[], rows: number[]) => {
    list.sort((a, b) => a.x - b.x);
    list.forEach((c, i) => {
      const yRow = rows[Math.min(i, rows.length - 1)];
      const yLine = yRow < c.y ? yRow + 3 : yRow - 10;
      const label = text(Math.max(c.x, 4), yRow, c.t, 'start');
      cv.lab(c.part).append(node('polyline', { points: `${c.x},${c.y} ${c.x},${yLine}`, class: 'sk-leader' }), label);
      cv.keepInside(label);
    });
  };

  // Ear: the canal bore in the ear (hatched) from the tip onwards.
  const { tipLen, xEntrance, nozOuter } = L;
  const ear = cv.part('ear');
  const earTop = py(L.half) - 6;
  const earBottom = py(-L.half) + 6;
  ear.append(
    node('rect', { x: px(xEntrance), y: earTop, width: px(L.xEnd) - px(xEntrance), height: py(L.dc / 2) - earTop, class: 'sk-head' }),
    node('rect', { x: px(xEntrance), y: py(-L.dc / 2), width: px(L.xEnd) - px(xEntrance), height: earBottom - py(-L.dc / 2), class: 'sk-head' }),
    node('rect', { x: px(L.xOut), y: py(L.dc / 2), width: (L.xEnd - L.xOut) * s, height: L.dc * s, class: 'sk-air' }),
    node('line', { x1: px(xEntrance), y1: earTop, x2: px(xEntrance), y2: py(L.dc / 2), class: 'sk-surface' }),
    node('line', { x1: px(xEntrance), y1: py(-L.dc / 2), x2: px(xEntrance), y2: earBottom, class: 'sk-surface' }),
  );
  if (g.dc !== undefined) {
    const xd = px(L.xEnd) - 6;
    cv.lab('ear').append(dim(xd, py(g.dc / 2), xd, py(-g.dc / 2)));
    if ((L.xEnd - L.xOut) * s >= 64) cv.lab('ear').append(text(xd - 5, yAxis - 5, `Ø ${mm(g.dc)}`, 'end'));
  }
  const earLabel = text(Wp - 4, below(3), g.ear ? `Ear load: ${g.ear}` : 'Ear canal', 'end');
  cv.lab('ear').append(earLabel);
  cv.keepInside(earLabel);

  // Ear tip: a ring between the nozzle wall and the canal wall (schematic).
  const tip = cv.part('tip');
  for (const sgn of [1, -1]) {
    const yTop = sgn > 0 ? py(L.dc / 2) : py(-nozOuter);
    tip.append(node('rect', { x: px(xEntrance), y: yTop, width: tipLen * s, height: (L.dc / 2 - nozOuter) * s, rx: 3, class: 'sk-tip' }));
  }

  // Shell around the rear, the air behind the diaphragm and the front volume.
  const shell = cv.part('shell');
  const sh = rd + IEM_WALL;
  shell.append(
    node('rect', { x: px(0), y: py(sh), width: L.xNoz * s + IEM_WALL * s, height: 2 * sh * s, rx: 4, class: 'sk-solid' }),
    // Nozzle walls, as far as the tip.
    node('rect', { x: px(L.xNoz), y: py(nozOuter), width: g.Ln * s, height: 2 * nozOuter * s, class: 'sk-solid' }),
  );

  // Air spaces (to scale): rear volume, air behind the diaphragm, front volume, nozzle bore.
  if (g.Dr !== undefined) cv.part('rear').append(node('rect', { x: px(L.xRear), y: py(rd), width: L.Dr * s, height: g.d * s, class: 'sk-air' }));
  const back = cv.part('damping');
  if (L.Db > 0) back.append(node('rect', { x: px(L.xDamp), y: py(rd), width: L.Db * s, height: g.d * s, class: 'sk-air' }));
  if ((g.damping ?? 0) > 0) back.append(node('rect', { x: px(L.xDamp) - 1.5, y: py(rd), width: 3, height: g.d * s, class: 'sk-damping' }));
  cv.part('front').append(node('rect', { x: px(L.xDia), y: py(rd), width: g.Df * s, height: g.d * s, class: 'sk-air' }));
  const noz = cv.part('nozzle');
  noz.append(node('rect', { x: px(L.xNoz), y: py(g.dn / 2), width: g.Ln * s, height: g.dn * s, class: 'sk-air' }));
  if ((g.nmesh ?? 0) > 0) noz.append(node('rect', { x: px(L.xOut) - 2, y: py(g.dn / 2) - 1, width: 4, height: g.dn * s + 2, class: 'sk-mesh' }));

  // Diaphragm across the driver's diameter; the dome is a symbol.
  const dome = Math.min(0.1 * g.d, 0.8 * Math.max(g.Df, 0.2));
  cv.part('driver').append(
    node('path', { d: `M ${px(L.xDia)} ${py(rd)} Q ${px(L.xDia + 2 * dome)} ${yAxis} ${px(L.xDia)} ${py(-rd)}`, class: 'sk-diaphragm' }),
  );

  // Vents through the back wall, as wide as they are, positions schematic.
  const n = Math.max(0, Math.round(g.n ?? 0));
  if (g.n !== undefined) {
    const vents = cv.part('vents');
    const dv = g.dv ?? 0;
    for (let k = 0; k < n; k++) {
      const yk = -rd + ((k + 1) * 2 * rd) / (n + 1);
      vents.append(node('rect', { x: px(0) - 1, y: py(yk + dv / 2), width: L.t * s + 2, height: Math.max(dv * s, 1), class: 'sk-hole' }));
      if ((g.vmesh ?? 0) > 0) vents.append(node('rect', { x: px(0) - 5, y: py(yk + dv / 2) - 1, width: 4, height: Math.max(dv * s, 1) + 2, class: 'sk-mesh' }));
    }
    let vl = n === 0 ? 'no vents' : `${n} vent${n === 1 ? '' : 's'}`;
    if (n > 0 && g.dv !== undefined) vl += ` Ø ${mm(g.dv)}`;
    if (n > 0 && g.t !== undefined) vl += `, ${mm(g.t)} long`;
    if (n > 0 && g.vmesh !== undefined) vl += g.vmesh > 0 ? `, mesh ${formatParam(g.vmesh, 3)} rayl` : ', open';
    up.push({ part: 'vents', x: px(L.t / 2), y: py(sh), t: vl });
  }

  // Leak past the tip: a tube to scale in diameter and length, along the
  // route of iemLayout (position schematic).
  if (g.fit !== undefined || g.dl !== undefined) {
    if (g.dl !== undefined && g.dl > 0) {
      const w = Math.max(g.dl * s, 1.5);
      const d = L.leak.map(([x, y], k) => `${k ? 'L' : 'M'} ${px(x)} ${py(y)}`).join(' ');
      cv.part('leak').append(node('path', { d, 'stroke-width': w, class: 'sk-leak-path' }));
      const [xe, ye] = L.leak[L.leak.length - 1];
      down.push({ part: 'leak', x: px(xe), y: py(ye) + w / 2, t: `leak Ø ${mm(g.dl)}${g.Ll !== undefined ? ` × ${mm(g.Ll)}` : ''}` });
    } else {
      down.push({ part: 'leak', x: px(L.xOut - tipLen / 2), y: py(-L.dc / 2), t: g.fit ? `tip: ${g.fit}` : 'tip sealed' });
    }
  }

  // Dimensions and labels.
  const xd = px(0) - 12;
  const dLabel = text(xd - 6, yAxis + 4, `Ø ${mm(g.d)}`, 'end');
  cv.lab('driver').append(dim(xd, py(rd), xd, py(-rd)), dLabel);
  cv.keepInside(dLabel);
  if (g.Dr !== undefined) {
    up.push({ part: 'rear', x: px(L.xRear + L.Dr / 2), y: py(sh), t: `rear ${mm(L.Dr)}${g.Vr !== undefined ? `, ${cm3(g.Vr)}` : ''}` });
  }
  if ((g.damping ?? 0) > 0) up.push({ part: 'damping', x: px(L.xDamp), y: py(sh), t: `damping ${rayl(g.damping!)}` });
  down.push({ part: 'front', x: px(L.xDia + g.Df / 2), y: py(-sh), t: `front ${mm(g.Df)}${g.Vf !== undefined ? `, ${cm3(g.Vf)}` : ''}` });
  // The nozzle's length along its bore.
  cv.lab('nozzle').append(dim(px(L.xNoz), yAxis, px(L.xOut), yAxis));
  down.push({
    part: 'nozzle',
    x: px(L.xNoz + g.Ln / 2),
    y: py(-nozOuter),
    t: `nozzle Ø ${mm(g.dn)} × ${mm(g.Ln)}${g.nmesh !== undefined && g.nmesh > 0 ? `, mesh ${formatParam(g.nmesh, 3)} rayl` : ''}`,
  });
  place(up, [above(1), above(2), above(3)]);
  place(down, [below(2), below(1), below(0)]);

  // Axis and scale bar.
  cv.shapes.append(node('line', { x1: px(0) - 4, y1: yAxis, x2: px(L.xEnd) + 4, y2: yAxis, class: 'sk-axis' }));
  cv.scaleBar(8, above(0) - 4, s);
  return describeInEar(g);
}
