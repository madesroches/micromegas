"""
Copy Unreal Engine modules from a local Perforce workspace into this git repo.

Environment variables:
  MICROMEGAS_UNREAL_ROOT_DIR             - root of the Unreal Engine source tree
  MICROMEGAS_UNREAL_TELEMETRY_MODULE_DIR - directory that contains MicromegasTelemetrySink
                                           (may be inside a plugin folder)

Run this script whenever the Perforce workspace has changes you want to bring
into the git repo, then review the diff and commit.
"""

import os
import pathlib
import shutil
import subprocess
import tempfile


def has_untracked_files(directory: pathlib.Path) -> bool:
    repo_root = pathlib.Path(__file__).parent.parent
    result = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard", str(directory)],
        capture_output=True,
        text=True,
        check=True,
        cwd=repo_root,
    )
    return bool(result.stdout.strip())


def has_dirty_files(directory: pathlib.Path) -> bool:
    repo_root = pathlib.Path(__file__).parent.parent
    result = subprocess.run(
        ["git", "status", "--porcelain", str(directory)],
        capture_output=True,
        text=True,
        check=True,
        cwd=repo_root,
    )
    return bool(result.stdout.strip())


def copy_tree(src: pathlib.Path, dst: pathlib.Path) -> None:
    if not src.exists():
        raise FileNotFoundError(f"source not found: {src}")
    if dst.exists():
        if has_untracked_files(dst):
            raise RuntimeError(
                f"untracked files found in {dst} — commit or remove them before copying"
            )
        if has_dirty_files(dst):
            raise RuntimeError(
                f"locally modified files found in {dst} — commit or stash them before copying"
            )
    tmp = dst.parent / (dst.name + ".tmp")
    if tmp.exists():
        shutil.rmtree(tmp)
    try:
        shutil.copytree(src, tmp, ignore=shutil.ignore_patterns("*.~*", "*~"))
    except Exception:
        shutil.rmtree(tmp, ignore_errors=True)
        raise
    if dst.exists():
        shutil.rmtree(dst)
    tmp.rename(dst)
    print(f"copied {src}\n     → {dst}")


unreal_root_dir = os.environ.get("MICROMEGAS_UNREAL_ROOT_DIR")
telemetry_module_dir = os.environ.get("MICROMEGAS_UNREAL_TELEMETRY_MODULE_DIR")

if not unreal_root_dir:
    raise ValueError("MICROMEGAS_UNREAL_ROOT_DIR is not set")
if not telemetry_module_dir:
    raise ValueError("MICROMEGAS_UNREAL_TELEMETRY_MODULE_DIR is not set")

core_dir = pathlib.Path(unreal_root_dir) / "Engine" / "Source" / "Runtime" / "Core"
repo_unreal = pathlib.Path(__file__).parent.parent / "unreal"

copy_tree(
    core_dir / "Public" / "MicromegasTracing",
    repo_unreal / "MicromegasTracing" / "Public" / "MicromegasTracing",
)

copy_tree(
    core_dir / "Private" / "MicromegasTracing",
    repo_unreal / "MicromegasTracing" / "Private",
)

copy_tree(
    pathlib.Path(telemetry_module_dir) / "MicromegasTelemetrySink",
    repo_unreal / "MicromegasTelemetrySink",
)

print("\nDone. Review `git diff unreal/` and commit.")
