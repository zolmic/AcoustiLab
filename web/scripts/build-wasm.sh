#!/usr/bin/env bash
# Builds the engine for the browser: compiles crates/acoustilab-wasm to
# wasm32-unknown-unknown and runs wasm-bindgen (--target web) into
# crates/acoustilab-wasm/pkg, which the web app imports as "@engine".
#
# The wasm-bindgen CLI must match the wasm-bindgen crate version locked in
# Cargo.lock exactly (the generated glue and the wasm's custom section are
# version-coupled). If the CLI is missing, it is installed with
# `cargo install --locked`; if a different version is on PATH, the script
# stops unless WASM_BINDGEN_INSTALL=1 asks it to install the right one.
#
# Environment:
#   PROFILE=release|dev          cargo profile (default release)
#   WASM_BINDGEN_INSTALL=1       install the matching CLI over a mismatched one
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROFILE="${PROFILE:-release}"
OUT="$ROOT/crates/acoustilab-wasm/pkg"

# Locked wasm-bindgen crate version: the line after `name = "wasm-bindgen"`.
WB_VERSION="$(awk '
  $0 == "name = \"wasm-bindgen\"" { getline; gsub(/version = |"/, ""); print; exit }
' "$ROOT/Cargo.lock")"
if [[ -z "$WB_VERSION" ]]; then
  # Not locked yet (fresh checkout without Cargo.lock): resolve it.
  cargo generate-lockfile --manifest-path "$ROOT/Cargo.toml"
  WB_VERSION="$(awk '
    $0 == "name = \"wasm-bindgen\"" { getline; gsub(/version = |"/, ""); print; exit }
  ' "$ROOT/Cargo.lock")"
fi

installed=""
if command -v wasm-bindgen >/dev/null 2>&1; then
  installed="$(wasm-bindgen --version | awk '{print $2}')"
fi
if [[ "$installed" != "$WB_VERSION" ]]; then
  if [[ -n "$installed" && "${WASM_BINDGEN_INSTALL:-0}" != "1" ]]; then
    echo "error: wasm-bindgen CLI $installed on PATH, but Cargo.lock pins the crate at $WB_VERSION." >&2
    echo "       Run: cargo install wasm-bindgen-cli --version $WB_VERSION --locked" >&2
    echo "       (or rerun with WASM_BINDGEN_INSTALL=1)" >&2
    exit 1
  fi
  echo "installing wasm-bindgen-cli $WB_VERSION"
  cargo install wasm-bindgen-cli --version "$WB_VERSION" --locked
fi

if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
  echo "error: the wasm32-unknown-unknown target is missing: rustup target add wasm32-unknown-unknown" >&2
  exit 1
fi

cargo_profile_flag="--release"
profile_dir="release"
if [[ "$PROFILE" == "dev" ]]; then
  cargo_profile_flag=""
  profile_dir="debug"
fi

cargo build --manifest-path "$ROOT/Cargo.toml" -p acoustilab-wasm \
  --target wasm32-unknown-unknown $cargo_profile_flag

TARGET_DIR="$(cargo metadata --manifest-path "$ROOT/Cargo.toml" --format-version 1 --no-deps |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).target_directory))')"
WASM="$TARGET_DIR/wasm32-unknown-unknown/$profile_dir/acoustilab_wasm.wasm"

rm -rf "$OUT"
wasm-bindgen --target web --out-dir "$OUT" "$WASM"

# Optional size optimisation when binaryen is installed.
if command -v wasm-opt >/dev/null 2>&1 && [[ "$PROFILE" != "dev" ]]; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int \
    --enable-sign-ext --enable-mutable-globals \
    -o "$OUT/acoustilab_wasm_bg.wasm" "$OUT/acoustilab_wasm_bg.wasm"
fi

echo "wasm-bindgen $WB_VERSION -> $OUT ($(wc -c <"$OUT/acoustilab_wasm_bg.wasm") bytes)"
