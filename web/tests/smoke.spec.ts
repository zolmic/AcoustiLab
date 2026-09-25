// Smoke tests of the built UI in Chromium: load, solve an example in the
// worker, check curves and validity shading are drawn, the crosshair,
// legend, data table, error display, cancel and accessibility.
//
// `UPDATE_DOCS_SCREENSHOT=1` also writes docs/img/web-ui.png.

import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../../', import.meta.url));

interface Hook {
  result(): {
    frequencies_Hz: number[];
    shading: { begin_hz: number | null; deep_hz: number | null };
    probes: { id: string; spl_dB?: (number | null)[]; magnitude: (number | null)[] }[];
    meta: { level: number };
  } | null;
  groups(): { key: string; kind: string; title: string; scale: string; series: string[] }[];
  view(): [number, number];
  cursor(): number | null;
  cursorY(): { key: string; y: Record<string, number> }[];
  sample(key: string, f: number, t: number, h?: number): number[][];
  countColor(key: string, cssVar: string, tol?: number): number;
}

const hook = <T>(page: Page, fn: (h: Hook) => T): Promise<T> =>
  page.evaluate(`(${fn.toString()})(window.acoustilab)`) as Promise<T>;

async function solved(page: Page): Promise<void> {
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solved', { timeout: 20_000 });
}

async function setNetlist(page: Page, text: string): Promise<void> {
  await page.locator('#netlist').fill(text);
}

async function cssColor(page: Page, name: string): Promise<number[]> {
  const hex = await page.evaluate((n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim(), name);
  return [1, 3, 5].map((k) => parseInt(hex.slice(k, k + 2), 16));
}

const close = (a: number[], b: number[], tol = 6) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]) <= tol;

test('solves an example in the worker and draws curves with validity shading', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/');
  await solved(page);

  // Every examples/*.json of the repository is offered.
  const files = readdirSync(`${repo}/examples`).filter((f) => f.endsWith('.json')).map((f) => f.replace(/\.json$/, ''));
  const offered = await page.locator('#example-select option').evaluateAll((os) => os.map((o) => (o as HTMLOptionElement).value));
  expect(offered.sort()).toEqual(files.sort());

  await page.selectOption('#example-select', 'sealed_cup');
  await solved(page);
  await page.getByRole('button', { name: 'Run' }).click();
  await solved(page);
  await expect(page.locator('#run-status')).toContainText('Solved 265 frequencies × 4 probes');

  // One plot per quantity: SPL, |Z| and its phase, displacement, volume velocity.
  const groups = await hook(page, (h) => h.groups());
  expect(groups.map((g) => [g.kind, g.series.join()])).toEqual([
    ['spl', 'p_front'],
    ['mag', 'zin'],
    ['phase', 'zin'],
    ['mag', 'x'],
    ['mag', 'u_leak'],
  ]);
  await expect(page.locator('figure.plot canvas')).toHaveCount(5);
  await expect(page.locator('figure.plot figcaption').first()).toContainText('Sound pressure level');
  await expect(page.locator('figure.plot figcaption').nth(1)).toContainText('Electrical impedance |Z| (Ω)');

  // Persistent strip.
  await expect(page.locator('#theory-badge')).toContainText('THEORY ONLY - not validated against measurements');
  await expect(page.locator('#strip-air')).toContainText('23.0 °C, 101.325 kPa');
  await expect(page.locator('#strip-level')).toHaveText('L1 distributed');
  await expect(page.locator('#strip-drive')).toContainText('amp (vsource: V_V 1, Zs_ohm 0)');
  await expect(page.locator('#strip-engine')).toContainText('acoustilab');

  // Default view 20 Hz to 20 kHz on the 10 Hz to 40 kHz axis.
  expect(await hook(page, (h) => h.view())).toEqual([20, 20000]);

  // Curves: each probe's colour appears in its plot. No background, grid or
  // shading pixel has a series colour, so a count of 100 (even the dotted
  // line has several hundred) means the curve was drawn.
  for (const [key, slot] of [
    ['spl', 1],
    ['imp:electrical:ohm', 2],
    ['imp:electrical:ohm:phase', 2],
    ['mag:m', 3],
    ['mag:m^3/s', 4],
  ] as const) {
    const n = await page.evaluate(`window.acoustilab.countColor(${JSON.stringify(key)}, '--series-${slot}', 40)`);
    expect(n, `${key} curve pixels`).toBeGreaterThan(100);
  }

  // Shading from result.shading: light band between begin_hz and deep_hz,
  // darker above deep_hz, plain surface below begin_hz. Sampled in the SPL
  // plot with its curve hidden, so only background and shading remain.
  const r = (await hook(page, (h) => h.result()))!;
  await page.locator('.legend-item[data-probe="p_front"]').click();
  const { begin_hz, deep_hz } = r.shading;
  expect(begin_hz).not.toBeNull();
  expect(deep_hz).not.toBeNull();
  const [shade1, shade2, bg] = await Promise.all([
    cssColor(page, '--shade-1'),
    cssColor(page, '--shade-2'),
    cssColor(page, '--plot-bg'),
  ]);
  const mode = async (f: number) => {
    const px = (await page.evaluate(`window.acoustilab.sample('spl', ${f}, 0.65, 3)`)) as number[][];
    return [shade1, shade2, bg].map((c) => px.filter((p) => close(p, c)).length);
  };
  const light = await mode(Math.sqrt(begin_hz! * deep_hz!) * 1.03);
  const dark = await mode(deep_hz! * 4.1);
  const clear = await mode(begin_hz! / 3.3);
  expect(light[0], 'light band').toBeGreaterThan(30);
  expect(dark[1], 'dark band').toBeGreaterThan(30);
  expect(clear[2], 'unshaded').toBeGreaterThan(30);
  expect(light[1] + light[2]).toBeLessThan(10);
  expect(dark[0] + dark[2]).toBeLessThan(10);

  // The samples above go through the plot's own frequency-to-x mapping, so
  // they would agree with a wrong mapping. Locate the band edges and the
  // decade gridlines in a raw pixel row instead, and convert with a log axis
  // over the plot box computed here: x = x0 + (x1 - x0)·ln(f/lo)/ln(hi/lo).
  const [lo, hi] = await hook(page, (h) => h.view());
  const row = (await page.evaluate(`window.acoustilab.row('spl', 0.65)`)) as {
    dpr: number;
    x0: number;
    x1: number;
    px: number[][];
  };
  const xOfF = (f: number) => row.x0 + ((row.x1 - row.x0) * Math.log(f / lo)) / Math.log(hi / lo);
  const fOfX = (x: number) => lo * (hi / lo) ** ((x - row.x0) / (row.x1 - row.x0));
  const firstX = (c: number[]) => {
    for (let k = Math.ceil(row.x0 * row.dpr); k < row.x1 * row.dpr; k++) if (close(row.px[k], c, 3)) return k / row.dpr;
    return NaN;
  };
  // Edges within 2.5 px (anti-aliased fill edge plus the 1 px edge line).
  const pxPerLn = (row.x1 - row.x0) / Math.log(hi / lo);
  for (const [x, f] of [
    [firstX(shade1), begin_hz!],
    [firstX(shade2), deep_hz!],
  ] as const) {
    expect(Math.abs(Math.log(fOfX(x) / f)) * pxPerLn, `band edge at ${f} Hz`).toBeLessThan(2.5);
    expect(x).toBeGreaterThanOrEqual(xOfF(f) - 0.5);
  }
  const grid = await cssColor(page, '--grid');
  for (const f of [100, 1000, 10000]) {
    const k = Math.round(xOfF(f) * row.dpr - 0.5);
    const hit = [k - 1, k, k + 1].some((j) => close(row.px[j], grid, 3));
    expect(hit, `decade gridline at ${f} Hz`).toBe(true);
  }
  await page.locator('.legend-item[data-probe="p_front"]').click();
  expect(await page.evaluate(`window.acoustilab.countColor('spl', '--series-1', 40)`)).toBeGreaterThan(100);

  // Validity limits table lists the elements the shading came from.
  await page.locator('#validity-details summary').click();
  await expect(page.locator('#validity-table')).toContainText('lumped cavity kL');
  expect(errors).toEqual([]);
});

test('crosshair readout, legend toggles, zoom and data table', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const r = (await hook(page, (h) => h.result()))!;

  // Keyboard crosshair on the SPL plot.
  const canvas = page.locator('figure.plot canvas').first();
  await canvas.focus();
  await page.keyboard.press('Home');
  const i0 = (await hook(page, (h) => h.cursor()))!;
  expect(r.frequencies_Hz[i0]).toBeGreaterThanOrEqual(20);
  expect(r.frequencies_Hz[i0 - 1]).toBeLessThan(20);
  await page.keyboard.press('Shift+ArrowRight');
  const i1 = (await hook(page, (h) => h.cursor()))!;
  expect(i1).toBe(i0 + 10);
  const readout = page.locator('#readout');
  for (const id of ['p_front', 'zin', 'x', 'u_leak']) await expect(readout).toContainText(id);
  const spl = r.probes[0].spl_dB![i1]!;
  await expect(readout).toContainText(`${spl.toFixed(2)} dB SPL`);
  await expect(page.locator('#sr-readout')).toContainText('p_front');
  // A marker is drawn on every curve at the cursor.
  const ys = await hook(page, (h) => h.cursorY());
  expect(ys.every((p) => Object.keys(p.y).length === 1)).toBe(true);

  // Pointer crosshair.
  const box = (await canvas.boundingBox())!;
  await page.mouse.move(box.x + box.width * 0.5, box.y + box.height * 0.5);
  await expect.poll(() => hook(page, (h) => h.cursor())).not.toBe(i1);

  // Legend toggle (keyboard): hide the displacement curve.
  const xBtn = page.locator('.legend-item[data-probe="x"]');
  await xBtn.focus();
  await page.keyboard.press('Space');
  await expect(xBtn).toHaveAttribute('aria-pressed', 'false');
  expect(await page.evaluate(`window.acoustilab.countColor('mag:m', '--series-3', 40)`)).toBeLessThan(20);
  await expect(page.locator('.readout-name', { hasText: /^x ·/ })).toHaveCount(0);
  await xBtn.click();
  await expect(xBtn).toHaveAttribute('aria-pressed', 'true');
  expect(await page.evaluate(`window.acoustilab.countColor('mag:m', '--series-3', 40)`)).toBeGreaterThan(100);

  // Zoom with the keyboard, then back to the default view.
  await canvas.focus();
  await page.keyboard.press('+');
  const [lo, hi] = await hook(page, (h) => h.view());
  expect(Math.log(hi / lo)).toBeCloseTo(Math.log(1000) / 2, 5);
  await page.keyboard.press('0');
  expect(await hook(page, (h) => h.view())).toEqual([20, 20000]);
  await page.getByRole('button', { name: '10 Hz–40 kHz' }).click();
  expect(await hook(page, (h) => h.view())).toEqual([10, 40000]);
  await expect(page.getByRole('button', { name: '10 Hz–40 kHz' })).toHaveAttribute('aria-pressed', 'true');

  // Data table of the plotted values in view (whole sweep at full view).
  await page.getByRole('button', { name: 'Show data table' }).click();
  const rows = page.locator('#table-wrap tbody tr');
  await expect(rows).toHaveCount(r.frequencies_Hz.length);
  const headers = await page.locator('#table-wrap thead th').allTextContents();
  expect(headers).toEqual([
    'Frequency (Hz)',
    'Validity',
    'p_front SPL (dB SPL)',
    'zin |Z| (Ω)',
    'zin ∠Z (°)',
    'x |x| (m)',
    'u_leak |U| (m³/s)',
  ]);
  const k = 100;
  const cells = await rows.nth(k).locator('td').allTextContents();
  expect(Number(cells[1])).toBeCloseTo(r.probes[0].spl_dB![k]!, 2);
  expect(Number(cells[2])).toBeCloseTo(r.probes[1].magnitude[k]!, 2);
  const f = r.frequencies_Hz;
  const validity = (i: number) =>
    f[i] >= r.shading.deep_hz! ? 'dark' : f[i] >= r.shading.begin_hz! ? 'light' : '';
  for (const i of [0, 150, 200, 264]) expect((await rows.nth(i).locator('td').first().textContent()) ?? '').toBe(validity(i));
});

test('closed-form netlist: the UI reads the engine values with the right units', async ({ page }) => {
  // 8 Ω resistor on a 1 V source, and a 1 cm³/s flow source into a
  // lossless 1 cm³ cavity with the spec_reference air (ρ = 1.204 kg/m³,
  // c = 343 m/s): |Z| = 8 Ω and |p| = U/(ωC), C = V/(ρc²), RMS. Tolerance:
  // the displayed rounding (0.01 dB, 4 significant figures).
  const net = {
    schema: 'acoustilab-netlist/0.1',
    title: 'Closed-form check',
    air: { preset: 'spec_reference' },
    sweep: { frequencies_Hz: [100, 1000, 10000] },
    level: 0,
    nodes: [
      { id: 'e1', domain: 'electrical' },
      { id: 'a1', domain: 'acoustic' },
    ],
    elements: [
      { id: 'src', type: 'vsource', node: 'e1', V_V: 1 },
      { id: 'r', type: 'resistor', node: 'e1', R_ohm: 8 },
      { id: 'q', type: 'flow_source', node: 'a1', U_m3_per_s: 1e-6 },
      { id: 'cav', type: 'cavity', node: 'a1', volume_cm3: 1, wall_loss: false },
    ],
    probes: [
      { id: 'p', quantity: 'pressure', node: 'a1' },
      { id: 'z', quantity: 'impedance', element: 'src' },
    ],
  };
  await page.goto('/');
  await solved(page);
  await setNetlist(page, JSON.stringify(net, null, 2));
  await page.locator('#netlist').press('Control+Enter');
  await expect(page.locator('#run-status')).toContainText('Solved 3 frequencies × 2 probes');
  await expect(page.locator('#result-title')).toHaveText('Closed-form check');
  await expect(page.locator('#strip-level')).toHaveText('L0 lumped');
  await expect(page.locator('#strip-air')).toContainText('20.0 °C');

  const C = 1e-6 / (1.204 * 343 * 343);
  const spl = (f: number) => 20 * Math.log10(1e-6 / (2 * Math.PI * f * C) / 20e-6);
  expect(spl(1000)).toBeCloseTo(121.04, 2);

  const canvas = page.locator('figure.plot canvas').first();
  await canvas.focus();
  await page.keyboard.press('Home'); // 100 Hz
  await page.keyboard.press('ArrowRight'); // 1 kHz
  const readout = page.locator('#readout');
  await expect(readout).toContainText('1.000 kHz');
  await expect(readout).toContainText(`${spl(1000).toFixed(2)} dB SPL`);
  await expect(readout).toContainText('8.000 Ω, ∠ 0.0°');

  await page.getByRole('button', { name: 'Show data table' }).click();
  const rows = page.locator('#table-wrap tbody tr');
  await expect(rows).toHaveCount(3);
  for (const [i, f] of [100, 1000, 10000].entries()) {
    const cells = await rows.nth(i).locator('td').allTextContents();
    expect(Number(cells[1])).toBeCloseTo(spl(f), 2);
    expect(Number(cells[2])).toBeCloseTo(8, 6);
    expect(Math.abs(Number(cells[3]))).toBeLessThan(1e-6);
  }
});

test('a linear magnitude axis starts at zero, not at a padded negative value', async ({ page }) => {
  // Two current probes sharing the ampere plot: one source delivers exactly
  // 0 A, the other 0.125 A. The values span less than a decade, so the axis
  // is linear; a zero magnitude must sit on the bottom edge of the plot.
  const net = {
    sweep: { frequencies_Hz: [100, 1000] },
    nodes: [
      { id: 'e1', domain: 'electrical' },
      { id: 'e2', domain: 'electrical' },
    ],
    elements: [
      { id: 'i0', type: 'isource', node: 'e1', I_A: 0 },
      { id: 'r0', type: 'resistor', node: 'e1', R_ohm: 8 },
      { id: 'i1', type: 'isource', node: 'e2', I_A: 0.125 },
      { id: 'r1', type: 'resistor', node: 'e2', R_ohm: 8 },
    ],
    probes: [
      { id: 'zero', quantity: 'current', element: 'i0' },
      { id: 'eighth', quantity: 'current', element: 'i1' },
    ],
  };
  await page.goto('/');
  await solved(page);
  await setNetlist(page, JSON.stringify(net, null, 2));
  await page.locator('#netlist').press('Control+Enter');
  await expect(page.locator('#run-status')).toContainText('Solved 2 frequencies × 2 probes');
  const groups = await hook(page, (h) => h.groups());
  expect(groups.map((g) => [g.key, g.scale, g.series.join()])).toEqual([['mag:A', 'linear', 'zero,eighth']]);
  await page.locator('figure.plot canvas').first().focus();
  await page.keyboard.press('Home');
  // row() redraws synchronously, so cursorY() then reflects this cursor.
  const box = (await page.evaluate(`window.acoustilab.row('mag:A', 0.5)`)) as { y0: number; y1: number };
  const y = (await hook(page, (h) => h.cursorY()))[0].y;
  expect(y.zero).toBeCloseTo(box.y1, 6);
  expect(y.eighth).toBeGreaterThan(box.y0);
  expect(y.eighth).toBeLessThan(box.y1 - 0.5 * (box.y1 - box.y0));
});

test('malformed netlists: the error names the element and jumps to it', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const good = await page.locator('#netlist').inputValue();

  // A key without a unit suffix on the "front" cavity.
  const bad = good.replace('"volume_cm3": 30', '"volume": 30');
  expect(bad).not.toBe(good);
  await setNetlist(page, bad);
  await expect(page.locator('#check-status')).toContainText('Not valid');
  await expect(page.locator('#check-status')).toContainText('element "front"');
  await page.getByRole('button', { name: 'Run' }).click();
  const box = page.locator('#error-box');
  await expect(box).toBeVisible();
  await expect(box).toHaveAttribute('role', 'alert');
  await expect(page.locator('#error-message')).toContainText("element 'front'");
  await expect(page.locator('#error-detail')).toContainText('element "front"');
  await expect(page.locator('#plots')).toHaveClass(/stale/);
  await page.getByRole('button', { name: 'Show in editor' }).click();
  const sel = await page.locator('#netlist').evaluate((t: HTMLTextAreaElement) => [
    document.activeElement === t,
    t.value.slice(t.selectionStart, t.selectionEnd),
  ]);
  expect(sel[0]).toBe(true);
  expect(sel[1]).toContain('"id": "front"');

  // Unknown element type names both the element and the type.
  await setNetlist(page, good.replace('"type": "coil"', '"type": "coill"'));
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(page.locator('#error-message')).toContainText("unknown element type 'coill' (element 'coil')");

  // JSON syntax error: jumps to the reported line.
  const lines = good.split('\n');
  lines[3] = lines[3] + ',,';
  await setNetlist(page, lines.join('\n'));
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(page.locator('#error-detail')).toContainText('line 4');
  await page.getByRole('button', { name: 'Show in editor' }).click();
  const selected = await page
    .locator('#netlist')
    .evaluate((t: HTMLTextAreaElement) => t.value.slice(t.selectionStart, t.selectionEnd));
  expect(selected).toBe(lines[3]);

  // Fixing the netlist clears the error on the next run.
  await setNetlist(page, good);
  await page.getByRole('button', { name: 'Run' }).click();
  await solved(page);
  await expect(page.locator('#error-box')).toBeHidden();
  await expect(page.locator('#plots')).not.toHaveClass(/stale/);

  // A probe on a port the element does not have: the engine only notices
  // while solving. Run at once (Ctrl+Enter), before the debounced live check
  // fires; the check that lands afterwards must agree with the run and must
  // not clear its error box.
  const badPort = JSON.parse(good);
  badPort.probes.push({ id: 'z_bad', quantity: 'impedance', element: 'coil', port: 3 });
  await setNetlist(page, JSON.stringify(badPort, null, 2));
  await page.locator('#netlist').press('Control+Enter');
  await expect(page.locator('body')).toHaveAttribute('data-state', 'error');
  await expect(page.locator('#error-message')).toContainText("probe 'z_bad'");
  await expect(page.locator('#check-status')).toContainText('Not valid');
  await expect(page.locator('#check-status')).toContainText('probe "z_bad"');
  await page.waitForTimeout(400);
  await expect(page.locator('#error-box')).toBeVisible();
  await expect(page.locator('#plots')).toHaveClass(/stale/);
  await page.getByRole('button', { name: 'Show in editor' }).click();
  expect(
    await page.locator('#netlist').evaluate((t: HTMLTextAreaElement) => t.value.slice(t.selectionStart, t.selectionEnd)),
  ).toContain('"id": "z_bad"');
});

test('a run error the live check cannot see stays until the text changes', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  // A current source into a floating node: well-formed (the check passes),
  // but the system is singular, which only the solve can find.
  const net = {
    sweep: { frequencies_Hz: [100, 1000] },
    nodes: [
      { id: 'e1', domain: 'electrical' },
      { id: 'e2', domain: 'electrical' },
    ],
    elements: [{ id: 'i1', type: 'isource', nodes: ['e1', 'e2'], I_A: 1 }],
    probes: [{ id: 'v', quantity: 'voltage', node: 'e1' }],
  };
  await setNetlist(page, JSON.stringify(net, null, 2));
  await page.locator('#netlist').press('Control+Enter');
  await expect(page.locator('body')).toHaveAttribute('data-state', 'error');
  await expect(page.locator('#error-message')).toContainText('singular');
  await expect(page.locator('#check-status')).toContainText('Valid:');
  await page.waitForTimeout(400);
  await expect(page.locator('#error-box')).toBeVisible();
});

test('an engine panic is reported and the next run uses a fresh engine', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const good = await page.locator('#netlist').inputValue();
  // An absurd grid density overflows the frequency vector's capacity in the
  // engine (a Rust panic, which traps the wasm instance). This relies on the
  // engine not yet bounding the sweep size in grid.rs; once it rejects such
  // sweeps with an ordinary error, this test needs another panic trigger.
  const bad = JSON.parse(good);
  bad.sweep = { f_min_Hz: 10, f_max_Hz: 20000, points_per_octave: 1e300 };
  await setNetlist(page, JSON.stringify(bad));
  await expect(page.locator('#check-status')).toContainText('capacity overflow');
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(page.locator('body')).toHaveAttribute('data-state', 'error');
  await expect(page.locator('#error-message')).toContainText('engine panic');
  await expect(page.locator('#error-message')).toContainText('capacity overflow');
  // Both the solve and the live-check workers recover.
  await setNetlist(page, good);
  await page.getByRole('button', { name: 'Run' }).click();
  await solved(page);
  await expect(page.locator('#check-status')).toContainText('Valid:');
  await setNetlist(page, JSON.stringify(bad));
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(page.locator('#error-message')).toContainText('capacity overflow');
  await setNetlist(page, good);
  await page.getByRole('button', { name: 'Run' }).click();
  await solved(page);
});

test('a worker script that cannot load is reported once, not respawned in a loop', async ({ page }) => {
  await page.addInitScript(() => {
    const W = window.Worker;
    const w = window as unknown as { workersMade: number };
    w.workersMade = 0;
    window.Worker = class extends W {
      constructor(url: string | URL, opts?: WorkerOptions) {
        super(url, opts);
        w.workersMade++;
      }
    };
  });
  await page.route('**/assets/worker-*.js', (route) => route.fulfill({ status: 404, body: '' }));
  await page.goto('/');
  await expect(page.locator('body')).toHaveAttribute('data-state', 'error');
  await expect(page.locator('#error-box')).toBeVisible();
  await page.waitForTimeout(1000);
  // Solver and checker each start one worker per request (version, check,
  // solve), and none after a failure until the next request.
  const made = await page.evaluate(() => (window as unknown as { workersMade: number }).workersMade);
  expect(made).toBeGreaterThanOrEqual(1);
  expect(made).toBeLessThanOrEqual(4);
});

test('loading an example asks before replacing an edited netlist', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const edited = (await page.locator('#netlist').inputValue()).replace('"V_V": 1.0', '"V_V": 2.0');
  await setNetlist(page, edited);
  page.once('dialog', (d) => void d.dismiss());
  await page.selectOption('#example-select', 'sealed_cup');
  await expect(page.locator('#netlist')).toHaveValue(edited);
  page.once('dialog', (d) => void d.accept());
  await page.selectOption('#example-select', 'sealed_cup');
  await expect(page.locator('#netlist')).not.toHaveValue(edited);
  await solved(page);
});

test('a long solve runs in the worker: the page stays responsive and can cancel', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const good = await page.locator('#netlist').inputValue();
  const big = JSON.parse(good);
  big.sweep = { f_min_Hz: 10, f_max_Hz: 40000, points_per_octave: 40000 }; // ~480k frequencies
  await setNetlist(page, JSON.stringify(big));
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solving');
  await expect(page.getByRole('button', { name: 'Cancel' })).toBeEnabled();
  // The main thread answers promptly while the worker solves.
  for (let k = 0; k < 5; k++) {
    const t0 = Date.now();
    await page.evaluate(() => 1 + 1);
    expect(Date.now() - t0).toBeLessThan(500);
  }
  await page.locator('.legend-item').first().click();
  await expect(page.locator('.legend-item').first()).toHaveAttribute('aria-pressed', 'false');
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solving');
  await page.getByRole('button', { name: 'Cancel' }).click();
  await expect(page.locator('#run-status')).toHaveText('Cancelled.');
  await expect(page.getByRole('button', { name: 'Cancel' })).toBeDisabled();
  // A fresh worker solves the next run.
  await setNetlist(page, good);
  await page.getByRole('button', { name: 'Run' }).click();
  await solved(page);
});

test('a dense sweep with the data table open does not freeze the page', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  await page.getByRole('button', { name: 'Show data table' }).click();
  const big = JSON.parse(await page.locator('#netlist').inputValue());
  big.sweep = { f_min_Hz: 10, f_max_Hz: 40000, points_per_octave: 4000 }; // ~48k frequencies
  await setNetlist(page, JSON.stringify(big));
  // Longest gap between 10 ms timer ticks on the main thread. Building one
  // table row per grid point took over 10 s here.
  await page.evaluate(() => {
    const w = window as unknown as { maxGap: number };
    w.maxGap = 0;
    let last = performance.now();
    const tick = () => {
      const t = performance.now();
      w.maxGap = Math.max(w.maxGap, t - last);
      last = t;
      setTimeout(tick, 10);
    };
    tick();
  });
  await page.getByRole('button', { name: 'Run' }).click();
  await solved(page);
  await page.getByRole('button', { name: 'Zoom out' }).click();
  await page.getByRole('button', { name: '20 Hz–20 kHz' }).click();
  const gap = await page.evaluate(() => (window as unknown as { maxGap: number }).maxGap);
  expect(gap, 'longest main-thread block (ms)').toBeLessThan(2000);

  // The table lists at most 1000 rows plus the last point in view, says so,
  // and starts and ends at the first and last grid points in the view.
  // Only the grid is fetched: shipping the whole ~1M-number result out of
  // the page would dominate the test's run time.
  const f = await hook(page, (h) => h.result()!.frequencies_Hz);
  const first = f.findIndex((x) => x >= 20 * (1 - 1e-9));
  let last = f.length - 1;
  while (f[last] > 20000 * (1 + 1e-9)) last--;
  const rows = page.locator('#table-wrap tbody tr');
  const n = await rows.count();
  expect(n).toBeLessThanOrEqual(1001);
  expect(n).toBeGreaterThan(900);
  await expect(page.locator('#table-wrap caption')).toContainText(`(${last - first + 1} rows; ${n} shown`);
  expect(Number(await rows.first().locator('th').textContent())).toBeCloseTo(f[first], 2);
  expect(Number(await rows.last().locator('th').textContent())).toBeCloseTo(f[last], 1);
});

for (const scheme of ['light', 'dark'] as const) {
  test(`accessibility (${scheme}): no axe violations, keyboard reaches every control`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: scheme });
    await page.goto('/');
    await solved(page);
    await page.getByRole('button', { name: 'Show data table' }).click();
    await page.locator('#validity-details summary').click();
    const axe = await new AxeBuilder({ page })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
      .analyze();
    expect(axe.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);

    // Tab order reaches the editor, Run, a plot and the legend.
    const seen = new Set<string>();
    for (let k = 0; k < 40; k++) {
      await page.keyboard.press('Tab');
      seen.add(
        await page.evaluate(() => {
          const a = document.activeElement as HTMLElement | null;
          if (!a) return '';
          return a.id || a.tagName.toLowerCase() + (a.classList.length ? `.${a.classList[0]}` : '');
        }),
      );
    }
    for (const id of ['example-select', 'run-btn', 'netlist', 'button.legend-item', 'canvas']) {
      expect([...seen], id).toContain(id);
    }
  });
}

test('screenshot for docs/img/web-ui.png', async ({ page }) => {
  test.skip(!process.env.UPDATE_DOCS_SCREENSHOT, 'set UPDATE_DOCS_SCREENSHOT=1 to refresh the docs screenshot');
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.emulateMedia({ colorScheme: 'light' });
  await page.goto('/');
  await solved(page);
  const canvas = page.locator('figure.plot canvas').first();
  await canvas.focus();
  const r = (await hook(page, (h) => h.result()))!;
  const target = r.frequencies_Hz.findIndex((f) => f >= 400);
  await page.keyboard.press('Home');
  const start = (await hook(page, (h) => h.cursor()))!;
  for (let k = start; k < target; k++) await page.keyboard.press('ArrowRight');
  await page.locator('#netlist').evaluate((t: HTMLTextAreaElement) => t.scrollTo(0, 0));
  await canvas.evaluate((c: HTMLElement) => c.blur());
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${repo}/docs/img/web-ui.png` });
});
