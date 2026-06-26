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


def target_profile(target):
    """Return (lib-suffixes, platform-tag) for a cargo target triple. When
    target is None, profile the host platform instead.

    The lib suffixes select the cdylib/staticlib/import-lib artifacts cargo
    emits for that platform; everything else in the target dir (.rlib, .d, ...)
    is ignored.
    """
    if target is None:
        if sys.platform == "win32":
            return [".dll", ".dll.lib", ".lib"], "windows-x86_64"
        if sys.platform == "darwin":
            return [".dylib", ".a"], "macos-x86_64"
        return [".so", ".a"], "linux-x86_64"
    # Cross-compilation: select by target triple, not the host.
    if "windows-gnu" in target:
        # mingw: cdylib -> micromegas_capi.dll, import lib ->
        # libmicromegas_capi.dll.a, staticlib -> libmicromegas_capi.a
        return [".dll", ".dll.a", ".a"], "windows-x86_64"
    if "windows" in target:
        # MSVC: cdylib -> .dll, import lib -> .dll.lib, staticlib -> .lib
        return [".dll", ".dll.lib", ".lib"], "windows-x86_64"
    if "darwin" in target:
        return [".dylib", ".a"], "macos-x86_64"
    return [".so", ".a"], "linux-x86_64"


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
        "--target",
        help="cargo target triple to (cross-)build and package for, e.g. "
        "x86_64-pc-windows-gnu (defaults to the host target)",
    )
    parser.add_argument(
        "--target-dir",
        help="cargo release output directory containing the built libraries "
        "(defaults to rust/target[/<target>]/release)",
    )
    parser.add_argument(
        "--out-dir",
        default=os.path.join(os.path.dirname(__file__), "..", "dist"),
        help="directory to write the staged tree and final archive into",
    )
    args = parser.parse_args()

    repo_root = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
    cargo_cwd = os.path.join(repo_root, "rust")
    if args.target_dir:
        base = args.target_dir
    else:
        # Honor CARGO_TARGET_DIR (the dev-worker points it at a persistent
        # cache outside rust/target); cargo resolves a relative value against
        # its working directory. Otherwise default to rust/target.
        env_dir = os.environ.get("CARGO_TARGET_DIR")
        if env_dir:
            base = (
                env_dir if os.path.isabs(env_dir) else os.path.join(cargo_cwd, env_dir)
            )
        else:
            base = os.path.join(cargo_cwd, "target")
    # cargo nests cross-compiled output under <base>/<triple>/release.
    parts = [base, args.target, "release"] if args.target else [base, "release"]
    target_dir = os.path.normpath(os.path.join(*parts))
    out_dir = os.path.normpath(args.out_dir)
    header = os.path.join(repo_root, "rust", "capi", "include", "micromegas.h")
    version = args.version or workspace_version(repo_root)

    if args.build:
        cmd = ["cargo", "build", "-p", "micromegas-capi", "--release"]
        if args.target:
            cmd += ["--target", args.target]
        subprocess.run(
            cmd,
            cwd=cargo_cwd,
            check=True,
        )

    suffixes, tag = target_profile(args.target)
    name = f"micromegas-capi-{version}-{tag}"
    stage = os.path.join(out_dir, name)
    if os.path.isdir(stage):
        shutil.rmtree(stage)
    os.makedirs(os.path.join(stage, "lib"), exist_ok=True)
    os.makedirs(os.path.join(stage, "include"), exist_ok=True)

    # Header is checked in (cbindgen-generated); ship it as-is.
    shutil.copy2(header, os.path.join(stage, "include", "micromegas.h"))

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

    fmt = "zip" if tag.startswith("windows") else "gztar"
    archive = shutil.make_archive(stage, fmt, root_dir=out_dir, base_dir=name)

    print(f"staged libraries: {', '.join(copied)}")
    print(f"archive: {archive}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
