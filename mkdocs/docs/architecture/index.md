# Architecture Overview

Micromegas is built on a modern lakehouse architecture designed for high-performance observability data collection and analytics.

## Core Components

### Data Collection
- **Tracing Library**: Ultra-low overhead (20ns per event) instrumentation
- **Telemetry Sink**: Event collection and transmission
- **HTTP Gateway**: REST API for telemetry ingestion

### Data Storage
- **PostgreSQL**: Metadata, process information, and stream definitions
- **Object Storage**: Raw telemetry data in efficient binary format
- **Lakehouse**: Materialized Parquet views for fast analytics

### Analytics Engine
- **DataFusion**: SQL query engine with vectorized execution
- **FlightSQL**: High-performance query protocol
- **Apache Arrow**: Columnar data format for efficient transfer

## Data Flow

1. **Instrumentation**: Applications emit telemetry events
2. **Collection**: Events batched and sent to ingestion service
3. **Storage**: Metadata in PostgreSQL, payloads in object storage
4. **Materialization**: Views created on-demand from raw data
5. **Query**: SQL interface provides analytics capabilities

## Design Principles

- **High-frequency collection**: Support for 100k+ events/second per process
- **Cost-efficient storage**: Cheap object storage for raw data
- **On-demand processing**: ETL only when querying data
- **Unified observability**: Logs, metrics, and traces in single format

More architecture details coming soon...
