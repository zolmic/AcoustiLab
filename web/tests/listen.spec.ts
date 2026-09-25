// The Listen view and the audition chain in Chromium (spec Section 16).
//
// The processing tests render the built AudioWorklet module (the bundle's
// processor-*.js) in an OfflineAudioContext, with a filter designed by the
// engine (the wasm package, loaded in Node here), and compare what comes out
// with references computed in Node: direct convolution, the engine's
// analytic filter, a 16× true-peak reconstruction, and BS.1770 loudness.
// Real-time playback cannot be heard in a test; the UI tests check that it
// starts, meters and switches.

import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { copyFileSync, mkdtempSync, readdirSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import type * as Engine from '../../crates/acoustilab-wasm/pkg/acoustilab_wasm.js';
import { circularConvolve, delayTaps, matchGainDb, measure } from '../src/audio/level';
import { integratedLoudness } from '../src/audio/loudness';
import { pinkKellet } from '../src/audio/noise';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const TEMPLATE = readFileSync(`${repo}/examples/design_over_ear.json`, 'utf8');
// The engine in Node: wasm-bindgen's `--target web` glue is an ES module
// without a package.json saying so, so a copy named .mjs is imported, and
// initialised from the bytes of the same .wasm the bundle serves.
const glue = join(mkdtempSync(join(tmpdir(), 'acoustilab-')), 'acoustilab_wasm.mjs');
copyFileSync(`${repo}/crates/acoustilab-wasm/pkg/acoustilab_wasm.js`, glue);
const engine = (await import(pathToFileURL(glue).href)) as typeof Engine;
engine.initSync({ module: readFileSync(`${repo}/crates/acoustilab-wasm/pkg/acoustilab_wasm_bg.wasm`) });

const FS = 48000;
const worklet = () => {
  const f = readdirSync(`${repo}/web/dist/assets`).find((n) => /^processor-.*\.js$/.test(n));
  if (!f) throw new Error('no processor chunk in web/dist/assets (run npm run build)');
  return `/assets/${f}`;
};

interface Filter {
  taps: number[];
  latency_samples: number;
  frequencies_Hz: number[];
  design_dB: number[];
  check: { met: boolean; max_abs_error_dB: number };
  n: number;
}

/** The engine's filter: the template with 3 vents against the template. */
function engineFilter(options: Record<string, unknown> = {}): Filter {
  const v = JSON.parse(
    engine.audition_filter(
      JSON.stringify({ netlist: TEMPLATE, overrides: { vent_count: 3 } }),
      JSON.stringify({ kind: 'netlist', netlist: TEMPLATE }),
      JSON.stringify(options),
    ),
  );
  if (v.error) throw new Error(v.error);
  return v as Filter;
}

function noise(n: number, seed: number): number[] {
  // A small LCG: any wideband signal will do here.
  let s = seed >>> 0;
  return Array.from({ length: n }, () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 2 ** 31 - 1;
  });
}

function directConvolve(x: ArrayLike<number>, h: ArrayLike<number>, n: number): Float64Array {
  const y = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    let s = 0;
    for (let m = 0; m < h.length && m <= i; m++) s += h[m] * x[i - m];
    y[i] = s;
  }
  return y;
}

interface RenderSpec {
  url: string;
  inputs: number[][];
  length: number;
  /** Messages posted before rendering. */
  messages: unknown[];
  /** Messages posted at render times (s, a multiple of 128/fs). */
  events?: { t: number; msg: unknown }[];
  volume?: number;
  /** Context rate (default 48 kHz). */
  fs?: number;
}

/** Renders the worklet offline; returns [left, right, limiter gain, state] per sample. */
async function render(page: Page, spec: RenderSpec): Promise<number[][]> {
  return page.evaluate(async (s) => {
    const rate = s.fs ?? 48000;
    const ctx = new OfflineAudioContext(4, s.length, rate);
    await ctx.audioWorklet.addModule(s.url);
    const node = new AudioWorkletNode(ctx, 'acoustilab-audition', {
      numberOfInputs: 1,
      numberOfOutputs: 3,
      outputChannelCount: [2, 1, 1],
      processorOptions: { maxTaps: 16384, ceilingDb: -1 },
    });
    node.parameters.get('volume')!.value = s.volume ?? 1;
    const buf = new AudioBuffer({ length: s.inputs[0].length, numberOfChannels: 2, sampleRate: rate });
    buf.copyToChannel(Float32Array.from(s.inputs[0]), 0);
    buf.copyToChannel(Float32Array.from(s.inputs[1] ?? s.inputs[0]), 1);
    const src = new AudioBufferSourceNode(ctx, { buffer: buf });
    const split = new ChannelSplitterNode(ctx, { numberOfOutputs: 2 });
    const merge = new ChannelMergerNode(ctx, { numberOfInputs: 4 });
    src.connect(node);
    node.connect(split, 0);
    split.connect(merge, 0, 0);
    split.connect(merge, 1, 1);
    node.connect(merge, 1, 2);
    node.connect(merge, 2, 3);
    merge.connect(ctx.destination);
    const toArrays = (m: unknown) => {
      const o = m as Record<string, unknown>;
      return { ...o, left: o.left ? Float32Array.from(o.left as number[]) : undefined, right: o.right ? Float32Array.from(o.right as number[]) : undefined };
    };
    for (const m of s.messages) node.port.postMessage(toArrays(m));
    await new Promise((r) => setTimeout(r, 300));
    for (const e of s.events ?? []) {
      void ctx.suspend(e.t).then(async () => {
        node.port.postMessage(toArrays(e.msg));
        await new Promise((r) => setTimeout(r, 150));
        await ctx.resume();
      });
    }
    src.start(0);
    const out = await ctx.startRendering();
    return [0, 1, 2, 3].map((c) => Array.from(out.getChannelData(c)));
  }, spec);
}

const LIMITER_LATENCY = 16 + 96;

test.describe('audition chain (OfflineAudioContext)', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
  });

  test('partitioned convolution in the worklet equals direct convolution to 1e-6', async ({ page }) => {
    const f = engineFilter();
    expect(f.n).toBe(8192);
    const n = 1 << 15;
    const xl = noise(n, 1).map((v) => v * 0.25);
    const xr = noise(n, 2).map((v) => v * 0.25);
    const out = await render(page, {
      url: worklet(),
      inputs: [xl, xr],
      length: n,
      messages: [
        { type: 'limiter', enabled: false },
        { type: 'load', slot: 0, left: f.taps, right: f.taps, gain: 1 },
      ],
    });
    // The first load completes within 8192/128/4 = 16 quanta; the filter
    // then convolves the whole input history: exact from there on.
    const state = out[3];
    const from = state.findIndex((v, i) => i > 0 && v === 0 && state[i - 1] !== 0);
    expect(from).toBeGreaterThan(0);
    expect(from).toBeLessThanOrEqual(17 * 128);
    const taps = Float32Array.from(f.taps);
    const yl = directConvolve(Float32Array.from(xl), taps, n);
    const yr = directConvolve(Float32Array.from(xr), taps, n);
    let worst = 0;
    for (let i = from + LIMITER_LATENCY; i < n; i++) {
      worst = Math.max(worst, Math.abs(out[0][i] - yl[i - LIMITER_LATENCY]), Math.abs(out[1][i] - yr[i - LIMITER_LATENCY]));
    }
    expect(worst).toBeLessThan(1e-6);
  });

  test('the chain reproduces the analytic filter within 0.1 dB from 20 Hz to 20 kHz', async ({ page }) => {
    for (const phase of ['minimum', 'mixed']) {
      const f = engineFilter({ phase });
      expect(f.check.met).toBe(true);
      const at = 4096;
      const n = 1 << 14;
      const impulse = new Array(n).fill(0);
      impulse[at] = 1;
      const out = await render(page, {
        url: worklet(),
        inputs: [impulse, impulse],
        length: n,
        messages: [
          { type: 'limiter', enabled: false },
          { type: 'load', slot: 0, left: f.taps, right: f.taps, gain: 1 },
        ],
      });
      const start = at + LIMITER_LATENCY;
      const ir = out[0].slice(start, start + f.n);
      let worst = 0;
      f.frequencies_Hz.forEach((fr, i) => {
        const w = (2 * Math.PI * fr) / FS;
        let re = 0;
        let im = 0;
        for (let k = 0; k < ir.length; k++) {
          re += ir[k] * Math.cos(w * k);
          im -= ir[k] * Math.sin(w * k);
        }
        worst = Math.max(worst, Math.abs(20 * Math.log10(Math.hypot(re, im)) - f.design_dB[i]));
      });
      expect(worst, `${phase}: ${worst} dB`).toBeLessThan(0.1);
      // And it is the engine's own check, to f32 rounding.
      expect(Math.abs(worst - f.check.max_abs_error_dB)).toBeLessThan(1e-3);
    }
  });

  test('at 96 kHz: the filter designed for the rate, through the chain, within 0.1 dB', async ({ page }) => {
    const fs = 96000;
    const f = engineFilter({ fs_Hz: fs });
    expect(f.n).toBe(16384);
    expect(f.check.met).toBe(true);
    // 16 384 taps load in 32 quanta; the impulse comes after that.
    const at = 8192;
    const n = 1 << 15;
    const impulse = new Array(n).fill(0);
    impulse[at] = 1;
    const out = await render(page, {
      url: worklet(),
      inputs: [impulse, impulse],
      length: n,
      fs,
      messages: [
        { type: 'limiter', enabled: false },
        { type: 'load', slot: 0, left: f.taps, right: f.taps, gain: 1 },
      ],
    });
    // The limiter's delay scales with the rate: 16 + 2 ms.
    const start = at + 16 + Math.round(0.002 * fs);
    const ir = out[0].slice(start, start + f.n);
    let worst = 0;
    f.frequencies_Hz.forEach((fr, i) => {
      const w = (2 * Math.PI * fr) / fs;
      let re = 0;
      let im = 0;
      for (let k = 0; k < ir.length; k++) {
        re += ir[k] * Math.cos(w * k);
        im -= ir[k] * Math.sin(w * k);
      }
      worst = Math.max(worst, Math.abs(20 * Math.log10(Math.hypot(re, im)) - f.design_dB[i]));
    });
    expect(worst, `${worst} dB`).toBeLessThan(0.1);
    expect(Math.abs(worst - f.check.max_abs_error_dB)).toBeLessThan(1e-3);
  });

  test('A/B crossfade: linear over two quanta, no discontinuity', async ({ page }) => {
    const n = 1 << 15;
    const sine = Array.from({ length: n }, (_, i) => 0.5 * Math.sin((2 * Math.PI * 441 * i) / FS));
    // A: the identity; B: −6 dB and a 12-sample delay (a different phase).
    const b = new Array(13).fill(0);
    b[12] = 0.5;
    const t = (64 * 128) / FS;
    const out = await render(page, {
      url: worklet(),
      inputs: [sine, sine],
      length: n,
      messages: [
        { type: 'limiter', enabled: false },
        { type: 'load', slot: 0, left: [1], right: [1], gain: 1 },
        { type: 'load', slot: 1, left: b, right: b, gain: 1 },
      ],
      events: [{ t, msg: { type: 'select', slot: 1 } }],
    });
    const state = out[3];
    const fadeStart = state.findIndex((v) => (v & 4) !== 0);
    expect(fadeStart % 128).toBe(0);
    expect(state[fadeStart + 256]).toBe(1);
    // Exactly (1 − g)·A + g·B with g = (k + 1)/256 over the fade.
    const a = (i: number) => sine[i];
    const bb = (i: number) => (i >= 12 ? 0.5 * sine[i - 12] : 0);
    let worst = 0;
    for (let k = -512; k < 768; k++) {
      const i = fadeStart + k;
      const g = k < 0 ? 0 : Math.min(1, (k + 1) / 256);
      // The limiter (disabled) delays the convolver's output by its latency.
      const want = (1 - g) * a(i) + g * bb(i);
      worst = Math.max(worst, Math.abs(out[0][i + LIMITER_LATENCY] - want));
    }
    expect(worst).toBeLessThan(1e-6);
    // No step: the sample-to-sample change never exceeds the steady
    // signals' own largest change (0.5·2π·441/48000 = 0.0289).
    let jump = 0;
    for (let i = fadeStart - 256; i < fadeStart + 768; i++) {
      const j = i + LIMITER_LATENCY;
      jump = Math.max(jump, Math.abs(out[0][j] - out[0][j - 1]));
    }
    expect(jump).toBeLessThan(0.5 * 2 * Math.PI * (441 / FS) * 1.001);
  });

  test('the output limiter holds −1 dBTP', async ({ page }) => {
    const n = 1 << 16;
    const x = Array.from(pinkKellet(1 << 16, 3, FS).map((v) => v * 12));
    const bursts = x.map((v, i) => (i > 30000 && i < 32000 ? 1.8 * Math.sin((Math.PI / 2) * i + Math.PI / 4) : v));
    const out = await render(page, {
      url: worklet(),
      inputs: [bursts, x],
      length: n,
      messages: [{ type: 'load', slot: 0, left: [1], right: [1], gain: 1 }],
    });
    const tp = Math.max(truePeak16(out[0].slice(2048)), truePeak16(out[1].slice(2048)));
    expect(20 * Math.log10(tp)).toBeLessThanOrEqual(-1 + 1e-3);
    expect(out[2].slice(2048).reduce((m, v) => Math.min(m, v), 1)).toBeLessThan(0.3);
  });

  test('A and B, level matched over the programme, render within 0.1 LU', async ({ page }) => {
    const f = engineFilter();
    const loop = pinkKellet(1 << 17, 7, FS);
    const prog = [loop, loop];
    const a = Float32Array.from(f.taps);
    const b = delayTaps(f.latency_samples);
    const plain = measure(prog, FS);
    const ga = matchGainDb(measure([circularConvolve(loop, a), circularConvolve(loop, a)], FS), 'bs1770', plain);
    const gb = matchGainDb(measure([circularConvolve(loop, b), circularConvolve(loop, b)], FS), 'bs1770', plain);
    const three = Array.from({ length: 3 * loop.length }, (_, i) => loop[i % loop.length]);
    const loud: number[] = [];
    for (const [taps, g] of [[a, ga], [b, gb]] as [Float32Array, number][]) {
      const out = await render(page, {
        url: worklet(),
        inputs: [three, three],
        length: three.length,
        messages: [
          { type: 'limiter', enabled: false },
          { type: 'load', slot: 0, left: Array.from(taps), right: Array.from(taps), gain: 10 ** (g / 20) },
        ],
      });
      const s = 2 * loop.length;
      loud.push(integratedLoudness([out[0].slice(s), out[1].slice(s)], FS).integrated);
    }
    expect(Math.abs(loud[0] - loud[1]), `A ${loud[0]} LUFS, B ${loud[1]} LUFS`).toBeLessThan(0.1);
    expect(Math.abs(loud[0] + 23)).toBeLessThan(0.1);
  });

  test('ConvolverNode fallback: normalize = false before the buffer keeps the filter exact', async ({ page }) => {
    const f = engineFilter();
    const n = 1 << 15;
    const x = noise(n, 5).map((v) => v * 0.25);
    const [exact, normalised] = await page.evaluate(
      async ({ taps, x }) => {
        const run = async (normalize: boolean) => {
          const ctx = new OfflineAudioContext(1, x.length, 48000);
          const node = new ConvolverNode(ctx, { disableNormalization: !normalize });
          node.normalize = normalize;
          const hb = new AudioBuffer({ length: taps.length, numberOfChannels: 1, sampleRate: 48000 });
          hb.copyToChannel(Float32Array.from(taps), 0);
          node.buffer = hb;
          const xb = new AudioBuffer({ length: x.length, numberOfChannels: 1, sampleRate: 48000 });
          xb.copyToChannel(Float32Array.from(x), 0);
          const src = new AudioBufferSourceNode(ctx, { buffer: xb });
          src.connect(node).connect(ctx.destination);
          src.start();
          return Array.from((await ctx.startRendering()).getChannelData(0));
        };
        return [await run(false), await run(true)];
      },
      { taps: f.taps, x },
    );
    const y = directConvolve(Float32Array.from(x), Float32Array.from(f.taps), n);
    let worst = 0;
    let peak = 0;
    for (let i = 0; i < n; i++) {
      worst = Math.max(worst, Math.abs(exact[i] - y[i]));
      peak = Math.max(peak, Math.abs(y[i]));
    }
    expect(worst / peak).toBeLessThan(1e-4);
    // The default normalisation rescales the filter: the level match would be lost.
    const ratio = Math.sqrt(normalised.reduce((s, v) => s + v * v, 0) / exact.reduce((s, v) => s + v * v, 0));
    expect(Math.abs(20 * Math.log10(ratio))).toBeGreaterThan(1);
  });
});

/** 16× windowed-sinc reconstruction (independent of the page's 4× meter). */
function truePeak16(x: ArrayLike<number>): number {
  const half = 48;
  const i0 = (v: number) => {
    let s = 1;
    let t = 1;
    for (let k = 1; k < 300; k++) {
      t *= (v * v) / (4 * k * k);
      s += t;
      if (t < 1e-18 * s) break;
    }
    return s;
  };
  const kern: Float64Array[] = [];
  for (let p = 1; p < 16; p++) {
    const c = new Float64Array(2 * half);
    for (let j = 0; j < 2 * half; j++) {
      const t = p / 16 - (j - half + 1);
      const w = Math.abs(t) < half ? i0(12 * Math.sqrt(1 - (t / half) ** 2)) / i0(12) : 0;
      c[j] = (Math.sin(Math.PI * t) / (Math.PI * t)) * w;
    }
    kern.push(c);
  }
  let peak = 0;
  for (let m = half; m < x.length - half; m++) {
    peak = Math.max(peak, Math.abs(x[m]));
    for (const c of kern) {
      let v = 0;
      for (let j = 0; j < 2 * half; j++) v += c[j] * x[m + j - half + 1];
      peak = Math.max(peak, Math.abs(v));
    }
  }
  return peak;
}

// ----- the view --------------------------------------------------------------------

async function solved(page: Page): Promise<void> {
  await expect(page.locator('body')).toHaveAttribute('data-state', 'solved', { timeout: 20_000 });
}

async function openListen(page: Page): Promise<void> {
  await page.goto('/');
  await solved(page);
  const inc = page.locator('.prow[data-param="vent_count"]').getByRole('button', { name: 'Increase Number of rear vents' });
  await inc.click();
  await solved(page);
  await inc.click();
  await solved(page);
  await page.getByRole('tablist', { name: 'Result views' }).getByRole('tab', { name: 'Listen', exact: true }).click();
  await expect(page.locator('#view-listen')).toBeVisible();
}

async function designed(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Design filter', exact: true }).click();
  await expect(page.locator('#view-listen .listen-status')).toContainText('Filter designed', { timeout: 30_000 });
}

test('Listen view: design against the template, report, plot, table, state', async ({ page }) => {
  await openListen(page);
  const view = page.locator('#view-listen');
  await expect(page.locator('#listen-baseline')).toHaveValue('template');
  await designed(page);
  await expect(view.locator('.listen-status')).toContainText('8192 taps at 48000 Hz, minimum phase');
  const facts = view.locator('.listen-facts');
  await expect(facts).toContainText('candidate ÷ design “template values”, at p_drp');
  await expect(facts).toContainText("Minimum phase: the filter's largest excess group delay");
  await expect(facts).toContainText(/within 0\.1 dB/);
  await expect(view.locator('figure.plot canvas')).toHaveCount(2);
  await expect(view.locator('figure.plot canvas').first()).toHaveAttribute('aria-label', /Audition filter \|H\|.*analytic.*taps/);
  // The data table lists every check-grid point.
  await view.getByText('Data table', { exact: true }).click();
  await expect(view.locator('.table-wrap table tbody tr')).toHaveCount(483);
  // The state names the netlist by the SHA-256 the browser computes too.
  const state = JSON.parse(await page.locator('#listen-state').inputValue());
  const text = await page.locator('#netlist').inputValue();
  const sha = await page.evaluate(async (t) => {
    const d = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(t));
    return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, '0')).join('');
  }, text);
  expect(state.candidate.netlist_sha256).toBe(sha);
  expect(state.baseline.kind).toBe('netlist');
  expect(state.phase).toEqual({ requested: 'auto', used: 'minimum' });
  // Absolute mode is flagged; linear phase is flagged.
  await page.getByLabel('Absolute (diagnostic)').check();
  await expect(view.locator('.listen-flags')).toContainText('Absolute mode', { timeout: 30_000 });
  await page.getByLabel('Difference (candidate ÷ baseline)').check();
  await page.getByLabel('Linear (diagnostic)').check();
  await expect(view.locator('.listen-flags')).toContainText('Linear phase is a diagnostic', { timeout: 30_000 });
  // A target baseline: inverted, band-limited, the fixture mismatch flagged.
  await page.getByLabel('Minimum', { exact: true }).check();
  await page.locator('#listen-baseline').selectOption('target:ravizza2023_5128');
  await designed(page);
  await expect(facts).toContainText('Kirkeby–Nelson');
  await expect(view.locator('.listen-flags')).toContainText('fixture bk5128');
});

test('Listen view: play, meters, A/B, diagnostics, stop', async ({ page }) => {
  await openListen(page);
  await designed(page);
  const view = page.locator('#view-listen');
  // Nothing plays before Play; the level warning is shown.
  await expect(view.locator('.listen-warning')).toContainText('start with your headphone volume low');
  await expect(page.locator('#listen-volume')).toHaveValue('-20');
  await page.getByRole('button', { name: 'Play', exact: true }).click();
  await expect(view.locator('.listen-playing')).toContainText('Playing', { timeout: 30_000 });
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  // Level match over the programme after convolution: A and B at −23 LUFS.
  const matched = view.locator('.listen-level-table tbody tr td:last-child');
  await expect(matched).toHaveCount(2);
  for (const v of await matched.allTextContents()) expect(Math.abs(Number(v) + 23)).toBeLessThanOrEqual(0.01);
  // Meters run.
  await expect(view.locator('.listen-meter-value').first()).toHaveText(/LUFS/, { timeout: 10_000 });
  await page.getByLabel('B: reference (programme alone)').check();
  await expect(view.locator('.listen-playing')).toContainText('Playing B', { timeout: 10_000 });
  await view.getByText('Diagnostics', { exact: true }).click();
  const diag = view.locator('details', { has: page.locator('summary', { hasText: 'Diagnostics' }) });
  await expect(diag).toContainText('48000 Hz');
  await expect(diag).toContainText('AudioWorklet: partitioned convolution');
  await expect(diag).toContainText('128 frames');
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(view.locator('.listen-playing')).toHaveText('Stopped.');
});

for (const theme of ['light', 'dark'] as const) {
  test(`Listen view (${theme}): no axe violations`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: theme });
    await openListen(page);
    await designed(page);
    // Playing: the Stop button, meters and the level-match table are shown.
    await page.getByRole('button', { name: 'Play', exact: true }).click();
    await expect(page.locator('#view-listen .listen-level-table')).toBeVisible({ timeout: 30_000 });
    await page.locator('#view-listen').getByText('Data table', { exact: true }).click();
    await page.locator('#view-listen').getByText('Diagnostics', { exact: true }).click();
    await page.locator('#view-listen').getByText('Audition state', { exact: true }).click();
    const r = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
    expect(r.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
  });
}

test('Listen view: nothing scrolls sideways at 390 px; keyboard reaches the controls', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openListen(page);
  await designed(page);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  await page.locator('#listen-baseline').focus();
  const reached = new Set<string>();
  for (let i = 0; i < 40; i++) {
    await page.keyboard.press('Tab');
    const id = await page.evaluate(() => document.activeElement?.id || document.activeElement?.textContent || '');
    reached.add(id);
  }
  for (const want of ['listen-mode-difference', 'listen-phase-auto', 'Design filter', 'listen-follow', 'listen-programme', 'listen-seed', 'listen-match-bs1770', 'Play', 'listen-ab-a', 'listen-volume', 'listen-volume-num']) {
    expect([...reached], want).toContain(want);
  }
});
