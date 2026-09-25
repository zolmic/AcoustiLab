import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';

// The engine is the wasm-bindgen output of crates/acoustilab-wasm (built by
// scripts/build-wasm.sh); example netlists come from the repository's
// examples/ directory. Both live outside web/, so the dev server may read
// exactly those two directories besides web/ itself, and not the rest of the
// repository (in particular not the untracked private/ directory of licensed
// standards data, which `vite --host` would otherwise serve to the network).
const dir = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));
const pkgDir = dir('../crates/acoustilab-wasm/pkg');

export default defineConfig({
  // Relative asset URLs: dist/ can be served from any path.
  base: './',
  resolve: {
    alias: {
      '@engine': pkgDir,
    },
  },
  server: {
    fs: { allow: [dir('.'), dir('../examples'), pkgDir] },
  },
  worker: { format: 'es' },
  build: {
    target: 'es2022',
    // The engine .wasm must stay a separate file (fetched by the worker).
    assetsInlineLimit: 0,
  },
});
