## Getting Started

For testing purposes, you can run the Micromegas stack on your local workstation. This guide covers two setup modes:

*   **Full Local Stack** — all backend services running locally, ideal for end-to-end development.
*   **Hybrid Setup** — local frontend with a remote production backend, ideal for frontend development or when you only need to query existing data.

### Prerequisites

*   [Docker](https://www.docker.com/get-started/) (for PostgreSQL)
*   [Python](https://www.python.org/downloads/) (for database setup script)
*   [Rust](https://www.rust-lang.org/tools/install) and Cargo (for building Micromegas services)
*   [Node.js](https://nodejs.org/) 18+
*   [Yarn](https://yarnpkg.com/) (`npm install -g yarn`)
*   wasm32 Rust target: `rustup target add wasm32-unknown-unknown`
*   [wasm-bindgen CLI](https://rustwasm.github.io/wasm-bindgen/) (version must match `rust/datafusion-wasm/Cargo.lock`)

---

### Full Local Stack

Run all services locally: PostgreSQL, ingestion, FlightSQL, admin daemon, and optionally the web app.

#### Environment Variables

Before starting, set the following environment variables:

*   `MICROMEGAS_DB_USERNAME` and `MICROMEGAS_DB_PASSWD`: Used by the database configuration script.
*   `export MICROMEGAS_TELEMETRY_URL=http://localhost:9000`
*   `export MICROMEGAS_SQL_CONNECTION_STRING=postgres://{uname}:{passwd}@localhost:5432`
*   `export MICROMEGAS_OBJECT_STORE_URI=file:///some/local/path` (Replace `/some/local/path` with a directory on your system for object storage)

#### Steps

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

---

### Hybrid Setup: Local Frontend + Remote Backend

This mode runs the analytics web app locally but queries data from a remote FlightSQL server. You skip the local ingestion server, FlightSQL server, and admin daemon. You still need a local PostgreSQL instance for the app database.

#### Environment Variables

Set the same database variables as the full local stack (`MICROMEGAS_DB_USERNAME`, `MICROMEGAS_DB_PASSWD`, `MICROMEGAS_SQL_CONNECTION_STRING`, `MICROMEGAS_OBJECT_STORE_URI`). You do **not** need `MICROMEGAS_TELEMETRY_URL`.

#### Steps

1.  **Clone the repository** (same as Full Local Stack step 1).

2.  **Start a local PostgreSQL instance** (same as Full Local Stack step 2).

3.  **Build the WASM engine:**

    The WASM binary is not committed to git and must be built locally.

    ```bash
    rustup target add wasm32-unknown-unknown

    # Check the required wasm-bindgen version
    grep -A2 'name = "wasm-bindgen"' rust/datafusion-wasm/Cargo.lock | grep version

    # Install the matching version
    cargo install wasm-bindgen-cli --version <VERSION_FROM_ABOVE>

    # Build the WASM binary
    python3 rust/datafusion-wasm/build.py
    ```

4.  **Start the Analytics Web App with remote backend:**

    ```bash
    cd analytics-web-app
    python3 start_analytics_web.py --remote-backend <FLIGHTSQL_URL>
    ```

    Replace `<FLIGHTSQL_URL>` with the URL of the remote FlightSQL server. This command starts the Rust backend on port 8000, seeds a remote data source in the local app database, builds the WASM engine if needed, and starts the Vite dev server on port 3000.

5.  **Open the app** at [http://localhost:3000/mmlocal/](http://localhost:3000/mmlocal/).
