# Micromegas - high frequency telemetry

## Big Picture

### data flow
```mermaid
graph LR;
    rust-->ingestion-srv
    unreal-->ingestion-srv
    ingestion-srv-->postgresql[(PostgreSQL)]
    ingestion-srv-->s3[(S3)]
    postgresql-->analytics
    s3-->analytics
    analytics-->grafana
    analytics-->python_cli

```


## Instrumentation

### Data structures

```mermaid
classDiagram
Process *-- Stream
Stream *-- Block
Block *-- HeterogenousQueue~Events~


Process: uuid process_id
Process: size_t cpu_frequency
Process: string computer
Process: string username
Process: string exe

Stream : uuid process_id
Stream : uuid stream_id
Stream : string type [cpu, metrics, log]
Stream : list~event memory layout~

Block: uuid process_id
Block: uuid stream_id
Block: uuid block_id
Block: time begin_timestamp
Block: time end_timestamp

HeterogenousQueue~Events~: vector~byte~ events
```
## low overhead instrumentation

## fast & compact transmission

## scalable ingestion

### ingestion
```mermaid
graph LR;
    rust-->ingestion-srv;
    unreal-->ingestion-srv;
    ingestion-srv-->datalake[(Data Lake)];
    datalake-->postgresql[("`**PostgreSQL**
    processes
    streams
    blocks`")]
    datalake-->S3[("S3
    payloads")]
```

### analytics
```mermaid
graph RL;
    cli[python cli]-->analytics-srv;
    grafana-->analytics-srv;
    analytics-srv-->etl[JIT ETL];
    etl-->datalake[(Datalake)];
    etl-->lakehouse[(Lakehouse)];
    lakehouse-->postgres[("`**PostgreSQL**
    tables
    partitions`")]
    lakehouse-->s3[("`**S3**
    parquet files`")]
    analytics-srv-->datafusion[Datafusion SQL engine];
    datafusion-->lakehouse
```

## just-in-time extract-transform-load
