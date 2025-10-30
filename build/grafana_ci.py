#!/usr/bin/env python3
"""
Grafana Plugin CI validation script.
Runs all checks locally before pushing to CI.
"""
import subprocess
import sys
from pathlib import Path

def run_cmd(cmd: str, cwd: Path) -> int:
    print(f"Running: {cmd} in {cwd}")
    result = subprocess.run(cmd, shell=True, cwd=cwd, check=False)
    return result.returncode

def main():
    repo_root = Path(__file__).parent.parent
    grafana_dir = repo_root / "grafana"
    
    print("=== Grafana Plugin CI Validation ===\n")
    
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
    if run_cmd("yarn workspace micromegas-datasource lint", repo_root) != 0:
        print("❌ Linting failed")
        return 1
    
    # Unit tests
    print("\n=== Unit tests ===")
    if run_cmd("yarn workspace micromegas-datasource test:ci", repo_root) != 0:
        print("❌ Unit tests failed")
        return 1
    
    # Frontend build
    print("\n=== Frontend build ===")
    if run_cmd("yarn workspace micromegas-datasource build", repo_root) != 0:
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
