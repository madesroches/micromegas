#!/bin/python3
from rust_command import run_command, show_disk_space

print("=" * 60)
print("Starting Rust CI Pipeline")
print("=" * 60)
show_disk_space()

print("\n" + "=" * 60)
print("Step 1/4: Formatting Check")
print("=" * 60)
run_command("cargo fmt --check")

print("\n" + "=" * 60)
print("Step 2/4: Clippy Linting")
print("=" * 60)
run_command("cargo clippy --workspace -- -D warnings")

print("\n" + "=" * 60)
print("Step 3/4: Unused Dependencies Check")
print("=" * 60)
run_command("cargo machete")

print("\n" + "=" * 60)
print("Step 4/4: Running Tests")
print("=" * 60)
run_command("cargo test")

print("\n" + "=" * 60)
print("✅ All CI steps completed successfully!")
print("=" * 60)
show_disk_space()
