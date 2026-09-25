// "Parameter table": every parameter of the solved design with its value, the
// loaded template's value, and whether the design uses it, plus the resolved
// elements. The
// numbers come from the last solve's `meta` (values, parameters_used,
// elements) and the labels from `parameters()`; nothing here is computed in
// the UI.

import { formatParam } from '../format';
import type { ParamDesc, ResultView, Scalar, ViewHost } from './types';

function valueText(v: Scalar | null | undefined, unit: string | null): string {
  if (v === null || v === undefined) return '—';
  if (typeof v === 'number') return unit ? `${formatParam(v)} ${unit}` : formatParam(v);
  if (typeof v === 'boolean') return v ? 'yes' : 'no';
  return v;
}

function cell(tag: 'td' | 'th', text: string, cls?: string): HTMLTableCellElement {
  const c = document.createElement(tag);
  c.textContent = text;
  if (cls) c.className = cls;
  if (tag === 'th') c.scope = 'row';
  return c;
}

class DesignTable implements ResultView {
  readonly id = 'parameter-table';
  readonly label = 'Parameter table';
  readonly order = 90;
  private host!: ViewHost;
  private body!: HTMLElement;

  mount(el: HTMLElement, host: ViewHost): void {
    this.host = host;
    const intro = document.createElement('p');
    intro.className = 'hint';
    intro.textContent =
      'The solved design in numbers: each parameter with its value, the loaded template’s value, and whether the current ' +
      'design uses it (a parameter the topology ignores, such as vent sizes with an open back, is marked unused).';
    this.body = document.createElement('div');
    el.append(intro, this.body);
  }

  refresh(): void {
    this.body.replaceChildren();
    const cur = this.host.current();
    if (!cur) {
      this.body.append(Object.assign(document.createElement('p'), { className: 'hint', textContent: 'Run a netlist to see its parameter table.' }));
      return;
    }
    const meta = cur.result.meta;
    const values = meta.parameters ?? {};
    const used = new Set(meta.parameters_used ?? []);
    const doc = this.host.parameters();
    const descs = new Map<string, ParamDesc>((doc?.parameters ?? []).map((p) => [p.name, p]));
    const ref = this.host.reference();
    const names = Object.keys(values);
    if (names.length) {
      const table = document.createElement('table');
      table.className = 'data-table design-table';
      const cap = document.createElement('caption');
      cap.textContent = `Parameters of the solved design (${names.length}; ${used.size} used by the netlist)`;
      const head = document.createElement('tr');
      for (const h of ['Parameter', 'Group', 'Value', 'Template', 'Used']) {
        const c = document.createElement('th');
        c.scope = 'col';
        c.textContent = h;
        head.append(c);
      }
      const thead = document.createElement('thead');
      thead.append(head);
      const tbody = document.createElement('tbody');
      for (const name of names) {
        const d = descs.get(name);
        const unit = d?.unit ?? null;
        const tr = document.createElement('tr');
        tr.dataset.param = name;
        const v = values[name];
        const def = d?.kind === 'derived' ? undefined : ref?.get(name);
        const changed = def !== undefined && def !== v;
        tr.append(
          cell('th', d ? `${d.label} (${name})` : name),
          cell('td', d?.group ?? ''),
          cell('td', valueText(v, unit) + (changed ? ' (changed)' : ''), changed ? 'changed' : undefined),
          cell('td', d?.kind === 'derived' ? `= ${d.expr ?? ''}` : valueText(def, unit)),
          cell('td', used.has(name) ? 'yes' : 'no', used.has(name) ? undefined : 'unused'),
        );
        tbody.append(tr);
      }
      table.append(cap, thead, tbody);
      // A scrollable region must be keyboard-reachable (WCAG 2.1.1).
      const wrap = document.createElement('div');
      wrap.className = 'table-wrap';
      wrap.tabIndex = 0;
      wrap.setAttribute('role', 'region');
      wrap.setAttribute('aria-label', 'Parameter table');
      wrap.append(table);
      this.body.append(wrap);
    } else {
      this.body.append(Object.assign(document.createElement('p'), { className: 'hint', textContent: 'This netlist declares no parameters.' }));
    }
    const elements = meta.elements ?? [];
    if (elements.length) {
      const h = document.createElement('h3');
      h.textContent = `Elements of the solved netlist (${elements.length})`;
      const ul = document.createElement('ul');
      ul.className = 'element-list';
      for (const e of elements) {
        const li = document.createElement('li');
        const code = document.createElement('code');
        code.textContent = e.id;
        li.append(code, ` ${e.type}`);
        ul.append(li);
      }
      this.body.append(h, ul);
    }
  }
}

export const view = new DesignTable();
