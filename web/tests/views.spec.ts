// Result views (src/views/*.view.ts): the tab bar, lazy mounting, refresh on
// new results, keyboard operation, and the "Parameter table" view, whose oracle
// is the engine's own meta (parameter values, parameters_used, elements).

import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

async function solved(page: Page): Promise<void> {
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solved', { timeout: 20_000 });
}

const cellOf = (page: Page, param: string, col: number) =>
  page.locator(`#view-parameter-table tr[data-param="${param}"] td`).nth(col);

test('result views: tabs, lazy mount, refresh on a new result, keyboard', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const tabs = page.getByRole('tablist', { name: 'Result views' });
  await expect(tabs).toBeVisible();
  await expect(tabs.getByRole('tab', { name: 'Response' })).toHaveAttribute('aria-selected', 'true');
  // Not mounted until opened.
  await expect(page.locator('#view-parameter-table')).toBeHidden();
  await expect(page.locator('#view-parameter-table table')).toHaveCount(0);

  await tabs.getByRole('tab', { name: 'Parameter table' }).click();
  await expect(page.locator('#view-response')).toBeHidden();
  await expect(page.locator('#view-parameter-table table')).toBeVisible();
  // Closed back: the vents are used, the grille is not (engine meta.parameters_used).
  await expect(cellOf(page, 'vent_count', 1)).toHaveText('1');
  await expect(cellOf(page, 'vent_count', 3)).toHaveText('yes');
  await expect(cellOf(page, 'grille_rayl', 3)).toHaveText('no');
  await expect(cellOf(page, 'front_volume_cm3', 2)).toHaveText(/^= pi/);
  await expect(page.locator('#view-parameter-table .element-list')).toContainText('iec60318_4');

  // A design edit re-solves; the open view refreshes with the new result.
  await page.getByRole('radio', { name: /Open back/ }).check();
  await solved(page);
  await expect(cellOf(page, 'rear', 1)).toHaveText('open (changed)');
  await expect(cellOf(page, 'vent_count', 3)).toHaveText('no');
  await expect(cellOf(page, 'grille_rayl', 3)).toHaveText('yes');
  await expect(page.locator('#view-parameter-table .element-list')).toContainText('grille');

  // Arrow keys move between tabs (roving tabindex); Response draws its plots again.
  await tabs.getByRole('tab', { name: 'Parameter table' }).focus();
  await page.keyboard.press('Home');
  await expect(tabs.getByRole('tab', { name: 'Response' })).toBeFocused();
  await expect(tabs.getByRole('tab', { name: 'Response' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('#view-response')).toBeVisible();
  await expect(page.locator('#plots canvas').first()).toBeVisible();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('#view-parameter-table')).toBeVisible();

  // The choice is remembered per viewer.
  await page.reload();
  await solved(page);
  await expect(page.locator('#view-parameter-table')).toBeVisible();
});

for (const theme of ['light', 'dark'] as const) {
  test(`result views (${theme}): no axe violations with a view open`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: theme });
    await page.goto('/');
    await solved(page);
    await page.getByRole('tablist', { name: 'Result views' }).getByRole('tab', { name: 'Parameter table' }).click();
    await expect(page.locator('#view-parameter-table table')).toBeVisible();
    const r = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
    expect(r.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
  });
}
