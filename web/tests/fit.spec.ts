// The Fit view (fit.view.ts): the measurement round trip of spec Section 12
// with the virtual rig. Oracles: the true parameter value the rig measured
// with, the engine's exports called in Node on the same inputs, the
// combined-uncertainty closed form, and the template text with exactly
// one value token replaced.

import { expect, test, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { axe, engine, example, noSideScroll, open, openView, viewHook } from './measure.helpers';

const TEMPLATE = example('design_over_ear');
/** The comment line the view writes into virtual-rig data files (fit-curves.ts VIRTUAL_MARK). */
const MARK = 'VIRTUAL RIG: synthetic data from the AcoustiLab virtual rig, not a measurement';
/** formatParam(v, digits) of format.ts for ordinary magnitudes. */
const param = (v: number, digits = 6) => String(Number(v.toPrecision(digits)));

/** The rig request the view builds from its form (defaults, seed 7, leak 0.12 mm). */
function rigSpec(extra: { seatings?: number; microphoneOffset?: number } = {}): Record<string, unknown> {
  return {
    probe: 'p_drp',
    f_min_Hz: 10,
    f_max_Hz: 20000,
    points_per_octave: 12,
    noise: {
      seed: 7,
      level_dB: 0.1,
      phase_deg: 1,
      seatings: extra.seatings ?? 1,
      averaging: 'complex',
      repositioning_dB: 0,
      repositioning_delay_us: 0,
      microphone_offset_dB: extra.microphoneOffset ?? 0,
      microphone_slope_dB_per_decade: 0,
      coupler_dB: 0,
    },
    overrides: { leak_gap_mm: 0.12 },
  };
}

async function fitView(page: Page): Promise<void> {
  await open(page);
  await openView(page, 'Fit');
  await expect(page.locator('#view-fit input[data-param="leak_gap_mm"]')).toBeVisible();
}

async function generate(page: Page, seatings = 1): Promise<void> {
  const panel = page.locator('#view-fit');
  await panel.getByLabel('True values that differ from the netlist').fill('leak_gap_mm=0.12');
  await panel.getByLabel('Seed').fill('7');
  await panel.getByLabel('Seatings').fill(String(seatings));
  await panel.getByRole('button', { name: 'Generate synthetic measurement' }).click();
  await expect(panel.locator('[data-rig="result"]')).toContainText('VIRTUAL RIG · synthetic, not measured');
}

const curveCard = (page: Page, name: string) => page.locator(`#view-fit li.mv-curve[data-curve="${name}"]`);

test('fit: a virtual-rig measurement, downloaded and read back, recovers the perturbed leak within its interval; Apply writes exactly that value', async ({ page }) => {
  await fitView(page);
  const panel = page.locator('#view-fit');
  await generate(page);
  const rig = panel.locator('[data-rig="result"]');
  const [d1] = await Promise.all([page.waitForEvent('download'), rig.getByRole('button', { name: 'Download virtual-rig-p_drp-seed7.frd', exact: true }).click()]);
  const [d2] = await Promise.all([page.waitForEvent('download'), rig.getByRole('button', { name: 'Download virtual-rig-p_drp-seed7.frd.sidecar.json', exact: true }).click()]);
  expect([d1.suggestedFilename(), d2.suggestedFilename()]).toEqual(['virtual-rig-p_drp-seed7.frd', 'virtual-rig-p_drp-seed7.frd.sidecar.json']);
  const frd = readFileSync((await d1.path())!, 'utf8');
  const sidecar = readFileSync((await d2.path())!, 'utf8');
  // The files are the engine's rig output for that request, the data file
  // with one comment line after the engine's header saying what it is.
  const call = await engine();
  const ref = call('virtual_measure', TEMPLATE, JSON.stringify(rigSpec()));
  const lines = frd.split('\n');
  expect(lines[1]).toBe(`* ${MARK} (probe p_drp, seed 7, true values leak_gap_mm=0.12)`);
  expect([lines[0], ...lines.slice(2)].join('\n')).toBe(ref.text);
  expect(sidecar).toBe(ref.sidecar);
  expect(JSON.parse(sidecar).provenance.origin).toBe('virtual_rig');

  // Read back through the file picker, paired by name with the sidecar.
  await panel.locator('input[data-input="curves"]').setInputFiles([
    { name: 'virtual-rig-p_drp-seed7.frd', mimeType: 'text/plain', buffer: Buffer.from(frd) },
    { name: 'virtual-rig-p_drp-seed7.frd.sidecar.json', mimeType: 'application/json', buffer: Buffer.from(sidecar) },
  ]);
  const card = curveCard(page, 'virtual-rig-p_drp-seed7.frd');
  await expect(card).toContainText('VIRTUAL RIG · synthetic, not measured');
  await expect(card.locator('[data-field="compat"]')).toContainText('Compatible with the model’s probe p_drp', { timeout: 30_000 });
  await expect(card.locator('.legend-item[data-series="measured (virtual rig)"]')).toBeVisible();

  await panel.locator('input[data-param="leak_gap_mm"]').check();
  await panel.getByRole('button', { name: 'Run fit', exact: true }).click();
  await expect(panel.locator('.mv-job[data-state="done"]')).toContainText('Converged', { timeout: 60_000 });
  const report = await viewHook(page, 'fit', 'report');
  const run = await viewHook(page, 'fit', 'run');
  const p = report.parameters[0];
  expect(p.name).toBe('leak_gap_mm');
  expect(p.status).toBe('determined');
  // The truth lies in the reported 95 % interval.
  expect(p.ci95[0]).toBeLessThan(0.12);
  expect(p.ci95[1]).toBeGreaterThan(0.12);
  // Resumed in bounded steps from the template's 0.08 mm; one unbounded call
  // of the engine from there reaches the same optimum (to a tenth of an sd).
  expect(run.starts.leak_gap_mm).toBe(0.08);
  expect(run.calls).toBeGreaterThan(1);
  const single = call('fit', TEMPLATE, JSON.stringify({ schema: 'acoustilab-fit/0.1', parameters: ['leak_gap_mm'], curves: [{ probe: 'p_drp', curve: call('import_curve', frd, JSON.stringify({ format: 'frd', sidecar: JSON.parse(sidecar) })) }] }));
  expect(Math.abs(Math.log(single.parameters[0].value / p.value))).toBeLessThan(0.1 * p.sd);

  // The report as shown.
  const row = panel.locator('table.mv-fit-params tr[data-param="leak_gap_mm"]');
  await expect(row.locator('td').nth(0)).toHaveText('0.08 mm');
  await expect(row.locator('td').nth(1)).toHaveText(`${param(p.value)} mm`);
  await expect(row.locator('td').nth(2)).toHaveText(`${param(p.ci95[0])} to ${param(p.ci95[1])}`);
  await expect(row.locator('td').nth(3)).toHaveText('determined');
  await expect(panel.getByText(report.identifiability.directions[0].text)).toBeVisible();
  // The report and the Apply section say the data are synthetic.
  const synthetic = 'Fitted to synthetic virtual-rig data, not to a measurement: virtual-rig-p_drp-seed7.frd.';
  await expect(panel.locator('.mv-report [data-field="cautions"]')).toHaveText(synthetic);
  await expect(panel.locator('.mv-apply [data-field="apply-cautions"]')).toHaveText(synthetic);
  // The residual plot of the curve.
  await expect(card.locator('.mv-figure figure[data-group="residual"]')).toBeVisible();

  // Apply: the preview shows the change, and the text changes in that one token.
  const apply = panel.locator('.mv-apply');
  await expect(apply.locator('tr[data-param="leak_gap_mm"] td').nth(2)).toHaveText('0.08 mm');
  await expect(apply.locator('tr[data-param="leak_gap_mm"] input[type="checkbox"]')).toBeChecked();
  // From the keyboard; the re-solve that follows rebuilds the section and the focus stays on the button.
  const applyBtn = apply.getByRole('button', { name: 'Apply fitted values' });
  await applyBtn.focus();
  await page.keyboard.press('Enter');
  const expected = TEMPLATE.replace('"leak_gap_mm": {"value": 0.08,', `"leak_gap_mm": {"value": ${JSON.stringify(p.value)},`);
  expect(expected).not.toBe(TEMPLATE);
  await expect(page.locator('#netlist')).toHaveValue(expected);
  await expect(apply).toContainText(`Wrote leak_gap_mm = ${String(p.value)} into the netlist.`);
  // Only the fitted value changed since the fit: said, but nothing is withdrawn.
  await expect(apply).toContainText('The fitted parameters’ values in the netlist have changed since this fit (applied or edited)');
  await expect(applyBtn).toBeFocused();

  // Another change to the netlist: the fitted values belong to the earlier
  // text, so Apply no longer proposes them.
  await page.getByRole('tab', { name: 'Netlist', exact: true }).click();
  const depth = JSON.parse(TEMPLATE).parameters.front_depth_mm.value;
  const moved = expected.replace(`"front_depth_mm": {"value": ${depth},`, `"front_depth_mm": {"value": ${depth + 1},`);
  expect(moved).not.toBe(expected);
  await page.locator('#netlist').fill(moved);
  await page.getByRole('button', { name: 'Run', exact: true }).click();
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solved', { timeout: 30_000 });
  await openView(page, 'Fit');
  await expect(apply.locator('[data-field="apply-cautions"]')).toContainText('The netlist has changed since this fit in more than the fitted parameters’ values');
  await expect(apply.locator('tr[data-param="leak_gap_mm"] input[type="checkbox"]')).not.toBeChecked();
});

test('fit: “Load as a measurement” reads the rig’s file back; the uncertainty band is the budget’s', async ({ page }) => {
  await fitView(page);
  await generate(page, 4);
  const panel = page.locator('#view-fit');
  await panel.getByRole('button', { name: 'Load as a measurement' }).click();
  const card = curveCard(page, 'virtual-rig-p_drp-seed7.frd');
  await expect(card.locator('[data-field="compat"]')).toContainText('Compatible', { timeout: 30_000 });
  const call = await engine();
  const ref = call('virtual_measure', TEMPLATE, JSON.stringify(rigSpec({ seatings: 4 })));
  // What the view read back: the rig's FRD text (six decimals) with its sidecar.
  const read = call('import_curve', ref.text, JSON.stringify({ format: 'frd', sidecar: JSON.parse(ref.sidecar) }));
  const curves = await viewHook(page, 'fit', 'curves');
  expect(curves).toHaveLength(1);
  expect(curves[0].virtual).toBe(true);
  expect(curves[0].points).toBe(read.frequencies_Hz.length);
  // Budget: noise 0.1 dB per seating over 4 seatings, u = 0.1/√4 = 0.05 dB.
  for (const u of curves[0].uncertainty) expect(Math.abs(u - 0.05)).toBeLessThan(1e-12);
  // The model curve: the probe at the curve's frequencies (equal to the engine's probe_curve there).
  const net = JSON.parse(TEMPLATE);
  net.sweep = { frequencies_Hz: read.frequencies_Hz };
  net.drive = read.sidecar.drive;
  const model = call('probe_curve', JSON.stringify(net), '{}', 'p_drp');
  expect(curves[0].model).toEqual(model.level_dB);
  // Readouts: measured, its ±u lines and the model at one point.
  const canvas = card.locator('canvas').first();
  await canvas.focus();
  await page.keyboard.press('Home');
  const i = curves[0].figure ? (await viewHook(page, 'fit', 'curves'))[0].figure.cursor : 0;
  const level = read.level_dB[i];
  const li = (s: string) => card.locator(`.readout li[data-series="${s}"] strong`);
  await expect(li('measured (virtual rig)')).toHaveText(`${level.toFixed(2)} dB SPL`);
  await expect(li('measured +u (1σ)')).toHaveText(`${(level + 0.05).toFixed(2)} dB SPL`);
  await expect(li('model')).toHaveText(`${model.level_dB[i].toFixed(2)} dB SPL`);
});

test('fit: malformed files are refused with the file and the line named', async ({ page }) => {
  await fitView(page);
  const panel = page.locator('#view-fit');
  const file = (name: string, text: string) => ({ name, mimeType: 'text/plain', buffer: Buffer.from(text) });
  await panel.locator('input[data-input="curves"]').setInputFiles([
    file('short.frd', '* a response\n20 90.1 0\n25 91.2\n31.5 92 1\n'),
    file('twice.zma', '20 30 0\n20 31 1\n'),
    file('empty.csv', 'frequency_Hz,level_dB\n'),
    file('odd.frd', '20 90 0\n25 91 0\n'),
    file('odd.frd.sidecar.json', '{"schema": "acoustilab-curve-sidecar/0.1", "fixtur": "GRAS 45CA"}'),
    file('broken.json', '{"schema": '),
  ]);
  const errors = panel.locator('.mv-error li');
  await expect(errors).toHaveCount(5);
  await expect(errors.filter({ hasText: '“short.frd” could not be read' })).toContainText('line 3: 2 numbers, but line 2 has 3 (line 3: “25 91.2”)');
  await expect(errors.filter({ hasText: '“twice.zma”' })).toContainText('line 2: frequency 20 Hz appears twice');
  await expect(errors.filter({ hasText: '“empty.csv”' })).toContainText('no data');
  await expect(errors.filter({ hasText: '“odd.frd”' })).toContainText("unknown key 'fixtur'");
  await expect(errors.filter({ hasText: '“broken.json”' })).toContainText('is not valid JSON');
  await expect(panel.locator('li.mv-curve')).toHaveCount(0);
  await expect(panel.getByRole('button', { name: 'Run fit', exact: true })).toBeDisabled();

  // Drag and drop: a curve file and its sidecar dropped together are read as a pair.
  const call = await engine();
  const ref = call('virtual_measure', TEMPLATE, JSON.stringify(rigSpec()));
  const dt = await page.evaluateHandle(
    ([frd, sidecar]) => {
      const d = new DataTransfer();
      d.items.add(new File([frd], 'dropped.frd', { type: 'text/plain' }));
      d.items.add(new File([sidecar], 'dropped.frd.sidecar.json', { type: 'application/json' }));
      return d;
    },
    [ref.text, ref.sidecar],
  );
  const zone = panel.locator('[data-drop="curves"]');
  await zone.dispatchEvent('dragover', { dataTransfer: dt });
  await expect(zone).toHaveClass(/over/);
  await zone.dispatchEvent('drop', { dataTransfer: dt });
  await expect(zone).not.toHaveClass(/over/);
  const card = curveCard(page, 'dropped.frd');
  await expect(card).toContainText('VIRTUAL RIG · synthetic, not measured');
  await expect(card.locator('[data-field="compat"]')).toContainText('Compatible', { timeout: 30_000 });
  await expect(panel.locator('.mv-error li')).toHaveCount(0);
});

test('fit: the compatibility check names the blocking fields; suggestion, SPL-only refusal and cancel', async ({ page }) => {
  await fitView(page);
  await generate(page);
  const panel = page.locator('#view-fit');
  const call = await engine();
  const ref = call('virtual_measure', TEMPLATE, JSON.stringify(rigSpec()));
  // The same data stating a fixture and another drive than the netlist's.
  const sc = { ...JSON.parse(ref.sidecar), fixture: 'GRAS 45CA', drive: { power_mW: 10, rated_ohm: 32 } };
  await panel.locator('input[data-input="curves"]').setInputFiles([
    { name: 'fixture.frd', mimeType: 'text/plain', buffer: Buffer.from(ref.text) },
    { name: 'fixture.frd.sidecar.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify(sc)) },
  ]);
  const card = curveCard(page, 'fixture.frd');
  const compat = card.locator('[data-field="compat"]');
  await expect(compat).toContainText('Blocked against the model’s probe p_drp', { timeout: 30_000 });
  const curve = call('import_curve', ref.text, JSON.stringify({ format: 'frd', sidecar: sc }));
  const cmp = call('compare_curves', JSON.stringify(curve), JSON.stringify(call('probe_curve', TEMPLATE, '', 'p_drp')), '[]');
  expect(cmp.blocking.map((d: { field: string }) => d.field).sort()).toEqual(['drive', 'fixture']);
  await expect(compat).toContainText(cmp.message);
  await panel.locator('input[data-k="param-leak_gap_mm"]').check();
  await expect(panel.getByRole('button', { name: 'Run fit', exact: true })).toBeDisabled();
  await expect(panel.getByText(/^Not compatible with the model \(see each curve\): fixture\.frd\./)).toBeVisible();
  for (const field of ['fixture', 'drive']) await compat.getByLabel(new RegExp(`^Allow the difference in ${field}`)).check();
  await expect(compat).toContainText('Compatible with the model’s probe p_drp (engine check), allowing fixture, drive', { timeout: 30_000 });
  await expect(panel.getByRole('button', { name: 'Run fit', exact: true })).toBeEnabled();
  // Fitted with the fixture difference allowed: the report and Apply say so
  // (the drive is not listed: the fit simulates the curve at its own drive),
  // and no value is proposed for the netlist.
  await panel.getByLabel('At most iterations').fill('4');
  await panel.getByRole('button', { name: 'Run fit', exact: true }).click();
  await expect(panel.locator('.mv-job[data-state="done"]')).toBeVisible({ timeout: 60_000 });
  const fixtureDiff = cmp.blocking.find((d: { field: string }) => d.field === 'fixture');
  const allowed = `The fit allowed differences between the curves and the model: fixture.frd: fixture (curve ${fixtureDiff.a}; model ${fixtureDiff.b}).`;
  await expect(panel.locator('.mv-report [data-field="cautions"]')).toContainText(allowed);
  await expect(panel.locator('.mv-apply [data-field="apply-cautions"]')).toContainText(allowed);
  await expect(panel.locator('.mv-apply [data-field="apply-cautions"]')).not.toContainText('drive (');
  await expect(panel.locator('.mv-apply tr[data-param="leak_gap_mm"] input[type="checkbox"]')).not.toBeChecked();
  await panel.getByLabel('At most iterations').fill('100');
  await card.getByRole('button', { name: 'Remove' }).click();
  await panel.getByRole('button', { name: 'Load as a measurement' }).click();
  await expect(curveCard(page, 'virtual-rig-p_drp-seed7.frd').locator('[data-field="compat"]')).toContainText('Compatible', { timeout: 30_000 });

  // Suggestion: each parameter alone. The leak is determined; driver parameters are refused on pressure data alone.
  await panel.getByRole('button', { name: 'Suggest: fit each parameter alone' }).click();
  await expect(panel.getByText(/^Checked \d+ parameters\.$/)).toBeVisible({ timeout: 60_000 });
  await expect(panel.locator('tr[data-param="leak_gap_mm"] td').first()).toHaveText('determined');
  await expect(panel.locator('tr[data-param="driver_fs_Hz"] td').first()).toContainText('refused');
  await expect(panel.getByText('refusing to fit driver parameters (driver_fs_Hz) to pressure curves alone', { exact: false })).toBeVisible();

  // The engine refuses a driver parameter on SPL alone, and says why.
  await panel.locator('input[data-k="param-leak_gap_mm"]').uncheck();
  await panel.locator('input[data-k="param-driver_fs_Hz"]').check();
  await panel.getByRole('button', { name: 'Run fit', exact: true }).click();
  await expect(panel.locator('.mv-error').filter({ hasText: 'Fit refused' })).toContainText('spec Section 12');

  // Cancel a running fit: the job says so and a new run works.
  await panel.locator('input[data-k="param-driver_fs_Hz"]').uncheck();
  for (const n of ['leak_gap_mm', 'front_depth_mm', 'front_radius_mm', 'rear_volume_cm3', 'vent_mesh_rayl']) await panel.locator(`input[data-k="param-${n}"]`).check();
  await panel.getByRole('button', { name: 'Run fit', exact: true }).click();
  await panel.locator('.mv-job-cancel:visible').click();
  await expect(panel.locator('.mv-job[data-state="cancelled"]')).toBeVisible();
  await expect(panel.getByRole('button', { name: 'Run fit', exact: true })).toBeEnabled({ timeout: 30_000 });
});

test('fit: keyboard, axe in both themes with a report and the sidecar form open, and 390 px', async ({ page }) => {
  await fitView(page);
  await generate(page);
  const panel = page.locator('#view-fit');
  await panel.getByRole('button', { name: 'Load as a measurement' }).click();
  const card = curveCard(page, 'virtual-rig-p_drp-seed7.frd');
  await expect(card.locator('[data-field="compat"]')).toContainText('Compatible', { timeout: 30_000 });
  await panel.locator('input[data-param="leak_gap_mm"]').check();
  // Keyboard: the Run button is reachable and works from the keyboard.
  await panel.getByRole('button', { name: 'Run fit', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(panel.locator('.mv-job[data-state="done"]')).toBeVisible({ timeout: 60_000 });
  // A setting that re-renders its card keeps the focus: fitting the phase of
  // this pressure curve adds its phase plot.
  const phase = card.locator('select[data-k="phase"]');
  await phase.focus();
  await phase.selectOption('yes');
  await expect(card.locator('figure[data-group="phase"]')).toBeVisible();
  await expect(card.locator('select[data-k="phase"]')).toBeFocused();
  await card.getByRole('button', { name: 'Edit sidecar' }).click();
  await expect(card.getByRole('button', { name: 'Apply sidecar' })).toBeVisible();
  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    expect(await axe(page), theme).toEqual([]);
  }
  // The sidecar form edits the file's statements; the engine checks them.
  await card.getByLabel('Fixture', { exact: true }).fill('GRAS 45CA');
  await card.getByRole('button', { name: 'Apply sidecar' }).click();
  await expect(card.locator('[data-field="compat"]')).toContainText('Blocked', { timeout: 30_000 });
  await expect(card).toContainText('GRAS 45CA');
  await page.setViewportSize({ width: 390, height: 844 });
  await noSideScroll(page);
});

test('fit: virtual-rig data stay labelled without their sidecar; files dropped beside the zone are refused', async ({ page }) => {
  await fitView(page);
  await generate(page);
  const panel = page.locator('#view-fit');
  const rig = panel.locator('[data-rig="result"]');
  const [d1] = await Promise.all([page.waitForEvent('download'), rig.getByRole('button', { name: 'Download virtual-rig-p_drp-seed7.frd', exact: true }).click()]);
  const frd = readFileSync((await d1.path())!, 'utf8');
  // The data file alone, under a name that does not tell.
  await panel.locator('input[data-input="curves"]').setInputFiles([{ name: 'response.frd', mimeType: 'text/plain', buffer: Buffer.from(frd) }]);
  const card = curveCard(page, 'response.frd');
  await expect(card.locator('.mv-curve-head .mv-badge')).toHaveText('VIRTUAL RIG · synthetic, not measured (so marked in the file; its sidecar does not say so)');
  await expect(card.locator('.legend-item[data-series="measured (virtual rig)"]')).toBeVisible({ timeout: 30_000 });
  // Exported again: the mark goes with the CSV, and the engine keeps it on reading.
  const [d2] = await Promise.all([page.waitForEvent('download'), card.getByRole('button', { name: 'Download curve (CSV + sidecar)' }).click()]);
  const csv = readFileSync((await d2.path())!, 'utf8');
  expect(csv.split('\n')[0]).toBe(`# ${MARK} (exported from response.frd)`);
  const call = await engine();
  expect(call('import_curve', csv, JSON.stringify({ format: 'csv' })).comments[0]).toBe(`${MARK} (exported from response.frd)`);

  // A file dropped on the view beside the drop zone is not handed to the
  // browser (which would replace the page with it), and nothing is read.
  const prevented = await page.evaluate(() => {
    const d = new DataTransfer();
    d.items.add(new File(['20 90 0\n25 91 0\n'], 'beside.frd', { type: 'text/plain' }));
    const target = document.querySelector('#view-fit .mv-curves')!;
    const over = new DragEvent('dragover', { dataTransfer: d, bubbles: true, cancelable: true });
    target.dispatchEvent(over);
    const drop = new DragEvent('drop', { dataTransfer: d, bubbles: true, cancelable: true });
    target.dispatchEvent(drop);
    return [over.defaultPrevented, drop.defaultPrevented];
  });
  expect(prevented).toEqual([true, true]);
  await expect(panel.locator('li.mv-curve')).toHaveCount(1);
});

test('fit: a fit resumed in bounded calls reports what one engine call reports, parameter scales included; Continue resumes a capped run', async ({ page }) => {
  test.setTimeout(120_000);
  // driver_bench: impedance in free air and with a 300 mg test mass. Le_uH
  // starts at 0, so the engine fits it on a linear scale; a resumed call
  // starting from a positive value would pick a log scale unless told.
  const BENCH = example('driver_bench');
  await open(page, 'driver_bench');
  await openView(page, 'Fit');
  const panel = page.locator('#view-fit');
  const rigForm = panel.getByRole('group', { name: 'Virtual rig settings' });
  await rigForm.getByLabel('Probe', { exact: true }).selectOption('zin');
  await rigForm.getByLabel('Level noise (dB)').fill('0.05');
  await rigForm.getByLabel('Phase noise (°)').fill('0.3');
  const truth = { Bl_Tm: 4, Mms_g: 0.8, Cms_mm_per_N: 5, Le_uH: 1 };
  for (const [seed, extra] of [
    [11, ''],
    [12, ', added_mass_mg=300'],
  ] as const) {
    await rigForm.getByLabel('Seed').fill(String(seed));
    await rigForm.getByLabel('True values that differ from the netlist').fill(`Bl_Tm=4, Mms_g=0.8, Cms_mm_per_N=5, Le_uH=1${extra}`);
    await panel.getByRole('button', { name: 'Generate synthetic measurement' }).click();
    await expect(panel.locator('[data-rig="result"] .mv-curve-name')).toHaveText(`virtual-rig-zin-seed${seed}.zma`);
    await panel.getByRole('button', { name: 'Load as a measurement' }).click();
    await expect(curveCard(page, `virtual-rig-zin-seed${seed}.zma`).locator('[data-field="compat"]')).toContainText('Compatible', { timeout: 30_000 });
  }
  const cond = curveCard(page, 'virtual-rig-zin-seed12.zma').getByLabel('Measurement condition');
  await cond.fill('added_mass_mg=300');
  await cond.press('Tab');
  const names = ['Re_ohm', 'Bl_Tm', 'Mms_g', 'Cms_mm_per_N', 'Rms_Ns_per_m', 'Le_uH'];
  for (const n of names) await panel.locator(`input[data-param="${n}"]`).check();

  // The oracle: one engine call on the same data.
  const call = await engine();
  const rig = (seed: number, overrides: Record<string, number>) =>
    call(
      'virtual_measure',
      BENCH,
      JSON.stringify({
        probe: 'zin',
        f_min_Hz: 10,
        f_max_Hz: 20000,
        points_per_octave: 12,
        noise: { seed, level_dB: 0.05, phase_deg: 0.3, seatings: 1, averaging: 'complex', repositioning_dB: 0, repositioning_delay_us: 0, microphone_offset_dB: 0, microphone_slope_dB_per_decade: 0, coupler_dB: 0 },
        overrides,
      }),
    );
  const read = (x: { text: string; sidecar: string }) => call('import_curve', x.text, JSON.stringify({ format: 'zma', sidecar: JSON.parse(x.sidecar) }));
  const single = call(
    'fit',
    BENCH,
    JSON.stringify({
      schema: 'acoustilab-fit/0.1',
      parameters: names,
      curves: [
        { probe: 'zin', curve: read(rig(11, truth)) },
        { probe: 'zin', curve: read(rig(12, { ...truth, added_mass_mg: 300 })), overrides: { added_mass_mg: 300 } },
      ],
      f_min_Hz: 10,
      f_max_Hz: 20000,
      max_iterations: 100,
    }),
  );
  expect(single.converged).toBe(true);
  expect(single.iterations).toBeGreaterThan(4);
  const same = (report: { parameters: { name: string; scale: string; status: string; value: number; sd: number | null }[] }) => {
    for (const [k, p] of report.parameters.entries()) {
      const q = single.parameters[k];
      expect(p.name).toBe(q.name);
      expect(p.scale, p.name).toBe(q.scale);
      expect(p.status, p.name).toBe(q.status);
      expect(Math.abs(p.value / q.value - 1), p.name).toBeLessThan(1e-6);
      if (q.sd !== null) expect(Math.abs(p.sd! / q.sd - 1), p.name).toBeLessThan(1e-3);
    }
  };

  // Capped at 4 iterations: stopped unconverged, and said so against the cap.
  await panel.getByLabel('At most iterations').fill('4');
  await panel.getByRole('button', { name: 'Run fit', exact: true }).click();
  await expect(panel.locator('.mv-job[data-state="done"]')).toContainText('Stopped without converging after 4 iterations', { timeout: 60_000 });
  await expect(panel.locator('.mv-report [data-field="stop"]')).toContainText('Not converged: stopped at the cap of 4 iterations before converging.');
  // Continue: resumed from the fitted values with the first call's scales.
  await panel.getByRole('button', { name: 'Continue from the fitted values (up to 4 more iterations)' }).click();
  await expect(panel.locator('.mv-job[data-state="done"]')).toContainText('Converged', { timeout: 60_000 });
  const run = await viewHook(page, 'fit', 'run');
  expect(run.calls).toBeGreaterThan(1);
  expect(run.iterations).toBeGreaterThan(4);
  const report = await viewHook(page, 'fit', 'report');
  same(report);
  expect(report.parameters.find((p: { name: string }) => p.name === 'Le_uH').scale).toBe('linear');
  // Apply proposes the determined and weakly determined parameters only.
  for (const q of single.parameters) {
    const box = panel.locator(`.mv-apply tr[data-param="${q.name}"] input[type="checkbox"]`);
    if (q.status === 'determined' || q.status === 'weakly_determined') await expect(box, q.name).toBeChecked();
    else await expect(box, q.name).not.toBeChecked();
  }

  // Uncapped, in calls of a few iterations: the same report again.
  await panel.getByLabel('At most iterations').fill('100');
  await panel.getByRole('button', { name: 'Run fit', exact: true }).click();
  await expect(panel.locator('.mv-job[data-state="done"]')).toContainText('Converged', { timeout: 60_000 });
  expect((await viewHook(page, 'fit', 'run')).calls).toBeGreaterThan(1);
  same(await viewHook(page, 'fit', 'report'));
});

test('fit: the fitted model is drawn with the curve’s fitted level offset', async ({ page }) => {
  await fitView(page);
  const panel = page.locator('#view-fit');
  // A sensor calibration error: the budget's calibration term becomes a level offset with that prior.
  await panel.getByLabel('Sensor offset (dB)').fill('0.5');
  await generate(page);
  await panel.getByRole('button', { name: 'Load as a measurement' }).click();
  const card = curveCard(page, 'virtual-rig-p_drp-seed7.frd');
  await expect(card.locator('[data-field="compat"]')).toContainText('Compatible', { timeout: 30_000 });
  await panel.locator('input[data-param="leak_gap_mm"]').check();
  await panel.getByRole('button', { name: 'Run fit', exact: true }).click();
  await expect(panel.locator('.mv-job[data-state="done"]')).toContainText('Converged', { timeout: 60_000 });
  const report = await viewHook(page, 'fit', 'report');
  expect(report.offsets).toHaveLength(1);
  const off = report.offsets[0].value_dB;
  expect(Math.abs(off)).toBeGreaterThan(0.01);
  await expect(card.locator('.legend-item[data-series="model, fitted values and offset"]')).toBeVisible({ timeout: 30_000 });
  // The oracle: the probe with the fitted values at the curve's frequencies and drive, plus the offset.
  const call = await engine();
  const ref = call('virtual_measure', TEMPLATE, JSON.stringify(rigSpec({ microphoneOffset: 0.5 })));
  const read = call('import_curve', ref.text, JSON.stringify({ format: 'frd', sidecar: JSON.parse(ref.sidecar) }));
  const net = JSON.parse(TEMPLATE);
  net.sweep = { frequencies_Hz: read.frequencies_Hz };
  net.drive = read.sidecar.drive;
  const model = call('probe_curve', JSON.stringify(net), JSON.stringify(report.fitted), 'p_drp');
  const c = (await viewHook(page, 'fit', 'curves'))[0];
  expect(c.fitted.offsetDb).toBe(off);
  expect(c.fitted.values).toEqual(model.level_dB);
  const canvas = card.locator('canvas').first();
  await canvas.focus();
  for (const key of ['Home', 'End']) {
    await page.keyboard.press(key);
    const i = (await viewHook(page, 'fit', 'curves'))[0].figure.cursor;
    await expect(card.locator('.readout li[data-series="model, fitted values and offset"] strong')).toHaveText(`${(model.level_dB[i] + off).toFixed(2)} dB SPL`);
  }
});
