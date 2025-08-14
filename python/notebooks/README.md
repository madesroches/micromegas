# Async Events Notebooks

This directory contains Jupyter notebooks for exploring and analyzing async events data from the micromegas telemetry system.

## Notebooks

- `async_traces.ipynb` - Interactive exploration of async events, queries, and data analysis

## Setup

To set up the Jupyter environment and run the notebooks:

### Automatic Setup

Run the setup script to create a virtual environment with all dependencies:

```bash
python3 setup_jupyter.py
```

This will:
- Create a `jupyter_env/` virtual environment
- Install Jupyter Lab and required packages (pandas, pyarrow, matplotlib, etc.)
- Install the micromegas Python package in development mode

### Manual Setup

Alternatively, you can set up manually:

```bash
# Create virtual environment
python3 -m venv jupyter_env

# Activate it
source jupyter_env/bin/activate  # Linux/Mac
# or
jupyter_env\Scripts\activate     # Windows

# Install dependencies
pip install jupyter jupyterlab pandas pyarrow numpy matplotlib seaborn plotly

# Install micromegas package
pip install -e ../micromegas
```

## Usage

1. **Activate the environment:**
   ```bash
   source jupyter_env/bin/activate
   ```

2. **Start Jupyter Lab:**
   ```bash
   jupyter lab
   ```

3. **Open notebooks:** Navigate to `async_traces.ipynb` in the Jupyter interface

4. **Deactivate when done:**
   ```bash
   deactivate
   ```

## Prerequisites

- Python 3.8+
- Micromegas services running (telemetry ingestion and flight-sql servers)
- Async events data (run the telemetry generator to create test data)

## Notes

- The `jupyter_env/` directory is excluded from version control (it's user-specific)
- The notebooks connect to the local micromegas FlightSQL server on the default port
- Run the telemetry generator first to have async events data to explore