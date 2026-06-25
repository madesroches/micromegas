#!/bin/python3
"""Package the built micromegas-capi library for distribution.

Collects the platform's shared/static libraries plus the C header into a
staging directory and produces a single archive (`.tar.gz` on Unix, `.zip` on
Windows) under the output directory. Intended for the capi-release workflow,
but also for on-demand local packaging.

On demand (build + package in one step):
    python3 build/package_capi.py --build

Or package an already-built release:
    cargo build -p micromegas-capi --release   # from rust/
    python3 build/package_capi.py
"""
import argparse
import os
import re
import shutil
import subprocess
import sys


def lib_patterns():
    """Return (matched-by-suffix, exact-names) the cdylib/staticlib/import-lib
    artifacts cargo emits for the current platform."""
    if sys.platform == "win32":
        # cdylib -> micromegas_capi.dll, its import lib -> micromegas_capi.dll.lib
        # staticlib -> micromegas_capi.lib
        return [".dll", ".dll.lib", ".lib"]
    if sys.platform == "darwin":
        return [".dylib", ".a"]
    return [".so", ".a"]


def platform_tag():
    if sys.platform == "win32":
        return "windows-x86_64"
    if sys.platform == "darwin":
        return "macos-x86_64"
    return "linux-x86_64"


def workspace_version(repo_root):
    """Read the workspace package version from rust/Cargo.toml."""
    cargo_toml = os.path.join(repo_root, "rust", "Cargo.toml")
    with open(cargo_toml, encoding="utf-8") as f:
        in_workspace_package = False
        for line in f:
            stripped = line.strip()
            if stripped.startswith("["):
                in_workspace_package = stripped == "[workspace.package]"
            elif in_workspace_package:
                m = re.match(r'version\s*=\s*"([^"]+)"', stripped)
                if m:
                    return m.group(1)
    raise RuntimeError(f"could not find [workspace.package] version in {cargo_toml}")


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--version",
        help="release version (defaults to the rust/ workspace.package version)",
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="run `cargo build -p micromegas-capi --release` before packaging",
    )
    parser.add_argument(
        "--target-dir",
        default=os.path.join(
            os.path.dirname(__file__), "..", "rust", "target", "release"
        ),
        help="cargo release output directory containing the built libraries",
    )
    parser.add_argument(
        "--out-dir",
        default=os.path.join(os.path.dirname(__file__), "..", "dist"),
        help="directory to write the staged tree and final archive into",
    )
    args = parser.parse_args()

    repo_root = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
    target_dir = os.path.normpath(args.target_dir)
    out_dir = os.path.normpath(args.out_dir)
    header = os.path.join(repo_root, "rust", "capi", "include", "micromegas.h")
    version = args.version or workspace_version(repo_root)

    if args.build:
        subprocess.run(
            ["cargo", "build", "-p", "micromegas-capi", "--release"],
            cwd=os.path.join(repo_root, "rust"),
            check=True,
        )

    name = f"micromegas-capi-{version}-{platform_tag()}"
    stage = os.path.join(out_dir, name)
    if os.path.isdir(stage):
        shutil.rmtree(stage)
    os.makedirs(os.path.join(stage, "lib"), exist_ok=True)
    os.makedirs(os.path.join(stage, "include"), exist_ok=True)

    # Header is checked in (cbindgen-generated); ship it as-is.
    shutil.copy2(header, os.path.join(stage, "include", "micromegas.h"))

    suffixes = lib_patterns()
    copied = []
    for entry in sorted(os.listdir(target_dir)):
        if not entry.startswith(("libmicromegas_capi", "micromegas_capi")):
            continue
        if any(entry.endswith(s) for s in suffixes):
            shutil.copy2(
                os.path.join(target_dir, entry), os.path.join(stage, "lib", entry)
            )
            copied.append(entry)

    if not copied:
        print(
            f"error: no micromegas_capi libraries found in {target_dir}",
            file=sys.stderr,
        )
        return 1

    fmt = "zip" if sys.platform == "win32" else "gztar"
    archive = shutil.make_archive(stage, fmt, root_dir=out_dir, base_dir=name)

    print(f"staged libraries: {', '.join(copied)}")
    print(f"archive: {archive}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
