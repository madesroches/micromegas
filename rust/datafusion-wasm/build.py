#!/usr/bin/env python3
"""Build datafusion-wasm and copy artifacts to analytics-web-app."""

import argparse
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
WASM_FILE = "datafusion_wasm.wasm"


def run(cmd: list[str], **kwargs) -> None:
    print(f"  → {' '.join(cmd)}")
    subprocess.run(cmd, check=True, **kwargs)


def check_tools() -> None:
    for tool in ["wasm-bindgen", "wasm-opt"]:
        if shutil.which(tool) is None:
            print(f"ERROR: {tool} not found. See README.md for install instructions.")
            sys.exit(1)


def test() -> None:
    """Run wasm-pack test in headless Chrome."""
    if shutil.which("wasm-pack") is None:
        print("ERROR: wasm-pack not found. Install with: cargo install wasm-pack")
        sys.exit(1)

    print("Running wasm-pack test --headless --firefox...")
    run(["wasm-pack", "test", "--headless", "--firefox"], cwd=CRATE_DIR)
    print("Tests passed!")


def build() -> None:
    """Build and package the WASM artifacts."""
    check_tools()

    print("Building datafusion-wasm...")
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

    print("Optimizing with wasm-opt...")
    wasm_bg = PKG_DIR / "datafusion_wasm_bg.wasm"
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
        '{\n  "name": "datafusion-wasm",\n  "version": "0.1.0",\n'
        '  "type": "module",\n  "main": "datafusion_wasm.js",\n'
        '  "types": "datafusion_wasm.d.ts"\n}\n'
    )
    print("  package.json")

    print("Done!")


def main() -> None:
    parser = argparse.ArgumentParser(description="Build or test datafusion-wasm")
    parser.add_argument(
        "--test", action="store_true", help="Run WASM integration tests in headless Chrome"
    )
    args = parser.parse_args()

    if args.test:
        test()
    else:
        build()


if __name__ == "__main__":
    main()
