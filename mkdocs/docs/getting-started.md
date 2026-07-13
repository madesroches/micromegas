# Getting Started

Try Micromegas locally with Docker — no build, no dev environment.

## Prerequisites

- **[Docker](https://www.docker.com/get-started/)** with Compose v2 (v2.23.1+; bundled with current Docker Desktop and recent Docker Engine)

## 1. Start Micromegas

### Option A: clone-free

Fetch the compose file and start it, without cloning the repo:

```bash
curl -O https://raw.githubusercontent.com/madesroches/micromegas/main/docker/docker-compose.monolith.yaml
docker compose -f docker-compose.monolith.yaml up
```

### Option B: from a clone

```bash
git clone https://github.com/madesroches/micromegas.git
cd micromegas
docker compose -f docker/docker-compose.monolith.yaml up
```

Either way, Docker pulls `marcantoinedesroches/micromegas-monolith:latest` and starts PostgreSQL plus a single monolith process running all roles (ingestion, analytics, web, maintenance). Data is stored in Docker volumes (Postgres data + a file-backed object store), so it persists across restarts but is easy to wipe (see [Stopping / cleaning up](#stopping-cleaning-up)).

??? note "Compose file contents"
    ```yaml
    services:
      postgres:
        image: postgres:16
        environment:
          POSTGRES_USER: micromegas
          POSTGRES_PASSWORD: micromegas
          POSTGRES_DB: micromegas
        configs:
          - source: pg_init
            target: /docker-entrypoint-initdb.d/init-databases.sql
        volumes:
          - pgdata:/var/lib/postgresql/data
        healthcheck:
          test: ["CMD-SHELL", "pg_isready -U micromegas"]
          interval: 5s
          timeout: 5s
          retries: 5

      micromegas:
        image: marcantoinedesroches/micromegas-monolith:latest
        depends_on:
          postgres:
            condition: service_healthy
        command:
          - "--roles"
          - "all"
          - "--listen-endpoint-http"
          - "0.0.0.0:9000"
          - "--frontend-dir"
          - "/app/frontend"
          - "--disable-auth"
        ports:
          - "9000:9000"
          - "50051:50051"
          - "3000:3000"
        environment:
          MICROMEGAS_SQL_CONNECTION_STRING: "postgres://micromegas:micromegas@postgres:5432/micromegas"
          MICROMEGAS_APP_SQL_CONNECTION_STRING: "postgres://micromegas:micromegas@postgres:5432/micromegas_app"
          MICROMEGAS_TELEMETRY_URL: "http://micromegas:9000"
          MICROMEGAS_FLUSH_PERIOD: "5"
          MICROMEGAS_OBJECT_STORE_URI: "file:///data"
          MICROMEGAS_WEB_CORS_ORIGIN: "http://localhost:3000"
          MICROMEGAS_BASE_PATH: "/"
        volumes:
          - lake:/data

    configs:
      pg_init:
        content: |
          CREATE DATABASE micromegas_app;

    volumes:
      pgdata:
      lake:
    ```

## 2. Open the web app

Once the containers are up, open [http://localhost:3000](http://localhost:3000). This is the analytics web app — a notebook-style UI for querying and visualizing the data in your Micromegas instance. Since the monolith ingests its own telemetry (see below), you can immediately open a notebook and query its self-telemetry.

## 3. (Optional) Run a query from Python

```bash
pip install micromegas
```

```python
import datetime
import micromegas

# Connects to grpc://localhost:50051 by default — matches the compose
# file's FlightSQL port, so no configuration is needed locally.
client = micromegas.connect()

now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(minutes=5)
end = now

sql = """
    SELECT time, process_id, level, target, msg
    FROM log_entries
    ORDER BY time DESC
    LIMIT 10;
"""

df = client.query(sql, begin, end)
print(df)
```

The monolith ingests its own traces and logs by default, so this query returns Micromegas's own self-telemetry rather than an empty table. Rows only appear after the first sink flush (`MICROMEGAS_FLUSH_PERIOD`, 5s in the compose file) **and** the maintenance role's continuous materialization of the global view (a ~1s cron) — so give it a couple of seconds after startup before you expect to see results.

!!! tip "Querying from outside Docker"
    The Python client's `MICROMEGAS_ANALYTICS_URI` environment variable can override the FlightSQL endpoint the CLI/client connects to — useful if you're running the query from a different host or container.

## What you just ran

This setup is for **evaluation, not production**:

- `--disable-auth` — no authentication on ingestion, FlightSQL, or the web app
- A file-backed object store (`file:///data`) instead of S3/GCS
- A single process running every role, with no isolation or horizontal scaling
- Data lives in Docker volumes that are easy to delete (see below)

!!! warning "Not for production"
    For a real deployment, see [Authentication](admin/authentication.md) to secure your instance and [Monolith Deployment](admin/monolith.md) for configuration options, or split into per-role services for horizontal scale-out.

## Stopping / cleaning up

```bash
# Stop the containers (keeps data)
docker compose -f docker-compose.monolith.yaml down

# Stop and delete all data (Postgres + object store volumes)
docker compose -f docker-compose.monolith.yaml down -v
```

## Next Steps

1. **[Query Guide](query-guide/index.md)** - Learn how to query your observability data
2. **[Architecture Overview](architecture/index.md)** - Understand the system design
3. **[Unreal Engine Integration](unreal/index.md)** - Add observability to your Unreal Engine games
4. **[Instrument Your Application](query-guide/python-api.md)** - Start collecting telemetry from your own applications

Building from source or contributing code? See the [Build Guide](development/build.md).

## Troubleshooting

### Common Issues

**Port already in use**
: The compose file binds `3000` (web app), `9000` (ingestion), and `50051` (FlightSQL) on the host. Stop whatever else is using those ports, or edit the `ports:` mappings in the compose file.

**Image pull fails or hangs**
: Make sure Docker can reach Docker Hub, and that you're signed in if your organization requires it (`docker login`).

**`docker compose` rejects the `configs.content` block**
: You need Docker Compose v2.23.1+ for inline config content. Check your version with `docker compose version` and upgrade Docker Desktop / the `docker-compose-plugin` package.

**Empty query results**
: Expected for the first few seconds after startup — the sample query returns Micromegas's own self-telemetry, but rows only appear after the first sink flush (~5s) and the next maintenance materialization cycle (~1s cron). Wait a couple of seconds and re-run the query.

### Getting Help

If you encounter issues:

1. Check the container logs: `docker compose -f docker-compose.monolith.yaml logs`
2. Create an issue on [GitHub](https://github.com/madesroches/micromegas/issues) with details about your setup
