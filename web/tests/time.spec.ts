// The Time view (time.view.ts): impulse response, phase decision, pole
// table, group delay, attribution job and WAV download. Oracles: closed
// forms of an acoustic RC high-pass and a series resonance, and the
// engine's own reports computed in Node (measure.helpers.ts).

import { expect, test, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { engine, example, noSideScroll, open, openView, runNetlist, viewHook, axe, cssColor, near } from './measure.helpers';

// Acoustic RC high-pass: 1 Pa source, series compliance C, shunt resistance R.
// H(s) = sRC/(1 + sRC), h(t) = δ(t) − (1/RC)·e^{−t/RC}, fc = 100 Hz.
const C = 1e-9;
const RC = 1 / (2 * Math.PI * 100);
const HP = {
  schema: 'acoustilab-netlist/0.2',
  title: 'Acoustic RC high-pass, 100 Hz',
  sweep: { f_min_Hz: 10, f_max_Hz: 20000, points_per_octave: 24 },
  level: 0,
  nodes: [
    { id: 'a', domain: 'acoustic' },
    { id: 'b', domain: 'acoustic' },
  ],
  elements: [
    { id: 'src', type: 'pressure_source', nodes: ['a'], p_Pa: 1 },
    { id: 'c', type: 'acoustic_compliance', nodes: ['a', 'b'], C_m3_per_Pa: C },
    { id: 'r', type: 'acoustic_resistance', nodes: ['b'], R_Pa_s_per_m3: RC / C },
  ],
  probes: [{ id: 'p_out', quantity: 'pressure', node: 'b' }],
};

// Series M–R–C driven by 1 Pa, pressure across C: a second-order low-pass
// with f0 = 1/(2π√(MC)) = 1 kHz and Q = √(M/C)/R = 5.
const F0 = 1000;
const Q = 5;
const W0 = 2 * Math.PI * F0;
const CC = 1e-10;
const M = 1 / (W0 * W0 * CC);
const LP = {
  schema: 'acoustilab-netlist/0.2',
  title: 'Acoustic series resonance, 1 kHz, Q 5',
  sweep: { f_min_Hz: 10, f_max_Hz: 20000, points_per_octave: 24 },
  level: 0,
  nodes: [
    { id: 'a', domain: 'acoustic' },
    { id: 'b', domain: 'acoustic' },
    { id: 'c', domain: 'acoustic' },
  ],
  elements: [
    { id: 'src', type: 'pressure_source', nodes: ['a'], p_Pa: 1 },
    { id: 'm', type: 'acoustic_inertance', nodes: ['a', 'b'], M_kg_per_m4: M },
    { id: 'r', type: 'acoustic_resistance', nodes: ['b', 'c'], R_Pa_s_per_m3: Math.sqrt(M / CC) / Q },
    { id: 'cc', type: 'acoustic_compliance', nodes: ['c'], C_m3_per_Pa: CC },
  ],
  probes: [{ id: 'p_c', quantity: 'pressure', node: 'c' }],
};

/** "1.592 ms" -> 1.592e-3 */
function seconds(text: string): number {
  const m = /^(−?-?[\d.]+) (s|ms|µs|ns)$/.exec(text.trim());
  if (!m) throw new Error(`not a time: ${text}`);
  return Number(m[1].replace('−', '-')) * { s: 1, ms: 1e-3, µs: 1e-6, ns: 1e-9 }[m[2] as 's']!;
}

const readout = (page: Page, figure: string, series: string) =>
  page.locator(`#view-time .mv-figure[data-figure="${figure}"] .readout li[data-series="${series}"] strong`);

async function timeView(page: Page, text?: string): Promise<void> {
  await open(page);
  if (text) await runNetlist(page, text);
  await openView(page, 'Time');
  await expect(page.locator('#view-time [data-field="mode"]')).toBeVisible({ timeout: 30_000 });
  await expect(page.locator('#view-time table.mv-poles')).toBeVisible({ timeout: 30_000 });
}

test('time: the impulse response of an RC high-pass follows its closed form, and the WAV holds it', async ({ page }) => {
  const text = JSON.stringify(HP, null, 2);
  await timeView(page, text);
  const panel = page.locator('#view-time');
  // A minimum-phase network: the decision says so.
  await expect(panel.locator('[data-field="mode"]')).toHaveText('minimum phase');
  const r = await viewHook(page, 'time', 'report');
  const call = await engine();
  const ref = call('impulse', text, '', JSON.stringify({ n: 8192, fs_Hz: 48000, align_delay: false, order: 30 }));
  // The view shows the export's numbers: same buffers, sample for sample.
  expect(r.ir).toEqual(ref.ir);
  expect(r.decision).toEqual(ref.decision);
  expect([r.n, r.fs_Hz, r.dt_s]).toEqual([8192, 48000, 1 / 48000]);

  // h[n] ≈ h(t_n)·dt away from t = 0 (docs/time-domain.md): the band-limited
  // delta has died out after a few samples, the time aliasing (period 171 ms)
  // is e^{-107}. Tolerance 1e-3 of the peak ωc·dt; observed about 1e-5.
  for (const tms of [1, 2, 4, 8]) {
    const m = Math.round((tms * 1e-3 - r.t0_s) / r.dt_s);
    const t = r.t0_s + m * r.dt_s;
    await viewHook(page, 'time', 'setTimeCursor', m);
    const shown = Number((await readout(page, 'time-ir', 'mixed phase').nth(0).textContent())!.replace(' Pa', '').replace('−', '-'));
    const closed = -(1 / RC) * Math.exp(-t / RC) * r.dt_s;
    expect(Math.abs(shown - closed)).toBeLessThan(1e-3 * (r.dt_s / RC));
    // The step settles as e^{-t/RC}; the running sum of the samples is a
    // rectangle rule, low by about dt/(2RC) = 0.65 %.
    const step = Number((await readout(page, 'time-ir', 'mixed phase').nth(1).textContent())!.replace(' Pa', ''));
    expect(Math.abs(step / Math.exp(-t / RC) - 1)).toBeLessThan(0.01);
  }
  // The keyboard move is announced with the values.
  await expect(page.locator('#view-status')).toContainText('mixed phase');

  // WAV: the mixed-phase response, normalised to a peak of 1 as `acoustilab ir --wav` does.
  await panel.getByLabel('WAV of').selectOption('mixed');
  const [dl] = await Promise.all([page.waitForEvent('download'), panel.getByRole('button', { name: 'Download WAV' }).click()]);
  expect(dl.suggestedFilename()).toBe('p_out-mixed-phase-48000Hz-N8192.wav');
  const buf = readFileSync((await dl.path())!);
  const v = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const ascii = (at: number, n: number) => buf.subarray(at, at + n).toString('latin1');
  // The layout of the engine's writer (crates/acoustilab/src/time/wav.rs).
  expect(buf.length).toBe(58 + 4 * 8192);
  expect([ascii(0, 4), ascii(8, 8), ascii(38, 4), ascii(50, 4)]).toEqual(['RIFF', 'WAVEfmt ', 'fact', 'data']);
  expect(v.getUint32(4, true)).toBe(buf.length - 8);
  expect([v.getUint32(16, true), v.getUint16(20, true), v.getUint16(22, true), v.getUint32(24, true), v.getUint32(28, true)]).toEqual([18, 3, 1, 48000, 192000]);
  expect([v.getUint16(32, true), v.getUint16(34, true), v.getUint16(36, true)]).toEqual([4, 32, 0]);
  expect([v.getUint32(42, true), v.getUint32(46, true), v.getUint32(54, true)]).toEqual([4, 8192, 4 * 8192]);
  const peak = Math.max(...ref.ir.map((x: number) => Math.abs(x)));
  let worst = 0;
  for (let i = 0; i < 8192; i++) worst = Math.max(worst, Math.abs(v.getFloat32(58 + 4 * i, true) - Math.fround(ref.ir[i] / peak)));
  expect(worst).toBe(0);
  await expect(panel.getByText(/Wrote p_out-mixed-phase-48000Hz-N8192\.wav/)).toBeVisible();

  // Raw values: the samples as the report has them (float32).
  await panel.getByLabel('Raw values (not normalised to a peak of 1)').check();
  await panel.getByLabel('WAV of').selectOption('minimum');
  const [dl2] = await Promise.all([page.waitForEvent('download'), panel.getByRole('button', { name: 'Download WAV' }).click()]);
  const b2 = readFileSync((await dl2.path())!);
  const v2 = new DataView(b2.buffer, b2.byteOffset, b2.byteLength);
  for (const i of [0, 255, 256, 300, 4000, 8191]) expect(v2.getFloat32(58 + 4 * i, true)).toBe(Math.fround(ref.ir_min_phase[i]));
});

test('time: pole table, marked pole and group delay of a 1 kHz, Q = 5 resonance', async ({ page }) => {
  await timeView(page, JSON.stringify(LP, null, 2));
  const panel = page.locator('#view-time');
  const rows = panel.locator('table.mv-poles tbody tr.mv-resonant');
  await expect(rows).toHaveCount(1);
  // f0, Q and T60 = ln(1000)·2Q/ω0 from the closed form, at the table's rounding.
  const t60 = (Math.log(1000) * 2 * Q) / W0;
  await expect(rows.locator('th')).toHaveText(`${(F0 / 1000).toPrecision(4)} kHz`);
  await expect(rows.locator('td').nth(1)).toHaveText(Q.toPrecision(4));
  await expect(rows.locator('td').nth(2)).toHaveText(`${Number((t60 * 1e3).toPrecision(4))} ms`);
  await expect(rows.locator('td').nth(5)).toHaveText('yes');

  // Group delay of the fit against τ(ω) = (1/(ω0·Q))·(1 + x²)/((1 − x²)² + x²/Q²), x = ω/ω0.
  const p = await viewHook(page, 'time', 'poles');
  const f: number[] = p.frequencies_Hz;
  for (const target of [20, 300, 950, 1000, 1100, 5000]) {
    let i = 0;
    for (let k = 1; k < f.length; k++) if (Math.abs(Math.log(f[k] / target)) < Math.abs(Math.log(f[i] / target))) i = k;
    await viewHook(page, 'time', 'setGdCursor', i);
    const x = f[i] / F0;
    const closed = ((1 / (W0 * Q)) * (1 + x * x)) / ((1 - x * x) ** 2 + (x * x) / (Q * Q));
    const shown = seconds((await readout(page, 'time-gd', 'group delay').textContent())!);
    expect(Math.abs(shown / closed - 1)).toBeLessThan(1e-3);
  }

  // "Mark" picks out the pole's frequency on the frequency plots.
  const mark = rows.getByRole('button', { name: /^Mark 1\.000 kHz/ });
  await page.keyboard.press('Escape'); // no crosshair in the pixel rows below
  await viewHook(page, 'time', 'setGdCursor', null);
  const rect0 = (await viewHook(page, 'time', 'gdFigure')).plots[0].rect;
  // Rows where the curve is away from 1 kHz (it peaks there) and the mark's label is not drawn.
  const ys = [0.61, 0.73, 0.82].map((t) => rect0.y0 + t * (rect0.y1 - rect0.y0));
  const before = await Promise.all(ys.map((y) => viewHook(page, 'time', 'gdRow', 'gd', y)));
  await mark.click();
  await expect(mark).toHaveAttribute('aria-pressed', 'true');
  const fig = await viewHook(page, 'time', 'gdFigure');
  expect(fig.highlight.lo).toBeLessThan(1000);
  expect(fig.highlight.hi).toBeGreaterThan(1000);
  expect(fig.highlight.label).toMatch(/^pole 1\.000 kHz, Q 5\.000/);
  const excess = await viewHook(page, 'time', 'excessFigure');
  expect(excess.highlight.label).toBe(fig.highlight.label);
  // What the mark changes in a pixel row across the plot lies next to x(1 kHz),
  // and its edges are drawn in the highlight colour.
  const x = await viewHook(page, 'time', 'gdX', 'gd', 1000);
  const edge = await cssColor(page, '--hl-edge');
  const bg = await cssColor(page, '--plot-bg');
  const toward = (c: number[]) => [0, 1, 2].reduce((a, j) => a + (c[j] - bg[j]) * (edge[j] - bg[j]), 0) / [0, 1, 2].reduce((a, j) => a + (edge[j] - bg[j]) ** 2, 0);
  let changedRows = 0;
  for (const [n, y] of ys.entries()) {
    const after = await viewHook(page, 'time', 'gdRow', 'gd', y);
    const changed = after.px.map((c: number[], k: number) => (near(c, before[n].px[k], 3) ? null : k / after.dpr)).filter((k: number | null) => k !== null) as number[];
    for (const k of changed) expect(Math.abs(k - x)).toBeLessThan(6);
    if (changed.length && Math.max(...after.px.map(toward)) > 0.5) changedRows++;
  }
  expect(changedRows).toBeGreaterThanOrEqual(2);
  await mark.click();
  await expect(mark).toHaveAttribute('aria-pressed', 'false');
  expect((await viewHook(page, 'time', 'gdFigure')).highlight).toBeNull();
});

test('time: attribution runs as a job with the engine’s numbers, and a cancel leaves the view working', async ({ page }) => {
  await timeView(page);
  const panel = page.locator('#view-time');
  const attr = panel.getByRole('button', { name: 'Attribute resonances to parameters' });

  // Cancel at once: the job says so and the reports stay.
  await attr.click();
  await panel.locator('.mv-job-cancel:visible').click();
  await expect(panel.getByText('Attribution cancelled.')).toBeVisible();
  await expect(panel.locator('table.mv-poles')).toBeVisible();
  await expect(attr).toBeEnabled();

  await attr.click();
  await expect(panel.getByText(/^Attribution done: \d+ parameters perturbed\.$/)).toBeVisible({ timeout: 30_000 });
  const call = await engine();
  const ref = call('vector_fit', example('design_over_ear'), '', JSON.stringify({ order: 30, n: 8192, fs_Hz: 48000, attribute: true })).attribution;
  expect(ref.poles.length).toBeGreaterThan(0);
  for (const pole of ref.poles) {
    const t = panel.locator(`section.mv-attr[data-f="${pole.f_Hz}"] table`);
    const first = pole.parameters[0];
    await expect(t.locator('tbody tr').first().locator('th')).toHaveText(first.parameter);
    await expect(t.locator('tbody tr').first().locator('td').first()).toHaveText(first.dlnf_dlnp.toPrecision(3));
  }
  // docs/time-domain.md: the 936 Hz coupled resonance goes to the diaphragm area first.
  expect(ref.poles[0].parameters[0].parameter).toBe('driver_Sd_cm2');
  await expect(panel.getByText(`Perturbed: ${ref.perturbed.join(', ')}.`, { exact: false })).toBeAttached();
});

test('time: keyboard, announcements, axe in both themes, and 390 px', async ({ page }) => {
  await timeView(page);
  const canvas = page.locator('#view-time .mv-figure[data-figure="time-ir"] canvas').first();
  await canvas.focus();
  await page.keyboard.press('ArrowRight');
  const c1 = (await viewHook(page, 'time', 'timeFigure')).cursor;
  expect(c1).not.toBeNull();
  await page.keyboard.press('Shift+ArrowRight');
  expect((await viewHook(page, 'time', 'timeFigure')).cursor).toBe(c1 + 10);
  await expect(page.locator('#view-status')).toContainText('minimum phase');
  const [lo, hi] = (await viewHook(page, 'time', 'timeFigure')).view;
  await page.keyboard.press('+');
  const [lo2, hi2] = (await viewHook(page, 'time', 'timeFigure')).view;
  expect(hi2 - lo2).toBeCloseTo((hi - lo) / 2, 9);
  await page.keyboard.press('0');
  expect((await viewHook(page, 'time', 'timeFigure')).view).toEqual([lo, hi]);
  await page.keyboard.press('Escape');
  expect((await viewHook(page, 'time', 'timeFigure')).cursor).toBeNull();
  // The data table lists the samples in view.
  await page.locator('#view-time .mv-figure[data-figure="time-ir"]').getByRole('button', { name: 'Show data table' }).click();
  await expect(page.locator('#view-time .mv-figure[data-figure="time-ir"] table thead th').first()).toHaveText('Time (ms)');

  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    expect(await axe(page), theme).toEqual([]);
  }
  await page.setViewportSize({ width: 390, height: 844 });
  await noSideScroll(page);
});
