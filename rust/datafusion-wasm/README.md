# micromegas-datafusion-wasm

DataFusion compiled to WebAssembly for in-browser SQL query execution, part of the [Micromegas](https://github.com/madesroches/micromegas/) observability platform.

## Prerequisites

- `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- `wasm-bindgen-cli` — version must match `Cargo.lock`: `cargo install wasm-bindgen-cli --version <version>`
- `wasm-opt` (from binaryen): install via your package manager (optional — skipped with a warning if absent)

## Build

`build.py` is the **only** supported generator of committed bindings. Run it from the crate root:

```bash
python3 build.py
```

This runs `cargo build --target wasm32-unknown-unknown --release`, then `wasm-bindgen --target web`,
then copies `.js`, `.d.ts`, `_bg.wasm`, `_bg.wasm.d.ts`, and a canonical `package.json` into
`analytics-web-app/src/lib/datafusion-wasm/`. The `.js`/`.d.ts`/`package.json` are committed; the
`.wasm` binary is not (too large).

> **Do not run `wasm-pack build` into the output directory.**
> `wasm-pack build` produces a structurally different `.js` glue and a richer `package.json`
> (missing `"private": true`, which reintroduces a yarn workspace warning) and leaves behind
> `README.md` and `.gitignore` as side artifacts. CI runs `build.py --check`, which compares
> committed bindings against a fresh `build.py` run — a `wasm-pack`-form commit fails that check.
> `wasm-pack` is only used by `build.py --test` (headless Firefox integration tests), which runs
> `wasm-pack test` and does not emit artifacts into the output directory.

## Manual Build (debugging aid)

The commands below produce the same `wasm-bindgen --target web` output as `build.py`. They do **not**
write `package.json` or clean up wasm-pack leftovers — use `build.py` for committed artifacts.

```bash
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/micromegas_datafusion_wasm.wasm --out-dir pkg --target web
wasm-opt pkg/micromegas_datafusion_wasm_bg.wasm -Os --enable-reference-types -o pkg/micromegas_datafusion_wasm_bg.wasm
```

Note: anything that ends up in `pkg/` is copied into the output directory by the next `build()` run.
`build.py` prunes known wasm-pack leftovers (`README.md`, `.gitignore`) from the output directory
after every copy, so `build()` is self-healing regardless of how `pkg/` was populated.

## Test

```bash
python3 build.py --test
```

Runs integration tests via `wasm-pack test --headless --firefox`. This does **not** regenerate
committed bindings.

## CI Check

```bash
python3 build.py --check
```

Rebuilds and compares committed bindings against a fresh run (ignoring compiler-generated hash
churn). This is what `build/rust_ci.py` runs in CI.

## License

Apache-2.0
