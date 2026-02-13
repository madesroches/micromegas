#!/bin/python3
import sys
import pathlib
from rust_command import run_command, show_disk_space

wasm_crate = pathlib.Path(__file__).parent.parent.absolute() / "rust" / "datafusion-wasm"


def run_native():
    steps = [
        ("Formatting Check", "cargo fmt --check", None),
        ("Clippy Linting", "cargo clippy --workspace -- -D warnings", None),
        ("Unused Dependencies Check", "cargo machete", None),
        ("Running Tests", "cargo test", None),
    ]
    _run_steps("Native", steps)


def run_wasm():
    steps = [
        ("WASM Formatting Check", "cargo fmt --check", wasm_crate),
        ("WASM Clippy", "cargo clippy --target wasm32-unknown-unknown -- -D warnings", wasm_crate),
        ("WASM Tests", "python3 build.py --test", wasm_crate),
    ]
    _run_steps("WASM", steps)


def _run_steps(label, steps):
    total = len(steps)
    print("=" * 60)
    print(f"Starting {label} CI Pipeline")
    print("=" * 60)
    show_disk_space()
    for i, (name, cmd, cwd) in enumerate(steps, 1):
        print(f"\n{'=' * 60}")
        print(f"Step {i}/{total}: {name}")
        print("=" * 60)
        kwargs = {"cwd": cwd} if cwd else {}
        run_command(cmd, **kwargs)
    print(f"\n{'=' * 60}")
    print(f"{label} CI steps completed successfully!")
    print("=" * 60)
    show_disk_space()


if __name__ == "__main__":
    targets = sys.argv[1:] if len(sys.argv) > 1 else ["native", "wasm"]
    for target in targets:
        if target == "native":
            run_native()
        elif target == "wasm":
            run_wasm()
        else:
            print(f"Unknown target: {target}")
            print("Usage: rust_ci.py [native] [wasm]")
            sys.exit(1)
