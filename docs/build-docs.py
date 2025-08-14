#!/usr/bin/env python3
"""
Build script for Micromegas MkDocs documentation.
"""

import subprocess
import sys
import os
from pathlib import Path

def run_command(cmd, cwd=None):
    """Run a command and print output."""
    print(f"Running: {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if result.stdout:
        print(result.stdout)
    if result.stderr:
        print(result.stderr, file=sys.stderr)
    return result.returncode == 0

def main():
    # Get the script directory (docs/) and project root
    script_dir = Path(__file__).parent
    project_root = script_dir.parent
    docs_dir = script_dir
    
    print("🏗️  Building Micromegas Documentation with MkDocs")
    print(f"Project root: {project_root}")
    print(f"Docs directory: {docs_dir}")
    
    # Check if virtual environment exists
    venv_dir = project_root / "docs-venv"
    if not venv_dir.exists():
        print("📦 Creating virtual environment for docs...")
        if not run_command([sys.executable, "-m", "venv", str(venv_dir)]):
            print("❌ Failed to create virtual environment")
            return 1
    
    # Determine pip path
    if os.name == 'nt':  # Windows
        pip_path = venv_dir / "Scripts" / "pip.exe"
        python_path = venv_dir / "Scripts" / "python.exe"
    else:  # Unix/Linux/macOS
        pip_path = venv_dir / "bin" / "pip"
        python_path = venv_dir / "bin" / "python"
    
    # Install requirements
    requirements_file = script_dir / "docs-requirements.txt"
    if requirements_file.exists():
        print("📦 Installing documentation dependencies...")
        if not run_command([str(pip_path), "install", "-r", str(requirements_file)]):
            print("❌ Failed to install requirements")
            return 1
    
    # Install MkDocs if not in requirements
    print("📦 Ensuring MkDocs is installed...")
    if not run_command([str(pip_path), "install", "mkdocs", "mkdocs-material"]):
        print("❌ Failed to install MkDocs")
        return 1
    
    # Build documentation
    mkdocs_path = venv_dir / ("Scripts" if os.name == 'nt' else "bin") / "mkdocs"
    
    print("🔨 Building MkDocs site...")
    if not run_command([str(mkdocs_path), "build", "--config-file", str(script_dir / "mkdocs.yml")], cwd=project_root):
        print("❌ Failed to build documentation")
        return 1
    
    site_dir = project_root / "site"
    print(f"✅ Documentation built successfully!")
    print(f"📁 Site files: {site_dir}")
    print(f"🌐 Open {site_dir / 'index.html'} in your browser")
    
    return 0

if __name__ == "__main__":
    sys.exit(main())
