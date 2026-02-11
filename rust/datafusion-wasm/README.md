# datafusion-wasm

DataFusion compiled to WebAssembly for in-browser SQL query execution.

## Prerequisites

- `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- `wasm-bindgen-cli`: `cargo install wasm-bindgen-cli`
- `wasm-opt` (from binaryen): install via your package manager

## Build

```bash
python3 build.py
```

This builds the WASM binary and copies the output to `../../analytics-web-app/src/lib/datafusion-wasm/`.

## Manual Build

```bash
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/datafusion_wasm.wasm --out-dir pkg --target web
wasm-opt pkg/datafusion_wasm_bg.wasm -Os -o pkg/datafusion_wasm_bg.wasm
```
