#!/usr/bin/env python3
"""
Grafana Plugin CI validation script.
Runs all checks locally before pushing to CI.
"""
import subprocess
import sys
import os
from pathlib import Path

def setup_nvm_and_node(repo_root: Path) -> bool:
    """Setup NVM and switch to the correct Node version."""
    # Look for .nvmrc in both repo root and grafana directory
    nvmrc_paths = [
        repo_root / "grafana" / ".nvmrc",  # Check grafana directory first
        repo_root / ".nvmrc",               # Then repo root
    ]

    nvmrc_path = None
    for path in nvmrc_paths:
        if path.exists():
            nvmrc_path = path
            break

    # Check if .nvmrc exists and read required version
    if nvmrc_path:
        with open(nvmrc_path, 'r') as f:
            required_version = f.read().strip()
        print(f"Found .nvmrc at {nvmrc_path} with Node version: {required_version}")
    else:
        # Create .nvmrc in grafana directory if it doesn't exist
        nvmrc_path = repo_root / "grafana" / ".nvmrc"
        required_version = "20"  # Default to Node 20 LTS
        print(f"No .nvmrc found, creating {nvmrc_path} with Node {required_version}")
        nvmrc_path.parent.mkdir(parents=True, exist_ok=True)
        with open(nvmrc_path, 'w') as f:
            f.write(f"{required_version}\n")

    # Update .nvmrc to a compatible version if needed
    # ESLint requires ^18.18.0 || ^20.9.0 || >=21.1.0
    if required_version in ["16", "17", "19"]:
        required_version = "20"
        print(f"Updating {nvmrc_path} to Node {required_version} (required by dependencies)")
        with open(nvmrc_path, 'w') as f:
            f.write(f"{required_version}\n")

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
    grafana_dir = repo_root / "grafana"

    print("=== Grafana Plugin CI Validation ===\n")

    # Setup Node version first
    if not setup_nvm_and_node(repo_root):
        return 1
    
    # Install dependencies (must be done from root for yarn workspaces)
    # Set NODE_ENV=development to ensure devDependencies are installed
    print("\n=== Installing dependencies ===")
    if run_cmd("NODE_ENV=development yarn install", repo_root) != 0:
        print("❌ Dependency installation failed")
        return 1
    
    # Type checking
    print("\n=== Type checking ===")
    if run_cmd("yarn typecheck", grafana_dir) != 0:
        print("❌ Type checking failed")
        return 1
    
    # Linting
    print("\n=== Linting ===")
    if run_cmd("yarn workspace micromegas-micromegas-datasource lint", repo_root) != 0:
        print("❌ Linting failed")
        return 1

    # Unit tests
    print("\n=== Unit tests ===")
    if run_cmd("yarn workspace micromegas-micromegas-datasource test:ci", repo_root) != 0:
        print("❌ Unit tests failed")
        return 1

    # Frontend build
    print("\n=== Frontend build ===")
    if run_cmd("yarn workspace micromegas-micromegas-datasource build", repo_root) != 0:
        print("❌ Frontend build failed")
        return 1
    
    # Check for Go backend
    magefile = grafana_dir / "Magefile.go"
    if magefile.exists():
        print("\n=== Go backend detected ===")
        
        # Go vet
        print("\n=== Go vet ===")
        if run_cmd("go vet ./...", grafana_dir) != 0:
            print("❌ Go vet failed")
            return 1
        
        # Go test
        print("\n=== Go test ===")
        if run_cmd("mage coverage", grafana_dir) != 0:
            print("❌ Go tests failed")
            return 1
        
        # Go build
        print("\n=== Go build ===")
        if run_cmd("mage build", grafana_dir) != 0:
            print("❌ Go build failed")
            return 1
    
    print("\n✅ All checks passed!")
    return 0

if __name__ == "__main__":
    sys.exit(main())
