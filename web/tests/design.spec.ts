// Design mode: the parameter panel over the design template, the surgical
// netlist rewrites behind it, coalesced live solves, the cross-section
// sketch, warnings, low-side validity shading and frozen baselines.
//
// Oracles: the template file itself (the netlist text after an edit must be
// the template text with exactly one value token replaced), the engine's
// own result fields (meta.parameters, warnings, shading), closed forms for
// derived values (front volume = pi r^2 d), and canvas pixels for what is
// drawn.

import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const TEMPLATE = readFileSync(`${repo}/examples/design_over_ear.json`, 'utf8');
/** The template's cup: front radius and depth (mm), and pad width. */
const R = 27.5;
const D = 20;
const PAD = 15;
/** Front volume in cm³ at depth `d`, as the panel shows it (4 significant figures). */
const volume = (d: number) => `${Number(((Math.PI * R * R * d) / 1000).toPrecision(4))} cm³`;

interface Result {
  frequencies_Hz: number[];
  shading: { begin_hz: number | null; deep_hz: number | null; low_begin_hz: number | null; low_deep_hz: number | null };
  probes: { id: string; spl_dB?: (number | null)[] }[];
  meta: { parameters: Record<string, number | string | boolean>; drive: { label: string } };
  warnings: { code: string; element: string; f_min_Hz: number | null; f_max_Hz: number | null; at_Hz: number | null }[];
}

interface Hook {
  result(): Result | null;
  groups(): { key: string; kind: string; series: string[]; overlays: string[] }[];
  cursor(): number | null;
  cursorY(): { key: string; y: Record<string, number> }[];
  solves(): number;
  solveMs(): number;
  params(): Record<string, number | string | boolean>;
  baselines(): { id: number; name: string }[];
  highlight(): { lo: number; hi: number; label: string } | null;
  sketch(): { parts: string[]; glow: string[] };
  sample(key: string, f: number, t: number, h?: number): number[][];
  countColor(key: string, cssVar: string, tol?: number): number;
}

const hook = <T>(page: Page, fn: (h: Hook) => T): Promise<T> =>
  page.evaluate(`(${fn.toString()})(window.acoustilab)`) as Promise<T>;

async function solved(page: Page): Promise<void> {
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solved', { timeout: 20_000 });
}

async function open(page: Page): Promise<void> {
  await page.goto('/');
  await solved(page);
}

const text = (page: Page) => page.locator('#netlist').inputValue();
const row = (page: Page, name: string) => page.locator(`.prow[data-param="${name}"]`);

/** The template text with one value token replaced (the declaration's `"value": x,`). */
function withValue(base: string, name: string, from: string, to: string): string {
  const needle = `"${name}": {"value": ${from},`;
  expect(base.split(needle).length, `${needle} occurs once`).toBe(2);
  return base.replace(needle, `"${name}": {"value": ${to},`);
}

async function cssColor(page: Page, name: string): Promise<number[]> {
  const hex = await page.evaluate((n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim(), name);
  return [1, 3, 5].map((k) => parseInt(hex.slice(k, k + 2), 16));
}
const close = (a: number[], b: number[], tol = 6) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]) <= tol;

test('the template opens in Design mode with its groups, sketch and primary probe', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await open(page);
  await expect(page.getByRole('tab', { name: 'Design', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('#panel-design')).toBeVisible();
  await expect(page.locator('#panel-netlist')).toBeHidden();
  expect(await text(page)).toBe(TEMPLATE);

  // Picker: design templates (examples whose `ui` block names a `template`)
  // first, every other example after them, parametric or not.
  const optgroups = await page.locator('#example-select optgroup').evaluateAll((gs) =>
    gs.map((g) => [(g as HTMLOptGroupElement).label, [...g.querySelectorAll('option')].map((o) => o.value)]),
  );
  // Oracle: the examples directory, read here.
  const examples = readdirSync(`${repo}/examples`)
    .filter((f) => f.endsWith('.json'))
    .map((f) => [f.replace(/\.json$/, ''), JSON.parse(readFileSync(`${repo}/examples/${f}`, 'utf8'))] as const);
  const isTemplate = (d: { ui?: { template?: unknown } }) => typeof d.ui?.template === 'string' && d.ui.template !== '';
  const templates = examples.filter(([, d]) => isTemplate(d)).map(([n]) => n);
  const others = examples.filter(([, d]) => !isTemplate(d)).map(([n]) => n);
  expect(optgroups[0][0]).toBe('Design templates');
  expect([...(optgroups[0][1] as string[])].sort()).toEqual(templates.sort());
  expect(optgroups[0][1]).toContain('design_over_ear');
  for (const t of ['design_in_ear', 'design_on_ear', 'design_over_ear']) expect(optgroups[0][1]).toContain(t);
  expect(optgroups[1][0]).toBe('Example netlists');
  expect([...(optgroups[1][1] as string[])].sort()).toEqual(others.sort());
  expect(optgroups[1][1]).toContain('sealed_cup');
  // A parametric example that is not a template lists with the others.
  expect(others.some((n) => 'parameters' in examples.find(([m]) => m === n)![1])).toBe(true);

  // The template's description (its provenance), as written in the netlist.
  await page.locator('#design-about summary').click();
  await expect(page.locator('#design-about-text')).toHaveText(JSON.parse(TEMPLATE).description);

  // Sections per group, in declaration order; "Model" holds only detailed parameters.
  const visibleGroups = page.locator('.pgroup:visible .pgroup-name');
  await expect(visibleGroups).toHaveText(['Driver', 'Front cavity', 'Pad and leak', 'Rear', 'Ear', 'Source']);
  await expect(row(page, 'driver_Le_uH')).toBeHidden();

  // One control per kind.
  await expect(row(page, 'driver_fs_Hz').locator('input[type="range"]')).toHaveCount(1);
  await expect(page.locator('#p-driver_fs_Hz')).toHaveValue('81.8');
  // A text entry announces neither its bounds nor its arrow keys by itself.
  await expect(page.locator('#p-front_depth_mm')).toHaveAccessibleName('Driver-to-ear depth, mm');
  await expect(page.locator('#p-front_depth_mm')).toHaveAccessibleDescription(
    `3 to 40 mm; arrow keys step the value. ${JSON.parse(TEMPLATE).parameters.front_depth_mm.description} Tolerance ±0.3 mm (normal, 2σ), estimate (pad compression)`,
  );
  await expect(page.locator('#p-vent_count')).toHaveAccessibleDescription('0 to 12; arrow keys step the value.');
  await expect(row(page, 'vent_count').getByRole('button', { name: 'Increase Number of rear vents' })).toBeVisible();
  await expect(page.getByRole('radiogroup', { name: 'Back of the driver' }).getByRole('radio')).toHaveCount(2);
  await expect(page.getByRole('radio', { name: 'Closed cup with vents' })).toBeChecked();
  // Derived: pi * 27.5^2 * 20 / 1000 = 47.517 cm³, shown to 4 significant figures.
  await expect(row(page, 'front_volume_cm3').locator('output')).toHaveText('47.52 cm³');
  await expect(row(page, 'driver_diameter_mm').locator('output')).toHaveText(`${(2 * Math.sqrt(1000 / Math.PI)).toPrecision(4)} mm`);

  // The cross-section is drawn and described.
  await expect(page.locator('#sketch')).toBeVisible();
  const sk = await hook(page, (h) => h.sketch());
  for (const p of ['ear', 'front', 'rear', 'pad', 'leak', 'shell', 'vents', 'driver', 'damping']) expect(sk.parts, p).toContain(p);
  expect(sk.parts).not.toContain('grille');
  const desc = await page.locator('#sketch-desc').textContent();
  expect(desc).toContain('radius 27.5 mm, depth 20 mm, volume 47.5 cm³');
  expect(desc).toContain('Damping cloth of 600 rayl behind the diaphragm');
  expect(desc).toContain('leak gap 0.08 mm');
  expect(desc).toContain('Ear load: IEC 60318-4 ear simulator');
  await expect(page.getByRole('img', { name: /Cross-section/ })).toBeVisible();

  // Primary probe first and emphasised; the strip states drive, ear load and reference.
  const g = await hook(page, (h) => h.groups());
  expect(g[0].key).toBe('spl');
  expect(g[0].series[0]).toBe('p_drp');
  await expect(page.locator('.legend-item').first()).toHaveAttribute('data-probe', 'p_drp');
  await expect(page.locator('.legend-item').first()).toContainText('primary');
  const r = (await hook(page, (h) => h.result()))!;
  await expect(page.locator('#strip-drive')).toHaveText(r.meta.drive.label);
  expect(r.meta.drive.label).toContain('1 mW into 32 ohm rated');
  await expect(page.locator('#strip-ear')).toHaveText('IEC 60318-4 ear simulator');
  await expect(page.locator('#strip-ref')).toHaveText('p_drp at ear.drp');
  expect(errors).toEqual([]);
});

test('a stepper and a slider rewrite exactly the value token and re-solve', async ({ page }) => {
  await open(page);
  const spl0 = (await hook(page, (h) => h.result()!.probes[0].spl_dB!))!;
  const n0 = await hook(page, (h) => h.solves());

  // Integer stepper: vent_count 1 -> 2.
  await page.getByRole('button', { name: 'Increase Number of rear vents' }).click();
  await solved(page);
  const t1 = withValue(TEMPLATE, 'vent_count', '1', '2');
  expect(await text(page)).toBe(t1);
  const r1 = (await hook(page, (h) => h.result()))!;
  expect(r1.meta.parameters.vent_count).toBe(2);
  expect(await hook(page, (h) => h.solves())).toBeGreaterThan(n0);
  // The curve changed (the vents shape the bass).
  const d = r1.probes[0].spl_dB!.map((v, i) => Math.abs(v! - spl0[i]!));
  expect(Math.max(...d)).toBeGreaterThan(0.1);
  await expect(page.locator('#sketch-desc')).toContainText('with 2 vents of 3 mm diameter');

  // Slider, keyboard: one step (0.5 mm), then PageUp (10 steps).
  const slider = row(page, 'front_depth_mm').locator('input[type="range"]');
  await slider.focus();
  await page.keyboard.press('ArrowRight');
  await solved(page);
  expect(await text(page)).toBe(withValue(t1, 'front_depth_mm', '20', '20.5'));
  await expect(page.locator('#p-front_depth_mm')).toHaveValue('20.5');
  await expect(slider).toHaveAttribute('aria-valuetext', '20.5 mm');
  // Derived value: pi * 27.5^2 * 20.5 / 1000 = 48.70 cm³.
  await expect(row(page, 'front_volume_cm3').locator('output')).toHaveText(volume(20.5));
  await page.keyboard.press('PageUp');
  await solved(page);
  expect(await text(page)).toBe(withValue(t1, 'front_depth_mm', '20', '25.5'));
  const r2 = (await hook(page, (h) => h.result()))!;
  expect(r2.meta.parameters.front_depth_mm).toBe(25.5);
  expect(r2.meta.parameters.front_volume_cm3 as number).toBeCloseTo((Math.PI * R * R * 25.5) / 1000, 10);
  await expect(row(page, 'front_volume_cm3').locator('output')).toHaveText(volume(25.5));

  // Slider, pointer: a click at the far right is the maximum (40 mm).
  const box = (await slider.boundingBox())!;
  await page.mouse.click(box.x + box.width - 2, box.y + box.height / 2);
  await solved(page);
  expect((await hook(page, (h) => h.result()))!.meta.parameters.front_depth_mm).toBe(40);

  // Every other character of the text is still the template's.
  expect(await text(page)).toBe(withValue(t1, 'front_depth_mm', '20', '40'));

  // Reset all restores the template text byte for byte.
  await expect(page.getByRole('button', { name: 'Reset all (2)' })).toBeEnabled();
  await page.getByRole('button', { name: 'Reset all (2)' }).focus();
  await page.keyboard.press('Enter');
  await solved(page);
  expect(await text(page)).toBe(TEMPLATE);
  await expect(page.getByRole('button', { name: 'Reset all' })).toBeDisabled();
  // Unavailable, but still holding the keyboard focus (a disabled button
  // would have dropped it to the page); pressing it again does nothing.
  await expect(page.getByRole('button', { name: 'Reset all' })).toBeFocused();
  const n1 = await hook(page, (h) => h.solves());
  await page.keyboard.press('Enter');
  await page.waitForTimeout(300);
  expect(await hook(page, (h) => h.solves())).toBe(n1);
});

test('a choice switches the topology (open back) and the sketch; reset brings it back', async ({ page }) => {
  await open(page);
  await page.getByRole('radio', { name: /Open back/ }).check();
  await solved(page);
  expect(await text(page)).toBe(TEMPLATE.replace('"value": "closed", "choices"', '"value": "open", "choices"'));
  const r = (await hook(page, (h) => h.result()))!;
  expect(r.meta.parameters.rear).toBe('open');
  expect(r.meta.parameters.open_back).toBe(true);
  expect(r.probes.map((p) => p.id)).not.toContain('u_vent');
  const sk = await hook(page, (h) => h.sketch());
  expect(sk.parts).toContain('grille');
  for (const p of ['rear', 'vents', 'shell']) expect(sk.parts).not.toContain(p);
  await expect(page.locator('#sketch-desc')).toContainText('Rear: open back behind a 30 rayl grille');

  const reset = row(page, 'rear').getByRole('button', { name: 'Reset Back of the driver to Closed cup with vents' });
  await reset.focus();
  await page.keyboard.press('Enter');
  await solved(page);
  expect(await text(page)).toBe(TEMPLATE);
  // The reset button hides; the keyboard focus moves to the restored option.
  await expect(reset).toBeHidden();
  await expect(page.getByRole('radio', { name: 'Closed cup with vents' })).toBeFocused();
  expect((await hook(page, (h) => h.sketch())).parts).toContain('vents');
});

test('typed entries: out-of-range and non-numbers are rejected inline, never sent', async ({ page }) => {
  await open(page);
  const entry = page.locator('#p-front_radius_mm');
  const msg = row(page, 'front_radius_mm').locator('.pmsg');
  const n0 = await hook(page, (h) => h.solves());

  await entry.fill('99');
  await entry.press('Enter');
  await expect(msg).toHaveText('The maximum is 40 mm.');
  await expect(entry).toHaveAttribute('aria-invalid', 'true');
  await entry.fill('abc');
  await entry.press('Enter');
  await expect(msg).toHaveText('Enter a number in mm.');
  await entry.fill('14.5');
  await entry.press('Enter');
  await expect(msg).toHaveText('The minimum is 15 mm.');
  await page.waitForTimeout(900); // past the typing debounce
  expect(await text(page)).toBe(TEMPLATE);
  expect(await hook(page, (h) => h.solves())).toBe(n0);

  // Escape restores the value; a valid entry (with its unit typed) is sent.
  await entry.press('Escape');
  await expect(entry).toHaveValue('27.5');
  await expect(msg).toHaveText('');
  await expect(entry).not.toHaveAttribute('aria-invalid', 'true');
  await entry.fill('30.25 mm');
  await entry.press('Enter');
  await solved(page);
  expect(await text(page)).toBe(withValue(TEMPLATE, 'front_radius_mm', '27.5', '30.25'));
  await expect(entry).toHaveValue('30.25');
  // The derived leak perimeter follows: 2 pi r.
  await expect(row(page, 'leak_perimeter_mm').locator('output')).toHaveText(`${(2 * Math.PI * 30.25).toPrecision(4)} mm`);
});

test('rapid input events are coalesced: one solve in flight, only the latest queued', async ({ page }) => {
  await open(page);
  const n0 = await hook(page, (h) => h.solves());
  // 41 input events in one task, as a fast drag produces: positions 20..60
  // of the 0.5 mm step slider (3 mm + 0.5 mm × position).
  await row(page, 'front_depth_mm')
    .locator('input[type="range"]')
    .evaluate((el: HTMLInputElement) => {
      for (let p = 20; p <= 60; p++) {
        el.value = String(p);
        el.dispatchEvent(new Event('input', { bubbles: true }));
      }
    });
  await solved(page);
  const n = (await hook(page, (h) => h.solves())) - n0;
  expect(n, 'worker solves for 41 input events').toBeGreaterThanOrEqual(1);
  expect(n, 'worker solves for 41 input events').toBeLessThanOrEqual(2);
  expect((await hook(page, (h) => h.result()))!.meta.parameters.front_depth_mm).toBe(33);
  expect(await text(page)).toBe(withValue(TEMPLATE, 'front_depth_mm', '20', '33'));
  await expect(page.locator('#p-front_depth_mm')).toHaveValue('33');
});

test('solve time of the template in Chromium (reported, loosely bounded)', async ({ page }) => {
  await open(page);
  // Worker time per solve (the engine call alone), over repeated runs.
  const worker: number[] = [];
  for (let k = 0; k < 8; k++) {
    await page.getByRole('button', { name: 'Run' }).click();
    await solved(page);
    worker.push(await hook(page, (h) => h.solveMs()));
  }
  // End to end: from a stepper click to the new result drawn.
  const e2e: number[] = [];
  for (let k = 0; k < 8; k++) {
    const name = k % 2 ? 'Decrease Number of rear vents' : 'Increase Number of rear vents';
    const ms = await page.evaluate(async (label) => {
      const b = document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
      const t0 = performance.now();
      const done = new Promise<void>((resolve) => {
        const mo = new MutationObserver(() => {
          if (document.body.dataset.state === 'solved') {
            mo.disconnect();
            requestAnimationFrame(() => resolve());
          }
        });
        mo.observe(document.body, { attributes: true, attributeFilter: ['data-state'] });
      });
      b.click();
      await done;
      return performance.now() - t0;
    }, name);
    e2e.push(ms);
  }
  const median = (a: number[]) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];
  const report = `template, 265 frequencies × 7 probes: worker solve median ${median(worker).toFixed(1)} ms (${worker.map((x) => x.toFixed(0)).join(', ')}); click to drawn median ${median(e2e).toFixed(1)} ms (${e2e.map((x) => x.toFixed(0)).join(', ')})`;
  test.info().annotations.push({ type: 'solve time', description: report });
  console.log(report);
  expect(median(worker)).toBeLessThan(2000);
});

test('raising the drive raises operating-limit warnings; clicking one highlights its range', async ({ page }) => {
  await open(page);
  await expect(page.locator('#warnings-none')).toHaveText('No operating limit is exceeded at the stated drive.');
  await expect(page.locator('#notes-summary')).toHaveText('Model notes (1)');
  await expect(page.locator('#warn-jump')).toBeHidden();
  expect(await page.evaluate(`window.acoustilab.countColor('spl', '--hl-edge', 30)`)).toBeLessThan(20);

  // At 1 W the coil is overdriven and the vent's air exceeds 1 m/s in the bass.
  const drive = page.locator('#p-drive_mW');
  await drive.fill('1000');
  await drive.press('Enter');
  await solved(page);
  expect(await text(page)).toBe(withValue(TEMPLATE, 'drive_mW', '1', '1000'));
  const r = (await hook(page, (h) => h.result()))!;
  await expect(page.locator('#strip-drive')).toContainText('1000 mW into 32 ohm rated');
  const ops = r.warnings.filter((w) => w.f_min_Hz !== null);
  expect(ops.map((w) => `${w.element}:${w.code}`).sort()).toEqual(['drv:coil_power', 'vent:particle_velocity']);
  await expect(page.locator('.warning-item')).toHaveCount(2);
  await expect(page.locator('#warn-jump')).toContainText('2 operating limits exceeded');
  // Announced with the solve (a status region), not only shown.
  await expect(page.locator('#run-status')).toContainText('2 operating limits exceeded at the stated drive.');

  const vent = ops.find((w) => w.element === 'vent')!;
  const item = page.locator('.warning-item', { hasText: 'vent · particle velocity' });
  await expect(item).toContainText('limit 1 m/s');
  await item.click();
  await expect(item).toHaveAttribute('aria-pressed', 'true');
  const hl = (await hook(page, (h) => h.highlight()))!;
  expect(hl.lo).toBe(vent.f_min_Hz);
  expect(hl.hi).toBe(vent.f_max_Hz);
  // The band's edges are drawn in every plot, and the crosshair sits on the worst point.
  expect(await page.evaluate(`window.acoustilab.countColor('spl', '--hl-edge', 30)`)).toBeGreaterThan(100);
  expect(await page.evaluate(`window.acoustilab.countColor('imp:electrical:ohm', '--hl-edge', 30)`)).toBeGreaterThan(100);
  const cursor = (await hook(page, (h) => h.cursor()))!;
  expect(Math.abs(Math.log(r.frequencies_Hz[cursor] / vent.at_Hz!))).toBeLessThan(1e-9);
  // Inside the band the tint is present, outside it is not (SPL curves
  // hidden; 150 and 800 Hz sit between gridlines and outside the
  // validity shading, which starts at 100 Hz below and near 1 kHz above).
  expect(vent.f_max_Hz!).toBeGreaterThan(200);
  expect(vent.f_max_Hz!).toBeLessThan(800);
  for (const id of ['p_drp', 'p_front', 'p_rear']) await page.locator(`.legend-item[data-probe="${id}"]`).click();
  const bg = await cssColor(page, '--plot-bg');
  const inside = (await page.evaluate(`window.acoustilab.sample('spl', 150, 0.5, 2)`)) as number[][];
  const outside = (await page.evaluate(`window.acoustilab.sample('spl', 800, 0.5, 2)`)) as number[][];
  expect(inside.filter((p) => close(p, bg)).length).toBe(0);
  // Most of the block (a horizontal gridline may cross it).
  expect(outside.filter((p) => close(p, bg)).length).toBeGreaterThan(12);

  // A second click clears the highlight.
  await item.click();
  await expect(item).toHaveAttribute('aria-pressed', 'false');
  expect(await hook(page, (h) => h.highlight())).toBeNull();
  expect(await page.evaluate(`window.acoustilab.countColor('imp:electrical:ohm', '--hl-edge', 30)`)).toBeLessThan(20);
});

test('low-side shading: below 100 Hz for the IEC 60318-4 ear, none for Type 4.3', async ({ page }) => {
  await open(page);
  const r = (await hook(page, (h) => h.result()))!;
  expect(r.shading.low_begin_hz).toBe(100);
  expect(r.shading.low_deep_hz).toBeNull();
  for (const id of ['p_drp', 'p_front', 'p_rear']) await page.locator(`.legend-item[data-probe="${id}"]`).click();
  const [shade1, bg] = await Promise.all([cssColor(page, '--shade-1'), cssColor(page, '--plot-bg')]);
  // 35 and 250 Hz sit between gridlines.
  const at = async (f: number) => (await page.evaluate(`window.acoustilab.sample('spl', ${f}, 0.5, 3)`)) as number[][];
  expect((await at(35)).filter((p) => close(p, shade1)).length, 'light below 100 Hz').toBeGreaterThan(40);
  expect((await at(250)).filter((p) => close(p, bg)).length, 'clear above 100 Hz').toBeGreaterThan(40);

  // Readout, data table and the per-element table say so too.
  await page.locator('figure.plot canvas').first().focus();
  await page.keyboard.press('Home');
  await expect(page.locator('#readout')).toContainText('in the light band below 100.0 Hz: below the range an element is validated for');
  await page.getByRole('button', { name: 'Show data table' }).click();
  await expect(page.locator('#table-wrap tbody tr').first().locator('td').first()).toHaveText('light');
  await page.locator('#validity-details summary').click();
  const heads = await page.locator('#validity-table thead th').allTextContents();
  expect(heads).toEqual(['Element', 'Criterion', 'Onset', 'Deep', 'Lower onset', 'Lower deep']);
  await expect(page.locator('#validity-table tr', { hasText: 'IEC 60318-4 literature model' })).toContainText('100.0 Hz');

  await page.getByRole('radio', { name: /Type 4\.3/ }).check();
  await solved(page);
  const r2 = (await hook(page, (h) => h.result()))!;
  expect(r2.shading.low_begin_hz ?? null).toBeNull();
  await expect(page.locator('#strip-ear')).toHaveText('ITU-T P.57 Type 4.3 ear (canal and drum)');
  expect((await at(35)).filter((p) => close(p, bg)).length, 'clear at 35 Hz with Type 4.3').toBeGreaterThan(40);
});

test('freeze baselines, Δ readout, difference plot, rename and remove', async ({ page }) => {
  // Oracle: the network is linear, so raising the drive from 1 mW to 10 mW
  // into the same rated impedance raises every level by exactly
  // 10·log10(10) = 10 dB (the EMF scales by sqrt(10)).
  await open(page);
  await page.getByRole('button', { name: 'Freeze current as baseline' }).click();
  expect(await hook(page, (h) => h.baselines())).toEqual([{ id: 1, name: 'template values' }]);
  await expect(page.getByLabel('Name of baseline 1')).toHaveValue('template values');
  expect((await hook(page, (h) => h.groups()))[0].overlays).toContain('p_drp@template values');

  // Change the design: the default name lists what differs from the template.
  await page.locator('#p-drive_mW').fill('10');
  await page.locator('#p-drive_mW').press('Enter');
  await solved(page);
  await page.getByRole('button', { name: 'Freeze current as baseline' }).click();
  expect((await hook(page, (h) => h.baselines())).map((b) => b.name)).toEqual(['template values', 'drive_mW=10']);
  await expect(page.getByRole('button', { name: 'Remove baseline “drive_mW=10”' })).toBeVisible();
  await expect(page.locator('figure.plot canvas').first()).toHaveAttribute('aria-label', /frozen baselines, drawn as thin patterned lines: “template values”, “drive_mW=10”/);

  // Δ readout against the first baseline (the default reference).
  await expect(page.getByLabel('Δ against')).toHaveValue('1');
  await page.locator('figure.plot canvas').first().focus();
  await page.keyboard.press('Home');
  for (let k = 0; k < 3; k++) await page.keyboard.press('Shift+ArrowRight');
  await expect(page.locator('#readout li').first()).toContainText('Δ +10.00 dB vs baseline “template values”');
  // The 1 mW baseline of p_drp is drawn 10 dB below the live curve. The
  // pixel scale comes from two points of the live curve (cursorY); a block
  // 10 dB below the live curve must hold p_drp-coloured pixels (blue well
  // above red: the overlay is anti-aliased, so partly blended), and a
  // control block 10 dB above it none. p_front and p_rear are hidden (their
  // curves run near that level).
  for (const id of ['p_front', 'p_rear']) await page.locator(`.legend-item[data-probe="${id}"]`).click();
  const live = (await hook(page, (h) => h.result()))!;
  const yAt = async () => {
    const box = (await page.evaluate(`window.acoustilab.row('spl', 0.5)`)) as { y0: number; y1: number };
    const i = (await hook(page, (h) => h.cursor()))!;
    const y = (await hook(page, (h) => h.cursorY())).find((p) => p.key === 'spl')!.y.p_drp;
    return { box, i, y, v: live.probes[0].spl_dB![i]! };
  };
  await page.locator('figure.plot canvas').first().focus();
  const a = await yAt(); // about 50 Hz, where the response is flat
  for (let k = 0; k < 10; k++) await page.keyboard.press('Shift+ArrowRight');
  const b = await yAt(); // about 850 Hz, below the coupled resonance
  const pxPerDb = (a.y - b.y) / (b.v - a.v);
  expect(Math.abs(b.v - a.v), 'two distinct levels to scale from').toBeGreaterThan(3);
  const blueAt = async (dy: number) => {
    const t = (a.y + dy - a.box.y0) / (a.box.y1 - a.box.y0);
    const px = (await page.evaluate(`window.acoustilab.sample('spl', ${live.frequencies_Hz[a.i] * 1.03}, ${t}, 2)`)) as number[][];
    return px.filter((p) => p[2] - p[0] > 50).length;
  };
  expect(await blueAt(10 * pxPerDb), 'baseline 10 dB below').toBeGreaterThan(2);
  expect(await blueAt(-10 * pxPerDb), 'nothing 10 dB above').toBe(0);
  for (const id of ['p_front', 'p_rear']) await page.locator(`.legend-item[data-probe="${id}"]`).click();

  // Difference plot, right after the SPL plot: live minus baseline, per probe.
  await page.getByLabel('Difference plot').check();
  const g = await hook(page, (h) => h.groups());
  expect(g.map((x) => x.key).slice(0, 2)).toEqual(['spl', 'delta']);
  expect(g[1].series).toEqual(['p_drp', 'p_front', 'p_rear']);
  await expect(page.locator('figure.plot figcaption').nth(1)).toContainText('SPL difference, current − “template values”');
  const diff = (await page.evaluate(`window.acoustilab.values('delta', 'p_drp')`)) as number[];
  expect(diff.length).toBe(265);
  expect(Math.max(...diff.map((d) => Math.abs(d - 10)))).toBeLessThan(1e-9);
  // Against the second baseline the difference is zero.
  await page.getByLabel('Δ against').selectOption('2');
  const zero = (await page.evaluate(`window.acoustilab.values('delta', 'p_rear')`)) as number[];
  expect(Math.max(...zero.map(Math.abs))).toBe(0);
  await expect(page.locator('#readout li').first()).toContainText('Δ 0.00 dB vs baseline “drive_mW=10”');
  await page.getByLabel('Δ against').selectOption('1');

  // Rename; the new name reaches the Δ picker and the readout.
  await page.getByLabel('Name of baseline 1').fill('1 mW');
  await expect(page.getByLabel('Δ against').locator('option[value="1"]')).toHaveText('1 mW');
  await expect(page.locator('#readout li').first()).toContainText('vs baseline “1 mW”');

  // Remove: the reference moves to the remaining baseline, then none is left.
  await page.getByRole('button', { name: 'Remove baseline “1 mW”' }).click();
  expect((await hook(page, (h) => h.baselines())).map((b) => b.name)).toEqual(['drive_mW=10']);
  await expect(page.getByLabel('Δ against')).toHaveValue('2');
  await page.getByRole('button', { name: 'Remove baseline “drive_mW=10”' }).click();
  expect(await hook(page, (h) => h.baselines())).toEqual([]);
  await expect(page.locator('#baseline-opts')).toBeHidden();
  const g2 = await hook(page, (h) => h.groups());
  expect(g2.map((x) => x.key)).not.toContain('delta');
  expect(g2[0].overlays).toEqual([]);
  await expect(page.getByRole('button', { name: 'Freeze current as baseline' })).toBeFocused();
});

test('a hand edit in the Netlist tab updates the Design tab, and back', async ({ page }) => {
  await open(page);
  await page.getByRole('tab', { name: 'Design', exact: true }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: 'Netlist', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('tab', { name: 'Netlist', exact: true })).toBeFocused();
  await expect(page.locator('#netlist')).toBeVisible();

  await page.locator('#netlist').fill(withValue(TEMPLATE, 'front_depth_mm', '20', '25'));
  await page.getByRole('tab', { name: 'Design', exact: true }).click();
  await expect(page.locator('#p-front_depth_mm')).toHaveValue('25');
  await expect(row(page, 'front_volume_cm3').locator('output')).toHaveText(volume(25));
  // The Design view solves what it shows.
  await solved(page);
  expect((await hook(page, (h) => h.result()))!.meta.parameters.front_depth_mm).toBe(25);
  expect(await hook(page, (h) => h.params())).toMatchObject({ front_depth_mm: 25 });
  await expect(row(page, 'front_depth_mm')).toHaveClass(/changed/);

  // A netlist that cannot be read: the Design tab says so and offers the editor.
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  await page.locator('#netlist').fill(TEMPLATE.replace('"level": "=fidelity",', '"level": "=fidelity",,'));
  await page.getByRole('tab', { name: 'Design', exact: true }).click();
  await expect(page.locator('#design-message')).toContainText('The netlist cannot be read');
  await expect(page.locator('#design-groups')).toBeHidden();
  await page.getByRole('button', { name: 'Open the Netlist tab' }).click();
  await expect(page.getByRole('tab', { name: 'Netlist', exact: true })).toHaveAttribute('aria-selected', 'true');
  await page.locator('#netlist').fill(TEMPLATE);
  await page.getByRole('tab', { name: 'Design', exact: true }).click();
  await expect(page.locator('#design-groups')).toBeVisible();
  await expect(page.locator('#p-front_depth_mm')).toHaveValue(String(D));

  // Detailed view: tolerances, expressions, and "show in netlist".
  await page.getByRole('switch', { name: 'Show detailed parameters' }).click();
  await expect(page.locator('.pgroup:visible .pgroup-name')).toHaveText(['Driver', 'Front cavity', 'Pad and leak', 'Rear', 'Ear', 'Source', 'Model']);
  await expect(row(page, 'driver_Le_uH')).toBeVisible();
  await expect(row(page, 'driver_fs_Hz').locator('.ptol')).toHaveText('Tolerance ±15 % (normal, 2σ), datasheet (Tymphany HPD-40N16PET00-32)');
  await expect(row(page, 'leak_gap_mm').locator('.ptol')).toHaveText('Tolerance ±50 % (log-normal, 2σ), estimate (fit variation)');
  await expect(row(page, 'front_volume_cm3').locator('.pexpr')).toHaveText('pi * front_radius_mm^2 * front_depth_mm / 1000');
  const help = row(page, 'leak_gap_mm').getByRole('button', { name: 'About Pad leak gap (0 = sealed)' });
  await help.click();
  await expect(help).toHaveAttribute('aria-expanded', 'true');
  await expect(row(page, 'leak_gap_mm').locator('.phelp')).toContainText('thermoviscous slit');
  await row(page, 'vent_count').getByRole('button', { name: 'Show Number of rear vents in the netlist' }).click();
  await expect(page.locator('#netlist')).toBeFocused();
  const sel = await page.locator('#netlist').evaluate((t: HTMLTextAreaElement) => t.value.slice(t.selectionStart, t.selectionEnd));
  expect(sel.startsWith('"vent_count": {"value": 1,')).toBe(true);
});

test('race: a Run still solving older text is followed by a solve of the text the Design tab shows', async ({ page }) => {
  await open(page);
  // A Run from the Netlist tab is still solving a dense sweep when the text
  // is edited and the Design tab is shown: the design text is solved after
  // it, and its result is what stays on screen.
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  const slow = TEMPLATE.replace('"points_per_octave": "=points_per_octave"}', '"points_per_octave": 3000}');
  expect(slow).not.toBe(TEMPLATE);
  await page.locator('#netlist').fill(slow);
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solving');
  const edited = withValue(TEMPLATE, 'front_depth_mm', '20', '25');
  await page.locator('#netlist').fill(edited);
  await page.getByRole('tab', { name: 'Design', exact: true }).click();
  await expect
    .poll(async () => (await hook(page, (h) => ({ n: h.result()?.frequencies_Hz.length, d: h.result()?.meta.parameters.front_depth_mm }))), {
      timeout: 30_000,
    })
    .toEqual({ n: 265, d: 25 });
  await solved(page);
});

test('race: a control clicked before a hand edit is described never overwrites the edit', async ({ page }) => {
  // A hand edit, then (in the same task, before the engine has described
  // the new text) a stepper click computed from the old value: the click is
  // dropped rather than overwriting the edit, and the panel then shows the
  // edited value.
  await open(page);
  const edited = withValue(TEMPLATE, 'front_depth_mm', '20', '25');
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  const five = withValue(edited, 'vent_count', '1', '5');
  await page.evaluate((t) => {
    const ed = document.querySelector<HTMLTextAreaElement>('#netlist')!;
    ed.value = t;
    ed.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector<HTMLButtonElement>('#tab-design')!.click();
    document.querySelector<HTMLButtonElement>('button[aria-label="Increase Number of rear vents"]')!.click();
  }, five);
  await expect(page.locator('#p-vent_count')).toHaveValue('5');
  await solved(page);
  expect(await text(page)).toBe(five);
  expect((await hook(page, (h) => h.result()))!.meta.parameters.vent_count).toBe(5);
  // Once described, the stepper works from the edited value.
  await page.getByRole('button', { name: 'Increase Number of rear vents' }).click();
  await solved(page);
  expect(await text(page)).toBe(withValue(edited, 'vent_count', '1', '6'));
});

test('the sketch is drawn to scale and linked to the controls', async ({ page }) => {
  await open(page);
  // Front cavity rectangle: width / height = 2r / depth.
  const ratio = () =>
    page.locator('#sketch-svg [data-part="front"] rect.sk-air').evaluate((r: SVGRectElement) => r.width.baseVal.value / r.height.baseVal.value);
  expect(await ratio()).toBeCloseTo((2 * R) / D, 6);
  const drv = await page.locator('#sketch-svg [data-part="driver"] path').evaluate((p: SVGPathElement) => {
    const b = p.getBBox();
    const f = document.querySelector<SVGRectElement>('#sketch-svg [data-part="front"] rect.sk-air')!;
    return b.width / f.width.baseVal.value;
  });
  expect(drv).toBeCloseTo((2 * Math.sqrt(1000 / Math.PI)) / (2 * R), 3);
  // Every other drawn dimension, relative to the cup diameter (2r = 55 mm):
  // the rear cavity's depth V/(pi r^2) = 10.52 mm, the pad width 15 mm, the
  // vent diameter 3 mm (SVG lengths are single precision: 6 digits).
  const rel = (sel: string, dim: 'width' | 'height') =>
    page.locator(sel).first().evaluate((r: SVGRectElement, dm) => {
      const f = document.querySelector<SVGRectElement>('#sketch-svg [data-part="front"] rect.sk-air')!;
      return r[dm].baseVal.value / f.width.baseVal.value;
    }, dim);
  expect(await rel('#sketch-svg [data-part="rear"] rect.sk-air', 'height')).toBeCloseTo(25000 / (Math.PI * R * R) / (2 * R), 6);
  expect(await rel('#sketch-svg [data-part="pad"] rect.sk-pad', 'width')).toBeCloseTo(PAD / (2 * R), 6);
  expect(await rel('#sketch-svg [data-part="vents"] rect.sk-hole', 'width')).toBeCloseTo(3 / (2 * R), 6);
  // The leak gap (0.08 mm, under a pixel) is drawn enlarged, and says so.
  await expect(page.locator('#sketch-svg')).toContainText('leak gap 0.08 mm (drawn enlarged)');
  await expect(page.locator('#sketch-desc')).toContainText('leak gap 0.08 mm (drawn enlarged).');
  await page.locator('#p-front_depth_mm').fill('25');
  await page.locator('#p-front_depth_mm').press('Enter');
  await solved(page);
  await expect.poll(ratio).toBeCloseTo((2 * R) / 25, 6);

  // Pointing at a control glows the parts it drives, directly or through
  // derived parameters (the rear depth follows the radius at fixed volume).
  const glow = async () => [...new Set((await hook(page, (h) => h.sketch())).glow)].sort();
  await row(page, 'vent_diameter_mm').locator('.plabel').hover();
  await expect.poll(glow).toEqual(['vents']);
  await row(page, 'front_radius_mm').locator('.plabel').hover();
  await expect.poll(glow).toEqual(['front', 'pad', 'rear', 'shell']);
  await row(page, 'driver_Qms').locator('.plabel').hover();
  await expect.poll(glow).toEqual(['driver']);
  // Keyboard focus does the same.
  await page.mouse.move(0, 0);
  await page.locator('#p-leak_gap_mm').focus();
  await expect.poll(glow).toEqual(['leak']);

  // And the other way: pointing at a part marks its controls; a click focuses one.
  await page.locator('#sketch-svg [data-part="pad"] rect').first().hover();
  const marked = await page.locator('.prow.pglow').evaluateAll((rs) => rs.map((r) => (r as HTMLElement).dataset.param));
  expect(marked).toEqual(expect.arrayContaining(['front_radius_mm', 'front_depth_mm', 'pad_width_mm']));
  expect(marked).not.toContain('vent_count');
  await page.locator('#sketch-svg [data-part="leak"] rect').first().click();
  await expect(page.locator('#p-leak_gap_mm')).toBeFocused();

  // Values the sketch reports in its text alternative.
  await page.locator('#p-leak_gap_mm').fill('0');
  await page.locator('#p-leak_gap_mm').press('Enter');
  await solved(page);
  await expect(page.locator('#sketch-desc')).toContainText('sealed (no leak gap)');
  await page.getByRole('button', { name: 'Decrease Number of rear vents' }).click();
  await solved(page);
  await expect(page.locator('#sketch-desc')).toContainText('no vents');
});

test('generic controls: switch, select, entry-only and ungrouped parameters', async ({ page }) => {
  // A hand-written parametric netlist with the kinds the template lacks.
  // Closed form: the source sees the resistor, |Z| = R.
  const net = `{
  "schema": "acoustilab-netlist/0.2",
  "title": "Generic controls",
  "parameters": {
    "R_ohm": 8,
    "wall_loss": {"value": true, "label": "Thermal wall loss", "group": "Cavity"},
    "lossless": {"expr": "!wall_loss", "label": "Lossless walls", "group": "Cavity"},
    "V_cm3": {"value": 2, "min": 0.5, "max": 20, "step": 0.5, "label": "Volume", "group": "Cavity"},
    "resolution": {"value": "coarse", "label": "Frequency resolution", "group": "Sweep", "choices": [
      {"value": "coarse", "label": "6 per octave"}, {"value": "medium", "label": "12 per octave"},
      {"value": "fine", "label": "24 per octave"}, {"value": "finest", "label": "48 per octave"}]}
  },
  "sweep": {"f_min_Hz": 100, "f_max_Hz": 1000,
            "points_per_octave": "=if(resolution == 'coarse', 6, if(resolution == 'medium', 12, if(resolution == 'fine', 24, 48)))"},
  "level": 0,
  "nodes": [{"id": "e1", "domain": "electrical"}, {"id": "a1", "domain": "acoustic"}],
  "elements": [
    {"id": "src", "type": "vsource", "node": "e1", "V_V": 1},
    {"id": "r", "type": "resistor", "node": "e1", "R_ohm": "=R_ohm"},
    {"id": "q", "type": "flow_source", "node": "a1", "U_m3_per_s": 1e-6},
    {"id": "cav", "type": "cavity", "node": "a1", "volume_cm3": "=V_cm3", "wall_loss": "=wall_loss"}
  ],
  "probes": [{"id": "z", "quantity": "impedance", "element": "src"}, {"id": "p", "quantity": "pressure", "node": "a1"}]
}`;
  await open(page);
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  await page.locator('#netlist').fill(net);
  await page.getByRole('tab', { name: 'Design', exact: true }).click();
  await solved(page);
  await expect(page.locator('.pgroup:visible .pgroup-name')).toHaveText(['Parameters', 'Cavity', 'Sweep']);
  // No sketch binding, no sketch.
  await expect(page.locator('#sketch')).toBeHidden();

  // A shorthand number without bounds: a numeric entry, no slider; its
  // label is its name, its unit its suffix. ArrowUp moves it by 1 %.
  await expect(row(page, 'R_ohm').locator('input[type="range"]')).toHaveCount(0);
  await expect(row(page, 'R_ohm').locator('.plabel')).toHaveText('R_ohm');
  await expect(row(page, 'R_ohm').locator('.punit')).toHaveText('Ω');
  await page.locator('#p-R_ohm').press('ArrowUp');
  await solved(page);
  expect(await text(page)).toBe(net.replace('"R_ohm": 8,', '"R_ohm": 8.08,'));
  const z = (await hook(page, (h) => h.result()))!.probes[0] as unknown as { magnitude: number[] };
  for (const m of z.magnitude) expect(Math.abs(m - 8.08)).toBeLessThan(1e-9);

  // Boolean: a switch; the derived negation follows.
  const sw = page.getByRole('switch', { name: 'Thermal wall loss' });
  await expect(sw).toHaveAttribute('aria-checked', 'true');
  await expect(row(page, 'lossless').locator('output')).toHaveText('off');
  await sw.click();
  await solved(page);
  await expect(sw).toHaveAttribute('aria-checked', 'false');
  expect(await text(page)).toContain('"wall_loss": {"value": false, "label"');
  await expect(row(page, 'lossless').locator('output')).toHaveText('on');

  // More than three choices: a select.
  const sel = page.getByLabel('Frequency resolution', { exact: true });
  await expect(sel).toHaveValue('coarse');
  const n6 = (await hook(page, (h) => h.result()))!.frequencies_Hz.length;
  await sel.selectOption('finest');
  await solved(page);
  expect(await text(page)).toContain('"resolution": {"value": "finest", "label"');
  const n48 = (await hook(page, (h) => h.result()))!.frequencies_Hz.length;
  // 100 Hz to 1 kHz is log2(10) = 3.32 octaves; the engine's grid has
  // ceil(octaves × points per octave) + 1 points, both ends included.
  expect(n6).toBe(Math.ceil(6 * Math.log2(10)) + 1);
  expect(n48).toBe(Math.ceil(48 * Math.log2(10)) + 1);

  // An engine error from a design change shows above the (dimmed) plots;
  // "Show in editor" takes it to the Netlist tab, beside the editor.
  await page.locator('#p-R_ohm').fill('-1');
  await page.locator('#p-R_ohm').press('Enter');
  await expect(page.locator('body')).toHaveAttribute('data-state', 'error');
  await expect(page.locator('.plot-panel #error-box')).toBeVisible();
  await expect(page.locator('#error-detail')).toContainText('element "r"');
  await expect(page.locator('#plots')).toHaveClass(/stale/);
  await page.getByRole('button', { name: 'Show in editor' }).click();
  await expect(page.locator('.model-panel #error-box')).toBeVisible();
  expect(await page.locator('#netlist').evaluate((t: HTMLTextAreaElement) => t.value.slice(t.selectionStart, t.selectionEnd))).toContain('"id": "r"');
  await page.getByRole('tab', { name: 'Design', exact: true }).click();
  await page.locator('#p-R_ohm').fill('8');
  await page.locator('#p-R_ohm').press('Enter');
  await solved(page);
  await expect(page.locator('#error-box')).toBeHidden();

  // A netlist without parameters: the Design tab says how to get controls.
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  await page.locator('#netlist').fill(JSON.stringify({ ...JSON.parse(net), parameters: undefined, sweep: { frequencies_Hz: [100] } }).replace(/"=[^"]*"/g, '1'));
  await page.getByRole('tab', { name: 'Design', exact: true }).click();
  await expect(page.locator('#design-message')).toContainText('This netlist declares no parameters');
});

test('section open state is remembered per viewer', async ({ page }) => {
  await open(page);
  const driver = page.locator('.pgroup[data-group="Driver"]');
  await expect(driver).toHaveAttribute('open', '');
  await driver.locator('summary').click();
  await expect(driver).not.toHaveAttribute('open', '');
  await page.reload();
  await solved(page);
  await expect(page.locator('.pgroup[data-group="Driver"]')).not.toHaveAttribute('open', '');
  await expect(page.locator('.pgroup[data-group="Rear"]')).toHaveAttribute('open', '');
});

test('narrow screens: panels stack and nothing scrolls sideways at 390 px', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await open(page);
  const noSideScroll = () => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth);
  expect(await noSideScroll()).toBe(true);
  const model = (await page.locator('.model-panel').boundingBox())!;
  const plots = (await page.locator('.plot-panel').boundingBox())!;
  expect(model.y + model.height).toBeLessThanOrEqual(plots.y);
  await page.getByRole('switch', { name: 'Show detailed parameters' }).click();
  await page.locator('#p-drive_mW').fill('300');
  await page.locator('#p-drive_mW').press('Enter');
  await solved(page);
  await page.getByRole('button', { name: 'Freeze current as baseline' }).click();
  await page.getByRole('button', { name: 'Show data table' }).click();
  await page.locator('#validity-details summary').click();
  expect(await noSideScroll()).toBe(true);
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  expect(await noSideScroll()).toBe(true);
});

for (const scheme of ['light', 'dark'] as const) {
  test(`design mode accessibility (${scheme}): axe clean, every control reachable by keyboard`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: scheme });
    await open(page);
    // Everything open: detailed view, help text, warnings, a baseline and
    // the difference plot, notes, data table and validity limits.
    await page.getByRole('switch', { name: 'Show detailed parameters' }).click();
    await row(page, 'leak_gap_mm').getByRole('button', { name: /About/ }).click();
    await page.locator('#p-drive_mW').fill('300');
    await page.locator('#p-drive_mW').press('Enter');
    await solved(page);
    await page.getByRole('button', { name: 'Freeze current as baseline' }).click();
    await page.getByRole('button', { name: 'Increase Number of rear vents' }).click();
    await solved(page);
    await page.getByLabel('Difference plot').check();
    await page.locator('.warning-item').first().click();
    await page.locator('#notes-details summary').click();
    await page.getByRole('button', { name: 'Show data table' }).click();
    await page.locator('#validity-details summary').click();
    await page.locator('#p-front_radius_mm').fill('99');
    await page.locator('#p-front_radius_mm').press('Enter');
    await expect(row(page, 'front_radius_mm').locator('.pmsg')).not.toBeEmpty();
    const axe = await new AxeBuilder({ page })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
      .analyze();
    expect(axe.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);

    // Tab order from the top reaches every kind of control.
    // Start from the top of the page (a click on the header sets the
    // sequential focus starting point).
    await page.locator('#p-front_radius_mm').press('Escape');
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.locator('.brand h1').click();
    await page.evaluate(() => {
      const w = window as unknown as { focused: string[] };
      w.focused = [];
      document.addEventListener('focusin', (ev) => {
        const a = ev.target as HTMLElement;
        w.focused.push(
          a instanceof HTMLInputElement && a.type === 'radio'
            ? 'radio'
            : a.id || a.tagName.toLowerCase() + (a.classList.length ? `.${a.classList[0]}` : ''),
        );
      });
    });
    for (let k = 0; k < 300; k++) await page.keyboard.press('Tab');
    const seen = new Set(await page.evaluate(() => (window as unknown as { focused: string[] }).focused));
    for (const id of [
      'example-select',
      'run-btn',
      'tab-design',
      'detail-toggle',
      'reset-all',
      'p-front_radius_mm',
      'input.pslider',
      'button.pstep',
      'button.picon',
      'radio',
      'button.linkish',
      'freeze-btn',
      'delta-ref',
      'diff-toggle',
      'input.baseline-name',
      'button.legend-item',
      'canvas',
      'button.warning-item',
    ]) {
      expect([...seen], id).toContain(id);
    }
  });
}
