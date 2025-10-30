#!/usr/bin/env python3
"""
Grafana Plugin E2E Tests runner.
Runs Playwright e2e tests with Docker Grafana instance.
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
    
    print("=== Grafana Plugin E2E Tests ===\n")
    
    # Install dependencies
    print("\n=== Installing dependencies ===")
    if run_cmd("NODE_ENV=development yarn install", repo_root) != 0:
        print("❌ Dependency installation failed")
        return 1
    
    # Install Playwright browsers
    print("\n=== Installing Playwright browsers ===")
    if run_cmd("yarn playwright install --with-deps chromium", grafana_dir) != 0:
        print("❌ Playwright browser installation failed")
        return 1
    
    # Start Docker Grafana
    print("\n=== Starting Grafana Docker container ===")
    if run_cmd("docker compose up -d", grafana_dir) != 0:
        print("❌ Failed to start Grafana Docker")
        return 1
    
    # Run e2e tests
    print("\n=== Running e2e tests ===")
    test_result = run_cmd("yarn e2e", grafana_dir)
    
    # Stop Docker
    print("\n=== Stopping Grafana Docker container ===")
    run_cmd("docker compose down", grafana_dir)
    
    if test_result != 0:
        print("❌ E2E tests failed")
        return 1
    
    print("\n✅ All e2e tests passed!")
    return 0

if __name__ == "__main__":
    sys.exit(main())
