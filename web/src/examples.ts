// Example netlists: every examples/*.json in the repository, bundled at
// build time. Those whose `ui` block names a `template` are offered as
// design templates; any example that declares parameters opens in the
// Design tab.

const raw = import.meta.glob('../../examples/*.json', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

export interface Example {
  name: string;
  title: string;
  text: string;
  /** Declares at least one parameter (opens in the Design tab). */
  parametric: boolean;
  /** A design template: its `ui` block names a `template`. */
  template: boolean;
}

/** Whether a netlist text declares a non-empty `parameters` block. */
export function hasParameters(text: string): boolean {
  try {
    const p = JSON.parse(text)?.parameters;
    return typeof p === 'object' && p !== null && !Array.isArray(p) && Object.keys(p).length > 0;
  } catch {
    return false;
  }
}

/** Whether a netlist text's `ui` block names a design `template`. */
export function isTemplate(text: string): boolean {
  try {
    const t = JSON.parse(text)?.ui?.template;
    return typeof t === 'string' && t.length > 0;
  } catch {
    return false;
  }
}

export const EXAMPLES: Example[] = Object.entries(raw)
  .map(([path, text]) => {
    const name = path.split('/').pop()!.replace(/\.json$/, '');
    let title = '';
    try {
      const t = JSON.parse(text)?.title;
      if (typeof t === 'string') title = t;
    } catch {
      /* shown as-is; the engine reports the syntax error */
    }
    return { name, title, text, parametric: hasParameters(text), template: isTemplate(text) };
  })
  .sort((a, b) => a.name.localeCompare(b.name));
