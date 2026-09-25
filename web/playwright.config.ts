import { defineConfig, devices } from '@playwright/test';

// Smoke tests against the production build (`npm test` builds first), served
// by `vite preview`. Uses the preinstalled Chromium (PLAYWRIGHT_BROWSERS_PATH);
// @playwright/test is pinned to the version that matches that browser build.
// PW_PORT moves the preview server off 4173 (two checkouts testing at once).
const port = Number(process.env.PW_PORT ?? 4173);
export default defineConfig({
  testDir: './tests',
  timeout: 60_000,
  fullyParallel: false,
  workers: 1,
  reporter: [['list']],
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    viewport: { width: 1440, height: 900 },
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } }],
  webServer: {
    command: `npx vite preview --host 127.0.0.1 --port ${port} --strictPort`,
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
