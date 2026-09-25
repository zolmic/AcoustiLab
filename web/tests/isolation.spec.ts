// The Isolation view (isolation.view.ts). Oracle: the engine's `isolation`
// export called in Node on the same netlist; the view must show those
// numbers in its tables and readouts and draw them where a linear dB axis
// puts them.

import { expect, test, type Page } from '@playwright/test';
import { axe, cssColor, engine, example, near, noSideScroll, open, openView, runNetlist, viewHook } from './measure.helpers';

const dbText = (v: number, digits: number) => `${v < 0 && Number(Math.abs(v).toFixed(digits)) !== 0 ? '−' : ''}${Math.abs(v).toFixed(digits)} dB`;

async function isolationView(page: Page, name = 'closed_cup_isolation'): Promise<void> {
  await open(page, name);
  await openView(page, 'Isolation');
  await expect(page.locator('#view-isolation table.mv-bands')).toBeVisible({ timeout: 30_000 });
}

const readout = (page: Page, figure: string, series: string) =>
  page.locator(`#view-isolation .mv-figure[data-figure="${figure}"] .readout li[data-series="${series}"] strong`);

test('isolation: the closed-cup example shows the export’s numbers, and its curves are drawn where they belong', async ({ page }) => {
  await isolationView(page);
  const panel = page.locator('#view-isolation');
  const call = await engine();
  const ref = call('isolation', example('closed_cup_isolation'), '', JSON.stringify({ paths: true, bleed: false }));
  expect(await viewHook(page, 'isolation', 'report')).toEqual(ref);

  // Conventions as the engine states them; the ear and the paths.
  await expect(panel.locator('.mv-conventions')).toContainText(ref.convention);
  await expect(panel.getByText(ref.driven.join(', '), { exact: true })).toBeVisible();

  // ETSI summary and every 1/3-octave band.
  const s = ref.summary;
  await expect(panel.locator('[data-field="max"]')).toHaveText(`${s.max_dB.toFixed(1)} dB in the ${(s.max_at_Hz / 1000).toPrecision(4)} kHz band`);
  const rows = panel.locator('table.mv-bands tbody tr');
  await expect(rows).toHaveCount(ref.third_octave_bands.length);
  for (const b of ref.third_octave_bands) {
    await expect(panel.locator(`table.mv-bands tr[data-nominal="${b.nominal_Hz}"] td`).nth(1)).toHaveText(b.insertion_loss_dB.toFixed(1));
  }
  // The fixture's bound applies to the bands it names (80 Hz to 250 Hz: > 50 dB, ...).
  await expect(panel.locator('table.mv-bands tr[data-nominal="80"] td').nth(3)).toHaveText(/^> 50/);
  await expect(panel.locator('table.mv-bands tr[data-nominal="63"] td').nth(3)).toHaveText('—');
  await expect(panel.locator('table.mv-bands tr[data-nominal="6300"] td').nth(3)).toHaveText('> 55 (exceeded)');

  // Readout at grid points equals the export's IL and path levels.
  const f: number[] = ref.frequencies_Hz;
  const idx = (target: number) => f.reduce((best, x, k) => (Math.abs(Math.log(x / target)) < Math.abs(Math.log(f[best] / target)) ? k : best), 0);
  const points = [idx(40), idx(300), idx(1000), idx(3000)];
  const ys: number[] = [];
  for (const i of points) {
    await viewHook(page, 'isolation', 'setCursor', i);
    await expect(readout(page, 'isolation', 'insertion loss')).toHaveText(dbText(ref.insertion_loss_dB[i], 2));
    await expect(readout(page, 'isolation', 'path leak')).toHaveText(dbText(ref.paths[0].level_re_open_dB[i], 2));
    const fig = await viewHook(page, 'isolation', 'figure');
    ys.push(fig.plots.find((p: { key: string }) => p.key === 'il').cursorY['insertion loss']);
  }
  // The plot's y positions are one linear dB axis through all four points.
  const il = points.map((i) => ref.insertion_loss_dB[i]);
  const b = (ys[1] - ys[0]) / (il[1] - il[0]);
  const a = ys[0] - b * il[0];
  expect(b).toBeLessThan(0);
  for (let k = 2; k < 4; k++) expect(Math.abs(a + b * il[k] - ys[k])).toBeLessThan(0.75);

  // Without the crosshair, the curve's colour is found on that line at each
  // point's x (the IL curve is drawn last, never covered).
  await viewHook(page, 'isolation', 'setCursor', null);
  const blue = await cssColor(page, '--series-1');
  for (const [k, i] of points.entries()) {
    const x = await viewHook(page, 'isolation', 'xOf', 'il', f[i]);
    const col = await viewHook(page, 'isolation', 'column', 'il', x);
    const y = a + b * il[k];
    const hit = col.px.some((c: number[], j: number) => Math.abs(j / col.dpr - y) <= 2 && near(c, blue));
    expect(hit, `IL at ${f[i]} Hz`).toBe(true);
  }
  // The fixture's bound is drawn as a limit at 65 dB over 350 Hz–4 kHz, on the same axis.
  const amber = await cssColor(page, '--series-4');
  let found = 0;
  for (const fx of [500, 700, 1100, 1500, 2200, 3000]) {
    const x = await viewHook(page, 'isolation', 'xOf', 'il', fx);
    const col = await viewHook(page, 'isolation', 'column', 'il', x);
    if (col.px.some((c: number[], j: number) => Math.abs(j / col.dpr - (a + b * 65)) <= 2 && near(c, amber))) found++;
  }
  expect(found).toBeGreaterThanOrEqual(3);
  // Where the prediction exceeds that bound, the engine's frequencies are named.
  await expect(panel.getByText(`The predicted loss exceeds it at ${ref.fixture_self_insertion_loss.exceeded_at_Hz.length} frequencies`, { exact: false })).toBeVisible();
});

test('isolation: undriven paths are warned, bleed on request, and a netlist without a drum probe is refused', async ({ page }) => {
  // The leak ending at the reference instead of the outside air.
  const text = example('closed_cup_isolation').replace('"nodes": ["a_ear", "ambient"]', '"nodes": ["a_ear", "gnd"]');
  expect(text).not.toBe(example('closed_cup_isolation'));
  await open(page);
  await runNetlist(page, text);
  await openView(page, 'Isolation');
  const panel = page.locator('#view-isolation');
  const call = await engine();
  const ref = call('isolation', text, '', JSON.stringify({ paths: true, bleed: false }));
  const w = ref.warnings.find((x: { code: string }) => x.code === 'undriven_path');
  expect(w.element).toBe('leak');
  await expect(panel.locator('li[data-code="undriven_path"]')).toContainText(w.message);

  // Bleed at 0.3 m and 1 m with its ±6 dB band.
  await panel.getByLabel('Bleed at 0.3 m and 1 m (±6 dB)').check();
  const withBleed = call('isolation', text, '', JSON.stringify({ paths: true, bleed: true }));
  await expect(panel.locator('.mv-figure[data-figure="bleed"]')).toBeVisible({ timeout: 30_000 });
  const bl = withBleed.bleed;
  expect(bl.distances_m).toEqual([0.3, 1]);
  await expect(panel.getByText(bl.model, { exact: false })).toBeVisible();
  const canvas = panel.locator('.mv-figure[data-figure="bleed"] canvas');
  await canvas.focus();
  await page.keyboard.press('Home');
  const cur = (await viewHook(page, 'isolation', 'bleedFigure')).cursor as number;
  const v03 = bl.spl_dB[0][cur];
  await expect(panel.locator('.mv-figure[data-figure="bleed"] .readout li[data-series="0.3 m"] strong')).toHaveText(`${v03.toFixed(1)} dB SPL`);
  await expect(panel.locator('.mv-figure[data-figure="bleed"] .readout li[data-series="0.3 m +6 dB"] strong')).toHaveText(`${(v03 + bl.band_dB).toFixed(1)} dB SPL`);
  await expect(panel.locator('.mv-figure[data-figure="bleed"] .readout li[data-series="1 m −6 dB"] strong')).toHaveText(`${(bl.spl_dB[1][cur] - bl.band_dB).toFixed(1)} dB SPL`);

  // sealed_cup has no ui.primary_probe: the engine asks for the drum probe.
  await page.selectOption('#example-select', 'sealed_cup');
  await expect(panel.locator('.mv-error')).toContainText('name the drum-point probe', { timeout: 30_000 });
});

test('isolation: keyboard, axe in both themes, and 390 px', async ({ page }) => {
  await isolationView(page);
  const panel = page.locator('#view-isolation');
  await panel.getByLabel('Bleed at 0.3 m and 1 m (±6 dB)').check();
  await expect(panel.locator('.mv-figure[data-figure="bleed"]')).toBeVisible({ timeout: 30_000 });
  // Keyboard: Tab from the options reaches the plots; the arrow keys read them out.
  const il = panel.locator('.mv-figure[data-figure="isolation"] canvas').first();
  await il.focus();
  await page.keyboard.press('ArrowLeft');
  await expect(page.locator('#view-status')).toContainText('insertion loss');
  await panel.locator('.mv-figure[data-figure="isolation"]').getByRole('button', { name: 'Show data table' }).click();
  await expect(panel.locator('.mv-figure[data-figure="isolation"] table thead th').nth(1)).toHaveText('Validity');
  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    expect(await axe(page), theme).toEqual([]);
  }
  await page.setViewportSize({ width: 390, height: 844 });
  await noSideScroll(page);
});
