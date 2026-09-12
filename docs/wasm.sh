#!/bin/sh
# The compiler, for the page: the same `verify` that runs in the terminal,
# compiled to wasm and served next to the book.
#
# Without `http` and without `tui`: a browser has neither sockets nor a
# terminal, and those two are the only dependencies that do not exist there.
# What is left is the model and the rules, which touch nothing of the system.
set -eu

cd "$(dirname "$0")/.."
out="docs/src/playground"

cargo build --release --lib --target wasm32-unknown-unknown \
  --no-default-features --features wasm
wasm-bindgen --target web --no-typescript --out-dir "$out" \
  target/wasm32-unknown-unknown/release/axon.wasm

# `wasm-opt` if it is around; not a requirement, it only trims.
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz -o "$out/axon_bg.wasm" "$out/axon_bg.wasm"
fi

ls -l "$out" | awk '{print "  " $9 "  " $5}'
