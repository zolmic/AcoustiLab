// The design templates beyond the over-ear one (examples/design_on_ear.json,
// examples/design_in_ear.json) and their cross-section sketches.
//
// Oracles: the template files (parameter values, labels), the engine's
// resolved values (`meta.parameters`), and ratios of drawn SVG lengths,
// which must equal the ratios of the parameters they draw. SVG lengths are
// single precision, so ratios are compared to 6 digits.

import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const read = (name: string) => JSON.parse(readFileSync(`${repo}/examples/${name}.json`, 'utf8'));
const ON_EAR = read('design_on_ear');
const IN_EAR = read('design_in_ear');

interface Hook {
  result(): { meta: { parameters: Record<string, number | string | boolean> }; probes: { id: string }[] } | null;
  groups(): { key: string; series: string[] }[];
  sketch(): { parts: string[]; glow: string[] };
}

const hook = <T>(page: Page, fn: (h: Hook) => T): Promise<T> =>
  page.evaluate(`(${fn.toString()})(window.acoustilab)`) as Promise<T>;

async function solved(page: Page): Promise<void> {
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solved', { timeout: 20_000 });
}

async function openTemplate(page: Page, name: string): Promise<void> {
  await page.goto('/');
  await solved(page);
  await page.selectOption('#example-select', name);
  await solved(page);
  await expect(page.getByRole('tab', { name: 'Design', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('#sketch')).toBeVisible();
}

const row = (page: Page, name: string) => page.locator(`.prow[data-param="${name}"]`);
const params = async (page: Page) => (await hook(page, (h) => h.result()))!.meta.parameters as Record<string, number>;

/** Size of the first element matching `sel` (rect attributes, or a path's bounding box). */
const size = (page: Page, sel: string) =>
  page
    .locator(sel)
    .first()
    .evaluate((e: SVGGraphicsElement) =>
      e instanceof SVGRectElement ? { w: e.width.baseVal.value, h: e.height.baseVal.value } : { w: e.getBBox().width, h: e.getBBox().height },
    );

const glow = async (page: Page) => [...new Set((await hook(page, (h) => h.sketch())).glow)].sort();

test('the on-ear template opens with its sketch, drawn to scale and linked to the controls', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await openTemplate(page, 'design_on_ear');
  await expect(page.locator('#design-about-text')).toHaveText(ON_EAR.description);
  await expect(page.locator('.pgroup:visible .pgroup-name')).toHaveText(['Driver', 'Front chamber', 'Pad and leak', 'Rear', 'Ear', 'Source']);
  // Four fit states: a select.
  const fit = page.getByLabel('Fit on the pinna', { exact: true });
  await expect(fit).toHaveValue('low');
  await expect(page.locator('#strip-ear')).toHaveText(ON_EAR.parameters.ear.choices[0].label);
  expect((await hook(page, (h) => h.groups()))[0].series[0]).toBe('p_drp');

  const p = await params(page);
  const ri = ON_EAR.parameters.pad_inner_radius_mm.value;
  const D = ON_EAR.parameters.front_depth_mm.value;
  const W = ON_EAR.parameters.pad_width_mm.value;
  expect(p.cup_radius_mm).toBe(ri + W);
  // Front volume: the cylinder under the pad plus the concha.
  expect(p.front_volume_cm3).toBeCloseTo((Math.PI * ri * ri * D) / 1000 + ON_EAR.parameters.concha_volume_cm3.value, 12);

  const sk = await hook(page, (h) => h.sketch());
  for (const part of ['ear', 'pinna', 'front', 'pad', 'leak', 'rear', 'shell', 'vents', 'driver', 'damping']) expect(sk.parts, part).toContain(part);
  await expect(page.locator('#sketch-caption .hint')).toContainText('pinna, concha');
  await expect(page.locator('#sketch-caption .hint')).toContainText('leak gap enlarged and its position schematic');
  const desc = page.locator('#sketch-desc');
  await expect(desc).toContainText(`Front chamber: radius ${ri} mm, depth ${D} mm to the pinna`);
  await expect(desc).toContainText('including a 4.3 cm³ concha');
  await expect(desc).toContainText('fit: Low leak (P.57 Type 3.2 slit), leak slit 0.26 mm high (drawn enlarged), position schematic');

  // To scale: every drawn width and depth relative to the front chamber's width 2·ri.
  const front = await size(page, '#sketch-svg [data-part="front"] rect.sk-air');
  expect(front.w / front.h).toBeCloseTo((2 * ri) / D, 6);
  const rel = async (sel: string, dim: 'w' | 'h') => (await size(page, sel))[dim] / front.w;
  expect(await rel('#sketch-svg [data-part="pad"] rect.sk-pad', 'w')).toBeCloseTo(W / (2 * ri), 6);
  expect(await rel('#sketch-svg [data-part="rear"] rect.sk-air', 'w')).toBeCloseTo(p.cup_radius_mm / ri, 6);
  expect(await rel('#sketch-svg [data-part="rear"] rect.sk-air', 'h')).toBeCloseTo(p.rear_depth_mm / (2 * ri), 6);
  expect(await rel('#sketch-svg [data-part="vents"] rect.sk-hole', 'w')).toBeCloseTo(ON_EAR.parameters.vent_diameter_mm.value / (2 * ri), 6);
  expect(await rel('#sketch-svg [data-part="driver"] path', 'w')).toBeCloseTo(p.driver_diameter_mm / (2 * ri), 3);
  // One slit, drawn under one pad.
  await expect(page.locator('#sketch-svg [data-part="leak"] rect.sk-leak')).toHaveCount(1);

  // The fit states change the leak (P.57 Type 3.2 high leak: 0.50 mm).
  await fit.selectOption('high');
  await solved(page);
  expect((await params(page)).leak_gap_mm).toBe(0.5);
  await expect(desc).toContainText('leak slit 0.5 mm high');
  await fit.selectOption('sealed');
  await solved(page);
  await expect(page.locator('#sketch-svg [data-part="leak"] rect.sk-leak')).toHaveCount(0);
  await expect(page.locator('#sketch-svg')).toContainText('sealed on the pinna');
  expect((await hook(page, (h) => h.result()))!.probes.map((q) => q.id)).not.toContain('u_leak');

  // Hover and focus link controls and parts, through derived parameters:
  // the pad's inner radius also sets the cup radius the rear depth follows.
  await row(page, 'pad_inner_radius_mm').locator('.plabel').hover();
  await expect.poll(() => glow(page)).toEqual(['front', 'pad', 'rear', 'shell']);
  await row(page, 'concha_volume_cm3').locator('.plabel').hover();
  await expect.poll(() => glow(page)).toEqual(['front', 'pinna']);
  await page.mouse.move(0, 0);
  await fit.focus();
  await expect.poll(() => glow(page)).toEqual(['leak']);
  await page.locator('#sketch-svg [data-part="pinna"] rect').first().hover();
  const marked = await page.locator('.prow.pglow').evaluateAll((rs) => rs.map((r) => (r as HTMLElement).dataset.param));
  expect(marked).toEqual(expect.arrayContaining(['concha_volume_cm3']));
  expect(errors).toEqual([]);
});

test('the in-ear template opens with its sketch, drawn to scale and linked to the controls', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await openTemplate(page, 'design_in_ear');
  await expect(page.locator('#design-about-text')).toHaveText(IN_EAR.description);
  await expect(page.locator('.pgroup:visible .pgroup-name')).toHaveText(['Driver', 'Front and nozzle', 'Ear tip and fit', 'Rear and vent', 'Ear', 'Source']);
  await expect(page.locator('#strip-ear')).toHaveText('IEC 60318-4 occluded-ear simulator');
  await expect(page.locator('#strip-drive')).toContainText('1 mW into 16 ohm rated');
  // Qes follows from the force factor: 2 pi fs Mms Re / Bl^2.
  const p = await params(page);
  const P = IN_EAR.parameters;
  expect(p.driver_Qes).toBeCloseTo((2 * Math.PI * P.driver_fs_Hz.value * P.driver_Mms_mg.value * 1e-6 * P.driver_Re_ohm.value) / P.driver_Bl_Tm.value ** 2, 12);

  const sk = await hook(page, (h) => h.sketch());
  for (const part of ['ear', 'tip', 'shell', 'rear', 'damping', 'front', 'nozzle', 'driver', 'vents', 'leak']) expect(sk.parts, part).toContain(part);
  await expect(page.locator('#sketch-caption .hint')).toContainText('ear tip, canal length');
  const desc = page.locator('#sketch-desc');
  await expect(desc).toContainText(`Nozzle: bore ${P.nozzle_diameter_mm.value} mm, ${P.nozzle_length_mm.value} mm long, no mesh.`);
  await expect(desc).toContainText('Ear tip: Sealed, no leak');

  // To scale: lengths along the axis against the diaphragm's diameter d.
  const d = p.driver_diameter_mm;
  const front = await size(page, '#sketch-svg [data-part="front"] rect.sk-air');
  expect(front.w / front.h).toBeCloseTo(p.front_depth_mm / d, 6);
  const rel = async (sel: string, dim: 'w' | 'h') => (await size(page, sel))[dim] / front.h;
  expect(await rel('#sketch-svg [data-part="rear"] rect.sk-air', 'w')).toBeCloseTo(p.rear_depth_mm / d, 6);
  expect(await rel('#sketch-svg [data-part="damping"] rect.sk-air', 'w')).toBeCloseTo(p.back_depth_mm / d, 6);
  expect(await rel('#sketch-svg [data-part="nozzle"] rect.sk-air', 'w')).toBeCloseTo(P.nozzle_length_mm.value / d, 6);
  expect(await rel('#sketch-svg [data-part="nozzle"] rect.sk-air', 'h')).toBeCloseTo(P.nozzle_diameter_mm.value / d, 6);
  expect(await rel('#sketch-svg [data-part="ear"] rect.sk-air', 'h')).toBeCloseTo(P.canal_diameter_mm.value / d, 6);
  expect(await rel('#sketch-svg [data-part="vents"] rect.sk-hole', 'h')).toBeCloseTo(P.vent_diameter_mm.value / d, 6);
  expect(await rel('#sketch-svg [data-part="driver"] path', 'h')).toBeCloseTo(1, 3);
  await expect(page.locator('#sketch-svg [data-part="leak"] path')).toHaveCount(0);

  // A loose fit draws the leak tube to scale: 1.016 mm across, 13 mm long
  // (along a schematic route: through the tip, then out under the earphone).
  const fit = page.getByLabel('Ear-tip fit', { exact: true });
  await expect(fit).toHaveValue('sealed');
  await fit.selectOption('loose');
  await solved(page);
  expect((await params(page)).leak_diameter_mm).toBe(1.016);
  expect((await hook(page, (h) => h.result()))!.probes.map((q) => q.id)).toContain('u_leak');
  // The drawing zooms out to make room for the tube: measure again.
  const h = (await size(page, '#sketch-svg [data-part="front"] rect.sk-air')).h;
  const leak = await page
    .locator('#sketch-svg [data-part="leak"] path.sk-leak-path')
    .evaluate((e: SVGPathElement) => ({ len: e.getTotalLength(), w: Number(e.getAttribute('stroke-width')) }));
  expect(leak.len / h).toBeCloseTo(P.leak_length_mm.value / d, 4);
  expect(leak.w / h).toBeCloseTo(1.016 / d, 6);
  await expect(desc).toContainText('a leak tube of 1.02 mm diameter, 13 mm long, through the tip');
  // A mesh at the nozzle outlet is drawn and described.
  await page.locator('#p-nozzle_mesh_rayl').fill('200');
  await page.locator('#p-nozzle_mesh_rayl').press('Enter');
  await solved(page);
  await expect(page.locator('#sketch-svg [data-part="nozzle"] rect.sk-mesh')).toHaveCount(1);
  await expect(desc).toContainText('a 200 rayl mesh at its outlet');

  // Hover links: the diaphragm area reaches every depth drawn from it.
  await row(page, 'driver_Sd_cm2').locator('.plabel').hover();
  await expect.poll(() => glow(page)).toEqual(['damping', 'driver', 'front', 'rear', 'shell']);
  await row(page, 'nozzle_diameter_mm').locator('.plabel').hover();
  await expect.poll(() => glow(page)).toEqual(['nozzle', 'tip']);
  await page.mouse.move(0, 0);
  await page.locator('#p-custom_leak_diameter_mm').focus();
  await expect.poll(() => glow(page)).toEqual(['leak']);
  await page.locator('#sketch-svg [data-part="nozzle"] text').first().click();
  await expect(page.locator('#p-nozzle_diameter_mm')).toBeFocused();
  expect(errors).toEqual([]);
});

for (const name of ['design_on_ear', 'design_in_ear']) {
  test(`${name}: nothing scrolls sideways at 390 px and the sketch fits`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await openTemplate(page, name);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    // Every label of the sketch lies inside the drawing.
    const out = await page.locator('#sketch-svg').evaluate((svg: SVGSVGElement) => {
      const w = svg.viewBox.baseVal.width;
      return [...svg.querySelectorAll<SVGTextElement>('text')]
        .map((t) => ({ t: t.textContent, b: t.getBBox() }))
        .filter(({ b }) => b.x < 0 || b.x + b.width > w)
        .map(({ t }) => t);
    });
    expect(out).toEqual([]);
    await page.getByRole('switch', { name: 'Show detailed parameters' }).click();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });

  for (const scheme of ['light', 'dark'] as const) {
    test(`${name}: axe clean (${scheme}) with the detailed view open`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await openTemplate(page, name);
      await page.getByRole('switch', { name: 'Show detailed parameters' }).click();
      await page.locator('#design-about summary').click();
      const axe = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
      expect(axe.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
    });
  }
}
