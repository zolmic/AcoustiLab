// Warnings panel: operating limits exceeded at the stated drive, prominent
// and selectable (a selected one is picked out on the plots), and element
// notes collapsed under "Model notes (n)". Every sentence shown is the
// engine's own message or its numbers; nothing here explains causes.

import { formatHz, formatParam } from './format';
import type { Highlight } from './plot';
import type { SolveResult, Warning } from './types';

export const isOperating = (w: Warning) => w.f_min_Hz !== null && w.f_max_Hz !== null;

/** Identity of a warning across solves: element, code and what was measured. */
const warningKey = (w: Warning) => `${w.element}|${w.code}|${w.message.split(' reaches ')[0]}`;

const words = (code: string) => code.replace(/_/g, ' ');

function valueWithUnit(v: number | null, unit: string | null): string {
  return v === null ? 'n/a' : `${formatParam(v, 3)}${unit ? ` ${unit}` : ''}`;
}

export function hzRange(a: number, b: number): string {
  return Math.abs(b / a - 1) < 1e-9 ? formatHz(a) : `${formatHz(a)} to ${formatHz(b)}`;
}

/** The plot highlight of an operating-limit warning. */
export function highlightOf(w: Warning): Highlight {
  return { lo: w.f_min_Hz!, hi: w.f_max_Hz!, label: `${w.element ?? ''} ${words(w.code)}: ${hzRange(w.f_min_Hz!, w.f_max_Hz!)}` };
}

export interface WarningsElements {
  drive: HTMLElement;
  list: HTMLElement;
  none: HTMLElement;
  notes: HTMLDetailsElement;
  notesSummary: HTMLElement;
  noteList: HTMLElement;
  /** Link near the plots' heading that jumps here. */
  jump: HTMLElement;
  jumpLink: HTMLAnchorElement;
}

export class WarningsPanel {
  private selectedKey: string | null = null;
  private operating: Warning[] = [];

  constructor(
    private readonly els: WarningsElements,
    /** The selection changed (`fromClick`: by the user, not by a new result). */
    private readonly onSelect: (w: Warning | null, fromClick: boolean) => void,
  ) {}

  /** The selected operating-limit warning of the last result, if any. */
  get selected(): Warning | null {
    return this.operating.find((w) => warningKey(w) === this.selectedKey) ?? null;
  }

  render(r: SolveResult): void {
    const ws = r.warnings ?? [];
    const ops = ws.filter(isOperating);
    const notes = ws.filter((w) => !isOperating(w));
    this.operating = ops;
    const { els } = this;
    els.drive.textContent = `Operating limits are checked at the stated drive: ${r.meta.drive?.label ?? 'as written'}.`;
    els.list.replaceChildren();
    // A selection survives a re-solve while the same limit is still exceeded.
    if (this.selectedKey && !ops.some((w) => warningKey(w) === this.selectedKey)) this.selectedKey = null;
    for (const w of ops) els.list.append(this.item(w));
    els.none.hidden = ops.length > 0;
    els.none.textContent = 'No operating limit is exceeded at the stated drive.';
    els.jump.hidden = ops.length === 0;
    els.jumpLink.textContent = `⚠ ${ops.length} operating limit${ops.length === 1 ? '' : 's'} exceeded at the stated drive`;

    els.notes.hidden = notes.length === 0;
    els.notesSummary.textContent = `Model notes (${notes.length})`;
    els.noteList.replaceChildren();
    for (const w of notes) {
      const li = document.createElement('li');
      const el = document.createElement('strong');
      el.textContent = w.element ?? 'network';
      li.append(el, ` (${words(w.code)}${w.severity === 'warning' ? ', warning' : ''}): ${w.message}`);
      els.noteList.append(li);
    }
    this.onSelect(this.selected, false);
  }

  private item(w: Warning): HTMLLIElement {
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'warning-item';
    const key = warningKey(w);
    b.dataset.key = key;
    b.setAttribute('aria-pressed', String(key === this.selectedKey));
    const title = document.createElement('span');
    title.className = 'warning-title';
    const el = document.createElement('strong');
    el.textContent = w.element ?? 'network';
    title.append(el, ` · ${words(w.code)} · ${hzRange(w.f_min_Hz!, w.f_max_Hz!)}`);
    const vals = document.createElement('span');
    vals.className = 'warning-values';
    vals.textContent =
      `worst ${valueWithUnit(w.value, w.unit)}` +
      (w.at_Hz !== null ? ` at ${formatHz(w.at_Hz)}` : '') +
      `, limit ${valueWithUnit(w.limit, w.unit)}`;
    const msg = document.createElement('span');
    msg.className = 'warning-message';
    msg.textContent = w.message;
    const icon = document.createElement('span');
    icon.className = 'warning-icon';
    icon.setAttribute('aria-hidden', 'true');
    icon.textContent = '⚠';
    const body = document.createElement('span');
    body.className = 'warning-body';
    body.append(title, vals, msg);
    b.append(icon, body);
    b.title = 'Show this frequency range on the plots';
    b.addEventListener('click', () => {
      this.selectedKey = this.selectedKey === key ? null : key;
      for (const x of this.els.list.querySelectorAll<HTMLElement>('.warning-item')) {
        x.setAttribute('aria-pressed', String(x.dataset.key === this.selectedKey));
      }
      this.onSelect(this.selected, true);
    });
    li.append(b);
    return li;
  }
}
