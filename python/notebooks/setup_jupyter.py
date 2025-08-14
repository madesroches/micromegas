#!/usr/bin/env python3
"""
Setup script for Jupyter environment to run async events notebooks.
This creates a virtual environment with all necessary dependencies.
"""

import os
import sys
import subprocess
import venv
from pathlib import Path

def run_command(cmd, cwd=None):
    """Run a command and handle errors"""
    print(f"🔧 Running: {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"❌ Error running command: {' '.join(cmd)}")
        print(f"   stdout: {result.stdout}")
        print(f"   stderr: {result.stderr}")
        sys.exit(1)
    return result

def main():
    print("🔧 Setting up Jupyter environment for async events notebooks...")
    
    # Get script directory and venv path
    script_dir = Path(__file__).parent.absolute()
    venv_dir = script_dir / "jupyter_env"
    
    print(f"📂 Script directory: {script_dir}")
    print(f"📦 Virtual environment: {venv_dir}")
    
    # Create virtual environment if it doesn't exist
    if not venv_dir.exists():
        print("📦 Creating virtual environment...")
        venv.create(venv_dir, with_pip=True)
    else:
        print("📦 Virtual environment already exists")
    
    # Determine pip path based on OS
    if os.name == 'nt':  # Windows
        pip_path = venv_dir / "Scripts" / "pip"
        python_path = venv_dir / "Scripts" / "python"
    else:  # Unix/Linux/Mac
        pip_path = venv_dir / "bin" / "pip"
        python_path = venv_dir / "bin" / "python"
    
    # Upgrade pip
    print("⬆️ Upgrading pip...")
    run_command([str(python_path), "-m", "pip", "install", "--upgrade", "pip"])
    
    # Install Jupyter and required packages
    print("📚 Installing Jupyter and dependencies...")
    packages = [
        "jupyter",
        "jupyterlab", 
        "pandas",
        "pyarrow",
        "numpy",
        "matplotlib",
        "seaborn",
        "plotly"
    ]
    
    run_command([str(pip_path), "install"] + packages)
    
    # Install micromegas package in development mode
    print("🔗 Installing micromegas package...")
    micromegas_dir = script_dir.parent / "micromegas"
    
    if micromegas_dir.exists():
        run_command([str(pip_path), "install", "-e", str(micromegas_dir)])
    else:
        print(f"⚠️ Warning: micromegas package directory not found at {micromegas_dir}")
        print("   You may need to install it manually or adjust the path")
    
    print("✅ Setup complete!")
    print("")
    print("📝 To use the Jupyter environment:")
    
    if os.name == 'nt':  # Windows
        print(f"   1. Activate: {venv_dir}\\Scripts\\activate")
        print("   2. Start Jupyter: jupyter lab")
    else:  # Unix/Linux/Mac
        print(f"   1. Activate: source {venv_dir}/bin/activate")
        print("   2. Start Jupyter: jupyter lab")
    
    print("   3. Open async_traces.ipynb in the browser")
    print("")
    print("🛑 To deactivate when done: deactivate")
    print("")
    print(f"🗑️ To remove the environment: rm -rf {venv_dir}")

if __name__ == "__main__":
    main()