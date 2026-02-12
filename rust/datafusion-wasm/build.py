#!/usr/bin/env python3
"""Build micromegas-datafusion-wasm and copy artifacts to analytics-web-app."""

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

CRATE_DIR = Path(__file__).parent.resolve()
TARGET_DIR = CRATE_DIR / "target" / "wasm32-unknown-unknown" / "release"
PKG_DIR = CRATE_DIR / "pkg"
OUTPUT_DIR = (
    CRATE_DIR.parent.parent / "analytics-web-app" / "src" / "lib" / "datafusion-wasm"
)
WASM_FILE = "micromegas_datafusion_wasm.wasm"


def run(cmd: list[str], **kwargs) -> None:
    print(f"  → {' '.join(cmd)}")
    subprocess.run(cmd, check=True, **kwargs)


def get_lockfile_wasm_bindgen_version() -> str:
    """Parse the resolved wasm-bindgen version from Cargo.lock."""
    lock_path = CRATE_DIR / "Cargo.lock"
    text = lock_path.read_text()
    match = re.search(
        r'\[\[package\]\]\s*name\s*=\s*"wasm-bindgen"\s*version\s*=\s*"([^"]+)"',
        text,
    )
    if not match:
        print("ERROR: Could not find wasm-bindgen version in Cargo.lock")
        sys.exit(1)
    return match.group(1)


def check_tools() -> None:
    if shutil.which("wasm-bindgen") is None:
        print("ERROR: wasm-bindgen not found. See README.md for install instructions.")
        sys.exit(1)

    if shutil.which("wasm-opt") is None:
        print("WARNING: wasm-opt not found, skipping WASM optimization.")
        print("  Rust release profile (lto + opt-level=s) already optimizes the binary.")
        print("  Install binaryen for additional size reduction.")

    expected = get_lockfile_wasm_bindgen_version()
    result = subprocess.run(
        ["wasm-bindgen", "--version"], capture_output=True, text=True
    )
    installed = result.stdout.strip().removeprefix("wasm-bindgen ").strip()
    if installed != expected:
        print(f"ERROR: wasm-bindgen version mismatch")
        print(f"  Cargo.lock requires: {expected}")
        print(f"  Installed CLI:       {installed}")
        print(f"  Fix: cargo install wasm-bindgen-cli --version {expected}")
        sys.exit(1)


def test() -> None:
    """Run wasm-pack test in headless Firefox."""
    if shutil.which("wasm-pack") is None:
        print("ERROR: wasm-pack not found. Install with: cargo install wasm-pack")
        sys.exit(1)

    print("Running wasm-pack test --headless --firefox...")
    run(["wasm-pack", "test", "--headless", "--firefox"], cwd=CRATE_DIR)
    print("Tests passed!")


def build() -> None:
    """Build and package the WASM artifacts."""
    check_tools()

    print("Building micromegas-datafusion-wasm...")
    run(
        ["cargo", "build", "--target", "wasm32-unknown-unknown", "--release"],
        cwd=CRATE_DIR,
    )

    print("Running wasm-bindgen...")
    PKG_DIR.mkdir(exist_ok=True)
    run(
        [
            "wasm-bindgen",
            str(TARGET_DIR / WASM_FILE),
            "--out-dir",
            str(PKG_DIR),
            "--target",
            "web",
        ]
    )

    if shutil.which("wasm-opt"):
        print("Optimizing with wasm-opt...")
        wasm_bg = PKG_DIR / "micromegas_datafusion_wasm_bg.wasm"
        run(["wasm-opt", str(wasm_bg), "-Os", "-o", str(wasm_bg)])

    print(f"Copying artifacts to {OUTPUT_DIR}...")
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    for f in PKG_DIR.iterdir():
        dest = OUTPUT_DIR / f.name
        shutil.copy2(f, dest)
        print(f"  {f.name}")

    # Write a package.json so this can be used as a local dependency
    package_json = OUTPUT_DIR / "package.json"
    package_json.write_text(
        '{\n  "name": "micromegas-datafusion-wasm",\n  "version": "0.1.0",\n'
        '  "type": "module",\n  "main": "micromegas_datafusion_wasm.js",\n'
        '  "types": "micromegas_datafusion_wasm.d.ts"\n}\n'
    )
    print("  package.json")

    print("Done!")


def main() -> None:
    parser = argparse.ArgumentParser(description="Build or test micromegas-datafusion-wasm")
    parser.add_argument(
        "--test", action="store_true", help="Run WASM integration tests in headless Firefox"
    )
    args = parser.parse_args()

    if args.test:
        test()
    else:
        build()


if __name__ == "__main__":
    main()
