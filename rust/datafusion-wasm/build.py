#!/usr/bin/env python3
"""Build micromegas-datafusion-wasm and copy artifacts to analytics-web-app."""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

CRATE_DIR = Path(__file__).parent.resolve()
_TARGET_BASE = Path(os.environ["CARGO_TARGET_DIR"]) if "CARGO_TARGET_DIR" in os.environ else CRATE_DIR / "target"
TARGET_DIR = _TARGET_BASE / "wasm32-unknown-unknown" / "release"
PKG_DIR = CRATE_DIR / "pkg"
OUTPUT_DIR = (
    CRATE_DIR.parent.parent / "analytics-web-app" / "src" / "lib" / "datafusion-wasm"
)
WASM_FILE = "micromegas_datafusion_wasm.wasm"
WASM_PACKAGE_JSON = {
    "name": "micromegas-datafusion-wasm",
    "version": "0.1.0",
    "private": True,
    "type": "module",
    "main": "micromegas_datafusion_wasm.js",
    "types": "micromegas_datafusion_wasm.d.ts",
}


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


def build(skip_opt: bool = False) -> None:
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

    if skip_opt:
        print("Skipping wasm-opt (--debug)")
    elif shutil.which("wasm-opt"):
        print("Optimizing with wasm-opt...")
        wasm_bg = PKG_DIR / "micromegas_datafusion_wasm_bg.wasm"
        run(["wasm-opt", str(wasm_bg), "-Os", "--enable-reference-types", "-o", str(wasm_bg)])

    print(f"Copying artifacts to {OUTPUT_DIR}...")
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    for f in PKG_DIR.iterdir():
        dest = OUTPUT_DIR / f.name
        shutil.copy2(f, dest)
        print(f"  {f.name}")

    # Write a package.json so this can be used as a local dependency
    package_json = OUTPUT_DIR / "package.json"
    package_json.write_text(json.dumps(WASM_PACKAGE_JSON, indent=2) + "\n")
    print("  package.json")

    print("Done!")


TRACKED_BINDINGS = [
    OUTPUT_DIR / "micromegas_datafusion_wasm.js",
    OUTPUT_DIR / "micromegas_datafusion_wasm.d.ts",
    OUTPUT_DIR / "package.json",
]


def _normalize_symbol_hashes(text: str) -> str:
    """Replace compiler-generated hashes with a placeholder.

    Both Rust mangled symbols (``__invoke__h04fdd830bb54d5e4``) and
    wasm-bindgen glue names (``__wbg_call_389efe28435a9388``) contain
    hex hashes that change with compiler version even when the source is
    identical.  Normalizing them lets us detect *real* binding changes
    while ignoring hash churn.
    """
    # Rust symbol hashes: __h followed by 16 hex digits
    text = re.sub(r"__h[0-9a-f]{16}\b", "__hXXXX", text)
    # wasm-bindgen glue: trailing _<16 hex digits> on __wbg_ prefixed names
    text = re.sub(r"(__wbg_\w+?)_[0-9a-f]{16}\b", r"\1_XXXX", text)
    return text


def check() -> None:
    """Build WASM and verify tracked bindings are up to date."""
    build(skip_opt=True)

    repo_root = CRATE_DIR.parent.parent
    has_diff = False
    for path in TRACKED_BINDINGS:
        # Compare the committed version against the working-tree version
        committed = subprocess.run(
            ["git", "show", f"HEAD:{path.relative_to(repo_root)}"],
            capture_output=True,
            text=True,
            cwd=repo_root,
        )
        if committed.returncode != 0:
            # File is new / untracked — that counts as a real diff
            print(f"  {path.name}: new file (not yet committed)")
            has_diff = True
            continue

        current = path.read_text()
        if _normalize_symbol_hashes(committed.stdout) != _normalize_symbol_hashes(current):
            print(f"  {path.name}: binding change detected (beyond closure hashes)")
            has_diff = True

    if has_diff:
        print("\nERROR: Tracked WASM bindings are out of date.")
        print("Run: python3 rust/datafusion-wasm/build.py")
        sys.exit(1)
    print("Tracked WASM bindings are up to date.")


def main() -> None:
    parser = argparse.ArgumentParser(description="Build or test micromegas-datafusion-wasm")
    parser.add_argument(
        "--test", action="store_true", help="Run WASM integration tests in headless Firefox"
    )
    parser.add_argument(
        "--check", action="store_true", help="Build and verify tracked bindings are up to date"
    )
    parser.add_argument(
        "--debug", action="store_true", help="Skip wasm-opt optimization (faster builds)"
    )
    args = parser.parse_args()

    if args.test:
        test()
    elif args.check:
        check()
    else:
        build(skip_opt=args.debug)


if __name__ == "__main__":
    main()
