#!/bin/python3
"""Build micromegas-capi and package the Blender extension.

Compiles release binaries for Linux and/or Windows, copies them into
blender/micromegas_blender/lib/, and produces a ready-to-install
blender/micromegas_blender.zip.

Usage:
    python3 build/build_blender_plugin.py              # both platforms
    python3 build/build_blender_plugin.py --platform linux
    python3 build/build_blender_plugin.py --platform windows
    python3 build/build_blender_plugin.py --zip-only   # zip existing lib/ only

Platform notes:
  linux   — native cargo build (x86_64-unknown-linux-gnu).
  windows — native cargo build on Windows; cross-compiled on Linux using the
            MinGW-w64 toolchain (no Docker required):
              rustup target add x86_64-pc-windows-gnu
              sudo apt install gcc-mingw-w64-x86-64
            For an MSVC DLL use the CI workflow (capi-release.yml) instead.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import pathlib
import zipfile

REPO_ROOT = pathlib.Path(__file__).parent.parent.absolute()
RUST_DIR = REPO_ROOT / "rust"
LIB_OUT = REPO_ROOT / "blender" / "micromegas_blender" / "lib"
MANIFEST = REPO_ROOT / "blender" / "micromegas_blender" / "blender_manifest.toml"

LINUX_TARGET = "x86_64-unknown-linux-gnu"
WINDOWS_TARGET_LINUX = "x86_64-pc-windows-gnu"


def run(cmd: list[str], cwd: pathlib.Path, extra_env: dict | None = None) -> None:
    print(f"+ {' '.join(str(c) for c in cmd)}  (in {cwd})")
    env = None
    if extra_env:
        env = os.environ.copy()
        env.update(extra_env)
    subprocess.run(cmd, cwd=cwd, check=True, env=env)


def cargo_output_dir(target: str | None) -> pathlib.Path:
    base = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", RUST_DIR / "target"))
    if target:
        return base / target / "release"
    return base / "release"


def build_linux() -> None:
    print("\n=== Building Linux (x86_64-unknown-linux-gnu) ===")
    run(
        [
            "cargo",
            "build",
            "-p",
            "micromegas-capi",
            "--release",
            "--target",
            LINUX_TARGET,
        ],
        RUST_DIR,
    )
    src = cargo_output_dir(LINUX_TARGET) / "libmicromegas_capi.so"
    _copy(src, LIB_OUT / "libmicromegas_capi.so")


def build_windows_native() -> None:
    print("\n=== Building Windows (native) ===")
    run(
        ["cargo", "build", "-p", "micromegas-capi", "--release"],
        RUST_DIR,
    )
    src = cargo_output_dir(None) / "micromegas_capi.dll"
    _copy(src, LIB_OUT / "micromegas_capi.dll")


def build_windows_mingw() -> None:
    linker = "x86_64-w64-mingw32-gcc"
    if not shutil.which(linker):
        print(
            f"error: MinGW-w64 linker '{linker}' not found.\n"
            "Install the prerequisites and retry:\n"
            "  rustup target add x86_64-pc-windows-gnu\n"
            "  sudo apt install gcc-mingw-w64-x86-64",
            file=sys.stderr,
        )
        sys.exit(1)
    print(f"\n=== Building Windows via MinGW-w64 ({WINDOWS_TARGET_LINUX}) ===")
    # ws2_32 must be linked explicitly for x86_64-pc-windows-gnu; it is not
    # pulled in automatically the way MSVC handles Windows socket libraries.
    run(
        [
            "cargo",
            "build",
            "-p",
            "micromegas-capi",
            "--release",
            "--target",
            WINDOWS_TARGET_LINUX,
        ],
        RUST_DIR,
        extra_env={"RUSTFLAGS": "-C link-args=-lws2_32"},
    )
    src = cargo_output_dir(WINDOWS_TARGET_LINUX) / "micromegas_capi.dll"
    _copy(src, LIB_OUT / "micromegas_capi.dll")


def workspace_version() -> str:
    """Read the workspace package version from rust/Cargo.toml."""
    cargo_toml = RUST_DIR / "Cargo.toml"
    in_workspace_package = False
    for line in cargo_toml.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_workspace_package = stripped == "[workspace.package]"
        elif in_workspace_package:
            m = re.match(r'version\s*=\s*"([^"]+)"', stripped)
            if m:
                return m.group(1)
    raise RuntimeError(f"could not find [workspace.package] version in {cargo_toml}")


def sync_manifest_version() -> None:
    """Stamp blender_manifest.toml's version to the workspace version so the
    add-on ships the same version as the rest of the workspace."""
    version = workspace_version()
    text = MANIFEST.read_text(encoding="utf-8")
    # Only the top-level `version` key — not `schema_version`.
    new_text, n = re.subn(r'(?m)^version\s*=\s*"[^"]*"', f'version = "{version}"', text)
    if n != 1:
        raise RuntimeError(
            f"expected exactly one top-level version line in {MANIFEST}, found {n}"
        )
    if new_text != text:
        MANIFEST.write_text(new_text, encoding="utf-8")
    print(f"manifest version set to {version}")


def build_zip() -> None:
    sync_manifest_version()
    # Warn if a platform library is absent — a single-platform local build
    # (or a partial lib/) produces a zip that won't load on the missing OS.
    missing = [
        name
        for name in ("libmicromegas_capi.so", "micromegas_capi.dll")
        if not (LIB_OUT / name).exists()
    ]
    if missing:
        print(
            f"warning: lib/ is missing {', '.join(missing)}; the zip will not "
            "support the corresponding platform(s)",
            file=sys.stderr,
        )
    # Blender Extensions expect files at the root of the zip (no wrapping
    # directory). Blender creates extensions/user_default/<id>/ itself from
    # the manifest id on install, so arcnames must be relative to addon_dir.
    addon_dir = REPO_ROOT / "blender" / "micromegas_blender"
    zip_path = REPO_ROOT / "blender" / "micromegas_blender.zip"
    _SKIP_NAMES = {"__pycache__", "tests"}
    _SKIP_SUFFIXES = {".pyc"}

    print(f"\n=== Creating {zip_path.name} ===")
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for path in sorted(addon_dir.rglob("*")):
            if path.is_dir():
                continue
            if any(part in _SKIP_NAMES for part in path.parts):
                continue
            if path.suffix in _SKIP_SUFFIXES:
                continue
            arcname = path.relative_to(addon_dir)
            zf.write(path, arcname)
            print(f"  + {arcname}")
    print(f"zip: {zip_path}")


def _copy(src: pathlib.Path, dst: pathlib.Path) -> None:
    if not src.exists():
        print(f"error: expected output not found: {src}", file=sys.stderr)
        sys.exit(1)
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)
    print(f"copied {src.name} → {dst}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--platform",
        choices=["linux", "windows", "both"],
        default="both",
        help="which platform to build (default: both)",
    )
    parser.add_argument(
        "--zip-only",
        action="store_true",
        help="skip the cargo builds and only zip the existing lib/ contents "
        "(used by CI to package libraries built on separate runners)",
    )
    args = parser.parse_args()

    if args.zip_only:
        build_zip()
        print("\nDone.")
        return

    on_windows = sys.platform == "win32"
    build = args.platform

    if build in ("linux", "both"):
        if on_windows:
            print(
                "warning: skipping Linux build — not supported on Windows",
                file=sys.stderr,
            )
        else:
            build_linux()

    if build in ("windows", "both"):
        if on_windows:
            build_windows_native()
        else:
            build_windows_mingw()

    build_zip()
    print("\nDone.")


if __name__ == "__main__":
    main()
