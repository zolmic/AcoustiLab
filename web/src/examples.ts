// Example netlists: every examples/*.json in the repository, bundled at
// build time. Those that declare parameters are offered as design
// templates.

const raw = import.meta.glob('../../examples/*.json', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

export interface Example {
  name: string;
  title: string;
  text: string;
  /** Declares at least one parameter (a design template). */
  parametric: boolean;
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
    return { name, title, text, parametric: hasParameters(text) };
  })
  .sort((a, b) => a.name.localeCompare(b.name));
