#!/bin/python3
import sys
import pathlib
from concurrent.futures import ThreadPoolExecutor
from rust_command import run_command, run_captured, show_disk_space

repo_root = pathlib.Path(__file__).parent.parent.absolute()
wasm_crate = repo_root / "rust" / "datafusion-wasm"


def run_native():
    sequential_steps = [
        ("Formatting Check", "cargo fmt --check", None),
        ("Clippy Linting", "cargo clippy --workspace -- -D warnings", None),
        ("Running Tests", "cargo nextest run", None),
        ("Doc Tests", "cargo test --doc", None),
    ]
    parallel_steps = [
        ("Unused Dependencies Check", "cargo machete", None),
        ("Advisory Audit", "cargo audit", None),
        ("License & Supply-Chain (deny)", "cargo deny check licenses bans sources", None),
        ("Advisory Audit (datafusion-wasm)", "cargo audit", wasm_crate),
        (
            # The wasm tree reuses the main deny.toml, whose `bans.skip` list is
            # the union of both trees' duplicates. Skips that only apply to the
            # main tree show up here as `unnecessary-skip`; allow that one lint so
            # the shared config stays a single source of truth without noise.
            "License & Supply-Chain (deny, datafusion-wasm)",
            "cargo deny --config ../deny.toml check licenses bans sources --allow unnecessary-skip",
            wasm_crate,
        ),
    ]
    _run_steps("Native", sequential_steps, parallel_steps)


def run_wasm():
    steps = [
        ("WASM Dependency Version Check", "python3 build/check_wasm_deps.py", repo_root),
        ("WASM Formatting Check", "cargo fmt --check", wasm_crate),
        ("WASM Clippy", "cargo clippy --target wasm32-unknown-unknown -- -D warnings", wasm_crate),
        ("WASM Tests", "python3 build.py --test", wasm_crate),
        ("WASM Bindings Freshness Check", "python3 build.py --check", wasm_crate),
    ]
    _run_steps("WASM", steps)


def _toolchain_warmup_cwds(steps):
    """Distinct working directories of the cargo steps, in first-seen order."""
    return list(dict.fromkeys(cwd for _, cmd, cwd in steps if cmd.startswith("cargo")))


def _warm_toolchain(steps):
    """Install the pinned toolchain before any concurrent cargo runs.

    rustup auto-installs a missing pinned toolchain on the first cargo
    invocation. Several of those at once race on ~/.rustup/downloads and all but
    one fail with "could not rename 'downloaded' file", so resolve it serially.
    """
    cwds = _toolchain_warmup_cwds(steps)
    if not cwds:
        return
    print(f"\n{'=' * 60}")
    print("Toolchain warm-up")
    print("=" * 60)
    for cwd in cwds:
        kwargs = {"cwd": cwd} if cwd else {}
        run_command("cargo --version", **kwargs)


def _run_parallel_step(name, cmd, cwd):
    kwargs = {"cwd": cwd} if cwd else {}
    result = run_captured(cmd, **kwargs)
    status = "PASSED" if result.returncode == 0 else "FAILED"
    print(f"\n{'=' * 60}\n[parallel] {status}: {name}\n{'=' * 60}\n{result.stdout}{result.stderr}")
    return name, result.returncode == 0


def _run_steps(label, sequential_steps, parallel_steps=None):
    parallel_steps = parallel_steps or []
    print("=" * 60)
    print(f"Starting {label} CI Pipeline")
    print("=" * 60)
    show_disk_space()
    _warm_toolchain(sequential_steps + parallel_steps)

    with ThreadPoolExecutor(max_workers=max(len(parallel_steps), 1)) as pool:
        futures = [pool.submit(_run_parallel_step, *step) for step in parallel_steps]

        for i, (name, cmd, cwd) in enumerate(sequential_steps, 1):
            print(f"\n{'=' * 60}")
            print(f"Step {i}/{len(sequential_steps)}: {name}")
            print("=" * 60)
            kwargs = {"cwd": cwd} if cwd else {}
            run_command(cmd, **kwargs)

        results = [f.result() for f in futures]

    failed = [name for name, ok in results if not ok]
    if failed:
        print(f"\nParallel steps failed: {', '.join(failed)}")
        sys.exit(1)

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
