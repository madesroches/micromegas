# Getting Started

This guide will walk you through setting up Micromegas on your local workstation for testing and development purposes.

## Prerequisites

Before you begin, ensure you have the following installed:

- **[Docker](https://www.docker.com/get-started/)** - For running PostgreSQL
- **[Python 3.8+](https://www.python.org/downloads/)** - For the client API and setup scripts
- **[Rust](https://www.rust-lang.org/tools/install)** - For building Micromegas services

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

### 2. Start PostgreSQL Database

Start a local PostgreSQL instance using Docker:

```bash
cd local_test_env/db
python run.py
```

This will:
- Pull the PostgreSQL Docker image
- Start the database container
- Set up the initial schema

### 3. Start Core Services

You'll need to start three services in separate terminals:

#### Terminal 1: Ingestion Server
```bash
cd rust
cargo run -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000
```

#### Terminal 2: FlightSQL Server
```bash
cd rust
cargo run -p flight-sql-srv -- --disable-auth
```

#### Terminal 3: Maintenance Daemon
```bash
cd rust
cargo run -p telemetry-admin -- crond
```

!!! info "Service Roles"
    - **Ingestion Server**: Receives telemetry data from applications
    - **FlightSQL Server**: Provides SQL query interface for analytics
    - **Maintenance Daemon**: Handles background processing and view materialization

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

1. **[Learn to Query Data](query-guide/index.md)** - Explore the SQL interface and available data
2. **[Understand the Architecture](architecture/index.md)** - Learn how Micromegas components work together
3. **[Instrument Your Application](query-guide/python-api.md)** - Start collecting telemetry from your own applications

## Troubleshooting

### Common Issues

**Connection refused when querying**
: Make sure all three services are running and the FlightSQL server is listening on the default port.

**Database connection errors**
: Verify your PostgreSQL container is running and the connection string environment variable is correct.

**Empty query results**
: This is normal for a fresh installation. You'll need to instrument an application to start collecting telemetry data.

**Build errors**
: Ensure you have the latest Rust toolchain installed and try `cargo update` in the `rust/` directory.

### Getting Help

If you encounter issues:

1. Check the service logs in each terminal for error messages
2. Verify all environment variables are set correctly
3. Create an issue on [GitHub](https://github.com/madesroches/micromegas/issues) with details about your setup
