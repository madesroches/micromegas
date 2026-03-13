#!/usr/bin/env python3
"""Check that shared dependencies between the workspace and datafusion-wasm have matching versions."""

import sys
import pathlib

try:
    import tomllib
except ImportError:
    import tomli as tomllib


def load_toml(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def is_path_dep(dep):
    return isinstance(dep, dict) and "path" in dep


def extract_version(dep):
    if isinstance(dep, str):
        return dep
    if isinstance(dep, dict):
        return dep.get("version")
    return None


def main():
    root = pathlib.Path(__file__).parent.parent.absolute()
    workspace_toml = load_toml(root / "rust" / "Cargo.toml")
    wasm_toml = load_toml(root / "rust" / "datafusion-wasm" / "Cargo.toml")

    workspace_deps = workspace_toml.get("workspace", {}).get("dependencies", {})
    wasm_deps = {
        **wasm_toml.get("dependencies", {}),
        **wasm_toml.get("dev-dependencies", {}),
    }

    # Only check deps that appear in both (skip path-only workspace deps)
    mismatches = []
    for name, wasm_dep in wasm_deps.items():
        if name not in workspace_deps:
            continue
        ws_version = extract_version(workspace_deps[name])
        wasm_version = extract_version(wasm_dep)
        if ws_version is None or wasm_version is None:
            continue
        if ws_version != wasm_version:
            mismatches.append((name, ws_version, wasm_version))

    if mismatches:
        print("Dependency version mismatches between workspace and datafusion-wasm:")
        for name, ws_ver, wasm_ver in sorted(mismatches):
            print(f"  {name}: workspace={ws_ver}  wasm={wasm_ver}")
        sys.exit(1)
    else:
        print("All shared dependency versions match.")


if __name__ == "__main__":
    main()
