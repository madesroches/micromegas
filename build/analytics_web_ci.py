#!/usr/bin/env python3
"""
Analytics Web App CI validation script.
Runs all checks locally before pushing to CI.
"""
import subprocess
import sys
import os
from pathlib import Path

def setup_nvm_and_node(repo_root: Path) -> bool:
    """Setup NVM and switch to the correct Node version."""
    # Look for .nvmrc in analytics-web-app directory
    nvmrc_path = repo_root / "analytics-web-app" / ".nvmrc"

    if nvmrc_path.exists():
        with open(nvmrc_path, 'r') as f:
            required_version = f.read().strip()
        print(f"Found .nvmrc at {nvmrc_path} with Node version: {required_version}")
    else:
        required_version = "20"  # Default to Node 20 LTS
        print(f"No .nvmrc found, using Node {required_version}")

    # Find NVM installation
    nvm_dir = os.environ.get('NVM_DIR', os.path.expanduser('~/.nvm'))
    nvm_sh = Path(nvm_dir) / "nvm.sh"

    if not nvm_sh.exists():
        print(f"⚠️  NVM not found at {nvm_sh}, skipping Node version switch")
        return True

    print(f"\n=== Setting up Node.js version {required_version} with NVM ===")

    # Create a script that sources nvm and switches version
    switch_cmd = f"""
    source {nvm_sh}
    nvm install {required_version}
    nvm use {required_version}
    node --version
    """

    result = subprocess.run(
        ["bash", "-c", switch_cmd],
        cwd=repo_root,
        capture_output=True,
        text=True
    )

    if result.returncode != 0:
        print(f"❌ Failed to switch Node version: {result.stderr}")
        return False

    print(result.stdout)
    return True

def run_cmd(cmd: str, cwd: Path) -> int:
    """Run command with NVM environment."""
    print(f"Running: {cmd} in {cwd}")

    # Find NVM installation
    nvm_dir = os.environ.get('NVM_DIR', os.path.expanduser('~/.nvm'))
    nvm_sh = Path(nvm_dir) / "nvm.sh"

    if nvm_sh.exists():
        # Run command with nvm sourced
        full_cmd = f"source {nvm_sh} && nvm use && {cmd}"
        result = subprocess.run(
            ["bash", "-c", full_cmd],
            cwd=cwd,
            check=False
        )
    else:
        # Fall back to direct execution if nvm not found
        result = subprocess.run(cmd, shell=True, cwd=cwd, check=False)

    return result.returncode

def main():
    repo_root = Path(__file__).parent.parent
    web_app_dir = repo_root / "analytics-web-app"

    print("=== Analytics Web App CI Validation ===\n")

    # Setup Node version first
    if not setup_nvm_and_node(repo_root):
        return 1

    # Install dependencies
    print("\n=== Installing dependencies ===")
    if run_cmd("yarn install", web_app_dir) != 0:
        print("❌ Dependency installation failed")
        return 1

    # Type checking
    print("\n=== Type checking ===")
    if run_cmd("yarn type-check", web_app_dir) != 0:
        print("❌ Type checking failed")
        return 1

    # Linting
    print("\n=== Linting ===")
    if run_cmd("yarn lint", web_app_dir) != 0:
        print("❌ Linting failed")
        return 1

    # Unit tests
    print("\n=== Unit tests ===")
    if run_cmd("yarn test", web_app_dir) != 0:
        print("❌ Unit tests failed")
        return 1

    # Build
    print("\n=== Production build ===")
    if run_cmd("yarn build", web_app_dir) != 0:
        print("❌ Build failed")
        return 1

    print("\n✅ All checks passed!")
    return 0

if __name__ == "__main__":
    sys.exit(main())
