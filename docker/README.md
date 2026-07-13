# Docker Images

This directory contains Dockerfiles for building micromegas services.

## Images

Seven services are published to Docker Hub under `marcantoinedesroches/`:

| Dockerfile | Image | Description |
|------------|-------|-------------|
| `ingestion.Dockerfile` | `marcantoinedesroches/micromegas-ingestion` | Telemetry ingestion server (HTTP) |
| `flight-sql.Dockerfile` | `marcantoinedesroches/micromegas-flight-sql` | FlightSQL analytics server |
| `maintenance.Dockerfile` | `marcantoinedesroches/micromegas-maintenance` | Maintenance daemon |
| `object-cache.Dockerfile` | `marcantoinedesroches/micromegas-object-cache` | Shared object range cache service |
| `http-gateway.Dockerfile` | `marcantoinedesroches/micromegas-http-gateway` | HTTP gateway server |
| `analytics-web.Dockerfile` | `marcantoinedesroches/micromegas-analytics-web` | Analytics web app (frontend + backend) |
| `monolith.Dockerfile` | `marcantoinedesroches/micromegas-monolith` | Single-process monolith (all roles in one binary) |
| `all-in-one.Dockerfile` | `micromegas-all` | All services in one image (dev/test only, not published) |
| `github-runner.Dockerfile` | `micromegas-github-runner` | Self-hosted GitHub Actions runner (see `build/dev_worker.py`) |

### Tag scheme

| Arch | Tags |
|------|------|
| amd64 | `…:<version>`, `…:latest` |
| arm64 | `…:<version>-arm64`, `…:latest-arm64` |

## Building

Use the build script from the repository root:

```bash
# List available services
python build/build_docker_images.py --list

# Build all individual service images (amd64, local only)
python build/build_docker_images.py

# Build specific services
python build/build_docker_images.py ingestion flight-sql

# Build and push amd64 images to Docker Hub
python build/build_docker_images.py --push

# Build arm64 images locally (cross-compiled, no push)
python build/build_docker_images.py --arm64

# Build and push arm64 images to Docker Hub
python build/build_docker_images.py --arm64 --push

# Build both amd64 and arm64 locally (no push)
python build/build_docker_images.py --all-arches

# Build and push both amd64 and arm64 in one run (release)
python build/build_docker_images.py --all-arches --push
```

### One-time setup (required for arm64 builds)

```bash
# Create and activate a buildx builder
docker buildx create --use

# Install QEMU for the arm64 runtime stage
docker run --privileged --rm tonistiigi/binfmt --install arm64

# Log in to Docker Hub
docker login
```

### Release publish (both arches, all services)

```bash
SVCS="ingestion flight-sql maintenance object-cache http-gateway analytics-web monolith"
python build/build_docker_images.py $SVCS --all-arches --push --version X.Y.0
```

Verify after pushing:

```bash
docker buildx imagetools inspect marcantoinedesroches/micromegas-monolith:X.Y.0
```

## Running

### Monolith (recommended single-image deployment)

> Requires Docker Compose v2.23.1+ (for the compose file's inline `configs.content` DB-init block).

```bash
docker compose -f docker/docker-compose.monolith.yaml up
```

Or run directly:

```bash
docker run -d -p 9000:9000 -p 50051:50051 -p 3000:3000 \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  marcantoinedesroches/micromegas-monolith:latest \
  --roles all \
  --listen-endpoint-http 0.0.0.0:9000 \
  --frontend-dir /app/frontend
```

### Individual Images (Production)

```bash
# Ingestion server
docker run -d -p 9000:9000 \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  marcantoinedesroches/micromegas-ingestion:latest

# FlightSQL server
docker run -d -p 50051:50051 \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  marcantoinedesroches/micromegas-flight-sql:latest

# Maintenance daemon
docker run -d \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  marcantoinedesroches/micromegas-maintenance:latest

# Object cache (fronts a bucket-only S3/GCS origin; see Environment Variables below)
docker run -d -p 8080:8080 \
  -e MICROMEGAS_OBJECT_CACHE_ORIGIN_URI \
  -e MICROMEGAS_OBJECT_CACHE_DISK_PATH=/data \
  -e MICROMEGAS_API_KEYS \
  -v object-cache-data:/data \
  marcantoinedesroches/micromegas-object-cache:latest

# HTTP gateway
docker run -d -p 3000:3000 \
  marcantoinedesroches/micromegas-http-gateway:latest

# Analytics web app
docker run -d -p 3000:3000 \
  -e MICROMEGAS_FLIGHTSQL_URL \
  -e MICROMEGAS_WEB_CORS_ORIGIN \
  -e MICROMEGAS_STATE_SECRET \
  -e MICROMEGAS_OIDC_CONFIG \
  marcantoinedesroches/micromegas-analytics-web:latest
```

### All-in-One Image (Dev/Test)

Run multiple services from a single image by specifying the command:

```bash
# Ingestion server
docker run -d --name ingestion \
  -p 9000:9000 \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  micromegas-all:latest \
  telemetry-ingestion-srv --listen-endpoint-http 0.0.0.0:9000

# FlightSQL server
docker run -d --name flight-sql \
  -p 50051:50051 \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  micromegas-all:latest \
  flight-sql-srv

# Maintenance daemon
docker run -d --name maintenance \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  micromegas-all:latest \
  telemetry-maintenance-srv

# Analytics web app
docker run -d --name analytics-web \
  -p 3000:3000 \
  -e MICROMEGAS_FLIGHTSQL_URL \
  -e MICROMEGAS_WEB_CORS_ORIGIN \
  micromegas-all:latest \
  analytics-web-srv --frontend-dir /app/frontend --disable-auth
```

## Environment Variables

### Ingestion Server
| Variable | Required | Description |
|----------|----------|-------------|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes | PostgreSQL connection string |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes | S3/GCS bucket URI for payloads |
| `MICROMEGAS_API_KEYS` | No | JSON array of API keys |
| `MICROMEGAS_OIDC_CONFIG` | No | OIDC configuration JSON |

### FlightSQL Server
| Variable | Required | Description |
|----------|----------|-------------|
| `MICROMEGAS_SQL_CONNECTION_STRING` | Yes | PostgreSQL connection string |
| `MICROMEGAS_OBJECT_STORE_URI` | Yes | S3/GCS bucket URI for payloads |

### Object Cache
| Variable | Required | Description |
|----------|----------|-------------|
| `MICROMEGAS_OBJECT_CACHE_ORIGIN_URI` | Yes | Bucket-only origin URI (e.g. `s3://my-bucket`, no path — the lake-root prefix arrives inside each request key) |
| `MICROMEGAS_OBJECT_CACHE_DISK_PATH` | Yes | Local disk path for the cache backend |
| `MICROMEGAS_API_KEYS` | Yes* | JSON array of `{"name":"...","key":"..."}` |
| `MICROMEGAS_OBJECT_CACHE_RAM_MB` | No | In-memory cache size (default `512`) |
| `MICROMEGAS_OBJECT_CACHE_DISK_GB` | No | On-disk cache size (default `50`) |
| `MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE` | No | Cache block size in bytes (default `1048576`) |
| `MICROMEGAS_OBJECT_CACHE_PREFIX` | Yes** | Allowed key prefixes, comma-separated (e.g. `blobs,views`); only matching keys are served |

*Required unless running with `--disable-auth` (development mode only)
**Required unless running with `--allow-all-prefixes` (development mode only)

### Analytics Web App
| Variable | Required | Description |
|----------|----------|-------------|
| `MICROMEGAS_FLIGHTSQL_URL` | Yes | FlightSQL server URL |
| `MICROMEGAS_WEB_CORS_ORIGIN` | Yes | CORS origin (e.g., `https://app.example.com`) |
| `MICROMEGAS_STATE_SECRET` | Yes* | OAuth state signing secret |
| `MICROMEGAS_OIDC_CONFIG` | Yes* | OIDC provider configuration JSON |
| `MICROMEGAS_AUTH_REDIRECT_URI` | Yes* | OAuth callback URL |
| `MICROMEGAS_COOKIE_DOMAIN` | No | Cookie domain for auth |
| `MICROMEGAS_SECURE_COOKIES` | No | Set `true` for HTTPS |

*Required unless running with `--disable-auth`

## Ports

| Service | Port | Protocol |
|---------|------|----------|
| Ingestion | 9000 | HTTP |
| FlightSQL | 50051 | gRPC |
| Object Cache | 8080 | HTTP |
| HTTP Gateway | 3000 | HTTP |
| Analytics Web | 3000 | HTTP |
