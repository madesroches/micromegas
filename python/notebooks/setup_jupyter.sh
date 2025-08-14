#!/bin/bash
set -e

# Setup script for Jupyter environment to run async events notebooks
# This creates a virtual environment with all necessary dependencies

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENV_DIR="$SCRIPT_DIR/jupyter_env"

echo "🔧 Setting up Jupyter environment for async events notebooks..."

# Check if python3 is available
if ! command -v python3 &> /dev/null; then
    echo "❌ Error: python3 is required but not installed"
    exit 1
fi

# Create virtual environment if it doesn't exist
if [ ! -d "$VENV_DIR" ]; then
    echo "📦 Creating virtual environment..."
    python3 -m venv "$VENV_DIR"
else
    echo "📦 Virtual environment already exists"
fi

# Activate virtual environment
echo "🔌 Activating virtual environment..."
source "$VENV_DIR/bin/activate"

# Upgrade pip
echo "⬆️ Upgrading pip..."
pip install --upgrade pip

# Install Jupyter and required packages
echo "📚 Installing Jupyter and dependencies..."
pip install \
    jupyter \
    jupyterlab \
    pandas \
    pyarrow \
    numpy \
    matplotlib \
    seaborn \
    plotly

# Install micromegas package in development mode
echo "🔗 Installing micromegas package..."
MICROMEGAS_DIR="$SCRIPT_DIR/../micromegas"
if [ -d "$MICROMEGAS_DIR" ]; then
    pip install -e "$MICROMEGAS_DIR"
else
    echo "⚠️ Warning: micromegas package directory not found at $MICROMEGAS_DIR"
    echo "   You may need to install it manually or adjust the path"
fi

echo "✅ Setup complete!"
echo ""
echo "📝 To use the Jupyter environment:"
echo "   1. Activate the environment: source $VENV_DIR/bin/activate"
echo "   2. Start Jupyter Lab: jupyter lab"
echo "   3. Open async_traces.ipynb in the browser"
echo ""
echo "🛑 To deactivate when done: deactivate"
echo ""
echo "🗑️ To remove the environment: rm -rf $VENV_DIR"