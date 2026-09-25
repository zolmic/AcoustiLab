// Example netlists: every examples/*.json in the repository, bundled at
// build time.

const raw = import.meta.glob('../../examples/*.json', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

export interface Example {
  name: string;
  title: string;
  text: string;
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
    return { name, title, text };
  })
  .sort((a, b) => a.name.localeCompare(b.name));
