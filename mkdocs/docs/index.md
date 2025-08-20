# Micromegas Documentation

Welcome to Micromegas, a unified observability platform designed for high-performance telemetry collection and analytics.

[**→ Source Code on GitHub**](https://github.com/madesroches/micromegas){ .md-button .md-button--primary }
[**→ Get Started**](getting-started.md){ .md-button }

## What is Micromegas?

Micromegas is a comprehensive observability solution that provides:

- **High-performance instrumentation** with 20ns overhead per event
- **Unified data collection** for logs, metrics, spans, and traces
- **Cost-efficient storage** using object storage for raw data
- **Powerful SQL analytics** built on Apache DataFusion
- **Real-time and historical analysis** capabilities

## Key Features

### 🚀 High Performance
- Ultra-low overhead telemetry collection (20ns per event)
- Supports up to 100k events/second per process
- Thread-local storage for minimal performance impact

### 💰 Cost Effective
- Raw data stored in cheap object storage (S3/GCS)
- Metadata in PostgreSQL for fast queries
- Pay only for what you query with on-demand ETL

### 🔍 Unified Observability
- Logs, metrics, traces, and spans in a single queryable format
- SQL interface compatible with existing analytics tools
- Grafana plugin for visualization and dashboards

### 🏗️ Modern Architecture
- Apache Arrow FlightSQL for efficient data transfer
- DataFusion-powered analytics engine
- Lakehouse architecture based on Parquet (columnar format optimized for analytics workloads)

## Quick Start

Get started with Micromegas in just a few steps:

1. **[Getting Started Guide](getting-started.md)** - Set up your first Micromegas installation
2. **[Query Guide](query-guide/index.md)** - Learn how to query your observability data
3. **[Architecture Overview](architecture/index.md)** - Understand the system design

## Use Cases

### Application Performance Monitoring
Monitor your applications with detailed performance metrics, error tracking, and distributed tracing.

### Infrastructure Observability
Collect and analyze system metrics, logs, and performance data across your entire infrastructure.

### Cost-Effective Analytics
Store massive amounts of telemetry data cost-effectively while maintaining fast query performance.

### Development & Debugging
Use high-frequency instrumentation to debug performance issues and understand application behavior.

## Getting Help

- **Documentation**: Browse the guides in this documentation
- **GitHub Issues**: Report bugs or request features
- **Community**: Join discussions and get support

## License

Micromegas is open source software. See the [LICENSE](https://github.com/madesroches/micromegas/blob/main/LICENSE) file for details.
