#!/bin/python3
"""Build micromegas-capi for Blender add-on distribution.

Compiles release binaries for Linux and/or Windows and copies them into
blender/micromegas_blender/lib/ so the add-on can load them immediately.

Usage:
    python3 build/build_blender_plugin.py              # both platforms
    python3 build/build_blender_plugin.py --platform linux
    python3 build/build_blender_plugin.py --platform windows

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
import shutil
import subprocess
import sys
import pathlib

REPO_ROOT = pathlib.Path(__file__).parent.parent.absolute()
RUST_DIR = REPO_ROOT / "rust"
LIB_OUT = REPO_ROOT / "blender" / "micromegas_blender" / "lib"

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
    if target:
        return RUST_DIR / "target" / target / "release"
    return RUST_DIR / "target" / "release"


def build_linux() -> None:
    print("\n=== Building Linux (x86_64-unknown-linux-gnu) ===")
    run(
        ["cargo", "build", "-p", "micromegas-capi", "--release",
         "--target", LINUX_TARGET],
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
        ["cargo", "build", "-p", "micromegas-capi", "--release",
         "--target", WINDOWS_TARGET_LINUX],
        RUST_DIR,
        extra_env={"RUSTFLAGS": "-C link-args=-lws2_32"},
    )
    src = cargo_output_dir(WINDOWS_TARGET_LINUX) / "micromegas_capi.dll"
    _copy(src, LIB_OUT / "micromegas_capi.dll")


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
    args = parser.parse_args()

    on_windows = sys.platform == "win32"
    build = args.platform

    if build in ("linux", "both"):
        if on_windows:
            print("warning: skipping Linux build — not supported on Windows", file=sys.stderr)
        else:
            build_linux()

    if build in ("windows", "both"):
        if on_windows:
            build_windows_native()
        else:
            build_windows_mingw()

    print("\nDone. Libraries are in:", LIB_OUT)


if __name__ == "__main__":
    main()
