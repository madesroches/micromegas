## Getting Started

For testing purposes, you can run the entire Micromegas stack on your local workstation. This guide will walk you through setting up the backend services and running a simple query.

### Prerequisites

*   [Docker](https://www.docker.com/get-started/) (for PostgreSQL)
*   [Python](https://www.python.org/downloads/) (for database setup script)
*   [Rust](https://www.rust-lang.org/tools/install) and Cargo (for building Micromegas services)

### Environment Variables

Before starting, set the following environment variables:

*   `MICROMEGAS_DB_USERNAME` and `MICROMEGAS_DB_PASSWD`: Used by the database configuration script.
*   `export MICROMEGAS_TELEMETRY_URL=http://localhost:9000`
*   `export MICROMEGAS_SQL_CONNECTION_STRING=postgres://{uname}:{passwd}@localhost:5432`
*   `export MICROMEGAS_OBJECT_STORE_URI=file:///some/local/path` (Replace `/some/local/path` with a directory on your system for object storage)

### Steps

1.  **Clone the repository:**

    ```bash
    git clone https://github.com/madesroches/micromegas.git
    cd micromegas
    ```

2.  **Start a local PostgreSQL instance:**

    This will set up and run a PostgreSQL database using Docker.

    ```bash
    cd local_test_env/db
    ./run.py
    ```

3.  **Start the Ingestion Server:**

    Open a new terminal and run:

    ```bash
    cd rust
    cargo run -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000
    ```

4.  **Start the FlightSQL Server:**

    Open another new terminal and run:

    ```bash
    cd rust
    cargo run -p flight-sql-srv -- --disable-auth
    ```

5.  **Start the Daemon:**

    Open a third new terminal and run:

    ```bash
    cd rust
    cargo run -p telemetry-admin -- crond
    ```

6.  **Query the Analytics Service (Python Example):**

    Ensure you have the Python API installed (`pip install micromegas`). Then, in a Python interpreter or script:

    ```python
    import datetime
    import micromegas

    client = micromegas.connect() # Connects to localhost by default
    now = datetime.datetime.now(datetime.timezone.utc)
    begin = now - datetime.timedelta(days=1)
    end = now

    sql = """
    SELECT *
    FROM log_entries
    ORDER BY time DESC
    LIMIT 10
    ;"""

    df = client.query(sql, begin, end)
    print(df) # Dataframe containing the result of the query
    ```
