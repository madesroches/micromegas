#!/bin/python3
from rust_command import run_command, show_disk_space
import pathlib

wasm_crate = pathlib.Path(__file__).parent.parent.absolute() / "rust" / "datafusion-wasm"

print("=" * 60)
print("Starting Rust CI Pipeline")
print("=" * 60)
show_disk_space()

print("\n" + "=" * 60)
print("Step 1/7: Formatting Check")
print("=" * 60)
run_command("cargo fmt --check")

print("\n" + "=" * 60)
print("Step 2/7: Clippy Linting")
print("=" * 60)
run_command("cargo clippy --workspace -- -D warnings")

print("\n" + "=" * 60)
print("Step 3/7: Unused Dependencies Check")
print("=" * 60)
run_command("cargo machete")

print("\n" + "=" * 60)
print("Step 4/7: Running Tests")
print("=" * 60)
run_command("cargo test")

print("\n" + "=" * 60)
print("Step 5/7: WASM Formatting Check")
print("=" * 60)
run_command("cargo fmt --check", cwd=wasm_crate)

print("\n" + "=" * 60)
print("Step 6/7: WASM Clippy")
print("=" * 60)
run_command("cargo clippy --target wasm32-unknown-unknown -- -D warnings", cwd=wasm_crate)

print("\n" + "=" * 60)
print("Step 7/7: WASM Tests")
print("=" * 60)
run_command("python3 build.py --test", cwd=wasm_crate)

print("\n" + "=" * 60)
print("All CI steps completed successfully!")
print("=" * 60)
show_disk_space()
