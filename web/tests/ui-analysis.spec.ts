// Analysis in the UI: the Response tab's readouts, the inactive-parameter
// marks of the Design tab, and the Sensitivity, Tolerance and Target views.
//
// Oracles, independent of the page's own code paths:
// * closed forms: a lossless sealed cavity driven by a constant volume
//   velocity has p ∝ 1/V, so its level falls by exactly 20/ln 10/100 dB per
//   % of volume at every frequency, and its tornado ends are
//   20·log10(1/0.9) and 20·log10(1/1.1) dB;
// * the engine's exports called here in Node on the same wasm build, with
//   the same inputs: `readouts`, `parameters`, `sensitivity` (also by the
//   other method, complete solves, within the tolerance docs/analysis.md
//   states), `tornado`, `explain`, `mc_plan`/`mc_run`/`mc_envelope`,
//   `target_metrics`, `import_target_csv`;
// * what the page shows, read from the DOM (text, data attributes) and the
//   read-only hooks `window.acoustilab` and `window.acoustilabAnalysis`.

import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { formatHz } from '../src/format';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const TEMPLATE = readFileSync(`${repo}examples/design_over_ear.json`, 'utf8');

// ----- the engine in Node ------------------------------------------------------

type Engine = Record<string, (...a: string[]) => string>;
let engine: Engine | null = null;
/**
 * The wasm-bindgen glue is an ES module in a directory without
 * `"type": "module"`, so the test loader would read it as CommonJS: it is
 * imported from an `.mjs` copy, and the wasm bytes are given to it directly.
 */
async function eng(): Promise<Engine> {
  if (!engine) {
    const dir = mkdtempSync(join(tmpdir(), 'acoustilab-engine-'));
    const glue = join(dir, 'acoustilab_wasm.mjs');
    writeFileSync(glue, readFileSync(`${repo}crates/acoustilab-wasm/pkg/acoustilab_wasm.js`));
    const m = (await import(pathToFileURL(glue).href)) as Engine & {
      initSync(o: { module: Uint8Array }): void;
    };
    m.initSync({ module: readFileSync(`${repo}crates/acoustilab-wasm/pkg/acoustilab_wasm_bg.wasm`) });
    engine = m;
  }
  return engine;
}
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Any = any;
const call = async (fn: string, ...args: string[]): Promise<Any> => {
  const v = JSON.parse((await eng())[fn](...args));
  expect(v?.error, `${fn}: ${v?.error}`).toBeUndefined();
  return v;
};

// ----- page helpers ------------------------------------------------------------

async function solved(page: Page): Promise<void> {
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solved', { timeout: 20_000 });
}

const analysis = (page: Page, name: string): Promise<Any> =>
  page.evaluate((n) => (window as Any).acoustilabAnalysis.get(n), name);

const resultText = (page: Page): Promise<string> => page.evaluate(() => (document.getElementById('netlist') as HTMLTextAreaElement).value);

async function openView(page: Page, name: string): Promise<void> {
  await page.getByRole('tablist', { name: 'Result views' }).getByRole('tab', { name, exact: true }).click();
  await expect(page.locator(`#view-${name.toLowerCase()}`)).toBeVisible();
}

/** Sets a number parameter through its entry, as a user types it. */
async function setParam(page: Page, name: string, value: string): Promise<void> {
  await page.fill(`#p-${name}`, value);
  await page.press(`#p-${name}`, 'Enter');
  await expect.poll(() => page.evaluate((n) => (window as Any).acoustilab.result()?.meta.parameters[n], name)).toBe(Number(value));
  await solved(page);
}

/** Waits until the readouts on screen are those of the plotted text. */
async function readoutsSettled(page: Page): Promise<Any> {
  await expect
    .poll(async () => {
      const r = await analysis(page, 'readouts');
      const text = await page.evaluate(() => {
        const r = (window as Any).acoustilab.result();
        return r ? (document.getElementById('netlist') as HTMLTextAreaElement).value : null;
      });
      return r && !r.busy && r.shown === r.current && r.current === text;
    }, { timeout: 20_000 })
    .toBe(true);
  return analysis(page, 'readouts');
}

const signed = (x: number, d = 2) => {
  const t = Math.abs(x).toFixed(d);
  return `${Number(t) === 0 ? '' : x > 0 ? '+' : '−'}${t}`;
};

const DB_PER_PCT = -20 / Math.LN10 / 100;

/** A lossless sealed cavity fed by a constant volume velocity, with its volume as a parameter. */
const CAVITY = JSON.stringify(
  {
    parameters: { V_cm3: { value: 30, min: 1, max: 1000, label: 'Cavity volume' } },
    air: { preset: 'spec_reference' },
    level: 0,
    sweep: { f_min_Hz: 10, f_max_Hz: 10000, points_per_octave: 3 },
    nodes: [{ id: 'a', domain: 'acoustic' }],
    elements: [
      { id: 'q', type: 'flow_source', node: 'a', U_m3_per_s: 1e-6 },
      { id: 'v', type: 'cavity', node: 'a', volume_cm3: '=V_cm3', wall_loss: false },
    ],
    probes: [{ id: 'p', quantity: 'pressure', node: 'a' }],
  },
  null,
  2,
);

async function loadNetlist(page: Page, text: string): Promise<void> {
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  await page.fill('#netlist', text);
  await page.click('#run-btn');
  await solved(page);
}

async function runSensitivity(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Run sensitivity analysis', exact: true }).click();
  await expect(page.locator('#view-sensitivity .an-runbar .an-status')).toHaveText(/^Done:/, { timeout: 30_000 });
}

async function runMonteCarlo(page: Page, n: number, seed: number): Promise<void> {
  await page.getByLabel('Runs (N)').fill(String(n));
  await page.getByLabel('Seed', { exact: true }).fill(String(seed));
  await page.getByRole('button', { name: 'Run Monte Carlo', exact: true }).click();
  await expect(page.locator('#view-tolerance .an-runbar .an-status')).toHaveText(new RegExp(`^Done: ${n} runs`), { timeout: 60_000 });
}

// ----- readouts ----------------------------------------------------------------

test('readouts: the readouts export of the plotted design, with its units and methods', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const got = await readoutsSettled(page);
  const ref = await call('readouts', TEMPLATE, '', '');
  expect(got.doc.doc).toEqual(ref);
  const block = page.locator('#readouts');
  const cell = (key: string) => block.locator(`[data-readout="${key}"]`);
  const nums = async (key: string) => JSON.parse((await cell(key).locator('.ro-value').getAttribute('data-nums')) ?? 'null');

  const cr = ref.response.coupled_resonance;
  expect(await nums('coupled_resonance')).toEqual([cr.f_Hz]);
  await expect(cell('coupled_resonance').locator('.ro-value')).toHaveText(formatHz(cr.f_Hz));
  await expect(cell('coupled_resonance').locator('dt')).toContainText('max |v/i|');
  expect(await nums('z_resonance')).toEqual([ref.impedance.resonance.f_Hz, ref.impedance.resonance.z_ohm]);
  expect(await nums('q')).toEqual([ref.impedance.q.Qms, ref.impedance.q.Qes, ref.impedance.q.Qts]);
  expect(await nums('z_1khz')).toEqual([ref.impedance.z_1kHz_ohm]);
  expect(await nums('z_min')).toEqual([ref.impedance.rated_check.z_min_ohm]);
  await expect(cell('z_min')).toContainText('pass: above 25.6 Ω (80 % of 32 Ω rated)');
  const s1k = ref.response.sensitivity.find((s: Any) => s.f_Hz === 1000);
  expect(await nums('sensitivity_1000')).toEqual([s1k.dB_per_V, s1k.dB_per_mW]);
  await expect(cell('sensitivity_1000').locator('.ro-value')).toHaveText(`${s1k.dB_per_V.toFixed(1)} dB/V · ${s1k.dB_per_mW.toFixed(1)} dB/mW`);
  // The template's sealed design: no bass extension in the sweep, said so.
  expect(ref.response.bass_extension).toBeNull();
  await expect(cell('bass_extension').locator('.ro-value')).toHaveText('beyond the sweep');
  await expect(cell('bass_extension')).toContainText(`reference ${ref.response.level_500Hz_dB.toFixed(1)} dB SPL at 500 Hz`);
  await expect(block.locator('.ro-notes')).toContainText(ref.response.notes[0]);
  const fa = ref.drivers[0].free_air;
  expect(await nums('free_air_0')).toEqual([fa.f_Hz, fa.Qms, fa.Qes, fa.Qts]);
  // The engine's method texts, verbatim.
  await block.locator('.ro-methods summary').click();
  for (const t of Object.values(ref.methods)) await expect(block.locator('.ro-methods')).toContainText(t as string);
});

test('readouts: Δ against the reference baseline, ambiguous resonance and missing Q shown as such', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  await readoutsSettled(page);
  await page.click('#freeze-btn');
  const base = await call('readouts', TEMPLATE, '', '');

  // A 1 mm leak: two |v/i| peaks within 3 dB (docs/analysis.md), so the
  // coupled resonance is ambiguous and has no Δ.
  await setParam(page, 'leak_gap_mm', '1');
  await readoutsSettled(page);
  const cur = await call('readouts', await resultText(page), '', '');
  const cr = cur.response.coupled_resonance;
  expect(cr.ambiguous).toBe(true);
  const cell = (key: string) => page.locator(`#readouts [data-readout="${key}"]`);
  await expect(cell('coupled_resonance').locator('.ro-flag')).toHaveText('⚠ ambiguous');
  await expect(cell('coupled_resonance')).toContainText(`competing peak ${formatHz(cr.competing.f_Hz)}, ${cr.competing.margin_dB.toFixed(2)} dB lower`);
  await expect(cell('coupled_resonance').locator('.ro-delta')).toHaveText('Δ n/a');
  await expect(page.locator('#readouts .ro-status')).toContainText('Δ against baseline “template values”');
  const a = cur.response.sensitivity.find((s: Any) => s.f_Hz === 1000);
  const b = base.response.sensitivity.find((s: Any) => s.f_Hz === 1000);
  await expect(cell('sensitivity_1000').locator('.ro-delta')).toHaveText(`Δ ${signed(a.dB_per_V - b.dB_per_V)} dB · ${signed(a.dB_per_mW - b.dB_per_mW)} dB`);
  // The bass extension exists now: its value and Δ n/a (none for the baseline).
  expect(cur.response.bass_extension).not.toBeNull();
  await expect(cell('bass_extension').locator('.ro-value')).toHaveText(formatHz(cur.response.bass_extension.f_Hz));
  await expect(cell('bass_extension').locator('.ro-delta')).toHaveText('Δ n/a');

  // A 0.3 mm leak: overlapping |Z| resonances, no Q (the engine's note says why).
  await setParam(page, 'leak_gap_mm', '0.3');
  await readoutsSettled(page);
  const q = await call('readouts', await resultText(page), '', '');
  expect(q.impedance.q).toBeNull();
  await expect(cell('q').locator('.ro-value')).toHaveText('not estimated');
  for (const n of q.impedance.notes) await expect(page.locator('#readouts .ro-notes')).toContainText(n);
});

test('readouts: a drag never queues readouts calls', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const before = (await readoutsSettled(page)).calls;
  const slider = page.locator('.prow[data-param="front_depth_mm"] input[type="range"]');
  await slider.evaluate((el: HTMLInputElement) => {
    for (let k = 0; k <= 40; k++) {
      el.value = String(100 + k);
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await solved(page);
  const after = await readoutsSettled(page);
  // At most two solves land (design.spec.ts), so at most two new texts to read.
  expect(after.calls - before).toBeLessThanOrEqual(2);
  expect(after.shown).toBe(await resultText(page));
});

// ----- inactive parameters -----------------------------------------------------------

test('inactive parameters: marked from the engine’s flags, still operable, described, rows kept', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  const check = async () => {
    const text = await resultText(page);
    const doc = await call('parameters', text, '');
    await expect
      .poll(async () => {
        const marked = await page.locator('.prow.inactive').evaluateAll((rs) => rs.map((r) => (r as HTMLElement).dataset.param).sort());
        return marked;
      })
      .toEqual(doc.parameters.filter((p: Any) => p.active === false && p.kind !== 'derived').map((p: Any) => p.name).sort());
    return doc;
  };
  const rows = await page.locator('.prow').count();
  // Keep a handle on a row: the panel updates in place, it is not rebuilt.
  await page.locator('.prow[data-param="vent_diameter_mm"]').evaluate((r) => ((r as Any).__keep = true));

  const closed = await check();
  expect(closed.parameters.find((p: Any) => p.name === 'grille_rayl').active).toBe(false);
  await expect(page.locator('.prow[data-param="grille_rayl"] .pinactive')).toBeVisible();
  await expect(page.locator('#p-vent_diameter_mm')).not.toHaveAccessibleDescription(/not used by this design/);

  await page.getByRole('radio', { name: /Open back/ }).check();
  await solved(page);
  const open = await check();
  for (const n of ['vent_count', 'vent_diameter_mm', 'rear_volume_cm3']) expect(open.parameters.find((p: Any) => p.name === n).active).toBe(false);
  await expect(page.locator('.prow[data-param="vent_diameter_mm"] .pinactive')).toHaveText('not used by this design');
  await expect(page.locator('.prow[data-param="grille_rayl"]')).not.toHaveClass(/inactive/);
  // Described, not renamed.
  await expect(page.locator('#p-vent_diameter_mm')).toHaveAccessibleName('Vent diameter, mm');
  await expect(page.locator('#p-vent_diameter_mm')).toHaveAccessibleDescription(/not used by this design/);
  await expect(page.locator('#p-vent_count')).toHaveAccessibleDescription('0 to 12; arrow keys step the value. not used by this design');
  expect(await page.locator('.prow').count()).toBe(rows);
  expect(await page.locator('.prow[data-param="vent_diameter_mm"]').evaluate((r) => (r as Any).__keep)).toBe(true);
  // Still operable: the stepper writes the netlist.
  await page.getByRole('button', { name: 'Increase Number of rear vents' }).click();
  await expect.poll(() => resultText(page)).toContain('"vent_count": {"value": 2,');
  await solved(page);
  await check();

  await page.getByRole('radio', { name: 'Closed cup with vents' }).check();
  await solved(page);
  await check();
  await expect(page.locator('#p-vent_diameter_mm')).not.toHaveAccessibleDescription(/not used by this design/);
});

// ----- sensitivity ------------------------------------------------------------------

test('sensitivity: a sealed cavity’s map and tornado equal the closed form', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  await loadNetlist(page, CAVITY);
  await openView(page, 'Sensitivity');
  await expect(page.locator('#view-sensitivity .an-params summary')).toHaveText('Parameters: 1 of 1 continuous');
  await runSensitivity(page);
  const s = await analysis(page, 'sensitivity');
  expect(s.rows).toEqual(['V_cm3']);
  for (const v of s.map[0]) expect(Math.abs(v / DB_PER_PCT - 1)).toBeLessThan(1e-9);
  // The crosshair readout of a cell (keyboard).
  const canvas = page.locator('#view-sensitivity .an-heat canvas');
  await canvas.focus();
  await page.keyboard.press('ArrowRight');
  const readout = page.locator('#view-sensitivity .an-heat .an-readout');
  await expect(readout).toContainText('Cavity volume (V_cm3) at');
  await expect(readout).toContainText(`${DB_PER_PCT.toPrecision(4).replace('-', '−')} dB per %`);
  expect(Math.abs(Number(await readout.getAttribute('data-value')) / DB_PER_PCT - 1)).toBeLessThan(1e-9);
  // Tornado at 1 kHz, the volume at ±10 % (no tolerance: assumed): p ∝ 1/V.
  await page.locator('#view-sensitivity').getByRole('button', { name: 'Show data table' }).nth(1).click();
  const row = page.locator('#view-sensitivity table[data-table="tornado"] tbody tr').first();
  await expect(row.locator('th')).toHaveText('Cavity volume (V_cm3)');
  await expect(row.locator('td').nth(0)).toHaveText('assumed ±10 %');
  const lo = 20 * Math.log10(1 / 0.9);
  const hi = 20 * Math.log10(1 / 1.1);
  await expect(row.locator('td').nth(3)).toHaveText(`+${lo.toPrecision(4)}`);
  await expect(row.locator('td').nth(4)).toHaveText(`−${Math.abs(hi).toPrecision(4)}`);
});

test('sensitivity: map, tornado and explain equal the engine’s exports; links; stale; cancel', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  await openView(page, 'Sensitivity');
  await runSensitivity(page);
  const s = await analysis(page, 'sensitivity');
  expect(s.text).toBe(TEMPLATE);
  // Default parameters: the continuous ones the design uses, not at zero.
  const doc = await call('parameters', TEMPLATE, '');
  expect(s.params).toEqual(doc.parameters.filter((p: Any) => p.kind === 'number' && p.active !== false && p.value !== 0).map((p: Any) => p.name));

  // The map equals the export (forward sensitivities, one call), and the
  // other method (complete solves) within docs/analysis.md's agreement.
  const fwd = await call('sensitivity', TEMPLATE, '', JSON.stringify({ parameters: s.params, probes: ['p_drp'], method: 'forward_sensitivity' }));
  const full = await call('sensitivity', TEMPLATE, '', JSON.stringify({ parameters: s.params, probes: ['p_drp'], method: 'complete_solves' }));
  expect(s.rows).toEqual(fwd.parameters.map((p: Any) => p.name));
  expect(s.freqs).toEqual(fwd.frequencies_Hz);
  const credible = (f: number) =>
    !(f >= (fwd.shading.begin_hz ?? Infinity) || f < (fwd.shading.low_begin_hz ?? -Infinity));
  s.rows.forEach((name: string, i: number) => {
    const a = s.map[i] as number[];
    const b = fwd.parameters[i].dB_per_pct[0] as number[];
    const c = full.parameters[i].dB_per_pct[0] as number[];
    const big = Math.max(...c.filter((_, k) => credible(fwd.frequencies_Hz[k])).map(Math.abs), 1e-12);
    a.forEach((v, k) => {
      expect(Math.abs(v - b[k]), `${name} @${k}`).toBeLessThanOrEqual(1e-12 * big);
      expect(Math.abs(v - c[k]), `${name} @${k} vs complete solves`).toBeLessThanOrEqual((credible(fwd.frequencies_Hz[k]) ? 1e-6 : 1e-5) * big);
    });
  });
  // A cell read with the keyboard: the diaphragm area's row, the middle column.
  const canvas = page.locator('#view-sensitivity .an-heat canvas');
  await canvas.focus();
  await page.keyboard.press('ArrowRight');
  const sd = s.rows.indexOf('driver_Sd_cm2');
  for (let k = 0; k < sd; k++) await page.keyboard.press('ArrowDown');
  const cur = (await analysis(page, 'sensitivity')).cursor;
  expect(cur.row).toBe(sd);
  const readout = page.locator('#view-sensitivity .an-heat .an-readout');
  await expect(readout).toContainText(`Diaphragm area Sd (driver_Sd_cm2) at ${formatHz(s.freqs[cur.col])}`);
  expect(Number(await readout.getAttribute('data-value'))).toBe(fwd.parameters[sd].dB_per_pct[0][cur.col]);

  // Tornado of the level at 1 kHz: the engine's rows in the engine's order.
  const tor = await call('tornado', TEMPLATE, '', JSON.stringify({ metric: { kind: 'level', probe: 'p_drp', f_Hz: 1000 }, parameters: s.params }));
  expect(s.tornado).toEqual(tor.rows.map((r: Any) => r.name));
  for (let k = 1; k < tor.rows.length; k++) expect(tor.rows[k].effect).toBeLessThanOrEqual(tor.rows[k - 1].effect);
  await expect(page.locator('#view-sensitivity .an-tornado svg g.an-t-row')).toHaveCount(tor.rows.length);
  // A readout metric runs in parts; the merged rows keep the engine's order.
  await page.getByLabel('Metric').selectOption('readout');
  await page.getByLabel('Readout', { exact: true }).selectOption('coupled_resonance_Hz');
  await page.getByRole('button', { name: 'Update tornado', exact: true }).click();
  await expect(page.locator('#view-sensitivity .an-runbar .an-status')).toHaveText('Tornado chart updated.', { timeout: 30_000 });
  const torR = await call('tornado', TEMPLATE, '', JSON.stringify({ metric: { kind: 'readout', name: 'coupled_resonance_Hz' }, parameters: s.params }));
  expect((await analysis(page, 'sensitivity')).tornado).toEqual(torR.rows.map((r: Any) => r.name));
  await expect(page.locator('#view-sensitivity h3').filter({ hasText: 'Tornado chart:' })).toHaveText(`Tornado chart: ${torR.description}`);

  // Explain: the engine's sentences, each linked to its bands and parameter.
  const ex = await call('explain', TEMPLATE, '', JSON.stringify({ probe: 'p_drp', parameters: s.params }));
  expect(ex.sentences.length).toBeGreaterThan(0);
  await expect(page.locator('#view-sensitivity .an-sentence')).toHaveText(ex.sentences.map((x: Any) => x.text));
  const first = ex.sentences[0];
  const bd = first.bands[first.stated[0]];
  const btn = page.locator(`#view-sensitivity li[data-param="${first.parameter}"] .an-links > .an-band-btn`).first();
  await btn.click();
  await expect(btn).toHaveAttribute('aria-pressed', 'true');
  const hl = await page.evaluate(() => (window as Any).acoustilab.highlight());
  expect([hl.lo, hl.hi]).toEqual([bd.f_min_Hz, bd.f_max_Hz]);
  expect(hl.label).toContain(first.label);
  const after = await analysis(page, 'sensitivity');
  expect([after.highlight.lo, after.highlight.hi]).toEqual([bd.f_min_Hz, bd.f_max_Hz]);
  expect(after.rows[after.cursor.row]).toBe(first.parameter);
  await btn.click();
  await expect(btn).toHaveAttribute('aria-pressed', 'false');
  expect(await page.evaluate(() => (window as Any).acoustilab.highlight())).toBeNull();
  // The parameter link focuses its control in the Design tab.
  await page.getByRole('button', { name: `Go to the control of ${first.label} in the Design tab` }).click();
  await expect(page.getByRole('tab', { name: 'Design', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator(`#p-${first.parameter}`)).toBeFocused();

  // A design change marks the results stale; Recompute brings them up to date.
  await page.keyboard.press('ArrowUp');
  await expect.poll(() => resultText(page)).not.toBe(TEMPLATE);
  await solved(page);
  const banner = page.locator('#view-sensitivity .an-stale');
  await expect(banner).toBeVisible();
  await expect(banner).toContainText('Stale:');
  await expect(page.locator('#view-sensitivity .an-output')).toHaveClass(/is-stale/);
  await banner.getByRole('button', { name: 'Recompute' }).click();
  await expect(page.locator('#view-sensitivity .an-runbar .an-status')).toHaveText(/^Done:/, { timeout: 30_000 });
  await expect(banner).toBeHidden();
  expect((await analysis(page, 'sensitivity')).text).toBe(await resultText(page));

  // Cancel: the previous results stay, and the next run works.
  const kept = (await analysis(page, 'sensitivity')).text;
  await page.getByRole('button', { name: 'Run sensitivity analysis', exact: true }).click();
  await page.locator('#view-sensitivity .an-runbar').getByRole('button', { name: 'Cancel' }).click();
  await expect(page.locator('#view-sensitivity .an-runbar .an-status')).toHaveText('Cancelled; the results below are from the previous run.');
  expect((await analysis(page, 'sensitivity')).text).toBe(kept);
  await runSensitivity(page);
});

// ----- tolerance --------------------------------------------------------------------

test('tolerance: envelopes and readouts equal mc_envelope, contain the nominal; CSV; cancel; stale', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  await openView(page, 'Tolerance');
  await expect(page.getByLabel('Runs (N)')).toHaveValue('200');
  await runMonteCarlo(page, 40, 7);
  const t = await analysis(page, 'tolerance');
  expect(t.spec).toEqual({ method: 'lhs', n: 40, seed: 7 });

  // The same plan, runs and envelope in Node (other chunking: one call).
  const spec = { method: 'lhs', n: 40, seed: 7 };
  const probes = ['p_drp', 'p_front', 'p_rear', 'zin'];
  const run = await call('mc_run', TEMPLATE, '', JSON.stringify({ plan: spec, first: 0, count: 40 }), JSON.stringify({ probes, metrics: true }));
  const env = await call('mc_envelope', JSON.stringify(run));
  const ref = env.probes.find((p: Any) => p.id === 'p_drp').dB;
  expect(t.freqs).toEqual(run.frequencies_Hz);
  for (const k of ['median', 'p5', 'p10', 'p90', 'p95', 'min', 'max', 'n']) expect(t.stats[k], k).toEqual(ref[k]);
  expect(t.metrics).toEqual(env.metrics);
  // The nominal curve (the plotted design) lies inside min–max everywhere.
  const nominal = (await page.evaluate(() => (window as Any).acoustilab.result())).probes.find((p: Any) => p.id === 'p_drp').spl_dB;
  expect(t.nominal).toEqual(nominal);
  nominal.forEach((v: number, i: number) => {
    expect(v).toBeGreaterThanOrEqual(ref.min[i] - 1e-9);
    expect(v).toBeLessThanOrEqual(ref.max[i] + 1e-9);
  });
  // The data table lists the same numbers.
  await page.locator('#view-tolerance').getByRole('button', { name: 'Show data table' }).click();
  const tr = page.locator('#view-tolerance table[data-table="envelope"] tbody tr').first();
  const i0 = run.frequencies_Hz.findIndex((f: number) => f >= 20 * (1 - 1e-9));
  await expect(tr.locator('td').nth(2)).toHaveText(ref.median[i0].toFixed(2));
  await expect(tr.locator('td').nth(8)).toHaveText(ref.max[i0].toFixed(2));
  // The varied parameters: the plan's, with their sources.
  const plan = await call('mc_plan', TEMPLATE, '', JSON.stringify(spec));
  const dist = page.locator('#view-tolerance table').filter({ hasText: 'Distribution' });
  await expect(dist.locator('tbody tr')).toHaveCount(plan.distributions.length);
  await expect(dist).toContainText(plan.distributions[0].source);
  // The metric histogram reports how many runs have the readout.
  const n = env.metrics.coupled_resonance_Hz.n;
  await expect(page.locator('#view-tolerance .an-hist')).toContainText(`${n} of 40 runs have coupled_resonance_Hz`);

  // The DOE table: 40 rows, the runs' hashes.
  const [dl] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: /Download CSV/ }).click()]);
  expect(dl.suggestedFilename()).toBe('acoustilab-doe-lhs-n40-seed7.csv');
  const csv = readFileSync(await dl.path(), 'utf8').trim().split(/\r?\n/);
  expect(csv[0].startsWith('run,hash,engine,')).toBe(true);
  expect(csv.length).toBe(41);
  expect(csv.slice(1).map((l) => l.split(',')[1])).toEqual(run.samples.map((s: Any) => s.hash));

  // Cancel mid-run: the previous runs stay.
  await page.getByLabel('Runs (N)').fill('400');
  await page.getByRole('button', { name: 'Run Monte Carlo', exact: true }).click();
  await expect(page.locator('#view-tolerance .an-runbar .an-status')).toHaveText(/Run (20|40) of 400 solved/, { timeout: 30_000 });
  await page.locator('#view-tolerance .an-runbar').getByRole('button', { name: 'Cancel' }).click();
  await expect(page.locator('#view-tolerance .an-runbar .an-status')).toHaveText('Cancelled; the results below are from the previous run.');
  expect((await analysis(page, 'tolerance')).runs).toBe(40);

  // A design change marks the runs stale.
  await setParam(page, 'front_depth_mm', '16');
  await expect(page.locator('#view-tolerance .an-stale')).toBeVisible();
  expect((await analysis(page, 'tolerance')).stale).toBe(true);
});

// ----- target ----------------------------------------------------------------------

test('target: the fixture rule, metrics and scores equal target_metrics; smoothing; import', async ({ page }) => {
  await page.goto('/');
  await solved(page);
  await openView(page, 'Target');
  const settled = async () => {
    await expect
      .poll(async () => {
        const t = await analysis(page, 'target');
        return !!t.report && t.text === (await resultText(page));
      }, { timeout: 20_000 })
      .toBe(true);
    return analysis(page, 'target');
  };
  let t = await settled();
  expect(t.target).toBe('ravizza2023_5128');
  const result = await page.evaluate(() => (window as Any).acoustilab.result());
  const fx = await call('probe_fixture', TEMPLATE, '', 'p_drp');
  expect(fx.fixture).toBe('iec60318_4');
  const ref = await call('target_metrics', JSON.stringify(result), 'ravizza2023_5128', JSON.stringify({ probe: 'p_drp', fixture: 'iec60318_4', smoothing: 'none' }));
  expect(t.report).toEqual(ref);

  // The fixture rule, first and visible.
  const box = page.locator('#view-target .an-fixture');
  await expect(box).toBeVisible();
  await expect(box).toContainText('Fixture mismatch');
  await expect(box).toContainText('(iec60318_4, inferred from the ear load)');
  await expect(box).toContainText('(bk5128)');
  await expect(box).toContainText(ref.flags.find((f: Any) => f.code === 'fixture_mismatch').message);

  // The metrics table shows the report's numbers.
  const main = page.locator('#view-target table[data-table="metrics"] tbody tr').first();
  await expect(main.locator('th')).toHaveText('main');
  await expect(main.locator('td').nth(3)).toHaveText(ref.metrics.main.rms_dB.toFixed(2));
  await expect(main.locator('td').nth(4)).toHaveText(ref.metrics.main.sd_dB.toFixed(2));
  await expect(page.locator('#view-target table[data-table="masks"] tbody tr').first()).toContainText(`${ref.metrics.bs708_mask.compliance_percent.toFixed(1)} %`);
  // Scores: value, greyed state and the flags behind it, in words.
  for (const s of ref.scores) {
    const card = page.locator(`#view-target .an-score[data-model="${s.model}"]`);
    await expect(card).toHaveAttribute('data-score', String(s.score));
    await expect(card).toHaveAttribute('data-greyed', String(s.greyed));
    await expect(card.locator('.an-score-head')).toContainText(s.greyed ? 'greyed' : 'applies');
    for (const f of s.flags) await expect(card).toContainText(f.message);
  }
  await expect(page.locator('#view-target .an-score').first()).toContainText('fixture differs from the model’s training fixture');
  await expect(page.locator('#view-target .an-score').first()).toContainText('simulated curve, not measured');

  // Smoothing is the engine's.
  await page.getByLabel('Smoothing').selectOption('1/3');
  await expect.poll(async () => (await analysis(page, 'target')).report?.options.smoothing).toBe('1/3 octave');
  const sm = await call('target_metrics', JSON.stringify(result), 'ravizza2023_5128', JSON.stringify({ probe: 'p_drp', fixture: 'iec60318_4', smoothing: '1/3' }));
  expect((await analysis(page, 'target')).report).toEqual(sm);

  // The Type 4.3 ear shares the 5128's ear simulator but not its pinna and head.
  await page.getByRole('radio', { name: /Type 4\.3/ }).check();
  await solved(page);
  t = await settled();
  expect(t.report.fixture_match).toBe('same_ear_simulator');
  await expect(box).toContainText('Same ear simulator, different pinna, head or canal extension');

  // Import a CSV target; the fixture is required.
  const csv = '# name: flat\nfrequency_Hz,dB\n20,0\n1000,0\n20000,0\n';
  await page.locator('#view-target .an-import summary').click();
  await page.getByLabel('CSV text').fill(csv);
  await page.getByRole('button', { name: 'Import', exact: true }).click();
  await expect(page.locator('#view-target .an-import .an-status')).toContainText('Not imported:');
  await expect(page.locator('#view-target .an-import .an-status')).toContainText('fixture');
  await page.locator('#view-target .an-import').getByLabel('Fixture').selectOption('p57_type4_3');
  await page.getByRole('button', { name: 'Import', exact: true }).click();
  await expect(page.locator('#view-target .an-import .an-status')).toContainText('Imported “flat” on fixture p57_type4_3');
  await expect(page.getByRole('combobox', { name: 'Target', exact: true })).toHaveValue('import:flat');
  t = await settled();
  const obj = await call('import_target_csv', csv, 'p57_type4_3', '');
  const r2 = await page.evaluate(() => (window as Any).acoustilab.result());
  const imp = await call('target_metrics', JSON.stringify(r2), JSON.stringify(obj), JSON.stringify({ probe: 'p_drp', fixture: 'p57_type4_3', smoothing: '1/3' }));
  expect(t.report).toEqual(imp);
  expect(t.report.fixture_match).toBe('same');
  await expect(box).toContainText('Same fixture');
});

// ----- accessibility, keyboard, narrow screens -------------------------------------

async function openAndRun(page: Page, view: string): Promise<void> {
  if (view === 'Response') return;
  await openView(page, view);
  if (view === 'Sensitivity') await runSensitivity(page);
  if (view === 'Tolerance') await runMonteCarlo(page, 20, 1);
  if (view === 'Target') await expect.poll(async () => !!(await analysis(page, 'target')).report).toBe(true);
}

for (const theme of ['light', 'dark'] as const) {
  test(`analysis views (${theme}): no axe violations with each view open and filled`, async ({ page }) => {
    test.setTimeout(120_000);
    await page.emulateMedia({ colorScheme: theme });
    await page.goto('/');
    await solved(page);
    await readoutsSettled(page);
    for (const view of ['Response', 'Sensitivity', 'Tolerance', 'Target']) {
      await openAndRun(page, view);
      // Every disclosure open, so their content is checked too.
      for (const b of await page.locator(`#view-${view.toLowerCase()} button[aria-expanded="false"]`).all()) if (await b.isVisible()) await b.click();
      for (const d of await page.locator(`#view-${view.toLowerCase()} details:not([open]) > summary`).all()) if (await d.isVisible()) await d.click();
      const r = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
      expect(r.violations.map((v) => `${view}: ${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
    }
  });
}

test('analysis views: nothing scrolls sideways at 390 px', async ({ page }) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await solved(page);
  for (const view of ['Response', 'Sensitivity', 'Tolerance', 'Target']) {
    await openAndRun(page, view);
    for (const b of await page.locator(`#view-${view.toLowerCase()} button[aria-expanded="false"]`).all()) if (await b.isVisible()) await b.click();
    expect(await page.evaluate(() => document.documentElement.scrollWidth), view).toBeLessThanOrEqual(390);
  }
});

test('analysis views: every new control works from the keyboard', async ({ page }) => {
  test.setTimeout(120_000);
  await page.goto('/');
  await solved(page);
  const tab = page.getByRole('tablist', { name: 'Result views' }).getByRole('tab', { name: 'Response', exact: true });
  await tab.focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: 'Sensitivity', exact: true })).toBeFocused();
  await expect(page.locator('#view-sensitivity')).toBeVisible();

  // Sensitivity: Tab order reaches the probe, the parameter list, Run.
  const seen: string[] = [];
  for (let k = 0; k < 6; k++) {
    await page.keyboard.press('Tab');
    seen.push(await page.evaluate(() => {
      const a = document.activeElement as HTMLElement;
      return a.tagName === 'SUMMARY' ? 'summary' : a.textContent?.trim() || a.getAttribute('aria-label') || a.tagName;
    }));
  }
  expect(seen).toContain('summary');
  expect(seen).toContain('Run sensitivity analysis');
  // The parameter list opens with Enter; a checkbox toggles with Space.
  await page.locator('#view-sensitivity .an-params summary').focus();
  await page.keyboard.press('Enter');
  const box = page.locator('#view-sensitivity .an-params input[value="driver_fs_Hz"]');
  await box.focus();
  await page.keyboard.press('Space');
  await expect(box).not.toBeChecked();
  await expect(page.locator('#view-sensitivity .an-params summary')).toContainText('17 of 21');
  await page.keyboard.press('Space');
  await expect(box).toBeChecked();
  await page.getByRole('button', { name: 'Run sensitivity analysis', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('#view-sensitivity .an-runbar .an-status')).toHaveText(/^Done:/, { timeout: 30_000 });
  // A band button toggles with Space; the map's crosshair moves with the arrows.
  const band = page.locator('#view-sensitivity .an-band-btn').first();
  await band.focus();
  await page.keyboard.press('Space');
  await expect(band).toHaveAttribute('aria-pressed', 'true');
  await page.locator('#view-sensitivity .an-heat canvas').focus();
  await page.keyboard.press('End');
  const c = (await analysis(page, 'sensitivity')).cursor;
  expect(c.col).toBe((await analysis(page, 'sensitivity')).freqs.length - 1);
  await page.keyboard.press('Escape');
  expect((await analysis(page, 'sensitivity')).cursor).toBeNull();
  // The colour scale and the tornado metric are selects.
  await page.getByLabel('Colour scale').focus();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByLabel('Colour scale')).toHaveValue('all');

  // Tolerance: N, seed and Run from the keyboard.
  await page.getByRole('tab', { name: 'Tolerance', exact: true }).click();
  await page.getByLabel('Runs (N)').focus();
  await page.keyboard.press('Control+A');
  await page.keyboard.type('20');
  await page.keyboard.press('Tab');
  await expect(page.getByLabel('Seed', { exact: true })).toBeFocused();
  await page.getByRole('button', { name: 'Run Monte Carlo', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('#view-tolerance .an-runbar .an-status')).toHaveText(/^Done: 20 runs/, { timeout: 30_000 });
  const plot = page.locator('#view-tolerance .an-plots canvas');
  await plot.focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('#view-tolerance .an-readout')).toContainText('nominal');

  // Target: the smoothing select from the keyboard recomputes the report.
  await page.getByRole('tab', { name: 'Target', exact: true }).click();
  await expect.poll(async () => !!(await analysis(page, 'target')).report).toBe(true);
  await page.getByLabel('Smoothing').focus();
  await page.keyboard.press('ArrowDown');
  await expect.poll(async () => (await analysis(page, 'target')).report?.options.smoothing).not.toBe('none');
});
