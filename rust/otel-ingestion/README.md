# micromegas-otel-ingestion

OTLP/HTTP ingestion for the Micromegas data lake. It accepts OpenTelemetry logs, metrics and traces (`Export*ServiceRequest` messages) and writes them as Micromegas blocks, so they can be queried with SQL alongside native telemetry.

Point any OpenTelemetry SDK or Collector OTLP/HTTP exporter at a Micromegas ingestion server; this crate provides the translation layer used by that server.

## Documentation

- [OpenTelemetry (OTLP) guide](https://micromegas.info/docs/otlp/)
- [API documentation on docs.rs](https://docs.rs/micromegas-otel-ingestion)
- [Architecture Overview](https://micromegas.info/docs/architecture/)
- [GitHub Repository](https://github.com/madesroches/micromegas)
