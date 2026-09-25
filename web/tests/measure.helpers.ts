// Helpers of the time, isolation and fit view tests: page set-up, the
// views' read-only hooks (`window.acoustilabViews`), and the engine itself
// loaded in Node (the same wasm build, called directly rather than through
// the page), whose reports are the oracle for "the export's numbers".

import { expect, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { copyFileSync, mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const repo = fileURLToPath(new URL('../../', import.meta.url));

export const example = (name: string) => readFileSync(`${repo}examples/${name}.json`, 'utf8');

type Engine = Record<string, (...args: string[]) => string>;
let enginePromise: Promise<Engine> | null = null;

/** The engine's wasm exports in Node; each returns parsed JSON. */
export async function engine(): Promise<(fn: string, ...args: string[]) => any> {
  enginePromise ??= (async () => {
    // The glue is an ES module in a directory without a package.json; the
    // test runner would load a `.js` there as CommonJS, so it is imported
    // from an `.mjs` copy.
    const dir = mkdtempSync(join(tmpdir(), 'acoustilab-engine-'));
    const glue = join(dir, 'acoustilab_wasm.mjs');
    copyFileSync(`${repo}crates/acoustilab-wasm/pkg/acoustilab_wasm.js`, glue);
    const pkg = await import(pathToFileURL(glue).href);
    pkg.initSync({ module: readFileSync(`${repo}crates/acoustilab-wasm/pkg/acoustilab_wasm_bg.wasm`) });
    return pkg as Engine;
  })();
  const e = await enginePromise;
  return (fn, ...args) => JSON.parse(e[fn](...args));
}

export async function solved(page: Page): Promise<void> {
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solved', { timeout: 30_000 });
}

/** Opens the page on the design template, or on `name` from the example picker. */
export async function open(page: Page, name?: string): Promise<void> {
  await page.goto('/');
  await solved(page);
  if (name && name !== 'design_over_ear') {
    await page.selectOption('#example-select', name);
    await expect(page.locator('#netlist')).toHaveValue(example(name));
    await solved(page);
  }
}

/** Writes `text` into the netlist editor and runs it. */
export async function runNetlist(page: Page, text: string): Promise<void> {
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  await page.locator('#netlist').fill(text);
  await page.getByRole('button', { name: 'Run', exact: true }).click();
  await solved(page);
}

export async function openView(page: Page, label: string): Promise<void> {
  await page.getByRole('tablist', { name: 'Result views' }).getByRole('tab', { name: label, exact: true }).click();
}

/** Calls a view hook: `window.acoustilabViews[view][fn](...args)`. */
export function viewHook<T = any>(page: Page, view: string, fn: string, ...args: unknown[]): Promise<T> {
  return page.evaluate(([v, f, a]) => (window as any).acoustilabViews[v as string][f as string](...(a as unknown[])), [view, fn, args] as const) as Promise<T>;
}

export async function cssColor(page: Page, name: string): Promise<number[]> {
  const hex = await page.evaluate((n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim(), name);
  return [1, 3, 5].map((k) => parseInt(hex.slice(k, k + 2), 16));
}

export const near = (a: number[], b: number[], tol = 40) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]) <= tol;

/** axe-core with the WCAG 2.0–2.2 A and AA rules; returns the violations as strings. */
export async function axe(page: Page): Promise<string[]> {
  const r = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
  return r.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`);
}

/** No horizontal page scroll. */
export async function noSideScroll(page: Page): Promise<void> {
  const [sw, cw] = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
  expect(sw).toBeLessThanOrEqual(cw);
}
