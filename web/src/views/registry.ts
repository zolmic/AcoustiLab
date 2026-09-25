// Finds every `*.view.ts` module in this directory (see types.ts).

import type { ResultView } from './types';

const modules = import.meta.glob<{ view: ResultView }>('./*.view.ts', { eager: true });

export const VIEWS: ResultView[] = Object.values(modules)
  .map((m) => m.view)
  .sort((a, b) => a.order - b.order || a.label.localeCompare(b.label));
