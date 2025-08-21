# Architecture Overview

Micromegas is built on a modern lakehouse architecture designed for high-performance observability data collection and analytics.

## Core Components

```mermaid
graph TD
    subgraph "Application Layer"
        App1[Your Application]
        App2[Another Service]
        App3[Third Service]
    end
    
    subgraph "Micromegas Tracing"
        Lib1[micromegas-tracing]
        Lib2[micromegas-tracing]
        Lib3[micromegas-tracing]
        Sink1[telemetry-sink]
        Sink2[telemetry-sink]
        Sink3[telemetry-sink]
    end
    
    subgraph "Ingestion Layer"
        Ingestion[telemetry-ingestion-srv<br/>:9000 HTTP]
    end
    
    subgraph "Storage Layer"
        PG[(PostgreSQL<br/>Metadata & Schema)]
        S3[(Object Storage<br/>S3/GCS/Local<br/>Raw Payloads)]
    end
    
    subgraph "Maintenance"
        Admin[telemetry-admin<br/>crond]
    end
    
    subgraph "Analytics Layer"
        DataFusion[DataFusion Engine]
        Parquet[(Parquet Files<br/>Columnar Views)]
        FlightSQL[flight-sql-srv<br/>:50051 FlightSQL]
        WebApp[analytics-web-srv<br/>:8000 HTTP]
    end
    
    subgraph "Client Layer"
        PyClient[Python Client]
        Grafana[Grafana Plugin]
        Custom[Custom Clients]
        Browser[Web Browser<br/>Analytics UI]
    end
    
    App1 --> Lib1
    App2 --> Lib2 
    App3 --> Lib3
    Lib1 --> Sink1
    Lib2 --> Sink2
    Lib3 --> Sink3
    
    Sink1 --> Ingestion
    Sink2 --> Ingestion
    Sink3 --> Ingestion
    
    Ingestion --> PG
    Ingestion --> S3
    
    PG --> DataFusion
    S3 --> DataFusion
    DataFusion --> Parquet
    DataFusion --> FlightSQL
    
    Admin --> PG
    Admin --> S3
    Admin --> Parquet
    
    FlightSQL --> PyClient
    FlightSQL --> Grafana
    FlightSQL --> Custom
    FlightSQL --> WebApp
    WebApp --> Browser
    
    classDef app fill:#e8f5e8
    classDef tracing fill:#fff3e0
    classDef service fill:#f3e5f5
    classDef storage fill:#e1f5fe
    classDef client fill:#fce4ec
    
    class App1,App2,App3 app
    class Lib1,Lib2,Lib3,Sink1,Sink2,Sink3 tracing
    class Ingestion,FlightSQL,DataFusion,Admin,WebApp service
    class PG,S3,Parquet storage
    class PyClient,Grafana,Custom,Browser client
```

### Component Responsibilities

#### Data Collection
- **Tracing Library**: Ultra-low overhead (20ns per event) instrumentation embedded in applications
- **Telemetry Sink**: Batches events and handles transmission to ingestion service
- **Ingestion Service**: HTTP endpoint for receiving telemetry data from sinks

#### Data Storage
- **PostgreSQL**: Stores metadata, process information, and stream definitions
- **Object Storage**: Stores raw telemetry payloads in efficient binary format (S3, GCS, or local files)
- **Lakehouse**: Materialized Parquet views created on-demand for fast analytics

#### Analytics Engine
- **DataFusion**: SQL query engine with vectorized execution optimized for Parquet (columnar format)
- **FlightSQL**: High-performance query protocol using Apache Arrow for data transfer
- **HTTP Gateway**: REST API gateway for accessing FlightSQL analytics service
- **Analytics Web App**: Web interface for exploring data, generating Perfetto traces, and monitoring processes
- **Maintenance Daemon**: Background processing for view materialization and data lifecycle

## Data Flow

```mermaid
flowchart TD
    App[Application Code] --> Lib[Micromegas Tracing]
    Lib --> Sink[Telemetry Sink]
    Sink --> HTTP[HTTP Ingestion Service]
    
    HTTP --> PG[(PostgreSQL<br/>Metadata)]
    HTTP --> S3[(Object Storage<br/>Raw Payloads)]
    
    PG --> Analytics[Analytics Engine]
    S3 --> Analytics
    Analytics --> Parquet[(Parquet Files<br/>Lakehouse)]
    Analytics --> Client[SQL Client]
    
    Client --> Dashboard[Dashboards & Analysis]
    
    classDef storage fill:#e1f5fe
    classDef compute fill:#f3e5f5
    classDef client fill:#e8f5e8
    
    class PG,S3,Parquet storage
    class HTTP,Analytics compute
    class App,Client,Dashboard client
```

### Data Flow Steps

1. **Instrumentation**: Applications emit telemetry events using the Micromegas tracing library
2. **Collection**: Events are batched and sent to the ingestion service via HTTP
3. **Storage**: Metadata stored in PostgreSQL, raw payloads stored in object storage
4. **Materialization**: Views created on-demand from raw data using DataFusion
5. **Query**: SQL interface provides analytics capabilities through FlightSQL

## Lakehouse Architecture

```mermaid
flowchart TD
    subgraph "Data Lake Layer"
        Events[Live Events<br/>Logs, Metrics, Spans]
        Binary[(Binary Blocks<br/>LZ4 Compressed<br/>Custom Format)]
    end
    
    subgraph "Processing Layer"
        JIT[JIT ETL Engine]
        Live[Live ETL<br/>Maintenance Daemon]
    end
    
    subgraph "Lakehouse Layer"
        Parquet[(Parquet Files<br/>Columnar Format<br/>Optimized for Analytics)]
        Views[Materialized Views<br/>Global & Process-Scoped]
    end
    
    subgraph "Query Layer"
        DataFusion[DataFusion SQL Engine]
        Client[Query Clients]
    end
    
    Events --> Binary
    Binary --> JIT
    Binary --> Live
    
    JIT --> Parquet
    Live --> Views
    Views --> Parquet
    
    Parquet --> DataFusion
    DataFusion --> Client
    
    JIT -.->|"On-demand<br/>Process-scoped"| Views
    Live -.->|"Continuous<br/>Global views"| Views
    
    classDef datalake fill:#ffebee
    classDef process fill:#e8f5e8
    classDef lakehouse fill:#e3f2fd
    classDef query fill:#f3e5f5
    
    class Events,Binary datalake
    class JIT,Live process
    class Parquet,Views lakehouse
    class DataFusion,Client query
```

### Data Transformation Flow

#### 1. Data Lake Ingestion
- Events collected from applications in real-time
- Stored as compressed binary blocks in object storage
- Custom binary format optimized for high-throughput writes

#### 2. Dual Processing Strategies

**Live ETL (Maintenance Daemon)**:
- Processes recent data continuously (every second/minute/hour)
- Creates global materialized views for cross-process analytics
- Optimized for dashboards and real-time monitoring

**JIT ETL (On-Demand)**:
- Triggered when querying process-specific data
- Fetches relevant blocks, decompresses, and converts to Parquet
- Optimized for deep-dive analysis and debugging

#### 3. Lakehouse Analytics Optimization
- Parquet columnar format enables efficient scanning and filtering
- Dictionary compression reduces storage and improves query performance  
- Predicate pushdown leverages Parquet metadata for fast data pruning

## Analytics Web Application

The analytics web app provides a modern web interface for exploring telemetry data. It consists of:

- **Backend**: Rust-based web server (`analytics-web-srv`) using Axum framework
- **Frontend**: Next.js React application with TypeScript  
- **Integration**: Direct FlightSQL connection to analytics service

### Key Features

- **Process Explorer**: View active processes with real-time metadata
- **Log Viewer**: Stream log entries with level filtering and color coding
- **Trace Generation**: Generate and download Perfetto traces from process data
- **Process Statistics**: Detailed process metrics and monitoring

!!! warning "Development Stage"
    The Analytics Web Application is in early development and only suitable for local testing. Not recommended for production use.

## Design Principles

- **High-frequency collection**: Support for 100k+ events/second per process
- **Cost-efficient storage**: Cheap object storage for raw data with on-demand processing
- **Dual ETL strategy**: Live processing for recent data, JIT for historical analysis
- **Unified observability**: Logs, metrics, and traces in single queryable format
- **Tail sampling friendly**: Store everything cheaply, process selectively
