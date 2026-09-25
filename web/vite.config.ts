import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';

// The engine is the wasm-bindgen output of crates/acoustilab-wasm (built by
// scripts/build-wasm.sh); example netlists come from the repository's
// examples/ directory. Both live outside web/, so the dev server may read
// the repository root.
const repoRoot = fileURLToPath(new URL('..', import.meta.url));

export default defineConfig({
  // Relative asset URLs: dist/ can be served from any path.
  base: './',
  resolve: {
    alias: {
      '@engine': fileURLToPath(new URL('../crates/acoustilab-wasm/pkg', import.meta.url)),
    },
  },
  server: {
    fs: { allow: [repoRoot] },
  },
  worker: { format: 'es' },
  build: {
    target: 'es2022',
    // The engine .wasm must stay a separate file (fetched by the worker).
    assetsInlineLimit: 0,
  },
});
