# Getting Started

This guide will walk you through setting up Micromegas on your local workstation for testing and development purposes.

## Prerequisites

Before you begin, ensure you have the following installed:

- **[Docker](https://www.docker.com/get-started/)** - For running PostgreSQL
- **[Python 3.8+](https://www.python.org/downloads/)** - For the client API and setup scripts
- **[Rust](https://www.rust-lang.org/tools/install)** - For building Micromegas services
- **Build tools** - C/C++ compiler and linker (required for Rust compilation)

Optional:
- **[tmux](https://github.com/tmux/tmux/wiki)** - For managing multiple services in a single terminal (Linux/macOS)

## Environment Setup

Set the following environment variables for local development:

```bash
# Database credentials (used by setup scripts)
export MICROMEGAS_DB_USERNAME=your_username
export MICROMEGAS_DB_PASSWD=your_password

# Service endpoints
export MICROMEGAS_TELEMETRY_URL=http://localhost:9000
export MICROMEGAS_SQL_CONNECTION_STRING=postgres://your_username:your_password@localhost:5432

# Object storage (replace with your local path)
export MICROMEGAS_OBJECT_STORE_URI=file:///path/to/local/storage
```

!!! tip "Object Storage Path"
    Choose a local directory for object storage, e.g., `/tmp/micromegas-storage` or `C:\temp\micromegas-storage` on Windows.

## Installation Steps

### 1. Clone the Repository

```bash
git clone https://github.com/madesroches/micromegas.git
cd micromegas
```

### 2. Install Build Tools

Before building the Rust components, install C/C++ build tools:

**Linux:**
```bash
sudo apt-get update
sudo apt-get install build-essential clang mold
```

!!! note "mold linker requirement"
    On Linux, the project requires the [mold linker](https://github.com/rui314/mold) as configured in `.cargo/config.toml`. This provides faster linking for large projects.

**macOS:**
```bash
xcode-select --install
```

**Windows:**
Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/)

### 3. Start All Services

#### Option A: Using tmux (Linux/macOS)

The easiest way to start all required services is using the development script with tmux:

```bash
# Start all services in a tmux session (debug mode)
python3 local_test_env/dev.py

# Or in release mode for better performance
python3 local_test_env/dev.py release
```

This will automatically:

- Build all Rust services
- Start PostgreSQL database
- Start telemetry-ingestion-srv on port 9000
- Start flight-sql-srv on port 50051
- Start telemetry-admin service
- Open a tmux session with all services running in separate panes

!!! tip "Managing Services with tmux"
    - To switch between service panes: `Ctrl+B` then arrow keys
    - To detach from tmux (leave services running): `Ctrl+B` then `D`
    - To reattach to tmux: `tmux attach -t micromegas`
    - To stop all services: `python3 local_test_env/stop-dev.py`

#### Option B: Manual Startup (Windows or without tmux)

If you're on Windows or prefer not to use tmux, start each service in a separate terminal:

**Terminal 1: PostgreSQL Database**
```bash
cd local_test_env/db
python run.py
```

**Terminal 2: Ingestion Server**
```bash
cd rust
cargo run -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000
```

**Terminal 3: FlightSQL Server**
```bash
cd rust
cargo run -p flight-sql-srv -- --disable-auth
```

**Terminal 4: Admin Service**
```bash
cd rust
cargo run -p telemetry-admin -- crond
```

!!! info "Service Roles"
    - **PostgreSQL**: Stores metadata and service configuration
    - **Ingestion Server**: Receives telemetry data from applications (port 9000)
    - **FlightSQL Server**: Provides SQL query interface for analytics (port 50051)
    - **Admin Service**: Handles background processing and global view materialization

## Verify Installation

### Install Python Client

```bash
pip install micromegas
```

### Test with Sample Query

Create a test script to verify everything is working:

```python
import datetime
import micromegas

# Connect to local Micromegas instance
client = micromegas.connect()

# Set up time range for query
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=1)
end = now

# Query recent log entries
sql = """
    SELECT time, process_id, level, target, msg
    FROM log_entries
    ORDER BY time DESC
    LIMIT 10;
"""

# Execute query and display results
df = client.query(sql, begin, end)
print(f"Found {len(df)} log entries")
print(df.head())
```

If you see a DataFrame with log entries (or an empty DataFrame if no data has been ingested yet), your installation is working correctly!

## Next Steps

Now that you have Micromegas running locally, you can:

1. **[Unreal Engine Integration](unreal/index.md)** - Add observability to your Unreal Engine games
2. **[Learn to Query Data](query-guide/index.md)** - Explore the SQL interface and available data
3. **[Understand the Architecture](architecture/index.md)** - Learn how Micromegas components work together
4. **[Instrument Your Application](query-guide/python-api.md)** - Start collecting telemetry from your own applications

## Troubleshooting

### Common Issues

**Connection refused when querying**
: Make sure all three services are running and the FlightSQL server is listening on the default port.

**Database connection errors**
: Verify your PostgreSQL container is running and the connection string environment variable is correct.

**Empty query results**
: This is normal for a fresh installation. You'll need to instrument an application to start collecting telemetry data.

**Build errors**
: Ensure you have the latest Rust toolchain installed.

### Getting Help

If you encounter issues:

1. Check the service logs in each terminal for error messages
2. Verify all environment variables are set correctly
3. Create an issue on [GitHub](https://github.com/madesroches/micromegas/issues) with details about your setup
