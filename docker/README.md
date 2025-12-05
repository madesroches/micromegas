# Docker Images

This directory contains Dockerfiles for building micromegas services.

## Images

| Dockerfile | Image | Description |
|------------|-------|-------------|
| `ingestion.Dockerfile` | `micromegas-ingestion` | Telemetry ingestion server (HTTP) |
| `flight-sql.Dockerfile` | `micromegas-flight-sql` | FlightSQL analytics server |
| `admin.Dockerfile` | `micromegas-admin` | Telemetry admin CLI |
| `http-gateway.Dockerfile` | `micromegas-http-gateway` | HTTP gateway server |
| `analytics-web.Dockerfile` | `micromegas-analytics-web` | Analytics web app (frontend + backend) |
| `all-in-one.Dockerfile` | `micromegas-all` | All services in one image |

## Building

Use the build script from the repository root:

```bash
# List available services
python build/build_docker_images.py --list

# Build all individual service images
python build/build_docker_images.py

# Build specific services
python build/build_docker_images.py ingestion flight-sql

# Build the all-in-one image
python build/build_docker_images.py all

# Build and push to DockerHub
python build/build_docker_images.py --push all
```

## Running

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
  marcantoinedesroches/micromegas-all:latest \
  telemetry-ingestion-srv --listen-endpoint-http 0.0.0.0:9000

# FlightSQL server
docker run -d --name flight-sql \
  -p 50051:50051 \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  marcantoinedesroches/micromegas-all:latest \
  flight-sql-srv

# Admin daemon
docker run -d --name admin \
  -e MICROMEGAS_SQL_CONNECTION_STRING \
  -e MICROMEGAS_OBJECT_STORE_URI \
  marcantoinedesroches/micromegas-all:latest \
  telemetry-admin crond

# Analytics web app
docker run -d --name analytics-web \
  -p 3000:3000 \
  -e MICROMEGAS_FLIGHTSQL_URL \
  -e MICROMEGAS_WEB_CORS_ORIGIN \
  marcantoinedesroches/micromegas-all:latest \
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
| HTTP Gateway | 8080 | HTTP |
| Analytics Web | 3000 | HTTP |
